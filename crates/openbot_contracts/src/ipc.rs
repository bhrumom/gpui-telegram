//! Complete 65-group IPC endpoint registry (`IpcGroupName`, 50 nested `BridgedDesktopApi` paths),
//! `ScopedRequest<T>` (`AgentIpcRequest<T>`), `ScopedAgentEvent`, and all `AgentEvent` wire variants.
//! Mirrors `packages/contracts/src/{ipc-endpoints,ipc-agent-events,ipc-desktop-apis}.ts`.

use serde::{Deserialize, Deserializer, Serialize};

use crate::agents::{AgentTasksSnapshot, ClientAgentSummary, HiddenAgent};
pub use crate::avatar::AgentStepActivity;
use crate::conversation::{AgentPromptQuestion, ConversationSnapshot};
use crate::limits::{RequestId, is_identifier, non_null_option};
use crate::providers::AgentProviderId;
use crate::queue::QueueSnapshot;

pub const AGENT_STEP_ACTIVITIES: [&str; 8] = [
    "searching",
    "browsing",
    "reading",
    "running",
    "editing",
    "delegating",
    "working",
    "reviewing",
];

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ServerScope {
    Payload,
    Empty,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ServerArgument {
    Optional,
    Required,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentIpcRequest<T> {
    pub server_id: String,
    pub payload: T,
}

/// Alias matching `PROJECT.md § Interface Contracts § 1` (`ScopedRequest<T>`).
pub type ScopedRequest<T> = AgentIpcRequest<T>;

/// All 65 `IPC_ENDPOINTS` groups (`ipc-endpoints.ts:L775-L2151`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum IpcGroupName {
    App,
    Maintenance,
    Providers,
    ProviderRuntimes,
    Voice,
    DynamicIsland,
    BrowserDock,
    AgentDock,
    ComputerUse,
    MacAutomation,
    BackgroundProcesses,
    Skills,
    CustomProviders,
    ProviderDetection,
    CustomAgents,
    AcpRegistry,
    ProviderAdmin,
    Messaging,
    HostAdmin,
    GithubConnector,
    OnePasswordConnector,
    BitwardenConnector,
    PasswordManagers,
    LinkAccount,
    Vault,
    HostedSites,
    RoutineFeed,
    Workflows,
    Crm,
    RoutineResults,
    Billing,
    Referral,
    Credits,
    License,
    HostedServers,
    LocalServers,
    MarketplaceAgents,
    AgentTemplates,
    Auth,
    Update,
    Notifications,
    Agent,
    AgentMemories,
    SharedTables,
    AgentRoutines,
    ChannelMemories,
    ChannelRoutines,
    ChannelFolders,
    AgentAttachments,
    AgentFolders,
    AgentInstructions,
    AttachmentImports,
    Browser,
    BrowserInput,
    Servers,
    AgentAdmin,
    McpServers,
    Storage,
    AgentImport,
    InstanceTransfer,
    Plugins,
    Marketplace,
    Host,
    Events,
    RemoteDesktop,
}

impl IpcGroupName {
    pub const ALL: [Self; 65] = [
        Self::App,
        Self::Maintenance,
        Self::Providers,
        Self::ProviderRuntimes,
        Self::Voice,
        Self::DynamicIsland,
        Self::BrowserDock,
        Self::AgentDock,
        Self::ComputerUse,
        Self::MacAutomation,
        Self::BackgroundProcesses,
        Self::Skills,
        Self::CustomProviders,
        Self::ProviderDetection,
        Self::CustomAgents,
        Self::AcpRegistry,
        Self::ProviderAdmin,
        Self::Messaging,
        Self::HostAdmin,
        Self::GithubConnector,
        Self::OnePasswordConnector,
        Self::BitwardenConnector,
        Self::PasswordManagers,
        Self::LinkAccount,
        Self::Vault,
        Self::HostedSites,
        Self::RoutineFeed,
        Self::Workflows,
        Self::Crm,
        Self::RoutineResults,
        Self::Billing,
        Self::Referral,
        Self::Credits,
        Self::License,
        Self::HostedServers,
        Self::LocalServers,
        Self::MarketplaceAgents,
        Self::AgentTemplates,
        Self::Auth,
        Self::Update,
        Self::Notifications,
        Self::Agent,
        Self::AgentMemories,
        Self::SharedTables,
        Self::AgentRoutines,
        Self::ChannelMemories,
        Self::ChannelRoutines,
        Self::ChannelFolders,
        Self::AgentAttachments,
        Self::AgentFolders,
        Self::AgentInstructions,
        Self::AttachmentImports,
        Self::Browser,
        Self::BrowserInput,
        Self::Servers,
        Self::AgentAdmin,
        Self::McpServers,
        Self::Storage,
        Self::AgentImport,
        Self::InstanceTransfer,
        Self::Plugins,
        Self::Marketplace,
        Self::Host,
        Self::Events,
        Self::RemoteDesktop,
    ];
}

impl IpcGroupName {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::App => "app",
            Self::Maintenance => "maintenance",
            Self::Providers => "providers",
            Self::ProviderRuntimes => "providerRuntimes",
            Self::Voice => "voice",
            Self::DynamicIsland => "dynamicIsland",
            Self::BrowserDock => "browserDock",
            Self::AgentDock => "agentDock",
            Self::ComputerUse => "computerUse",
            Self::MacAutomation => "macAutomation",
            Self::BackgroundProcesses => "backgroundProcesses",
            Self::Skills => "skills",
            Self::CustomProviders => "customProviders",
            Self::ProviderDetection => "providerDetection",
            Self::CustomAgents => "customAgents",
            Self::AcpRegistry => "acpRegistry",
            Self::ProviderAdmin => "providerAdmin",
            Self::Messaging => "messaging",
            Self::HostAdmin => "hostAdmin",
            Self::GithubConnector => "githubConnector",
            Self::OnePasswordConnector => "onePasswordConnector",
            Self::BitwardenConnector => "bitwardenConnector",
            Self::PasswordManagers => "passwordManagers",
            Self::LinkAccount => "linkAccount",
            Self::Vault => "vault",
            Self::HostedSites => "hostedSites",
            Self::RoutineFeed => "routineFeed",
            Self::Workflows => "workflows",
            Self::Crm => "crm",
            Self::RoutineResults => "routineResults",
            Self::Billing => "billing",
            Self::Referral => "referral",
            Self::Credits => "credits",
            Self::License => "license",
            Self::HostedServers => "hostedServers",
            Self::LocalServers => "localServers",
            Self::MarketplaceAgents => "marketplaceAgents",
            Self::AgentTemplates => "agentTemplates",
            Self::Auth => "auth",
            Self::Update => "update",
            Self::Notifications => "notifications",
            Self::Agent => "agent",
            Self::AgentMemories => "agentMemories",
            Self::SharedTables => "sharedTables",
            Self::AgentRoutines => "agentRoutines",
            Self::ChannelMemories => "channelMemories",
            Self::ChannelRoutines => "channelRoutines",
            Self::ChannelFolders => "channelFolders",
            Self::AgentAttachments => "agentAttachments",
            Self::AgentFolders => "agentFolders",
            Self::AgentInstructions => "agentInstructions",
            Self::AttachmentImports => "attachmentImports",
            Self::Browser => "browser",
            Self::BrowserInput => "browserInput",
            Self::Servers => "servers",
            Self::AgentAdmin => "agentAdmin",
            Self::McpServers => "mcpServers",
            Self::Storage => "storage",
            Self::AgentImport => "agentImport",
            Self::InstanceTransfer => "instanceTransfer",
            Self::Plugins => "plugins",
            Self::Marketplace => "marketplace",
            Self::Host => "host",
            Self::Events => "events",
            Self::RemoteDesktop => "remoteDesktop",
        }
    }

    /// Exact `IPC_GROUP_PATHS` mapping (`ipc-endpoints.ts:L2085-L2151`).
    pub const fn group_path(self) -> Option<&'static str> {
        match self {
            Self::App | Self::Providers => Some(""),
            Self::AttachmentImports => None,
            Self::Agent
            | Self::AgentMemories
            | Self::SharedTables
            | Self::AgentRoutines
            | Self::ChannelMemories
            | Self::ChannelRoutines
            | Self::ChannelFolders
            | Self::AgentAttachments
            | Self::AgentFolders
            | Self::AgentInstructions
            | Self::AgentAdmin
            | Self::McpServers => Some("agent"),
            Self::Browser | Self::BrowserInput => Some("browser"),
            other => Some(other.as_str()),
        }
    }
}

/// All 50 nested `BridgedDesktopApi` namespaces (`Exclude<IpcGroupPaths[IpcGroupName], "" | null>`).
pub const IPC_NESTED_API_PATHS: [&str; 50] = [
    "maintenance",
    "providerRuntimes",
    "voice",
    "dynamicIsland",
    "browserDock",
    "agentDock",
    "computerUse",
    "macAutomation",
    "backgroundProcesses",
    "skills",
    "customProviders",
    "providerDetection",
    "customAgents",
    "acpRegistry",
    "providerAdmin",
    "messaging",
    "hostAdmin",
    "githubConnector",
    "onePasswordConnector",
    "bitwardenConnector",
    "passwordManagers",
    "linkAccount",
    "vault",
    "hostedSites",
    "routineFeed",
    "workflows",
    "crm",
    "routineResults",
    "billing",
    "referral",
    "credits",
    "license",
    "hostedServers",
    "localServers",
    "marketplaceAgents",
    "agentTemplates",
    "auth",
    "update",
    "notifications",
    "agent",
    "browser",
    "servers",
    "storage",
    "agentImport",
    "instanceTransfer",
    "plugins",
    "marketplace",
    "host",
    "events",
    "remoteDesktop",
];

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum IpcEndpointKind {
    Request,
    ScopedRequest,
    ScopedQuery,
    Event,
    UntypedRequest,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct IpcEndpointDescriptor {
    pub group: IpcGroupName,
    pub key: &'static str,
    pub channel: &'static str,
    pub kind: IpcEndpointKind,
    pub server_arg: Option<ServerArgument>,
}

impl IpcEndpointDescriptor {
    pub fn method_name(&self) -> String {
        group_api_method_name(self.key, self.kind)
    }

    pub const fn group_path(&self) -> Option<&'static str> {
        self.group.group_path()
    }
}

pub fn group_api_method_name(key: &str, kind: IpcEndpointKind) -> String {
    if kind == IpcEndpointKind::Event {
        let mut chars = key.chars();
        match chars.next() {
            Some(first) => format!("on{}{}", first.to_ascii_uppercase(), chars.as_str()),
            None => "on".to_string(),
        }
    } else {
        key.to_string()
    }
}

pub fn groups_at_path(path: Option<&str>) -> Vec<IpcGroupName> {
    IpcGroupName::ALL
        .into_iter()
        .filter(|g| g.group_path() == path)
        .collect()
}

const fn ep(
    group: IpcGroupName,
    key: &'static str,
    channel: &'static str,
    kind: IpcEndpointKind,
    server_arg: Option<ServerArgument>,
) -> IpcEndpointDescriptor {
    IpcEndpointDescriptor {
        group,
        key,
        channel,
        kind,
        server_arg,
    }
}

/// Canonical IPC endpoint descriptors covering all 65 `IPC_ENDPOINTS` groups.
pub const IPC_ENDPOINTS: &[IpcEndpointDescriptor] = &[
    ep(IpcGroupName::App, "getAppInfo", "app:get-info", IpcEndpointKind::Request, None),
    ep(IpcGroupName::App, "getSetupState", "app:get-setup-state", IpcEndpointKind::Request, None),
    ep(IpcGroupName::App, "setupState", "app:setup-state", IpcEndpointKind::Event, None),
    ep(IpcGroupName::Maintenance, "exportData", "maintenance:export-data", IpcEndpointKind::Request, None),
    ep(IpcGroupName::Maintenance, "exportDiagnostics", "maintenance:export-diagnostics", IpcEndpointKind::Request, None),
    ep(IpcGroupName::Providers, "setProviderOn", "app:set-provider-on", IpcEndpointKind::Request, None),
    ep(IpcGroupName::Providers, "connectProvider", "app:connect-provider", IpcEndpointKind::Request, None),
    ep(IpcGroupName::ProviderRuntimes, "getStatus", "provider-runtimes:get-status", IpcEndpointKind::Request, None),
    ep(IpcGroupName::ProviderRuntimes, "event", "provider-runtimes:event", IpcEndpointKind::Event, None),
    ep(IpcGroupName::Voice, "getSettings", "voice:get-settings", IpcEndpointKind::Request, None),
    ep(IpcGroupName::Voice, "transcribe", "voice:transcribe", IpcEndpointKind::Request, None),
    ep(IpcGroupName::Voice, "speak", "voice:speak", IpcEndpointKind::Request, None),
    ep(IpcGroupName::Voice, "startLive", "voice:start-live", IpcEndpointKind::ScopedRequest, Some(ServerArgument::Optional)),
    ep(IpcGroupName::Voice, "liveEvent", "voice:live-event", IpcEndpointKind::Event, None),
    ep(IpcGroupName::DynamicIsland, "getPreference", "dynamic-island:get-preference", IpcEndpointKind::Request, None),
    ep(IpcGroupName::DynamicIsland, "publishPresentation", "dynamic-island:publish-presentation", IpcEndpointKind::Request, None),
    ep(IpcGroupName::DynamicIsland, "presentation", "dynamic-island:presentation", IpcEndpointKind::Event, None),
    ep(IpcGroupName::DynamicIsland, "performAction", "dynamic-island:perform-action", IpcEndpointKind::Request, None),
    ep(IpcGroupName::BrowserDock, "getPresentation", "browser-dock:get-presentation", IpcEndpointKind::Request, None),
    ep(IpcGroupName::AgentDock, "getPresentation", "agent-dock:get-presentation", IpcEndpointKind::Request, None),
    ep(IpcGroupName::ComputerUse, "getState", "computer-use:get-state", IpcEndpointKind::Request, None),
    ep(IpcGroupName::MacAutomation, "request", "mac-automation:request", IpcEndpointKind::Request, None),
    ep(IpcGroupName::BackgroundProcesses, "list", "background-processes:list", IpcEndpointKind::ScopedQuery, Some(ServerArgument::Required)),
    ep(IpcGroupName::Skills, "list", "skills:list", IpcEndpointKind::Request, None),
    ep(IpcGroupName::CustomProviders, "list", "custom-providers:list", IpcEndpointKind::Request, None),
    ep(IpcGroupName::CustomProviders, "save", "custom-providers:save", IpcEndpointKind::Request, None),
    ep(IpcGroupName::ProviderDetection, "scanModelServers", "provider-detection:scan-model-servers", IpcEndpointKind::ScopedQuery, Some(ServerArgument::Required)),
    ep(IpcGroupName::AcpRegistry, "search", "acp-registry:search", IpcEndpointKind::ScopedRequest, Some(ServerArgument::Optional)),
    ep(IpcGroupName::CustomAgents, "list", "custom-agents:list", IpcEndpointKind::ScopedQuery, Some(ServerArgument::Required)),
    ep(IpcGroupName::ProviderAdmin, "accounts", "provider-admin:accounts", IpcEndpointKind::ScopedQuery, Some(ServerArgument::Required)),
    ep(IpcGroupName::Messaging, "getSlackOverview", "messaging:get-slack-overview", IpcEndpointKind::Request, None),
    ep(IpcGroupName::Messaging, "getTelegramOverview", "messaging:get-telegram-overview", IpcEndpointKind::Request, None),
    ep(IpcGroupName::HostAdmin, "updateIdentity", "host-admin:update-identity", IpcEndpointKind::ScopedRequest, Some(ServerArgument::Required)),
    ep(IpcGroupName::GithubConnector, "status", "github-connector:status", IpcEndpointKind::Request, None),
    ep(IpcGroupName::BitwardenConnector, "status", "bitwarden-connector:status", IpcEndpointKind::Request, None),
    ep(IpcGroupName::PasswordManagers, "list", "password-managers:list", IpcEndpointKind::Request, None),
    ep(IpcGroupName::LinkAccount, "status", "link-account:status", IpcEndpointKind::Request, None),
    ep(IpcGroupName::Vault, "list", "vault:list", IpcEndpointKind::Request, None),
    ep(IpcGroupName::OnePasswordConnector, "status", "onepassword-connector:status", IpcEndpointKind::Request, None),
    ep(IpcGroupName::HostedSites, "list", "hosted-sites:list", IpcEndpointKind::ScopedQuery, Some(ServerArgument::Required)),
    ep(IpcGroupName::RoutineFeed, "get", "routine-feed:get", IpcEndpointKind::Request, None),
    ep(IpcGroupName::Billing, "getState", "billing:get-state", IpcEndpointKind::Request, None),
    ep(IpcGroupName::Referral, "getState", "referral:get-state", IpcEndpointKind::Request, None),
    ep(IpcGroupName::Credits, "getState", "credits:get-state", IpcEndpointKind::Request, None),
    ep(IpcGroupName::License, "getState", "license:get-state", IpcEndpointKind::Request, None),
    ep(IpcGroupName::HostedServers, "list", "hosted-servers:list", IpcEndpointKind::Request, None),
    ep(IpcGroupName::LocalServers, "create", "local-servers:create", IpcEndpointKind::Request, None),
    ep(IpcGroupName::MarketplaceAgents, "list", "marketplace-agents:list", IpcEndpointKind::Request, None),
    ep(IpcGroupName::AgentTemplates, "preview", "agent-templates:preview", IpcEndpointKind::Request, None),
    ep(IpcGroupName::Auth, "getState", "auth:get-state", IpcEndpointKind::Request, None),
    ep(IpcGroupName::Update, "getStatus", "update:get-status", IpcEndpointKind::Request, None),
    ep(IpcGroupName::Notifications, "getPreference", "notifications:get-preference", IpcEndpointKind::Request, None),
    ep(IpcGroupName::Agent, "getStatus", "agent:get-status", IpcEndpointKind::ScopedQuery, Some(ServerArgument::Optional)),
    ep(IpcGroupName::Agent, "listAgents", "agent:list", IpcEndpointKind::ScopedQuery, Some(ServerArgument::Optional)),
    ep(IpcGroupName::Agent, "listChannels", "agent:channels:list", IpcEndpointKind::ScopedQuery, Some(ServerArgument::Optional)),
    ep(IpcGroupName::Agent, "readChannel", "agent:channels:read", IpcEndpointKind::ScopedRequest, Some(ServerArgument::Optional)),
    ep(IpcGroupName::Agent, "channelCommand", "agent:channels:command", IpcEndpointKind::ScopedRequest, Some(ServerArgument::Optional)),
    ep(IpcGroupName::Agent, "createAgent", "agent:create", IpcEndpointKind::ScopedRequest, Some(ServerArgument::Optional)),
    ep(IpcGroupName::Agent, "sendMessage", "agent:send-message", IpcEndpointKind::ScopedRequest, Some(ServerArgument::Optional)),
    ep(IpcGroupName::Agent, "scopedEvent", "agent:event", IpcEndpointKind::Event, None),
    ep(IpcGroupName::AgentMemories, "listMemories", "agent:list-memories", IpcEndpointKind::ScopedRequest, Some(ServerArgument::Optional)),
    ep(IpcGroupName::SharedTables, "listTables", "shared:list-tables", IpcEndpointKind::ScopedQuery, Some(ServerArgument::Optional)),
    ep(IpcGroupName::AgentRoutines, "listRoutines", "agent:list-routines", IpcEndpointKind::ScopedRequest, Some(ServerArgument::Optional)),
    ep(IpcGroupName::ChannelMemories, "listChannelMemories", "agent:channel-memories:list", IpcEndpointKind::ScopedRequest, Some(ServerArgument::Optional)),
    ep(IpcGroupName::ChannelFolders, "listChannelFolders", "agent:channel-folders:list", IpcEndpointKind::ScopedRequest, Some(ServerArgument::Optional)),
    ep(IpcGroupName::ChannelRoutines, "listChannelRoutines", "agent:channel-routines:list", IpcEndpointKind::ScopedRequest, Some(ServerArgument::Optional)),
    ep(IpcGroupName::Workflows, "list", "workflows:list", IpcEndpointKind::ScopedQuery, Some(ServerArgument::Optional)),
    ep(IpcGroupName::Workflows, "saveGraph", "workflows:save-graph", IpcEndpointKind::ScopedRequest, Some(ServerArgument::Optional)),
    ep(IpcGroupName::Crm, "overview", "crm:overview", IpcEndpointKind::ScopedQuery, Some(ServerArgument::Optional)),
    ep(IpcGroupName::Crm, "apply", "crm:apply", IpcEndpointKind::ScopedRequest, Some(ServerArgument::Optional)),
    ep(IpcGroupName::RoutineResults, "options", "routine-results:options", IpcEndpointKind::ScopedQuery, Some(ServerArgument::Required)),
    ep(IpcGroupName::AgentAttachments, "importAttachmentData", "agent:import-attachment-data", IpcEndpointKind::ScopedRequest, Some(ServerArgument::Optional)),
    ep(IpcGroupName::AgentFolders, "listAgentFolders", "agent:folders:list", IpcEndpointKind::ScopedRequest, Some(ServerArgument::Optional)),
    ep(IpcGroupName::AgentInstructions, "readAgentInstructions", "agent:instructions:read", IpcEndpointKind::ScopedRequest, Some(ServerArgument::Optional)),
    ep(IpcGroupName::AttachmentImports, "importAttachments", "agent:import-attachments", IpcEndpointKind::ScopedRequest, Some(ServerArgument::Optional)),
    ep(IpcGroupName::Browser, "open", "browser:open", IpcEndpointKind::Request, None),
    ep(IpcGroupName::BrowserInput, "sendLiveViewInput", "browser:send-live-view-input", IpcEndpointKind::UntypedRequest, None),
    ep(IpcGroupName::Servers, "list", "servers:list", IpcEndpointKind::Request, None),
    ep(IpcGroupName::AgentAdmin, "getAgentHostSettings", "agent:admin:get-host-settings", IpcEndpointKind::ScopedRequest, Some(ServerArgument::Optional)),
    ep(IpcGroupName::McpServers, "listMcpServers", "servers:mcp:list", IpcEndpointKind::ScopedQuery, Some(ServerArgument::Required)),
    ep(IpcGroupName::Storage, "getUsage", "storage:get-usage", IpcEndpointKind::ScopedRequest, Some(ServerArgument::Required)),
    ep(IpcGroupName::AgentImport, "choose", "agent-import:choose", IpcEndpointKind::ScopedQuery, Some(ServerArgument::Required)),
    ep(IpcGroupName::InstanceTransfer, "summary", "instance-transfer:summary", IpcEndpointKind::ScopedQuery, Some(ServerArgument::Required)),
    ep(IpcGroupName::Plugins, "takePendingListing", "plugins:take-pending-listing", IpcEndpointKind::Request, None),
    ep(IpcGroupName::Marketplace, "takePendingListing", "marketplace:take-pending-listing", IpcEndpointKind::Request, None),
    ep(IpcGroupName::Host, "getStatus", "host:get-status", IpcEndpointKind::Request, None),
    ep(IpcGroupName::Events, "getStatus", "events:get-status", IpcEndpointKind::ScopedQuery, Some(ServerArgument::Required)),
    ep(IpcGroupName::RemoteDesktop, "list", "remote-desktop:list", IpcEndpointKind::Request, None),
];

pub fn endpoints_in_group(group: IpcGroupName) -> Vec<&'static IpcEndpointDescriptor> {
    IPC_ENDPOINTS.iter().filter(|d| d.group == group).collect()
}

pub fn endpoint_by_channel(channel: &str) -> Option<&'static IpcEndpointDescriptor> {
    IPC_ENDPOINTS.iter().find(|d| d.channel == channel)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum AgentTurnOrigin {
    User,
    Routine,
    Agent,
    Unknown,
}

fn deserialize_optional_true<'de, D>(deserializer: D) -> Result<Option<bool>, D::Error>
where
    D: Deserializer<'de>,
{
    let val = bool::deserialize(deserializer)?;
    if !val {
        return Err(serde::de::Error::custom(
            "optional flag must be true or omitted",
        ));
    }
    Ok(Some(true))
}

fn deserialize_agent_input_kind<'de, D>(deserializer: D) -> Result<String, D::Error>
where
    D: Deserializer<'de>,
{
    let kind = String::deserialize(deserializer)?;
    if !matches!(kind.as_str(), "prompt" | "approval") {
        return Err(serde::de::Error::custom(
            "AgentInputResolved.kind must be 'prompt' or 'approval'",
        ));
    }
    Ok(kind)
}

fn deserialize_browser_permission<'de, D>(deserializer: D) -> Result<String, D::Error>
where
    D: Deserializer<'de>,
{
    let perm = String::deserialize(deserializer)?;
    if perm != "geolocation" {
        return Err(serde::de::Error::custom(
            "BrowserPermissionEntry.permission must be 'geolocation'",
        ));
    }
    Ok(perm)
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentTurnCompletedPayload {
    pub agent_id: String,
    pub thread_id: String,
    pub turn_id: String,
    pub status: String,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "non_null_option"
    )]
    pub origin: Option<AgentTurnOrigin>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "deserialize_optional_true"
    )]
    pub quiet: Option<bool>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "deserialize_optional_true"
    )]
    pub continues: Option<bool>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BrowserPageStateEntry {
    pub tab_id: String,
    pub can_go_back: bool,
    pub can_go_forward: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BrowserPermissionEntry {
    pub tab_id: String,
    pub id: String,
    pub origin: String,
    #[serde(deserialize_with = "deserialize_browser_permission")]
    pub permission: String,
}

impl BrowserPermissionEntry {
    pub fn is_valid(&self) -> bool {
        self.permission == "geolocation"
    }
}

/// All 55 `AgentEvent` wire variants (`ipc-agent-events.ts:L68-L197`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "kebab-case")]
pub enum AgentEvent {
    ChannelsChanged {
        #[serde(rename = "channelId")]
        channel_id: String,
        revision: u64,
    },
    Status {
        status: serde_json::Value,
    },
    UsageChanged {
        usage: serde_json::Value,
    },
    AgentsChanged {
        agents: Vec<ClientAgentSummary>,
    },
    MemoriesChanged {
        #[serde(rename = "agentId")]
        agent_id: String,
    },
    RoutinesChanged {
        #[serde(rename = "agentId")]
        agent_id: String,
    },
    SkillsChanged {
        #[serde(rename = "agentId")]
        agent_id: String,
    },
    RoutineFlowsChanged {
        #[serde(rename = "agentId")]
        agent_id: String,
    },
    WorkflowsChanged {
        #[serde(rename = "workflowId")]
        workflow_id: String,
        #[serde(
            rename = "agentId",
            default,
            skip_serializing_if = "Option::is_none",
            deserialize_with = "non_null_option"
        )]
        agent_id: Option<String>,
    },
    CrmChanged,
    VaultChanged,
    BrowserProfilesChanged,
    HostTurnLimitsChanged,
    LibraryFoldersChanged,
    WorkflowFoldersChanged,
    UsageCreditsChanged,
    OpenbotCreditsChanged,
    AgentSessionSettingsChanged {
        #[serde(rename = "agentId")]
        agent_id: String,
    },
    MessagePinsChanged {
        #[serde(rename = "agentId")]
        agent_id: String,
    },
    ChannelMemoriesChanged {
        #[serde(rename = "channelId")]
        channel_id: String,
    },
    ChannelRoutinesChanged {
        #[serde(rename = "channelId")]
        channel_id: String,
    },
    SidebarLayoutChanged {
        layout: serde_json::Value,
    },
    HiddenAgentsChanged {
        agents: Vec<HiddenAgent>,
    },
    AgentColorsChanged {
        agents: Vec<serde_json::Value>,
    },
    ChatColorsChanged {
        agents: Vec<serde_json::Value>,
        members: Vec<serde_json::Value>,
    },
    AgentNotificationLevelsChanged {
        #[serde(deserialize_with = "Option::deserialize")]
        server: Option<String>,
        agents: Vec<serde_json::Value>,
    },
    SidebarPinsChanged {
        items: Vec<serde_json::Value>,
        #[serde(rename = "updatedAt", deserialize_with = "Option::deserialize")]
        updated_at: Option<String>,
    },
    AgentAccessChanged {
        revision: u64,
    },
    Conversation {
        snapshot: ConversationSnapshot,
    },
    ConversationInvalidated {
        #[serde(rename = "agentId")]
        agent_id: String,
        revision: u64,
    },
    ConversationPage {
        page: serde_json::Value,
    },
    ConversationDelta {
        #[serde(rename = "agentId")]
        agent_id: String,
        #[serde(rename = "threadId")]
        thread_id: String,
        #[serde(rename = "turnId")]
        turn_id: String,
        #[serde(rename = "messageId")]
        message_id: String,
        delta: String,
        #[serde(rename = "createdAt")]
        created_at: String,
        revision: u64,
    },
    QueueInvalidated {
        #[serde(rename = "agentId")]
        agent_id: String,
    },
    QueueChanged {
        snapshot: QueueSnapshot,
    },
    WaitsChanged {
        snapshot: serde_json::Value,
    },
    AgentTasksChanged {
        snapshot: AgentTasksSnapshot,
    },
    AgentTaskConversation {
        #[serde(rename = "taskId")]
        task_id: String,
        snapshot: ConversationSnapshot,
    },
    AgentTaskConversationDelta {
        #[serde(rename = "taskId")]
        task_id: String,
        #[serde(rename = "agentId")]
        agent_id: String,
        #[serde(rename = "threadId")]
        thread_id: String,
        #[serde(rename = "turnId")]
        turn_id: String,
        #[serde(rename = "messageId")]
        message_id: String,
        delta: String,
        #[serde(rename = "createdAt")]
        created_at: String,
        revision: u64,
    },
    TurnProgress {
        #[serde(rename = "agentId")]
        agent_id: String,
        #[serde(rename = "threadId")]
        thread_id: String,
        #[serde(rename = "turnId")]
        turn_id: String,
        detail: String,
        #[serde(
            default,
            skip_serializing_if = "Option::is_none",
            deserialize_with = "non_null_option"
        )]
        running: Option<bool>,
        #[serde(
            default,
            skip_serializing_if = "Option::is_none",
            deserialize_with = "non_null_option"
        )]
        activity: Option<String>,
        #[serde(
            default,
            skip_serializing_if = "Option::is_none",
            deserialize_with = "non_null_option"
        )]
        error: Option<String>,
    },
    TurnStarted {
        #[serde(rename = "agentId")]
        agent_id: String,
        #[serde(rename = "threadId")]
        thread_id: String,
        #[serde(rename = "turnId")]
        turn_id: String,
        #[serde(
            default,
            skip_serializing_if = "Option::is_none",
            deserialize_with = "non_null_option"
        )]
        origin: Option<AgentTurnOrigin>,
    },
    TurnCompleted(AgentTurnCompletedPayload),
    ThreadTurnCompleted {
        #[serde(rename = "taskId")]
        task_id: String,
        routine: bool,
        turn: serde_json::Value,
    },
    Prompt {
        #[serde(rename = "requestId")]
        request_id: RequestId,
        #[serde(rename = "agentId")]
        agent_id: String,
        #[serde(rename = "threadId")]
        thread_id: String,
        #[serde(rename = "turnId")]
        turn_id: String,
        questions: Vec<AgentPromptQuestion>,
        #[serde(
            default,
            skip_serializing_if = "Option::is_none",
            deserialize_with = "deserialize_optional_true"
        )]
        detached: Option<bool>,
    },
    AgentInputResolved {
        #[serde(deserialize_with = "deserialize_agent_input_kind")]
        kind: String,
        #[serde(rename = "requestId")]
        request_id: RequestId,
        #[serde(rename = "agentId")]
        agent_id: String,
    },
    BrowserTakeoverRequested {
        request: serde_json::Value,
    },
    BrowserTakeoverResolved {
        #[serde(rename = "requestId")]
        request_id: RequestId,
        #[serde(rename = "agentId")]
        agent_id: String,
    },
    Approval {
        approval: serde_json::Value,
    },
    RuntimeSnapshot {
        snapshot: serde_json::Value,
    },
    BrowserChanged {
        tabs: Vec<serde_json::Value>,
        #[serde(rename = "activeTabId", deserialize_with = "Option::deserialize")]
        active_tab_id: Option<String>,
    },
    BrowserControlChanged {
        state: serde_json::Value,
    },
    BrowserUserControlChanged {
        #[serde(rename = "tabIds")]
        tab_ids: Vec<String>,
    },
    BrowserPageStateChanged {
        tabs: Vec<BrowserPageStateEntry>,
    },
    BrowserPermissionsChanged {
        requests: Vec<BrowserPermissionEntry>,
    },
    Error {
        #[serde(
            rename = "agentId",
            default,
            skip_serializing_if = "Option::is_none",
            deserialize_with = "non_null_option"
        )]
        agent_id: Option<String>,
        code: String,
        message: String,
        #[serde(
            default,
            skip_serializing_if = "Option::is_none",
            deserialize_with = "non_null_option"
        )]
        provider: Option<AgentProviderId>,
        #[serde(
            default,
            skip_serializing_if = "Option::is_none",
            deserialize_with = "non_null_option"
        )]
        model: Option<String>,
        #[serde(
            default,
            skip_serializing_if = "Option::is_none",
            deserialize_with = "non_null_option"
        )]
        link: Option<String>,
    },
    UsageLimitReached {
        #[serde(rename = "agentId")]
        agent_id: String,
        provider: AgentProviderId,
        #[serde(rename = "resetsAt", deserialize_with = "Option::deserialize")]
        resets_at: Option<f64>,
        #[serde(rename = "agentCount")]
        agent_count: u64,
    },
    SubagentFailed {
        #[serde(rename = "agentId")]
        agent_id: String,
        #[serde(rename = "threadId")]
        thread_id: String,
        #[serde(rename = "turnId")]
        turn_id: String,
        #[serde(rename = "subagentId")]
        subagent_id: String,
        title: String,
    },
}

impl AgentEvent {
    pub fn is_valid(&self) -> bool {
        match self {
            Self::AgentInputResolved { kind, .. } => matches!(kind.as_str(), "prompt" | "approval"),
            Self::BrowserPermissionsChanged { requests } => {
                requests.iter().all(BrowserPermissionEntry::is_valid)
            }
            Self::TurnCompleted(payload) => {
                payload.quiet.is_none_or(|q| q) && payload.continues.is_none_or(|c| c)
            }
            Self::Prompt { detached, .. } => detached.is_none_or(|d| d),
            _ => true,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ScopedAgentEvent {
    pub server_id: String,
    pub event: AgentEvent,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "non_null_option"
    )]
    pub buffered_live: Option<bool>,
}

impl ScopedAgentEvent {
    pub fn is_valid(&self) -> bool {
        is_identifier(&self.server_id) && self.event.is_valid()
    }
}
