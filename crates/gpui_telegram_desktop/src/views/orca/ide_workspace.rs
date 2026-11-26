//! Root GPUI `0.2.2` `Entity<OrcaIdeWorkspace>` unifying Split Anything panes,
//! top/bottom IDE bars, slide-over drawers (Kanban/Orchestration, Diff/Attribution/TasksHub,
//! 9-Provider Usage + Hot-Swap, AI Vault FTS5, 4-Tier Plugins V2 + MCP + Skills),
//! embedded `ScopedJumpPalette` (`Cmd+J`), and non-themable `SecuritySurfaceModal`.

use std::collections::BTreeSet;
use std::sync::Arc;

use gpui::prelude::FluentBuilder;
use gpui::*;
use orca_core::workspace_layout::{QuickSwitcherItemKind, TabContentType};
use orca_ide::{CreateWorktreeRequest, ORCA_LOCALHOST_SUFFIX};

use crate::components::ui::{delete_prev_word, pop_last_char, sanitize_paste_text};
use crate::i18n::tr_str;
use crate::theme::{IconName, TdesktopThemePalette, tg_icon};

use super::diff_tasks_panel::render_diff_tasks_drawer;
use super::ide_state::{IdeInputTarget, OrcaIdeAction, OrcaIdeDrawer, OrcaIdeSurfaceState};
use super::jump_palette::{JumpPaletteSelection, ScopedJumpPalette};
use super::orchestration_board::{open_popout_kanban_window, render_orchestration_board_drawer};
use super::runtime::{OrcaDesktopRuntime, OrcaRuntimeBindingMode, run_on_orca_tokio};
use super::security_surface::{SecuritySurfaceDecision, SecuritySurfaceModal};
use super::split_layout::render_split_layout;
use super::usage_vault_plugins::{
    render_ai_vault_drawer, render_plugins_drawer, render_usage_popover_drawer,
};

#[derive(Debug, Clone)]
enum IdeSecurityApprovalContext {
    PluginApproval {
        plugin_id: String,
        fingerprint: String,
    },
}

pub struct OrcaIdeWorkspace {
    pub runtime: Arc<OrcaDesktopRuntime>,
    pub state: OrcaIdeSurfaceState,
    pub palette: TdesktopThemePalette,
    pub language_id: String,
    focus_handle: FocusHandle,
    pub drawer_scroll: ScrollHandle,
    pending_actions: Vec<OrcaIdeAction>,
    jump_palette: Entity<ScopedJumpPalette>,
    jump_palette_open: bool,
    security_modal: Option<Entity<SecuritySurfaceModal>>,
    security_context: Option<IdeSecurityApprovalContext>,
}

impl OrcaIdeWorkspace {
    pub fn new(
        runtime: Arc<OrcaDesktopRuntime>,
        palette: TdesktopThemePalette,
        language_id: impl Into<String>,
        cx: &mut Context<Self>,
    ) -> Self {
        let lang = language_id.into();
        let mut state = OrcaIdeSurfaceState::default();
        let now_ms = chrono::Utc::now().timestamp_millis();
        if let Err(err) = state.refresh_all(&runtime, now_ms) {
            state.error_banner = Some(err);
        }
        // Seed initial AgentSession tab if workspace layout is empty so the IDE opens ready.
        if state.layout.tab_count() == 0 {
            let pane_id = state.layout.focused_pane_id.clone();
            let initial_tab = state.make_tab(TabContentType::AgentSession, now_ms);
            let _ = state.open_tab_and_sync(&runtime, &pane_id, initial_tab, now_ms);
        }

        let jp_runtime = runtime.clone();
        let jp_lang = lang.clone();
        let jump_palette =
            cx.new(|jcx| ScopedJumpPalette::new(jp_runtime, palette, jp_lang, jcx));

        cx.observe(
            &jump_palette,
            |this: &mut Self, jp: Entity<ScopedJumpPalette>, cx: &mut Context<Self>| {
                this.jump_palette_open = jp.read(cx).state().is_open;
                let selection = jp.update(cx, |p, _| p.take_selected_action());
                if let Some(sel) = selection {
                    this.apply_jump_palette_selection(sel, cx);
                }
            },
        )
        .detach();

        let warning_count = state.usage_roster.iter().filter(|u| u.warning_chip).count();
        let active_agents = state.agent_statuses.len();

        let wake_rt = runtime.clone();
        let mut wake_rx = state.subscribe_orchestration_wakes(&wake_rt);
        let tokio_handle = wake_rt.tokio_handle().clone();
        cx.spawn(async move |weak: WeakEntity<Self>, app: &mut AsyncApp| {
            loop {
                let next_res = run_on_orca_tokio(&tokio_handle, async move {
                    let recv = wake_rx.recv().await;
                    (wake_rx, recv)
                })
                .await;
                let Ok((rx_back, recv)) = next_res else {
                    break;
                };
                wake_rx = rx_back;
                match recv {
                    Ok(ev) => {
                        let rt_for_update = wake_rt.clone();
                        let updated = weak.update(app, |this, cx| {
                            this.state.record_wake_event(ev);
                            let _ = this.state.refresh_kanban_and_orchestration(&rt_for_update);
                            cx.notify();
                        });
                        if updated.is_err() {
                            break;
                        }
                    }
                    Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => continue,
                    Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
                }
            }
        })
        .detach();

        Self {
            runtime,
            state,
            palette,
            language_id: lang,
            focus_handle: cx.focus_handle(),
            drawer_scroll: ScrollHandle::new(),
            pending_actions: vec![OrcaIdeAction::StatusBarSummaryChanged {
                warning_count,
                active_agents,
            }],
            jump_palette,
            jump_palette_open: false,
            security_modal: None,
            security_context: None,
        }
    }

    pub fn state(&self) -> &OrcaIdeSurfaceState {
        &self.state
    }

    pub fn state_mut(&mut self) -> &mut OrcaIdeSurfaceState {
        &mut self.state
    }

    pub fn take_pending_actions(&mut self) -> Vec<OrcaIdeAction> {
        std::mem::take(&mut self.pending_actions)
    }

    pub(crate) fn push_action(&mut self, action: OrcaIdeAction) {
        self.pending_actions.push(action);
    }

    /// Synchronizes workspace theme palette and locale.
    /// NEVER propagates `TdesktopThemePalette` into `SecuritySurfaceModal` (locale only!).
    pub fn set_theme_and_locale(
        &mut self,
        palette: TdesktopThemePalette,
        language_id: impl Into<String>,
        cx: &mut Context<Self>,
    ) {
        self.palette = palette;
        let lang = language_id.into();
        self.language_id = lang.clone();
        self.jump_palette
            .update(cx, |jp, jcx| jp.set_theme_and_locale(palette, lang.clone(), jcx));
        if let Some(ref modal) = self.security_modal {
            modal.update(cx, |m, mcx| m.set_locale(lang, mcx));
        }
        cx.notify();
    }

    pub fn has_active_security_modal(&self) -> bool {
        self.security_modal.is_some()
    }

    pub fn has_active_text_input(&self) -> bool {
        self.state.active_input != IdeInputTarget::None || self.jump_palette_open
    }
}

impl OrcaIdeWorkspace {
    /// Flips the active dual-view tab between Chat and Terminal (`Cmd+Shift+J`).
    pub fn flip_active_chat_terminal(&mut self, cx: &mut Context<Self>) {
        let now_ms = chrono::Utc::now().timestamp_millis();
        let target_tab_id = self
            .state
            .active_tab()
            .filter(|t| t.content_type.traits().supports_dual_view)
            .map(|t| t.tab_id.clone())
            .or_else(|| {
                self.state
                    .all_open_tabs()
                    .into_iter()
                    .find(|(_, t, _)| t.content_type.traits().supports_dual_view)
                    .map(|(_, t, _)| t.tab_id)
            });

        let Some(tab_id) = target_tab_id else {
            let pane_id = self.state.layout.focused_pane_id.clone();
            let tab = self.state.make_tab(TabContentType::AgentSession, now_ms);
            let rt = self.runtime.clone();
            let _ = self.state.open_tab_and_sync(&rt, &pane_id, tab, now_ms);
            cx.notify();
            return;
        };

        let rt = self.runtime.clone();
        let sid = self.state.tab_session_bindings.get(&tab_id).cloned();
        match self
            .state
            .flip_chat_terminal(&rt, &tab_id, sid.as_deref(), now_ms)
        {
            Ok(mode) => {
                self.state.status_banner = Some(format!("Flipped tab '{tab_id}' to {mode:?}"));
            }
            Err(e) => {
                self.state.error_banner = Some(e);
            }
        }
        cx.notify();
    }

    pub fn open_jump_palette(&mut self, cx: &mut Context<Self>) {
        let host_id = Some(self.state.layout.host_id.clone());
        let repo_id = self.state.worktrees.first().map(|w| w.repo_root.clone());
        let layout_clone = self.state.layout.clone();
        self.jump_palette.update(cx, |jp, jcx| {
            jp.open_with_live_candidates(host_id, repo_id, Some(&layout_clone), jcx);
        });
        self.jump_palette_open = true;
        cx.notify();
    }

    pub fn toggle_jump_palette(&mut self, cx: &mut Context<Self>) {
        if self.jump_palette_open {
            self.jump_palette.update(cx, |jp, jcx| {
                jp.state_mut().close();
                jcx.notify();
            });
            self.jump_palette_open = false;
            cx.notify();
        } else {
            self.open_jump_palette(cx);
        }
    }

    pub(crate) fn open_popout_kanban_window(&mut self, cx: &mut Context<Self>) {
        let rt = self.runtime.clone();
        let pal = self.palette;
        let lang = self.language_id.clone();
        if let Err(err) = open_popout_kanban_window(rt, pal, lang, cx) {
            self.state.error_banner = Some(err);
        }
    }

    /// Routes Plugin V2 approval into a non-themable host-controlled `SecuritySurfaceModal`
    /// displaying the full SHA-256 `capability_fingerprint()`.
    pub fn open_plugin_approval_security_modal(
        &mut self,
        plugin_id: &str,
        cx: &mut Context<Self>,
    ) {
        let rt = self.runtime.clone();
        let modal_state = match self.state.request_plugin_approval_modal(&rt, plugin_id) {
            Ok(st) => st,
            Err(err) => {
                self.state.error_banner = Some(err);
                cx.notify();
                return;
            }
        };
        let fp = match &modal_state.kind {
            super::security_surface::SecurityModalKind::PluginApproval {
                capability_fingerprint,
                ..
            } => capability_fingerprint.clone(),
            _ => String::new(),
        };
        self.security_context = Some(IdeSecurityApprovalContext::PluginApproval {
            plugin_id: plugin_id.to_string(),
            fingerprint: fp,
        });
        let lang = self.language_id.clone();
        let modal = cx.new(|mcx| SecuritySurfaceModal::new(modal_state, lang, mcx));
        cx.observe(
            &modal,
            |this: &mut Self, ent: Entity<SecuritySurfaceModal>, cx: &mut Context<Self>| {
                let decision = ent.update(cx, |m, _| m.take_decision());
                if let Some(dec) = decision {
                    this.handle_security_modal_decision(dec, cx);
                }
            },
        )
        .detach();
        self.security_modal = Some(modal);
        cx.notify();
    }

    fn handle_security_modal_decision(
        &mut self,
        decision: SecuritySurfaceDecision,
        cx: &mut Context<Self>,
    ) {
        let ctx = self.security_context.take();
        self.security_modal = None;
        let now_ms = chrono::Utc::now().timestamp_millis();
        match (ctx, decision) {
            (
                Some(IdeSecurityApprovalContext::PluginApproval {
                    plugin_id,
                    fingerprint,
                }),
                SecuritySurfaceDecision::Approved,
            ) => {
                let rt = self.runtime.clone();
                match self
                    .state
                    .approve_plugin_with_fingerprint(&rt, &plugin_id, &fingerprint, now_ms)
                {
                    Ok(_) => {
                        self.state.status_banner = Some(format!(
                            "Approved plugin '{plugin_id}' via non-themable SecuritySurface"
                        ));
                    }
                    Err(e) => self.state.error_banner = Some(e),
                }
            }
            (
                Some(IdeSecurityApprovalContext::PluginApproval { plugin_id, .. }),
                SecuritySurfaceDecision::Denied,
            ) => {
                self.state.status_banner = Some(format!("Denied plugin approval for '{plugin_id}'"));
            }
            (None, _) => {}
        }
        cx.notify();
    }

    fn apply_jump_palette_selection(
        &mut self,
        selection: JumpPaletteSelection,
        cx: &mut Context<Self>,
    ) {
        let now_ms = chrono::Utc::now().timestamp_millis();
        self.jump_palette_open = false;
        match selection {
            JumpPaletteSelection::CreateWorktree { suggested_name } => {
                let wt_root = self.runtime.data_dir().join("worktrees").join(&suggested_name);
                let repo_root = self.runtime.data_dir().join("repo");
                match self.runtime.ide().worktrees().create_worktree(CreateWorktreeRequest {
                    id: None,
                    repo_root,
                    worktree_path: wt_root,
                    branch_name: Some(suggested_name.clone()),
                    title: Some(suggested_name.clone()),
                    parent_worktree_id: None,
                    project_group_id: None,
                    created_by_orca: true,
                    existing_branches: BTreeSet::new(),
                    seed_hint: Some(suggested_name.clone()),
                }) {
                    Ok(wt) => {
                        let rt = self.runtime.clone();
                        let _ = self.state.refresh_kanban_and_orchestration(&rt);
                        self.state.status_banner = Some(format!(
                            "Created worktree '{}' ({})",
                            wt.title, wt.branch_name
                        ));
                    }
                    Err(e) => self.state.error_banner = Some(e.to_string()),
                }
            }
            JumpPaletteSelection::Item(item) => match item.kind {
                QuickSwitcherItemKind::OpenTab => {
                    if let Some(tab_id) = item.id.strip_prefix("tab:") {
                        let _ = self.state.layout.focus_tab(tab_id, now_ms);
                        let rt = self.runtime.clone();
                        self.state.persist_layout_to_runtime(&rt);
                    }
                }
                QuickSwitcherItemKind::Bot => {
                    self.push_action(OrcaIdeAction::OpenBotsWorkspace);
                }
                QuickSwitcherItemKind::Command => match item.id.as_str() {
                    "cmd:kanban" => self.state.active_drawer = OrcaIdeDrawer::Kanban,
                    "cmd:flip" => self.flip_active_chat_terminal(cx),
                    "cmd:diff" => self.state.active_drawer = OrcaIdeDrawer::DiffTasks,
                    "cmd:usage" => self.state.active_drawer = OrcaIdeDrawer::UsagePopover,
                    "cmd:vault" => self.state.active_drawer = OrcaIdeDrawer::AiVault,
                    "cmd:plugins" => self.state.active_drawer = OrcaIdeDrawer::Plugins,
                    "cmd:design" => self.push_action(OrcaIdeAction::OpenBrowserDesignMode),
                    _ => {}
                },
                QuickSwitcherItemKind::PullRequest | QuickSwitcherItemKind::MergeRequest => {
                    self.state.active_drawer = OrcaIdeDrawer::DiffTasks;
                    self.state.diff_tab = super::ide_state::DiffTasksSubTab::TasksHub;
                }
                _ => {
                    self.state.status_banner = Some(format!("Jumped to {}", item.title));
                }
            },
        }
        cx.notify();
    }
}

impl OrcaIdeWorkspace {
    pub fn set_active_input(
        &mut self,
        target: IdeInputTarget,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.state.active_input = target;
        window.focus(&self.focus_handle);
        cx.notify();
    }

    fn active_input_string_mut(&mut self) -> Option<&mut String> {
        match self.state.active_input {
            IdeInputTarget::None => None,
            IdeInputTarget::SessionPromptDraft => Some(&mut self.state.session_prompt_draft),
            IdeInputTarget::NewRunTitle => Some(&mut self.state.new_run_title),
            IdeInputTarget::NewTaskTitle => Some(&mut self.state.new_task_title),
            IdeInputTarget::NewTaskDeps => Some(&mut self.state.new_task_deps),
            IdeInputTarget::MailboxRecipient => Some(&mut self.state.mailbox_target_input),
            IdeInputTarget::MailboxBody => Some(&mut self.state.mailbox_body_draft),
            IdeInputTarget::GatePrompt => Some(&mut self.state.gate_prompt_draft),
            IdeInputTarget::DiffFilePath => Some(&mut self.state.diff_file_path),
            IdeInputTarget::DiffLineStart => Some(&mut self.state.diff_line_start_draft),
            IdeInputTarget::DiffLineEnd => Some(&mut self.state.diff_line_end_draft),
            IdeInputTarget::DiffAnnotationBody => Some(&mut self.state.diff_annotation_body_draft),
            IdeInputTarget::CommitSummary => Some(&mut self.state.commit_summary),
            IdeInputTarget::CommitScope => Some(&mut self.state.commit_scope),
            IdeInputTarget::CommitParentPr => Some(&mut self.state.commit_parent_pr),
            IdeInputTarget::TaskHubSearch => Some(&mut self.state.tasks_hub_search),
            IdeInputTarget::TaskHubExternalKey => Some(&mut self.state.new_hub_task_key),
            IdeInputTarget::TaskHubTitle => Some(&mut self.state.new_hub_task_title),
            IdeInputTarget::TaskHubArtifactUrl => Some(&mut self.state.new_hub_task_url),
            IdeInputTarget::TaskHubCommandTemplate => Some(&mut self.state.issue_command_template),
            IdeInputTarget::ClaudeStatusLine => Some(&mut self.state.claude_status_line_draft),
            IdeInputTarget::HotSwapTargetAccount => Some(&mut self.state.hotswap_target_account),
            IdeInputTarget::VaultQuery => Some(&mut self.state.vault_query),
            IdeInputTarget::VaultHandoffTargetAgent => {
                Some(&mut self.state.vault_handoff_target_agent)
            }
            IdeInputTarget::PluginIdDraft => Some(&mut self.state.plugin_id_draft),
            IdeInputTarget::PluginPublisherDraft => Some(&mut self.state.plugin_publisher_draft),
            IdeInputTarget::PluginVersionDraft => Some(&mut self.state.plugin_version_draft),
            IdeInputTarget::McpServerName => Some(&mut self.state.mcp_server_name_draft),
            IdeInputTarget::McpServerCommand => Some(&mut self.state.mcp_server_cmd_draft),
            IdeInputTarget::McpTargetConfigPath => Some(&mut self.state.mcp_target_path_draft),
            IdeInputTarget::SkillNameDraft => Some(&mut self.state.skill_name_draft),
            IdeInputTarget::SkillSourceDirDraft => Some(&mut self.state.skill_source_dir_draft),
        }
    }

    pub fn insert_committed_text(&mut self, text: &str, cx: &mut Context<Self>) -> bool {
        if self.security_modal.is_some() {
            return true;
        }
        if self.jump_palette_open {
            return self
                .jump_palette
                .update(cx, |jp, jcx| jp.insert_committed_text(text, jcx));
        }
        let clean = sanitize_paste_text(text, false);
        if clean.is_empty() {
            return false;
        }
        if let Some(buf) = self.active_input_string_mut() {
            buf.push_str(&clean);
            cx.notify();
            return true;
        }
        false
    }

    pub fn handle_key_down(&mut self, ev: &KeyDownEvent, cx: &mut Context<Self>) -> bool {
        if let Some(ref modal) = self.security_modal {
            return modal.update(cx, |m, mcx| m.handle_key_down(ev, mcx));
        }
        if self.jump_palette_open {
            let handled = self
                .jump_palette
                .update(cx, |jp, jcx| jp.handle_key_down(ev, jcx));
            self.jump_palette_open = self.jump_palette.read(cx).state().is_open;
            if handled {
                cx.notify();
                return true;
            }
        }

        let key = ev.keystroke.key.as_str();
        let m = ev.keystroke.modifiers;
        let cmd = m.platform || m.control;

        if cmd && m.shift && !m.alt {
            if key.eq_ignore_ascii_case("j") {
                self.flip_active_chat_terminal(cx);
                return true;
            }
            if key.eq_ignore_ascii_case("t") {
                let now_ms = chrono::Utc::now().timestamp_millis();
                if let Ok(tab) = self.state.reopen_last_closed_tab(now_ms) {
                    let rt = self.runtime.clone();
                    self.state.persist_layout_to_runtime(&rt);
                    self.state.status_banner = Some(format!("Reopened tab '{}'", tab.title));
                }
                cx.notify();
                return true;
            }
            if key.eq_ignore_ascii_case("b") {
                self.state.active_drawer = if self.state.active_drawer == OrcaIdeDrawer::Kanban {
                    OrcaIdeDrawer::None
                } else {
                    OrcaIdeDrawer::Kanban
                };
                cx.notify();
                return true;
            }
        }
        if cmd && !m.shift && !m.alt && key.eq_ignore_ascii_case("j") {
            self.toggle_jump_palette(cx);
            return true;
        }

        if self.state.active_input != IdeInputTarget::None {
            match key {
                "escape" => {
                    self.state.active_input = IdeInputTarget::None;
                    cx.notify();
                    return true;
                }
                "backspace" => {
                    if let Some(buf) = self.active_input_string_mut() {
                        if m.alt {
                            delete_prev_word(buf);
                        } else {
                            pop_last_char(buf);
                        }
                        cx.notify();
                        return true;
                    }
                }
                _ => {
                    if !cmd
                        && !m.alt
                        && let Some(ref kc) = ev.keystroke.key_char
                        && !kc.chars().any(char::is_control)
                        && let Some(buf) = self.active_input_string_mut()
                    {
                        buf.push_str(kc);
                        cx.notify();
                        return true;
                    }
                }
            }
        }
        false
    }
}

impl Focusable for OrcaIdeWorkspace {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl Render for OrcaIdeWorkspace {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let pal = self.palette;
        let lang = self.language_id.as_str();
        let active_drawer = self.state.active_drawer;
        let tightest_warning = self.state.usage_roster.iter().find(|u| u.warning_chip);
        let active_acct_pill = self
            .state
            .active_accounts_by_provider
            .iter()
            .next()
            .map(|(p, a)| format!("{p}:{a}"))
            .unwrap_or_else(|| format!("{}:default", self.state.hotswap_provider));
        let ports_count = self.state.worktree_ports.len();
        let first_port_label = self
            .state
            .worktree_ports
            .first()
            .map(|p| format!("{}{}:{}", p.worktree_label, ORCA_LOCALHOST_SUFFIX, p.port))
            .unwrap_or_else(|| format!("*{ORCA_LOCALHOST_SUFFIX} (0)"));
        let hibernated_count = self.state.hibernated_tabs.len();
        let plugin_count = self.state.installed_plugins.len();
        let kb_count = self.state.keybindings.len();
        let mode_lbl = match self.runtime.binding_mode() {
            OrcaRuntimeBindingMode::LocalOnly => "Local",
            OrcaRuntimeBindingMode::RemoteSynced => "Synced",
        };

        let mut drawer_buttons = div().flex().flex_row().flex_wrap().gap(px(5.0));
        for (drawer, lbl) in [
            (OrcaIdeDrawer::Kanban, "Kanban"),
            (OrcaIdeDrawer::DiffTasks, "Diff & Tasks"),
            (OrcaIdeDrawer::UsagePopover, "Usage (9)"),
            (OrcaIdeDrawer::AiVault, "AI Vault"),
            (OrcaIdeDrawer::Plugins, "Plugins V2"),
        ] {
            let active = active_drawer == drawer;
            drawer_buttons = drawer_buttons.child(
                div()
                    .id(SharedString::from(format!("orca-ide-drawer-btn-{drawer:?}")))
                    .cursor_pointer()
                    .px(px(8.0))
                    .py(px(4.0))
                    .rounded(px(5.0))
                    .bg(if active {
                        rgb(pal.accent)
                    } else {
                        rgb(pal.input_bg)
                    })
                    .text_xs()
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(if active {
                        rgb(pal.active_button_fg)
                    } else {
                        rgb(pal.text_primary)
                    })
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |this, _ev, _w, cx| {
                            cx.stop_propagation();
                            this.state.active_drawer = if this.state.active_drawer == drawer {
                                OrcaIdeDrawer::None
                            } else {
                                drawer
                            };
                            let now_ms = chrono::Utc::now().timestamp_millis();
                            let rt = this.runtime.clone();
                            let _ = this.state.refresh_all(&rt, now_ms);
                            cx.notify();
                        }),
                    )
                    .child(lbl),
            );
        }

        let top_bar = div()
            .px(px(12.0))
            .py(px(8.0))
            .bg(rgb(pal.top_bar_bg))
            .border_b_1()
            .border_color(pal.separator_rgba())
            .flex()
            .flex_row()
            .flex_wrap()
            .items_center()
            .justify_between()
            .gap(px(8.0))
            .child(
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap(px(8.0))
                    .child(tg_icon(IconName::TerminalMcp, 17.0, pal.accent))
                    .child(
                        div()
                            .text_sm()
                            .font_weight(FontWeight::BOLD)
                            .text_color(rgb(pal.text_primary))
                            .child(tr_str(lang, "Orca 智能体 IDE", "Orca Agent IDE")),
                    )
                    .child(
                        div()
                            .id("orca-ide-cmd-j-btn")
                            .cursor_pointer()
                            .px(px(8.0))
                            .py(px(4.0))
                            .rounded(px(5.0))
                            .bg(rgb(pal.input_bg))
                            .border_1()
                            .border_color(pal.hairline_border_rgba())
                            .text_xs()
                            .text_color(rgb(pal.text_primary))
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(|this, _ev, _w, cx| {
                                    cx.stop_propagation();
                                    this.toggle_jump_palette(cx);
                                }),
                            )
                            .child("Jump (Cmd+J)"),
                    )
                    .child(
                        div()
                            .id("orca-ide-flip-btn")
                            .cursor_pointer()
                            .px(px(8.0))
                            .py(px(4.0))
                            .rounded(px(5.0))
                            .bg(rgb(pal.input_bg))
                            .text_xs()
                            .text_color(rgb(pal.accent))
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(|this, _ev, _w, cx| {
                                    cx.stop_propagation();
                                    this.flip_active_chat_terminal(cx);
                                }),
                            )
                            .child("Chat<->Term (Cmd+Shift+J)"),
                    ),
            )
            .child(drawer_buttons);

        let drawer_content = match active_drawer {
            OrcaIdeDrawer::None => None,
            OrcaIdeDrawer::Kanban => Some(render_orchestration_board_drawer(self, cx)),
            OrcaIdeDrawer::DiffTasks => Some(render_diff_tasks_drawer(self, cx)),
            OrcaIdeDrawer::UsagePopover => Some(render_usage_popover_drawer(self, cx)),
            OrcaIdeDrawer::AiVault => Some(render_ai_vault_drawer(self, cx)),
            OrcaIdeDrawer::Plugins => Some(render_plugins_drawer(self, cx)),
        };

        let status_bar = div()
            .px(px(12.0))
            .py(px(5.0))
            .bg(rgb(pal.top_bar_bg))
            .border_t_1()
            .border_color(pal.separator_rgba())
            .flex()
            .flex_row()
            .flex_wrap()
            .items_center()
            .justify_between()
            .gap(px(8.0))
            .child(
                div()
                    .flex()
                    .flex_row()
                    .flex_wrap()
                    .items_center()
                    .gap(px(8.0))
                    .when_some(tightest_warning, |el, warn| {
                        el.child(
                            div()
                                .id("orca-ide-status-usage-warn")
                                .cursor_pointer()
                                .px(px(7.0))
                                .py(px(2.0))
                                .rounded(px(4.0))
                                .bg(rgb(0xF59E0B))
                                .text_xs()
                                .font_weight(FontWeight::BOLD)
                                .text_color(rgb(0x111827))
                                .on_mouse_down(
                                    MouseButton::Left,
                                    cx.listener(|this, _ev, _w, cx| {
                                        cx.stop_propagation();
                                        this.state.active_drawer = OrcaIdeDrawer::UsagePopover;
                                        cx.notify();
                                    }),
                                )
                                .child(format!(
                                    "WARN {} {:.0}%",
                                    warn.provider, warn.percent_used
                                )),
                        )
                    })
                    .child(
                        div()
                            .id("orca-ide-status-account-pill")
                            .cursor_pointer()
                            .px(px(7.0))
                            .py(px(2.0))
                            .rounded(px(4.0))
                            .bg(rgb(pal.input_bg))
                            .text_xs()
                            .text_color(rgb(pal.text_primary))
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(|this, _ev, _w, cx| {
                                    cx.stop_propagation();
                                    this.state.active_drawer = OrcaIdeDrawer::UsagePopover;
                                    cx.notify();
                                }),
                            )
                            .child(format!("Acct: {active_acct_pill}")),
                    )
                    .child(
                        div()
                            .text_xs()
                            .text_color(rgb(pal.text_secondary))
                            .child(format!("Ports ({ports_count}): {first_port_label}")),
                    ),
            )
            .child(
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap(px(8.0))
                    .text_xs()
                    .text_color(rgb(pal.text_secondary))
                    .child(format!("Sleeping: {hibernated_count}"))
                    .child(format!(
                        "Plugins: {} ({})",
                        plugin_count,
                        if self.state.plugins_enabled { "ON" } else { "OFF" }
                    ))
                    .child(format!("Keys: {kb_count}"))
                    .child(format!("Mode: {mode_lbl}")),
            );

        let split_body = render_split_layout(self, cx);

        div()
            .id("orca-ide-workspace-root")
            .track_focus(&self.focus_handle)
            .relative()
            .flex_1()
            .min_w(px(0.0))
            .h_full()
            .overflow_hidden()
            .flex()
            .flex_col()
            .bg(rgb(pal.window_bg))
            .on_key_down(cx.listener(|this, ev: &KeyDownEvent, _w, cx| {
                if this.handle_key_down(ev, cx) {
                    cx.stop_propagation();
                }
            }))
            .child(top_bar)
            .when_some(self.state.error_banner.clone(), |el, err| {
                el.child(
                    div()
                        .px(px(12.0))
                        .py(px(5.0))
                        .bg(TdesktopThemePalette::rgba_with_alpha(pal.danger, 0.16))
                        .text_xs()
                        .text_color(rgb(pal.danger))
                        .child(err),
                )
            })
            .when_some(self.state.status_banner.clone(), |el, msg| {
                el.child(
                    div()
                        .px(px(12.0))
                        .py(px(4.0))
                        .bg(TdesktopThemePalette::rgba_with_alpha(pal.accent, 0.12))
                        .text_xs()
                        .text_color(rgb(pal.text_primary))
                        .child(msg),
                )
            })
            .child(
                div()
                    .flex_1()
                    .min_h(px(0.0))
                    .flex()
                    .flex_row()
                    .child(split_body)
                    .when_some(drawer_content, |row, content| {
                        row.child(
                            div()
                                .id("orca-ide-drawer-scroll")
                                .w(px(460.0))
                                .min_h(px(0.0))
                                .h_full()
                                .overflow_y_scroll()
                                .track_scroll(&self.drawer_scroll)
                                .bg(rgb(pal.dialogs_bg))
                                .border_l_1()
                                .border_color(pal.separator_rgba())
                                .p(px(12.0))
                                .child(content),
                        )
                    }),
            )
            .child(status_bar)
            .when(self.jump_palette_open, |el| el.child(self.jump_palette.clone()))
            .when_some(self.security_modal.clone(), |el, modal| el.child(modal))
            .into_any_element()
    }
}
