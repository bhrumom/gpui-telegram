//! Unified `Send + Sync` runtime facade (`OrcaBotsFacade`) orchestrating all `orca_bots`
//! subsystems for M4 (Server / CLI / Telegram fusion) and M5 (GPUI Desktop).

use std::collections::{BTreeMap, HashMap, HashSet};
use std::fs;
use std::path::PathBuf;
use std::sync::{Arc, Mutex as StdMutex};

use base64::Engine as _;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use orca_core::bot_ecosystem::{
    BotChatNode, BotImportCandidate, HandOverOutcome, NativeChatBotCard, UsageLimitContinue,
};
use orca_core::bot_items::{
    BotItem, BotItemActor, BotItemStatus, ItemTransitionOutcome, StatusTransitionRequest,
};
use orca_core::bot_memory::JournalCloseAttribution;
use orca_core::bot_types::OrcaBotRecord;
use orca_core::{BotErrorCode, LOCAL_EXECUTION_HOST_ID};
use serde::{Deserialize, Serialize};
use tokio::sync::Mutex as AsyncMutex;

use crate::connectors_triggers::{
    AccessRecoveryReport, AutomationTriggerEngine, BotAccessHost, SlackMirror, WebhookEngine,
};
use crate::controller::{BotController, CreateBotRequest};
use crate::ecosystem_handover::{
    BotChatTreeBuilder, BotChatTreeView, BotImportDiscoveryRoots, BotImportEngine,
    BotPackageEngine, BotTeamShareEngine, BotTemplateInstaller, ExportPackageOptions,
    ExportedBotPackage, HandOverTarget, ImportedBotPackage, InstalledTemplateResult,
    WorkspaceHandoverCoordinator, WorkspaceHandoverOutcome, WorkspaceLineageAdapter,
};
use crate::group_runtime::BotGroupRuntime;
use crate::home_store::{BotHomeStore, WorktreeReleaseBotHome};
use crate::items_store::{BotItemsStore, ItemCloseJournalTarget};
use crate::look_around::{LookAroundEngine, UpNextStore};
use crate::memory_service::{BotMemoryService, BotTurnMemorySync, TurnMemorySyncResult};
use crate::shared_mcp::{SharedMcpPool, SharedMcpServerConfig};
use crate::tool_oauth::{BotToolOAuth, OAuthProviderConfig, OAuthSecurityPolicy};
use crate::tool_relay::{BotRelayGrant, BotToolRelay, BotToolRelayConfig};
use crate::{OrcaBotsError, Result};

/// Host-level MCP server registry entry shared across F08 (`SharedMcpPool` + loopback relay)
/// and F19 (`mcpServers.sync` agent config sync), supporting both `stdio` and `streamable-http`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HostMcpServerRecord {
    pub name: String,
    #[serde(default = "default_mcp_transport")]
    pub transport: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub command: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub args: Vec<String>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub env: BTreeMap<String, String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cwd: Option<PathBuf>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub credential_handle: Option<String>,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub source: String,
}

fn default_mcp_transport() -> String {
    "stdio".to_string()
}

impl HostMcpServerRecord {
    /// Converts this record into a `SharedMcpServerConfig` when it represents a `stdio` server.
    pub fn as_shared_stdio_config(&self) -> Option<SharedMcpServerConfig> {
        let is_stdio = self.transport.trim().is_empty()
            || self.transport.eq_ignore_ascii_case("stdio");
        let cmd = self.command.as_deref()?.trim();
        if !is_stdio || cmd.is_empty() {
            return None;
        }
        Some(SharedMcpServerConfig {
            command: cmd.to_string(),
            args: self.args.clone(),
            env: self.env.clone(),
            cwd: self.cwd.clone(),
        })
    }
}

/// Host-side metadata for a per-bot registered tool (never stores secrets on bot disk).
#[derive(Debug, Clone)]
pub struct RegisteredBotTool {
    pub bot_id: String,
    pub name: String,
    pub credential_id: String,
    pub transport: String,
    pub upstream_url: String,
    pub oauth_issuer: Option<String>,
    pub oauth_allow_loopback: bool,
    pub oauth_provider: Option<OAuthProviderConfig>,
}

/// Configuration for initializing [`OrcaBotsFacade`].
#[derive(Debug, Clone)]
pub struct OrcaBotsConfig {
    pub data_dir: PathBuf,
    pub bots_root: PathBuf,
    pub host_id: String,
    pub oauth_policy: OAuthSecurityPolicy,
    pub webhook_base_url: String,
    pub import_roots: Option<BotImportDiscoveryRoots>,
}

impl OrcaBotsConfig {
    /// Creates a production-safe config with `host_id = "local"` and `OAuthSecurityPolicy::strict()`.
    pub fn new(data_dir: impl Into<PathBuf>, bots_root: impl Into<PathBuf>) -> Self {
        Self {
            data_dir: data_dir.into(),
            bots_root: bots_root.into(),
            host_id: LOCAL_EXECUTION_HOST_ID.to_string(),
            oauth_policy: OAuthSecurityPolicy::strict(),
            webhook_base_url: "https://hooks.orca.local".to_string(),
            import_roots: None,
        }
    }

    /// Creates a self-contained config rooted under `root_dir` (allowing loopback OAuth for local tests).
    pub fn isolated(root_dir: impl Into<PathBuf>) -> Self {
        let root = root_dir.into();
        Self {
            data_dir: root.join("data"),
            bots_root: root.join("bots"),
            host_id: LOCAL_EXECUTION_HOST_ID.to_string(),
            oauth_policy: OAuthSecurityPolicy::allow_loopback_for_tests(),
            webhook_base_url: "https://hooks.orca.local".to_string(),
            import_roots: Some(BotImportDiscoveryRoots::default_for_home(&root.join("fake_home"))),
        }
    }
}

/// Unified `Send + Sync` facade owning and coordinating all M2.1, M2.2, and M2.3 subsystems.
pub struct OrcaBotsFacade {
    config: OrcaBotsConfig,
    home_store: BotHomeStore,
    controller: BotController,
    memory_service: BotMemoryService,
    items_store: BotItemsStore,
    up_next_store: UpNextStore,
    look_around: LookAroundEngine,
    group_runtime: BotGroupRuntime,
    tool_oauth: BotToolOAuth,
    tool_relay: AsyncMutex<Option<Arc<BotToolRelay>>>,
    shared_mcp: SharedMcpPool,
    access_host: BotAccessHost,
    slack_mirror: SlackMirror,
    webhook_engine: WebhookEngine,
    automation_engine: AutomationTriggerEngine,
    team_share: BotTeamShareEngine,
    import_engine: BotImportEngine,
    handover: WorkspaceHandoverCoordinator,
    host_mcp_servers: StdMutex<BTreeMap<String, HostMcpServerRecord>>,
    bot_tools: StdMutex<HashMap<(String, String), RegisteredBotTool>>,
    bot_grants: StdMutex<HashMap<String, String>>,
    delta_trackers: StdMutex<HashMap<(String, String), BotTurnMemorySync>>,
    usage_limits: StdMutex<HashMap<String, UsageLimitContinue>>,
}

impl std::fmt::Debug for OrcaBotsFacade {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("OrcaBotsFacade")
            .field("host_id", &self.config.host_id)
            .field("data_dir", &self.config.data_dir)
            .field("bots_root", &self.config.bots_root)
            .finish()
    }
}

impl OrcaBotsFacade {
    /// Opens and wires all persistent stores and runtime engines under `config`.
    pub fn open(config: OrcaBotsConfig) -> Result<Self> {
        if config.host_id.trim().is_empty() {
            return Err(OrcaBotsError::bot(
                BotErrorCode::InvalidArgument,
                "OrcaBotsConfig.host_id must be non-empty.",
            ));
        }
        fs::create_dir_all(&config.data_dir)?;
        fs::create_dir_all(&config.bots_root)?;

        let home_store = BotHomeStore::new(&config.bots_root);
        let controller = BotController::open(config.data_dir.join("controller"), home_store.clone())?;
        let memory_service =
            BotMemoryService::new(home_store.clone(), config.data_dir.join("memory_state"));
        let items_store = BotItemsStore::open(config.data_dir.join("items.db"))?;
        items_store.set_memory_service(memory_service.clone())?;

        let up_next_store = UpNextStore::open(config.data_dir.join("up_next"))?;
        let look_around = LookAroundEngine::new(up_next_store.clone());
        let group_runtime =
            BotGroupRuntime::open(config.data_dir.join("groups.db"), &config.bots_root)?;
        let tool_oauth = BotToolOAuth::with_policy(config.oauth_policy);
        let shared_mcp = SharedMcpPool::new();
        let access_host =
            BotAccessHost::open(&config.host_id, config.data_dir.join("access_host"))?;
        let slack_mirror = SlackMirror::new();
        let webhook_engine = WebhookEngine::new(
            config.data_dir.join("webhooks_spool"),
            &config.webhook_base_url,
        )?;
        let automation_engine = AutomationTriggerEngine::new();
        let team_share = BotTeamShareEngine::generate(&config.host_id)?;
        let import_engine = BotImportEngine::new(
            config
                .import_roots
                .clone()
                .unwrap_or_else(BotImportDiscoveryRoots::default_from_env),
        );

        Ok(Self {
            config,
            home_store,
            controller,
            memory_service,
            items_store,
            up_next_store,
            look_around,
            group_runtime,
            tool_oauth,
            tool_relay: AsyncMutex::new(None),
            shared_mcp,
            access_host,
            slack_mirror,
            webhook_engine,
            automation_engine,
            team_share,
            import_engine,
            handover: WorkspaceHandoverCoordinator,
            host_mcp_servers: StdMutex::new(BTreeMap::new()),
            bot_tools: StdMutex::new(HashMap::new()),
            bot_grants: StdMutex::new(HashMap::new()),
            delta_trackers: StdMutex::new(HashMap::new()),
            usage_limits: StdMutex::new(HashMap::new()),
        })
    }

    /// Synchronizes all loaded bots' folder registrations into `items_store` and executes
    /// the `BotAccessHost` startup recovery sweep for any `GrantedNotAdded` access grants.
    pub async fn sync_startup(&self) -> Result<AccessRecoveryReport> {
        for bot in self.controller.list_bots().await {
            self.items_store
                .register_bot_folder(&bot.id, bot.folder.clone())?;
        }
        self.access_host
            .recover_granted_not_added(&self.controller)
            .await
    }

    pub fn config(&self) -> &OrcaBotsConfig {
        &self.config
    }

    pub fn home_store(&self) -> &BotHomeStore {
        &self.home_store
    }

    pub fn controller(&self) -> &BotController {
        &self.controller
    }

    pub fn memory_service(&self) -> &BotMemoryService {
        &self.memory_service
    }

    pub fn items_store(&self) -> &BotItemsStore {
        &self.items_store
    }

    pub fn up_next_store(&self) -> &UpNextStore {
        &self.up_next_store
    }

    pub fn look_around(&self) -> &LookAroundEngine {
        &self.look_around
    }

    pub fn group_runtime(&self) -> &BotGroupRuntime {
        &self.group_runtime
    }

    pub fn tool_oauth(&self) -> &BotToolOAuth {
        &self.tool_oauth
    }

    pub fn shared_mcp(&self) -> &SharedMcpPool {
        &self.shared_mcp
    }

    pub fn access_host(&self) -> &BotAccessHost {
        &self.access_host
    }

    pub fn slack_mirror(&self) -> &SlackMirror {
        &self.slack_mirror
    }

    pub fn webhook_engine(&self) -> &WebhookEngine {
        &self.webhook_engine
    }

    pub fn automation_engine(&self) -> &AutomationTriggerEngine {
        &self.automation_engine
    }

    pub fn team_share(&self) -> &BotTeamShareEngine {
        &self.team_share
    }

    pub fn import_engine(&self) -> &BotImportEngine {
        &self.import_engine
    }

    pub fn handover(&self) -> &WorkspaceHandoverCoordinator {
        &self.handover
    }

    /// Starts (or returns the already-running) loopback `BotToolRelay` bound to `127.0.0.1:0`,
    /// sharing this facade's `BotToolOAuth` manager and `SharedMcpPool`.
    pub async fn start_tool_relay(&self) -> Result<Arc<BotToolRelay>> {
        let mut guard = self.tool_relay.lock().await;
        if let Some(ref relay) = *guard {
            return Ok(relay.clone());
        }
        let cfg = BotToolRelayConfig {
            oauth: self.tool_oauth.clone(),
            shared_mcp: Some(self.shared_mcp.clone()),
            ..Default::default()
        };
        let relay = Arc::new(BotToolRelay::start(cfg).await?);
        if let Ok(servers) = self.host_mcp_servers.lock() {
            for rec in servers.values() {
                if let Some(stdio_cfg) = rec.as_shared_stdio_config() {
                    let _ = relay.set_host_mcp_server(rec.name.clone(), stdio_cfg);
                }
            }
        }
        *guard = Some(relay.clone());
        Ok(relay)
    }

    /// Returns the running loopback `BotToolRelay` if started.
    pub async fn tool_relay(&self) -> Option<Arc<BotToolRelay>> {
        self.tool_relay.lock().await.clone()
    }

    /// Registers or updates a host MCP server record (`stdio` or `streamable-http`),
    /// propagating `stdio` servers to the live loopback relay if started.
    pub async fn upsert_host_mcp_server(&self, record: HostMcpServerRecord) -> Result<HostMcpServerRecord> {
        let name = record.name.trim().to_string();
        if name.is_empty() {
            return Err(OrcaBotsError::bot(
                BotErrorCode::InvalidArgument,
                "MCP server name must be non-empty.",
            ));
        }
        if let Some(stdio_cfg) = record.as_shared_stdio_config() {
            stdio_cfg.validate()?;
            if let Some(relay) = self.tool_relay.lock().await.clone() {
                relay.set_host_mcp_server(name.clone(), stdio_cfg)?;
            }
        } else {
            let url = record.url.as_deref().map(str::trim).unwrap_or("");
            if url.is_empty() {
                return Err(OrcaBotsError::bot(
                    BotErrorCode::InvalidArgument,
                    "MCP server requires either stdio 'command' or streamable-http 'url'.",
                ));
            }
        }
        let mut map = self.host_mcp_servers.lock().map_err(|_| {
            OrcaBotsError::bot(BotErrorCode::InvalidArgument, "host_mcp_servers lock poisoned.")
        })?;
        map.insert(name, record.clone());
        Ok(record)
    }

    /// Removes a host MCP server by `name`, stopping any pooled stdio child process.
    pub async fn remove_host_mcp_server(&self, name: &str) -> Result<bool> {
        let removed = {
            let mut map = self.host_mcp_servers.lock().map_err(|_| {
                OrcaBotsError::bot(BotErrorCode::InvalidArgument, "host_mcp_servers lock poisoned.")
            })?;
            map.remove(name).is_some()
        };
        if let Some(relay) = self.tool_relay.lock().await.clone() {
            let _ = relay.remove_host_mcp_server(name);
        }
        let _ = self.shared_mcp.shutdown_server(name).await;
        Ok(removed)
    }

    /// Lists all registered host MCP server records (`stdio` + `streamable-http`).
    pub fn list_host_mcp_servers(&self) -> Result<Vec<HostMcpServerRecord>> {
        let map = self.host_mcp_servers.lock().map_err(|_| {
            OrcaBotsError::bot(BotErrorCode::InvalidArgument, "host_mcp_servers lock poisoned.")
        })?;
        Ok(map.values().cloned().collect())
    }

    /// Records host-side metadata for a per-bot registered tool.
    pub fn record_bot_tool(&self, tool: RegisteredBotTool) -> Result<()> {
        let mut map = self.bot_tools.lock().map_err(|_| {
            OrcaBotsError::bot(BotErrorCode::InvalidArgument, "bot_tools lock poisoned.")
        })?;
        map.insert((tool.bot_id.clone(), tool.name.clone()), tool);
        Ok(())
    }

    /// Fetches host-side metadata for a per-bot registered tool.
    pub fn get_bot_tool(&self, bot_id: &str, tool_name: &str) -> Result<Option<RegisteredBotTool>> {
        let map = self.bot_tools.lock().map_err(|_| {
            OrcaBotsError::bot(BotErrorCode::InvalidArgument, "bot_tools lock poisoned.")
        })?;
        Ok(map.get(&(bot_id.to_string(), tool_name.to_string())).cloned())
    }

    /// Lists all registered tool names for `bot_id`.
    pub fn list_bot_tools(&self, bot_id: &str) -> Result<Vec<RegisteredBotTool>> {
        let map = self.bot_tools.lock().map_err(|_| {
            OrcaBotsError::bot(BotErrorCode::InvalidArgument, "bot_tools lock poisoned.")
        })?;
        Ok(map
            .values()
            .filter(|t| t.bot_id == bot_id)
            .cloned()
            .collect())
    }

    /// Issues (or refreshes) a CSPRNG loopback relay bearer grant for `bot`, syncing its
    /// `allow_host_mcp_servers` bit directly from `bot.connectors.uses_host_mcp_servers()`.
    pub async fn issue_bot_relay_grant(&self, bot: &OrcaBotRecord) -> Result<(Arc<BotToolRelay>, String)> {
        let relay = self.start_tool_relay().await?;
        let token = {
            let mut grants = self.bot_grants.lock().map_err(|_| {
                OrcaBotsError::bot(BotErrorCode::InvalidArgument, "bot_grants lock poisoned.")
            })?;
            if let Some(existing) = grants.get(&bot.id) {
                existing.clone()
            } else {
                let mut raw = [0u8; 24];
                getrandom::getrandom(&mut raw).map_err(|e| {
                    OrcaBotsError::bot(BotErrorCode::InvalidArgument, format!("CSPRNG failed: {e}"))
                })?;
                let tok = format!("orca_grant_{}", URL_SAFE_NO_PAD.encode(raw));
                grants.insert(bot.id.clone(), tok.clone());
                tok
            }
        };
        let mut allowed_folders = HashSet::new();
        allowed_folders.insert(bot.folder.as_str().to_string());
        relay.register_bot_grant(BotRelayGrant {
            bot_id: bot.id.clone(),
            owner_id: "local-owner".to_string(),
            token: token.clone(),
            folder: bot.folder.as_str().to_string(),
            allowed_folders,
            allowed_tools: HashSet::new(),
            allow_host_mcp_servers: bot.connectors.uses_host_mcp_servers(),
        })?;
        Ok((relay, token))
    }

    /// Toggles `host_mcp_servers` via `BotController::set_host_mcp_servers` and updates any
    /// active relay grant immediately.
    pub async fn set_bot_host_mcp_servers(
        &self,
        bot_id: &str,
        enabled: bool,
    ) -> Result<OrcaBotRecord> {
        let updated = self.controller.set_host_mcp_servers(bot_id, enabled).await?;
        if let Some(relay) = self.tool_relay.lock().await.clone() {
            let _ = relay.set_bot_host_mcp_access(bot_id, enabled);
        }
        Ok(updated)
    }

    /// Computes and advances mid-session SHA-256 delta sync for `(bot.id, session_id)`.
    /// The first call for a session captures the baseline and returns an empty delta.
    pub fn sync_session_memory_delta(
        &self,
        bot: &OrcaBotRecord,
        session_id: &str,
        cli_cmd: Option<&str>,
    ) -> Result<Option<TurnMemorySyncResult>> {
        let mut map = self.delta_trackers.lock().map_err(|_| {
            OrcaBotsError::bot(BotErrorCode::InvalidArgument, "delta_trackers lock poisoned.")
        })?;
        let key = (bot.id.clone(), session_id.to_string());
        if let Some(tracker) = map.get_mut(&key) {
            let delta = tracker.sync_turn_memory_delta(&self.home_store, bot, cli_cmd)?;
            Ok(Some(delta))
        } else {
            let tracker = BotTurnMemorySync::capture(&self.home_store, bot, cli_cmd)?;
            map.insert(key, tracker);
            Ok(None)
        }
    }

    /// Stores or clears per-bot usage-limit auto-continue state.
    pub fn set_usage_limit_continue(
        &self,
        bot_id: &str,
        limit: Option<UsageLimitContinue>,
    ) -> Result<()> {
        let mut map = self.usage_limits.lock().map_err(|_| {
            OrcaBotsError::bot(BotErrorCode::InvalidArgument, "usage_limits lock poisoned.")
        })?;
        match limit {
            Some(v) => {
                map.insert(bot_id.to_string(), v);
            }
            None => {
                map.remove(bot_id);
            }
        }
        Ok(())
    }

    /// Fetches per-bot usage-limit auto-continue state if configured.
    pub fn get_usage_limit_continue(&self, bot_id: &str) -> Result<Option<UsageLimitContinue>> {
        let map = self.usage_limits.lock().map_err(|_| {
            OrcaBotsError::bot(BotErrorCode::InvalidArgument, "usage_limits lock poisoned.")
        })?;
        Ok(map.get(bot_id).cloned())
    }

    /// Creates a bot via `BotController` and registers its home folder in `BotItemsStore`
    /// so closing items automatically appends deterministic monthly journal lines.
    pub async fn create_bot(&self, req: CreateBotRequest) -> Result<OrcaBotRecord> {
        let bot = self.controller.create_bot(req).await?;
        self.items_store
            .register_bot_folder(&bot.id, bot.folder.clone())?;
        Ok(bot)
    }

    /// Deletes a bot safely:
    /// 1. Automatically revokes any active `BotToolRelay` grants (aborting in-flight streams!)
    ///    BEFORE releasing the bot's home directory.
    /// 2. Releases the bot's home directory via `BotController::delete_bot` (ownership-guarded).
    pub async fn delete_bot(&self, bot_id: &str) -> Result<WorktreeReleaseBotHome> {
        if let Some(relay) = self.tool_relay.lock().await.clone() {
            relay.revoke_bot(bot_id)?;
        }
        self.controller.delete_bot(bot_id).await
    }

    /// Closes a `BotItem` (`Done` or `Canceled`) and automatically appends a deterministic
    /// LLM-free monthly journal entry into `memory/journal/YYYY-MM.md` via `BotMemoryService`.
    #[allow(clippy::too_many_arguments)]
    pub async fn close_item(
        &self,
        item_uid: &str,
        actor: BotItemActor,
        status: BotItemStatus,
        close_note: Option<String>,
        expected_version: Option<u64>,
        attribution: Option<JournalCloseAttribution>,
        now_ms: i64,
    ) -> Result<(BotItem, ItemTransitionOutcome)> {
        if !status.is_closed() {
            return Err(OrcaBotsError::bot(
                BotErrorCode::InvalidArgument,
                "close_item requires a closed status (`done` or `canceled`).",
            ));
        }
        let existing = self.items_store.get_item_by_uid(item_uid)?;
        let bot = self.controller.get_bot(&existing.owner_bot_id).await?;
        let journal_target = ItemCloseJournalTarget {
            memory_service: &self.memory_service,
            folder: &bot.folder,
            attribution,
        };
        self.items_store.set_status_with_journal(
            item_uid,
            StatusTransitionRequest {
                actor,
                to_status: status,
                expected_version,
                wait_note: None,
                close_note,
                note: None,
                ask: None,
            },
            now_ms,
            Some(&journal_target),
        )
    }

    /// Installs a bot from one of the 24 built-in templates (disabled schedules + linked skills)
    /// and registers its folder for automatic closed-item journaling.
    pub async fn install_from_template(
        &self,
        template_id: &str,
        custom_name_opt: Option<&str>,
    ) -> Result<InstalledTemplateResult> {
        let installed = BotTemplateInstaller::install_from_template(
            &self.controller,
            &self.home_store,
            template_id,
            custom_name_opt,
        )
        .await?;
        self.items_store
            .register_bot_folder(&installed.bot.id, installed.bot.folder.clone())?;
        Ok(installed)
    }

    /// Exports a bot to a v1 Deflate `.zip` archive after running the 5-class secret scanner.
    pub async fn export_package(
        &self,
        bot_id: &str,
        opts: &ExportPackageOptions,
    ) -> Result<ExportedBotPackage> {
        let bot = self.controller.get_bot(bot_id).await?;
        BotPackageEngine::export_package(&self.home_store, &bot, opts)
    }

    /// Imports a v1 Deflate `.zip` bot package and registers its home folder for journaling.
    pub async fn import_package(
        &self,
        zip_bytes: &[u8],
        custom_name_opt: Option<&str>,
    ) -> Result<ImportedBotPackage> {
        let imported = BotPackageEngine::import_package(
            &self.controller,
            &self.home_store,
            zip_bytes,
            custom_name_opt,
        )
        .await?;
        self.items_store
            .register_bot_folder(&imported.bot.id, imported.bot.folder.clone())?;
        Ok(imported)
    }

    /// Discovers foreign bot candidates across OpenClaw, Hermes, and OpenBot (cap 24/app, redacted).
    pub fn discover_imports(&self, now_ms: i64) -> Result<Vec<BotImportCandidate>> {
        self.import_engine.discover_all(now_ms)
    }

    /// Imports a discovered or generic-export candidate into a newly provisioned Orca bot.
    pub async fn import_candidate(
        &self,
        candidate: &BotImportCandidate,
        custom_name_opt: Option<&str>,
        raw_export_reply_opt: Option<&str>,
    ) -> Result<OrcaBotRecord> {
        let bot = self
            .import_engine
            .import_candidate(
                &self.controller,
                &self.home_store,
                candidate,
                custom_name_opt,
                raw_export_reply_opt,
            )
            .await?;
        self.items_store
            .register_bot_folder(&bot.id, bot.folder.clone())?;
        Ok(bot)
    }

    /// Executes transactional `move -> notify -> restore` workspace hand-over.
    pub async fn hand_over_workspace<A: WorkspaceLineageAdapter>(
        &self,
        adapter: &A,
        workspace_id: &str,
        target: HandOverTarget,
    ) -> Result<WorkspaceHandoverOutcome> {
        let outcome = self
            .handover
            .hand_over_workspace(adapter, &self.controller, workspace_id, target)
            .await?;
        if let HandOverOutcome::Created {
            ref bot_id,
            ref folder,
            ..
        } = outcome
        {
            self.items_store
                .register_bot_folder(bot_id, folder.clone())?;
        }
        Ok(outcome)
    }

    /// Builds a unified cycle-checked `BotChatTreeView` with optional usage-limit notice & typed cards.
    pub fn build_chat_tree(
        &self,
        bot_id: &str,
        nodes: &[BotChatNode],
        usage_limit: Option<UsageLimitContinue>,
        cards: Vec<NativeChatBotCard>,
        now_ms: i64,
    ) -> Result<BotChatTreeView> {
        let mut builder = BotChatTreeBuilder::new(bot_id);
        for node in nodes {
            builder = builder.push_node(node.clone());
        }
        if let Some(limit) = usage_limit {
            builder = builder.with_usage_limit(limit, now_ms);
        }
        for card in cards {
            builder = builder.push_card(card);
        }
        builder.build()
    }

    /// Gracefully shuts down the loopback `BotToolRelay` (if running) and reaps all shared MCP children.
    pub async fn shutdown(&self) -> Result<()> {
        if let Some(relay) = self.tool_relay.lock().await.take() {
            relay.shutdown().await;
        }
        self.shared_mcp.shutdown_all().await;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ecosystem_handover::InMemoryWorkspaceLineage;
    use crate::items_store::CreateBotItemRequest;
    use crate::tool_relay::{BotRelayGrant, RelayToolAuth, RelayToolRegistration};
    use orca_core::bot_ecosystem::{
        BotChatLiveState, BotChatNodeKind, NativeChatBotCardKind,
    };
    use orca_core::bot_types::BotAgentId;
    use std::collections::HashSet;
    use tempfile::TempDir;

    fn assert_send_sync<T: Send + Sync>() {}

    #[tokio::test]
    async fn facade_end_to_end_orchestration_and_cross_subsystem_invariants() {
        assert_send_sync::<OrcaBotsFacade>();

        let dir = TempDir::new().expect("tempdir");
        let cfg = OrcaBotsConfig::isolated(dir.path());
        let fake_home = dir.path().join("fake_home");
        fs::create_dir_all(fake_home.join(".hermes").join("profiles").join("demo"))
            .expect("mkdir hermes");
        fs::write(
            fake_home
                .join(".hermes")
                .join("profiles")
                .join("demo")
                .join("SOUL.md"),
            "# Demo Hermes\n\nHelps with ops.\n",
        )
        .expect("write hermes soul");

        let facade = OrcaBotsFacade::open(cfg).expect("open facade");
        let startup = facade.sync_startup().await.expect("sync startup");
        assert!(startup.recovered_request_ids.is_empty());

        // 1. Install from template -> disabled schedules + skills linked
        let installed = facade
            .install_from_template("ci-failures", Some("CI Watcher"))
            .await
            .expect("install template");
        assert!(!installed.look_around_enabled);
        assert!(installed.suggested_schedules.iter().all(|s| !s.enabled));
        let bot = installed.bot;

        // 2. Create BotItem & close via facade -> deterministic monthly journal written
        // 2026-11-25T12:00:00Z
        let close_ts = 1_795_608_000_000i64;
        let (item, _) = facade
            .items_store()
            .create_item(
                CreateBotItemRequest::new(&bot.id, BotItemActor::Owner, "Fix flaky CI job"),
                close_ts - 10_000,
            )
            .expect("create item");
        let (closed, _) = facade
            .close_item(
                &item.uid,
                BotItemActor::Owner,
                BotItemStatus::Done,
                Some("Green on main".to_string()),
                Some(item.version),
                Some(JournalCloseAttribution::Owner),
                close_ts,
            )
            .await
            .expect("close item");
        assert_eq!(closed.status, BotItemStatus::Done);
        let journal_path = facade
            .home_store()
            .bot_dir(&bot.folder)
            .join("memory")
            .join("journal")
            .join("2026-11.md");
        let journal = fs::read_to_string(&journal_path).expect("journal exists");
        assert!(journal.contains("Fix flaky CI job"));
        assert!(journal.contains("done"));

        // 3. Export & Import v1 zip package round-trip
        let exported = facade
            .export_package(&bot.id, &ExportPackageOptions::default())
            .await
            .expect("export");
        let imported = facade
            .import_package(&exported.zip_bytes, Some("CI Watcher Clone"))
            .await
            .expect("import");
        assert_eq!(imported.bot.name.as_str(), "CI Watcher Clone");

        // 4. Discover & import Hermes candidate
        let discovered = facade.discover_imports(close_ts).expect("discover");
        assert_eq!(discovered.len(), 1);
        let imported_hermes = facade
            .import_candidate(&discovered[0], None, None)
            .await
            .expect("import candidate");
        assert_eq!(imported_hermes.name.as_str(), "Demo Hermes");

        // 5. Workspace hand-over creating a supervising bot
        let lineage = InMemoryWorkspaceLineage::new();
        lineage
            .set_workspace_parent("wt-42", Some("human".to_string()))
            .expect("set parent");
        let handover = facade
            .hand_over_workspace(
                &lineage,
                "wt-42",
                HandOverTarget::CreateNewBot {
                    workspace_name: "fix-relay-reconnect".to_string(),
                    repo_slug: Some("acme/orca".to_string()),
                    recent_prompt: Some("Finish reconnect tests".to_string()),
                    agent: BotAgentId::Claude,
                    custom_name: None,
                },
            )
            .await
            .expect("handover");
        assert!(matches!(handover, HandOverOutcome::Created { .. }));

        // 6. Build unified chat tree + usage limit + typed card
        let limit = UsageLimitContinue::new("claude", close_ts - 5_000, close_ts + 60_000, true)
            .expect("limit");
        let card = BotChatTreeBuilder::make_card(
            NativeChatBotCardKind::Webhook,
            &bot.id,
            "home",
            "Webhook Ready",
            None,
            serde_json::json!({ "hook": "wh_1234567890abcdef" }),
            close_ts,
        )
        .expect("card");
        let tree_view = facade
            .build_chat_tree(
                &bot.id,
                &[BotChatNode {
                    session_id: "home".to_string(),
                    parent_session_id: None,
                    title: "Home".to_string(),
                    kind: BotChatNodeKind::Home,
                    live_state: BotChatLiveState::Idle,
                    note: None,
                    unread: false,
                    needs_you: false,
                    updated_at_ms: close_ts,
                }],
                Some(limit),
                vec![card],
                close_ts,
            )
            .expect("tree view");
        assert_eq!(tree_view.tree.total_nodes, 1);
        assert_eq!(tree_view.cards.len(), 1);
        assert!(
            tree_view
                .usage_limit_notice
                .as_deref()
                .is_some_and(|n| n.starts_with("Continues at "))
        );

        // 7. Start loopback BotToolRelay, register grant, then delete_bot auto-revokes before release!
        let relay = facade.start_tool_relay().await.expect("start relay");
        let mut folders = HashSet::new();
        folders.insert(bot.folder.as_str().to_string());
        relay
            .register_tool(RelayToolRegistration {
                credential_id: "cred-1".to_string(),
                tool_handle: "github".to_string(),
                upstream_url: "https://api.github.com".to_string(),
                allowed_folders: folders.clone(),
                auth: RelayToolAuth::None,
            })
            .expect("register tool");
        relay
            .register_bot_grant(BotRelayGrant {
                bot_id: bot.id.clone(),
                owner_id: "owner".to_string(),
                token: "tok-secret-bot".to_string(),
                folder: bot.folder.as_str().to_string(),
                allowed_folders: folders,
                allowed_tools: HashSet::new(),
                allow_host_mcp_servers: false,
            })
            .expect("register grant");

        let released = facade.delete_bot(&bot.id).await.expect("delete bot");
        assert_eq!(released.bot_id, bot.id);
        assert!(!released.home_path.exists());

        // Verify revoked token gets HTTP 404 CREDENTIAL_ENDED from the live loopback relay!
        let client = reqwest::Client::builder().no_proxy().build().expect("http");
        let resp = client
            .get(format!("{}/tools/github", relay.base_url()))
            .header("authorization", "Bearer tok-secret-bot")
            .send()
            .await
            .expect("send");
        assert_eq!(resp.status().as_u16(), 404);
        let text = resp.text().await.expect("body");
        assert!(text.contains("CREDENTIAL_ENDED"));

        facade.shutdown().await.expect("shutdown");
    }
}
