//! `AppShell` integration for native Orca desktop surfaces:
//! - Lazy `OrcaDesktopRuntime` construction (`OrcaBotsFacade` + `OrcaIdeFacade` + `OrcaClient`)
//! - `OrcaBotsWorkspace`, `OrcaIdeWorkspace`, top-level `ScopedJumpPalette` (`Cmd+J`),
//!   Chat<->Terminal flip (`Cmd+Shift+J`), and top-Z non-themable `SecuritySurfaceModal`
//! - `BrowserWorkspaceEvent::SendDesignCaptureToChat` routing into Orca Bots or active chat composer.

use std::collections::BTreeSet;
use std::sync::Arc;

use gpui::{App, AppContext, ClipboardItem, Context, Entity};
use gpui_telegram_client::OrcaClient;
use orca_core::workspace_layout::QuickSwitcherItemKind;
use orca_ide::CreateWorktreeRequest;

use crate::views::browser::{BrowserWorkspaceEvent, DesignHandoffTarget};
use crate::views::nav_rail::NavDestination;
use crate::views::orca::{
    open_popout_kanban_window, DiffTasksSubTab, JumpPaletteSelection, OrcaBotsAction,
    OrcaBotsWorkspace, OrcaDesktopRuntime, OrcaIdeAction, OrcaIdeDrawer, OrcaIdeWorkspace,
    ScopedJumpPalette, SecuritySurfaceDecision, SecuritySurfaceModal, SecuritySurfaceModalState,
};
use crate::views::shell::{ActiveWorkspacePanel, AppShell};

impl AppShell {
    /// Builds an authenticated (or unauthenticated fallback) `OrcaClient` sharing `self.client.http_client`.
    pub fn build_orca_client(&self) -> OrcaClient {
        let base = OrcaClient::new(self.client.http_client.clone());
        match self.session_sync.session_token.as_deref() {
            Some(tok) if !tok.is_empty() => base.with_token(tok),
            _ => base,
        }
    }

    /// Lazily initializes and caches the shared `OrcaDesktopRuntime` on `self.client.runtime()`.
    pub fn ensure_orca_runtime(&mut self) -> Arc<OrcaDesktopRuntime> {
        if let Some(ref rt) = self.orca_runtime {
            return Arc::clone(rt);
        }
        let orca_dir = match self.session_sync.user_id {
            Some(uid) if uid > 0 => self.preferences_dir.join(format!("orca_u{uid}")),
            _ => self.preferences_dir.join("orca"),
        };
        let tokio_handle = self.client.runtime().handle().clone();
        let remote = self
            .session_sync
            .session_token
            .as_deref()
            .filter(|t| !t.is_empty())
            .map(|_| self.build_orca_client());
        let runtime = OrcaDesktopRuntime::open_default(&orca_dir, remote, tokio_handle.clone())
            .or_else(|_| {
                let fallback = std::env::temp_dir()
                    .join(format!("fabushi-orca-runtime-{}", std::process::id()));
                OrcaDesktopRuntime::open_isolated(&fallback, tokio_handle)
            })
            .expect("open OrcaDesktopRuntime");
        let arc_rt = Arc::new(runtime);
        self.orca_runtime = Some(Arc::clone(&arc_rt));
        arc_rt
    }

    /// Returns `true` when either the top-level `SecuritySurfaceModal` or an active workspace's
    /// embedded `SecuritySurfaceModal` is open.
    pub fn has_active_orca_security_modal(&self, cx: &App) -> bool {
        self.orca_security_modal.is_some()
            || (self.active_panel == ActiveWorkspacePanel::OrcaBots
                && self
                    .orca_bots_workspace
                    .as_ref()
                    .is_some_and(|w| w.read(cx).has_active_security_modal()))
            || (self.active_panel == ActiveWorkspacePanel::OrcaIde
                && self
                    .orca_ide_workspace
                    .as_ref()
                    .is_some_and(|w| w.read(cx).has_active_security_modal()))
    }

    fn prepare_orca_panel_transition(
        &mut self,
        dest: NavDestination,
        panel: ActiveWorkspacePanel,
        cx: &mut Context<Self>,
    ) {
        self.leave_browser_panel_if_active(cx);
        self.leave_community_panel_if_active(cx);
        self.cleanup_settings_panel_if_active(cx);
        if self.active_panel != panel {
            self.clear_ime_and_blur_content_hub_for_transition(true, cx);
        }
        self.active_panel = panel;
        self.sync_nav_rail_destination_silent(dest, cx);
        self.sync_apple_sidebar_to_active_panel(cx);
        self.sidebar.update(cx, |sb, scx| {
            sb.cancel_compose_mode();
            sb.is_account_popover_open = false;
            sb.search_focused = false;
            scx.notify();
        });
        if self.composer.read(cx).is_compose_isolated
            && !self.is_compose_or_creation_modal_active(cx)
        {
            self.exit_compose_or_modal_isolation(true, cx);
        } else {
            self.sync_compose_state_to_composer(true, cx);
        }
        self.composer.update(cx, |c, ccx| {
            let mut changed = false;
            if c.is_input_focused {
                c.is_input_focused = false;
                changed = true;
            }
            if c.ime_machine.is_composing() {
                c.ime_machine.on_ime_cancel();
                c.sync_autocomplete();
                changed = true;
            }
            if changed {
                ccx.notify();
            }
        });
        self.stage.update(cx, |st, st_cx| {
            if st.more_menu_open || st.ttl_submenu_open {
                st.more_menu_open = false;
                st.ttl_submenu_open = false;
                st_cx.notify();
            }
        });
    }

    /// Updates the `NavDestination::OrcaBots` unread `"Needs You"` badge on `NavRail`.
    pub fn set_orca_bots_unread_badge(&mut self, count: u32, cx: &mut Context<Self>) {
        self.nav_rail.update(cx, |r, rcx| {
            r.set_unread(NavDestination::OrcaBots, count);
            rcx.notify();
        });
        self.sidebar.update(cx, |_, scx| scx.notify());
        cx.notify();
    }

    /// Opens or focuses the native GPUI `OrcaBotsWorkspace` main-pane panel.
    pub fn open_orca_bots_workspace(&mut self, cx: &mut Context<Self>) -> Entity<OrcaBotsWorkspace> {
        self.prepare_orca_panel_transition(
            NavDestination::OrcaBots,
            ActiveWorkspacePanel::OrcaBots,
            cx,
        );
        let pal = self.theme_state.palette;
        let lang = self.active_lang_id().to_string();

        let ob = if let Some(existing) = self.orca_bots_workspace.clone() {
            existing.update(cx, |w, wcx| {
                w.set_theme_and_locale(pal, lang.clone(), wcx);
            });
            existing
        } else {
            let rt = self.ensure_orca_runtime();
            let created = cx.new(|wcx| OrcaBotsWorkspace::new(rt, pal, lang, wcx));
            cx.observe(&created, |shell: &mut AppShell, ent, cx| {
                let actions = ent.update(cx, |w, _| w.take_pending_actions());
                for act in actions {
                    shell.handle_orca_bots_action(act, cx);
                }
            })
            .detach();
            let initial_actions = created.update(cx, |w, _| w.take_pending_actions());
            self.orca_bots_workspace = Some(created.clone());
            for act in initial_actions {
                self.handle_orca_bots_action(act, cx);
            }
            created
        };
        self.reconcile_chat_scoped_toasts(cx);
        cx.notify();
        ob
    }

    fn handle_orca_bots_action(&mut self, action: OrcaBotsAction, cx: &mut Context<Self>) {
        match action {
            OrcaBotsAction::OpenIdeWorkspace => {
                let _ = self.open_orca_ide_workspace(cx);
            }
            OrcaBotsAction::NeedsYouBadgeChanged(count) => {
                self.set_orca_bots_unread_badge(count as u32, cx);
            }
            OrcaBotsAction::ShowToast(msg) => {
                self.show_non_chat_toast(msg, 3200, cx);
            }
            OrcaBotsAction::OpenSecurityModal(modal_state) => {
                let _ = self.open_orca_security_modal(modal_state, None, cx);
            }
            OrcaBotsAction::CopyToClipboard(text) => {
                cx.write_to_clipboard(ClipboardItem::new_string(text));
            }
        }
    }
}

impl AppShell {
    /// Opens or focuses the native GPUI `OrcaIdeWorkspace` main-pane panel.
    pub fn open_orca_ide_workspace(&mut self, cx: &mut Context<Self>) -> Entity<OrcaIdeWorkspace> {
        self.prepare_orca_panel_transition(
            NavDestination::OrcaIde,
            ActiveWorkspacePanel::OrcaIde,
            cx,
        );
        let pal = self.theme_state.palette;
        let lang = self.active_lang_id().to_string();

        let oi = if let Some(existing) = self.orca_ide_workspace.clone() {
            existing.update(cx, |w, wcx| {
                w.set_theme_and_locale(pal, lang.clone(), wcx);
            });
            existing
        } else {
            let rt = self.ensure_orca_runtime();
            let created = cx.new(|wcx| OrcaIdeWorkspace::new(rt, pal, lang, wcx));
            cx.observe(&created, |shell: &mut AppShell, ent, cx| {
                let actions = ent.update(cx, |w, _| w.take_pending_actions());
                for act in actions {
                    shell.handle_orca_ide_action(act, cx);
                }
            })
            .detach();
            let initial_actions = created.update(cx, |w, _| w.take_pending_actions());
            self.orca_ide_workspace = Some(created.clone());
            for act in initial_actions {
                self.handle_orca_ide_action(act, cx);
            }
            created
        };
        self.reconcile_chat_scoped_toasts(cx);
        cx.notify();
        oi
    }

    fn handle_orca_ide_action(&mut self, action: OrcaIdeAction, cx: &mut Context<Self>) {
        match action {
            OrcaIdeAction::OpenBotsWorkspace => {
                let _ = self.open_orca_bots_workspace(cx);
            }
            OrcaIdeAction::OpenBrowserDesignMode => {
                let bw = self.open_browser_workspace(cx);
                bw.update(cx, |w, wcx| w.set_design_mode_enabled(true, wcx));
            }
            OrcaIdeAction::ShowToast(msg) => {
                self.show_non_chat_toast(msg, 3200, cx);
            }
            OrcaIdeAction::SendPromptToActiveChat(prompt) => {
                self.inject_prompt_into_active_chat_composer(prompt, None, cx);
            }
            OrcaIdeAction::OpenJumpPalette => {
                let _ = self.open_scoped_jump_palette(cx);
            }
            OrcaIdeAction::OpenPopoutKanban => {
                let rt = self.ensure_orca_runtime();
                let pal = self.theme_state.palette;
                let lang = self.active_lang_id().to_string();
                if let Err(err) = open_popout_kanban_window(rt, pal, lang, cx) {
                    self.show_non_chat_toast(err, 3200, cx);
                }
            }
            OrcaIdeAction::OpenSecurityModal(modal_state) => {
                let _ = self.open_orca_security_modal(modal_state, None, cx);
            }
            OrcaIdeAction::StatusBarSummaryChanged { warning_count, .. } => {
                self.nav_rail.update(cx, |r, rcx| {
                    r.set_unread(NavDestination::OrcaIde, warning_count as u32);
                    rcx.notify();
                });
                self.sidebar.update(cx, |_, scx| scx.notify());
            }
        }
    }

    /// Opens the top-level `ScopedJumpPalette` (`Cmd+J`) overlay hydrated with live runtime items.
    pub fn open_scoped_jump_palette(&mut self, cx: &mut Context<Self>) -> Entity<ScopedJumpPalette> {
        self.clear_ime_and_blur_content_hub_for_transition(true, cx);
        let rt = self.ensure_orca_runtime();
        let pal = self.theme_state.palette;
        let lang = self.active_lang_id().to_string();
        let (host_id, repo_id, active_layout) = self
            .orca_ide_workspace
            .as_ref()
            .map(|w| {
                let r = w.read(cx);
                let host = Some(r.state().layout.host_id.clone());
                let repo = r.state().worktrees.first().map(|wt| wt.repo_root.clone());
                (host, repo, Some(r.state().layout.clone()))
            })
            .unwrap_or((None, None, None));

        let created = cx.new(|jcx| {
            let mut palette = ScopedJumpPalette::new(rt, pal, lang, jcx);
            palette.open_with_live_candidates(host_id, repo_id, active_layout.as_ref(), jcx);
            palette
        });
        cx.observe(&created, |shell: &mut AppShell, jp, cx| {
            let selection = jp.update(cx, |p, _| p.take_selected_action());
            let is_open = jp.read(cx).state().is_open;
            if let Some(sel) = selection {
                shell.orca_jump_palette = None;
                shell.apply_shell_jump_palette_selection(sel, cx);
                cx.notify();
            } else if !is_open {
                shell.orca_jump_palette = None;
                cx.notify();
            }
        })
        .detach();
        self.orca_jump_palette = Some(created.clone());
        cx.notify();
        created
    }

    /// Toggles the top-level `ScopedJumpPalette` overlay open or closed.
    pub fn toggle_scoped_jump_palette(&mut self, cx: &mut Context<Self>) {
        if self.orca_jump_palette.is_some() {
            self.close_scoped_jump_palette(cx);
        } else {
            let _ = self.open_scoped_jump_palette(cx);
        }
    }

    /// Closes the top-level `ScopedJumpPalette` overlay if open.
    pub fn close_scoped_jump_palette(&mut self, cx: &mut Context<Self>) {
        if let Some(jp) = self.orca_jump_palette.take() {
            jp.update(cx, |p, pcx| {
                p.state_mut().close();
                pcx.notify();
            });
            self.clear_all_ime_preedits(cx);
            cx.notify();
        }
    }

    fn apply_shell_jump_palette_selection(
        &mut self,
        selection: JumpPaletteSelection,
        cx: &mut Context<Self>,
    ) {
        let now_ms = chrono::Utc::now().timestamp_millis();
        match selection {
            JumpPaletteSelection::CreateWorktree { suggested_name } => {
                let rt = self.ensure_orca_runtime();
                let wt_root = rt.data_dir().join("worktrees").join(&suggested_name);
                let repo_root = rt.data_dir().join("repo");
                match rt.ide().worktrees().create_worktree(CreateWorktreeRequest {
                    id: None,
                    repo_root,
                    worktree_path: wt_root,
                    branch_name: Some(suggested_name.clone()),
                    title: Some(suggested_name.clone()),
                    parent_worktree_id: None,
                    project_group_id: None,
                    created_by_orca: true,
                    existing_branches: BTreeSet::new(),
                    seed_hint: Some(suggested_name),
                }) {
                    Ok(wt) => {
                        let ide = self.open_orca_ide_workspace(cx);
                        ide.update(cx, |w, wcx| {
                            let rt_clone = w.runtime.clone();
                            let _ = w.state.refresh_kanban_and_orchestration(&rt_clone);
                            wcx.notify();
                        });
                        self.show_non_chat_toast(
                            format!("Created worktree '{}' ({})", wt.title, wt.branch_name),
                            3000,
                            cx,
                        );
                    }
                    Err(e) => self.show_non_chat_toast(e.to_string(), 3200, cx),
                }
            }
            JumpPaletteSelection::Item(item) => match item.kind {
                QuickSwitcherItemKind::Bot => {
                    let ob = self.open_orca_bots_workspace(cx);
                    if let Some(bot_id) = item.id.strip_prefix("bot:") {
                        ob.update(cx, |w, wcx| w.select_bot_by_id(bot_id, wcx));
                    }
                }
                QuickSwitcherItemKind::OpenTab => {
                    let ide = self.open_orca_ide_workspace(cx);
                    if let Some(tab_id) = item.id.strip_prefix("tab:") {
                        ide.update(cx, |w, wcx| {
                            let _ = w.state.layout.focus_tab(tab_id, now_ms);
                            let rt_clone = w.runtime.clone();
                            w.state.persist_layout_to_runtime(&rt_clone);
                            wcx.notify();
                        });
                    }
                }
                QuickSwitcherItemKind::Command => match item.id.as_str() {
                    "cmd:kanban" => {
                        let ide = self.open_orca_ide_workspace(cx);
                        ide.update(cx, |w, wcx| {
                            w.state.active_drawer = OrcaIdeDrawer::Kanban;
                            wcx.notify();
                        });
                    }
                    "cmd:flip" => self.flip_chat_terminal_session(cx),
                    "cmd:diff" => {
                        let ide = self.open_orca_ide_workspace(cx);
                        ide.update(cx, |w, wcx| {
                            w.state.active_drawer = OrcaIdeDrawer::DiffTasks;
                            wcx.notify();
                        });
                    }
                    "cmd:usage" => {
                        let ide = self.open_orca_ide_workspace(cx);
                        ide.update(cx, |w, wcx| {
                            w.state.active_drawer = OrcaIdeDrawer::UsagePopover;
                            wcx.notify();
                        });
                    }
                    "cmd:vault" => {
                        let ide = self.open_orca_ide_workspace(cx);
                        ide.update(cx, |w, wcx| {
                            w.state.active_drawer = OrcaIdeDrawer::AiVault;
                            wcx.notify();
                        });
                    }
                    "cmd:plugins" => {
                        let ide = self.open_orca_ide_workspace(cx);
                        ide.update(cx, |w, wcx| {
                            w.state.active_drawer = OrcaIdeDrawer::Plugins;
                            wcx.notify();
                        });
                    }
                    "cmd:design" => {
                        let bw = self.open_browser_workspace(cx);
                        bw.update(cx, |w, wcx| w.set_design_mode_enabled(true, wcx));
                    }
                    _ => {}
                },
                QuickSwitcherItemKind::PullRequest | QuickSwitcherItemKind::MergeRequest => {
                    let ide = self.open_orca_ide_workspace(cx);
                    ide.update(cx, |w, wcx| {
                        w.state.active_drawer = OrcaIdeDrawer::DiffTasks;
                        w.state.diff_tab = DiffTasksSubTab::TasksHub;
                        wcx.notify();
                    });
                }
                _ => {
                    let _ = self.open_orca_ide_workspace(cx);
                    self.show_non_chat_toast(format!("Jumped to {}", item.title), 2400, cx);
                }
            },
        }
    }
}

impl AppShell {
    /// Flips the active IDE dual-view session between Native Chat and Terminal PTY (`Cmd+Shift+J`).
    pub fn flip_chat_terminal_session(&mut self, cx: &mut Context<Self>) {
        let ide = self.open_orca_ide_workspace(cx);
        ide.update(cx, |w, wcx| {
            w.flip_active_chat_terminal(wcx);
        });
    }

    /// Mounts a top-Z, non-themable host-controlled [`SecuritySurfaceModal`] immediately below
    /// `is_passcode_locked`. Deliberately passes ONLY locale (`language_id`), NEVER any
    /// `TdesktopThemePalette` or plugin theme token.
    pub fn open_orca_security_modal(
        &mut self,
        modal_state: SecuritySurfaceModalState,
        on_decision: Option<Arc<dyn Fn(SecuritySurfaceDecision) + Send + Sync>>,
        cx: &mut Context<Self>,
    ) -> Entity<SecuritySurfaceModal> {
        self.clear_all_ime_preedits(cx);
        let lang = self.active_lang_id().to_string();
        let modal = cx.new(|mcx| SecuritySurfaceModal::new(modal_state, lang, mcx));
        cx.observe(&modal, move |shell: &mut AppShell, ent, cx| {
            let decision = ent.update(cx, |m, _| m.take_decision());
            if let Some(dec) = decision {
                shell.orca_security_modal = None;
                if let Some(ref cb) = on_decision {
                    cb(dec);
                }
                let msg = match dec {
                    SecuritySurfaceDecision::Approved => "Security surface approval confirmed",
                    SecuritySurfaceDecision::Denied => "Security surface request denied",
                };
                shell.show_non_chat_toast(msg, 2800, cx);
                cx.notify();
            }
        })
        .detach();
        self.orca_security_modal = Some(modal.clone());
        cx.notify();
        modal
    }

    /// Closes the top-level `SecuritySurfaceModal` (equivalent to denying/dismissing it).
    pub fn close_orca_security_modal(&mut self, cx: &mut Context<Self>) {
        if let Some(modal) = self.orca_security_modal.take() {
            modal.update(cx, |m, mcx| m.trigger_deny(mcx));
            cx.notify();
        }
    }

    fn inject_prompt_into_active_chat_composer(
        &mut self,
        prompt_markdown: String,
        cropped_png: Option<(Vec<u8>, Option<String>)>,
        cx: &mut Context<Self>,
    ) {
        let attachment_path = cropped_png.and_then(|(bytes, sha_opt)| {
            if bytes.is_empty() {
                return None;
            }
            let prefix = sha_opt
                .as_deref()
                .map(|s| {
                    s.chars()
                        .filter(|c| c.is_ascii_alphanumeric())
                        .take(12)
                        .collect::<String>()
                })
                .filter(|s| !s.is_empty())
                .unwrap_or_else(|| format!("{}", chrono::Utc::now().timestamp_millis()));
            let path = std::env::temp_dir().join(format!("fabushi-design-{prefix}.png"));
            std::fs::write(&path, &bytes).ok()?;
            Some(path)
        });

        self.composer.update(cx, |c, ccx| {
            if let Some(path) = attachment_path
                && !c.editor.attachments.contains(&path)
            {
                c.editor.attachments.push(path);
            }
            if c.text().trim().is_empty() {
                c.set_text(prompt_markdown);
            } else {
                c.ime_machine
                    .insert_text(&format!("\n\n{prompt_markdown}"));
            }
            c.sync_autocomplete();
            ccx.notify();
        });
        self.activate_nav_destination(NavDestination::Chats, cx);
        self.set_composer_focused(true, cx);
        self.show_toast("Design capture added to chat composer", 3000, cx);
    }

    /// Handles typed `BrowserWorkspaceEvent`s emitted by `BrowserWorkspace`, routing Design Mode
    /// captures directly into `OrcaBotsWorkspace` or the active chat `CanonicalComposer`.
    pub fn handle_browser_workspace_event(
        &mut self,
        ev: &BrowserWorkspaceEvent,
        cx: &mut Context<Self>,
    ) {
        match ev {
            BrowserWorkspaceEvent::SendDesignCaptureToChat { target, handoff } => match target {
                DesignHandoffTarget::BrowserCopilot => {
                    self.show_non_chat_toast(
                        "Design capture loaded into Browser Copilot",
                        2600,
                        cx,
                    );
                }
                DesignHandoffTarget::OrcaBot { bot_id } => {
                    let ob = self.open_orca_bots_workspace(cx);
                    ob.update(cx, |w, wcx| {
                        if let Some(bid) = bot_id {
                            w.select_bot_by_id(bid, wcx);
                        }
                        w.inject_design_mode_handoff(handoff.clone(), wcx);
                    });
                }
                DesignHandoffTarget::ActiveChat => {
                    let png = handoff
                        .cropped_png_bytes
                        .clone()
                        .map(|bytes| (bytes, handoff.screenshot_sha256.clone()));
                    self.inject_prompt_into_active_chat_composer(
                        handoff.prompt_markdown.clone(),
                        png,
                        cx,
                    );
                }
            },
        }
    }
}

impl AppShell {
    /// Synchronously drains any pending selection or close state from `self.orca_jump_palette`.
    pub(crate) fn drain_jump_palette_selection(&mut self, cx: &mut Context<Self>) {
        let Some(jp) = self.orca_jump_palette.clone() else {
            return;
        };
        let selection = jp.update(cx, |p, _| p.take_selected_action());
        let is_open = jp.read(cx).state().is_open;
        if let Some(sel) = selection {
            self.orca_jump_palette = None;
            self.apply_shell_jump_palette_selection(sel, cx);
            cx.notify();
        } else if !is_open {
            self.orca_jump_palette = None;
            cx.notify();
        }
    }

    /// Routes `Cmd+1..6` directly to the open `ScopedJumpPalette` before global nav digits.
    pub(crate) fn handle_jump_palette_shortcut_digit(
        &mut self,
        digit_1_to_6: u8,
        cx: &mut Context<Self>,
    ) -> bool {
        let Some(jp) = self.orca_jump_palette.clone() else {
            return false;
        };
        let handled = jp.update(cx, |p, pcx| p.handle_shortcut_digit(digit_1_to_6, pcx));
        if handled {
            self.drain_jump_palette_selection(cx);
        }
        handled
    }
}
