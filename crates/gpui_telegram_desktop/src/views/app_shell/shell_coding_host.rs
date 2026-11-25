//! Local Coding Agent Host runtime lifecycle, CLI provider probing, and WS command bridge.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use gpui::{AsyncApp, Context, WeakEntity};
use gpui_telegram_coding::{
    probe_local_coding_providers, CodingHostRuntimeConfig, LocalCodingHostRuntime,
};
use gpui_telegram_proto::{CodingHostCommand, CodingHostEvent};

use crate::views::shell::AppShell;

/// Returns the `owner_user_id` declared by a [`CodingHostCommand`].
pub fn coding_command_owner_user_id(cmd: &CodingHostCommand) -> u64 {
    match cmd {
        CodingHostCommand::ProbeProviders { owner_user_id, .. }
        | CodingHostCommand::StartTurn { owner_user_id, .. }
        | CodingHostCommand::CancelTurn { owner_user_id, .. }
        | CodingHostCommand::ResolveApproval { owner_user_id, .. }
        | CodingHostCommand::Control { owner_user_id, .. } => *owner_user_id,
    }
}

/// Returns the optional `target_device_id` declared by a [`CodingHostCommand`].
pub fn coding_command_target_device_id(cmd: &CodingHostCommand) -> Option<&str> {
    match cmd {
        CodingHostCommand::ProbeProviders { target_device_id, .. }
        | CodingHostCommand::StartTurn { target_device_id, .. }
        | CodingHostCommand::CancelTurn { target_device_id, .. }
        | CodingHostCommand::ResolveApproval { target_device_id, .. }
        | CodingHostCommand::Control { target_device_id, .. } => target_device_id.as_deref(),
    }
}

/// Verifies that an incoming `CodingHostCommand` matches the active signed-in owner and device.
pub fn should_accept_coding_host_command(
    is_authenticated: bool,
    session_user_id: Option<u64>,
    local_device_id: &str,
    cmd: &CodingHostCommand,
) -> bool {
    if !is_authenticated {
        return false;
    }
    let owner_id = coding_command_owner_user_id(cmd);
    if owner_id == 0 || session_user_id != Some(owner_id) {
        return false;
    }
    if let Some(target) = coding_command_target_device_id(cmd) {
        let trimmed = target.trim();
        if !trimmed.is_empty() && trimmed != local_device_id {
            return false;
        }
    }
    true
}

/// Returns the per-account local coding host directory under `preferences_dir`.
pub fn user_coding_host_dir(preferences_dir: &Path, user_id: u64) -> PathBuf {
    preferences_dir
        .join("coding_hosts")
        .join(format!("user_{user_id}"))
}

impl AppShell {
    /// Initializes or reuses the account- and device-bound `LocalCodingHostRuntime`.
    pub fn ensure_coding_host_runtime(
        &mut self,
        _cx: &mut Context<Self>,
    ) -> Option<Arc<LocalCodingHostRuntime>> {
        if !self.is_authenticated {
            return None;
        }
        let user_id = self.session_sync.user_id.filter(|&uid| uid != 0)?;
        let device_id = self.client.device_id();
        let host_dir = user_coding_host_dir(&self.preferences_dir, user_id);
        let db_path = host_dir.join("coding_host_state.db");
        let worktrees_root = host_dir.join("coding-worktrees");
        if let Some(existing) = self.coding_host.runtime.as_ref() {
            if existing.owner_user_id() == user_id
                && existing.device_id() == device_id
                && existing.db_path() == db_path
                && existing.worktrees_root() == worktrees_root
            {
                return Some(Arc::clone(existing));
            }
        }
        if let Some(old) = self.coding_host.runtime.take() {
            self.client.runtime().spawn(async move {
                old.shutdown().await;
            });
        }
        let cfg = CodingHostRuntimeConfig {
            owner_user_id: user_id,
            device_id,
            db_path,
            worktrees_root,
        };
        match LocalCodingHostRuntime::new(cfg) {
            Ok(rt) => {
                let arc = Arc::new(rt);
                self.coding_host.runtime = Some(Arc::clone(&arc));
                Some(arc)
            }
            Err(err) => {
                eprintln!("[CodingHost] Failed to initialize LocalCodingHostRuntime: {err}");
                None
            }
        }
    }

    /// Probes installed local coding CLI providers off the UI thread and updates any open bot modal.
    pub fn refresh_detected_coding_providers(&mut self, cx: &mut Context<Self>) {
        if !self.is_authenticated || self.session_sync.user_id.is_none() {
            return;
        }
        let auth_gen = self.session_sync.auth_generation;
        let owner_user_id = self.session_sync.user_id;
        let client = self.client.clone();
        let runtime_handle = client.runtime().handle().clone();
        cx.spawn(async move |weak: WeakEntity<Self>, app: &mut AsyncApp| {
            let Ok(providers) = runtime_handle
                .spawn(async move { probe_local_coding_providers().await })
                .await
            else {
                return;
            };
            let _ = weak.update(app, |shell, cx| {
                if !shell.is_authenticated
                    || shell.session_sync.auth_generation != auth_gen
                    || shell.session_sync.user_id != owner_user_id
                    || !Arc::ptr_eq(&shell.client, &client)
                {
                    return;
                }
                shell.coding_host.detected_providers = providers.clone();
                let device_id = shell.client.device_id();
                if let Some(ref box_ent) = shell.miniapps.create_bot_box {
                    box_ent.update(cx, |b, b_cx| {
                        b.set_detected_coding_providers(providers, Some(device_id));
                        b_cx.notify();
                    });
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// Probes local coding providers and announces this host device to the server over WebSocket.
    pub fn announce_coding_host_to_server(&mut self, cx: &mut Context<Self>) {
        if !self.is_authenticated
            || self.session_sync.session_token.is_none()
            || self.session_sync.user_id.is_none()
        {
            return;
        }
        let _ = self.ensure_coding_host_runtime(cx);
        let auth_gen = self.session_sync.auth_generation;
        let owner_user_id = self.session_sync.user_id;
        let expected_token = self.session_sync.session_token.clone();
        let client = self.client.clone();
        let runtime_handle = client.runtime().handle().clone();
        cx.spawn(async move |weak: WeakEntity<Self>, app: &mut AsyncApp| {
            let Ok((providers, display_name, os_name)) = runtime_handle
                .spawn(async move {
                    let providers = probe_local_coding_providers().await;
                    let host = gpui_telegram_client::detect_local_hostname();
                    let display_name = if host.trim().is_empty() {
                        "Desktop".to_string()
                    } else {
                        host.trim().to_string()
                    };
                    let (_, os_name, _) = gpui_telegram_client::detect_local_host_info();
                    (providers, display_name, os_name)
                })
                .await
            else {
                return;
            };
            let _ = weak.update(app, |shell, cx| {
                if !shell.is_authenticated
                    || shell.session_sync.auth_generation != auth_gen
                    || shell.session_sync.user_id != owner_user_id
                    || shell.session_sync.session_token.as_deref() != expected_token.as_deref()
                    || !Arc::ptr_eq(&shell.client, &client)
                {
                    return;
                }
                let device_id = shell.client.device_id();
                shell.coding_host.detected_providers = providers.clone();
                if let Some(ref box_ent) = shell.miniapps.create_bot_box {
                    box_ent.update(cx, |b, b_cx| {
                        b.set_detected_coding_providers(providers.clone(), Some(device_id.clone()));
                        b_cx.notify();
                    });
                }
                let ev = CodingHostEvent::HostRegister {
                    device_id,
                    display_name,
                    os_name,
                    providers,
                };
                if shell.client.send_coding_host_event(ev).is_ok() {
                    shell.coding_host.registered_for_auth_gen = Some(auth_gen);
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// Verifies ownership/device targeting and executes a server `CodingHostCommand` on Tokio,
    /// streaming every emitted `CodingHostEvent` immediately back over WebSocket.
    pub fn handle_coding_host_command(&mut self, cmd: CodingHostCommand, cx: &mut Context<Self>) {
        let local_device_id = self.client.device_id();
        if !should_accept_coding_host_command(
            self.is_authenticated,
            self.session_sync.user_id,
            &local_device_id,
            &cmd,
        ) {
            eprintln!(
                "[CodingHost] Ignoring CodingHostCommand due to owner/device/session mismatch (cmd_owner={}, session_user={:?})",
                coding_command_owner_user_id(&cmd),
                self.session_sync.user_id,
            );
            return;
        }
        let Some(runtime) = self.ensure_coding_host_runtime(cx) else {
            return;
        };
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<CodingHostEvent>();
        let tokio_handle = self.client.runtime().handle().clone();
        let forward_client = self.client.clone();
        let owner_user_id = coding_command_owner_user_id(&cmd);
        let expected_token = self.session_sync.session_token.clone();
        let runtime_for_fwd = Arc::clone(&runtime);
        tokio_handle.spawn(async move {
            while let Some(ev) = rx.recv().await {
                if runtime_for_fwd.is_shutting_down()
                    || forward_client.current_user_id() != Some(owner_user_id)
                    || forward_client.token().as_deref() != expected_token.as_deref()
                {
                    break;
                }
                if let Err(err) = forward_client.send_coding_host_event(ev) {
                    eprintln!("[CodingHost] Failed forwarding CodingHostEvent over WS: {err}");
                }
            }
        });
        tokio_handle.spawn(async move {
            runtime.handle_command(cmd, tx).await;
        });
    }
}
