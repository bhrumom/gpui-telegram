//! Unified OpenBot Host Runtime Facade (`OpenBotHostRuntime`).
//!
//! Initializes `<data_dir>/openbot.db`, `<data_dir>/Shared/Data/agent-data.db`,
//! all core M3 runtime services (`OB-F06`..`OB-F10`), and executes boot recovery.
//! Exposes a clean typed extension registry so higher-level platform services can attach
//! without breaking M3 signatures.

use std::any::{Any, TypeId};
use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use openbot_storage::{OpenBotDatabase, SharedAgentDataDb};
use parking_lot::RwLock;

use crate::agent_lifecycle::AgentLifecycleService;
use crate::attention_registry::AttentionRegistry;
use crate::browser_service::BrowserService;
use crate::channel_service::ChannelService;
use crate::context_compactor::ContextCompactor;
use crate::drain_scheduler::{BootRecoveryReport, DrainScheduler};
use crate::error::RuntimeResult;
use crate::providers::ProviderRuntimeManager;
use crate::skill_mcp_service::{McpGateway, SkillService};
use crate::tool_catalog::ToolCatalog;
use crate::tool_router::{ExternalToolDelegate, OpenBotToolRouter};
use crate::vault_service::VaultService;

/// Top-level facade owning the complete OpenBot core runtime for a single `<data_dir>`.
pub struct OpenBotHostRuntime {
    data_dir: PathBuf,
    database: OpenBotDatabase,
    shared_agent_data: SharedAgentDataDb,
    providers: Arc<ProviderRuntimeManager>,
    agents: Arc<AgentLifecycleService>,
    compactor: Arc<ContextCompactor>,
    attention: Arc<AttentionRegistry>,
    scheduler: Arc<DrainScheduler>,
    channels: Arc<ChannelService>,
    tool_catalog: Arc<ToolCatalog>,
    skills: Arc<SkillService>,
    mcp: Arc<McpGateway>,
    vault: Arc<VaultService>,
    browser: Arc<BrowserService>,
    tool_router: Arc<OpenBotToolRouter>,
    boot_recovery: BootRecoveryReport,
    expired_attention_on_boot: usize,
    extensions: RwLock<HashMap<TypeId, Arc<dyn Any + Send + Sync>>>,
}

impl OpenBotHostRuntime {
    /// Opens or creates `<data_dir>/openbot.db` and `<data_dir>/Shared/Data/agent-data.db`,
    /// wires every M3 core runtime service, and executes boot crash recovery.
    pub fn open(data_dir: impl AsRef<Path>) -> RuntimeResult<Arc<Self>> {
        let data_dir = data_dir.as_ref().to_path_buf();
        fs::create_dir_all(&data_dir)?;

        let openbot_db_path = data_dir.join("openbot.db");
        let database = OpenBotDatabase::open(&openbot_db_path)?;

        let shared_data_path = data_dir.join("Shared").join("Data").join("agent-data.db");
        let shared_agent_data = SharedAgentDataDb::open(&shared_data_path)?;

        let providers = ProviderRuntimeManager::open(database.clone(), &data_dir)?;
        let agents =
            AgentLifecycleService::new(database.clone(), &data_dir, Arc::clone(&providers))?;
        let compactor = ContextCompactor::new(database.clone());
        let attention = AttentionRegistry::open(database.clone(), &data_dir)?;
        let scheduler = DrainScheduler::new(
            database.clone(),
            Arc::clone(&providers),
            Arc::clone(&compactor),
            Arc::clone(&attention),
        )?;
        let channels = ChannelService::new(database.clone(), Arc::clone(&providers))?;
        let tool_catalog = Arc::new(ToolCatalog::new());
        let skills = SkillService::open(database.clone(), &data_dir)?;
        let mcp = McpGateway::open(database.clone(), &data_dir)?;
        let vault = VaultService::open(database.clone(), &data_dir)?;
        let browser = BrowserService::open(database.clone(), &data_dir, Arc::clone(&vault))?;

        let tool_router = OpenBotToolRouter::new(
            database.clone(),
            shared_agent_data.clone(),
            Arc::clone(&tool_catalog),
            Arc::clone(&providers),
            Arc::clone(&agents),
            Arc::clone(&scheduler),
            Arc::clone(&attention),
            Arc::clone(&channels),
            Arc::clone(&skills),
            Arc::clone(&mcp),
            Arc::clone(&vault),
            Arc::clone(&browser),
        );

        let boot_recovery = scheduler.recover_boot_state()?;
        let expired_attention_on_boot = attention.expire_stale();

        Ok(Arc::new(Self {
            data_dir,
            database,
            shared_agent_data,
            providers,
            agents,
            compactor,
            attention,
            scheduler,
            channels,
            tool_catalog,
            skills,
            mcp,
            vault,
            browser,
            tool_router,
            boot_recovery,
            expired_attention_on_boot,
            extensions: RwLock::new(HashMap::new()),
        }))
    }

    pub fn data_dir(&self) -> &Path {
        &self.data_dir
    }

    pub fn database(&self) -> &OpenBotDatabase {
        &self.database
    }

    pub fn shared_agent_data(&self) -> &SharedAgentDataDb {
        &self.shared_agent_data
    }

    pub fn providers(&self) -> &Arc<ProviderRuntimeManager> {
        &self.providers
    }

    pub fn agents(&self) -> &Arc<AgentLifecycleService> {
        &self.agents
    }

    pub fn compactor(&self) -> &Arc<ContextCompactor> {
        &self.compactor
    }

    pub fn attention(&self) -> &Arc<AttentionRegistry> {
        &self.attention
    }

    pub fn scheduler(&self) -> &Arc<DrainScheduler> {
        &self.scheduler
    }

    pub fn channels(&self) -> &Arc<ChannelService> {
        &self.channels
    }

    pub fn tool_catalog(&self) -> &Arc<ToolCatalog> {
        &self.tool_catalog
    }

    pub fn skills(&self) -> &Arc<SkillService> {
        &self.skills
    }

    pub fn mcp(&self) -> &Arc<McpGateway> {
        &self.mcp
    }

    pub fn vault(&self) -> &Arc<VaultService> {
        &self.vault
    }

    pub fn browser(&self) -> &Arc<BrowserService> {
        &self.browser
    }

    pub fn tool_router(&self) -> &Arc<OpenBotToolRouter> {
        &self.tool_router
    }

    pub fn boot_recovery(&self) -> &BootRecoveryReport {
        &self.boot_recovery
    }

    pub fn expired_attention_on_boot(&self) -> usize {
        self.expired_attention_on_boot
    }

    /// Registers an optional external tool delegate on the runtime's [`OpenBotToolRouter`].
    pub fn set_external_tool_delegate(&self, delegate: Option<Arc<dyn ExternalToolDelegate>>) {
        self.tool_router.set_external_delegate(delegate);
    }

    /// Attaches a typed runtime extension (e.g., platform services container) to this host.
    pub fn attach_extension<T: Send + Sync + 'static>(&self, extension: Arc<T>) {
        self.extensions.write().insert(TypeId::of::<T>(), extension);
    }

    /// Retrieves a previously attached typed runtime extension.
    pub fn extension<T: Send + Sync + 'static>(&self) -> Option<Arc<T>> {
        self.extensions
            .read()
            .get(&TypeId::of::<T>())
            .and_then(|arc| Arc::clone(arc).downcast::<T>().ok())
    }
}
