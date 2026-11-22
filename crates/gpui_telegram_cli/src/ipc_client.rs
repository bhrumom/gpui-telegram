use crate::cli_parser::GlobalOptions;
use crate::error::CliError;
use crate::http_client::ActionHttpClient;
use crate::session_store::{load_cli_session, CliSession};
use fabushi_remote_engine::ipc::{default_ipc_endpoint, LocalIpcClient};
use gpui_telegram_proto::remote_control_contracts::LocalIpcResponse;
use reqwest::Method;
use serde_json::{json, Value};
use std::path::PathBuf;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BackendSource {
    LocalIpc,
    HttpServer,
}

pub struct FabushiBackendClient {
    ipc_endpoint: PathBuf,
    ipc_explicit: bool,
    force_remote: bool,
    server_url: String,
    server_explicit: bool,
    token: Option<String>,
    saved_session: Option<CliSession>,
}

impl FabushiBackendClient {
    pub fn from_options(opts: &GlobalOptions) -> Self {
        let saved_session = load_cli_session();
        let ipc_explicit = opts.socket_path.is_some()
            || std::env::var_os("FABUSHI_IPC_SOCKET")
                .map(|v| !v.is_empty())
                .unwrap_or(false);
        let ipc_endpoint = opts
            .socket_path
            .clone()
            .unwrap_or_else(default_ipc_endpoint);

        let server_url = if opts.server_explicit {
            opts.server_url.clone()
        } else if let Some(ref sess) = saved_session {
            if !sess.server_url.trim().is_empty() {
                sess.server_url.trim_end_matches('/').to_string()
            } else {
                opts.server_url.clone()
            }
        } else {
            opts.server_url.clone()
        };

        let token = opts
            .token
            .clone()
            .or_else(|| saved_session.as_ref().map(|s| s.token.clone()));

        Self {
            ipc_endpoint,
            ipc_explicit,
            force_remote: opts.remote,
            server_url,
            server_explicit: opts.server_explicit,
            token,
            saved_session,
        }
    }

    pub fn ipc_endpoint(&self) -> &PathBuf {
        &self.ipc_endpoint
    }

    pub fn ipc_client(&self) -> LocalIpcClient {
        LocalIpcClient::new(self.ipc_endpoint.clone())
    }

    pub fn server_url(&self) -> &str {
        &self.server_url
    }

    pub fn token(&self) -> Option<&str> {
        self.token.as_deref()
    }

    pub fn saved_session(&self) -> Option<&CliSession> {
        self.saved_session.as_ref()
    }

    pub fn http_client(&self) -> Result<ActionHttpClient, CliError> {
        ActionHttpClient::new(&self.server_url)
    }

    pub fn require_token(&self) -> Result<&str, CliError> {
        self.token.as_deref().ok_or_else(|| {
            CliError::Unauthorized(
                "Authentication required: start Fabushi Desktop/Daemon for zero-config local IPC, run 'fabushi auth login', or pass --token / FABUSHI_TOKEN".into(),
            )
        })
    }

    /// Attempts local IPC call against running Fabushi Desktop or `fabushi-remote-daemon`.
    /// Returns `Ok(Some(resp))` when IPC responded, `Ok(None)` if IPC endpoint is offline/unreachable
    /// or the target does not implement `method`.
    pub async fn try_ipc(&self, method: &str, params: Value) -> Result<Option<LocalIpcResponse>, CliError> {
        if self.force_remote || (self.server_explicit && !self.ipc_explicit) {
            return Ok(None);
        }
        let client = LocalIpcClient::new(self.ipc_endpoint.clone());
        match client.call(method, params).await {
            Ok(resp) => {
                if !resp.success {
                    let code = resp
                        .error
                        .as_ref()
                        .map(|e| e.code.as_str())
                        .unwrap_or("");
                    if matches!(code, "method_not_found" | "not_implemented" | "unsupported") {
                        return Ok(None);
                    }
                }
                Ok(Some(resp))
            }
            Err(_) => Ok(None),
        }
    }

    /// Executes an IPC method directly without HTTP fallback (used for local daemon / local PTY mux).
    pub async fn call_ipc_required(&self, method: &str, params: Value) -> Result<Value, CliError> {
        let client = LocalIpcClient::new(self.ipc_endpoint.clone());
        let resp = client.call(method, params).await.map_err(|e| {
            CliError::Execution(format!(
                "Local Fabushi IPC unavailable at '{}': {e}",
                self.ipc_endpoint.display()
            ))
        })?;
        map_ipc_response(resp)
    }

    /// Tries local IPC first (unless `--remote` / explicit `--server-url`), falling back cleanly
    /// to the HTTP server using `--token` / `FABUSHI_TOKEN` / `~/.fabushi/cli_session.json`.
    pub async fn call_ipc_or_http<F, Fut>(
        &self,
        ipc_method: &str,
        ipc_params: Value,
        http_fallback: F,
    ) -> Result<(BackendSource, Value), CliError>
    where
        F: FnOnce(ActionHttpClient, Option<String>) -> Fut,
        Fut: std::future::Future<Output = Result<Value, CliError>>,
    {
        if let Some(resp) = self.try_ipc(ipc_method, ipc_params).await? {
            let val = map_ipc_response(resp)?;
            return Ok((BackendSource::LocalIpc, val));
        }

        let http = self.http_client()?;
        let val = http_fallback(http, self.token.clone()).await?;
        Ok((BackendSource::HttpServer, val))
    }

    pub async fn http_authenticated_json(
        &self,
        method: Method,
        path: &str,
        body: Option<&Value>,
        query: &[(&str, String)],
    ) -> Result<Value, CliError> {
        let token = self.require_token()?;
        let http = self.http_client()?;
        http.request_json(method, path, Some(token), body, query)
            .await
    }

    pub async fn probe_ipc_status(&self) -> Value {
        let client = LocalIpcClient::new(self.ipc_endpoint.clone());
        match client.call("status", json!({})).await {
            Ok(resp) if resp.success => json!({
                "connected": true,
                "endpoint": self.ipc_endpoint.display().to_string(),
                "status": resp.data.unwrap_or_else(|| json!({})),
            }),
            Ok(resp) => json!({
                "connected": false,
                "endpoint": self.ipc_endpoint.display().to_string(),
                "error": resp.error.map(|e| e.message).unwrap_or_else(|| "IPC error".into()),
            }),
            Err(e) => json!({
                "connected": false,
                "endpoint": self.ipc_endpoint.display().to_string(),
                "error": e.to_string(),
            }),
        }
    }
}

pub fn map_ipc_response(resp: LocalIpcResponse) -> Result<Value, CliError> {
    if resp.success {
        return Ok(resp.data.unwrap_or_else(|| json!({ "ok": true })));
    }
    let err = resp.error.unwrap_or_default();
    match err.code.as_str() {
        "unauthorized" => Err(CliError::Unauthorized(err.message)),
        "forbidden" | "recursive_mux_refused" => Err(CliError::Forbidden(err.message)),
        "invalid_input" | "invalid_argument" | "validation_error" => {
            Err(CliError::Validation(err.message))
        }
        "not_found" | "unknown_command" => Err(CliError::UnknownCommand(err.message)),
        _ => Err(CliError::Execution(if err.message.is_empty() {
            format!("IPC call failed ({})", err.code)
        } else {
            err.message
        })),
    }
}
