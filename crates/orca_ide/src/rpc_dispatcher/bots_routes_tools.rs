//! Unified RPC handlers for `bots.tools.*` (Loopback Tool Relay, OAuth 2.1, Host MCP toggle)
//! and `mcpServers.*` (Shared Stdio MCP Pool & Agent Config Sync).

use std::collections::{BTreeMap, HashSet};
use std::path::PathBuf;
use std::sync::Arc;

use orca_bots::wire_json::bot_record_to_wire_json;
use orca_bots::{
    DynamicClientRegistrationRequest, HostMcpServerRecord, OAuthCallbackParams,
    OAuthProviderConfig, OAuthSecurityPolicy, OrcaBotsFacade, RegisteredBotTool, RelayToolAuth,
    RelayToolRegistration, scrub_bot_tool_secrets,
};
use orca_core::BotErrorCode;
use orca_core::bot_types::{BotToolName, bot_tool_name_from, free_bot_tool_name};
use serde_json::{Value, json};

use crate::plugin_host::{McpConfigSync, McpServerEntry};
use crate::rpc_dispatcher::RpcSubsystemHandles;
use crate::{OrcaIdeRuntimeError, Result};

fn opt_str<'a>(params: &'a Value, keys: &[&str]) -> Option<&'a str> {
    for key in keys {
        if let Some(s) = params
            .get(*key)
            .and_then(Value::as_str)
            .filter(|s| !s.trim().is_empty())
        {
            return Some(s);
        }
    }
    None
}

fn req_str<'a>(params: &'a Value, keys: &[&str]) -> Result<&'a str> {
    opt_str(params, keys).ok_or_else(|| {
        OrcaIdeRuntimeError::invalid_argument(format!(
            "missing required string '{}'",
            keys.first().copied().unwrap_or("param")
        ))
    })
}

pub(super) async fn dispatch_bots_tools(
    bots: &Arc<OrcaBotsFacade>,
    sub: &str,
    params: &Value,
) -> Result<Value> {
    match sub {
        "setHostMcpServers" | "set_host_mcp_servers" => {
            let bot_id = req_str(params, &["botId", "bot_id", "id"])?;
            let enabled = params
                .get("enabled")
                .and_then(Value::as_bool)
                .ok_or_else(|| OrcaIdeRuntimeError::invalid_argument("missing bool 'enabled'"))?;
            let updated = bots.set_bot_host_mcp_servers(bot_id, enabled).await?;
            Ok(bot_record_to_wire_json(&updated, bots.home_store()))
        }
        "add" => handle_tools_add(bots, params).await,
        "relayInfo" | "relay_info" => {
            let relay = bots.start_tool_relay().await?;
            Ok(json!({
                "running": true,
                "port": relay.addr().port(),
                "addr": relay.addr().to_string(),
                "baseUrl": relay.base_url(),
                "basePath": "/tools",
            }))
        }
        "grant" => {
            let bot_id = req_str(params, &["botId", "bot_id", "id"])?;
            let bot = bots.controller().get_bot(bot_id).await?;
            let (relay, token) = bots.issue_bot_relay_grant(&bot).await?;
            Ok(json!({
                "botId": bot.id,
                "port": relay.addr().port(),
                "token": token,
                "bearer": token,
                "path": "/tools",
                "basePath": "/tools",
                "baseUrl": relay.base_url(),
            }))
        }
        "oauthStart" | "oauth_start" => handle_oauth_start(bots, params).await,
        "oauthCallback" | "oauth_callback" => handle_oauth_callback(bots, params).await,
        "oauthRevoke" | "oauth_revoke" => handle_oauth_revoke(bots, params).await,
        other => Err(OrcaIdeRuntimeError::not_found(format!(
            "unknown bots.tools RPC method 'bots.tools.{other}'"
        ))),
    }
}

async fn handle_tools_add(bots: &Arc<OrcaBotsFacade>, params: &Value) -> Result<Value> {
    let bot_id = req_str(params, &["botId", "bot_id", "id"])?;
    let bot = bots.controller().get_bot(bot_id).await?;
    let transport = opt_str(params, &["transport"]).unwrap_or("http").to_string();
    let url = req_str(params, &["url"])?.to_string();

    let existing_names: HashSet<String> = bots
        .list_bot_tools(&bot.id)?
        .into_iter()
        .map(|t| t.name)
        .collect();
    let tool_name = match opt_str(params, &["name"]) {
        Some(raw) => BotToolName::new(raw)
            .map_err(|code| OrcaIdeRuntimeError::bot(code, format!("invalid tool name '{raw}'")))?,
        None => {
            let derived = bot_tool_name_from(&url);
            free_bot_tool_name(&derived, &existing_names)
        }
    };
    let name_str = tool_name.as_str().to_string();
    let credential_id = format!("cred-{}-{}", bot.id, name_str);

    let oauth_obj = params.get("oauth").and_then(Value::as_object);
    let oauth_issuer = oauth_obj
        .and_then(|o| o.get("issuer"))
        .and_then(Value::as_str)
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty());
    let oauth_allow_loopback = oauth_obj
        .and_then(|o| o.get("allowLoopbackTest").or_else(|| o.get("allow_loopback_test")))
        .and_then(Value::as_bool)
        .unwrap_or(false);

    let mut headers = BTreeMap::new();
    if let Some(hobj) = params.get("headers").and_then(Value::as_object) {
        for (k, v) in hobj {
            if let Some(s) = v.as_str() {
                headers.insert(k.clone(), s.to_string());
            }
        }
    }

    let auth = if oauth_issuer.is_some() {
        RelayToolAuth::OAuth {
            credential_id: credential_id.clone(),
        }
    } else if !headers.is_empty() {
        RelayToolAuth::StaticHeaders(headers)
    } else {
        RelayToolAuth::None
    };

    let relay = bots.start_tool_relay().await?;
    let mut allowed_folders = HashSet::new();
    allowed_folders.insert(bot.folder.as_str().to_string());
    relay.register_tool(RelayToolRegistration {
        credential_id: credential_id.clone(),
        tool_handle: name_str.clone(),
        upstream_url: url.clone(),
        allowed_folders,
        auth,
    })?;

    bots.record_bot_tool(RegisteredBotTool {
        bot_id: bot.id.clone(),
        name: name_str.clone(),
        credential_id: credential_id.clone(),
        transport: transport.clone(),
        upstream_url: url,
        oauth_issuer,
        oauth_allow_loopback,
        oauth_provider: None,
    })?;

    // Sync opaque secret-free mirror in `<bot_home>/.mcp.json` and scrub any legacy secrets.
    let home_dir = bots.home_store().bot_dir(&bot.folder);
    let mcp_path = home_dir.join(".mcp.json");
    if mcp_path.is_file() {
        let _ = scrub_bot_tool_secrets(&[mcp_path.clone()], &[]);
    }
    let mut root = if mcp_path.is_file() {
        std::fs::read_to_string(&mcp_path)
            .ok()
            .and_then(|raw| serde_json::from_str::<Value>(&raw).ok())
            .filter(Value::is_object)
            .unwrap_or_else(|| json!({}))
    } else {
        json!({})
    };
    let stub = relay.build_bot_home_tool_stub(&name_str, &credential_id)?;
    if let Some(obj) = root.as_object_mut() {
        let servers = obj
            .entry("mcpServers".to_string())
            .or_insert_with(|| json!({}));
        if !servers.is_object() {
            *servers = json!({});
        }
        if let Some(smap) = servers.as_object_mut() {
            smap.insert(name_str.clone(), stub);
        }
    }
    let pretty = serde_json::to_vec_pretty(&root)?;
    bots.home_store().replace_bytes(&mcp_path, &pretty)?;
    let _ = scrub_bot_tool_secrets(&[mcp_path], &[]);

    Ok(json!({
        "name": name_str,
        "credentialId": credential_id,
        "transport": transport,
        "relayPath": format!("/tools/{name_str}"),
        "relayUrl": format!("{}/tools/{name_str}", relay.base_url()),
    }))
}

async fn handle_oauth_start(bots: &Arc<OrcaBotsFacade>, params: &Value) -> Result<Value> {
    let bot_id = req_str(params, &["botId", "bot_id"])?;
    let tool_name = req_str(params, &["toolName", "tool_name", "name"])?;
    let mut tool = bots
        .get_bot_tool(bot_id, tool_name)?
        .ok_or_else(|| OrcaIdeRuntimeError::not_found(format!("tool '{tool_name}' not found for bot '{bot_id}'")))?;
    let issuer = tool
        .oauth_issuer
        .clone()
        .or_else(|| opt_str(params, &["issuer"]).map(ToOwned::to_owned))
        .ok_or_else(|| OrcaIdeRuntimeError::invalid_argument("tool has no OAuth issuer configured"))?;

    let req_policy = if tool.oauth_allow_loopback {
        OAuthSecurityPolicy::allow_loopback_for_tests()
    } else {
        OAuthSecurityPolicy::strict()
    };
    let oauth = bots.tool_oauth();
    let meta = oauth.discover_metadata_with_policy(&issuer, req_policy).await?;
    let relay = bots.start_tool_relay().await?;
    let redirect_uri = opt_str(params, &["redirectUri", "redirect_uri"])
        .map(ToOwned::to_owned)
        .unwrap_or_else(|| format!("{}/oauth/callback", relay.base_url()));

    let (client_id, client_secret) = if let Some(ref reg_ep) = meta.registration_endpoint {
        let dcr_req = DynamicClientRegistrationRequest {
            client_name: format!("Orca Bot {}", bot_id),
            redirect_uris: vec![redirect_uri.clone()],
            grant_types: vec![
                "authorization_code".to_string(),
                "refresh_token".to_string(),
            ],
            response_types: vec!["code".to_string()],
            token_endpoint_auth_method: "none".to_string(),
        };
        let dcr = oauth.register_dynamic_client(reg_ep, &dcr_req).await?;
        (dcr.client_id, dcr.client_secret)
    } else {
        let cid = opt_str(params, &["clientId", "client_id"])
            .ok_or_else(|| OrcaIdeRuntimeError::invalid_argument("missing clientId and no RFC 7591 registration_endpoint"))?
            .to_string();
        (cid, opt_str(params, &["clientSecret", "client_secret"]).map(ToOwned::to_owned))
    };

    let provider_cfg = OAuthProviderConfig {
        issuer: meta.issuer,
        authorization_endpoint: meta.authorization_endpoint,
        token_endpoint: meta.token_endpoint,
        revocation_endpoint: meta.revocation_endpoint,
        registration_endpoint: meta.registration_endpoint,
        client_id,
        client_secret,
        redirect_uri,
        resource: tool.upstream_url.clone(),
        scopes: vec!["mcp".to_string()],
    };
    let start = oauth.begin_authorization(&tool.credential_id, provider_cfg.clone())?;
    tool.oauth_provider = Some(provider_cfg);
    bots.record_bot_tool(tool)?;

    Ok(json!({
        "authorizationUrl": start.authorization_url,
        "authUrl": start.authorization_url,
        "state": start.state,
        "codeChallenge": start.code_challenge,
        "expiresAtMs": start.expires_at_ms,
    }))
}

async fn handle_oauth_callback(bots: &Arc<OrcaBotsFacade>, params: &Value) -> Result<Value> {
    let code = req_str(params, &["code"])?.to_string();
    let state = req_str(params, &["state"])?.to_string();
    let iss = opt_str(params, &["iss", "issuer"]).map(ToOwned::to_owned);
    let rec = bots
        .tool_oauth()
        .complete_authorization(OAuthCallbackParams { code, state, iss })
        .await?;
    Ok(json!({
        "ok": true,
        "credentialId": rec.credential_id,
        "tokenType": rec.token_type,
        "expiresAtMs": rec.expires_at_ms,
        "resource": rec.resource,
    }))
}

async fn handle_oauth_revoke(bots: &Arc<OrcaBotsFacade>, params: &Value) -> Result<Value> {
    let bot_id = req_str(params, &["botId", "bot_id"])?;
    let tool_name = req_str(params, &["toolName", "tool_name", "name"])?;
    let tool = bots
        .get_bot_tool(bot_id, tool_name)?
        .ok_or_else(|| OrcaIdeRuntimeError::not_found(format!("tool '{tool_name}' not found for bot '{bot_id}'")))?;
    let revoked = bots.tool_oauth().revoke_and_delete(&tool.credential_id).await?;
    if let Some(relay) = bots.tool_relay().await {
        let _ = relay.end_credential(&tool.credential_id);
    }
    Ok(json!({
        "revoked": revoked,
        "credentialId": tool.credential_id,
    }))
}

pub(super) async fn dispatch_mcp_servers_namespace(
    handles: &Arc<RpcSubsystemHandles>,
    op: &str,
    params: &Value,
) -> Result<Value> {
    let bots = handles.bots.as_ref().ok_or_else(|| {
        OrcaIdeRuntimeError::bot(
            BotErrorCode::BotsDisabled,
            "orca_bots facade is not attached to this OrcaIdeFacade instance",
        )
    })?;
    match op {
        "add" | "update" => {
            let name = req_str(params, &["name"])?.to_string();
            let command = opt_str(params, &["command"]).map(ToOwned::to_owned);
            let url = opt_str(params, &["url"]).map(ToOwned::to_owned);
            let transport = opt_str(params, &["transport"])
                .map(ToOwned::to_owned)
                .unwrap_or_else(|| {
                    if command.is_some() {
                        "stdio".to_string()
                    } else {
                        "streamable-http".to_string()
                    }
                });
            let transport_lc = transport.trim().to_ascii_lowercase();
            if !matches!(
                transport_lc.as_str(),
                "stdio" | "http" | "sse" | "streamable-http" | "streamable_http"
            ) {
                return Err(OrcaIdeRuntimeError::invalid_argument(format!(
                    "unsupported MCP transport '{transport}' (expected stdio|http|sse|streamable-http)"
                )));
            }
            if transport_lc == "stdio"
                && command
                    .as_deref()
                    .map(str::trim)
                    .is_none_or(str::is_empty)
            {
                return Err(OrcaIdeRuntimeError::invalid_argument(
                    "stdio MCP server requires a non-empty 'command'",
                ));
            }
            let raw_blob = serde_json::to_string(params).unwrap_or_default();
            for pat in [
                "sk-ant-",
                "sk-proj-",
                "sk-live-",
                "ghp_",
                "gho_",
                "ghu_",
                "github_pat_",
                "AKIA",
                "orcawh_",
                "whsec_",
                "whk_",
            ] {
                if raw_blob.contains(pat) {
                    return Err(OrcaIdeRuntimeError::invalid_argument(
                        "plaintext API secret blocked in MCP server config; pass a credential_handle instead",
                    ));
                }
            }
            let args: Vec<String> = params
                .get("args")
                .and_then(Value::as_array)
                .map(|a| a.iter().filter_map(Value::as_str).map(ToOwned::to_owned).collect())
                .unwrap_or_default();
            let mut env = BTreeMap::new();
            if let Some(eobj) = params.get("env").and_then(Value::as_object) {
                for (k, v) in eobj {
                    if let Some(s) = v.as_str() {
                        env.insert(k.clone(), s.to_string());
                    }
                }
            }
            let cwd = opt_str(params, &["cwd"]).map(PathBuf::from);
            let credential_handle =
                opt_str(params, &["credential_handle", "credentialHandle"]).map(ToOwned::to_owned);
            let source = opt_str(params, &["source"]).unwrap_or("host").to_string();
            let rec = bots
                .upsert_host_mcp_server(HostMcpServerRecord {
                    name,
                    transport,
                    command,
                    args,
                    env,
                    cwd,
                    url,
                    credential_handle,
                    source,
                })
                .await?;
            Ok(json!({ "server": rec, "name": rec.name }))
        }
        "list" => {
            let servers = bots.list_host_mcp_servers()?;
            Ok(json!({ "servers": servers }))
        }
        "remove" | "delete" => {
            let name = req_str(params, &["name"])?;
            let removed = bots.remove_host_mcp_server(name).await?;
            Ok(json!({ "removed": removed, "name": name }))
        }
        "sync" => {
            let target_dir = PathBuf::from(req_str(params, &["target_dir", "targetDir"])?);
            std::fs::create_dir_all(&target_dir)?;
            let agents: Vec<String> = params
                .get("agents")
                .and_then(Value::as_array)
                .map(|a| a.iter().filter_map(Value::as_str).map(ToOwned::to_owned).collect())
                .filter(|v: &Vec<String>| !v.is_empty())
                .unwrap_or_else(|| vec!["claude".to_string(), "codex".to_string()]);
            let all_servers = bots.list_host_mcp_servers()?;
            let entries: Vec<McpServerEntry> = all_servers
                .iter()
                .map(|s| McpServerEntry {
                    name: s.name.clone(),
                    transport: s.transport.clone(),
                    command: s.command.clone().unwrap_or_default(),
                    args: s.args.clone(),
                    env: s.env.clone(),
                    url: s.url.clone(),
                    credential_handle: s.credential_handle.clone(),
                    source: s.source.clone(),
                })
                .collect();
            let mut synced_paths = Vec::new();
            for agent in &agents {
                let target_path = target_dir.join(format!("{agent}.mcp.json"));
                McpConfigSync::sync_to_agent_config(&target_path, &entries, &[], true)?;
                synced_paths.push(target_path.to_string_lossy().to_string());
            }
            Ok(json!({ "synced": synced_paths, "serverCount": all_servers.len() }))
        }
        other => Err(OrcaIdeRuntimeError::not_found(format!(
            "unknown mcpServers RPC method 'mcpServers.{other}'"
        ))),
    }
}
