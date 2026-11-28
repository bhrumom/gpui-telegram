//! OpenBot Platform Subsystems (`OB-F11`, `OB-F12`, `OB-F13`):
//! routines, 12-node DAG workflows, loopback automation HTTP server, 13-table CRM,
//! static sites, 3-tab marketplace, sandboxed HTML/credit media, voice STT/TTS/Live,
//! external chat bridges, and the `/v1/*` Team API server + joined-server client.

use std::path::Path;
use std::sync::Arc;

use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use openbot_storage::OpenBotDatabase;

use crate::error::RuntimeResult;

pub mod automation_http;
pub mod bridges;
pub mod connector_driver;
pub mod crm;
pub mod html_media;
pub mod marketplace;
pub mod routines;
pub mod static_sites;
pub mod team_api;
pub mod tool_delegate;
pub mod voice;
pub mod workflows;

pub use automation_http::{AutomationHttpServer, AutomationServerConfig, AutomationServerHandle};
pub use bridges::{
    BridgeEvalOutcome, BridgeEvalRequest, ExternalBridgeOrchestrator, WhatsAppDecision,
    WhatsAppSenderFacts, WhatsAppSenderRules,
};
pub use connector_driver::{
    NativeConnectorVerified, execute_native_connector_tool, native_connector_real_endpoint,
    native_connector_tools, verify_native_connector,
};
pub use crm::{
    CrmProposeOutcome, CrmProposeRequest, CrmService, CrmSyncOutcome, CrmSyncRequest,
};
pub use html_media::{
    CreditMediaOutcome, HtmlAndCreditMediaService, HtmlRenderMode, HtmlRenderOutcome,
    HtmlRenderRequest, OpenBotPageRpcOutcome,
};
pub use marketplace::{
    ConnectedAppRecord, MarketplaceItem, MarketplaceSearchOutcome, MarketplaceSearchRequest,
    MarketplaceService, MarketplaceTab,
};
pub use routines::{
    RoutineEngine, RoutineRunOutcome, RoutineRunRequest, RoutineSaveOutcome, RoutineSaveRequest,
    RoutineTimer, WebhookPostOutcome, WebhookPostRequest,
};
pub use static_sites::{
    StaticSiteDeleteOutcome, StaticSiteDeleteRequest, StaticSitePublishRequest, StaticSiteRecord,
    StaticSiteService,
};
pub use team_api::{
    JoinedServerClient, OpenBotServeConfig, OpenBotServeHandle, TeamApiConfig, TeamApiHandle,
    TeamApiServer, TeamProtocolEvalOutcome, TeamProtocolEvalRequest,
};
pub use tool_delegate::PlatformToolDelegate;
pub use voice::{VoiceEvalMode, VoiceEvalOutcome, VoiceEvalRequest, VoiceService};
pub use workflows::{
    WorkflowEngine, WorkflowRunOutcome, WorkflowRunRequest, WorkflowSaveOutcome,
    WorkflowSaveRequest, WorkflowStepReport,
};

/// Constant-time byte-slice equality check without early-exit on content mismatch.
///
/// Accumulates XOR differences across `max(a.len(), b.len())` positions alongside
/// length equality so bearer token and HMAC comparisons never leak prefix matches.
pub fn ct_eq(a: &[u8], b: &[u8]) -> bool {
    let len_diff = a.len() ^ b.len();
    let max_len = a.len().max(b.len());
    let mut diff = if len_diff == 0 { 0u8 } else { 1u8 };
    for idx in 0..max_len {
        let lhs = if idx < a.len() { a[idx] } else { 0 };
        let rhs = if idx < b.len() { b[idx] } else { 0 };
        diff |= lhs ^ rhs;
    }
    std::hint::black_box(diff) == 0 && std::hint::black_box(len_diff) == 0
}

/// Generates a 256-bit cryptographically random token encoded as unpadded `base64url`
/// (43 ASCII URL-safe characters), matching `randomBytes(32).toString("base64url")`.
pub fn generate_base64url_token_32() -> String {
    let mut bytes = [0u8; 32];
    getrandom::getrandom(&mut bytes).expect("OS CSPRNG unavailable via getrandom");
    URL_SAFE_NO_PAD.encode(bytes)
}

/// Aggregate container owning all OpenBot platform subsystem services (`OB-F11`..`OB-F13`).
#[derive(Clone)]
pub struct PlatformServices {
    pub routines: Arc<RoutineEngine>,
    pub workflows: Arc<WorkflowEngine>,
    pub crm: Arc<CrmService>,
    pub static_sites: Arc<StaticSiteService>,
    pub marketplace: Arc<MarketplaceService>,
    pub html_media: Arc<HtmlAndCreditMediaService>,
    pub voice: Arc<VoiceService>,
    pub bridges: Arc<ExternalBridgeOrchestrator>,
}

impl PlatformServices {
    /// Initializes every OpenBot platform service backed by `database` and `data_dir`.
    pub fn open(
        database: OpenBotDatabase,
        data_dir: impl AsRef<Path>,
    ) -> RuntimeResult<Arc<Self>> {
        let dir = data_dir.as_ref();
        let routines = RoutineEngine::new(database.clone());
        let workflows = WorkflowEngine::new(database.clone());
        let crm = CrmService::open(database.clone(), dir)?;
        let static_sites = StaticSiteService::open(dir)?;
        let marketplace = MarketplaceService::open(dir)?;
        let html_media = HtmlAndCreditMediaService::open(dir)?;
        let voice = VoiceService::open(database.clone(), dir)?;
        let bridges = ExternalBridgeOrchestrator::open(database, dir)?;

        Ok(Arc::new(Self {
            routines,
            workflows,
            crm,
            static_sites,
            marketplace,
            html_media,
            voice,
            bridges,
        }))
    }
}
