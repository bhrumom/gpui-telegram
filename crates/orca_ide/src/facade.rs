//! Unified `Send + Sync` runtime facade (`OrcaIdeFacade`) owning and coordinating all
//! M3.1, M3.2, and M3.3 subsystems alongside optional `Arc<orca_bots::OrcaBotsFacade>`
//! and [`UnifiedRpcDispatcher`].

use std::collections::BTreeMap;
use std::fmt;
use std::fs;
use std::path::PathBuf;
use std::sync::Arc;

use orca_bots::OrcaBotsFacade;
use orca_core::LOCAL_EXECUTION_HOST_ID;
use orca_core::orchestration_protocol::{RpcRequestEnvelope, RpcResponseEnvelope};
use orca_core::workspace_layout::WorkspaceLayoutModel;
use parking_lot::RwLock;
use serde::{Deserialize, Serialize};

use crate::accounts_usage::{AccountHomeManager, UsageRosterManager};
use crate::agent_catalog::AgentCatalog;
use crate::agent_status_hooks::AgentStatusEngine;
use crate::chat_journal::ChatJournalStore;
use crate::diff_review::{PersistentDiffReviewStore, TasksHubStore};
use crate::hibernation::HibernationController;
use crate::orchestration_engine::OrchestrationEngine;
use crate::plugin_host::{PluginHostV2, SkillRecoveryReport, TransactionalSkillInstaller};
use crate::ports_proxy::ListeningPortScanner;
use crate::profile_state::{
    InMemoryKeyringProvider, KeyringMasterKeyProvider, ProfileStateAuthority,
    ProtectedSecretPersistence,
};
use crate::pty_multiplex::{MultiplexConfig, PtyMultiplexController};
use crate::rpc_dispatcher::{
    DEFAULT_RPC_DEDUP_CAPACITY, E2eeV2SessionManager, RpcSubsystemHandles, UnifiedRpcDispatcher,
};
use crate::system_services::{
    ComputerUseService, EphemeralVmService, MobileEmulatorService, ShareableArtifactStore,
    SleepAwareHangWatchdog, SttDictationService,
};
use crate::vault_search::AiVaultSearchEngine;
use crate::worktree_manager::{RemovalRecoveryReport, WorktreeManager};
use crate::{OrcaIdeRuntimeError, Result, now_epoch_ms};

/// Configuration for initializing [`OrcaIdeFacade`].
#[derive(Clone)]
pub struct OrcaIdeConfig {
    pub data_dir: PathBuf,
    pub workspaces_root: PathBuf,
    pub host_id: String,
    pub plugins_enabled: bool,
    pub artifact_base_url: String,
    pub rpc_dedup_capacity: usize,
    pub keyring: Option<Arc<dyn KeyringMasterKeyProvider>>,
}

impl fmt::Debug for OrcaIdeConfig {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("OrcaIdeConfig")
            .field("data_dir", &self.data_dir)
            .field("workspaces_root", &self.workspaces_root)
            .field("host_id", &self.host_id)
            .field("plugins_enabled", &self.plugins_enabled)
            .field("artifact_base_url", &self.artifact_base_url)
            .field("rpc_dedup_capacity", &self.rpc_dedup_capacity)
            .finish()
    }
}

impl OrcaIdeConfig {
    /// Creates a standard production configuration with `host_id = "local"`.
    pub fn new(data_dir: impl Into<PathBuf>, workspaces_root: impl Into<PathBuf>) -> Self {
        Self {
            data_dir: data_dir.into(),
            workspaces_root: workspaces_root.into(),
            host_id: LOCAL_EXECUTION_HOST_ID.to_string(),
            plugins_enabled: true,
            artifact_base_url: "https://share.orca.local".to_string(),
            rpc_dedup_capacity: DEFAULT_RPC_DEDUP_CAPACITY,
            keyring: None,
        }
    }

    /// Creates a self-contained isolated configuration rooted under `root_dir`.
    pub fn isolated(root_dir: impl Into<PathBuf>) -> Self {
        let root = root_dir.into();
        Self::new(root.join("ide_data"), root.join("workspaces"))
    }
}

/// Summary returned by [`OrcaIdeFacade::sync_startup`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct IdeStartupSyncReport {
    pub worktree_removals: RemovalRecoveryReport,
    pub skill_installs: SkillRecoveryReport,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bots_access_recovered: Option<usize>,
}

/// Unified `Send + Sync` facade owning all `orca_ide` runtime engines, optional
/// `Arc<OrcaBotsFacade>`, E2EE v2 session manager, layout models, and [`UnifiedRpcDispatcher`].
pub struct OrcaIdeFacade {
    config: OrcaIdeConfig,
    agent_catalog: AgentCatalog,
    secrets: Arc<ProtectedSecretPersistence>,
    handles: Arc<RpcSubsystemHandles>,
    rpc_dispatcher: UnifiedRpcDispatcher,
}

impl fmt::Debug for OrcaIdeFacade {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("OrcaIdeFacade")
            .field("host_id", &self.config.host_id)
            .field("data_dir", &self.config.data_dir)
            .field("bots_attached", &self.handles.bots.is_some())
            .finish()
    }
}

impl OrcaIdeFacade {
    /// Opens and wires all persistent stores and runtime engines under `config`, optionally
    /// attaching an [`OrcaBotsFacade`] for unified `bots.*`/`groups.*`/`webhooks.*`/`automations.*` RPCs.
    pub fn open(config: OrcaIdeConfig, bots: Option<Arc<OrcaBotsFacade>>) -> Result<Self> {
        if config.host_id.trim().is_empty() {
            return Err(OrcaIdeRuntimeError::invalid_argument(
                "OrcaIdeConfig.host_id must be non-empty",
            ));
        }
        fs::create_dir_all(&config.data_dir)?;
        fs::create_dir_all(&config.workspaces_root)?;

        let keyring: Arc<dyn KeyringMasterKeyProvider> = match &config.keyring {
            Some(k) => k.clone(),
            None => Arc::new(InMemoryKeyringProvider::generate_available()?),
        };
        let secrets = Arc::new(ProtectedSecretPersistence::open(
            config.data_dir.join("protected_secrets.json"),
            keyring,
        )?);

        let orchestration = Arc::new(OrchestrationEngine::open(
            config.data_dir.join("orchestration.db"),
        )?);
        let chat_journal = Arc::new(ChatJournalStore::open(
            config.data_dir.join("chat_journal.db"),
        )?);
        let agent_status = Arc::new(AgentStatusEngine::new()?);
        let worktrees = Arc::new(WorktreeManager::open(config.data_dir.join("worktrees"))?);
        let port_scanner = Arc::new(ListeningPortScanner::new());
        let pty = Arc::new(PtyMultiplexController::open(
            config.data_dir.join("pty"),
            "default",
            config.host_id.clone(),
            "main",
            MultiplexConfig::default(),
        )?);
        let hibernation = Arc::new(HibernationController::default_controller());
        let accounts = Arc::new(AccountHomeManager::open(config.data_dir.join("accounts"))?);
        let usage_roster = Arc::new(UsageRosterManager::new());
        let profile_state = Arc::new(ProfileStateAuthority::open(
            config.data_dir.join("profile_state.db"),
        )?);
        let vault_search = Arc::new(AiVaultSearchEngine::open(
            config.data_dir.join("ai_vault_fts5.db"),
        )?);
        let diff_review = Arc::new(PersistentDiffReviewStore::open(
            config.data_dir.join("diff_review.db"),
        )?);
        let tasks_hub = Arc::new(TasksHubStore::open(config.data_dir.join("tasks_hub.db"))?);
        let plugin_host = Arc::new(PluginHostV2::open(
            config.data_dir.join("plugins_v2.db"),
            config.plugins_enabled,
        )?);
        let skill_installer = Arc::new(TransactionalSkillInstaller::open(
            config.data_dir.join("skill_installs.db"),
        )?);
        let computer_use = Arc::new(ComputerUseService::default());
        let mobile_emulator = Arc::new(MobileEmulatorService::default());
        let stt = Arc::new(SttDictationService::open(
            config.data_dir.join("stt"),
            None,
            None,
        )?);
        let artifacts = Arc::new(ShareableArtifactStore::open(
            config.data_dir.join("artifacts.db"),
            secrets.clone(),
            config.artifact_base_url.clone(),
        )?);
        let ephemeral_vm = Arc::new(EphemeralVmService::new());
        let watchdog = Arc::new(SleepAwareHangWatchdog::default());
        let e2ee = Arc::new(E2eeV2SessionManager::generate()?);

        let mut initial_layouts = BTreeMap::new();
        initial_layouts.insert(
            "default".to_string(),
            WorkspaceLayoutModel::new("default", config.host_id.clone(), "main"),
        );
        let layouts = Arc::new(RwLock::new(initial_layouts));

        let handles = Arc::new(RpcSubsystemHandles {
            host_id: config.host_id.clone(),
            orchestration,
            chat_journal,
            agent_status,
            worktrees,
            port_scanner,
            pty,
            hibernation,
            accounts,
            usage_roster,
            profile_state,
            vault_search,
            diff_review,
            tasks_hub,
            plugin_host,
            skill_installer,
            computer_use,
            mobile_emulator,
            stt,
            artifacts,
            ephemeral_vm,
            watchdog,
            e2ee,
            layouts,
            bots,
        });
        let rpc_dispatcher = UnifiedRpcDispatcher::new(handles.clone(), config.rpc_dedup_capacity);

        Ok(Self {
            config,
            agent_catalog: AgentCatalog::new(),
            secrets,
            handles,
            rpc_dispatcher,
        })
    }

    /// Opens a standalone `OrcaIdeFacade` without an attached `OrcaBotsFacade`.
    pub fn open_standalone(config: OrcaIdeConfig) -> Result<Self> {
        Self::open(config, None)
    }

    /// Opens an `OrcaIdeFacade` wired directly to an `Arc<OrcaBotsFacade>`.
    pub fn open_with_bots(config: OrcaIdeConfig, bots: Arc<OrcaBotsFacade>) -> Result<Self> {
        Self::open(config, Some(bots))
    }

    pub fn config(&self) -> &OrcaIdeConfig {
        &self.config
    }

    pub fn orchestration(&self) -> &OrchestrationEngine {
        &self.handles.orchestration
    }

    pub fn chat_journal(&self) -> &ChatJournalStore {
        &self.handles.chat_journal
    }

    pub fn agent_catalog(&self) -> &AgentCatalog {
        &self.agent_catalog
    }

    pub fn agent_status(&self) -> &AgentStatusEngine {
        &self.handles.agent_status
    }

    pub fn worktrees(&self) -> &WorktreeManager {
        &self.handles.worktrees
    }

    pub fn port_scanner(&self) -> &ListeningPortScanner {
        &self.handles.port_scanner
    }

    pub fn pty(&self) -> &PtyMultiplexController {
        &self.handles.pty
    }

    pub fn hibernation(&self) -> &HibernationController {
        &self.handles.hibernation
    }

    pub fn accounts(&self) -> &AccountHomeManager {
        &self.handles.accounts
    }

    pub fn usage_roster(&self) -> &UsageRosterManager {
        &self.handles.usage_roster
    }

    pub fn profile_state(&self) -> &ProfileStateAuthority {
        &self.handles.profile_state
    }

    pub fn secrets(&self) -> &ProtectedSecretPersistence {
        &self.secrets
    }

    pub fn vault_search(&self) -> &AiVaultSearchEngine {
        &self.handles.vault_search
    }

    pub fn diff_review(&self) -> &PersistentDiffReviewStore {
        &self.handles.diff_review
    }

    pub fn tasks_hub(&self) -> &TasksHubStore {
        &self.handles.tasks_hub
    }

    pub fn plugin_host(&self) -> &PluginHostV2 {
        &self.handles.plugin_host
    }

    pub fn skill_installer(&self) -> &TransactionalSkillInstaller {
        &self.handles.skill_installer
    }

    pub fn computer_use(&self) -> &ComputerUseService {
        &self.handles.computer_use
    }

    pub fn mobile_emulator(&self) -> &MobileEmulatorService {
        &self.handles.mobile_emulator
    }

    pub fn stt(&self) -> &SttDictationService {
        &self.handles.stt
    }

    pub fn artifacts(&self) -> &ShareableArtifactStore {
        &self.handles.artifacts
    }

    pub fn ephemeral_vm(&self) -> &EphemeralVmService {
        &self.handles.ephemeral_vm
    }

    pub fn watchdog(&self) -> &SleepAwareHangWatchdog {
        &self.handles.watchdog
    }

    pub fn e2ee(&self) -> &E2eeV2SessionManager {
        &self.handles.e2ee
    }

    pub fn layouts(&self) -> &RwLock<BTreeMap<String, WorkspaceLayoutModel>> {
        &self.handles.layouts
    }

    pub fn bots(&self) -> Option<&Arc<OrcaBotsFacade>> {
        self.handles.bots.as_ref()
    }

    pub fn rpc_dispatcher(&self) -> &UnifiedRpcDispatcher {
        &self.rpc_dispatcher
    }

    /// Dispatches a Unified Runtime RPC v3 request across all `orca_ide` and `orca_bots` namespaces.
    pub async fn dispatch_rpc(&self, req: RpcRequestEnvelope) -> RpcResponseEnvelope {
        self.rpc_dispatcher.dispatch(req).await
    }

    /// Runs startup crash-recovery sweeps across worktree removals, transactional skill
    /// installs, and (if attached) `OrcaBotsFacade::sync_startup`.
    pub async fn sync_startup(&self) -> Result<IdeStartupSyncReport> {
        let now_ms = now_epoch_ms();
        let worktree_removals = self.handles.worktrees.recover_interrupted_removals(now_ms)?;
        let skill_installs = self
            .handles
            .skill_installer
            .recover_interrupted_installs(now_ms)?;
        let bots_access_recovered = if let Some(ref bots) = self.handles.bots {
            let report = bots.sync_startup().await?;
            Some(report.recovered_request_ids.len())
        } else {
            None
        };
        Ok(IdeStartupSyncReport {
            worktree_removals,
            skill_installs,
            bots_access_recovered,
        })
    }

    /// Gracefully shuts down attached runtime subsystems.
    pub async fn shutdown(&self) -> Result<()> {
        if let Some(ref bots) = self.handles.bots {
            bots.shutdown().await?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::rpc_dispatcher::E2eeHandshakeServerResponse;
    use base64::Engine as _;
    use base64::engine::general_purpose::STANDARD as BASE64_STD;
    use orca_bots::OrcaBotsConfig;
    use orca_core::orchestration_protocol::{
        CallerScope, E2eeTransportKind, E2eeV2Frame, generate_nonce_32, generate_x25519_keypair,
    };
    use serde_json::json;
    use tempfile::TempDir;

    fn assert_send_sync<T: Send + Sync>() {}

    fn rpc_req(
        method: &str,
        params: serde_json::Value,
        version: u32,
        scope: CallerScope,
        op_id: Option<&str>,
    ) -> RpcRequestEnvelope {
        RpcRequestEnvelope {
            method: method.to_string(),
            params,
            protocol_version: version,
            caller_scope: scope,
            operation_id: op_id.map(ToOwned::to_owned),
        }
    }

    #[tokio::test]
    async fn facade_is_send_sync_protocol_v3_dedup_e2ee_and_cross_domain_rpc() {
        assert_send_sync::<OrcaIdeFacade>();
        assert_send_sync::<UnifiedRpcDispatcher>();
        assert_send_sync::<E2eeV2SessionManager>();

        let dir = TempDir::new().unwrap();
        let bots = Arc::new(
            OrcaBotsFacade::open(OrcaBotsConfig::isolated(dir.path().join("bots_env"))).unwrap(),
        );
        let ide = OrcaIdeFacade::open_with_bots(
            OrcaIdeConfig::isolated(dir.path().join("ide_env")),
            bots,
        )
        .unwrap();

        let startup = ide.sync_startup().await.unwrap();
        assert_eq!(startup.bots_access_recovered, Some(0));

        // 1. Protocol v3 negotiation: accepts 2 & 3, rejects 1.
        let v1_err = ide
            .dispatch_rpc(rpc_req("runtime.health", json!({}), 1, CallerScope::Desktop, None))
            .await;
        assert!(!v1_err.ok);
        assert_eq!(v1_err.error.as_ref().unwrap().code, "protocol_version");

        let v2_ok = ide
            .dispatch_rpc(rpc_req("runtime.negotiate", json!({}), 2, CallerScope::Desktop, None))
            .await;
        assert!(v2_ok.ok);
        assert_eq!(v2_ok.result.as_ref().unwrap()["negotiated_version"], 2);

        let v3_ok = ide
            .dispatch_rpc(rpc_req("runtime.negotiate", json!({}), 3, CallerScope::Desktop, None))
            .await;
        assert!(v3_ok.ok);
        assert_eq!(v3_ok.result.as_ref().unwrap()["negotiated_version"], 3);

        // 2. CallerScope ACL check: Bot cannot approve plugins.
        let acl_err = ide
            .dispatch_rpc(rpc_req(
                "plugins.approve",
                json!({"plugin_id": "x", "fingerprint": "abc"}),
                3,
                CallerScope::Bot,
                None,
            ))
            .await;
        assert!(!acl_err.ok);
        assert_eq!(acl_err.error.as_ref().unwrap().code, "permission_denied");

        // 3. `operation_id` idempotency dedup across `bots.create`!
        let c1 = ide
            .dispatch_rpc(rpc_req(
                "bots.create",
                json!({"name": "AlphaBot", "agent": "claude"}),
                3,
                CallerScope::Desktop,
                Some("op-bot-create-1"),
            ))
            .await;
        assert!(c1.ok, "bots.create failed: {:?}", c1.error);
        let c2 = ide
            .dispatch_rpc(rpc_req(
                "bots.create",
                json!({"name": "AlphaBot", "agent": "claude"}),
                3,
                CallerScope::Desktop,
                Some("op-bot-create-1"),
            ))
            .await;
        assert_eq!(c1, c2);
        let bot_id = c1.result.as_ref().unwrap()["id"].as_str().unwrap().to_string();

        let listed_bots = ide
            .dispatch_rpc(rpc_req("bots.list", json!({}), 3, CallerScope::Desktop, None))
            .await;
        assert!(listed_bots.ok);
        assert_eq!(listed_bots.result.as_ref().unwrap().as_array().unwrap().len(), 1);

        // 4. Webhooks create + list: NEVER leaks URL or signing secret!
        let wh_create = ide
            .dispatch_rpc(rpc_req(
                "webhooks.create",
                json!({"bot_id": bot_id, "automation_id": "auto-1", "adapter": "github"}),
                3,
                CallerScope::Desktop,
                None,
            ))
            .await;
        assert!(wh_create.ok, "webhooks.create failed: {:?}", wh_create.error);
        let wh_list = ide
            .dispatch_rpc(rpc_req("webhooks.list", json!({}), 3, CallerScope::Desktop, None))
            .await;
        assert!(wh_list.ok);
        let wh_raw = serde_json::to_string(&wh_list.result).unwrap();
        assert!(!wh_raw.contains("secret_key"));
        assert!(!wh_raw.contains("ingest_url"));

        // 5. Real E2EE v2 handshake + bidirectional encrypt/decrypt + replay rejection!
        let (client_sec, client_pub) = generate_x25519_keypair().unwrap();
        let client_nonce = generate_nonce_32().unwrap();
        let hs_env = ide
            .dispatch_rpc(rpc_req(
                "e2ee.handshake",
                json!({
                    "transport": "direct",
                    "client_ephemeral_pub_b64": BASE64_STD.encode(client_pub),
                    "client_nonce_b64": BASE64_STD.encode(client_nonce),
                    "protocol_version": 3
                }),
                3,
                CallerScope::Desktop,
                None,
            ))
            .await;
        assert!(hs_env.ok, "e2ee.handshake failed: {:?}", hs_env.error);
        let hs_resp: E2eeHandshakeServerResponse =
            serde_json::from_value(hs_env.result.unwrap()).unwrap();
        let mut client_sess = E2eeV2SessionManager::complete_client_handshake(
            &client_sec,
            client_pub,
            client_nonce,
            E2eeTransportKind::Direct,
            None,
            &hs_resp,
        )
        .unwrap();

        let c2s_frame = client_sess.encrypt_frame(b"hello-orca-e2ee-v2").unwrap();
        let dec_ok = ide
            .dispatch_rpc(rpc_req(
                "e2ee.decrypt",
                json!({"session_id": hs_resp.session_id, "frame": c2s_frame}),
                3,
                CallerScope::Desktop,
                None,
            ))
            .await;
        assert!(dec_ok.ok);
        assert_eq!(dec_ok.result.as_ref().unwrap()["plaintext_utf8"], "hello-orca-e2ee-v2");

        // Replaying the exact same frame MUST fail!
        let replay_err = ide
            .dispatch_rpc(rpc_req(
                "e2ee.decrypt",
                json!({"session_id": hs_resp.session_id, "frame": c2s_frame}),
                3,
                CallerScope::Desktop,
                None,
            ))
            .await;
        assert!(!replay_err.ok);

        // Server -> Client encrypt round-trip.
        let s2c_env = ide
            .dispatch_rpc(rpc_req(
                "e2ee.encrypt",
                json!({"session_id": hs_resp.session_id, "plaintext": "server-ack"}),
                3,
                CallerScope::Desktop,
                None,
            ))
            .await;
        assert!(s2c_env.ok);
        let s2c_frame: E2eeV2Frame = serde_json::from_value(s2c_env.result.unwrap()).unwrap();
        assert_eq!(client_sess.decrypt_frame(&s2c_frame).unwrap(), b"server-ack");

        // 6. End-to-end IDE domains via RPC + ide.* aliases!
        let run = ide
            .dispatch_rpc(rpc_req(
                "orchestration.run_create",
                json!({"title": "M3 Verification Run"}),
                3,
                CallerScope::Desktop,
                None,
            ))
            .await;
        assert!(run.ok);

        let yaml = ide
            .dispatch_rpc(rpc_req(
                "worktree.parse_yaml",
                json!({"yaml": "setup:\n  - cargo check\nports:\n  - 3000\n"}),
                3,
                CallerScope::Desktop,
                None,
            ))
            .await;
        assert!(yaml.ok);

        let prof_put = ide
            .dispatch_rpc(rpc_req(
                "ide.profile_put",
                json!({"domain": "ui", "payload": {"density": "compact"}, "expected_revision": 0}),
                3,
                CallerScope::Desktop,
                None,
            ))
            .await;
        assert!(prof_put.ok);
        let prof_get = ide
            .dispatch_rpc(rpc_req(
                "ide.profile_get",
                json!({"domain": "ui"}),
                3,
                CallerScope::Desktop,
                None,
            ))
            .await;
        assert_eq!(prof_get.result.as_ref().unwrap()["revision"], 1);

        let idx = ide
            .dispatch_rpc(rpc_req(
                "aiVault.index",
                json!({
                    "session_id": "s-rpc-1",
                    "agent_id": "claude",
                    "workspace_id": "ws-rpc",
                    "messages": [{"role": "user", "content": "Review parseOrcaYamlConfig"}]
                }),
                3,
                CallerScope::Desktop,
                None,
            ))
            .await;
        assert!(idx.ok);
        let srch = ide
            .dispatch_rpc(rpc_req(
                "ide.vault_search",
                json!({"query": "yaml"}),
                3,
                CallerScope::Desktop,
                None,
            ))
            .await;
        assert_eq!(srch.result.as_ref().unwrap()["hits"].as_array().unwrap().len(), 1);

        let diff_ann = ide
            .dispatch_rpc(rpc_req(
                "ide.diff_annotate",
                json!({"file_path": "lib.rs", "line_start": 4, "body": "Check error code"}),
                3,
                CallerScope::Desktop,
                None,
            ))
            .await;
        assert!(diff_ann.ok);
        let diff_batch = ide
            .dispatch_rpc(rpc_req("ide.diff_batch", json!({}), 3, CallerScope::Desktop, None))
            .await;
        assert!(diff_batch.result.as_ref().unwrap()["prompt"].as_str().unwrap().contains("lib.rs:L4"));

        let plug_val = ide
            .dispatch_rpc(rpc_req(
                "ide.plugin_validate",
                json!({
                    "id": "demo-theme",
                    "version": "1.0.0",
                    "schema_version": 2,
                    "tier": "data",
                    "sha256": "a".repeat(64)
                }),
                3,
                CallerScope::Desktop,
                None,
            ))
            .await;
        assert!(plug_val.ok);

        let comp_err = ide
            .dispatch_rpc(rpc_req(
                "ide.computer_guard",
                json!({
                    "target_id": "win-1",
                    "target_kind": "password_field",
                    "action": {"action": "type_text", "text_len": 4, "via_stdin": true}
                }),
                3,
                CallerScope::Desktop,
                None,
            ))
            .await;
        assert!(!comp_err.ok);
        assert_eq!(comp_err.error.as_ref().unwrap().code, "emulator_input_blocked");

        let art = ide
            .dispatch_rpc(rpc_req(
                "ide.artifact_share",
                json!({
                    "title": "Design Doc",
                    "kind": "markdown",
                    "content": "# Hello",
                    "user_confirmed_share": true
                }),
                3,
                CallerScope::Desktop,
                None,
            ))
            .await;
        assert!(art.ok);
        assert!(!serde_json::to_string(&art.result).unwrap().contains("edit_token"));

        let hib = ide
            .dispatch_rpc(rpc_req(
                "ide.hibernation_check",
                json!({
                    "tab_id": "tab-1",
                    "agent_state_is_done": true,
                    "tab_visible": false,
                    "keystrokes_since_done": 0,
                    "agent_id": "claude",
                    "idle_duration_ms": 2_000_000,
                    "mobile_driver_attached": false,
                    "unsettled_orchestration_dispatch": false,
                    "live_subagent_count": 0,
                    "git_operation_in_progress": false,
                    "active_port_forward": false,
                    "pending_bot_approval": false,
                    "unsynced_local_snapshot": false,
                    "pinned_no_hibernate": false,
                    "session_handle": "claude-sess-42",
                    "original_cwd": "/tmp",
                    "original_args": [],
                    "original_env": {}
                }),
                3,
                CallerScope::Desktop,
                None,
            ))
            .await;
        assert!(hib.ok);
        assert_eq!(hib.result.as_ref().unwrap()["decision"], "hibernate_now");

        let usage_res = ide
            .dispatch_rpc(rpc_req("ide.usage", json!({}), 3, CallerScope::Desktop, None))
            .await;
        assert!(usage_res.ok);

        ide.shutdown().await.unwrap();
    }
}
