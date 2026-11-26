//! Unified Runtime RPC v3 Dispatcher & E2EE v2 Session Manager (`F20`).
//!
//! Validates `protocol_version` against `RUNTIME_PROTOCOL_VERSION = 3` (min 2) via
//! `negotiate_runtime_protocol` / `negotiate_runtime_capabilities`, enforces `CallerScope`
//! access control, deduplicates mutating operations via `OperationIdDedupRing`, and routes
//! all 18+ namespaces across `orca_ide` and optional `orca_bots::OrcaBotsFacade`.

mod bots_routes;
mod bots_routes_ext;
mod bots_routes_tools;
pub mod e2ee_manager;
mod ide_routes;
mod ide_routes_ext;
mod orch_worktree_routes;

use std::collections::BTreeMap;
use std::fmt;
use std::sync::Arc;

use orca_bots::OrcaBotsFacade;
use orca_core::RUNTIME_PROTOCOL_VERSION;
use orca_core::orchestration_protocol::{
    CallerScope, OperationIdDedupRing, RpcRequestEnvelope, RpcResponseEnvelope,
    negotiate_runtime_protocol,
};
use std::path::PathBuf;
use orca_core::workspace_layout::{WorkspaceLayoutModel, WorkspaceTab};
use parking_lot::{Mutex, RwLock};

use crate::accounts_usage::{AccountHomeManager, UsageRosterManager};
use crate::agent_status_hooks::{AgentHookServer, AgentHookServerConfig, AgentStatusEngine};
use crate::chat_journal::ChatJournalStore;
use crate::diff_review::{PersistentDiffReviewStore, TasksHubStore};
use crate::hibernation::HibernationController;
use crate::orchestration_engine::OrchestrationEngine;
use crate::plugin_host::{PluginHostV2, TransactionalSkillInstaller};
use crate::ports_proxy::{ListeningPortScanner, LocalhostWorktreeLabelProxy};
use crate::profile_state::{
    AtomicSnapshotWriter, ProfileStateAuthority, ProtectedSecretPersistence,
};
use crate::pty_multiplex::PtyMultiplexController;
use crate::system_services::{
    ComputerUseService, EphemeralVmService, MobileEmulatorService, ShareableArtifactStore,
    SleepAwareHangWatchdog, SttDictationService,
};
use crate::vault_search::AiVaultSearchEngine;
use crate::worktree_manager::WorktreeManager;
use crate::{OrcaIdeRuntimeError, Result};

pub use e2ee_manager::{
    E2eeHandshakeInitRequest, E2eeHandshakeServerResponse, E2eeV2SessionManager,
};

/// State for an open multiplexed PTY stream (`terminal.multiplex.open`/`poll`/`ackOutput`).
#[derive(Debug, Clone)]
pub struct MuxStreamState {
    pub stream_id: String,
    pub session_ids: Vec<String>,
    pub window_bytes: usize,
    pub available_credits: usize,
    pub overflow_count: usize,
}

/// Registered worktree-to-localhost label route exposed through `LocalhostWorktreeLabelProxy`.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct RegisteredWorktreePort {
    pub worktree_id: String,
    pub label: String,
    pub target_port: u16,
    pub hostname: String,
}

/// Per-tab browser session state (Design Mode, viewport preset/dimensions, last capture).
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct BrowserTabSessionState {
    pub tab_id: String,
    pub url: String,
    pub design_mode_enabled: bool,
    pub viewport_preset: String,
    pub viewport_width: u32,
    pub viewport_height: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_capture: Option<serde_json::Value>,
}

impl BrowserTabSessionState {
    pub fn new(tab_id: impl Into<String>) -> Self {
        Self {
            tab_id: tab_id.into(),
            url: "about:blank".to_string(),
            design_mode_enabled: false,
            viewport_preset: "web".to_string(),
            viewport_width: 1280,
            viewport_height: 800,
            last_capture: None,
        }
    }
}

/// Default bounded FIFO capacity for `OperationIdDedupRing` idempotency caching.
pub const DEFAULT_RPC_DEDUP_CAPACITY: usize = 2_048;

/// Shared `Arc` handles to every `orca_ide` + optional `orca_bots` subsystem routed by
/// [`UnifiedRpcDispatcher`].
pub struct RpcSubsystemHandles {
    pub host_id: String,
    pub orchestration: Arc<OrchestrationEngine>,
    pub chat_journal: Arc<ChatJournalStore>,
    pub agent_status: Arc<AgentStatusEngine>,
    pub worktrees: Arc<WorktreeManager>,
    pub port_scanner: Arc<ListeningPortScanner>,
    pub pty: Arc<PtyMultiplexController>,
    pub hibernation: Arc<HibernationController>,
    pub accounts: Arc<AccountHomeManager>,
    pub usage_roster: Arc<UsageRosterManager>,
    pub profile_state: Arc<ProfileStateAuthority>,
    pub vault_search: Arc<AiVaultSearchEngine>,
    pub diff_review: Arc<PersistentDiffReviewStore>,
    pub tasks_hub: Arc<TasksHubStore>,
    pub plugin_host: Arc<PluginHostV2>,
    pub skill_installer: Arc<TransactionalSkillInstaller>,
    pub computer_use: Arc<ComputerUseService>,
    pub mobile_emulator: Arc<MobileEmulatorService>,
    pub stt: Arc<SttDictationService>,
    pub artifacts: Arc<ShareableArtifactStore>,
    pub ephemeral_vm: Arc<EphemeralVmService>,
    pub watchdog: Arc<SleepAwareHangWatchdog>,
    pub e2ee: Arc<E2eeV2SessionManager>,
    pub layouts: Arc<RwLock<BTreeMap<String, WorkspaceLayoutModel>>>,
    pub label_proxy: Arc<tokio::sync::OnceCell<Arc<LocalhostWorktreeLabelProxy>>>,
    pub registered_ports: Arc<RwLock<Vec<RegisteredWorktreePort>>>,
    pub hook_server: Arc<tokio::sync::OnceCell<Arc<AgentHookServer>>>,
    pub hook_endpoint_dir: PathBuf,
    pub mux_streams: Arc<RwLock<BTreeMap<String, MuxStreamState>>>,
    pub hibernated_tabs: Arc<RwLock<BTreeMap<String, WorkspaceTab>>>,
    pub snapshot_writers: Arc<Mutex<BTreeMap<PathBuf, Arc<AtomicSnapshotWriter>>>>,
    pub browser_tabs: Arc<RwLock<BTreeMap<String, BrowserTabSessionState>>>,
    pub browser_profiles: Arc<RwLock<BTreeMap<String, String>>>,
    pub secrets: Arc<ProtectedSecretPersistence>,
    pub closed_tab_tombstones: Arc<RwLock<BTreeMap<String, i64>>>,
    pub bots: Option<Arc<OrcaBotsFacade>>,
}

impl RpcSubsystemHandles {
    /// Lazily binds the live loopback `*.orca.localhost` reverse proxy on `127.0.0.1:0`.
    pub async fn ensure_label_proxy(&self) -> Result<Arc<LocalhostWorktreeLabelProxy>> {
        let proxy = self
            .label_proxy
            .get_or_try_init(|| async {
                LocalhostWorktreeLabelProxy::bind_loopback()
                    .await
                    .map(Arc::new)
            })
            .await?;
        Ok(Arc::clone(proxy))
    }

    /// Lazily binds the live loopback HTTP/1.1 `AgentHookServer` (`127.0.0.1:0`) and writes
    /// `endpoint.env` + `endpoint.cmd` atomically inside `hook_endpoint_dir`.
    pub async fn ensure_hook_server(&self) -> Result<Arc<AgentHookServer>> {
        let srv = self
            .hook_server
            .get_or_try_init(|| async {
                let cfg = AgentHookServerConfig::new(self.hook_endpoint_dir.clone());
                AgentHookServer::bind(Arc::clone(&self.agent_status), cfg)
                    .await
                    .map(Arc::new)
            })
            .await?;
        Ok(Arc::clone(srv))
    }
}

/// Unified Runtime RPC v3 Dispatcher (`Send + Sync`).
pub struct UnifiedRpcDispatcher {
    handles: Arc<RpcSubsystemHandles>,
    dedup_ring: Mutex<OperationIdDedupRing>,
}

impl fmt::Debug for UnifiedRpcDispatcher {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("UnifiedRpcDispatcher")
            .field("host_id", &self.handles.host_id)
            .field("bots_attached", &self.handles.bots.is_some())
            .field("dedup_entries", &self.dedup_ring.lock().len())
            .finish()
    }
}

fn expand_ide_alias(method: &str) -> &str {
    match method {
        "ide.vault_search" => "aiVault.search",
        "ide.diff_annotate" => "diff.annotate",
        "ide.diff_batch" => "diff.batch",
        "ide.plugin_validate" => "plugins.validate",
        "ide.plugin_install" => "plugins.install",
        "ide.computer_guard" => "computer.guard",
        "ide.artifact_share" => "artifacts.share",
        "ide.hibernation_check" => "hibernation.check",
        "ide.usage" => "usage.roster",
        "ide.e2ee_handshake" => "e2ee.handshake",
        "ide.profile_get" => "profile.get",
        "ide.profile_put" => "profile.put",
        other => other,
    }
}

fn is_host_mcp_toggle_method(canonical_method: &str) -> bool {
    matches!(
        canonical_method,
        "bots.tools.setHostMcpServers"
            | "bot.tools.setHostMcpServers"
            | "bots.tools.set_host_mcp_servers"
            | "bot.tools.set_host_mcp_servers"
    )
}

fn is_host_mcp_server_mutation(canonical_method: &str) -> bool {
    matches!(
        canonical_method,
        "mcpServers.add"
            | "mcpServers.update"
            | "mcpServers.remove"
            | "mcpServers.delete"
            | "mcpServers.sync"
            | "mcp_servers.add"
            | "mcp_servers.update"
            | "mcp_servers.remove"
            | "mcp_servers.delete"
            | "mcp_servers.sync"
    )
}

fn check_caller_scope(scope: CallerScope, canonical_method: &str) -> Result<()> {
    match scope {
        CallerScope::Desktop | CallerScope::Cli => Ok(()),
        CallerScope::Bot => {
            if is_host_mcp_toggle_method(canonical_method)
                || is_host_mcp_server_mutation(canonical_method)
                || matches!(
                    canonical_method,
                    "plugins.approve"
                        | "plugins.install"
                        | "accounts.swap"
                        | "webhooks.create"
                        | "webhook.create"
                        | "e2ee.handshake"
                )
            {
                return Err(OrcaIdeRuntimeError::permission_denied(format!(
                    "caller_scope 'bot' is not authorized for privileged method '{canonical_method}'"
                )));
            }
            Ok(())
        }
        CallerScope::Relay => {
            if is_host_mcp_toggle_method(canonical_method)
                || is_host_mcp_server_mutation(canonical_method)
                || canonical_method.starts_with("computer.")
                || canonical_method.starts_with("emulator.")
                || canonical_method.starts_with("plugins.")
                || canonical_method == "worktree.cleanup"
                || canonical_method == "worktree.remove"
                || canonical_method == "accounts.swap"
            {
                return Err(OrcaIdeRuntimeError::permission_denied(format!(
                    "caller_scope 'relay' is not authorized for local host method '{canonical_method}'"
                )));
            }
            Ok(())
        }
        CallerScope::RemoteHost => {
            if is_host_mcp_toggle_method(canonical_method)
                || is_host_mcp_server_mutation(canonical_method)
                || matches!(canonical_method, "plugins.approve" | "accounts.swap")
            {
                return Err(OrcaIdeRuntimeError::permission_denied(format!(
                    "caller_scope 'remote_host' is not authorized for method '{canonical_method}'"
                )));
            }
            Ok(())
        }
    }
}

impl UnifiedRpcDispatcher {
    pub fn new(handles: Arc<RpcSubsystemHandles>, dedup_capacity: usize) -> Self {
        Self {
            handles,
            dedup_ring: Mutex::new(OperationIdDedupRing::new(dedup_capacity.max(1))),
        }
    }

    pub fn dedup_len(&self) -> usize {
        self.dedup_ring.lock().len()
    }

    /// Dispatches a complete [`RpcRequestEnvelope`] and returns a canonical
    /// [`RpcResponseEnvelope`] (`{ok, result? | error?}`).
    pub async fn dispatch(&self, req: RpcRequestEnvelope) -> RpcResponseEnvelope {
        self.dispatch_with_caller_profile(req, None).await
    }

    /// Dispatches a complete [`RpcRequestEnvelope`] bound to an authenticated HTTP caller
    /// profile (e.g. `user:<id>`) so user-owned mutations cannot cross profile boundaries.
    pub async fn dispatch_with_caller_profile(
        &self,
        req: RpcRequestEnvelope,
        authenticated_profile: Option<&str>,
    ) -> RpcResponseEnvelope {
        if let Err(err) = negotiate_runtime_protocol(req.protocol_version, RUNTIME_PROTOCOL_VERSION)
        {
            let rt_err = OrcaIdeRuntimeError::ProtocolVersion(err);
            return RpcResponseEnvelope::err(rt_err.code_str(), rt_err.to_string());
        }

        let op_id = req
            .operation_id
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(|id| match authenticated_profile {
                Some(profile) => format!("{profile}\0{id}"),
                None => id.to_string(),
            });

        if let Some(ref id) = op_id
            && let Some(cached) = self.dedup_ring.lock().get(id).cloned()
        {
            return cached;
        }

        let canonical_method = expand_ide_alias(req.method.trim());
        let response = match self
            .execute_method(canonical_method, &req, authenticated_profile)
            .await
        {
            Ok(val) => RpcResponseEnvelope::ok(val),
            Err(err) => RpcResponseEnvelope::err(err.code_str(), err.to_string()),
        };

        if let Some(id) = op_id {
            self.dedup_ring.lock().record(id, response.clone());
        }
        response
    }

    async fn execute_method(
        &self,
        canonical_method: &str,
        req: &RpcRequestEnvelope,
        authenticated_profile: Option<&str>,
    ) -> Result<serde_json::Value> {
        check_caller_scope(req.caller_scope, canonical_method)?;

        let Some((namespace, op)) = canonical_method.split_once('.') else {
            return Err(OrcaIdeRuntimeError::invalid_argument(format!(
                "RPC method '{}' must follow '<namespace>.<operation>' format",
                req.method
            )));
        };

        match namespace {
            "runtime" => ide_routes::dispatch_runtime_namespace(
                &self.handles,
                op,
                &req.params,
                req.protocol_version,
            ),
            "bots" | "bot" => {
                bots_routes::dispatch_bots_namespace(&self.handles, op, &req.params).await
            }
            "mcpServers" | "mcp_servers" => {
                bots_routes_tools::dispatch_mcp_servers_namespace(&self.handles, op, &req.params)
                    .await
            }
            "groups" | "group" => {
                bots_routes::dispatch_groups_namespace(self.handles.bots.as_ref(), op, &req.params)
                    .await
            }
            "webhooks" | "webhook" => {
                bots_routes::dispatch_webhooks_namespace(
                    self.handles.bots.as_ref(),
                    op,
                    &req.params,
                )
                .await
            }
            "automations" | "automation" => {
                bots_routes::dispatch_automations_namespace(
                    self.handles.bots.as_ref(),
                    op,
                    &req.params,
                )
                .await
            }
            "orchestration" => {
                orch_worktree_routes::dispatch_orchestration_namespace(&self.handles, op, &req.params)
            }
            "agent" => orch_worktree_routes::dispatch_agent_catalog_namespace(op, &req.params),
            "agentSession" => orch_worktree_routes::dispatch_agent_session_namespace(
                &self.handles,
                op,
                &req.params,
                req.operation_id.as_deref(),
            ),
            "nativeChat" | "native_chat" => {
                orch_worktree_routes::dispatch_native_chat_namespace(&self.handles, op, &req.params)
            }
            "agentStatus" | "agent_status" => {
                ide_routes::dispatch_agent_status_namespace(&self.handles, op, &req.params).await
            }
            "terminal" => {
                orch_worktree_routes::dispatch_terminal_namespace(&self.handles, op, &req.params)
            }
            "worktree" => {
                orch_worktree_routes::dispatch_worktree_namespace(&self.handles, op, &req.params)
            }
            "projectGroup" | "project_group" => {
                orch_worktree_routes::dispatch_project_group_namespace(&self.handles, op, &req.params)
            }
            "folderWorkspace" | "folder_workspace" => {
                orch_worktree_routes::dispatch_folder_workspace_namespace(
                    &self.handles,
                    op,
                    &req.params,
                )
            }
            "workspacePorts" | "workspace_ports" => {
                orch_worktree_routes::dispatch_workspace_ports_namespace(
                    &self.handles,
                    op,
                    &req.params,
                )
                .await
            }
            "workspaceCleanup" | "workspace_cleanup" => {
                orch_worktree_routes::dispatch_workspace_cleanup_namespace(
                    &self.handles,
                    op,
                    &req.params,
                )
            }
            "layout" => {
                orch_worktree_routes::dispatch_layout_namespace(&self.handles, op, &req.params)
            }
            "jumpPalette" | "jump_palette" => {
                orch_worktree_routes::dispatch_jump_palette_namespace(&self.handles, op, &req.params)
            }
            "aiVault" | "ai_vault" => {
                ide_routes::dispatch_ai_vault_namespace(&self.handles, op, &req.params)
            }
            "diff" | "diffReview" | "diff_review" => {
                ide_routes::dispatch_diff_namespace(&self.handles, op, &req.params)
            }
            "git" => ide_routes_ext::dispatch_git_namespace(&self.handles, op, &req.params),
            "tasksHub" | "tasks_hub" => {
                ide_routes_ext::dispatch_tasks_hub_namespace(&self.handles, op, &req.params)
            }
            "browser" => {
                ide_routes_ext::dispatch_browser_namespace(&self.handles, op, &req.params)
            }
            "plugins" => {
                ide_routes::dispatch_plugins_namespace(&self.handles, op, &req.params).await
            }
            "skills" => ide_routes_ext::dispatch_skills_namespace(&self.handles, op, &req.params),
            "computer" => {
                ide_routes::dispatch_computer_namespace(&self.handles, op, &req.params).await
            }
            "emulator" => ide_routes::dispatch_emulator_namespace(&self.handles, op, &req.params),
            "artifacts" => ide_routes::dispatch_artifacts_namespace(
                &self.handles,
                op,
                &req.params,
                authenticated_profile,
            ),
            "accounts" => ide_routes::dispatch_accounts_namespace(&self.handles, op, &req.params),
            "usage" => ide_routes::dispatch_usage_namespace(&self.handles, op, &req.params),
            "hibernation" => {
                ide_routes::dispatch_hibernation_namespace(&self.handles, op, &req.params)
            }
            "e2ee" => ide_routes::dispatch_e2ee_namespace(&self.handles, op, &req.params),
            "profile" | "profileState" | "profile_state" => {
                ide_routes::dispatch_profile_namespace(&self.handles, op, &req.params)
            }
            "vm" => ide_routes::dispatch_vm_namespace(&self.handles, op, &req.params),
            "serve" => ide_routes::dispatch_serve_namespace(&self.handles, op, &req.params),
            "speech" => ide_routes::dispatch_speech_namespace(&self.handles, op, &req.params),
            "watchdog" => ide_routes::dispatch_watchdog_namespace(&self.handles, op, &req.params),
            other => Err(OrcaIdeRuntimeError::not_found(format!(
                "unknown RPC namespace '{other}' in method '{}'",
                req.method
            ))),
        }
    }
}

#[cfg(test)]
mod caller_scope_tests {
    use super::*;

    #[test]
    fn host_mcp_toggle_and_server_mutations_blocked_for_non_host_scopes() {
        let privileged = [
            "bots.tools.setHostMcpServers",
            "bot.tools.setHostMcpServers",
            "bots.tools.set_host_mcp_servers",
            "bot.tools.set_host_mcp_servers",
            "mcpServers.add",
            "mcpServers.update",
            "mcpServers.remove",
            "mcpServers.sync",
            "mcp_servers.add",
        ];
        for method in privileged {
            assert!(check_caller_scope(CallerScope::Desktop, method).is_ok());
            assert!(check_caller_scope(CallerScope::Cli, method).is_ok());
            assert!(
                check_caller_scope(CallerScope::Bot, method).is_err(),
                "Bot scope must be denied '{method}'"
            );
            assert!(
                check_caller_scope(CallerScope::Relay, method).is_err(),
                "Relay scope must be denied '{method}'"
            );
            assert!(
                check_caller_scope(CallerScope::RemoteHost, method).is_err(),
                "RemoteHost scope must be denied '{method}'"
            );
        }
    }
}
