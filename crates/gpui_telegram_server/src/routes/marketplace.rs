//! Live REST API and real executable `McpRegistry` bridge for the server-published
//! OpenBot Marketplace (`/api/v1/marketplace/*`).
//!
//! Zero mock responses:
//! - Catalog listings & real install counts are served from SQLite (`marketplace_apps`,
//!   `marketplace_agents`, `marketplace_skills`), seeded idempotently with all 266 OpenBot
//!   connectors/plugins (starting at 0 installs) plus any user-published connectors.
//! - Connecting a native connector (`Slack`, `Discord`, `Telegram`, `WhatsApp`, `GitHub`, `X`,
//!   `1Password`, `Bitwarden`, `Proton Pass`) or an HTTP MCP plugin executes real upstream
//!   verification (`auth.test`, `/users/@me`, `getMe`, `/user`, MCP `initialize` + `tools/list`),
//!   encrypts credentials at rest, and registers live executable `McpToolHandler`s directly into
//!   `AppState::mcp_registry` (shared live with every Fabushi Bot's `HermesEngine`).

use std::collections::HashMap;
use std::sync::{Arc, RwLock};
use std::time::Duration;

use axum::{
    body::Bytes,
    extract::{Path, Query, State},
    http::StatusCode,
    response::{IntoResponse, Json, Response},
};
use chrono::Utc;
use gpui_telegram_agent::{McpRegistry, McpToolDefinition, McpToolHandler, ToolRiskLevel};
use gpui_telegram_proto::marketplace::{
    CallMarketplaceToolRequest, CallMarketplaceToolResponse, ConnectMarketplaceAppRequest,
    ConnectMarketplaceAppResponse, MarketplaceCatalogResponse, MarketplaceConnectionDto,
    MarketplaceEnvPairDto, MarketplaceInstallResponse, MarketplaceToolSchemaDto,
    PublishMarketplaceAppRequest,
};
use openbot_runtime::{
    McpDiscoveredTool, call_http_mcp_endpoint_jsonrpc, execute_native_connector_tool,
    probe_http_mcp_endpoint, verify_native_connector,
};
use openbot_storage::{McpKeyValue, redact_secrets};
use serde_json::{Value, json};

use crate::db::ServerDatabase;
use crate::db::marketplace_store::{HydratedMarketplaceConnection, StoredConnectorSecret};
use crate::routes::{err_resp, map_telegram_err};
use crate::server::{AppState, AuthBearer};

fn build_http_client() -> reqwest::Client {
    reqwest::Client::builder()
        .timeout(Duration::from_secs(20))
        .user_agent(concat!("Fabushi-OpenBot/", env!("CARGO_PKG_VERSION")))
        .build()
        .unwrap_or_else(|_| reqwest::Client::new())
}

fn to_proto_tools(tools: &[McpDiscoveredTool]) -> Vec<MarketplaceToolSchemaDto> {
    tools
        .iter()
        .map(|t| MarketplaceToolSchemaDto {
            name: t.name.clone(),
            description: t.description.clone(),
            input_schema: t.input_schema.clone(),
        })
        .collect()
}

fn mcp_server_id_for_user_app(user_id: u64, app_id: &str) -> String {
    let clean: String = app_id
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c.to_ascii_lowercase() } else { '_' })
        .collect();
    format!("mkt_u{user_id}_{clean}")
}

fn tool_risk_for_name(name: &str) -> ToolRiskLevel {
    let lower = name.to_ascii_lowercase();
    if lower.contains("delete") || lower.contains("remove") || lower.contains("destroy") {
        ToolRiskLevel::High
    } else if lower.contains("post")
        || lower.contains("send")
        || lower.contains("create")
        || lower.contains("update")
        || lower.contains("write")
    {
        ToolRiskLevel::Medium
    } else {
        ToolRiskLevel::ReadOnly
    }
}

/// Registers real executable `McpToolHandler`s into `mcp_registry` for a verified connection so
/// both `/api/v1/mcp/call` and every Fabushi Bot's `mcp_list_tools`/`mcp_call` invoke live APIs.
pub fn register_live_connection_in_mcp_registry(
    mcp_registry: &Arc<RwLock<McpRegistry>>,
    conn: &HydratedMarketplaceConnection,
) {
    let server_id = conn
        .dto
        .mcp_server_id
        .clone()
        .unwrap_or_else(|| mcp_server_id_for_user_app(conn.user_id, &conn.dto.app_id));

    let Ok(mut reg) = mcp_registry.write() else {
        return;
    };
    reg.remove_by_server_id(&server_id);
    if conn.dto.status != "connected" {
        return;
    }

    let is_native = conn.dto.kind == "connector";
    let connector_id = conn
        .dto
        .metadata
        .get("connector_id")
        .cloned()
        .unwrap_or_else(|| {
            conn.dto
                .app_id
                .strip_prefix("connector:")
                .unwrap_or(&conn.dto.app_id)
                .to_string()
        });
    let endpoint = conn.dto.endpoint.clone().unwrap_or_default();
    let server_name = conn.dto.name.clone();
    let credential = conn.secret.credential.clone();
    let headers: Vec<McpKeyValue> = conn
        .secret
        .headers
        .iter()
        .map(|h| McpKeyValue {
            key: h.key.clone(),
            value: h.value.clone(),
        })
        .collect();
    let metadata = conn.dto.metadata.clone();

    for tool in &conn.dto.discovered_tools {
        let schema_str =
            serde_json::to_string(&tool.input_schema).unwrap_or_else(|_| "{}".to_string());
        let def = McpToolDefinition::new(
            tool.name.clone(),
            tool.description.clone(),
            schema_str,
            tool_risk_for_name(&tool.name),
        )
        .with_server_id(server_id.clone());

        if is_native {
            let cid = connector_id.clone();
            let cred = credential.clone();
            let meta = metadata.clone();
            let tname = tool.name.clone();
            let handler: McpToolHandler = Arc::new(move |args: Value| {
                let cid = cid.clone();
                let cred = cred.clone();
                let meta = meta.clone();
                let tname = tname.clone();
                run_async_blocking(async move {
                    let client = build_http_client();
                    execute_native_connector_tool(&client, &cid, &cred, &meta, &tname, &args)
                        .await
                        .map_err(|e| redact_secrets(&e.to_string()))
                })
            });
            reg.register_with_handler(def, handler);
        } else if conn.dto.transport.eq_ignore_ascii_case("http")
            && (endpoint.starts_with("http://") || endpoint.starts_with("https://"))
        {
            let ep = endpoint.clone();
            let sname = server_name.clone();
            let hdrs = headers.clone();
            let tname = tool.name.clone();
            let handler: McpToolHandler = Arc::new(move |args: Value| {
                let ep = ep.clone();
                let sname = sname.clone();
                let hdrs = hdrs.clone();
                let tname = tname.clone();
                run_async_blocking(async move {
                    let client = build_http_client();
                    let args_obj = if args.is_object() { args } else { json!({}) };
                    call_http_mcp_endpoint_jsonrpc(
                        &client,
                        &sname,
                        &ep,
                        &hdrs,
                        "tools/call",
                        json!({ "name": tname, "arguments": args_obj }),
                    )
                    .await
                    .map_err(|(e, _)| redact_secrets(&e.to_string()))
                })
            });
            reg.register_with_handler(def, handler);
        } else {
            // Locally-executed STDIO/desktop MCP server: expose definition metadata cleanly.
            reg.register(def);
        }
    }
}

fn run_async_blocking<F, T>(fut: F) -> std::result::Result<T, String>
where
    F: std::future::Future<Output = std::result::Result<T, String>> + Send + 'static,
    T: Send + 'static,
{
    if let Ok(handle) = tokio::runtime::Handle::try_current() {
        if handle.runtime_flavor() == tokio::runtime::RuntimeFlavor::MultiThread {
            return tokio::task::block_in_place(|| handle.block_on(fut));
        }
        return std::thread::spawn(move || {
            let rt = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .map_err(|e| format!("Failed to construct runtime for MCP tool call: {e}"))?;
            rt.block_on(fut)
        })
        .join()
        .map_err(|_| "MCP tool worker thread panicked".to_string())?;
    }
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|e| format!("Failed to construct runtime for MCP tool call: {e}"))?;
    rt.block_on(fut)
}

/// Restores all persisted `status == "connected"` marketplace connections into `mcp_registry` on boot.
pub fn hydrate_marketplace_connections_into_mcp_registry(
    db: &ServerDatabase,
    mcp_registry: &Arc<RwLock<McpRegistry>>,
) -> usize {
    let Ok(rows) = db.list_all_active_marketplace_connections_with_secrets() else {
        return 0;
    };
    let count = rows.len();
    for conn in &rows {
        register_live_connection_in_mcp_registry(mcp_registry, conn);
    }
    count
}

fn sync_mcp_servers_db_row(db: &ServerDatabase, conn: &MarketplaceConnectionDto, server_id: &str) {
    let defs: Vec<McpToolDefinition> = conn
        .discovered_tools
        .iter()
        .map(|t| {
            McpToolDefinition::new(
                t.name.clone(),
                t.description.clone(),
                serde_json::to_string(&t.input_schema).unwrap_or_else(|_| "{}".into()),
                tool_risk_for_name(&t.name),
            )
            .with_server_id(server_id.to_string())
        })
        .collect();
    let endpoint = conn.endpoint.as_deref().unwrap_or("");
    let tools_val = serde_json::to_value(&defs).unwrap_or_else(|_| json!([]));
    let _ = db.upsert_mcp_server(server_id, &conn.name, endpoint, &conn.status, &tools_val);
}

/// `GET /api/v1/marketplace/catalog` — returns full server-published OpenBot Marketplace catalog
/// from SQLite along with real install counts and the caller's live verified connections.
pub async fn marketplace_catalog_get(
    auth: AuthBearer,
    State(state): State<Arc<AppState>>,
) -> Response {
    let apps = match state.db.list_marketplace_apps(None, None) {
        Ok(list) => list,
        Err(err) => return map_telegram_err(err),
    };
    let agents = match state.db.list_marketplace_agents() {
        Ok(list) => list,
        Err(err) => return map_telegram_err(err),
    };
    let skills = match state.db.list_marketplace_skills() {
        Ok(list) => list,
        Err(err) => return map_telegram_err(err),
    };
    let connections = match state.db.list_marketplace_user_connections(auth.user_id) {
        Ok(list) => list,
        Err(err) => return map_telegram_err(err),
    };

    let total_apps = apps.len();
    let total_connectors = apps.iter().filter(|a| a.kind == "connector").count();
    let total_plugins = apps.iter().filter(|a| a.kind == "plugin").count();
    let total_published_custom = apps.iter().filter(|a| a.kind == "custom_published").count();

    (
        StatusCode::OK,
        Json(MarketplaceCatalogResponse {
            apps,
            agents,
            skills,
            connections,
            total_apps,
            total_connectors,
            total_plugins,
            total_published_custom,
            synced_at_ms: Utc::now().timestamp_millis(),
        }),
    )
        .into_response()
}

/// `GET /api/v1/marketplace/apps` — search/filter published connectors & MCP plugins in SQLite.
pub async fn marketplace_apps_list(
    _auth: AuthBearer,
    State(state): State<Arc<AppState>>,
    Query(params): Query<HashMap<String, String>>,
) -> Response {
    let category = params.get("category").map(String::as_str);
    let q = params.get("q").map(String::as_str);
    match state.db.list_marketplace_apps(category, q) {
        Ok(apps) => (
            StatusCode::OK,
            Json(json!({ "apps": apps, "total": apps.len() })),
        )
            .into_response(),
        Err(err) => map_telegram_err(err),
    }
}

/// `POST /api/v1/marketplace/apps` — publishes a real new connector / MCP server into `marketplace_apps`.
pub async fn marketplace_apps_publish(
    auth: AuthBearer,
    State(state): State<Arc<AppState>>,
    body: Bytes,
) -> Response {
    let req: PublishMarketplaceAppRequest = match serde_json::from_slice(&body) {
        Ok(r) => r,
        Err(e) => {
            return err_resp(
                StatusCode::BAD_REQUEST,
                "invalid_json",
                &format!("Malformed publish payload: {e}"),
            );
        }
    };

    if req.verify_before_publish && req.transport.eq_ignore_ascii_case("http") {
        let Some(ref target_url) = req.url else {
            return err_resp(
                StatusCode::BAD_REQUEST,
                "missing_url",
                "HTTP MCP connector requires a valid url.",
            );
        };
        let client = build_http_client();
        let probe = probe_http_mcp_endpoint(&client, "publish-probe", &req.name, target_url, &[]).await;
        if probe.status == "error" {
            return err_resp(
                StatusCode::UNPROCESSABLE_ENTITY,
                "mcp_unreachable",
                probe
                    .error
                    .as_deref()
                    .unwrap_or("Failed to reach MCP server endpoint during pre-publish verification."),
            );
        }
    }

    let creator_name = state
        .db
        .get_user(auth.user_id)
        .ok()
        .flatten()
        .map(|u| {
            u.username
                .filter(|s| !s.trim().is_empty())
                .unwrap_or(u.first_name)
        })
        .unwrap_or_else(|| format!("user_{}", auth.user_id.0));

    match state
        .db
        .publish_custom_marketplace_app(auth.user_id, &creator_name, &req)
    {
        Ok(listing) => (StatusCode::CREATED, Json(listing)).into_response(),
        Err(err) => map_telegram_err(err),
    }
}

/// `GET /api/v1/marketplace/connections` — lists caller's real connections from SQLite.
pub async fn marketplace_connections_list(
    auth: AuthBearer,
    State(state): State<Arc<AppState>>,
) -> Response {
    match state.db.list_marketplace_user_connections(auth.user_id) {
        Ok(connections) => (
            StatusCode::OK,
            Json(json!({ "connections": connections, "total": connections.len() })),
        )
            .into_response(),
        Err(err) => map_telegram_err(err),
    }
}

/// `POST /api/v1/marketplace/connections/connect` — live-verifies a native connector or MCP server,
/// persists encrypted credentials, increments real `install_count`, and registers live executable
/// tools directly into `AppState::mcp_registry`.
pub async fn marketplace_connections_connect(
    auth: AuthBearer,
    State(state): State<Arc<AppState>>,
    body: Bytes,
) -> Response {
    let req: ConnectMarketplaceAppRequest = match serde_json::from_slice(&body) {
        Ok(r) => r,
        Err(e) => {
            return err_resp(
                StatusCode::BAD_REQUEST,
                "invalid_json",
                &format!("Malformed connect payload: {e}"),
            );
        }
    };

    let app_id = req.app_id.trim();
    if app_id.is_empty() {
        return err_resp(StatusCode::BAD_REQUEST, "missing_app_id", "app_id is required.");
    }

    let listing = state
        .db
        .get_marketplace_app(app_id)
        .ok()
        .flatten()
        .or_else(|| {
            req.slug
                .as_deref()
                .and_then(|s| state.db.get_marketplace_app(s).ok().flatten())
        });

    let effective_kind = req
        .kind
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .or_else(|| listing.as_ref().map(|l| l.kind.as_str()))
        .unwrap_or(if app_id.starts_with("connector:") {
            "connector"
        } else if app_id.starts_with("custom:") {
            "custom"
        } else {
            "plugin"
        })
        .to_string();

    let effective_name = req
        .name
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .or_else(|| listing.as_ref().map(|l| l.name.as_str()))
        .unwrap_or(app_id)
        .to_string();

    let server_id = mcp_server_id_for_user_app(auth.user_id.0, app_id);
    let client = build_http_client();
    let now_ms = Utc::now().timestamp_millis();

    // Branch A: Native OpenBot Connector (`Slack`, `Discord`, `Telegram`, `WhatsApp`, `GitHub`, `X`, Vault CLIs)
    if effective_kind == "connector" {
        let connector_id = app_id
            .strip_prefix("connector:")
            .or(req.slug.as_deref())
            .unwrap_or(app_id)
            .to_ascii_lowercase();
        let is_local_vault_cli = matches!(
            connector_id.as_str(),
            "onepassword" | "bitwarden" | "protonpass"
        );

        let (verified_account, status, endpoint, tools_proto, out_meta, verified_at_ms) =
            if is_local_vault_cli && !req.verify_live && !req.local_discovered_tools.is_empty() {
                let mut m = req.metadata.clone();
                m.insert("connector_id".into(), connector_id.clone());
                (
                    req.account_label
                        .clone()
                        .unwrap_or_else(|| effective_name.clone()),
                    "connected".to_string(),
                    format!("cli://{connector_id}"),
                    req.local_discovered_tools.clone(),
                    m,
                    Some(now_ms),
                )
            } else {
                match verify_native_connector(
                    &client,
                    &connector_id,
                    req.account_label.as_deref().unwrap_or(""),
                    req.credential.as_deref(),
                    &req.metadata,
                )
                .await
                {
                    Ok(v) => (
                        v.verified_account,
                        v.status,
                        v.endpoint,
                        to_proto_tools(&v.tools),
                        v.metadata,
                        Some(now_ms),
                    ),
                    Err(err) => {
                        return err_resp(
                            StatusCode::UNPROCESSABLE_ENTITY,
                            "connector_verification_failed",
                            &redact_secrets(&err.to_string()),
                        );
                    }
                }
            };

        let secret = StoredConnectorSecret {
            credential: req.credential.clone().unwrap_or_default(),
            headers: Vec::new(),
            env: Vec::new(),
            command: None,
            args: Vec::new(),
        };
        let transport = if is_local_vault_cli { "stdio" } else { "http" };
        let dto = match state.db.upsert_marketplace_user_connection(
            auth.user_id,
            app_id,
            "connector",
            &effective_name,
            &status,
            Some(&verified_account),
            Some(&endpoint),
            transport,
            Some(&server_id),
            &secret,
            &out_meta,
            &tools_proto,
            None,
            verified_at_ms,
        ) {
            Ok(d) => d,
            Err(err) => return map_telegram_err(err),
        };
        let install_count = state
            .db
            .increment_marketplace_app_installs(app_id)
            .unwrap_or(0);
        sync_mcp_servers_db_row(&state.db, &dto, &server_id);
        register_live_connection_in_mcp_registry(
            &state.mcp_registry,
            &HydratedMarketplaceConnection {
                user_id: auth.user_id.0,
                dto: dto.clone(),
                secret,
            },
        );
        return (
            StatusCode::OK,
            Json(ConnectMarketplaceAppResponse {
                connection: dto,
                install_count,
                requires_oauth: false,
                www_authenticate: None,
            }),
        )
            .into_response();
    }

    // Branch B: Upstream OpenBot MCP Plugin, User-Published Connector, or Custom STDIO/HTTP MCP Server
    let effective_transport = req
        .transport
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .or_else(|| listing.as_ref().map(|l| l.transport.as_str()))
        .unwrap_or("http")
        .to_ascii_lowercase();

    let mut headers: Vec<MarketplaceEnvPairDto> = req.headers.clone();
    let mut env_pairs: Vec<MarketplaceEnvPairDto> = listing
        .as_ref()
        .map(|l| l.env.clone())
        .unwrap_or_default();

    if let Some(ref l) = listing {
        for field in &l.auth_fields {
            let val = req
                .auth_values
                .get(&field.id)
                .map(|s| s.trim())
                .unwrap_or("");
            if val.is_empty() && !field.optional && req.verify_live {
                return err_resp(
                    StatusCode::BAD_REQUEST,
                    "missing_credential",
                    &format!("Missing required credential field '{}'.", field.label),
                );
            }
            if val.is_empty() {
                continue;
            }
            let full_val = match &field.prefix {
                Some(pref) if !val.starts_with(pref) => format!("{pref}{val}"),
                _ => val.to_string(),
            };
            if let Some(ref hdr) = field.header {
                headers.push(MarketplaceEnvPairDto {
                    key: hdr.clone(),
                    value: full_val,
                });
            } else if let Some(ref env_k) = field.env {
                env_pairs.push(MarketplaceEnvPairDto {
                    key: env_k.clone(),
                    value: full_val,
                });
            }
        }
    }

    if let Some(tok) = req
        .auth_values
        .get("oauth_token")
        .or(req.credential.as_ref())
        .map(|s| s.trim())
        .filter(|s| !s.is_empty())
    {
        if !headers.iter().any(|h| h.key.eq_ignore_ascii_case("authorization")) {
            headers.push(MarketplaceEnvPairDto {
                key: "Authorization".into(),
                value: if tok.to_ascii_lowercase().starts_with("bearer ") {
                    tok.to_string()
                } else {
                    format!("Bearer {tok}")
                },
            });
        }
    }

    let endpoint = req
        .override_url
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
        .or_else(|| listing.as_ref().and_then(|l| l.url.clone()))
        .or_else(|| req.command.clone())
        .or_else(|| listing.as_ref().and_then(|l| l.command.clone()));

    let mut status = "connected".to_string();
    let mut discovered_tools = req.local_discovered_tools.clone();
    let mut last_error: Option<String> = None;
    let mut requires_oauth = false;
    let mut www_authenticate: Option<String> = None;
    let mut verified_at_ms = (!discovered_tools.is_empty()).then_some(now_ms);

    if effective_transport == "http" && req.verify_live {
        let Some(ref target_url) = endpoint else {
            return err_resp(
                StatusCode::BAD_REQUEST,
                "missing_endpoint",
                "HTTP MCP plugin is missing its endpoint URL.",
            );
        };
        let kv_headers: Vec<McpKeyValue> = headers
            .iter()
            .map(|h| McpKeyValue {
                key: h.key.clone(),
                value: h.value.clone(),
            })
            .collect();
        let probe = probe_http_mcp_endpoint(
            &client,
            &server_id,
            &effective_name,
            target_url,
            &kv_headers,
        )
        .await;
        match probe.status.as_str() {
            "ready" => {
                status = "connected".to_string();
                discovered_tools = to_proto_tools(&probe.tool_defs);
                last_error = None;
                verified_at_ms = Some(now_ms);
            }
            "needs_sign_in" => {
                status = "needs_sign_in".to_string();
                discovered_tools.clear();
                last_error = probe.error;
                requires_oauth = true;
                www_authenticate = probe.www_authenticate;
                verified_at_ms = None;
            }
            "needs_key" => {
                return err_resp(
                    StatusCode::UNPROCESSABLE_ENTITY,
                    "mcp_auth_failed",
                    probe
                        .error
                        .as_deref()
                        .unwrap_or("MCP server rejected the supplied API key or token."),
                );
            }
            _ => {
                return err_resp(
                    StatusCode::UNPROCESSABLE_ENTITY,
                    "mcp_probe_failed",
                    probe
                        .error
                        .as_deref()
                        .unwrap_or("Failed to handshake with MCP server."),
                );
            }
        }
    }

    let secret = StoredConnectorSecret {
        credential: req.credential.clone().unwrap_or_default(),
        headers,
        env: env_pairs,
        command: req
            .command
            .clone()
            .or_else(|| listing.as_ref().and_then(|l| l.command.clone())),
        args: if !req.args.is_empty() {
            req.args.clone()
        } else {
            listing.as_ref().map(|l| l.args.clone()).unwrap_or_default()
        },
    };

    let account_label = req
        .account_label
        .clone()
        .or_else(|| endpoint.clone())
        .or_else(|| Some(effective_name.clone()));

    let dto = match state.db.upsert_marketplace_user_connection(
        auth.user_id,
        app_id,
        &effective_kind,
        &effective_name,
        &status,
        account_label.as_deref(),
        endpoint.as_deref(),
        &effective_transport,
        Some(&server_id),
        &secret,
        &req.metadata,
        &discovered_tools,
        last_error.as_deref(),
        verified_at_ms,
    ) {
        Ok(d) => d,
        Err(err) => return map_telegram_err(err),
    };

    let install_count = if status == "connected" {
        sync_mcp_servers_db_row(&state.db, &dto, &server_id);
        register_live_connection_in_mcp_registry(
            &state.mcp_registry,
            &HydratedMarketplaceConnection {
                user_id: auth.user_id.0,
                dto: dto.clone(),
                secret,
            },
        );
        state
            .db
            .increment_marketplace_app_installs(app_id)
            .unwrap_or(0)
    } else {
        listing.map(|l| l.install_count).unwrap_or(0)
    };

    (
        StatusCode::OK,
        Json(ConnectMarketplaceAppResponse {
            connection: dto,
            install_count,
            requires_oauth,
            www_authenticate,
        }),
    )
        .into_response()
}

/// `POST /api/v1/marketplace/connections/{app_id}/probe` — live re-probes a stored connection.
pub async fn marketplace_connection_probe(
    auth: AuthBearer,
    State(state): State<Arc<AppState>>,
    Path(app_id): Path<String>,
) -> Response {
    let Some(hydrated) = (match state
        .db
        .get_marketplace_user_connection_with_secret(auth.user_id, &app_id)
    {
        Ok(v) => v,
        Err(err) => return map_telegram_err(err),
    }) else {
        return err_resp(StatusCode::NOT_FOUND, "not_found", "Connection not found.");
    };

    let client = build_http_client();
    let now_ms = Utc::now().timestamp_millis();
    let server_id = hydrated
        .dto
        .mcp_server_id
        .clone()
        .unwrap_or_else(|| mcp_server_id_for_user_app(auth.user_id.0, &app_id));

    if hydrated.dto.kind == "connector" {
        let connector_id = hydrated
            .dto
            .metadata
            .get("connector_id")
            .cloned()
            .unwrap_or_else(|| app_id.strip_prefix("connector:").unwrap_or(&app_id).to_string());
        match verify_native_connector(
            &client,
            &connector_id,
            hydrated.dto.account_label.as_deref().unwrap_or(""),
            (!hydrated.secret.credential.is_empty()).then_some(hydrated.secret.credential.as_str()),
            &hydrated.dto.metadata,
        )
        .await
        {
            Ok(v) => {
                let tools_proto = to_proto_tools(&v.tools);
                let dto = match state.db.upsert_marketplace_user_connection(
                    auth.user_id,
                    &app_id,
                    "connector",
                    &hydrated.dto.name,
                    &v.status,
                    Some(&v.verified_account),
                    Some(&v.endpoint),
                    &hydrated.dto.transport,
                    Some(&server_id),
                    &hydrated.secret,
                    &v.metadata,
                    &tools_proto,
                    None,
                    Some(now_ms),
                ) {
                    Ok(d) => d,
                    Err(err) => return map_telegram_err(err),
                };
                sync_mcp_servers_db_row(&state.db, &dto, &server_id);
                register_live_connection_in_mcp_registry(
                    &state.mcp_registry,
                    &HydratedMarketplaceConnection {
                        user_id: auth.user_id.0,
                        dto: dto.clone(),
                        secret: hydrated.secret,
                    },
                );
                return (
                    StatusCode::OK,
                    Json(ConnectMarketplaceAppResponse {
                        connection: dto,
                        install_count: state
                            .db
                            .get_marketplace_app(&app_id)
                            .ok()
                            .flatten()
                            .map(|a| a.install_count)
                            .unwrap_or(0),
                        requires_oauth: false,
                        www_authenticate: None,
                    }),
                )
                    .into_response();
            }
            Err(err) => {
                return err_resp(
                    StatusCode::UNPROCESSABLE_ENTITY,
                    "probe_failed",
                    &redact_secrets(&err.to_string()),
                );
            }
        }
    }

    if !hydrated.dto.transport.eq_ignore_ascii_case("http") {
        return (
            StatusCode::OK,
            Json(ConnectMarketplaceAppResponse {
                connection: hydrated.dto,
                install_count: 0,
                requires_oauth: false,
                www_authenticate: None,
            }),
        )
            .into_response();
    }

    let endpoint = hydrated.dto.endpoint.as_deref().unwrap_or("");
    let kv_headers: Vec<McpKeyValue> = hydrated
        .secret
        .headers
        .iter()
        .map(|h| McpKeyValue {
            key: h.key.clone(),
            value: h.value.clone(),
        })
        .collect();
    let probe = probe_http_mcp_endpoint(
        &client,
        &server_id,
        &hydrated.dto.name,
        endpoint,
        &kv_headers,
    )
    .await;
    let next_status = if probe.status == "ready" {
        "connected"
    } else {
        probe.status.as_str()
    };
    let tools_proto = to_proto_tools(&probe.tool_defs);
    let verified_at = (next_status == "connected").then_some(now_ms);
    let dto = match state.db.upsert_marketplace_user_connection(
        auth.user_id,
        &app_id,
        &hydrated.dto.kind,
        &hydrated.dto.name,
        next_status,
        hydrated.dto.account_label.as_deref(),
        hydrated.dto.endpoint.as_deref(),
        &hydrated.dto.transport,
        Some(&server_id),
        &hydrated.secret,
        &hydrated.dto.metadata,
        &tools_proto,
        probe.error.as_deref(),
        verified_at,
    ) {
        Ok(d) => d,
        Err(err) => return map_telegram_err(err),
    };
    sync_mcp_servers_db_row(&state.db, &dto, &server_id);
    register_live_connection_in_mcp_registry(
        &state.mcp_registry,
        &HydratedMarketplaceConnection {
            user_id: auth.user_id.0,
            dto: dto.clone(),
            secret: hydrated.secret,
        },
    );

    (
        StatusCode::OK,
        Json(ConnectMarketplaceAppResponse {
            connection: dto,
            install_count: state
                .db
                .get_marketplace_app(&app_id)
                .ok()
                .flatten()
                .map(|a| a.install_count)
                .unwrap_or(0),
            requires_oauth: probe.status == "needs_sign_in",
            www_authenticate: probe.www_authenticate,
        }),
    )
        .into_response()
}

/// `POST /api/v1/marketplace/connections/{app_id}/call` — executes a real live tool call against
/// the connected native API or Streamable HTTP MCP server.
pub async fn marketplace_connection_call(
    auth: AuthBearer,
    State(state): State<Arc<AppState>>,
    Path(app_id): Path<String>,
    body: Bytes,
) -> Response {
    let req: CallMarketplaceToolRequest = match serde_json::from_slice(&body) {
        Ok(r) => r,
        Err(e) => {
            return err_resp(
                StatusCode::BAD_REQUEST,
                "invalid_json",
                &format!("Malformed tool call payload: {e}"),
            );
        }
    };
    let Some(hydrated) = (match state
        .db
        .get_marketplace_user_connection_with_secret(auth.user_id, &app_id)
    {
        Ok(v) => v,
        Err(err) => return map_telegram_err(err),
    }) else {
        return err_resp(StatusCode::NOT_FOUND, "not_found", "Connection not found.");
    };

    let client = build_http_client();
    let res = if hydrated.dto.kind == "connector" {
        let connector_id = hydrated
            .dto
            .metadata
            .get("connector_id")
            .cloned()
            .unwrap_or_else(|| app_id.strip_prefix("connector:").unwrap_or(&app_id).to_string());
        execute_native_connector_tool(
            &client,
            &connector_id,
            &hydrated.secret.credential,
            &hydrated.dto.metadata,
            &req.tool_name,
            &req.arguments,
        )
        .await
        .map_err(|e| redact_secrets(&e.to_string()))
    } else if hydrated.dto.transport.eq_ignore_ascii_case("http") {
        let ep = hydrated.dto.endpoint.as_deref().unwrap_or("");
        let kv_headers: Vec<McpKeyValue> = hydrated
            .secret
            .headers
            .iter()
            .map(|h| McpKeyValue {
                key: h.key.clone(),
                value: h.value.clone(),
            })
            .collect();
        let args_obj = if req.arguments.is_object() {
            req.arguments.clone()
        } else {
            json!({})
        };
        call_http_mcp_endpoint_jsonrpc(
            &client,
            &hydrated.dto.name,
            ep,
            &kv_headers,
            "tools/call",
            json!({ "name": req.tool_name, "arguments": args_obj }),
        )
        .await
        .map_err(|(e, _)| redact_secrets(&e.to_string()))
    } else {
        Err("Local STDIO MCP servers execute directly on the user's desktop client.".to_string())
    };

    match res {
        Ok(val) => (
            StatusCode::OK,
            Json(CallMarketplaceToolResponse {
                app_id,
                tool_name: req.tool_name,
                is_error: false,
                result: val,
            }),
        )
            .into_response(),
        Err(err_msg) => (
            StatusCode::OK,
            Json(CallMarketplaceToolResponse {
                app_id,
                tool_name: req.tool_name,
                is_error: true,
                result: json!({ "error": err_msg }),
            }),
        )
            .into_response(),
    }
}

/// `DELETE /api/v1/marketplace/connections/{app_id}` — disconnects and unregisters live MCP tools.
pub async fn marketplace_connection_disconnect(
    auth: AuthBearer,
    State(state): State<Arc<AppState>>,
    Path(app_id): Path<String>,
) -> Response {
    match state
        .db
        .delete_marketplace_user_connection(auth.user_id, &app_id)
    {
        Ok(Some(mcp_server_id)) => {
            if !mcp_server_id.is_empty() {
                let _ = state.db.delete_mcp_server(&mcp_server_id);
                if let Ok(mut reg) = state.mcp_registry.write() {
                    reg.remove_by_server_id(&mcp_server_id);
                }
            }
            (
                StatusCode::OK,
                Json(json!({ "disconnected": true, "appId": app_id })),
            )
                .into_response()
        }
        Ok(None) => (
            StatusCode::OK,
            Json(json!({ "disconnected": false, "appId": app_id })),
        )
            .into_response(),
        Err(err) => map_telegram_err(err),
    }
}

/// `POST /api/v1/marketplace/agents/{slug}/install` — records a real agent install count increment.
pub async fn marketplace_agent_install(
    _auth: AuthBearer,
    State(state): State<Arc<AppState>>,
    Path(slug): Path<String>,
) -> Response {
    match state.db.increment_marketplace_agent_installs(&slug) {
        Ok(install_count) => (
            StatusCode::OK,
            Json(MarketplaceInstallResponse {
                slug,
                install_count,
            }),
        )
            .into_response(),
        Err(err) => map_telegram_err(err),
    }
}

/// `POST /api/v1/marketplace/skills/{slug}/install` — records a real skill install count increment.
pub async fn marketplace_skill_install(
    _auth: AuthBearer,
    State(state): State<Arc<AppState>>,
    Path(slug): Path<String>,
) -> Response {
    match state.db.increment_marketplace_skill_installs(&slug) {
        Ok(install_count) => (
            StatusCode::OK,
            Json(MarketplaceInstallResponse {
                slug,
                install_count,
            }),
        )
            .into_response(),
        Err(err) => map_telegram_err(err),
    }
}
