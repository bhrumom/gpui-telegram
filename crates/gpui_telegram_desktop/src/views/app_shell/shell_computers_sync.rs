//! Multi-device & cloud computer synchronization, local relay execution loop,
//! Bot-to-computer profile bindings, and remote workbench command dispatch.

use std::collections::BTreeMap;
use std::future::Future;
use std::sync::Arc;
use std::time::Duration;

use gpui::{App, AppContext, AsyncApp, ClipboardItem, Context, Entity, WeakEntity};
use gpui_telegram_client::TelegramClient;
use gpui_telegram_client::net::shared_remote_engine;
use gpui_telegram_proto::computer_contracts::{
    ComputerListResponse, ComputerNodeKind, PendingComputerCommandDto, RemoteComputerCommandRequest,
    RemoteComputerOp, UpdateBotComputerRequest,
};
use gpui_telegram_proto::remote_control_contracts::{
    DisplayInfoDto, RemoteControllerKind, RemoteSessionDto,
};

use crate::app::cli_installer::{
    detect_cli_install_status, install_fabushi_cli_one_click, startup_ensure_cli_linked_if_bundled,
};
use crate::i18n::tr_str;
use crate::platform::mac::macos_permissions_drag::{
    cached_permissions_snapshot, hide_drag_hud, request_permissions_refresh,
    should_defer_local_capture_for_permissions,
};
use crate::platform::mac::tray_mac::tray_action_ids;
use crate::views::app_shell::desktop_ipc_host::DesktopLocalIpcHandler;
use crate::views::app_shell::shell_actions::resolve_active_target_chat_id;
use crate::views::app_shell::shell_truth::ClientCallFence;
use crate::views::computer_workbench::{ComputerWorkbench, WorkbenchCommand, WorkbenchTab};
use crate::views::drawer::inspector_tabs::{InspectorAction, InspectorSubpage};
use crate::views::nav_rail::NavDestination;
use crate::views::remote_terminal_view::RemoteTerminalAction;
use crate::views::settings::sections::SettingsComputersEvent;
use crate::views::settings::settings_type::SettingsSectionType;
use crate::views::shell::{ActiveWorkspacePanel, AppShell};

async fn run_on_tokio<F, T>(client: &Arc<TelegramClient>, fut: F) -> Result<T, String>
where
    T: Send + 'static,
    F: Future<Output = T> + Send + 'static,
{
    client
        .runtime()
        .spawn(fut)
        .await
        .map_err(|e| format!("tokio task join error: {e}"))
}

#[derive(Clone)]
struct ComputerAuthFence {
    call: ClientCallFence,
    relay_generation: u64,
}

impl ComputerAuthFence {
    fn client(&self) -> &Arc<TelegramClient> {
        &self.call.client
    }

    fn token(&self) -> &str {
        &self.call.token
    }

    fn client_epoch(&self) -> u64 {
        self.call.client_epoch
    }

    async fn preflight_ui(&self, weak: &WeakEntity<AppShell>, app: &mut AsyncApp) -> bool {
        weak.update(app, |shell, _| shell.computer_fence_matches(self))
            .unwrap_or(false)
    }
}

impl AppShell {
    fn enqueue_computer_toast(
        &mut self,
        zh: &str,
        en: &str,
        duration_ms: u64,
        cx: &mut Context<Self>,
    ) {
        let lang = self.active_lang_id().to_string();
        self.show_non_chat_toast(tr_str(&lang, zh, en), duration_ms, cx);
    }

    fn computer_fence(&self) -> Option<ComputerAuthFence> {
        let call = self.capture_client_fence()?;
        Some(ComputerAuthFence {
            call,
            relay_generation: self.computers.relay_generation,
        })
    }

    fn computer_fence_matches(&self, fence: &ComputerAuthFence) -> bool {
        self.computers.relay_generation == fence.relay_generation && fence.call.is_valid(self)
    }

    /// Collects unique AI Bot `(bot_id, display_name)` pairs known to this workspace.
    pub fn known_bots_for_computers(&self, cx: &App) -> Vec<(u64, String)> {
        let mut by_id: BTreeMap<u64, String> = BTreeMap::new();
        for rc in self.session_sync.rich_chats_by_id.values() {
            if rc.is_bot {
                let id = rc
                    .peer_user_id
                    .unwrap_or_else(|| rc.chat_id.max(0) as u64);
                if id > 0 {
                    let name = rc
                        .peer_display_name
                        .as_deref()
                        .filter(|s| !s.trim().is_empty())
                        .unwrap_or(&rc.title)
                        .to_string();
                    by_id.entry(id).or_insert(name);
                }
            }
        }
        for app in self
            .miniapps
            .installed_miniapps
            .iter()
            .chain(self.miniapps.catalog_miniapps.iter())
        {
            if app.bot_user_id > 0 {
                let id = app.bot_user_id as u64;
                by_id
                    .entry(id)
                    .or_insert_with(|| format!("@{} ({})", app.bot_username, app.title));
            }
        }
        let sb = self.sidebar.read(cx);
        for cid in &sb.bot_ids {
            if let Some(chat) = sb.dialogs.iter().find(|c| c.id == *cid) {
                let resolved_id = self
                    .session_sync
                    .rich_chats_by_id
                    .get(cid)
                    .and_then(|rc| rc.peer_user_id)
                    .unwrap_or(*cid);
                if resolved_id > 0 {
                    by_id
                        .entry(resolved_id)
                        .or_insert_with(|| chat.peer_name.clone());
                }
            }
        }
        let insp = self.inspector.read(cx);
        if insp.profile.is_bot && insp.profile.bot_id > 0 && !insp.profile.name.is_empty() {
            by_id.insert(insp.profile.bot_id, insp.profile.name.clone());
        }
        by_id.into_iter().collect()
    }

    fn effective_inspector_bot_id(&self, cx: &App) -> Option<u64> {
        let insp = self.inspector.read(cx);
        if !insp.profile.is_bot {
            return None;
        }
        if insp.profile.bot_id > 0 {
            return Some(insp.profile.bot_id);
        }
        let selected_chat = self.sidebar.read(cx).active_chat_id?;
        Some(
            self.session_sync
                .rich_chats_by_id
                .get(&selected_chat)
                .and_then(|rc| rc.peer_user_id)
                .unwrap_or(selected_chat),
        )
    }

    /// Synchronizes cached computers and bot computer profiles into Inspector, Settings, and Workbench.
    pub fn sync_computers_to_views(&mut self, cx: &mut Context<Self>) {
        let nodes = self.computers.computers.clone();
        let tailnet_status = self.computers.tailnet_status.clone();
        let bots = self.known_bots_for_computers(cx);
        let active_bot_id = self.effective_inspector_bot_id(cx);
        let active_bot_profile = active_bot_id
            .and_then(|id| self.computers.bot_profiles.get(&id).cloned());
        let assist_cfg = self.computers.assist_config.clone();
        let active_sessions = self.combined_remote_sessions();
        self.computers.active_sessions = active_sessions.clone();
        let mcp_grants = self.computers.mcp_grants.clone();
        let mcp_url = self.computers.mcp_endpoint_url.clone().or_else(|| {
            Some(format!(
                "{}/mcp",
                self.client.config.server_url.trim_end_matches('/')
            ))
        });
        let cli_status = self.computers.cli_status.clone();

        self.inspector.update(cx, |insp, icx| {
            insp.sync_computers(nodes.clone());
            if let Some(id) = active_bot_id {
                if insp.profile.is_bot && insp.profile.bot_id == 0 {
                    insp.profile.bot_id = id;
                }
            }
            if insp.profile.is_bot {
                if let Some(prof) = active_bot_profile {
                    insp.sync_bot_computer_profile(Some(prof));
                }
            }
            icx.notify();
        });

        let account_label = {
            let nav_name = self.nav_rail.read(cx).account_name.trim().to_string();
            if !nav_name.is_empty() {
                nav_name
            } else {
                self.session_sync.username.clone().unwrap_or_default()
            }
        };
        let sr_computers_opt = self.settings_router.read(cx).settings_computers.clone();
        if let Some(sc_ent) = sr_computers_opt {
            sc_ent.update(cx, |sc, sc_cx| {
                sc.sync_account_label(account_label.clone());
                sc.sync_computers(nodes.clone());
                sc.sync_known_bots(bots, active_bot_id);
                sc.sync_tailnet_status(tailnet_status.clone());
                sc.sync_assist_config(assist_cfg.clone());
                sc.sync_active_sessions(active_sessions.clone());
                sc.sync_mcp_grants(mcp_url.clone(), mcp_grants.clone());
                sc.sync_cli_status(cli_status.clone());
                sc_cx.notify();
            });
        }

        if let Some(ref handler) = self.remote_host.ipc_handler {
            handler.set_local_node_id(self.computers.current_host_node_id.clone());
        }
        self.sync_remote_tray_status(cx);

        if let Some(ref wb_ent) = self.overlays.computer_overlay.clone() {
            let target_id = wb_ent.read(cx).node_id.clone();
            if let Some(tid) = target_id {
                let node_opt = self.computers.computers.iter().find(|n| n.node_id == tid).cloned();
                let peer_opt = tailnet_status.as_ref().and_then(|ts| {
                    ts.peers.iter().find(|p| {
                        p.node_id == tid
                            || node_opt
                                .as_ref()
                                .and_then(|n| n.device_id.as_deref())
                                .is_some_and(|did| !did.is_empty() && p.device_id == did)
                    })
                });
                if node_opt.is_some() || peer_opt.is_some() {
                    wb_ent.update(cx, |wb, wcx| {
                        if let Some(ref node_clone) = node_opt {
                            wb.refresh_target_node_status(node_clone);
                        }
                        wb.sync_tailnet_peer_state(peer_opt);
                        wcx.notify();
                    });
                }
            }
        }
    }

    pub fn combined_remote_sessions(&self) -> Vec<RemoteSessionDto> {
        let mut sessions = shared_remote_engine().hub.list_sessions();
        for s in &self.computers.active_sessions {
            if !sessions.iter().any(|x| x.session_id == s.session_id) {
                sessions.push(s.clone());
            }
        }
        sessions.sort_by_key(|s| s.started_at_ms);
        sessions
    }

    /// Initializes local IPC host (`LocalIpcServer` + `PtyMuxServer` + `RemoteControlHub`),
    /// first-run `fabushi` CLI symlink detection, and native system tray menu bar indicator.
    pub fn ensure_remote_host_runtime_bootstrapped(&mut self, cx: &mut Context<Self>) {
        if self.remote_host.services_bootstrapped {
            return;
        }
        self.remote_host.services_bootstrapped = true;

        if cfg!(test) || std::env::var_os("GPUI_OFFLINE_TEST").is_some() {
            return;
        }

        let cli_status = startup_ensure_cli_linked_if_bundled();
        self.computers.cli_status = Some(cli_status);

        let handler = Arc::new(DesktopLocalIpcHandler::new(self.client.clone()));
        handler.set_local_node_id(self.computers.current_host_node_id.clone());
        self.remote_host.ipc_handler = Some(handler.clone());
        let ipc_client = self.client.clone();
        cx.spawn(async move |weak: WeakEntity<Self>, app: &mut AsyncApp| {
            let h = handler.clone();
            let bind_res = run_on_tokio(&ipc_client, async move {
                fabushi_remote_engine::ipc::LocalIpcServer::bind_default(h)
                    .await
                    .map_err(|e| e.to_string())
            })
            .await
            .and_then(|inner| inner);
            let _ = weak.update(app, |shell, _| match bind_res {
                Ok(srv) => {
                    shell.remote_host.ipc_server = Some(srv);
                }
                Err(err) => {
                    eprintln!("[desktop-ipc] failed to bind desktop LocalIpcServer: {err}");
                }
            });
        })
        .detach();

        let _ = self.remote_host.tray_mac.create_icon();
        let _ = self.remote_host.tray_win.tray();
        self.sync_remote_tray_status(cx);
        self.spawn_remote_host_poll_loop(cx);
    }

    pub fn sync_remote_tray_status(&mut self, cx: &App) {
        let _ = cx;
        let lang = self.active_lang_id().to_string();
        let sessions = self.combined_remote_sessions();
        let count = sessions.len();
        let by_ai = sessions
            .iter()
            .any(|s| s.controller_kind == RemoteControllerKind::AiMcp);
        let online = self.is_authenticated;
        let allow_remote = self
            .computers
            .assist_config
            .as_ref()
            .map_or(true, |c| c.allow_assist);
        let assist_id = self
            .computers
            .assist_config
            .as_ref()
            .map(|c| c.assist_id.clone())
            .unwrap_or_default();
        let assist_code = self
            .computers
            .assist_config
            .as_ref()
            .and_then(|c| c.current_code.clone());

        self.remote_host.tray_mac.set_remote_control_status(
            online,
            count,
            by_ai,
            allow_remote,
            assist_id.clone(),
            assist_code.clone(),
            &lang,
        );
        self.remote_host.tray_win.set_remote_control_status(
            online,
            count,
            by_ai,
            allow_remote,
            assist_id,
            assist_code,
            &lang,
        );
    }

    pub fn disconnect_all_remote_control(&mut self, cx: &mut Context<Self>) {
        let local_count = shared_remote_engine().hub.disconnect_all();
        self.computers.active_sessions.clear();
        self.sync_computers_to_views(cx);

        let Some(fence) = self.computer_fence() else {
            self.enqueue_computer_toast(
                if local_count > 0 {
                    "已立即断开所有远程控制与 AI MCP 会话"
                } else {
                    "已触发全局远控抢占保护"
                },
                "Disconnected all remote control sessions",
                2200,
                cx,
            );
            cx.notify();
            return;
        };

        cx.spawn(async move |weak: WeakEntity<Self>, app: &mut AsyncApp| {
            if !fence.preflight_ui(&weak, app).await {
                return;
            }
            let c = fence.client().clone();
            let tok = fence.token().to_string();
            let epoch = fence.client_epoch();
            let _ = run_on_tokio(&c.clone(), async move {
                c.disconnect_all_remote_sessions_with_token(&tok, epoch)
                    .await
                    .ok()
            })
            .await;
            let _ = weak.update(app, |shell, cx| {
                if !shell.computer_fence_matches(&fence) {
                    return;
                }
                shell.enqueue_computer_toast(
                    if local_count > 0 {
                        "已立即断开所有远程控制与 AI MCP 会话"
                    } else {
                        "已触发全局远控抢占保护"
                    },
                    "Disconnected all remote control sessions",
                    2200,
                    cx,
                );
                shell.sync_computers_to_views(cx);
                cx.notify();
            });
        })
        .detach();
    }

    pub fn disconnect_remote_session(&mut self, session_id: String, cx: &mut Context<Self>) {
        shared_remote_engine().hub.disconnect_session(&session_id);
        self.computers
            .active_sessions
            .retain(|s| s.session_id != session_id);
        self.sync_computers_to_views(cx);

        if let Some(fence) = self.computer_fence() {
            cx.spawn(async move |weak: WeakEntity<Self>, app: &mut AsyncApp| {
                if !fence.preflight_ui(&weak, app).await {
                    return;
                }
                let c = fence.client().clone();
                let tok = fence.token().to_string();
                let epoch = fence.client_epoch();
                let sid = session_id.clone();
                let _ = run_on_tokio(&c.clone(), async move {
                    c.disconnect_remote_session_with_token(&tok, epoch, &sid)
                        .await
                        .ok()
                })
                .await;
                let _ = weak.update(app, |shell, cx| {
                    if !shell.computer_fence_matches(&fence) {
                        return;
                    }
                    shell.sync_computers_to_views(cx);
                    cx.notify();
                });
            })
            .detach();
        }
    }

    pub fn refresh_remote_assist_and_grants(&mut self, cx: &mut Context<Self>) {
        let status = detect_cli_install_status();
        self.computers.cli_status = Some(status);
        let Some(fence) = self.computer_fence() else {
            self.sync_computers_to_views(cx);
            return;
        };
        cx.spawn(async move |weak: WeakEntity<Self>, app: &mut AsyncApp| {
            if !fence.preflight_ui(&weak, app).await {
                return;
            }
            let c = fence.client().clone();
            let tok = fence.token().to_string();
            let epoch = fence.client_epoch();
            let (assist_res, sessions_res, grants_res) = run_on_tokio(&c.clone(), async move {
                let assist = c
                    .get_remote_assist_info_with_token(&tok, epoch)
                    .await
                    .map_err(|e| e.to_string());
                let sess = c
                    .list_remote_sessions_with_token(&tok, epoch)
                    .await
                    .map_err(|e| e.to_string());
                let grants = c
                    .list_mcp_grants_with_token(&tok, epoch)
                    .await
                    .map_err(|e| e.to_string());
                (assist, sess, grants)
            })
            .await
            .unwrap_or_else(|e| (Err(e.clone()), Err(e.clone()), Err(e)));

            let _ = weak.update(app, |shell, cx| {
                if !shell.computer_fence_matches(&fence) {
                    return;
                }
                if let Ok(cfg) = assist_res {
                    shell.computers.assist_config = Some(cfg);
                }
                if let Ok(s) = sessions_res {
                    shell.computers.active_sessions = s.sessions;
                }
                if let Ok(g) = grants_res {
                    shell.computers.mcp_grants = g.grants;
                }
                shell.sync_computers_to_views(cx);
                cx.notify();
            });
        })
        .detach();
    }

    pub fn handle_tray_action(&mut self, action_id: &str, cx: &mut Context<Self>) {
        match action_id {
            tray_action_ids::SHOW_FABUSHI | tray_action_ids::OPEN_ALIAS => {
                self.open_remote_computers_hub(cx);
                cx.activate(true);
            }
            tray_action_ids::DISCONNECT_ALL => {
                self.disconnect_all_remote_control(cx);
            }
            tray_action_ids::TOGGLE_ALLOW_REMOTE => {
                let prev = self
                    .computers
                    .assist_config
                    .as_ref()
                    .map_or(true, |c| c.allow_assist);
                let next = !prev;
                if let Some(cfg) = self.computers.assist_config.as_mut() {
                    cfg.allow_assist = next;
                }
                self.sync_computers_to_views(cx);
                if let Some(fence) = self.computer_fence() {
                    cx.spawn(async move |weak: WeakEntity<Self>, app: &mut AsyncApp| {
                        if !fence.preflight_ui(&weak, app).await {
                            return;
                        }
                        let c = fence.client().clone();
                        let tok = fence.token().to_string();
                        let epoch = fence.client_epoch();
                        let res = run_on_tokio(&c.clone(), async move {
                            c.set_remote_assist_allow_with_token(&tok, epoch, next)
                                .await
                                .map_err(|e| e.to_string())
                        })
                        .await
                        .and_then(|inner| inner);
                        let _ = weak.update(app, |shell, cx| {
                            if !shell.computer_fence_matches(&fence) {
                                return;
                            }
                            match res {
                                Ok(cfg) => {
                                    shell.computers.assist_config = Some(cfg);
                                    shell.sync_computers_to_views(cx);
                                }
                                Err(e) => {
                                    if let Some(cfg) = shell.computers.assist_config.as_mut() {
                                        if cfg.allow_assist == next {
                                            cfg.allow_assist = prev;
                                        }
                                    }
                                    shell.sync_computers_to_views(cx);
                                    let pfx = tr_str(
                                        shell.active_lang_id(),
                                        "切换允许远控状态失败",
                                        "Failed to update remote assist setting",
                                    );
                                    shell.show_non_chat_toast(format!("{pfx}: {e}"), 2400, cx);
                                }
                            }
                            cx.notify();
                        });
                    })
                    .detach();
                }
            }
            tray_action_ids::COPY_ASSIST => {
                if !self.remote_host.tray_mac.assist_id.trim().is_empty() {
                    let clip = self.remote_host.tray_mac.formatted_assist_clipboard_text();
                    cx.write_to_clipboard(ClipboardItem::new_string(clip));
                    self.enqueue_computer_toast(
                        "已复制设备协助码与验证码",
                        "Copied Assist ID & code",
                        2000,
                        cx,
                    );
                } else {
                    self.refresh_remote_assist_and_grants(cx);
                }
            }
            tray_action_ids::QUIT => {
                cx.quit();
            }
            _ => {}
        }
    }

    fn spawn_remote_host_poll_loop(&mut self, cx: &mut Context<Self>) {
        if self.remote_host.hub_events_started {
            return;
        }
        self.remote_host.hub_events_started = true;
        cx.spawn(async move |weak: WeakEntity<Self>, app: &mut AsyncApp| {
            let mut last_count = 0usize;
            loop {
                app.background_executor()
                    .timer(Duration::from_millis(350))
                    .await;
                let keep = weak
                    .update(app, |shell, cx| {
                        while let Some(act) = shell.remote_host.tray_mac.poll_native_action() {
                            shell.handle_tray_action(&act, cx);
                        }
                        while let Some(act) = shell.remote_host.tray_win.poll_native_action() {
                            shell.handle_tray_action(&act, cx);
                        }
                        let current_sessions = shell.combined_remote_sessions();
                        let count = current_sessions.len();
                        if count != last_count {
                            last_count = count;
                            shell.sync_remote_tray_status(cx);
                            shell.sync_computers_to_views(cx);
                            cx.notify();
                        }
                        true
                    })
                    .unwrap_or(false);
                if !keep {
                    break;
                }
            }
        })
        .detach();
    }

    fn apply_computer_list_response(&mut self, resp: ComputerListResponse) {
        self.computers.computers = resp.computers;
        if let Some(host_node) = self
            .computers
            .computers
            .iter()
            .find(|n| n.is_current_device)
        {
            self.computers.current_host_node_id = Some(host_node.node_id.clone());
        }
        self.computers.pending_approvals = resp.pending_approvals;
    }

    fn merge_pending_approvals(&mut self, incoming: Vec<PendingComputerCommandDto>) {
        for item in incoming {
            if item.requires_user_approval && !item.approved {
                if !self
                    .computers
                    .pending_approvals
                    .iter()
                    .any(|p| p.command_id == item.command_id)
                {
                    self.computers.pending_approvals.push(item);
                }
            }
        }
    }

    /// Registers the current local machine on login and starts the background local-command relay loop.
    pub fn register_current_host_and_start_relay(&mut self, cx: &mut Context<Self>) {
        self.ensure_remote_host_runtime_bootstrapped(cx);
        let Some(fence) = self.computer_fence() else {
            return;
        };
        if self.computers.relay_loop_started {
            return;
        }
        self.computers.relay_loop_started = true;
        self.client.bind_persistent_identity_dir(&self.preferences_dir);
        self.client.start_tailnet_mesh();
        cx.spawn(async move |weak: WeakEntity<Self>, app: &mut AsyncApp| {
            if !fence.preflight_ui(&weak, app).await {
                return;
            }
            let reg_client = fence.client().clone();
            let reg_token = fence.token().to_string();
            let reg_epoch = fence.client_epoch();
            let (reg_res, list_res, tailnet_st, tailnet_waiting, assist_res, sess_res, grants_res) =
                run_on_tokio(&reg_client.clone(), async move {
                    let reg = reg_client
                        .register_current_host_computer_with_token(&reg_token, reg_epoch)
                        .await;
                    let list = reg_client
                        .list_computers_with_token(&reg_token, reg_epoch)
                        .await;
                    let (ts, waiting) = if reg_client.is_same_session_epoch(reg_epoch) {
                        (
                            Some(reg_client.tailnet_status()),
                            reg_client.tailnet_pending_approvals(),
                        )
                    } else {
                        (None, Vec::new())
                    };
                    let assist = reg_client
                        .get_remote_assist_info_with_token(&reg_token, reg_epoch)
                        .await
                        .ok();
                    let sess = reg_client
                        .list_remote_sessions_with_token(&reg_token, reg_epoch)
                        .await
                        .ok();
                    let grants = reg_client
                        .list_mcp_grants_with_token(&reg_token, reg_epoch)
                        .await
                        .ok();
                    (reg, list, ts, waiting, assist, sess, grants)
                })
                .await
                .map(|(reg, list, ts, waiting, assist, sess, grants)| {
                    (
                        reg.map_err(|e| e.to_string()),
                        list.map_err(|e| e.to_string()),
                        ts,
                        waiting,
                        assist,
                        sess,
                        grants,
                    )
                })
                .unwrap_or_else(|e| {
                    (Err(e.clone()), Err(e), None, Vec::new(), None, None, None)
                });

            let keep = weak
                .update(app, |shell, cx| {
                    if !shell.computer_fence_matches(&fence) {
                        return false;
                    }
                    if let Ok(node) = reg_res {
                        shell.computers.current_host_node_id = Some(node.node_id);
                    }
                    if let Ok(list) = list_res {
                        shell.apply_computer_list_response(list);
                    }
                    if let Some(ts) = tailnet_st {
                        shell.computers.tailnet_status = Some(ts);
                    }
                    if let Some(cfg) = assist_res {
                        shell.computers.assist_config = Some(cfg);
                    }
                    if let Some(sr) = sess_res {
                        shell.computers.active_sessions = sr.sessions;
                    }
                    if let Some(gr) = grants_res {
                        shell.computers.mcp_grants = gr.grants;
                    }
                    shell.merge_pending_approvals(tailnet_waiting);
                    shell.sync_computers_to_views(cx);
                    cx.notify();
                    true
                })
                .unwrap_or(false);

            if !keep {
                return;
            }

            let mut tick_count: u64 = 0;
            loop {
                app.background_executor()
                    .timer(Duration::from_millis(2000))
                    .await;
                tick_count = tick_count.wrapping_add(1);

                if !fence.preflight_ui(&weak, app).await {
                    break;
                }

                let do_heartbeat = tick_count % 10 == 0;
                let poll_client = fence.client().clone();
                let poll_token = fence.token().to_string();
                let poll_epoch = fence.client_epoch();
                let (relay_res, list_res, tailnet_st, tailnet_waiting, sess_res) =
                    run_on_tokio(&poll_client.clone(), async move {
                        if do_heartbeat {
                            let _ = poll_client
                                .register_current_host_computer_with_token(&poll_token, poll_epoch)
                                .await;
                        }
                        let relay = poll_client
                            .process_ready_local_computer_commands_with_token(
                                &poll_token,
                                poll_epoch,
                            )
                            .await;
                        let list = poll_client
                            .list_computers_with_token(&poll_token, poll_epoch)
                            .await;
                        let (ts, waiting) = if poll_client.is_same_session_epoch(poll_epoch) {
                            (
                                Some(poll_client.tailnet_status()),
                                poll_client.tailnet_pending_approvals(),
                            )
                        } else {
                            (None, Vec::new())
                        };
                        let sess = if tick_count % 3 == 0 {
                            poll_client
                                .list_remote_sessions_with_token(&poll_token, poll_epoch)
                                .await
                                .ok()
                        } else {
                            None
                        };
                        (relay, list, ts, waiting, sess)
                    })
                    .await
                    .map(|(r, l, ts, waiting, sess)| {
                        (
                            r.map_err(|e| e.to_string()),
                            l.map_err(|e| e.to_string()),
                            ts,
                            waiting,
                            sess,
                        )
                    })
                    .unwrap_or_else(|e| (Err(e.clone()), Err(e), None, Vec::new(), None));

                let cont = weak
                    .update(app, |shell, cx| {
                        if !shell.computer_fence_matches(&fence) {
                            return false;
                        }
                        if let Ok(list) = list_res {
                            shell.apply_computer_list_response(list);
                        }
                        if let Ok(waiting) = relay_res {
                            shell.merge_pending_approvals(waiting);
                        }
                        if let Some(ts) = tailnet_st {
                            shell.computers.tailnet_status = Some(ts);
                        }
                        if let Some(sr) = sess_res {
                            shell.computers.active_sessions = sr.sessions;
                        }
                        shell.merge_pending_approvals(tailnet_waiting);
                        shell.sync_computers_to_views(cx);
                        cx.notify();
                        true
                    })
                    .unwrap_or(false);

                if !cont {
                    break;
                }
            }
        })
        .detach();
    }

    /// Refreshes the computer inventory and pending approvals off the UI thread.
    pub fn refresh_computers(&mut self, cx: &mut Context<Self>) {
        let Some(fence) = self.computer_fence() else {
            return;
        };
        cx.spawn(async move |weak: WeakEntity<Self>, app: &mut AsyncApp| {
            if !fence.preflight_ui(&weak, app).await {
                return;
            }
            let c = fence.client().clone();
            let tok = fence.token().to_string();
            let epoch = fence.client_epoch();
            let (res, ts, waiting) = run_on_tokio(&c.clone(), async move {
                let list = c
                    .list_computers_with_token(&tok, epoch)
                    .await
                    .map_err(|e| e.to_string());
                let (ts, waiting) = if c.is_same_session_epoch(epoch) {
                    (Some(c.tailnet_status()), c.tailnet_pending_approvals())
                } else {
                    (None, Vec::new())
                };
                (list, ts, waiting)
            })
            .await
            .unwrap_or_else(|e| (Err(e), None, Vec::new()));
            let _ = weak.update(app, |shell, cx| {
                if !shell.computer_fence_matches(&fence) {
                    return;
                }
                if let Some(ts) = ts {
                    shell.computers.tailnet_status = Some(ts);
                }
                let ok = res.is_ok();
                if let Ok(list) = res {
                    shell.apply_computer_list_response(list);
                }
                shell.merge_pending_approvals(waiting);
                if ok || shell.computers.tailnet_status.is_some() {
                    shell.sync_computers_to_views(cx);
                    cx.notify();
                }
            });
        })
        .detach();
    }

    /// Ensures the active Inspector AI Bot gets its computer inventory, `BotComputerProfileDto`,
    /// and live Settings AI model catalog.
    pub fn sync_active_bot_computer_to_inspector(&mut self, cx: &mut Context<Self>) {
        self.sync_computers_to_views(cx);
        self.sync_cached_ai_models_to_inspector(cx);
        let Some(bot_id) = self.effective_inspector_bot_id(cx) else {
            return;
        };
        let Some(fence) = self.computer_fence() else {
            return;
        };
        if self.computers.computers.is_empty() {
            self.refresh_computers(cx);
        }
        cx.spawn(async move |weak: WeakEntity<Self>, app: &mut AsyncApp| {
            if !fence.preflight_ui(&weak, app).await {
                return;
            }
            let c = fence.client().clone();
            let tok = fence.token().to_string();
            let epoch = fence.client_epoch();
            let res = run_on_tokio(&c.clone(), async move {
                c.get_bot_computer_profile_with_token(&tok, epoch, bot_id)
                    .await
                    .map_err(|e| e.to_string())
            })
            .await
            .and_then(|inner| inner);
            let _ = weak.update(app, |shell, cx| {
                if !shell.computer_fence_matches(&fence) {
                    return;
                }
                if let Ok(profile) = res {
                    shell.computers.bot_profiles.insert(bot_id, profile.clone());
                    if shell.effective_inspector_bot_id(cx) == Some(bot_id) {
                        shell.inspector.update(cx, |insp, icx| {
                            insp.sync_bot_computer_profile(Some(profile));
                            icx.notify();
                        });
                    }
                    cx.notify();
                }
            });
        })
        .detach();
    }

    /// Opens the interactive `ComputerWorkbench` overlay bound to `node_id` on the Desktop tab.
    pub fn open_remote_workbench_for_node(&mut self, node_id: String, cx: &mut Context<Self>) {
        self.open_remote_workbench_for_node_with_tab(node_id, WorkbenchTab::DesktopScreen, cx);
    }

    /// Opens the interactive `ComputerWorkbench` overlay bound to `node_id` on `initial_tab`.
    pub fn open_remote_workbench_for_node_with_tab(
        &mut self,
        node_id: String,
        initial_tab: WorkbenchTab,
        cx: &mut Context<Self>,
    ) {
        let pal = self.theme_state.palette;
        let lang = self.active_lang_id().to_string();
        let node_opt = self
            .computers
            .computers
            .iter()
            .find(|c| c.node_id == node_id)
            .cloned();
        let fallback_name = node_opt
            .as_ref()
            .map(|n| n.display_name.clone())
            .unwrap_or_else(|| node_id.clone());
        let peer_opt = self
            .computers
            .tailnet_status
            .as_ref()
            .and_then(|ts| {
                ts.peers.iter().find(|p| {
                    p.node_id == node_id
                        || node_opt
                            .as_ref()
                            .and_then(|n| n.device_id.as_deref())
                            .is_some_and(|did| !did.is_empty() && p.device_id == did)
                })
            })
            .cloned();

        let is_local_node = node_opt.as_ref().is_some_and(|n| n.is_current_device);
        let initial_local_perm_snap = if is_local_node && cfg!(target_os = "macos") {
            request_permissions_refresh(true);
            Some(cached_permissions_snapshot())
        } else {
            None
        };
        let defer_initial_capture = initial_tab == WorkbenchTab::DesktopScreen
            && initial_local_perm_snap.as_ref().is_some_and(|snap| {
                should_defer_local_capture_for_permissions(
                    is_local_node,
                    cfg!(target_os = "macos"),
                    snap.screen_state,
                )
            });

        let overlay = cx.new(|wcx| {
            let mut wb = ComputerWorkbench::new_live(fallback_name, pal, &lang);
            if let Some(ref node) = node_opt {
                wb.set_target_node(node);
            } else {
                wb.node_id = Some(node_id.clone());
            }
            if peer_opt.is_some() {
                wb.sync_tailnet_peer_state(peer_opt.as_ref());
            }
            wb.active_tab = initial_tab;
            if initial_tab == WorkbenchTab::RemoteTerminal {
                wb.terminal_focused = true;
                wb.ensure_terminal_view(wcx);
                if let Some(term) = wb.terminal_view.as_ref() {
                    term.update(wcx, |t, _| t.is_focused = true);
                }
            }
            if defer_initial_capture {
                if let Some(ref snap) = initial_local_perm_snap {
                    wb.needs_screen_permission_guide = true;
                    if !snap.accessibility_granted {
                        wb.needs_accessibility_permission_guide = true;
                    }
                    wb.status_banner = Some(if snap.screen_state.needs_restart() {
                        tr_str(
                            &wb.language_id,
                            "录屏权限已在系统设置中开启，请点击「重启 Fabushi」使其生效",
                            "Screen Recording enabled in Settings — click Restart Fabushi to apply",
                        )
                        .to_string()
                    } else {
                        tr_str(
                            &wb.language_id,
                            "未获取到录屏权限：请点击「在设置中开启」并将悬浮卡片中的 Fabushi 拖入系统设置列表",
                            "Screen Recording required: click Open in Settings and drag Fabushi into the list",
                        )
                        .to_string()
                    });
                }
            }
            wb.ensure_terminal_input(wcx);
            wb
        });

        let nid_for_obs = node_id.clone();
        cx.observe(&overlay, move |shell, ov, cx| {
            let (close, cmds) = ov.update(cx, |w, _| (w.should_close, w.take_commands()));
            if close {
                hide_drag_hud();
                shell.hide_remote_computer(cx);
                return;
            }
            for command in cmds {
                shell.handle_workbench_command(nid_for_obs.clone(), command, ov.clone(), cx);
            }
        })
        .detach();

        self.occlude_browser_for_overlay(cx);
        self.overlays.computer_overlay = Some(overlay.clone());
        self.reconcile_chat_scoped_toasts(cx);
        if initial_tab == WorkbenchTab::DesktopScreen && !defer_initial_capture {
            self.handle_workbench_command(node_id, WorkbenchCommand::RefreshScreen, overlay, cx);
        } else if initial_tab == WorkbenchTab::RemoteTerminal {
            let cwd = overlay.read(cx).remote_cwd.clone();
            self.handle_workbench_command(
                node_id,
                WorkbenchCommand::ListFiles { path: cwd },
                overlay,
                cx,
            );
        }
        cx.notify();
    }

    /// Executes an interactive `WorkbenchCommand` on `node_id` and feeds results back into `overlay`.
    pub fn handle_workbench_command(
        &mut self,
        node_id: String,
        cmd: WorkbenchCommand,
        overlay: Entity<ComputerWorkbench>,
        cx: &mut Context<Self>,
    ) {
        if let WorkbenchCommand::TerminalAction(RemoteTerminalAction::CopyToClipboard(text)) = &cmd
        {
            cx.write_to_clipboard(ClipboardItem::new_string(text.clone()));
            overlay.update(cx, |wb, wcx| {
                wb.status_banner = Some(
                    tr_str(&wb.language_id, "已复制终端选区到剪贴板", "Copied terminal selection")
                        .to_string(),
                );
                wcx.notify();
            });
            return;
        }
        if let WorkbenchCommand::FileTransferAction {
            transfer_id,
            action,
            ..
        } = &cmd
        {
            let banner = if action == "pause" {
                format!("Paused transfer {transfer_id}")
            } else {
                format!("Resumed transfer {transfer_id}")
            };
            overlay.update(cx, |wb, wcx| {
                wb.status_banner = Some(banner);
                wcx.notify();
            });
            return;
        }
        if let WorkbenchCommand::SetStreamSettings(settings) = &cmd {
            overlay.update(cx, |wb, wcx| {
                wb.status_banner = Some(format!(
                    "{} ({}fps · {:?})",
                    tr_str(&wb.language_id, "已应用画质与控制流设置", "Stream settings applied"),
                    settings.target_fps,
                    settings.quality
                ));
                wcx.notify();
            });
            return;
        }
        if let WorkbenchCommand::ToggleVirtualDisplay {
            enabled,
            width,
            height,
        } = &cmd
        {
            let dp = &shared_remote_engine().display_privacy;
            if *enabled {
                let _ = dp.create_virtual_display(*width, *height, 60);
            } else {
                for vd in dp.virtual_displays() {
                    dp.remove_virtual_display(vd.display_id);
                }
            }
            overlay.update(cx, |wb, wcx| {
                wb.status_banner = Some(if *enabled {
                    format!("Virtual display enabled ({width}x{height})")
                } else {
                    "Virtual display disabled".to_string()
                });
                wcx.notify();
            });
            return;
        }

        let Some(fence) = self.computer_fence() else {
            return;
        };
        let operation = match &cmd {
            WorkbenchCommand::RefreshScreen | WorkbenchCommand::TakeScreenshot => {
                RemoteComputerOp::CaptureScreen
            }
            WorkbenchCommand::SendGuiClick { x, y } => RemoteComputerOp::GuiInput {
                action: "click".to_string(),
                x: Some(*x),
                y: Some(*y),
                text: None,
                key: None,
            },
            WorkbenchCommand::SendGuiText { text } => RemoteComputerOp::GuiInput {
                action: "type".to_string(),
                x: None,
                y: None,
                text: Some(text.clone()),
                key: None,
            },
            WorkbenchCommand::RunShell { command, cwd } => RemoteComputerOp::ShellExec {
                command: command.clone(),
                cwd: cwd.clone(),
                timeout_ms: Some(15_000),
            },
            WorkbenchCommand::ListFiles { path } => RemoteComputerOp::ListDir { path: path.clone() },
            WorkbenchCommand::ReadRemoteFile { path } => RemoteComputerOp::ReadFile {
                path: path.clone(),
                max_bytes: Some(64 * 1024),
            },
            WorkbenchCommand::ListDisplays => RemoteComputerOp::DisplayList,
            WorkbenchCommand::SwitchDisplay { display_index } => RemoteComputerOp::DisplaySwitch {
                display_index: *display_index,
            },
            WorkbenchCommand::TogglePrivacyScreen { enabled, message } => {
                RemoteComputerOp::PrivacyScreenToggle {
                    enabled: *enabled,
                    message: message.clone(),
                }
            }
            WorkbenchCommand::SendRelativeMouseDelta { dx, dy } => RemoteComputerOp::MouseAction {
                action: "move".to_string(),
                x: None,
                y: None,
                dx: Some(*dx),
                dy: Some(*dy),
                button: None,
                clicks: None,
                scroll_x: None,
                scroll_y: None,
                relative: true,
            },
            WorkbenchCommand::SendHotkey { key, modifiers } => RemoteComputerOp::KeyboardAction {
                action: "hotkey".to_string(),
                text: None,
                key: Some(key.clone()),
                modifiers: modifiers.clone(),
            },
            WorkbenchCommand::PowerAction(action) => RemoteComputerOp::PowerAction {
                action: action.clone(),
            },
            WorkbenchCommand::TerminalAction(act) => match act {
                RemoteTerminalAction::NewSession {
                    name,
                    shell,
                    cols,
                    rows,
                } => RemoteComputerOp::TerminalOp {
                    op: "new".to_string(),
                    session_name: Some(name.clone()),
                    shell: shell.clone(),
                    input: None,
                    command: None,
                    cols: Some(*cols),
                    rows: Some(*rows),
                    timeout_ms: None,
                    new_name: None,
                },
                RemoteTerminalAction::RenameSession {
                    session_id,
                    new_name,
                } => RemoteComputerOp::TerminalOp {
                    op: "rename".to_string(),
                    session_name: Some(session_id.clone()),
                    shell: None,
                    input: None,
                    command: None,
                    cols: None,
                    rows: None,
                    timeout_ms: None,
                    new_name: Some(new_name.clone()),
                },
                RemoteTerminalAction::KillSession { session_id } => RemoteComputerOp::TerminalOp {
                    op: "kill".to_string(),
                    session_name: Some(session_id.clone()),
                    shell: None,
                    input: None,
                    command: None,
                    cols: None,
                    rows: None,
                    timeout_ms: None,
                    new_name: None,
                },
                RemoteTerminalAction::SendInput { session_id, data } => RemoteComputerOp::TerminalOp {
                    op: "write".to_string(),
                    session_name: Some(session_id.clone()),
                    shell: None,
                    input: Some(data.clone()),
                    command: None,
                    cols: None,
                    rows: None,
                    timeout_ms: None,
                    new_name: None,
                },
                RemoteTerminalAction::Resize {
                    session_id,
                    cols,
                    rows,
                } => RemoteComputerOp::TerminalOp {
                    op: "resize".to_string(),
                    session_name: Some(session_id.clone()),
                    shell: None,
                    input: None,
                    command: None,
                    cols: Some(*cols),
                    rows: Some(*rows),
                    timeout_ms: None,
                    new_name: None,
                },
                RemoteTerminalAction::RefreshSession { session_id } => {
                    RemoteComputerOp::TerminalOp {
                        op: "read".to_string(),
                        session_name: Some(session_id.clone()),
                        shell: None,
                        input: None,
                        command: None,
                        cols: None,
                        rows: None,
                        timeout_ms: None,
                        new_name: None,
                    }
                }
                RemoteTerminalAction::CopyToClipboard(_) => unreachable!(),
            },
            WorkbenchCommand::SetStreamSettings(_)
            | WorkbenchCommand::ToggleVirtualDisplay { .. }
            | WorkbenchCommand::FileTransferAction { .. } => unreachable!(),
        };

        let caller_label = self
            .session_sync
            .username
            .clone()
            .unwrap_or_else(|| "Desktop".to_string());
        let client = self.client.clone();
        let req = RemoteComputerCommandRequest {
            node_id: node_id.clone(),
            caller_device_id: Some(client.device_id()),
            caller_bot_id: None,
            caller_label,
            operation,
        };

        cx.spawn(async move |weak: WeakEntity<Self>, app: &mut AsyncApp| {
            if !fence.preflight_ui(&weak, app).await {
                return;
            }
            let c = fence.client().clone();
            let tok = fence.token().to_string();
            let epoch = fence.client_epoch();
            let res = run_on_tokio(&c.clone(), async move {
                c.execute_remote_computer_op_with_token(&tok, epoch, &req)
                    .await
                    .map_err(|e| e.to_string())
            })
            .await
            .and_then(|inner| inner);

            let _ = weak.update(app, |shell, cx| {
                if !shell.computer_fence_matches(&fence) {
                    return;
                }
                match res {
                    Ok(out) => {
                        let pending_hint = !out.ok
                            && (out.stderr.contains("approval")
                                || out.stderr.contains("pending")
                                || out.stderr.contains("审批"));
                        if let Some(ref frame) = out.frame_data_url {
                            if let Some(node) = shell
                                .computers
                                .computers
                                .iter_mut()
                                .find(|n| n.node_id == node_id)
                            {
                                node.latest_frame_data_url = Some(frame.clone());
                            }
                            shell.sync_computers_to_views(cx);
                        }
                        overlay.update(cx, |wb, wcx| {
                            match &cmd {
                                WorkbenchCommand::RefreshScreen | WorkbenchCommand::TakeScreenshot => {
                                    if out.stdout.contains("fallback_png") {
                                        wb.update_frame_url(None);
                                        let is_perm_denied = out
                                            .stdout
                                            .contains("fallback_png_permission_denied")
                                            || out.stderr.is_empty()
                                            || out.stderr.to_ascii_lowercase().contains("permission")
                                            || out.stderr.to_ascii_lowercase().contains("tcc")
                                            || out.stderr.contains("-3801")
                                            || out.stderr.contains("录屏");
                                        wb.remote_permission_error = if !out.stderr.is_empty() {
                                            Some(out.stderr.clone())
                                        } else {
                                            None
                                        };
                                        if is_perm_denied {
                                            wb.needs_screen_permission_guide = true;
                                            if wb.is_local_device {
                                                request_permissions_refresh(true);
                                                let snap = cached_permissions_snapshot();
                                                wb.status_banner = Some(if snap.screen_state.needs_restart() {
                                                    tr_str(
                                                        &wb.language_id,
                                                        "录屏权限已在系统设置中开启，请点击「重启 Fabushi」使其生效",
                                                        "Screen Recording enabled in Settings — click Restart Fabushi to apply",
                                                    )
                                                    .to_string()
                                                } else {
                                                    tr_str(
                                                        &wb.language_id,
                                                        "未获取到录屏权限：请点击「在设置中开启」并将悬浮卡片中的 Fabushi 拖入系统设置列表",
                                                        "Screen Recording required: click Open in Settings and drag Fabushi into the list",
                                                    )
                                                    .to_string()
                                                });
                                            } else {
                                                wb.status_banner = Some(
                                                    tr_str(
                                                        &wb.language_id,
                                                        "被控端未开启录屏权限（或开启后尚未重启 Fabushi），请在被控端完成授权",
                                                        "Remote host missing Screen Recording permission (or needs restart after enabling)",
                                                    )
                                                    .to_string(),
                                                );
                                            }
                                        } else {
                                            wb.status_banner = Some(format!(
                                                "{}: {}",
                                                tr_str(&wb.language_id, "抓取桌面画面失败", "Screen capture failed"),
                                                out.stderr
                                            ));
                                        }
                                    } else if out.frame_data_url.is_some() {
                                        wb.needs_screen_permission_guide = false;
                                        wb.remote_permission_error = None;
                                        wb.update_frame_url(out.frame_data_url.clone());
                                        wb.status_banner = Some(format!(
                                            "{} ({}ms)",
                                            if matches!(cmd, WorkbenchCommand::TakeScreenshot) {
                                                tr_str(&wb.language_id, "已抓取高清截屏", "Screenshot captured")
                                            } else {
                                                tr_str(&wb.language_id, "已更新实时画面", "Screen captured")
                                            },
                                            out.duration_ms
                                        ));
                                    } else if !out.stderr.is_empty() {
                                        wb.status_banner = Some(out.stderr.clone());
                                    }
                                }
                                WorkbenchCommand::SendGuiClick { x, y } => {
                                    if out.ok {
                                        wb.needs_accessibility_permission_guide = false;
                                        wb.remote_permission_error = None;
                                        wb.status_banner = Some(format!(
                                            "{} ({x}, {y})",
                                            tr_str(&wb.language_id, "远程点击已生效", "Click applied")
                                        ));
                                    } else {
                                        if out.stderr.contains("辅助功能")
                                            || out.stderr.to_ascii_lowercase().contains("accessibility")
                                            || out.stderr.to_ascii_lowercase().contains("permission")
                                        {
                                            wb.needs_accessibility_permission_guide = true;
                                            wb.remote_permission_error = Some(out.stderr.clone());
                                            if wb.is_local_device {
                                                request_permissions_refresh(true);
                                            }
                                        }
                                        wb.status_banner = Some(out.stderr.clone());
                                    }
                                }
                                WorkbenchCommand::SendGuiText { .. } => {
                                    if out.ok {
                                        wb.needs_accessibility_permission_guide = false;
                                        wb.status_banner = Some(
                                            tr_str(&wb.language_id, "文本已发送到远程桌面", "Text delivered")
                                                .to_string(),
                                        );
                                    } else {
                                        if out.stderr.contains("辅助功能")
                                            || out.stderr.to_ascii_lowercase().contains("accessibility")
                                            || out.stderr.to_ascii_lowercase().contains("permission")
                                        {
                                            wb.needs_accessibility_permission_guide = true;
                                            wb.remote_permission_error = Some(out.stderr.clone());
                                            if wb.is_local_device {
                                                request_permissions_refresh(true);
                                            }
                                        }
                                        wb.status_banner = Some(out.stderr.clone());
                                    }
                                }
                                WorkbenchCommand::RunShell { command, .. } => {
                                    wb.push_terminal_output(
                                        command.clone(),
                                        out.stdout.clone(),
                                        out.stderr.clone(),
                                        out.exit_code,
                                        None,
                                    );
                                    if pending_hint {
                                        wb.status_banner = Some(out.stderr.clone());
                                    }
                                }
                                WorkbenchCommand::ListFiles { path } => {
                                    wb.set_remote_files(path.clone(), out.entries.clone());
                                    if !out.ok && !out.stderr.is_empty() {
                                        wb.status_banner = Some(out.stderr.clone());
                                    }
                                }
                                WorkbenchCommand::ReadRemoteFile { path } => {
                                    if out.ok {
                                        wb.set_file_preview(path.clone(), out.stdout.clone());
                                    } else if !out.stderr.is_empty() {
                                        wb.status_banner = Some(out.stderr.clone());
                                    }
                                }
                                WorkbenchCommand::ListDisplays => {
                                    if let Ok(displays) =
                                        serde_json::from_str::<Vec<DisplayInfoDto>>(&out.stdout)
                                    {
                                        wb.set_displays(displays);
                                    }
                                }
                                WorkbenchCommand::SwitchDisplay { display_index } => {
                                    if out.ok {
                                        wb.status_banner = Some(format!(
                                            "{} #{display_index}",
                                            tr_str(&wb.language_id, "已切换显示器", "Switched to display")
                                        ));
                                    } else if !out.stderr.is_empty() {
                                        wb.status_banner = Some(out.stderr.clone());
                                    }
                                }
                                WorkbenchCommand::TogglePrivacyScreen { enabled, .. } => {
                                    wb.status_banner = Some(if out.ok {
                                        if *enabled {
                                            tr_str(&wb.language_id, "隐私屏已开启", "Privacy screen enabled")
                                        } else {
                                            tr_str(&wb.language_id, "隐私屏已关闭", "Privacy screen disabled")
                                        }
                                        .to_string()
                                    } else {
                                        out.stderr.clone()
                                    });
                                }
                                WorkbenchCommand::SendHotkey { key, modifiers } => {
                                    if !out.ok && !out.stderr.is_empty() {
                                        if out.stderr.contains("辅助功能")
                                            || out.stderr.to_ascii_lowercase().contains("accessibility")
                                            || out.stderr.to_ascii_lowercase().contains("permission")
                                        {
                                            wb.needs_accessibility_permission_guide = true;
                                            wb.remote_permission_error = Some(out.stderr.clone());
                                            if wb.is_local_device {
                                                request_permissions_refresh(true);
                                            }
                                        }
                                        wb.status_banner = Some(out.stderr.clone());
                                    } else {
                                        wb.needs_accessibility_permission_guide = false;
                                        wb.status_banner = Some(format!(
                                            "{} {:?}+{key}",
                                            tr_str(&wb.language_id, "已发送组合键", "Sent hotkey"),
                                            modifiers
                                        ));
                                    }
                                }
                                WorkbenchCommand::PowerAction(act) => {
                                    wb.status_banner = Some(if out.ok {
                                        format!(
                                            "{}: {:?}",
                                            tr_str(&wb.language_id, "电源指令已发送", "Power action sent"),
                                            act
                                        )
                                    } else {
                                        out.stderr.clone()
                                    });
                                }
                                WorkbenchCommand::TerminalAction(act) => match act {
                                    RemoteTerminalAction::NewSession { name, .. } => {
                                        if out.ok && !out.stdout.is_empty() {
                                            wb.feed_pty_output(Some(name), out.stdout.as_bytes(), wcx);
                                        } else if !out.ok && !out.stderr.is_empty() {
                                            wb.status_banner = Some(out.stderr.clone());
                                        }
                                    }
                                    RemoteTerminalAction::RefreshSession { session_id }
                                    | RemoteTerminalAction::SendInput { session_id, .. } => {
                                        if out.ok && !out.stdout.is_empty() {
                                            wb.feed_pty_output(
                                                Some(session_id),
                                                out.stdout.as_bytes(),
                                                wcx,
                                            );
                                        } else if !out.ok && !out.stderr.is_empty() {
                                            wb.status_banner = Some(out.stderr.clone());
                                        }
                                    }
                                    _ => {
                                        if !out.ok && !out.stderr.is_empty() {
                                            wb.status_banner = Some(out.stderr.clone());
                                        }
                                    }
                                },
                                WorkbenchCommand::SendRelativeMouseDelta { .. } => {
                                    if !out.ok
                                        && (out.stderr.contains("辅助功能")
                                            || out.stderr.to_ascii_lowercase().contains("accessibility")
                                            || out.stderr.to_ascii_lowercase().contains("permission"))
                                    {
                                        wb.needs_accessibility_permission_guide = true;
                                        wb.remote_permission_error = Some(out.stderr.clone());
                                        if wb.is_local_device {
                                            request_permissions_refresh(true);
                                        }
                                        wb.status_banner = Some(out.stderr.clone());
                                    }
                                }
                                WorkbenchCommand::SetStreamSettings(_)
                                | WorkbenchCommand::ToggleVirtualDisplay { .. }
                                | WorkbenchCommand::FileTransferAction { .. } => {}
                            }
                            wcx.notify();
                        });
                        if pending_hint {
                            shell.refresh_computers(cx);
                        }
                        if out.ok
                            && shell.overlays.computer_overlay.as_ref() == Some(&overlay)
                            && matches!(
                                cmd,
                                WorkbenchCommand::SendGuiClick { .. }
                                    | WorkbenchCommand::SendGuiText { .. }
                                    | WorkbenchCommand::SwitchDisplay { .. }
                                    | WorkbenchCommand::SendHotkey { .. }
                            )
                        {
                            let nid_refresh = node_id.clone();
                            let ov_refresh = overlay.clone();
                            let fence_refresh = fence.clone();
                            cx.spawn(async move |weak_shell: WeakEntity<Self>, app: &mut AsyncApp| {
                                app.background_executor()
                                    .timer(Duration::from_millis(260))
                                    .await;
                                let _ = weak_shell.update(app, |shell, cx| {
                                    if shell.computer_fence_matches(&fence_refresh)
                                        && shell.overlays.computer_overlay.as_ref()
                                            == Some(&ov_refresh)
                                    {
                                        shell.handle_workbench_command(
                                            nid_refresh,
                                            WorkbenchCommand::RefreshScreen,
                                            ov_refresh,
                                            cx,
                                        );
                                    }
                                });
                            })
                            .detach();
                        }
                    }
                    Err(e) => {
                        overlay.update(cx, |wb, wcx| {
                            wb.status_banner = Some(format!(
                                "{}: {e}",
                                tr_str(&wb.language_id, "远程操作失败", "Remote command failed")
                            ));
                            wcx.notify();
                        });
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// Handles computer-related actions emitted from `ContextInspector`. Returns `true` when consumed.
    pub fn handle_computer_inspector_action(
        &mut self,
        action: InspectorAction,
        cx: &mut Context<Self>,
    ) -> bool {
        let fallback_bot_id = self.effective_inspector_bot_id(cx);
        match action {
            InspectorAction::OpenBotComputersSubpage => {
                self.refresh_computers(cx);
                true
            }
            InspectorAction::CloseBotComputersSubpage => true,
            InspectorAction::OpenRemoteWorkbench { node_id } => {
                self.open_remote_workbench_for_node(node_id, cx);
                true
            }
            InspectorAction::OpenComputerDetailInSettings { node_id } => {
                self.open_computers_settings_page(Some(node_id), None, cx);
                true
            }
            InspectorAction::OpenCreateCloudComputerModal { bind_bot_id } => {
                let target_bot = bind_bot_id.or(fallback_bot_id);
                self.open_computers_settings_page(None, Some(target_bot), cx);
                true
            }
            InspectorAction::BindBotToComputer { bot_id, node_id } => {
                let resolved_bot = if bot_id > 0 {
                    Some(bot_id)
                } else {
                    fallback_bot_id
                };
                if let Some(bid) = resolved_bot {
                    let req = UpdateBotComputerRequest {
                        bound_node_id: node_id.clone(),
                        clear_bound_node: node_id.is_none(),
                        ..Default::default()
                    };
                    self.persist_bot_computer_profile(bid, req, false, cx);
                }
                true
            }
            InspectorAction::SaveBotComputerConfig {
                bot_id,
                persona_prompt,
                preferred_model,
                approval_mode,
                allow_cross_device_control,
                allow_cloud_self_config,
            } => {
                let resolved_bot = if bot_id > 0 {
                    Some(bot_id)
                } else {
                    fallback_bot_id
                };
                if let Some(bid) = resolved_bot {
                    let trimmed_model = preferred_model.trim().to_string();
                    let req = UpdateBotComputerRequest {
                        persona_prompt: Some(persona_prompt),
                        preferred_model: Some(trimmed_model),
                        approval_mode: Some(approval_mode),
                        allow_cross_device_control: Some(allow_cross_device_control),
                        allow_cloud_self_config: Some(allow_cloud_self_config),
                        ..Default::default()
                    };
                    self.persist_bot_computer_profile(bid, req, true, cx);
                }
                true
            }
            _ => false,
        }
    }

    fn persist_bot_computer_profile(
        &mut self,
        bot_id: u64,
        req: UpdateBotComputerRequest,
        return_to_main_subpage: bool,
        cx: &mut Context<Self>,
    ) {
        let Some(fence) = self.computer_fence() else {
            return;
        };
        let target_cid = resolve_active_target_chat_id(
            self.stage.read(cx).active_chat_id,
            self.sidebar.read(cx).active_chat_id,
        );
        let captured_panel = self.active_panel;
        let captured_plus_open = self.sidebar.read(cx).is_plus_menu_open;
        cx.spawn(async move |weak: WeakEntity<Self>, app: &mut AsyncApp| {
            if !fence.preflight_ui(&weak, app).await {
                return;
            }
            let c = fence.client().clone();
            let tok = fence.token().to_string();
            let epoch = fence.client_epoch();
            let res = run_on_tokio(&c.clone(), async move {
                c.update_bot_computer_profile_with_token(&tok, epoch, bot_id, &req)
                    .await
                    .map_err(|e| e.to_string())
            })
            .await
            .and_then(|inner| inner);
            let _ = weak.update(app, |shell, cx| {
                if !shell.computer_fence_matches(&fence) {
                    return;
                }
                let cur_stage_cid = shell.stage.read(cx).active_chat_id;
                let cur_active = resolve_active_target_chat_id(
                    cur_stage_cid,
                    shell.sidebar.read(cx).active_chat_id,
                );
                let still_in_target_context = match target_cid {
                    Some(cid) => cur_stage_cid == Some(cid),
                    None => {
                        shell.active_panel == captured_panel
                            && cur_active.is_none()
                            && shell.sidebar.read(cx).is_plus_menu_open == captured_plus_open
                    }
                };
                match res {
                    Ok(profile) => {
                        shell.computers.bot_profiles.insert(bot_id, profile.clone());
                        for fb in &mut shell.miniapps.fabushi_bots {
                            if fb.bot_user_id == bot_id as i64 {
                                fb.preferred_model = profile.preferred_model.clone();
                                if !profile.persona_prompt.is_empty() {
                                    fb.description = profile.persona_prompt.clone();
                                }
                            }
                        }
                        for cached in shell.miniapps.bot_profiles_cache.values_mut() {
                            if cached.bot.bot_user_id == bot_id as i64 {
                                cached.bot.preferred_model = profile.preferred_model.clone();
                                if !profile.persona_prompt.is_empty() {
                                    cached.bot.ai_persona = profile.persona_prompt.clone();
                                }
                            }
                        }
                        if still_in_target_context {
                            shell.inspector.update(cx, |insp, icx| {
                                if insp.profile.is_bot
                                    && (insp.profile.bot_id == 0 || insp.profile.bot_id == bot_id)
                                {
                                    if !profile.persona_prompt.is_empty() {
                                        insp.profile.bot_persona_prompt =
                                            profile.persona_prompt.clone();
                                    }
                                    if let Some(ref mut fab) = insp.active_fabushi_bot {
                                        if fab.bot_user_id == bot_id as i64 {
                                            fab.preferred_model = profile.preferred_model.clone();
                                            if !profile.persona_prompt.is_empty() {
                                                fab.description = profile.persona_prompt.clone();
                                            }
                                        }
                                    }
                                    insp.sync_bot_computer_profile(Some(profile));
                                    if return_to_main_subpage {
                                        insp.subpage = InspectorSubpage::Main;
                                    }
                                    icx.notify();
                                }
                            });
                        }
                        let msg = tr_str(
                            shell.active_lang_id(),
                            "Bot 电脑与运行配置已保存",
                            "Bot computer configuration saved",
                        );
                        if let Some(cid) = target_cid {
                            if cur_stage_cid == Some(cid) {
                                shell.show_chat_toast(cid, msg, 2000, cx);
                            }
                        } else if still_in_target_context {
                            shell.show_non_chat_toast(msg, 2000, cx);
                        }
                        shell.refresh_computers(cx);
                    }
                    Err(e) => {
                        let pfx = tr_str(
                            shell.active_lang_id(),
                            "保存 Bot 配置失败",
                            "Failed to save bot config",
                        );
                        let msg = format!("{pfx}: {e}");
                        if let Some(cid) = target_cid {
                            if cur_stage_cid == Some(cid) {
                                shell.show_chat_toast(cid, msg, 2600, cx);
                            }
                        } else if still_in_target_context {
                            shell.show_non_chat_toast(msg, 2600, cx);
                        }
                        cx.notify();
                    }
                }
            });
        })
        .detach();
    }

    /// Opens the standalone NetEase UU Remote-style Remote Computers hub (triggered from
    /// the computer icon beside the user avatar, main menu, tray, or `/computer`).
    pub fn open_remote_computers_hub(&mut self, cx: &mut Context<Self>) {
        self.leave_browser_panel_if_active(cx);
        self.rtc_talk.talk_mode_overlay = None;
        self.rtc_talk.active_talk_session_is_ai = false;
        self.overlays.media_viewer_overlay = None;
        self.overlays.iv_viewer_overlay = None;
        self.overlays.computer_overlay = None;
        self.main_menu_drawer.update(cx, |d, _| d.close());
        self.sidebar.update(cx, |sb, sb_cx| {
            sb.cancel_compose_mode();
            sb.is_account_popover_open = false;
            sb_cx.notify();
        });
        if self.composer.read(cx).is_compose_isolated
            && !self.is_compose_or_creation_modal_active(cx)
        {
            self.exit_compose_or_modal_isolation(true, cx);
        } else {
            self.sync_compose_state_to_composer(true, cx);
        }
        self.stage.update(cx, |st, st_cx| {
            st.more_menu_open = false;
            st.ttl_submenu_open = false;
            st_cx.notify();
        });
        if cfg!(target_os = "macos") {
            request_permissions_refresh(false);
        }
        self.settings_router.update(cx, |sr, scx| {
            sr.standalone_computers_mode = true;
            sr.navigate_to(SettingsSectionType::Computers, true, scx);
            let sc = sr.ensure_computers(scx);
            sc.update(scx, |view, vcx| {
                view.standalone_mode = true;
                view.open_overview();
                vcx.notify();
            });
            scx.notify();
        });
        self.active_panel = ActiveWorkspacePanel::Settings;
        self.reconcile_chat_scoped_toasts(cx);
        self.last_handled_nav_dest = Some(NavDestination::RemoteComputer);
        self.nav_rail
            .update(cx, |r, _| r.set_active_destination(NavDestination::RemoteComputer));
        self.sync_computers_to_views(cx);
        self.refresh_computers(cx);
        self.refresh_remote_assist_and_grants(cx);
        cx.notify();
    }

    fn open_computers_settings_page(
        &mut self,
        detail_node_id: Option<String>,
        create_cloud_for_bot: Option<Option<u64>>,
        cx: &mut Context<Self>,
    ) {
        self.sidebar.update(cx, |sb, sb_cx| {
            sb.cancel_compose_mode();
            sb.is_account_popover_open = false;
            sb_cx.notify();
        });
        if self.composer.read(cx).is_compose_isolated
            && !self.is_compose_or_creation_modal_active(cx)
        {
            self.exit_compose_or_modal_isolation(true, cx);
        } else {
            self.sync_compose_state_to_composer(true, cx);
        }
        self.stage.update(cx, |st, st_cx| {
            st.more_menu_open = false;
            st.ttl_submenu_open = false;
            st_cx.notify();
        });
        self.settings_router.update(cx, |sr, scx| {
            sr.standalone_computers_mode = false;
            sr.navigate_to(SettingsSectionType::Computers, true, scx);
            let sc = sr.ensure_computers(scx);
            sc.update(scx, |view, vcx| {
                view.standalone_mode = false;
                if let Some(nid) = detail_node_id {
                    view.open_detail(&nid);
                } else if let Some(bot_opt) = create_cloud_for_bot {
                    view.open_cloud_form(bot_opt, None);
                } else {
                    view.open_overview();
                }
                vcx.notify();
            });
            scx.notify();
        });
        self.leave_browser_panel_if_active(cx);
        self.active_panel = ActiveWorkspacePanel::Settings;
        self.reconcile_chat_scoped_toasts(cx);
        self.sync_computers_to_views(cx);
        self.refresh_computers(cx);
        self.refresh_remote_assist_and_grants(cx);
        cx.notify();
    }

    /// Handles events emitted by `SettingsComputersSections` via `SettingsRouter`.
    pub fn handle_computer_settings_event(
        &mut self,
        ev: &SettingsComputersEvent,
        cx: &mut Context<Self>,
    ) {
        match ev {
            SettingsComputersEvent::SelectNode(_) => {
                self.refresh_computers(cx);
            }
            SettingsComputersEvent::BackToOverview => {
                self.sync_computers_to_views(cx);
            }
            SettingsComputersEvent::OpenCreateCloudModal => {}
            SettingsComputersEvent::RefreshComputers => {
                self.refresh_computers(cx);
            }
            SettingsComputersEvent::OpenRemoteWorkbench(node_id) => {
                self.open_remote_workbench_for_node(node_id.clone(), cx);
            }
            SettingsComputersEvent::OpenRemoteTerminal(node_id) => {
                self.open_remote_workbench_for_node_with_tab(
                    node_id.clone(),
                    WorkbenchTab::RemoteTerminal,
                    cx,
                );
            }
            SettingsComputersEvent::CloseRequested | SettingsComputersEvent::BackToSettings => {}
            SettingsComputersEvent::SaveCloudServer(req) => {
                let Some(fence) = self.computer_fence() else {
                    return;
                };
                let req = req.clone();
                cx.spawn(async move |weak: WeakEntity<Self>, app: &mut AsyncApp| {
                    if !fence.preflight_ui(&weak, app).await {
                        return;
                    }
                    let c = fence.client().clone();
                    let tok = fence.token().to_string();
                    let epoch = fence.client_epoch();
                    let res = run_on_tokio(&c.clone(), async move {
                        c.upsert_cloud_computer_with_token(&tok, epoch, &req)
                            .await
                            .map_err(|e| e.to_string())
                    })
                    .await
                    .and_then(|inner| inner);
                    let _ = weak.update(app, |shell, cx| {
                        if !shell.computer_fence_matches(&fence) {
                            return;
                        }
                        match res {
                            Ok(node) => {
                                let detail_id = node.node_id.clone();
                                if let Some(sc_ent) =
                                    shell.settings_router.read(cx).settings_computers.clone()
                                {
                                    sc_ent.update(cx, |sc, sc_cx| {
                                        sc.status_message = Some(
                                            tr_str(
                                                &sc.language_id,
                                                "云服务器连接验证通过并已保存",
                                                "Cloud server verified & saved",
                                            )
                                            .to_string(),
                                        );
                                        sc.open_detail(&detail_id);
                                        sc_cx.notify();
                                    });
                                }
                                shell.enqueue_computer_toast(
                                    "云服务器已保存到云端",
                                    "Cloud server saved",
                                    2200,
                                    cx,
                                );
                                shell.refresh_computers(cx);
                            }
                            Err(e) => {
                                let err_msg = format!(
                                    "{}: {e}",
                                    tr_str(shell.active_lang_id(), "保存云服务器失败", "Failed to save cloud server")
                                );
                                if let Some(sc_ent) =
                                    shell.settings_router.read(cx).settings_computers.clone()
                                {
                                    sc_ent.update(cx, |sc, sc_cx| {
                                        sc.status_message = Some(err_msg.clone());
                                        sc_cx.notify();
                                    });
                                }
                                shell.show_non_chat_toast(err_msg, 2800, cx);
                                cx.notify();
                            }
                        }
                    });
                })
                .detach();
            }
            SettingsComputersEvent::UpdatePolicy { node_id, req } => {
                let Some(fence) = self.computer_fence() else {
                    return;
                };
                let nid = node_id.clone();
                let req = req.clone();
                cx.spawn(async move |weak: WeakEntity<Self>, app: &mut AsyncApp| {
                    if !fence.preflight_ui(&weak, app).await {
                        return;
                    }
                    let c = fence.client().clone();
                    let tok = fence.token().to_string();
                    let epoch = fence.client_epoch();
                    let res = run_on_tokio(&c.clone(), async move {
                        c.update_computer_policy_with_token(&tok, epoch, &nid, &req)
                            .await
                            .map_err(|e| e.to_string())
                    })
                    .await
                    .and_then(|inner| inner);
                    let _ = weak.update(app, |shell, cx| {
                        if !shell.computer_fence_matches(&fence) {
                            return;
                        }
                        match res {
                            Ok(updated) => {
                                if let Some(slot) = shell
                                    .computers
                                    .computers
                                    .iter_mut()
                                    .find(|n| n.node_id == updated.node_id)
                                {
                                    *slot = updated;
                                } else {
                                    shell.computers.computers.push(updated);
                                }
                                shell.sync_computers_to_views(cx);
                                shell.enqueue_computer_toast(
                                    "电脑权限与策略已更新",
                                    "Computer authorization policy updated",
                                    1800,
                                    cx,
                                );
                                cx.notify();
                            }
                            Err(e) => {
                                let pfx = tr_str(shell.active_lang_id(), "策略更新失败", "Policy update failed");
                                shell.show_non_chat_toast(format!("{pfx}: {e}"), 2400, cx);
                                cx.notify();
                            }
                        }
                    });
                })
                .detach();
            }
            SettingsComputersEvent::DeleteNode(node_id) => {
                let Some(fence) = self.computer_fence() else {
                    return;
                };
                let nid = node_id.clone();
                cx.spawn(async move |weak: WeakEntity<Self>, app: &mut AsyncApp| {
                    if !fence.preflight_ui(&weak, app).await {
                        return;
                    }
                    let c = fence.client().clone();
                    let tok = fence.token().to_string();
                    let epoch = fence.client_epoch();
                    let nid_task = nid.clone();
                    let res = run_on_tokio(&c.clone(), async move {
                        c.delete_computer_with_token(&tok, epoch, &nid_task)
                            .await
                            .map_err(|e| e.to_string())
                    })
                    .await
                    .and_then(|inner| inner);
                    let _ = weak.update(app, |shell, cx| {
                        if !shell.computer_fence_matches(&fence) {
                            return;
                        }
                        let is_local = shell
                            .computers
                            .computers
                            .iter()
                            .find(|n| n.node_id == nid)
                            .is_some_and(|n| n.kind == ComputerNodeKind::LocalDevice);
                        match res {
                            Ok(()) => {
                                shell.computers.computers.retain(|n| n.node_id != nid);
                                if let Some(sc_ent) =
                                    shell.settings_router.read(cx).settings_computers.clone()
                                {
                                    sc_ent.update(cx, |sc, sc_cx| {
                                        sc.open_overview();
                                        sc_cx.notify();
                                    });
                                }
                                shell.sync_computers_to_views(cx);
                                if is_local {
                                    shell.enqueue_computer_toast(
                                        "离线设备已移除",
                                        "Device removed",
                                        2000,
                                        cx,
                                    );
                                } else {
                                    shell.enqueue_computer_toast(
                                        "云服务器已删除",
                                        "Cloud server deleted",
                                        2000,
                                        cx,
                                    );
                                }
                                cx.notify();
                            }
                            Err(e) => {
                                let pfx = if is_local {
                                    tr_str(
                                        shell.active_lang_id(),
                                        "移除设备失败",
                                        "Failed to remove device",
                                    )
                                } else {
                                    tr_str(
                                        shell.active_lang_id(),
                                        "删除云服务器失败",
                                        "Failed to delete cloud server",
                                    )
                                };
                                shell.show_non_chat_toast(format!("{pfx}: {e}"), 2600, cx);
                                cx.notify();
                            }
                        }
                    });
                })
                .detach();
            }
            SettingsComputersEvent::TestNode(node_id) => {
                let Some(fence) = self.computer_fence() else {
                    return;
                };
                let nid = node_id.clone();
                let is_local = self
                    .computers
                    .computers
                    .iter()
                    .find(|n| n.node_id == nid)
                    .is_some_and(|n| n.kind == ComputerNodeKind::LocalDevice);
                let caller_label = self
                    .session_sync
                    .username
                    .clone()
                    .unwrap_or_else(|| "Desktop".to_string());
                cx.spawn(async move |weak: WeakEntity<Self>, app: &mut AsyncApp| {
                    if !fence.preflight_ui(&weak, app).await {
                        return;
                    }
                    let c = fence.client().clone();
                    let tok = fence.token().to_string();
                    let epoch = fence.client_epoch();
                    let res = run_on_tokio(&c.clone(), async move {
                        if is_local {
                            let req = RemoteComputerCommandRequest {
                                node_id: nid,
                                caller_device_id: Some(c.device_id()),
                                caller_bot_id: None,
                                caller_label,
                                operation: RemoteComputerOp::TestConnection,
                            };
                            c.execute_remote_computer_op_with_token(&tok, epoch, &req)
                                .await
                                .map_err(|e| e.to_string())
                        } else {
                            c.test_computer_connection_with_token(&tok, epoch, &nid)
                                .await
                                .map_err(|e| e.to_string())
                        }
                    })
                    .await
                    .and_then(|inner| inner);
                    let _ = weak.update(app, |shell, cx| {
                        if !shell.computer_fence_matches(&fence) {
                            return;
                        }
                        let msg = match res {
                            Ok(r) if r.ok => format!(
                                "{} ({}ms) {}",
                                tr_str(shell.active_lang_id(), "连接正常", "Connection OK"),
                                r.duration_ms,
                                r.stdout.trim()
                            ),
                            Ok(r) => format!(
                                "{}: {}",
                                tr_str(shell.active_lang_id(), "连接测试失败", "Connection failed"),
                                if r.stderr.is_empty() { "error" } else { r.stderr.trim() }
                            ),
                            Err(e) => format!(
                                "{}: {e}",
                                tr_str(shell.active_lang_id(), "连接测试异常", "Connection error")
                            ),
                        };
                        if let Some(sc_ent) =
                            shell.settings_router.read(cx).settings_computers.clone()
                        {
                            sc_ent.update(cx, |sc, sc_cx| {
                                sc.status_message = Some(msg.clone());
                                sc_cx.notify();
                            });
                        }
                        shell.show_non_chat_toast(msg, 2400, cx);
                        shell.refresh_computers(cx);
                    });
                })
                .detach();
            }
            SettingsComputersEvent::RefreshAssistInfo => {
                self.refresh_remote_assist_and_grants(cx);
            }
            SettingsComputersEvent::ResetAssistTempCode => {
                let Some(fence) = self.computer_fence() else { return; };
                cx.spawn(async move |weak: WeakEntity<Self>, app: &mut AsyncApp| {
                    if !fence.preflight_ui(&weak, app).await {
                        return;
                    }
                    let c = fence.client().clone();
                    let tok = fence.token().to_string();
                    let epoch = fence.client_epoch();
                    let res = run_on_tokio(&c.clone(), async move {
                        c.reset_remote_assist_code_with_token(&tok, epoch)
                            .await
                            .map_err(|e| e.to_string())
                    })
                    .await
                    .and_then(|x| x);
                    let _ = weak.update(app, |shell, cx| {
                        if !shell.computer_fence_matches(&fence) { return; }
                        match res {
                            Ok(cfg) => {
                                shell.computers.assist_config = Some(cfg);
                                shell.enqueue_computer_toast("已生成新临时验证码", "Temporary assist code rotated", 2000, cx);
                            }
                            Err(e) => {
                                shell.show_non_chat_toast(format!("Assist reset failed: {e}"), 2400, cx);
                            }
                        }
                        shell.sync_computers_to_views(cx);
                    });
                })
                .detach();
            }
            SettingsComputersEvent::SetAssistCustomCode(code) => {
                let Some(fence) = self.computer_fence() else { return; };
                let code = code.clone();
                cx.spawn(async move |weak: WeakEntity<Self>, app: &mut AsyncApp| {
                    if !fence.preflight_ui(&weak, app).await {
                        return;
                    }
                    let c = fence.client().clone();
                    let tok = fence.token().to_string();
                    let epoch = fence.client_epoch();
                    let res = run_on_tokio(&c.clone(), async move {
                        c.set_remote_assist_code_with_token(&tok, epoch, &code)
                            .await
                            .map_err(|e| e.to_string())
                    })
                    .await
                    .and_then(|x| x);
                    let _ = weak.update(app, |shell, cx| {
                        if !shell.computer_fence_matches(&fence) { return; }
                        match res {
                            Ok(cfg) => {
                                shell.computers.assist_config = Some(cfg);
                                shell.enqueue_computer_toast("已保存自定义协助验证码", "Custom assist code saved", 2000, cx);
                            }
                            Err(e) => {
                                shell.show_non_chat_toast(format!("Failed: {e}"), 2400, cx);
                            }
                        }
                        shell.sync_computers_to_views(cx);
                    });
                })
                .detach();
            }
            SettingsComputersEvent::SetAssistAllow(allow) => {
                let Some(fence) = self.computer_fence() else { return; };
                let allow = *allow;
                cx.spawn(async move |weak: WeakEntity<Self>, app: &mut AsyncApp| {
                    if !fence.preflight_ui(&weak, app).await {
                        return;
                    }
                    let c = fence.client().clone();
                    let tok = fence.token().to_string();
                    let epoch = fence.client_epoch();
                    let res = run_on_tokio(&c.clone(), async move {
                        c.set_remote_assist_allow_with_token(&tok, epoch, allow)
                            .await
                            .map_err(|e| e.to_string())
                    })
                    .await
                    .and_then(|x| x);
                    let _ = weak.update(app, |shell, cx| {
                        if !shell.computer_fence_matches(&fence) { return; }
                        match res {
                            Ok(cfg) => {
                                shell.computers.assist_config = Some(cfg);
                                shell.sync_computers_to_views(cx);
                            }
                            Err(e) => {
                                shell.sync_computers_to_views(cx);
                                let pfx = tr_str(
                                    shell.active_lang_id(),
                                    "切换允许远控状态失败",
                                    "Failed to update remote assist setting",
                                );
                                shell.show_non_chat_toast(format!("{pfx}: {e}"), 2400, cx);
                            }
                        }
                        cx.notify();
                    });
                })
                .detach();
            }
            SettingsComputersEvent::CopyAssistInvite { invite_text } => {
                cx.write_to_clipboard(ClipboardItem::new_string(invite_text.clone()));
                self.enqueue_computer_toast("已复制远程协助邀请", "Copied remote assist invite", 2000, cx);
            }
            SettingsComputersEvent::StartAssistConnect(req) => {
                let Some(fence) = self.computer_fence() else { return; };
                let mut req = req.clone();
                if req.requester_device_id.is_none() {
                    req.requester_device_id = Some(self.client.device_id());
                }
                if req.requester_platform.is_none() {
                    req.requester_platform = Some(std::env::consts::OS.to_string());
                }
                cx.spawn(async move |weak: WeakEntity<Self>, app: &mut AsyncApp| {
                    if !fence.preflight_ui(&weak, app).await {
                        return;
                    }
                    let c = fence.client().clone();
                    let tok = fence.token().to_string();
                    let epoch = fence.client_epoch();
                    let res = run_on_tokio(&c.clone(), async move {
                        c.start_remote_assist_with_token(&tok, epoch, &req)
                            .await
                            .map_err(|e| e.to_string())
                    })
                    .await
                    .and_then(|x| x);
                    let _ = weak.update(app, |shell, cx| {
                        if !shell.computer_fence_matches(&fence) { return; }
                        match res {
                            Ok(started) => {
                                shell.open_remote_workbench_for_node(started.target_node_id, cx);
                            }
                            Err(e) => {
                                if let Some(sc_ent) = shell.settings_router.read(cx).settings_computers.clone() {
                                    sc_ent.update(cx, |sc, sc_cx| {
                                        sc.status_message = Some(format!("Assist connect failed: {e}"));
                                        sc_cx.notify();
                                    });
                                }
                                shell.show_non_chat_toast(format!("Assist failed: {e}"), 2400, cx);
                            }
                        }
                    });
                })
                .detach();
            }
            SettingsComputersEvent::DisconnectAllRemoteControl => {
                self.disconnect_all_remote_control(cx);
            }
            SettingsComputersEvent::RefreshMcpGrants => {
                self.refresh_remote_assist_and_grants(cx);
            }
            SettingsComputersEvent::RevokeMcpGrant(family_id) => {
                let Some(fence) = self.computer_fence() else { return; };
                let fid = family_id.clone();
                cx.spawn(async move |weak: WeakEntity<Self>, app: &mut AsyncApp| {
                    if !fence.preflight_ui(&weak, app).await {
                        return;
                    }
                    let c = fence.client().clone();
                    let tok = fence.token().to_string();
                    let epoch = fence.client_epoch();
                    let fid_req = fid.clone();
                    let res = run_on_tokio(&c.clone(), async move {
                        c.revoke_mcp_grant_with_token(&tok, epoch, &fid_req)
                            .await
                            .map_err(|e| e.to_string())
                    })
                    .await
                    .and_then(|x| x);
                    let _ = weak.update(app, |shell, cx| {
                        if !shell.computer_fence_matches(&fence) { return; }
                        match res {
                            Ok(()) => {
                                shell.computers.mcp_grants.retain(|g| g.family_id != fid);
                                shell.enqueue_computer_toast("已吊销该 AI MCP 授权", "MCP OAuth grant revoked", 2000, cx);
                                shell.sync_computers_to_views(cx);
                            }
                            Err(e) => {
                                shell.show_non_chat_toast(format!("Revoke failed: {e}"), 2400, cx);
                            }
                        }
                    });
                })
                .detach();
            }
            SettingsComputersEvent::CopyMcpClientConfig {
                client_name,
                config_json,
            } => {
                cx.write_to_clipboard(ClipboardItem::new_string(config_json.clone()));
                self.show_non_chat_toast(
                    format!(
                        "{} {client_name}",
                        tr_str(self.active_lang_id(), "已复制 MCP 配置:", "Copied MCP config for")
                    ),
                    2200,
                    cx,
                );
            }
            SettingsComputersEvent::RefreshCliStatus => {
                self.computers.cli_status = Some(detect_cli_install_status());
                self.sync_computers_to_views(cx);
            }
            SettingsComputersEvent::InstallFabushiCli => {
                let msg = match install_fabushi_cli_one_click() {
                    Ok(status) => {
                        let m = status.status_message.clone();
                        self.computers.cli_status = Some(status);
                        m
                    }
                    Err(err) => err,
                };
                if let Some(sc_ent) = self.settings_router.read(cx).settings_computers.clone() {
                    sc_ent.update(cx, |sc, sc_cx| {
                        sc.status_message = Some(msg.clone());
                        sc_cx.notify();
                    });
                }
                self.show_non_chat_toast(msg, 2400, cx);
                self.sync_computers_to_views(cx);
            }
        }
    }

    /// Approves or denies a queued pending remote execution request for this device.
    pub fn decide_pending_computer_approval(
        &mut self,
        command_id: String,
        approved: bool,
        cx: &mut Context<Self>,
    ) {
        let Some(fence) = self.computer_fence() else {
            return;
        };
        self.computers
            .pending_approvals
            .retain(|p| p.command_id != command_id);
        cx.notify();

        cx.spawn(async move |weak: WeakEntity<Self>, app: &mut AsyncApp| {
            if !fence.preflight_ui(&weak, app).await {
                return;
            }
            let c = fence.client().clone();
            let tok = fence.token().to_string();
            let epoch = fence.client_epoch();
            let res = run_on_tokio(&c.clone(), async move {
                let is_p2p = command_id.starts_with("p2p:");
                let decision = if is_p2p {
                    match c.decide_tailnet_approval_with_token(&tok, epoch, &command_id, approved) {
                        Some(Ok(())) => Ok(()),
                        Some(Err(e)) => Err(e),
                        None => Err("Pending P2P approval expired or not found".to_string()),
                    }
                } else {
                    c.approve_computer_command_with_token(&tok, epoch, &command_id, approved)
                        .await
                        .map(|_| ())
                        .map_err(|e| e.to_string())
                };
                if !is_p2p && decision.is_ok() && approved {
                    let _ = c
                        .process_ready_local_computer_commands_with_token(&tok, epoch)
                        .await;
                }
                let list = c
                    .list_computers_with_token(&tok, epoch)
                    .await
                    .map_err(|e| e.to_string());
                let (ts, waiting) = if c.is_same_session_epoch(epoch) {
                    (Some(c.tailnet_status()), c.tailnet_pending_approvals())
                } else {
                    (None, Vec::new())
                };
                (decision, list, ts, waiting)
            })
            .await
            .unwrap_or_else(|e| (Err(e.clone()), Err(e), None, Vec::new()));

            let _ = weak.update(app, |shell, cx| {
                if !shell.computer_fence_matches(&fence) {
                    return;
                }
                match res.0 {
                    Ok(_) => {
                        if let Some(ts) = res.2 {
                            shell.computers.tailnet_status = Some(ts);
                        }
                        if let Ok(list) = res.1 {
                            shell.apply_computer_list_response(list);
                        }
                        shell.merge_pending_approvals(res.3);
                        shell.sync_computers_to_views(cx);
                        if approved {
                            shell.enqueue_computer_toast(
                                "已允许远程指令并立即执行",
                                "Approved remote command",
                                2000,
                                cx,
                            );
                        } else {
                            shell.enqueue_computer_toast(
                                "已拒绝远程执行请求",
                                "Denied remote command",
                                1800,
                                cx,
                            );
                        }
                        cx.notify();
                    }
                    Err(e) => {
                        let pfx = tr_str(shell.active_lang_id(), "审批操作失败", "Approval failed");
                        shell.show_non_chat_toast(format!("{pfx}: {e}"), 2400, cx);
                        shell.refresh_computers(cx);
                    }
                }
            });
        })
        .detach();
    }
}
