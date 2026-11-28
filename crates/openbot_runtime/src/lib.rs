//! OpenBot Core Runtime Engine (`OB-F06`..`OB-F10`).
//!
//! Pure-Rust implementation of the OpenBot agent runtime:
//! - 18 provider harnesses, custom OpenAI/Anthropic endpoints, local model server discovery,
//!   and `<= 8` backup-model failover with `usageLimitReturn` and `<= 32k` handoff packets
//! - Agent lifecycle (`<= 100` agents, AI profile generation with `Muse` refusal, `.openbot-stage`
//!   atomic duplication, `.openbot`/Grok `.zip`/Hermes/Template import-export, privilege guard)
//! - Multi-lane `DrainScheduler`, 80% `ContextCompactor`, `AttentionRegistry` (`10m` TTL + secret
//!   redaction), side threads, branching, zero-DB `ask_side_question`, RGI reactions, pins,
//!   `1..=32` `Mailbox` barrier + `interrupt_agent`, and `dm-<sha256>` DMs
//! - Full Multi-Bot Group-Chat `ChannelService` & `ChannelHistoryBuilder` (NFD `@mention` ranking,
//!   Lead JSON router, `CHANNEL_PARALLEL_LIMIT=2`, `CHANNEL_ASSIGNMENT_LIMIT=8`,
//!   `CHANNEL_REPLY_LIMIT=2` quarantine & deadlock prevention, DAG cycle guard, resource mutexes)
//! - 160-tool `ToolCatalog`, 3-tier memory & `SharedAgentDataDb` SQL routing (`OpenBotToolRouter`),
//!   `SkillService` (`SKILL.md`, `<= 32`), `McpGateway` (`<= 100`, PKCE S256, `computer_use`),
//!   `VaultService` (`ChaCha20Poly1305`, `0600` keys, RFC 6238 TOTP, Link payment), and
//!   `BrowserService` (`<= 25` tabs, `<= 200` steps, `submit_secret`, `fill_card` exfiltration lock).

pub mod agent_lifecycle;
pub mod attention_registry;
pub mod browser_service;
pub mod channel_history;
pub mod channel_service;
pub mod context_compactor;
pub mod drain_scheduler;
pub mod error;
pub mod host_runtime;
pub mod platform;
pub mod providers;
pub mod skill_mcp_service;
pub mod tool_catalog;
pub mod tool_router;
pub mod vault_service;

pub use agent_lifecycle::{
    AgentLifecycleService, AgentTemplateSpec, CreateAgentRequest, ExportArchiveResult,
    GenerateProfileRequest, GeneratedProfileOutcome, ImportAgentsResult, TemplateSkillSpec,
};
pub use attention_registry::{
    ATTENTION_TTL_MS, ActiveAttentionSnapshot, ApprovalDecision, ApprovalKind,
    ApprovalRequestInput, ApprovalStatus, AskUiInput, AskUserInput, AttentionRegistry,
    BrowserTakeoverRecord, PendingApprovalRecord, PendingQuestionRecord, PendingUiBlockRecord,
    QuestionResolutionOutcome, RegisterTakeoverInput, SecretTokenHandle, TakeoverReason,
    TakeoverStatus, UiBlockResolutionOutcome,
};
pub use browser_service::{
    BROWSER_SCRIPT_MAX_STEPS, BrowserRunScriptResult, BrowserRunStepsRequest, BrowserService,
    BrowserSnapshotView, BrowserTabSummary, BrowserTarget, CardFillTargets, FillCardRequest,
    FillCardResult, FillFieldEntry, FillFieldsRequest, FillFieldsResult, MAX_BROWSER_TABS,
    SubmitSecretRequest, SubmitSecretResult, TakeoverRequest, TakeoverState,
};
pub use channel_history::{
    BoundedHistoryPage, ChannelHistoryBuilder, ChannelHistorySlice, PacketMode,
    PreparePacketOptions, PreparedChannelPacket,
};
pub use channel_service::{
    ChannelAssignInput, ChannelAssignResult, ChannelCancelInput, ChannelCancelResult,
    ChannelContinueInput, ChannelHistoryPacketInput, ChannelReassignInput, ChannelReplyInput,
    ChannelReplyResult, ChannelResultInput, ChannelResultOutput, ChannelService,
    ChannelToolExecutionContext, ChannelTransferInput, ChannelTransferResult,
    CreateChannelInput, PostChannelMessageInput, PostChannelMessageResult, RankedChannelMention,
    UpdateChannelInput, fold_nfd_lower,
};
pub use context_compactor::{
    CONTEXT_COMPACTION_THRESHOLD, CompactionOutcome, CompactionReason, CompactionStatus,
    ContextCompactor, ContextStateSnapshot, ThreadContextBudget,
};
pub use drain_scheduler::{
    ActiveTurnOwnerHint, BootRecoveryReport, BranchThreadInput, BranchThreadOutcome,
    DrainScheduler, InterruptAgentInput, InterruptAgentOutcome, MailboxSendInput,
    MailboxSendOutcome, PinActor, PinMessageInput, PinMessageOutcome, QueueAddInput,
    QueueAddOutcome, QueueEditResult, QueueListItem, QueueListOutcome, QueuePauseOutcome,
    QueueResumeOutcome, QueueSteerInput, QueueSteerOutcome, ReactMessageInput,
    ReactMessageOutcome, SchedulerLane, SendChatMessageInput, SendChatMessageOutcome,
    SendToChannelInput, SendToChannelOutcome, SideQuestionInput, SideQuestionOutcome,
    SideQuestionTurn, TurnDispatchOutcome,
};
pub use error::{FailoverCause, OpenBotRuntimeError, RuntimeResult, SoftRefusal};
pub use host_runtime::OpenBotHostRuntime;
pub use platform::{
    AutomationHttpServer, AutomationServerConfig, AutomationServerHandle, BridgeEvalOutcome,
    BridgeEvalRequest, ConnectedAppRecord, CreditMediaOutcome, CrmProposeOutcome,
    CrmProposeRequest, CrmService, CrmSyncOutcome, CrmSyncRequest, ExternalBridgeOrchestrator,
    HtmlAndCreditMediaService, HtmlRenderMode, HtmlRenderOutcome, HtmlRenderRequest,
    JoinedServerClient, MarketplaceItem, MarketplaceSearchOutcome, MarketplaceSearchRequest,
    MarketplaceService, MarketplaceTab, NativeConnectorVerified, OpenBotPageRpcOutcome,
    OpenBotServeConfig, OpenBotServeHandle, PlatformServices, PlatformToolDelegate, RoutineEngine,
    RoutineRunOutcome, RoutineRunRequest, RoutineSaveOutcome, RoutineSaveRequest, RoutineTimer,
    StaticSiteDeleteOutcome, StaticSiteDeleteRequest, StaticSitePublishRequest, StaticSiteRecord,
    StaticSiteService, TeamApiConfig, TeamApiHandle, TeamApiServer, TeamProtocolEvalOutcome,
    TeamProtocolEvalRequest, VoiceEvalMode, VoiceEvalOutcome, VoiceEvalRequest, VoiceService,
    WebhookPostOutcome, WebhookPostRequest, WhatsAppDecision, WhatsAppSenderFacts,
    WhatsAppSenderRules, WorkflowEngine, WorkflowRunOutcome, WorkflowRunRequest,
    WorkflowSaveOutcome, WorkflowSaveRequest, WorkflowStepReport, ct_eq,
    execute_native_connector_tool, generate_base64url_token_32, native_connector_real_endpoint,
    native_connector_tools, verify_native_connector,
};
pub use providers::{
    HANDOFF_MAX_CHARS, HANDOFF_START, ProviderCliProbe, ProviderHarnessInfo,
    ProviderProtocolKind, ProviderRuntimeManager, StartTurnRequest, TurnExecutionOutcome,
    all_provider_harnesses, build_handoff_packet, provider_harness_info,
};
pub use skill_mcp_service::{
    ComputerUseDescriptor, InstalledSkillEntry, InstalledSkillsManifest, McpDiscoveredTool,
    McpGateway, McpOAuthDiscovery, McpOAuthTokenResponse, McpProbeOutcome, McpToolCallOutcome,
    PkceS256Challenge, RegisterMcpServerInput, SaveSkillInput, SaveSkillOutcome, SkillFrontmatter,
    SkillService, call_http_mcp_endpoint_jsonrpc, discover_mcp_oauth, exchange_mcp_oauth_code,
    generate_pkce_s256, parse_discovered_tools, probe_http_mcp_endpoint,
    register_mcp_oauth_client, verify_pkce_s256,
};
pub use tool_catalog::{
    CHANNEL_TOOLS_COUNT, DIRECT_BROWSER_COUNT, DIRECT_OPENBOT_COUNT, DeveloperInstructionsInput,
    FULL_CATALOG_COUNT, OPENBOT_BROWSER_NAMESPACE, OPENBOT_NAMESPACE, ToolCatalog,
    ToolCatalogList, ToolDescribeResult, ToolDescriptor, ToolSearchMatch, ToolSearchResult,
};
pub use tool_router::{
    ExternalToolDelegate, MemoryPutInput, MemoryPutOutcome, MemorySearchInput,
    MemorySearchOutcome, OpenBotToolRouter, ToolCallContext, ToolCallOutcome,
};
pub use vault_service::{
    CardForFill, ExternalLoginRef, LINK_PAYMENT_MIN_CONTEXT_CHARS, LinkPaymentRequest,
    LinkPaymentResult, LoginProviderKind, SavedApiKeyFile, SecretFillMethod, VaultCardBrand,
    VaultItemKind, VaultItemSummary, VaultLoginEntry, VaultSaveInput, VaultService, VaultState,
    VaultTwoStepCode, compute_totp, compute_totp_from_uri, mask_card_last4,
};
