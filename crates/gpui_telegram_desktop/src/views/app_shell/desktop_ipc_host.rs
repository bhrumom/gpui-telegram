//! Desktop Local IPC Host (`LocalIpcServer` + `PtyMuxServer` + `RemoteControlHub`)
//! bridging `fabushi` CLI requests to local native engines and authenticated `TelegramClient` APIs.

use std::sync::Arc;

use base64::Engine as _;
use fabushi_remote_engine::ipc::{BoxFuture, LocalIpcHandler};
use fabushi_remote_engine::terminal::handle_terminal_ipc_method;
use gpui_telegram_client::TelegramClient;
use gpui_telegram_client::net::{LocalRemoteEngine, execute_local_computer_op, shared_remote_engine};
use gpui_telegram_proto::computer_contracts::{
    RemoteComputerCommandRequest, RemoteComputerOp, UpsertCloudComputerRequest,
};
use gpui_telegram_proto::remote_control_contracts::{
    AssistStartRequest, LocalIpcRequest, LocalIpcResponse, PowerActionKind, RemoteControlMode,
    RemoteControllerKind, RemoteSessionDto,
};
use gpui_telegram_proto::tailnet_contracts::TailnetRelayMode;

use crate::app::cli_installer::detect_cli_install_status;

/// Handles local JSON-RPC 2.0 requests from `fabushi` CLI and local automation clients.
pub struct DesktopLocalIpcHandler {
    client: parking_lot::RwLock<Arc<TelegramClient>>,
    pub engine: &'static LocalRemoteEngine,
    pub local_node_id: parking_lot::RwLock<Option<String>>,
}

impl DesktopLocalIpcHandler {
    pub fn new(client: Arc<TelegramClient>) -> Self {
        Self::with_engine(client, shared_remote_engine())
    }

    pub fn with_engine(client: Arc<TelegramClient>, engine: &'static LocalRemoteEngine) -> Self {
        Self {
            client: parking_lot::RwLock::new(client),
            engine,
            local_node_id: parking_lot::RwLock::new(None),
        }
    }

    pub fn client(&self) -> Arc<TelegramClient> {
        self.client.read().clone()
    }

    pub fn set_client(&self, client: Arc<TelegramClient>) {
        let mut guard = self.client.write();
        let changed = !Arc::ptr_eq(&*guard, &client);
        *guard = client;
        drop(guard);
        if changed {
            *self.local_node_id.write() = None;
        }
    }

    pub fn set_local_node_id(&self, node_id: Option<String>) {
        *self.local_node_id.write() = node_id;
    }

    fn require_token(&self, client: &TelegramClient, id: &str) -> Result<String, LocalIpcResponse> {
        client
            .token()
            .filter(|t| !t.trim().is_empty())
            .ok_or_else(|| LocalIpcResponse::err(id, "unauthenticated", "desktop session not authenticated"))
    }

    fn target_node_id(&self, params: &serde_json::Value) -> Option<String> {
        params
            .get("node_id")
            .and_then(|v| v.as_str())
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::to_string)
    }

    fn is_remote_target(&self, node_id: &str) -> bool {
        let local = self.local_node_id.read();
        !node_id.is_empty() && node_id != "local" && local.as_deref() != Some(node_id)
    }

    async fn exec_computer_op(
        &self,
        id: &str,
        params: &serde_json::Value,
        operation: RemoteComputerOp,
    ) -> LocalIpcResponse {
        let client = self.client();
        let requested_node = self.target_node_id(params);
        if let Some(nid) = requested_node.as_deref() {
            if self.is_remote_target(nid) {
                let (epoch, token) = match client.snapshot_pinned_session() {
                    Ok(p) => p,
                    Err(e) => return LocalIpcResponse::err(id, "unauthenticated", e.to_string()),
                };
                let req = RemoteComputerCommandRequest {
                    node_id: nid.to_string(),
                    caller_device_id: Some(client.device_id()),
                    caller_bot_id: None,
                    caller_label: "fabushi-cli-ipc".to_string(),
                    operation,
                };
                return match client
                    .execute_remote_computer_op_with_token(&token, epoch, &req)
                    .await
                {
                    Ok(res) => LocalIpcResponse::ok(id, serde_json::to_value(res).unwrap_or_default()),
                    Err(e) => LocalIpcResponse::err(id, "remote_exec_failed", e.to_string()),
                };
            }
        }

        let local_nid = self
            .local_node_id
            .read()
            .clone()
            .unwrap_or_else(|| format!("local:{}", client.device_id()));
        let cmd_id = format!("ipc-{}", uuid::Uuid::new_v4());
        let sess_id = format!("cli-{}-{}", client.device_id(), cmd_id);
        struct EphemeralSessionGuard<'a> {
            hub: &'a fabushi_remote_engine::control_lease::RemoteControlHub,
            session_id: String,
        }
        impl Drop for EphemeralSessionGuard<'_> {
            fn drop(&mut self) {
                self.hub.finish_ephemeral_session(&self.session_id);
            }
        }
        let registered = self.engine.hub.register_session(RemoteSessionDto {
            session_id: sess_id.clone(),
            controller_kind: RemoteControllerKind::Cli,
            controller_name: "fabushi CLI".to_string(),
            controller_platform: std::env::consts::OS.to_string(),
            controller_id: client.device_id(),
            target_node_id: local_nid.clone(),
            target_device_id: client.device_id(),
            mode: match &operation {
                RemoteComputerOp::TerminalOp { .. } | RemoteComputerOp::ShellExec { .. } => {
                    RemoteControlMode::Terminal
                }
                RemoteComputerOp::FileChunkOp { .. }
                | RemoteComputerOp::ListDir { .. }
                | RemoteComputerOp::ReadFile { .. }
                | RemoteComputerOp::WriteFile { .. } => RemoteControlMode::FileTransfer,
                _ => RemoteControlMode::Desktop,
            },
            started_at_ms: 0,
            last_activity_ms: 0,
            active_tool: Some("ipc".to_string()),
            relay_mode: TailnetRelayMode::DirectUdp,
            rtt_ms: Some(1),
            fps: None,
        });
        let _ephemeral_guard = EphemeralSessionGuard {
            hub: &self.engine.hub,
            session_id: registered.session_id,
        };
        let result = execute_local_computer_op(&local_nid, &cmd_id, &operation).await;
        LocalIpcResponse::ok(id, serde_json::to_value(result).unwrap_or_default())
    }

    async fn dispatch(&self, req: LocalIpcRequest) -> LocalIpcResponse {
        let client = self.client();
        let id = req.id.clone();
        let method = req.method.as_str();
        let p = &req.params;

        // Core health & echo
        match method {
            "status" => {
                let auth = client.current_auth_session();
                let cli_status = detect_cli_install_status();
                let sessions = self.engine.hub.list_sessions();
                let pty_sessions = self.engine.pty_mux.list_sessions();
                return LocalIpcResponse::ok(
                    id,
                    serde_json::json!({
                        "running": true,
                        "pid": std::process::id(),
                        "authenticated": client.token().is_some(),
                        "user_id": auth.as_ref().map(|a| a.user_id),
                        "username": auth.as_ref().map(|a| a.username.clone()),
                        "device_id": client.device_id(),
                        "local_node_id": *self.local_node_id.read(),
                        "active_sessions": sessions.len(),
                        "sessions": sessions,
                        "pty_sessions": pty_sessions.len(),
                        "override_epoch": self.engine.hub.override_epoch(),
                        "cli_installed": cli_status.installed,
                        "cli_on_path": cli_status.on_path,
                    }),
                );
            }
            "echo" => {
                return LocalIpcResponse::ok(id, p.clone());
            }
            _ => {}
        }

        // Local PTY mux (`lterm.*`) and remote-aware terminal mux (`term.*`)
        if method.starts_with("lterm.") || method.starts_with("term.") {
            let normalized_method = match method {
                "lterm.list" => "lterm.ls",
                "term.list" => "term.ls",
                "lterm.send" => "lterm.write",
                "term.send" => "term.write",
                "term.open" => "term.new",
                "term.exit" => "term.kill",
                other => other,
            };
            if method.starts_with("term.") {
                if let Some(nid) = self.target_node_id(p) {
                    if self.is_remote_target(&nid) {
                        let sub_op = normalized_method
                            .strip_prefix("term.")
                            .unwrap_or("ls")
                            .to_string();
                        let op = RemoteComputerOp::TerminalOp {
                            op: sub_op,
                            session_name: p
                                .get("name")
                                .or_else(|| p.get("session_name"))
                                .or_else(|| p.get("old_name"))
                                .and_then(|v| v.as_str())
                                .map(str::to_string),
                            shell: p.get("shell").and_then(|v| v.as_str()).map(str::to_string),
                            input: p.get("input").and_then(|v| v.as_str()).map(str::to_string),
                            command: p.get("command").and_then(|v| v.as_str()).map(str::to_string),
                            cols: p.get("cols").and_then(|v| v.as_u64()).map(|n| n as u16),
                            rows: p.get("rows").and_then(|v| v.as_u64()).map(|n| n as u16),
                            timeout_ms: p.get("timeout_ms").and_then(|v| v.as_u64()),
                            new_name: p.get("new_name").and_then(|v| v.as_str()).map(str::to_string),
                        };
                        return self.exec_computer_op(&id, p, op).await;
                    }
                }
            }

            let mut norm_req = req.clone();
            norm_req.method = normalized_method.to_string();
            if let Some(obj) = norm_req.params.as_object_mut() {
                let has_name = obj
                    .get("name")
                    .and_then(|v| v.as_str())
                    .is_some_and(|s| !s.trim().is_empty());
                if !has_name {
                    if let Some(sess_name) = obj
                        .get("session_name")
                        .and_then(|v| v.as_str())
                        .filter(|s| !s.trim().is_empty())
                        .map(str::to_string)
                    {
                        obj.insert("name".to_string(), serde_json::Value::String(sess_name));
                    }
                }
            }
            if let Some(resp) = handle_terminal_ipc_method(&self.engine.pty_mux, &norm_req).await {
                return resp;
            }
            return LocalIpcResponse::err(id, "method_not_found", format!("unknown method: {method}"));
        }

        // User domain
        if method.starts_with("user.") {
            return match method {
                "user.info" | "user.me" | "user.profile" => {
                    if p.get("user_id").and_then(|v| v.as_u64()).is_some() {
                        return LocalIpcResponse::err(
                            id,
                            "method_not_found",
                            format!("method requires HTTP fallback for explicit user_id: {method}"),
                        );
                    }
                    let token = match self.require_token(&client, &id) {
                        Ok(t) => t,
                        Err(resp) => return resp,
                    };
                    match client.http_client.get_me(&token).await {
                        Ok(me) => LocalIpcResponse::ok(id, serde_json::to_value(me).unwrap_or_default()),
                        Err(e) => LocalIpcResponse::err(id, "user_me_failed", e.to_string()),
                    }
                }
                "user.status" => {
                    let auth = client.current_auth_session();
                    LocalIpcResponse::ok(
                        id,
                        serde_json::json!({
                            "authenticated": client.token().is_some(),
                            "user_id": auth.as_ref().map(|a| a.user_id),
                            "username": auth.as_ref().map(|a| a.username.clone()),
                            "server_url": client.config.server_url,
                            "device_id": client.device_id(),
                        }),
                    )
                }
                _ => LocalIpcResponse::err(id, "method_not_found", format!("unknown method: {method}")),
            };
        }

        // Device domain
        if method.starts_with("device.") {
            return match method {
                "device.list" => match client.list_computers_async().await {
                    Ok(list) => LocalIpcResponse::ok(id, serde_json::to_value(list).unwrap_or_default()),
                    Err(e) => LocalIpcResponse::err(id, "device_list_failed", e.to_string()),
                },
                "device.current" => {
                    let (start_epoch, start_token) = match client.snapshot_pinned_session() {
                        Ok(pinned) => pinned,
                        Err(e) => {
                            return LocalIpcResponse::err(id, "device_current_failed", e.to_string())
                        }
                    };
                    match client
                        .register_current_host_computer_with_token(&start_token, start_epoch)
                        .await
                    {
                        Ok(node) => {
                            if !Arc::ptr_eq(&self.client(), &client)
                                || client.ensure_pinned_session(start_epoch, &start_token).is_err()
                            {
                                return LocalIpcResponse::err(
                                    id,
                                    "session_changed",
                                    "session changed before host registration completed",
                                );
                            }
                            self.set_local_node_id(Some(node.node_id.clone()));
                            LocalIpcResponse::ok(id, serde_json::to_value(node).unwrap_or_default())
                        }
                        Err(e) => LocalIpcResponse::err(id, "device_current_failed", e.to_string()),
                    }
                }
                "device.test" => self.exec_computer_op(&id, p, RemoteComputerOp::TestConnection).await,
                "device.exec" => {
                    let cmd = p.get("command").and_then(|v| v.as_str()).unwrap_or("").to_string();
                    let cwd = p.get("cwd").and_then(|v| v.as_str()).map(str::to_string);
                    let timeout_ms = p.get("timeout_ms").and_then(|v| v.as_u64());
                    self.exec_computer_op(&id, p, RemoteComputerOp::ShellExec { command: cmd, cwd, timeout_ms }).await
                }
                "device.power" => {
                    let kind = p.get("action").and_then(|v| v.as_str()).unwrap_or("lock");
                    let action = match kind {
                        "sleep" => PowerActionKind::Sleep,
                        "reboot" => PowerActionKind::Reboot,
                        "shutdown" => PowerActionKind::Shutdown,
                        "wol" | "wake_on_lan" => PowerActionKind::WakeOnLan {
                            mac_address: p
                                .get("mac_address")
                                .and_then(|v| v.as_str())
                                .unwrap_or("FF:FF:FF:FF:FF:FF")
                                .to_string(),
                            broadcast_ip: p
                                .get("broadcast_ip")
                                .and_then(|v| v.as_str())
                                .map(str::to_string),
                        },
                        _ => PowerActionKind::Lock,
                    };
                    self.exec_computer_op(&id, p, RemoteComputerOp::PowerAction { action }).await
                }
                _ => LocalIpcResponse::err(id, "method_not_found", format!("unknown method: {method}")),
            };
        }

        // Cloud PC domain
        if method.starts_with("cloudpc.") {
            return match method {
                "cloudpc.list" => match client.list_computers_async().await {
                    Ok(mut list) => {
                        list.computers.retain(|c| c.kind != gpui_telegram_proto::ComputerNodeKind::LocalDevice);
                        LocalIpcResponse::ok(id, serde_json::to_value(list).unwrap_or_default())
                    }
                    Err(e) => LocalIpcResponse::err(id, "cloudpc_list_failed", e.to_string()),
                },
                "cloudpc.upsert" => match serde_json::from_value::<UpsertCloudComputerRequest>(p.clone()) {
                    Ok(req_dto) => match client.upsert_cloud_computer_async(&req_dto).await {
                        Ok(node) => LocalIpcResponse::ok(id, serde_json::to_value(node).unwrap_or_default()),
                        Err(e) => LocalIpcResponse::err(id, "cloudpc_upsert_failed", e.to_string()),
                    },
                    Err(e) => LocalIpcResponse::err(id, "invalid_params", format!("invalid cloudpc.upsert params: {e}")),
                },
                "cloudpc.delete" => {
                    let node_id = p.get("node_id").and_then(|v| v.as_str()).unwrap_or("");
                    match client.delete_computer_async(node_id).await {
                        Ok(()) => LocalIpcResponse::ok(id, serde_json::json!({ "deleted": true, "node_id": node_id })),
                        Err(e) => LocalIpcResponse::err(id, "cloudpc_delete_failed", e.to_string()),
                    }
                }
                _ => LocalIpcResponse::err(id, "method_not_found", format!("unknown method: {method}")),
            };
        }

        // Remote Assist & Sessions domain
        if method.starts_with("assist.") {
            return match method {
                "assist.info" | "assist.id" => match client.get_remote_assist_info_async().await {
                    Ok(info) => LocalIpcResponse::ok(id, serde_json::to_value(info).unwrap_or_default()),
                    Err(e) => LocalIpcResponse::err(id, "assist_info_failed", e.to_string()),
                },
                "assist.reset_code" => match client.reset_remote_assist_code_async().await {
                    Ok(info) => LocalIpcResponse::ok(id, serde_json::to_value(info).unwrap_or_default()),
                    Err(e) => LocalIpcResponse::err(id, "assist_reset_failed", e.to_string()),
                },
                "assist.set_code" => {
                    let code = p.get("code").and_then(|v| v.as_str()).unwrap_or("");
                    match client.set_remote_assist_code_async(code).await {
                        Ok(info) => LocalIpcResponse::ok(id, serde_json::to_value(info).unwrap_or_default()),
                        Err(e) => LocalIpcResponse::err(id, "assist_set_code_failed", e.to_string()),
                    }
                }
                "assist.allow" => {
                    let allow = p.get("allow_assist").and_then(|v| v.as_bool()).unwrap_or(true);
                    match client.set_remote_assist_allow_async(allow).await {
                        Ok(info) => LocalIpcResponse::ok(id, serde_json::to_value(info).unwrap_or_default()),
                        Err(e) => LocalIpcResponse::err(id, "assist_allow_failed", e.to_string()),
                    }
                }
                "assist.start" => match serde_json::from_value::<AssistStartRequest>(p.clone()) {
                    Ok(start_req) => match client.start_remote_assist_async(&start_req).await {
                        Ok(res) => LocalIpcResponse::ok(id, serde_json::to_value(res).unwrap_or_default()),
                        Err(e) => LocalIpcResponse::err(id, "assist_start_failed", e.to_string()),
                    },
                    Err(e) => LocalIpcResponse::err(id, "invalid_params", format!("invalid assist.start params: {e}")),
                },
                "assist.sessions" => {
                    let mut combined = self.engine.hub.list_sessions();
                    if let Ok(remote) = client.list_remote_sessions_async().await {
                        for s in remote.sessions {
                            if !combined.iter().any(|x| x.session_id == s.session_id) {
                                combined.push(s);
                            }
                        }
                    }
                    LocalIpcResponse::ok(id, serde_json::json!({ "sessions": combined }))
                }
                "assist.disconnect" => {
                    let sid = p.get("session_id").and_then(|v| v.as_str()).unwrap_or("");
                    let local_removed = self.engine.hub.disconnect_session(sid);
                    let remote_res = client.disconnect_remote_session_async(sid).await.is_ok();
                    LocalIpcResponse::ok(
                        id,
                        serde_json::json!({ "session_id": sid, "local_removed": local_removed, "remote_disconnected": remote_res }),
                    )
                }
                "assist.disconnect_all" => {
                    let local_count = self.engine.hub.disconnect_all();
                    let remote_val = client.disconnect_all_remote_sessions_async().await.ok();
                    LocalIpcResponse::ok(
                        id,
                        serde_json::json!({ "local_disconnected": local_count, "remote": remote_val }),
                    )
                }
                _ => LocalIpcResponse::err(id, "method_not_found", format!("unknown method: {method}")),
            };
        }

        // Screen domain
        if method.starts_with("screen.") {
            return match method {
                "screen.capture" => self.exec_computer_op(&id, p, RemoteComputerOp::CaptureScreen).await,
                "screen.displays" => self.exec_computer_op(&id, p, RemoteComputerOp::DisplayList).await,
                "screen.switch" => {
                    let display_index = p.get("display_index").and_then(|v| v.as_u64()).unwrap_or(0) as u32;
                    self.exec_computer_op(&id, p, RemoteComputerOp::DisplaySwitch { display_index }).await
                }
                "screen.privacy" => {
                    let enabled = p.get("enabled").and_then(|v| v.as_bool()).unwrap_or(false);
                    let message = p.get("message").and_then(|v| v.as_str()).map(str::to_string);
                    self.exec_computer_op(&id, p, RemoteComputerOp::PrivacyScreenToggle { enabled, message }).await
                }
                _ => LocalIpcResponse::err(id, "method_not_found", format!("unknown method: {method}")),
            };
        }

        // Input domain
        if method.starts_with("input.") {
            return match method {
                "input.click" | "input.drag" | "input.scroll" | "input.mouse" => {
                    let default_action = match method {
                        "input.drag" => "drag",
                        "input.scroll" => "scroll",
                        _ => "click",
                    };
                    let op = RemoteComputerOp::MouseAction {
                        action: p
                            .get("action")
                            .and_then(|v| v.as_str())
                            .unwrap_or(default_action)
                            .to_string(),
                        x: p.get("x").and_then(|v| v.as_i64()).map(|n| n as i32),
                        y: p.get("y").and_then(|v| v.as_i64()).map(|n| n as i32),
                        dx: p.get("dx").and_then(|v| v.as_i64()).map(|n| n as i32),
                        dy: p.get("dy").and_then(|v| v.as_i64()).map(|n| n as i32),
                        button: p.get("button").and_then(|v| v.as_str()).map(str::to_string),
                        clicks: p.get("clicks").and_then(|v| v.as_u64()).map(|n| n as u8),
                        scroll_x: p.get("scroll_x").and_then(|v| v.as_i64()).map(|n| n as i32),
                        scroll_y: p.get("scroll_y").and_then(|v| v.as_i64()).map(|n| n as i32),
                        relative: p.get("relative").and_then(|v| v.as_bool()).unwrap_or(false),
                    };
                    self.exec_computer_op(&id, p, op).await
                }
                "input.move" => {
                    let op = RemoteComputerOp::MouseAction {
                        action: "move".to_string(),
                        x: p.get("x").and_then(|v| v.as_i64()).map(|n| n as i32),
                        y: p.get("y").and_then(|v| v.as_i64()).map(|n| n as i32),
                        dx: p.get("dx").and_then(|v| v.as_i64()).map(|n| n as i32),
                        dy: p.get("dy").and_then(|v| v.as_i64()).map(|n| n as i32),
                        button: None,
                        clicks: None,
                        scroll_x: None,
                        scroll_y: None,
                        relative: p.get("relative").and_then(|v| v.as_bool()).unwrap_or(false),
                    };
                    self.exec_computer_op(&id, p, op).await
                }
                "input.type" => {
                    let text = p.get("text").and_then(|v| v.as_str()).unwrap_or("").to_string();
                    self.exec_computer_op(
                        &id,
                        p,
                        RemoteComputerOp::KeyboardAction {
                            action: "type".to_string(),
                            text: Some(text),
                            key: None,
                            modifiers: Vec::new(),
                        },
                    )
                    .await
                }
                "input.key" | "input.hotkey" => {
                    let modifiers = p
                        .get("modifiers")
                        .and_then(|v| v.as_array())
                        .map(|arr| {
                            arr.iter()
                                .filter_map(|x| x.as_str().map(str::to_string))
                                .collect()
                        })
                        .unwrap_or_default();
                    let op = RemoteComputerOp::KeyboardAction {
                        action: if method == "input.hotkey" {
                            "hotkey".to_string()
                        } else {
                            p.get("action").and_then(|v| v.as_str()).unwrap_or("press").to_string()
                        },
                        text: None,
                        key: p.get("key").and_then(|v| v.as_str()).map(str::to_string),
                        modifiers,
                    };
                    self.exec_computer_op(&id, p, op).await
                }
                _ => LocalIpcResponse::err(id, "method_not_found", format!("unknown method: {method}")),
            };
        }

        // File domain
        if method.starts_with("file.") {
            return match method {
                "file.ls" | "file.list" => {
                    let path = p.get("path").and_then(|v| v.as_str()).unwrap_or("~").to_string();
                    self.exec_computer_op(&id, p, RemoteComputerOp::ListDir { path }).await
                }
                "file.read" => {
                    let path = p.get("path").and_then(|v| v.as_str()).unwrap_or("").to_string();
                    let max_bytes = p.get("max_bytes").and_then(|v| v.as_u64()).map(|n| n as usize);
                    self.exec_computer_op(&id, p, RemoteComputerOp::ReadFile { path, max_bytes }).await
                }
                "file.write" => {
                    let path = p.get("path").and_then(|v| v.as_str()).unwrap_or("").to_string();
                    let content = p.get("content").and_then(|v| v.as_str()).unwrap_or("").to_string();
                    let create_dirs = p.get("create_dirs").and_then(|v| v.as_bool()).unwrap_or(true);
                    self.exec_computer_op(&id, p, RemoteComputerOp::WriteFile { path, content, create_dirs }).await
                }
                "file.push" => {
                    let remote_path = p
                        .get("remote_path")
                        .or_else(|| p.get("path"))
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .trim()
                        .to_string();
                    if remote_path.is_empty() {
                        return LocalIpcResponse::err(id, "invalid_params", "missing remote_path or path");
                    }
                    let raw_b64_opt = p.get("data_b64").and_then(|v| v.as_str());
                    let bytes = if let Some(b64) = raw_b64_opt {
                        match base64::engine::general_purpose::STANDARD.decode(b64.trim()) {
                            Ok(b) => b,
                            Err(e) => {
                                return LocalIpcResponse::err(
                                    id,
                                    "invalid_params",
                                    format!("invalid base64 payload for file.push: {e}"),
                                );
                            }
                        }
                    } else if let Some(content) = p.get("content").and_then(|v| v.as_str()) {
                        content.as_bytes().to_vec()
                    } else {
                        return LocalIpcResponse::err(
                            id,
                            "invalid_params",
                            "missing data_b64 or content for file.push",
                        );
                    };

                    let is_remote = self
                        .target_node_id(p)
                        .as_deref()
                        .is_some_and(|nid| self.is_remote_target(nid));
                    if is_remote {
                        if let Ok(text) = String::from_utf8(bytes.clone()) {
                            self.exec_computer_op(
                                &id,
                                p,
                                RemoteComputerOp::WriteFile {
                                    path: remote_path,
                                    content: text,
                                    create_dirs: true,
                                },
                            )
                            .await
                        } else {
                            let encoded = base64::engine::general_purpose::STANDARD.encode(&bytes);
                            self.exec_computer_op(
                                &id,
                                p,
                                RemoteComputerOp::FileChunkOp {
                                    op: "write_chunk".to_string(),
                                    transfer_id: format!("ipc-push-{}", uuid::Uuid::new_v4()),
                                    path: remote_path,
                                    offset: 0,
                                    data_b64: Some(encoded),
                                    sha256: None,
                                    total_size: Some(bytes.len() as u64),
                                },
                            )
                            .await
                        }
                    } else {
                        if let Some(parent) = std::path::Path::new(&remote_path).parent() {
                            if !parent.as_os_str().is_empty() {
                                if let Err(e) = tokio::fs::create_dir_all(parent).await {
                                    return LocalIpcResponse::err(
                                        id,
                                        "file_push_failed",
                                        format!("failed to create directories for '{remote_path}': {e}"),
                                    );
                                }
                            }
                        }
                        match tokio::fs::write(&remote_path, &bytes).await {
                            Ok(()) => LocalIpcResponse::ok(
                                id,
                                serde_json::json!({
                                    "ok": true,
                                    "path": remote_path,
                                    "bytes_written": bytes.len(),
                                    "stdout": format!("Wrote {} bytes to {}", bytes.len(), remote_path),
                                }),
                            ),
                            Err(e) => LocalIpcResponse::err(
                                id,
                                "file_push_failed",
                                format!("failed to write '{remote_path}': {e}"),
                            ),
                        }
                    }
                }
                "file.pull" => {
                    let remote_path = p
                        .get("remote_path")
                        .or_else(|| p.get("path"))
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .trim()
                        .to_string();
                    if remote_path.is_empty() {
                        return LocalIpcResponse::err(id, "invalid_params", "missing remote_path or path");
                    }
                    let max_bytes = p.get("max_bytes").and_then(|v| v.as_u64()).map(|n| n as usize);
                    let is_remote = self
                        .target_node_id(p)
                        .as_deref()
                        .is_some_and(|nid| self.is_remote_target(nid));
                    if is_remote {
                        self.exec_computer_op(
                            &id,
                            p,
                            RemoteComputerOp::ReadFile {
                                path: remote_path,
                                max_bytes,
                            },
                        )
                        .await
                    } else {
                        match tokio::fs::read(&remote_path).await {
                            Ok(mut bytes) => {
                                if let Some(limit) = max_bytes {
                                    if bytes.len() > limit {
                                        bytes.truncate(limit);
                                    }
                                }
                                let data_b64 = base64::engine::general_purpose::STANDARD.encode(&bytes);
                                LocalIpcResponse::ok(
                                    id,
                                    serde_json::json!({
                                        "ok": true,
                                        "path": remote_path,
                                        "size": bytes.len(),
                                        "data_b64": data_b64,
                                    }),
                                )
                            }
                            Err(e) => LocalIpcResponse::err(
                                id,
                                "file_pull_failed",
                                format!("failed to read '{remote_path}': {e}"),
                            ),
                        }
                    }
                }
                "file.chunk" | "file.upload" | "file.download" => {
                    let sub_op = p
                        .get("op")
                        .or_else(|| p.get("chunk_op"))
                        .and_then(|v| v.as_str())
                        .unwrap_or(match method {
                            "file.upload" => "write_chunk",
                            "file.download" => "read_chunk",
                            _ => "read_chunk",
                        })
                        .to_string();
                    let op = RemoteComputerOp::FileChunkOp {
                        op: sub_op,
                        transfer_id: p
                            .get("transfer_id")
                            .and_then(|v| v.as_str())
                            .unwrap_or("ipc-xfer")
                            .to_string(),
                        path: p
                            .get("path")
                            .or_else(|| p.get("remote_path"))
                            .and_then(|v| v.as_str())
                            .unwrap_or("")
                            .to_string(),
                        offset: p.get("offset").and_then(|v| v.as_u64()).unwrap_or(0),
                        data_b64: p.get("data_b64").and_then(|v| v.as_str()).map(str::to_string),
                        sha256: p.get("sha256").and_then(|v| v.as_str()).map(str::to_string),
                        total_size: p.get("total_size").and_then(|v| v.as_u64()),
                    };
                    self.exec_computer_op(&id, p, op).await
                }
                _ => LocalIpcResponse::err(id, "method_not_found", format!("unknown method: {method}")),
            };
        }

        // Clipboard domain
        if method.starts_with("clipboard.") {
            return match method {
                "clipboard.get" => self.exec_computer_op(&id, p, RemoteComputerOp::ClipboardGet).await,
                "clipboard.set" => {
                    let op = RemoteComputerOp::ClipboardSet {
                        text: p.get("text").and_then(|v| v.as_str()).map(str::to_string),
                        image_b64: p.get("image_b64").and_then(|v| v.as_str()).map(str::to_string),
                        mime_type: p.get("mime_type").and_then(|v| v.as_str()).map(str::to_string),
                    };
                    self.exec_computer_op(&id, p, op).await
                }
                _ => LocalIpcResponse::err(id, "method_not_found", format!("unknown method: {method}")),
            };
        }

        // Chat & Messaging domain
        if method.starts_with("chat.") || method.starts_with("msg.") {
            if method == "msg.history" && p.get("offset_id").and_then(|v| v.as_u64()).is_some() {
                return LocalIpcResponse::err(
                    id,
                    "method_not_found",
                    format!("offset_id requires HTTP fallback: {method}"),
                );
            }
            if !matches!(method, "chat.list" | "msg.list" | "msg.history" | "msg.send") {
                return LocalIpcResponse::err(id, "method_not_found", format!("unknown method: {method}"));
            }
            let token = match self.require_token(&client, &id) {
                Ok(t) => t,
                Err(resp) => return resp,
            };
            return match method {
                "chat.list" => match client.http_client.get_chats(&token).await {
                    Ok(chats) => LocalIpcResponse::ok(id, serde_json::json!({ "chats": chats })),
                    Err(e) => LocalIpcResponse::err(id, "chat_list_failed", e.to_string()),
                },
                "msg.list" | "msg.history" => {
                    let chat_id = p.get("chat_id").and_then(|v| v.as_u64()).unwrap_or(0);
                    let limit = p.get("limit").and_then(|v| v.as_u64()).map(|n| n as usize);
                    match client.http_client.get_chat_messages(&token, chat_id, limit).await {
                        Ok(msgs) => LocalIpcResponse::ok(id, serde_json::json!({ "messages": msgs })),
                        Err(e) => LocalIpcResponse::err(id, "msg_list_failed", e.to_string()),
                    }
                }
                "msg.send" => {
                    let chat_id = p.get("chat_id").and_then(|v| v.as_u64()).unwrap_or(0);
                    let text = p.get("text").and_then(|v| v.as_str()).unwrap_or("").to_string();
                    let reply_to = p
                        .get("reply_to")
                        .or_else(|| p.get("reply_to_id"))
                        .and_then(|v| v.as_u64());
                    let http_req = gpui_telegram_client::net::SendMessageHttpRequest {
                        chat_id,
                        text,
                        reply_to,
                    };
                    match client.http_client.send_message(&token, &http_req).await {
                        Ok(sent) => LocalIpcResponse::ok(id, serde_json::to_value(sent).unwrap_or_default()),
                        Err(e) => LocalIpcResponse::err(id, "msg_send_failed", e.to_string()),
                    }
                }
                _ => LocalIpcResponse::err(id, "method_not_found", format!("unknown method: {method}")),
            };
        }

        // MCP domain
        if method.starts_with("mcp.") {
            return match method {
                "mcp.grants" => match client.list_mcp_grants_async().await {
                    Ok(resp) => LocalIpcResponse::ok(id, serde_json::to_value(resp).unwrap_or_default()),
                    Err(e) => LocalIpcResponse::err(id, "mcp_grants_failed", e.to_string()),
                },
                "mcp.revoke" => {
                    let family_id = p.get("family_id").and_then(|v| v.as_str()).unwrap_or("");
                    match client.revoke_mcp_grant_async(family_id).await {
                        Ok(()) => LocalIpcResponse::ok(id, serde_json::json!({ "revoked": true, "family_id": family_id })),
                        Err(e) => LocalIpcResponse::err(id, "mcp_revoke_failed", e.to_string()),
                    }
                }
                "mcp.tools" => LocalIpcResponse::ok(
                    id,
                    serde_json::json!({
                        "tools": [
                            "devices.list", "devices.exec", "screen.capture", "gui.input",
                            "terminal.new", "terminal.exec", "terminal.read",
                            "files.list", "files.read", "files.write",
                            "chats.list", "messages.list", "messages.send"
                        ]
                    }),
                ),
                _ => LocalIpcResponse::err(id, "method_not_found", format!("unknown method: {method}")),
            };
        }

        LocalIpcResponse::err(id, "method_not_found", format!("unknown IPC method: {method}"))
    }
}

impl LocalIpcHandler for DesktopLocalIpcHandler {
    fn handle(&self, req: LocalIpcRequest) -> BoxFuture<'_, LocalIpcResponse> {
        Box::pin(async move { self.dispatch(req).await })
    }
}
