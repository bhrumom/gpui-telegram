//! Unified Runtime RPC v3 Dispatcher & E2EE v2 Session Manager (`F20`).
//!
//! Validates `protocol_version` against `RUNTIME_PROTOCOL_VERSION = 3` (min 2) via
//! `negotiate_runtime_protocol` / `negotiate_runtime_capabilities`, enforces `CallerScope`
//! access control, deduplicates mutating operations via `OperationIdDedupRing`, and routes
//! all 18+ namespaces across `orca_ide` and optional `orca_bots::OrcaBotsFacade`.

mod bots_routes;
pub mod e2ee_manager;
mod ide_routes;
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
use orca_core::workspace_layout::WorkspaceLayoutModel;
use parking_lot::{Mutex, RwLock};

use crate::accounts_usage::{AccountHomeManager, UsageRosterManager};
use crate::agent_status_hooks::AgentStatusEngine;
use crate::chat_journal::ChatJournalStore;
use crate::diff_review::{PersistentDiffReviewStore, TasksHubStore};
use crate::hibernation::HibernationController;
use crate::orchestration_engine::OrchestrationEngine;
use crate::plugin_host::{PluginHostV2, TransactionalSkillInstaller};
use crate::ports_proxy::ListeningPortScanner;
use crate::profile_state::ProfileStateAuthority;
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
    pub bots: Option<Arc<OrcaBotsFacade>>,
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

fn check_caller_scope(scope: CallerScope, canonical_method: &str) -> Result<()> {
    match scope {
        CallerScope::Desktop | CallerScope::Cli => Ok(()),
        CallerScope::Bot => {
            if matches!(
                canonical_method,
                "plugins.approve"
                    | "plugins.install"
                    | "accounts.swap"
                    | "webhooks.create"
                    | "webhook.create"
                    | "e2ee.handshake"
            ) {
                return Err(OrcaIdeRuntimeError::permission_denied(format!(
                    "caller_scope 'bot' is not authorized for privileged method '{canonical_method}'"
                )));
            }
            Ok(())
        }
        CallerScope::Relay => {
            if canonical_method.starts_with("computer.")
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
            if matches!(canonical_method, "plugins.approve" | "accounts.swap") {
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
            .map(ToOwned::to_owned);

        if let Some(ref id) = op_id
            && let Some(cached) = self.dedup_ring.lock().get(id).cloned()
        {
            return cached;
        }

        let canonical_method = expand_ide_alias(req.method.trim());
        let response = match self.execute_method(canonical_method, &req).await {
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
                bots_routes::dispatch_bots_namespace(self.handles.bots.as_ref(), op, &req.params)
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
            "agentSession" => {
                orch_worktree_routes::dispatch_agent_session_namespace(&self.handles, op, &req.params)
            }
            "terminal" => {
                orch_worktree_routes::dispatch_terminal_namespace(&self.handles, op, &req.params)
            }
            "worktree" => {
                orch_worktree_routes::dispatch_worktree_namespace(&self.handles, op, &req.params)
            }
            "layout" => {
                orch_worktree_routes::dispatch_layout_namespace(&self.handles, op, &req.params)
            }
            "aiVault" => ide_routes::dispatch_ai_vault_namespace(&self.handles, op, &req.params),
            "diff" => ide_routes::dispatch_diff_namespace(&self.handles, op, &req.params),
            "plugins" => ide_routes::dispatch_plugins_namespace(&self.handles, op, &req.params),
            "computer" => ide_routes::dispatch_computer_namespace(&self.handles, op, &req.params),
            "emulator" => ide_routes::dispatch_emulator_namespace(&self.handles, op, &req.params),
            "artifacts" => ide_routes::dispatch_artifacts_namespace(&self.handles, op, &req.params),
            "accounts" => ide_routes::dispatch_accounts_namespace(&self.handles, op, &req.params),
            "usage" => ide_routes::dispatch_usage_namespace(&self.handles, op, &req.params),
            "hibernation" => {
                ide_routes::dispatch_hibernation_namespace(&self.handles, op, &req.params)
            }
            "e2ee" => ide_routes::dispatch_e2ee_namespace(&self.handles, op, &req.params),
            "profile" => ide_routes::dispatch_profile_namespace(&self.handles, op, &req.params),
            "vm" => ide_routes::dispatch_vm_namespace(&self.handles, op, &req.params),
            "speech" => ide_routes::dispatch_speech_namespace(&self.handles, op, &req.params),
            "watchdog" => ide_routes::dispatch_watchdog_namespace(&self.handles, op, &req.params),
            other => Err(OrcaIdeRuntimeError::not_found(format!(
                "unknown RPC namespace '{other}' in method '{}'",
                req.method
            ))),
        }
    }
}
