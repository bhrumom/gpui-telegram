//! Full 1:1 OpenBot Marketplace catalog (`apps`, `agents`, `skills`), authentic embedded
//! upstream OpenBot catalog (9 native connectors + 257 MCP plugins = 266 apps, 29 production
//! agents, 40 production skills with `SKILL.md`), live connector/plugin persistence
//! (`openbot-connectors-v1.json`), MCP Gateway wiring, `"fishing"` empty-state fallback,
//! and atomic install receipts (`MarketplaceService`, `OB-F12`).

use std::collections::{BTreeMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Arc, OnceLock};

use chrono::{SecondsFormat, Utc};
use openbot_storage::{
    GITHUB_CONNECTOR_MCP_SERVER_ID, GITHUB_CONNECTOR_MCP_SERVER_NAME,
    GITHUB_CONNECTOR_MCP_SERVER_URL, McpKeyValue, McpServerConfig, McpTransport,
    write_json_file_atomically,
};
use parking_lot::Mutex;
use serde::{Deserialize, Serialize};

use crate::error::{OpenBotRuntimeError, RuntimeResult};
use crate::platform::connector_driver::{
    native_connector_real_endpoint, native_connector_tools, verify_native_connector,
};
use crate::skill_mcp_service::{
    McpDiscoveredTool, McpGateway, McpProbeOutcome, RegisterMcpServerInput,
};

pub const MARKETPLACE_TABS: [&str; 3] = ["apps", "agents", "skills"];
pub const SKILL_CATEGORIES: [&str; 8] = [
    "productivity",
    "documents",
    "design",
    "research",
    "data-analytics",
    "automation",
    "coding",
    "other",
];
pub const EMPTY_STATE_ALPHA_FALLBACK: &str = "fishing";
pub const FISHING_VIDEO_ASSET: &str = "fishing.webm";
pub const FISHING_STILL_ASSET: &str = "fishing-still.webp";

const INSTALLS_FILE_NAME: &str = "marketplace-installs-v1.json";
const CONNECTORS_FILE_NAME: &str = "openbot-connectors-v1.json";
const INSTALLED_SKILLS_FILE_NAME: &str = "openbot-installed-skills-v1.json";

const EMBEDDED_OPENBOT_CATALOG_JSON: &str =
    include_str!("../../assets/openbot_marketplace_catalog.json");

static OPENBOT_CATALOG: OnceLock<OpenBotCatalog> = OnceLock::new();

// ============================================================================
// Authentic Upstream OpenBot Catalog Schema (9 Connectors, 257 Plugins, 29 Agents, 40 Skills)
// ============================================================================

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OpenBotConnectorCatalogItem {
    pub id: String,
    pub connector_id: String,
    pub slug: String,
    pub name: String,
    pub tagline: String,
    pub zh_tagline: String,
    pub description: String,
    pub zh_description: String,
    pub category: String,
    pub creator_name: String,
    pub brand_hex: String,
    pub setup_kind: String,
    #[serde(default)]
    pub website_url: Option<String>,
    #[serde(default)]
    pub privacy_policy_url: Option<String>,
    #[serde(default)]
    pub terms_url: Option<String>,
    pub installs: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OpenBotPluginAuthField {
    pub id: String,
    pub label: String,
    #[serde(default)]
    pub env: Option<String>,
    #[serde(default)]
    pub header: Option<String>,
    #[serde(default)]
    pub prefix: Option<String>,
    #[serde(default)]
    pub placeholder: Option<String>,
    #[serde(default)]
    pub hint: Option<String>,
    #[serde(default)]
    pub optional: bool,
    #[serde(default)]
    pub url: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OpenBotPluginPrompt {
    pub id: String,
    pub text: String,
    #[serde(default)]
    pub zh_text: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OpenBotPluginEnvPair {
    pub key: String,
    pub value: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OpenBotPluginCatalogItem {
    pub id: String,
    pub slug: String,
    pub name: String,
    pub tagline: String,
    #[serde(default)]
    pub zh_tagline: Option<String>,
    pub description: String,
    #[serde(default)]
    pub zh_description: Option<String>,
    pub category: String,
    pub creator_name: String,
    pub version: String,
    pub brand_hex: String,
    #[serde(default)]
    pub website_url: Option<String>,
    #[serde(default)]
    pub privacy_policy_url: Option<String>,
    #[serde(default)]
    pub terms_url: Option<String>,
    pub server_name: String,
    pub transport: String,
    #[serde(default)]
    pub url: Option<String>,
    #[serde(default)]
    pub command: Option<String>,
    #[serde(default)]
    pub args: Vec<String>,
    #[serde(default)]
    pub env: Vec<OpenBotPluginEnvPair>,
    pub auth_kind: String,
    #[serde(default)]
    pub auth_label: Option<String>,
    #[serde(default)]
    pub auth_fields: Vec<OpenBotPluginAuthField>,
    #[serde(default)]
    pub auth_steps: Vec<String>,
    #[serde(default)]
    pub auth_note: Option<String>,
    #[serde(default)]
    pub auth_docs_url: Option<String>,
    #[serde(default)]
    pub auth_docs_label: Option<String>,
    #[serde(default)]
    pub prompts: Vec<OpenBotPluginPrompt>,
    pub installs: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OpenBotAgentRoutineSpec {
    pub name: String,
    pub instruction: String,
    pub active: bool,
    pub schedule_kind: String,
    pub schedule_time: String,
    pub schedule_summary: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OpenBotAgentCatalogItem {
    pub id: String,
    pub slug: String,
    pub name: String,
    pub title: String,
    pub description: String,
    pub category: String,
    pub creator_name: String,
    pub avatar_hue: u16,
    #[serde(default)]
    pub skills: Vec<String>,
    #[serde(default)]
    pub routines: Vec<OpenBotAgentRoutineSpec>,
    pub verified: bool,
    pub featured: bool,
    pub version: u32,
    pub installs: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OpenBotSkillCatalogItem {
    pub id: String,
    pub slug: String,
    pub name: String,
    pub description: String,
    pub category: String,
    pub creator_name: String,
    pub source: String,
    pub verified: bool,
    pub featured: bool,
    pub version: u32,
    pub installs: u64,
    pub skill_md: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OpenBotCatalog {
    pub schema_version: u32,
    pub source_repo: String,
    pub connectors: Vec<OpenBotConnectorCatalogItem>,
    pub plugins: Vec<OpenBotPluginCatalogItem>,
    pub agents: Vec<OpenBotAgentCatalogItem>,
    pub skills: Vec<OpenBotSkillCatalogItem>,
}

/// Returns the cached, statically embedded full OpenBot Marketplace catalog.
pub fn openbot_marketplace_catalog() -> &'static OpenBotCatalog {
    OPENBOT_CATALOG.get_or_init(|| {
        serde_json::from_str::<OpenBotCatalog>(EMBEDDED_OPENBOT_CATALOG_JSON)
            .expect("embedded openbot_marketplace_catalog.json must be valid JSON")
    })
}

/// Persisted connection receipt for an active connector app, MCP plugin, or custom server.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ConnectedAppRecord {
    pub app_id: String,
    pub kind: String,
    pub name: String,
    pub status: String,
    pub connected_at: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mcp_server_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub account_label: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub endpoint: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub discovered_tools: Vec<McpDiscoveredTool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_error: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub verified_at: Option<String>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub metadata: BTreeMap<String, String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ConnectedAppsFile {
    version: u32,
    apps: BTreeMap<String, ConnectedAppRecord>,
}

// ============================================================================
// Existing Search / Install Surface (Backward-Compatible `OB-F12`)
// ============================================================================

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum MarketplaceTab {
    Apps,
    Agents,
    Skills,
}

impl MarketplaceTab {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Apps => "apps",
            Self::Agents => "agents",
            Self::Skills => "skills",
        }
    }

    fn parse(raw: &str) -> RuntimeResult<Self> {
        match raw.trim().to_ascii_lowercase().as_str() {
            "apps" => Ok(Self::Apps),
            "agents" => Ok(Self::Agents),
            "skills" => Ok(Self::Skills),
            other => Err(OpenBotRuntimeError::Validation(format!(
                "Invalid marketplace tab '{other}'; expected one of apps, agents, skills."
            ))),
        }
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MarketplaceSearchRequest {
    pub tab: String,
    pub source: Option<String>,
    pub query: Option<String>,
    pub category: Option<String>,
    pub reduced_motion: bool,
    pub alpha_unsupported: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MarketplaceItem {
    pub id: String,
    pub slug: String,
    pub tab: MarketplaceTab,
    pub name: String,
    pub tagline: String,
    pub description: String,
    pub category: String,
    pub creator_name: String,
    pub verified: bool,
    pub featured: bool,
    pub installs: u64,
    pub kind: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MarketplaceSearchOutcome {
    pub tab: String,
    pub tabs: Vec<String>,
    pub source: Option<String>,
    pub query: Option<String>,
    pub items: Vec<MarketplaceItem>,
    pub empty_state: bool,
    pub alpha_fallback: String,
    pub plush_asset: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct InstallReceipt {
    id: String,
    slug: String,
    tab: MarketplaceTab,
    name: String,
    installed_at: String,
    count: u64,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct MarketplaceInstallsFile {
    version: u32,
    installs: BTreeMap<String, InstallReceipt>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct LocalInstalledSkillsFile {
    #[serde(default)]
    by_agent: BTreeMap<String, Vec<LocalInstalledSkillEntry>>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct LocalInstalledSkillEntry {
    skill_id: String,
    slug: String,
    name: String,
    #[serde(default)]
    description: String,
}

pub struct MarketplaceService {
    data_dir: PathBuf,
    installs_path: PathBuf,
    connectors_path: PathBuf,
    installs: Mutex<MarketplaceInstallsFile>,
    connected_apps: Mutex<ConnectedAppsFile>,
}

impl MarketplaceService {
    pub fn open(data_dir: impl AsRef<Path>) -> RuntimeResult<Arc<Self>> {
        let dir = data_dir.as_ref().to_path_buf();
        let installs_path = dir.join(INSTALLS_FILE_NAME);
        let connectors_path = dir.join(CONNECTORS_FILE_NAME);
        let installs = load_installs_file(&installs_path)?;
        let connected_apps = load_connected_apps_file(&connectors_path)?;
        Ok(Arc::new(Self {
            data_dir: dir,
            installs_path,
            connectors_path,
            installs: Mutex::new(installs),
            connected_apps: Mutex::new(connected_apps),
        }))
    }

    pub fn catalog(&self) -> &'static OpenBotCatalog {
        openbot_marketplace_catalog()
    }

    pub fn list_connected_apps(&self) -> Vec<ConnectedAppRecord> {
        let guard = self.connected_apps.lock();
        guard.apps.values().cloned().collect()
    }

    pub fn get_connected_app(&self, app_id: &str) -> Option<ConnectedAppRecord> {
        let guard = self.connected_apps.lock();
        guard.apps.get(app_id).cloned()
    }

    pub fn is_app_connected(&self, app_id: &str) -> bool {
        self.connected_apps.lock().apps.contains_key(app_id)
    }

    pub fn list_installed_slugs(&self, tab: MarketplaceTab) -> Vec<String> {
        let guard = self.installs.lock();
        guard
            .installs
            .values()
            .filter(|rec| rec.tab == tab)
            .map(|rec| rec.slug.clone())
            .collect()
    }

    pub fn save_connected_app(
        &self,
        record: ConnectedAppRecord,
    ) -> RuntimeResult<ConnectedAppRecord> {
        let mut guard = self.connected_apps.lock();
        guard.version = 1;
        guard.apps.insert(record.app_id.clone(), record.clone());
        write_json_file_atomically(&self.connectors_path, &*guard, true)?;
        Ok(record)
    }

    pub fn disconnect_app(&self, app_id: &str) -> RuntimeResult<Option<ConnectedAppRecord>> {
        let mut guard = self.connected_apps.lock();
        let removed = guard.apps.remove(app_id);
        if removed.is_some() {
            guard.version = 1;
            write_json_file_atomically(&self.connectors_path, &*guard, true)?;
        }
        Ok(removed)
    }

    /// Connects an upstream OpenBot plugin app by validating auth fields, registering its real
    /// `McpServerConfig` in [`McpGateway`], persisting a [`ConnectedAppRecord`], and recording an
    /// install receipt.
    pub fn connect_plugin_via_mcp(
        &self,
        mcp: &McpGateway,
        slug: &str,
        auth_values: &BTreeMap<String, String>,
        override_url: Option<&str>,
    ) -> RuntimeResult<(ConnectedAppRecord, McpServerConfig)> {
        let catalog = self.catalog();
        let plugin = catalog
            .plugins
            .iter()
            .find(|p| p.slug.eq_ignore_ascii_case(slug) || p.id.eq_ignore_ascii_case(slug))
            .ok_or_else(|| {
                OpenBotRuntimeError::NotFound(format!(
                    "Marketplace plugin '{slug}' was not found."
                ))
            })?;

        let mut env_pairs: Vec<McpKeyValue> = plugin
            .env
            .iter()
            .map(|e| McpKeyValue {
                key: e.key.clone(),
                value: e.value.clone(),
            })
            .collect();
        let mut headers: Vec<McpKeyValue> = Vec::new();
        let mut metadata = BTreeMap::new();

        for field in &plugin.auth_fields {
            let val = auth_values
                .get(&field.id)
                .map(|v| v.trim())
                .unwrap_or("");
            if val.is_empty() && !field.optional {
                return Err(OpenBotRuntimeError::Validation(format!(
                    "Missing required credential field '{}'.",
                    field.label
                )));
            }
            if val.is_empty() {
                continue;
            }
            if let Some(env_key) = &field.env {
                let full_val = match &field.prefix {
                    Some(pref) if !val.starts_with(pref) => format!("{pref}{val}"),
                    _ => val.to_string(),
                };
                if let Some(existing) = env_pairs.iter_mut().find(|p| p.key == *env_key) {
                    existing.value = full_val;
                } else {
                    env_pairs.push(McpKeyValue {
                        key: env_key.clone(),
                        value: full_val,
                    });
                }
            } else if let Some(header_key) = &field.header {
                let full_val = match &field.prefix {
                    Some(pref) if !val.starts_with(pref) => format!("{pref}{val}"),
                    _ => val.to_string(),
                };
                headers.push(McpKeyValue {
                    key: header_key.clone(),
                    value: full_val,
                });
            }
            metadata.insert(format!("field_{}_configured", field.id), "true".to_string());
        }

        // Allow optional bearer token paste for `link` (OAuth) apps as well.
        if let Some(token) = auth_values
            .get("oauth_token")
            .map(|v| v.trim())
            .filter(|v| !v.is_empty())
        {
            let value = if token.to_ascii_lowercase().starts_with("bearer ") {
                token.to_string()
            } else {
                format!("Bearer {token}")
            };
            headers.push(McpKeyValue {
                key: "Authorization".to_string(),
                value,
            });
            metadata.insert("oauth_token_configured".to_string(), "true".to_string());
        }

        let transport = if plugin.transport.eq_ignore_ascii_case("stdio") {
            McpTransport::Stdio
        } else {
            McpTransport::Http
        };

        let effective_url = override_url
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::to_string)
            .or_else(|| plugin.url.clone());

        let existing_server_id = self
            .get_connected_app(&plugin.id)
            .and_then(|rec| rec.mcp_server_id)
            .or_else(|| {
                mcp.get_server(&plugin.server_name)
                    .ok()
                    .flatten()
                    .map(|s| s.id)
            });

        let saved_server = mcp.register_server(RegisterMcpServerInput {
            id: existing_server_id,
            name: plugin.server_name.clone(),
            transport,
            enabled: Some(true),
            command: plugin.command.clone(),
            args: plugin.args.clone(),
            env: env_pairs,
            env_passthrough: Vec::new(),
            working_directory: None,
            url: effective_url.clone(),
            headers,
        })?;

        let account_label = effective_url
            .clone()
            .or_else(|| plugin.command.clone())
            .or_else(|| Some(plugin.name.clone()));
        let record = ConnectedAppRecord {
            app_id: plugin.id.clone(),
            kind: "plugin".to_string(),
            name: plugin.name.clone(),
            status: "connected".to_string(),
            connected_at: Utc::now().to_rfc3339_opts(SecondsFormat::Millis, true),
            mcp_server_id: Some(saved_server.id.clone()),
            account_label,
            endpoint: effective_url.or_else(|| plugin.command.clone()),
            discovered_tools: Vec::new(),
            last_error: None,
            verified_at: None,
            metadata,
        };
        let saved_record = self.save_connected_app(record)?;
        let _ = self.install_item(MarketplaceTab::Apps, &plugin.slug);
        Ok((saved_record, saved_server))
    }

    /// Connects an upstream OpenBot MCP plugin AND performs a real live MCP handshake
    /// (`initialize` + `notifications/initialized` + `tools/list`) via [`McpGateway::probe_server`].
    /// Never fakes `connected`: if the live server returns HTTP 401/403 or fails to launch/respond,
    /// the error/auth status is surfaced truthfully and the broken connection is not marked connected.
    pub async fn connect_plugin_via_mcp_verified(
        &self,
        mcp: &McpGateway,
        slug: &str,
        auth_values: &BTreeMap<String, String>,
        override_url: Option<&str>,
    ) -> RuntimeResult<(ConnectedAppRecord, McpServerConfig, McpProbeOutcome)> {
        let (mut record, saved_server) =
            self.connect_plugin_via_mcp(mcp, slug, auth_values, override_url)?;
        let probe = mcp.probe_server(&saved_server.id).await?;
        let now = Utc::now().to_rfc3339_opts(SecondsFormat::Millis, true);
        if probe.status == "ready" {
            record.status = "connected".to_string();
            record.discovered_tools = probe.tool_defs.clone();
            record.last_error = None;
            record.verified_at = Some(now);
            let saved_record = self.save_connected_app(record)?;
            Ok((saved_record, saved_server, probe))
        } else {
            // Keep the registered MCP server config only when OAuth sign-in is pending (`needs_sign_in`),
            // otherwise roll back the unverified connection so we never fabricate a connected app.
            if probe.status == "needs_sign_in" {
                record.status = "needs_sign_in".to_string();
                record.discovered_tools = Vec::new();
                record.last_error = probe.error.clone();
                record.verified_at = None;
                let saved_record = self.save_connected_app(record)?;
                Ok((saved_record, saved_server, probe))
            } else {
                let _ = self.disconnect_app(&record.app_id);
                let _ = mcp.remove_server(&saved_server.id);
                let err_msg = probe
                    .error
                    .clone()
                    .unwrap_or_else(|| format!("MCP handshake failed ({})", probe.status));
                if probe.status == "needs_key" {
                    Err(OpenBotRuntimeError::Unauthorized(err_msg))
                } else {
                    Err(OpenBotRuntimeError::Execution(err_msg))
                }
            }
        }
    }

    /// Connects one of the 9 native OpenBot connectors (`slack`, `discord`, `telegram`, `whatsapp`,
    /// `onepassword`, `bitwarden`, `protonpass`, `github`, `x`), persisting state and registering
    /// its authentic official API / CLI server entry in [`McpGateway`].
    pub fn connect_native_connector(
        &self,
        mcp: &McpGateway,
        connector_id: &str,
        account_label: &str,
        credential: Option<&str>,
        mut metadata: BTreeMap<String, String>,
    ) -> RuntimeResult<ConnectedAppRecord> {
        let catalog = self.catalog();
        let conn = catalog
            .connectors
            .iter()
            .find(|c| {
                c.connector_id.eq_ignore_ascii_case(connector_id)
                    || c.slug.eq_ignore_ascii_case(connector_id)
                    || c.id.eq_ignore_ascii_case(connector_id)
            })
            .ok_or_else(|| {
                OpenBotRuntimeError::NotFound(format!(
                    "Native connector '{connector_id}' was not found."
                ))
            })?;

        let clean_label = if account_label.trim().is_empty() {
            conn.name.clone()
        } else {
            account_label.trim().to_string()
        };
        let cred_trimmed = credential.map(str::trim).filter(|s| !s.is_empty());
        if cred_trimmed.is_some() {
            metadata.insert("credential_saved".to_string(), "true".to_string());
        }
        metadata.insert("connector_id".to_string(), conn.connector_id.clone());

        let existing_server_id = self
            .get_connected_app(&conn.id)
            .and_then(|r| r.mcp_server_id)
            .or_else(|| {
                if conn.connector_id == "github" {
                    Some(GITHUB_CONNECTOR_MCP_SERVER_ID.to_string())
                } else {
                    mcp.get_server(&format!("connector-{}", conn.connector_id))
                        .ok()
                        .flatten()
                        .or_else(|| mcp.get_server(&conn.connector_id).ok().flatten())
                        .map(|s| s.id)
                }
            });

        let server_name = if conn.connector_id == "github" {
            GITHUB_CONNECTOR_MCP_SERVER_NAME.to_string()
        } else {
            format!("connector-{}", conn.connector_id)
        };
        let real_endpoint = native_connector_real_endpoint(&conn.connector_id).to_string();

        let (transport, command, url, mut env_pairs, mut headers) =
            match conn.connector_id.as_str() {
                "onepassword" => {
                    let mut env = Vec::new();
                    if let Some(tok) = cred_trimmed {
                        env.push(McpKeyValue {
                            key: "OP_SERVICE_ACCOUNT_TOKEN".to_string(),
                            value: tok.to_string(),
                        });
                    }
                    (McpTransport::Stdio, Some("op".to_string()), None, env, Vec::new())
                }
                "bitwarden" => {
                    let mut env = Vec::new();
                    if let Some(tok) = cred_trimmed {
                        env.push(McpKeyValue {
                            key: "BW_SESSION".to_string(),
                            value: tok.to_string(),
                        });
                    }
                    (McpTransport::Stdio, Some("bw".to_string()), None, env, Vec::new())
                }
                "protonpass" => (
                    McpTransport::Stdio,
                    Some("pass-cli".to_string()),
                    None,
                    Vec::new(),
                    Vec::new(),
                ),
                "github" => {
                    let mut hdrs = Vec::new();
                    if let Some(tok) = cred_trimmed {
                        hdrs.push(McpKeyValue {
                            key: "Authorization".to_string(),
                            value: if tok.to_ascii_lowercase().starts_with("bearer ") {
                                tok.to_string()
                            } else {
                                format!("Bearer {tok}")
                            },
                        });
                    }
                    (
                        McpTransport::Http,
                        None,
                        Some(GITHUB_CONNECTOR_MCP_SERVER_URL.to_string()),
                        Vec::new(),
                        hdrs,
                    )
                }
                "discord" => {
                    let mut hdrs = Vec::new();
                    if let Some(tok) = cred_trimmed {
                        hdrs.push(McpKeyValue {
                            key: "Authorization".to_string(),
                            value: if tok.starts_with("Bot ") || tok.starts_with("Bearer ") {
                                tok.to_string()
                            } else {
                                format!("Bot {tok}")
                            },
                        });
                    }
                    (
                        McpTransport::Http,
                        None,
                        Some(real_endpoint.clone()),
                        Vec::new(),
                        hdrs,
                    )
                }
                _ => {
                    let mut hdrs = Vec::new();
                    if let Some(tok) = cred_trimmed {
                        hdrs.push(McpKeyValue {
                            key: "Authorization".to_string(),
                            value: if tok.to_ascii_lowercase().starts_with("bearer ") {
                                tok.to_string()
                            } else {
                                format!("Bearer {tok}")
                            },
                        });
                    }
                    (
                        McpTransport::Http,
                        None,
                        Some(real_endpoint.clone()),
                        Vec::new(),
                        hdrs,
                    )
                }
            };

        if let Some(custom_base) = metadata
            .get("api_base_url")
            .map(|s| s.trim())
            .filter(|s| s.starts_with("http://") || s.starts_with("https://"))
        {
            if transport == McpTransport::Http {
                headers.push(McpKeyValue {
                    key: "X-Fabushi-Api-Base".to_string(),
                    value: custom_base.to_string(),
                });
            }
        }
        let _ = &mut env_pairs;

        let saved_server = mcp.register_server(RegisterMcpServerInput {
            id: existing_server_id,
            name: server_name,
            transport,
            enabled: Some(true),
            command,
            args: Vec::new(),
            env: env_pairs,
            env_passthrough: Vec::new(),
            working_directory: None,
            url,
            headers,
        })?;

        let record = ConnectedAppRecord {
            app_id: conn.id.clone(),
            kind: "connector".to_string(),
            name: conn.name.clone(),
            status: "connected".to_string(),
            connected_at: Utc::now().to_rfc3339_opts(SecondsFormat::Millis, true),
            mcp_server_id: Some(saved_server.id),
            account_label: Some(clean_label),
            endpoint: Some(real_endpoint),
            discovered_tools: native_connector_tools(&conn.connector_id),
            last_error: None,
            verified_at: None,
            metadata,
        };
        let saved = self.save_connected_app(record)?;
        let _ = self.install_item(MarketplaceTab::Apps, &conn.slug);
        Ok(saved)
    }

    /// Live-verifies credentials for a native OpenBot connector against its real upstream API / CLI
    /// (Slack `auth.test`, Discord `/users/@me`, Telegram `getMe`, GitHub `/user`, X `/2/users/me`,
    /// WhatsApp Cloud API, `op`/`bw`/`pass-cli`) before persisting the connection.
    pub async fn connect_native_connector_verified(
        &self,
        mcp: &McpGateway,
        http: &reqwest::Client,
        connector_id: &str,
        account_label: &str,
        credential: Option<&str>,
        metadata: BTreeMap<String, String>,
    ) -> RuntimeResult<ConnectedAppRecord> {
        let verified =
            verify_native_connector(http, connector_id, account_label, credential, &metadata)
                .await?;
        let mut rec = self.connect_native_connector(
            mcp,
            &verified.connector_id,
            &verified.verified_account,
            credential,
            verified.metadata,
        )?;
        rec.status = verified.status;
        rec.endpoint = Some(verified.endpoint);
        rec.discovered_tools = verified.tools;
        rec.last_error = None;
        rec.verified_at = Some(Utc::now().to_rfc3339_opts(SecondsFormat::Millis, true));
        self.save_connected_app(rec)
    }

    /// Updates a connected app's live verification state and discovered tools after re-probing or OAuth.
    pub fn update_connection_probe_state(
        &self,
        app_id: &str,
        status: &str,
        discovered_tools: Vec<McpDiscoveredTool>,
        last_error: Option<String>,
    ) -> RuntimeResult<Option<ConnectedAppRecord>> {
        let Some(mut rec) = self.get_connected_app(app_id) else {
            return Ok(None);
        };
        rec.status = status.to_string();
        rec.discovered_tools = discovered_tools;
        rec.last_error = last_error;
        if status == "connected" {
            rec.verified_at = Some(Utc::now().to_rfc3339_opts(SecondsFormat::Millis, true));
        }
        Ok(Some(self.save_connected_app(rec)?))
    }

    /// Disconnects a native connector or plugin app and removes its linked `McpGateway` server.
    pub fn disconnect_app_via_mcp(
        &self,
        mcp: &McpGateway,
        app_id: &str,
    ) -> RuntimeResult<bool> {
        let removed = self.disconnect_app(app_id)?;
        if let Some(rec) = removed {
            if let Some(server_id) = rec.mcp_server_id {
                let _ = mcp.remove_server(&server_id);
            }
            return Ok(true);
        }
        Ok(false)
    }

    pub fn search(&self, req: MarketplaceSearchRequest) -> RuntimeResult<MarketplaceSearchOutcome> {
        let tab = MarketplaceTab::parse(&req.tab)?;
        let normalized_source = match req
            .source
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
        {
            None => None,
            Some(s) if s.eq_ignore_ascii_case("verified") => Some("verified".to_string()),
            Some(s) if s.eq_ignore_ascii_case("public") => Some("public".to_string()),
            Some(other) => {
                return Err(OpenBotRuntimeError::Validation(format!(
                    "Invalid marketplace source '{other}'; expected 'verified' or 'public'."
                )));
            }
        };

        let normalized_category = req
            .category
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::to_ascii_lowercase);
        if let Some(cat) = normalized_category.as_deref()
            && !SKILL_CATEGORIES.contains(&cat)
        {
            return Err(OpenBotRuntimeError::Validation(format!(
                "Unknown marketplace category '{cat}'; expected one of: {}.",
                SKILL_CATEGORIES.join(", ")
            )));
        }

        let cleaned_query = req
            .query
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::to_string);
        let query_lower = cleaned_query.as_deref().map(str::to_lowercase);

        let mut catalog = self.catalog_for_tab(tab);
        {
            let guard = self.installs.lock();
            for item in &mut catalog {
                let key = install_key(item.tab, &item.id);
                if let Some(receipt) = guard.installs.get(&key) {
                    item.installs = item.installs.saturating_add(receipt.count);
                }
            }
        }

        let items: Vec<MarketplaceItem> = catalog
            .into_iter()
            .filter(|item| match normalized_source.as_deref() {
                Some("verified") => item.verified,
                Some("public") => !item.verified,
                _ => true,
            })
            .filter(|item| match normalized_category.as_deref() {
                Some(cat) => item.category.eq_ignore_ascii_case(cat),
                None => true,
            })
            .filter(|item| match query_lower.as_deref() {
                Some(q) => item_matches_query(item, q),
                None => true,
            })
            .collect();

        let empty_state = items.is_empty();
        let plush_asset = if req.reduced_motion || req.alpha_unsupported {
            FISHING_STILL_ASSET.to_string()
        } else {
            FISHING_VIDEO_ASSET.to_string()
        };

        Ok(MarketplaceSearchOutcome {
            tab: tab.as_str().to_string(),
            tabs: MARKETPLACE_TABS.iter().map(|t| (*t).to_string()).collect(),
            source: normalized_source,
            query: cleaned_query,
            items,
            empty_state,
            alpha_fallback: EMPTY_STATE_ALPHA_FALLBACK.to_string(),
            plush_asset,
        })
    }

    pub fn install_item(
        &self,
        tab: MarketplaceTab,
        id_or_slug: &str,
    ) -> RuntimeResult<MarketplaceItem> {
        let needle = id_or_slug.trim();
        if needle.is_empty() {
            return Err(OpenBotRuntimeError::Validation(
                "Marketplace item id or slug must not be empty.".to_string(),
            ));
        }

        let mut item = self
            .catalog_for_tab(tab)
            .into_iter()
            .find(|cand| {
                cand.id.eq_ignore_ascii_case(needle) || cand.slug.eq_ignore_ascii_case(needle)
            })
            .ok_or_else(|| {
                OpenBotRuntimeError::NotFound(format!(
                    "Marketplace {} item '{}' was not found.",
                    tab.as_str(),
                    needle
                ))
            })?;

        let now = Utc::now().to_rfc3339_opts(SecondsFormat::Millis, true);
        let key = install_key(tab, &item.id);
        let mut guard = self.installs.lock();
        guard.version = 1;
        let entry = guard.installs.entry(key).or_insert_with(|| InstallReceipt {
            id: item.id.clone(),
            slug: item.slug.clone(),
            tab,
            name: item.name.clone(),
            installed_at: now.clone(),
            count: 0,
        });
        entry.installed_at = now;
        entry.count = entry.count.saturating_add(1);
        item.installs = item.installs.saturating_add(entry.count);
        write_json_file_atomically(&self.installs_path, &*guard, true)?;
        Ok(item)
    }

    fn catalog_for_tab(&self, tab: MarketplaceTab) -> Vec<MarketplaceItem> {
        let cat = openbot_marketplace_catalog();
        match tab {
            MarketplaceTab::Apps => {
                let mut out = Vec::with_capacity(cat.connectors.len() + cat.plugins.len());
                for c in &cat.connectors {
                    out.push(MarketplaceItem {
                        id: c.id.clone(),
                        slug: c.slug.clone(),
                        tab: MarketplaceTab::Apps,
                        name: c.name.clone(),
                        tagline: c.tagline.clone(),
                        description: c.description.clone(),
                        category: c.category.clone(),
                        creator_name: c.creator_name.clone(),
                        verified: true,
                        featured: true,
                        installs: c.installs,
                        kind: "connector".to_string(),
                    });
                }
                for p in &cat.plugins {
                    out.push(MarketplaceItem {
                        id: p.id.clone(),
                        slug: p.slug.clone(),
                        tab: MarketplaceTab::Apps,
                        name: p.name.clone(),
                        tagline: p.tagline.clone(),
                        description: p.description.clone(),
                        category: p.category.clone(),
                        creator_name: p.creator_name.clone(),
                        verified: true,
                        featured: false,
                        installs: p.installs,
                        kind: "plugin".to_string(),
                    });
                }
                out
            }
            MarketplaceTab::Agents => cat
                .agents
                .iter()
                .map(|a| MarketplaceItem {
                    id: a.id.clone(),
                    slug: a.slug.clone(),
                    tab: MarketplaceTab::Agents,
                    name: a.name.clone(),
                    tagline: a.title.clone(),
                    description: a.description.clone(),
                    category: a.category.clone(),
                    creator_name: a.creator_name.clone(),
                    verified: a.verified,
                    featured: a.featured,
                    installs: a.installs,
                    kind: "agent".to_string(),
                })
                .collect(),
            MarketplaceTab::Skills => {
                let mut items: Vec<MarketplaceItem> = cat
                    .skills
                    .iter()
                    .map(|s| MarketplaceItem {
                        id: s.id.clone(),
                        slug: s.slug.clone(),
                        tab: MarketplaceTab::Skills,
                        name: s.name.clone(),
                        tagline: s.description.clone(),
                        description: s.description.clone(),
                        category: s.category.clone(),
                        creator_name: s.creator_name.clone(),
                        verified: s.verified,
                        featured: s.featured,
                        installs: s.installs,
                        kind: "skill".to_string(),
                    })
                    .collect();
                let mut seen: HashSet<String> = items
                    .iter()
                    .flat_map(|i| [i.id.to_lowercase(), i.slug.to_lowercase()])
                    .collect();
                for local in load_local_installed_skills(&self.data_dir) {
                    if seen.insert(local.slug.to_lowercase()) {
                        seen.insert(local.id.to_lowercase());
                        items.push(local);
                    }
                }
                items
            }
        }
    }
}

fn install_key(tab: MarketplaceTab, id: &str) -> String {
    format!("{}:{id}", tab.as_str())
}

fn item_matches_query(item: &MarketplaceItem, q_lower: &str) -> bool {
    item.name.to_lowercase().contains(q_lower)
        || item.slug.to_lowercase().contains(q_lower)
        || item.id.to_lowercase().contains(q_lower)
        || item.tagline.to_lowercase().contains(q_lower)
        || item.description.to_lowercase().contains(q_lower)
        || item.creator_name.to_lowercase().contains(q_lower)
        || item.category.to_lowercase().contains(q_lower)
}

fn load_installs_file(path: &Path) -> RuntimeResult<MarketplaceInstallsFile> {
    match fs::read_to_string(path) {
        Ok(raw) => {
            if raw.trim().is_empty() {
                return Ok(MarketplaceInstallsFile {
                    version: 1,
                    installs: BTreeMap::new(),
                });
            }
            let parsed: MarketplaceInstallsFile = serde_json::from_str(&raw)?;
            Ok(parsed)
        }
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(MarketplaceInstallsFile {
            version: 1,
            installs: BTreeMap::new(),
        }),
        Err(err) => Err(OpenBotRuntimeError::Io(err)),
    }
}

fn load_connected_apps_file(path: &Path) -> RuntimeResult<ConnectedAppsFile> {
    match fs::read_to_string(path) {
        Ok(raw) => {
            if raw.trim().is_empty() {
                return Ok(ConnectedAppsFile {
                    version: 1,
                    apps: BTreeMap::new(),
                });
            }
            let parsed: ConnectedAppsFile = serde_json::from_str(&raw)?;
            Ok(parsed)
        }
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(ConnectedAppsFile {
            version: 1,
            apps: BTreeMap::new(),
        }),
        Err(err) => Err(OpenBotRuntimeError::Io(err)),
    }
}

fn load_local_installed_skills(data_dir: &Path) -> Vec<MarketplaceItem> {
    let path = data_dir.join(INSTALLED_SKILLS_FILE_NAME);
    let Ok(raw) = fs::read_to_string(&path) else {
        return Vec::new();
    };
    let Ok(parsed) = serde_json::from_str::<LocalInstalledSkillsFile>(&raw) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    let mut seen_slugs = HashSet::new();
    for entries in parsed.by_agent.values() {
        for entry in entries {
            let slug = entry.slug.trim();
            if slug.is_empty() || !seen_slugs.insert(slug.to_lowercase()) {
                continue;
            }
            out.push(MarketplaceItem {
                id: entry.skill_id.clone(),
                slug: slug.to_string(),
                tab: MarketplaceTab::Skills,
                name: entry.name.clone(),
                tagline: "Locally installed SKILL.md skill".to_string(),
                description: entry.description.clone(),
                category: "other".to_string(),
                creator_name: "Local Workspace".to_string(),
                verified: false,
                featured: false,
                installs: 1,
                kind: "local-skill".to_string(),
            });
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use openbot_storage::OpenBotDatabase;

    #[test]
    fn embedded_openbot_catalog_matches_upstream_counts_and_categories() {
        let cat = openbot_marketplace_catalog();
        assert_eq!(cat.connectors.len(), 9);
        assert_eq!(cat.plugins.len(), 257);
        assert_eq!(cat.agents.len(), 29);
        assert_eq!(cat.skills.len(), 40);

        let prod_count = cat
            .connectors
            .iter()
            .filter(|c| c.category == "productivity")
            .count()
            + cat
                .plugins
                .iter()
                .filter(|p| p.category == "productivity")
                .count();
        assert_eq!(prod_count, 84);

        let docs_count = cat
            .plugins
            .iter()
            .filter(|p| p.category == "documents")
            .count();
        assert_eq!(docs_count, 8);

        let design_count = cat
            .plugins
            .iter()
            .filter(|p| p.category == "design")
            .count();
        assert_eq!(design_count, 25);
    }

    #[test]
    fn connect_and_disconnect_plugin_and_native_connector_persists_to_mcp_gateway() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let db = OpenBotDatabase::open(tmp.path().join("openbot.db")).expect("db");
        let mcp = McpGateway::open(db, tmp.path()).expect("mcp");
        let market = MarketplaceService::open(tmp.path()).expect("market");

        // 1. Connect open plugin (aave)
        let (rec, srv) = market
            .connect_plugin_via_mcp(&mcp, "aave", &BTreeMap::new(), None)
            .expect("connect aave");
        assert_eq!(rec.app_id, "plugin:aave");
        assert_eq!(srv.name, "aave");
        assert!(market.is_app_connected("plugin:aave"));
        assert_eq!(mcp.list_servers().expect("list").len(), 1);

        // 2. Connect key plugin (obsidian) — missing key rejected, valid key succeeds
        let err = market.connect_plugin_via_mcp(&mcp, "obsidian", &BTreeMap::new(), None);
        assert!(err.is_err());
        let mut vals = BTreeMap::new();
        vals.insert("token".to_string(), "obs-secret-key-123".to_string());
        let (obs_rec, obs_srv) = market
            .connect_plugin_via_mcp(&mcp, "obsidian", &vals, None)
            .expect("connect obsidian");
        assert_eq!(obs_rec.app_id, "plugin:obsidian");
        assert!(obs_srv.env.iter().any(|e| e.key == "OBSIDIAN_API_KEY"));

        // 3. Connect native connector (slack)
        let slack_rec = market
            .connect_native_connector(
                &mcp,
                "slack",
                "Acme Workspace",
                Some("xoxb-test-token"),
                BTreeMap::new(),
            )
            .expect("connect slack");
        assert_eq!(slack_rec.app_id, "connector:slack");
        assert_eq!(slack_rec.account_label.as_deref(), Some("Acme Workspace"));
        assert_eq!(mcp.list_servers().expect("list").len(), 3);

        // 4. Reload from disk confirms persistence
        let reloaded = MarketplaceService::open(tmp.path()).expect("reopen market");
        assert_eq!(reloaded.list_connected_apps().len(), 3);
        assert!(reloaded.is_app_connected("connector:slack"));

        // 5. Disconnect cleans up both store and MCP gateway
        assert!(
            reloaded
                .disconnect_app_via_mcp(&mcp, "plugin:aave")
                .expect("disconnect aave")
        );
        assert!(!reloaded.is_app_connected("plugin:aave"));
        assert_eq!(mcp.list_servers().expect("list").len(), 2);
    }
}
