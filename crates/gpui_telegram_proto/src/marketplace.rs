//! Wire protocol DTOs for the live server-published OpenBot Marketplace
//! (`/api/v1/marketplace/*`): published connectors, MCP plugins, agents, skills,
//! user-published connectors, real live connection verification, and live tool calls.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Full schema of an executable tool discovered from a real MCP server or verified native connector.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MarketplaceToolSchemaDto {
    pub name: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub input_schema: Value,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MarketplaceAuthFieldDto {
    pub id: String,
    pub label: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub env: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub header: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub prefix: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub placeholder: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hint: Option<String>,
    #[serde(default)]
    pub optional: bool,
    #[serde(default)]
    pub url: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MarketplacePromptDto {
    pub id: String,
    pub text: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub zh_text: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MarketplaceEnvPairDto {
    pub key: String,
    pub value: String,
}

/// Real server-published connector or MCP plugin listing stored in `marketplace_apps`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MarketplaceAppListingDto {
    pub app_id: String,
    pub slug: String,
    /// `"connector"`, `"plugin"`, or `"custom_published"`.
    pub kind: String,
    pub name: String,
    pub tagline: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub zh_tagline: Option<String>,
    pub description: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub zh_description: Option<String>,
    pub category: String,
    pub creator_name: String,
    pub version: String,
    pub brand_hex: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub website_url: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub privacy_policy_url: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub terms_url: Option<String>,
    pub server_name: String,
    /// `"http"` or `"stdio"`.
    pub transport: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub command: Option<String>,
    #[serde(default)]
    pub args: Vec<String>,
    #[serde(default)]
    pub env: Vec<MarketplaceEnvPairDto>,
    /// `"open"`, `"key"`, `"link"`, `"local"`, or native setup kind (`"channel"`, `"vault"`, `"account"`).
    pub auth_kind: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub auth_label: Option<String>,
    #[serde(default)]
    pub auth_fields: Vec<MarketplaceAuthFieldDto>,
    #[serde(default)]
    pub auth_steps: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub auth_note: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub auth_docs_url: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub auth_docs_label: Option<String>,
    #[serde(default)]
    pub prompts: Vec<MarketplacePromptDto>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub setup_kind: Option<String>,
    pub verified: bool,
    pub install_count: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub publisher_user_id: Option<u64>,
    pub published_at_ms: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MarketplaceAgentListingDto {
    pub slug: String,
    pub name: String,
    pub title: String,
    pub description: String,
    pub category: String,
    pub creator_name: String,
    pub avatar_hue: u16,
    #[serde(default)]
    pub skills: Vec<String>,
    pub verified: bool,
    pub featured: bool,
    pub version: u32,
    pub install_count: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MarketplaceSkillListingDto {
    pub slug: String,
    pub name: String,
    pub description: String,
    pub category: String,
    pub creator_name: String,
    pub source: String,
    pub verified: bool,
    pub featured: bool,
    pub version: u32,
    pub install_count: u64,
    pub skill_md: String,
}

/// Real per-user connection state persisted in `marketplace_user_connections` (credentials never returned).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MarketplaceConnectionDto {
    pub app_id: String,
    pub kind: String,
    pub name: String,
    /// `"connected"`, `"needs_sign_in"`, `"needs_key"`, `"error"`, or `"pairing_required"`.
    pub status: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub account_label: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub endpoint: Option<String>,
    pub transport: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mcp_server_id: Option<String>,
    #[serde(default)]
    pub discovered_tools: Vec<MarketplaceToolSchemaDto>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_error: Option<String>,
    #[serde(default)]
    pub metadata: BTreeMap<String, String>,
    pub connected_at_ms: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub verified_at_ms: Option<i64>,
}

/// Response for `GET /api/v1/marketplace/catalog`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MarketplaceCatalogResponse {
    pub apps: Vec<MarketplaceAppListingDto>,
    pub agents: Vec<MarketplaceAgentListingDto>,
    pub skills: Vec<MarketplaceSkillListingDto>,
    pub connections: Vec<MarketplaceConnectionDto>,
    pub total_apps: usize,
    pub total_connectors: usize,
    pub total_plugins: usize,
    pub total_published_custom: usize,
    pub synced_at_ms: i64,
}

/// Request body for `POST /api/v1/marketplace/apps` to publish a real connector/MCP app to the marketplace DB.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PublishMarketplaceAppRequest {
    pub name: String,
    #[serde(default)]
    pub slug: Option<String>,
    pub tagline: String,
    #[serde(default)]
    pub zh_tagline: Option<String>,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub zh_description: Option<String>,
    #[serde(default)]
    pub category: Option<String>,
    #[serde(default)]
    pub brand_hex: Option<String>,
    #[serde(default)]
    pub website_url: Option<String>,
    /// `"http"` or `"stdio"`.
    pub transport: String,
    #[serde(default)]
    pub url: Option<String>,
    #[serde(default)]
    pub command: Option<String>,
    #[serde(default)]
    pub args: Vec<String>,
    #[serde(default)]
    pub env: Vec<MarketplaceEnvPairDto>,
    /// `"open"`, `"key"`, `"link"`, or `"local"`.
    #[serde(default)]
    pub auth_kind: Option<String>,
    #[serde(default)]
    pub auth_fields: Vec<MarketplaceAuthFieldDto>,
    #[serde(default)]
    pub verify_before_publish: bool,
}

/// Request body for `POST /api/v1/marketplace/connections/connect`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ConnectMarketplaceAppRequest {
    pub app_id: String,
    #[serde(default)]
    pub slug: Option<String>,
    /// `"connector"`, `"plugin"`, or `"custom"`.
    #[serde(default)]
    pub kind: Option<String>,
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub account_label: Option<String>,
    #[serde(default)]
    pub credential: Option<String>,
    #[serde(default)]
    pub auth_values: BTreeMap<String, String>,
    #[serde(default)]
    pub override_url: Option<String>,
    #[serde(default)]
    pub transport: Option<String>,
    #[serde(default)]
    pub command: Option<String>,
    #[serde(default)]
    pub args: Vec<String>,
    #[serde(default)]
    pub headers: Vec<MarketplaceEnvPairDto>,
    #[serde(default)]
    pub metadata: BTreeMap<String, String>,
    /// When `true` (default for remote HTTP / native APIs), the server verifies live against upstream.
    #[serde(default = "default_true")]
    pub verify_live: bool,
    /// Optional locally-verified tools reported by the desktop client for local/stdio servers.
    #[serde(default)]
    pub local_discovered_tools: Vec<MarketplaceToolSchemaDto>,
}

fn default_true() -> bool {
    true
}

/// Response body for `POST /api/v1/marketplace/connections/connect` and `POST .../probe`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ConnectMarketplaceAppResponse {
    pub connection: MarketplaceConnectionDto,
    pub install_count: u64,
    #[serde(default)]
    pub requires_oauth: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub www_authenticate: Option<String>,
}

/// Request body for `POST /api/v1/marketplace/connections/{app_id}/call`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CallMarketplaceToolRequest {
    pub tool_name: String,
    #[serde(default)]
    pub arguments: Value,
}

/// Response body for `POST /api/v1/marketplace/connections/{app_id}/call`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CallMarketplaceToolResponse {
    pub app_id: String,
    pub tool_name: String,
    pub is_error: bool,
    pub result: Value,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MarketplaceInstallResponse {
    pub slug: String,
    pub install_count: u64,
}
