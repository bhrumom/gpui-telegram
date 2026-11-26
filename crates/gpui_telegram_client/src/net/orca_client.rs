//! Typed async client for all Orca REST & Unified RPC v3 routes (`/api/v1/orca/*`).
//!
//! Preserves exact Orca `snake_case` error codes (`bot_name_taken`, `approval_required`,
//! `system_shortcut_blocked`, `stale_dispatch`, etc.) extracted from nested JSON envelopes
//! `{"error": {"code": "...", "message": "..."}}`.

use std::fmt;

use gpui_telegram_proto::orca_contracts::{
    OrcaAutomationCreateRequest, OrcaAutomationPatchRequest, OrcaBotChatReplyRequest,
    OrcaChatTreeRequest, OrcaCreateAccessRequest, OrcaCreateBotChatRequest, OrcaCreateBotRequest,
    OrcaCreateGroupRequest, OrcaCreateGroupThreadRequest, OrcaCreateItemRequest,
    OrcaDecideAccessRequest, OrcaDeleteBotResponse, OrcaFromTemplateRequest, OrcaHandOverRequest,
    OrcaImportDiscoverRequest, OrcaItemCommentRequest, OrcaItemSnoozeRequest,
    OrcaLookAroundRequest, OrcaMemoryForgetRequest, OrcaMemoryUndoRequest, OrcaMemoryWriteRequest,
    OrcaPackageExportRequest, OrcaPackageImportRequest, OrcaRelayRegisterGrantRequest,
    OrcaRelayRegisterToolRequest, OrcaRelayRevokeRequest, OrcaRelayStartResponse,
    OrcaRelayStatusResponse, OrcaRpcRequest, OrcaRpcResponse, OrcaSendGroupMessageRequest,
    OrcaShareOpenRequest, OrcaShareSealRequest, OrcaTelegramCardPostRequest,
    OrcaTelegramLinkRequest, OrcaTidyUpRequest, OrcaTidyUpUndoRequest, OrcaUpNextActionRequest,
    OrcaUpdateBotRequest, OrcaUpdateItemRequest, OrcaWatchPrRequest, OrcaWebhookCreateRequest,
    OrcaWebhookPatchRequest,
};
use orca_core::bot_ecosystem::TeamSharePublicIdentity;
use orca_core::bot_groups::BotGroup;
use orca_core::bot_items::BotItem;
use orca_core::bot_types::OrcaBotRecord;
use orca_core::orchestration_protocol::{
    ComputerUseGuardDecision, ComputerUsePolicy, ComputerUseRequest, DiffAnnotation,
    HibernationDecision,
};
use orca_core::workspace_layout::OrcaYamlConfig;
use reqwest::Method;
use serde::de::DeserializeOwned;
use serde::Serialize;
use serde_json::{json, Value};
use thiserror::Error;

use crate::net::http_client::{HttpClient, HttpClientError};
use crate::net::http_helpers::percent_encode_rfc3986;

/// Structured error returned by [`OrcaClient`], preserving Orca's exact `snake_case` error codes.
#[derive(Debug, Error)]
pub enum OrcaClientError {
    #[error("Orca API error ({status}, {code}): {message}")]
    Api {
        status: u16,
        code: String,
        message: String,
    },
    #[error("Network error: {0}")]
    Network(#[from] reqwest::Error),
    #[error("Proxy error: {0}")]
    Proxy(String),
    #[error("Serialization error: {0}")]
    Serialization(#[from] serde_json::Error),
}

impl OrcaClientError {
    /// Extracts structured `(code, message)` from an Orca error response body in canonical
    /// precedence order:
    /// `error.code` -> `error_detail.code` -> `error` (string) -> `code` (string) -> HTTP default.
    pub fn from_response(status: u16, body: &str) -> Self {
        let trimmed = body.trim();
        let (code, message) = extract_orca_error_fields(status, trimmed);
        Self::Api {
            status,
            code,
            message,
        }
    }

    pub fn code(&self) -> Option<&str> {
        match self {
            Self::Api { code, .. } => Some(code.as_str()),
            _ => None,
        }
    }

    pub fn status(&self) -> Option<u16> {
        match self {
            Self::Api { status, .. } => Some(*status),
            _ => None,
        }
    }

    pub fn is_code(&self, expected: &str) -> bool {
        self.code() == Some(expected)
    }
}

impl From<OrcaClientError> for HttpClientError {
    fn from(err: OrcaClientError) -> Self {
        match err {
            OrcaClientError::Network(e) => Self::Network(e),
            OrcaClientError::Proxy(msg) => Self::Proxy(msg),
            OrcaClientError::Serialization(e) => Self::Serialization(e),
            OrcaClientError::Api {
                status: 401,
                message,
                ..
            } => Self::Unauthorized(message),
            OrcaClientError::Api {
                status: 409,
                code,
                message,
            } => Self::Conflict(format!("{code}: {message}")),
            OrcaClientError::Api {
                status,
                code,
                message,
            } => Self::Api {
                status,
                message: format!("{code}: {message}"),
            },
        }
    }
}

fn default_code_for_status(status: u16) -> &'static str {
    match status {
        400 => "invalid_argument",
        401 => "unauthorized",
        403 => "forbidden",
        404 => "not_found",
        408 => "wait_timeout",
        409 => "conflict",
        410 => "expired",
        413 => "payload_too_large",
        422 => "unprocessable_entity",
        429 => "rate_limited",
        503 => "service_unavailable",
        _ => "server_error",
    }
}

/// Extracts `(code, message)` from an Orca JSON or plain-text HTTP error body.
pub fn extract_orca_error_fields(status: u16, body: &str) -> (String, String) {
    let trimmed = body.trim();
    if let Ok(val) = serde_json::from_str::<Value>(trimmed) {
        let code = val
            .get("error")
            .and_then(|e| e.get("code"))
            .and_then(Value::as_str)
            .or_else(|| {
                val.get("error_detail")
                    .and_then(|e| e.get("code"))
                    .and_then(Value::as_str)
            })
            .or_else(|| val.get("error").and_then(Value::as_str))
            .or_else(|| val.get("code").and_then(Value::as_str))
            .filter(|s| !s.trim().is_empty())
            .unwrap_or_else(|| default_code_for_status(status))
            .to_string();

        let message = val
            .get("error")
            .and_then(|e| e.get("message"))
            .and_then(Value::as_str)
            .or_else(|| {
                val.get("error_detail")
                    .and_then(|e| e.get("message"))
                    .and_then(Value::as_str)
            })
            .or_else(|| val.get("message").and_then(Value::as_str))
            .filter(|s| !s.trim().is_empty())
            .map(ToOwned::to_owned)
            .unwrap_or_else(|| format!("HTTP {status}: {code}"));

        return (code, message);
    }

    let code = default_code_for_status(status).to_string();
    let message = if trimmed.is_empty() {
        format!("HTTP {status} error")
    } else {
        trimmed.to_string()
    };
    (code, message)
}

/// Typed async client for Orca Bot OS, Multi-Agent Orchestration, and Agent IDE endpoints.
#[derive(Clone)]
pub struct OrcaClient {
    http: HttpClient,
    token: Option<String>,
}

impl fmt::Debug for OrcaClient {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("OrcaClient")
            .field("base_url", &self.http.base_url)
            .field("has_token", &self.token.is_some())
            .finish()
    }
}

fn build_query_path(base: &str, params: &[(&str, String)]) -> String {
    if params.is_empty() {
        return base.to_string();
    }
    let mut out = String::from(base);
    let mut sep = if base.contains('?') { '&' } else { '?' };
    for (k, v) in params {
        out.push(sep);
        out.push_str(&percent_encode_rfc3986(k));
        out.push('=');
        out.push_str(&percent_encode_rfc3986(v));
        sep = '&';
    }
    out
}

impl OrcaClient {
    pub fn new(http: HttpClient) -> Self {
        Self { http, token: None }
    }

    pub fn from_url(base_url: impl Into<String>, token: Option<String>) -> Self {
        Self {
            http: HttpClient::new(base_url.into()),
            token,
        }
    }

    pub fn with_token(mut self, token: impl Into<String>) -> Self {
        self.token = Some(token.into());
        self
    }

    pub fn http(&self) -> &HttpClient {
        &self.http
    }

    pub fn token(&self) -> Option<&str> {
        self.token.as_deref()
    }

    async fn send_json<B: Serialize + ?Sized, R: DeserializeOwned + Send + 'static>(
        &self,
        method: Method,
        path: &str,
        body: Option<&B>,
    ) -> Result<R, OrcaClientError> {
        let client = self.http.client.clone();
        let url = self.http.endpoint_url(path);
        let bearer = self.token.clone();
        let json_body = match body {
            Some(b) => Some(serde_json::to_value(b)?),
            None => None,
        };
        let (status, text) = self
            .http
            .run_on_tokio(async move {
                let mut req = client.request(method, &url);
                if let Some(ref t) = bearer {
                    req = req.bearer_auth(t);
                }
                if let Some(ref val) = json_body {
                    req = req.json(val);
                }
                let resp = req.send().await?;
                let status = resp.status().as_u16();
                let text = resp.text().await?;
                Ok((status, text))
            })
            .await
            .map_err(|err| match err {
                HttpClientError::Network(e) => OrcaClientError::Network(e),
                HttpClientError::Proxy(msg) => OrcaClientError::Proxy(msg),
                HttpClientError::Serialization(e) => OrcaClientError::Serialization(e),
                HttpClientError::Unauthorized(msg) => OrcaClientError::Api {
                    status: 401,
                    code: "unauthorized".to_string(),
                    message: msg,
                },
                HttpClientError::Conflict(msg) => OrcaClientError::Api {
                    status: 409,
                    code: "conflict".to_string(),
                    message: msg,
                },
                HttpClientError::Api { status, message } => OrcaClientError::Api {
                    status,
                    code: default_code_for_status(status).to_string(),
                    message,
                },
            })?;

        if !(200..300).contains(&status) {
            return Err(OrcaClientError::from_response(status, &text));
        }
        Ok(serde_json::from_str(&text)?)
    }

    // -----------------------------------------------------------------------
    // 1. Unified RPC v3
    // -----------------------------------------------------------------------

    pub async fn rpc(&self, req: &OrcaRpcRequest) -> Result<OrcaRpcResponse, OrcaClientError> {
        self.send_json(Method::POST, "/api/v1/orca/rpc", Some(req))
            .await
    }

    // -----------------------------------------------------------------------
    // 2. Bots Fleet & Built-in Templates
    // -----------------------------------------------------------------------

    pub async fn list_bots(&self) -> Result<Vec<OrcaBotRecord>, OrcaClientError> {
        self.send_json(Method::GET, "/api/v1/orca/bots", None::<&()>)
            .await
    }

    pub async fn create_bot(
        &self,
        req: &OrcaCreateBotRequest,
    ) -> Result<OrcaBotRecord, OrcaClientError> {
        self.send_json(Method::POST, "/api/v1/orca/bots", Some(req))
            .await
    }

    pub async fn get_bot(&self, bot_id: &str) -> Result<OrcaBotRecord, OrcaClientError> {
        let path = format!("/api/v1/orca/bots/{}", percent_encode_rfc3986(bot_id));
        self.send_json(Method::GET, &path, None::<&()>).await
    }

    pub async fn update_bot(
        &self,
        bot_id: &str,
        req: &OrcaUpdateBotRequest,
    ) -> Result<OrcaBotRecord, OrcaClientError> {
        let path = format!("/api/v1/orca/bots/{}", percent_encode_rfc3986(bot_id));
        self.send_json(Method::PATCH, &path, Some(req)).await
    }

    pub async fn delete_bot(&self, bot_id: &str) -> Result<OrcaDeleteBotResponse, OrcaClientError> {
        let path = format!("/api/v1/orca/bots/{}", percent_encode_rfc3986(bot_id));
        self.send_json(Method::DELETE, &path, None::<&()>).await
    }

    pub async fn list_templates(
        &self,
        category: Option<&str>,
    ) -> Result<Value, OrcaClientError> {
        let mut q = Vec::new();
        if let Some(cat) = category.filter(|s| !s.trim().is_empty()) {
            q.push(("category", cat.to_string()));
        }
        let path = build_query_path("/api/v1/orca/bots/templates", &q);
        self.send_json(Method::GET, &path, None::<&()>).await
    }

    pub async fn create_from_template(
        &self,
        req: &OrcaFromTemplateRequest,
    ) -> Result<Value, OrcaClientError> {
        self.send_json(Method::POST, "/api/v1/orca/bots/from-template", Some(req))
            .await
    }

    // -----------------------------------------------------------------------
    // 3. Memory Governance & Overnight Tidy-Up
    // -----------------------------------------------------------------------

    pub async fn get_memory(&self, bot_id: &str) -> Result<Value, OrcaClientError> {
        let path = format!(
            "/api/v1/orca/bots/{}/memory",
            percent_encode_rfc3986(bot_id)
        );
        self.send_json(Method::GET, &path, None::<&()>).await
    }

    pub async fn update_memory(
        &self,
        bot_id: &str,
        req: &OrcaMemoryWriteRequest,
    ) -> Result<Value, OrcaClientError> {
        let path = format!(
            "/api/v1/orca/bots/{}/memory",
            percent_encode_rfc3986(bot_id)
        );
        self.send_json(Method::POST, &path, Some(req)).await
    }

    pub async fn memory_forget(
        &self,
        bot_id: &str,
        req: &OrcaMemoryForgetRequest,
    ) -> Result<Value, OrcaClientError> {
        let path = format!(
            "/api/v1/orca/bots/{}/memory/forget",
            percent_encode_rfc3986(bot_id)
        );
        self.send_json(Method::POST, &path, Some(req)).await
    }

    pub async fn memory_undo(
        &self,
        bot_id: &str,
        req: &OrcaMemoryUndoRequest,
    ) -> Result<Value, OrcaClientError> {
        let path = format!(
            "/api/v1/orca/bots/{}/memory/undo",
            percent_encode_rfc3986(bot_id)
        );
        self.send_json(Method::POST, &path, Some(req)).await
    }

    pub async fn tidy_up(
        &self,
        bot_id: &str,
        req: &OrcaTidyUpRequest,
    ) -> Result<Value, OrcaClientError> {
        let path = format!(
            "/api/v1/orca/bots/{}/tidy-up",
            percent_encode_rfc3986(bot_id)
        );
        self.send_json(Method::POST, &path, Some(req)).await
    }

    pub async fn tidy_up_undo(
        &self,
        bot_id: &str,
        req: &OrcaTidyUpUndoRequest,
    ) -> Result<Value, OrcaClientError> {
        let path = format!(
            "/api/v1/orca/bots/{}/tidy-up/undo",
            percent_encode_rfc3986(bot_id)
        );
        self.send_json(Method::POST, &path, Some(req)).await
    }

    // -----------------------------------------------------------------------
    // 4. Bot Items, Needs You, Look Around & Up Next
    // -----------------------------------------------------------------------

    pub async fn list_items(
        &self,
        bot_id: &str,
        include_closed: bool,
        include_snoozed: bool,
        status: Option<&str>,
        limit: Option<usize>,
        offset: Option<usize>,
    ) -> Result<Vec<BotItem>, OrcaClientError> {
        let mut q = Vec::new();
        if include_closed {
            q.push(("include_closed", "true".to_string()));
        }
        if include_snoozed {
            q.push(("include_snoozed", "true".to_string()));
        }
        if let Some(st) = status.filter(|s| !s.trim().is_empty()) {
            q.push(("status", st.to_string()));
        }
        if let Some(l) = limit {
            q.push(("limit", l.to_string()));
        }
        if let Some(o) = offset {
            q.push(("offset", o.to_string()));
        }
        let base = format!(
            "/api/v1/orca/bots/{}/items",
            percent_encode_rfc3986(bot_id)
        );
        let path = build_query_path(&base, &q);
        self.send_json(Method::GET, &path, None::<&()>).await
    }

    pub async fn create_item(
        &self,
        bot_id: &str,
        req: &OrcaCreateItemRequest,
    ) -> Result<Value, OrcaClientError> {
        let path = format!(
            "/api/v1/orca/bots/{}/items",
            percent_encode_rfc3986(bot_id)
        );
        self.send_json(Method::POST, &path, Some(req)).await
    }

    pub async fn get_item(&self, item_num_or_uid: &str) -> Result<Value, OrcaClientError> {
        let path = format!(
            "/api/v1/orca/items/{}",
            percent_encode_rfc3986(item_num_or_uid)
        );
        self.send_json(Method::GET, &path, None::<&()>).await
    }

    pub async fn patch_item(
        &self,
        item_num_or_uid: &str,
        req: &OrcaUpdateItemRequest,
    ) -> Result<Value, OrcaClientError> {
        let path = format!(
            "/api/v1/orca/items/{}",
            percent_encode_rfc3986(item_num_or_uid)
        );
        self.send_json(Method::PATCH, &path, Some(req)).await
    }

    pub async fn comment_item(
        &self,
        item_num_or_uid: &str,
        req: &OrcaItemCommentRequest,
    ) -> Result<Value, OrcaClientError> {
        let path = format!(
            "/api/v1/orca/items/{}/comment",
            percent_encode_rfc3986(item_num_or_uid)
        );
        self.send_json(Method::POST, &path, Some(req)).await
    }

    pub async fn snooze_item(
        &self,
        item_num_or_uid: &str,
        req: &OrcaItemSnoozeRequest,
    ) -> Result<Value, OrcaClientError> {
        let path = format!(
            "/api/v1/orca/items/{}/snooze",
            percent_encode_rfc3986(item_num_or_uid)
        );
        self.send_json(Method::POST, &path, Some(req)).await
    }

    pub async fn needs_you(
        &self,
        bot_id: Option<&str>,
        focused_uid: Option<&str>,
        cap: Option<usize>,
    ) -> Result<Value, OrcaClientError> {
        let mut q = Vec::new();
        if let Some(b) = bot_id.filter(|s| !s.trim().is_empty()) {
            q.push(("bot_id", b.to_string()));
        }
        if let Some(f) = focused_uid.filter(|s| !s.trim().is_empty()) {
            q.push(("focused_uid", f.to_string()));
        }
        if let Some(c) = cap {
            q.push(("cap", c.to_string()));
        }
        let path = build_query_path("/api/v1/orca/needs-you", &q);
        self.send_json(Method::GET, &path, None::<&()>).await
    }

    pub async fn get_look_around(&self, bot_id: &str) -> Result<Value, OrcaClientError> {
        let path = format!(
            "/api/v1/orca/bots/{}/look-around",
            percent_encode_rfc3986(bot_id)
        );
        self.send_json(Method::GET, &path, None::<&()>).await
    }

    pub async fn post_look_around(
        &self,
        bot_id: &str,
        req: &OrcaLookAroundRequest,
    ) -> Result<Value, OrcaClientError> {
        let path = format!(
            "/api/v1/orca/bots/{}/look-around",
            percent_encode_rfc3986(bot_id)
        );
        self.send_json(Method::POST, &path, Some(req)).await
    }

    pub async fn get_up_next(&self, bot_id: &str) -> Result<Value, OrcaClientError> {
        let path = format!(
            "/api/v1/orca/bots/{}/up-next",
            percent_encode_rfc3986(bot_id)
        );
        self.send_json(Method::GET, &path, None::<&()>).await
    }

    pub async fn post_up_next(
        &self,
        bot_id: &str,
        req: &OrcaUpNextActionRequest,
    ) -> Result<Value, OrcaClientError> {
        let path = format!(
            "/api/v1/orca/bots/{}/up-next",
            percent_encode_rfc3986(bot_id)
        );
        self.send_json(Method::POST, &path, Some(req)).await
    }

    // -----------------------------------------------------------------------
    // 5. Groups, Access, Hand-Over, Chat Tree & Bot Chats/Reply
    // -----------------------------------------------------------------------

    pub async fn list_groups(&self) -> Result<Vec<BotGroup>, OrcaClientError> {
        self.send_json(Method::GET, "/api/v1/orca/groups", None::<&()>)
            .await
    }

    pub async fn create_group(
        &self,
        req: &OrcaCreateGroupRequest,
    ) -> Result<BotGroup, OrcaClientError> {
        self.send_json(Method::POST, "/api/v1/orca/groups", Some(req))
            .await
    }

    pub async fn get_group(&self, group_id: &str) -> Result<BotGroup, OrcaClientError> {
        let path = format!("/api/v1/orca/groups/{}", percent_encode_rfc3986(group_id));
        self.send_json(Method::GET, &path, None::<&()>).await
    }

    pub async fn create_group_thread(
        &self,
        group_id: &str,
        req: &OrcaCreateGroupThreadRequest,
    ) -> Result<Value, OrcaClientError> {
        let path = format!(
            "/api/v1/orca/groups/{}/threads",
            percent_encode_rfc3986(group_id)
        );
        self.send_json(Method::POST, &path, Some(req)).await
    }

    pub async fn send_group_message(
        &self,
        group_id: &str,
        req: &OrcaSendGroupMessageRequest,
    ) -> Result<Value, OrcaClientError> {
        let path = format!(
            "/api/v1/orca/groups/{}/send",
            percent_encode_rfc3986(group_id)
        );
        self.send_json(Method::POST, &path, Some(req)).await
    }

    pub async fn list_group_messages(
        &self,
        group_id: &str,
        thread_id: Option<&str>,
    ) -> Result<Value, OrcaClientError> {
        let mut q = Vec::new();
        if let Some(tid) = thread_id.filter(|s| !s.trim().is_empty()) {
            q.push(("thread_id", tid.to_string()));
        }
        let base = format!(
            "/api/v1/orca/groups/{}/messages",
            percent_encode_rfc3986(group_id)
        );
        let path = build_query_path(&base, &q);
        self.send_json(Method::GET, &path, None::<&()>).await
    }

    pub async fn list_access(
        &self,
        bot_id: Option<&str>,
        status: Option<&str>,
    ) -> Result<Value, OrcaClientError> {
        let mut q = Vec::new();
        if let Some(st) = status.filter(|s| !s.trim().is_empty()) {
            q.push(("status", st.to_string()));
        }
        let base = match bot_id.filter(|s| !s.trim().is_empty()) {
            Some(bid) => format!("/api/v1/orca/bots/{}/access", percent_encode_rfc3986(bid)),
            None => "/api/v1/orca/access".to_string(),
        };
        let path = build_query_path(&base, &q);
        self.send_json(Method::GET, &path, None::<&()>).await
    }

    pub async fn create_access_request(
        &self,
        bot_id: &str,
        req: &OrcaCreateAccessRequest,
    ) -> Result<Value, OrcaClientError> {
        let path = format!(
            "/api/v1/orca/bots/{}/access",
            percent_encode_rfc3986(bot_id)
        );
        self.send_json(Method::POST, &path, Some(req)).await
    }

    pub async fn decide_access(
        &self,
        bot_id: &str,
        req: &OrcaDecideAccessRequest,
    ) -> Result<Value, OrcaClientError> {
        let path = format!(
            "/api/v1/orca/bots/{}/access/decide",
            percent_encode_rfc3986(bot_id)
        );
        self.send_json(Method::POST, &path, Some(req)).await
    }

    pub async fn hand_over(
        &self,
        bot_id: &str,
        req: &OrcaHandOverRequest,
    ) -> Result<Value, OrcaClientError> {
        let path = format!(
            "/api/v1/orca/bots/{}/hand-over",
            percent_encode_rfc3986(bot_id)
        );
        self.send_json(Method::POST, &path, Some(req)).await
    }

    pub async fn chat_tree(
        &self,
        bot_id: &str,
        req: Option<&OrcaChatTreeRequest>,
    ) -> Result<Value, OrcaClientError> {
        let path = format!(
            "/api/v1/orca/bots/{}/chat-tree",
            percent_encode_rfc3986(bot_id)
        );
        match req {
            Some(body) => self.send_json(Method::POST, &path, Some(body)).await,
            None => self.send_json(Method::GET, &path, None::<&()>).await,
        }
    }

    pub async fn list_bot_chats(&self, bot_id: &str) -> Result<Value, OrcaClientError> {
        let path = format!(
            "/api/v1/orca/bots/{}/chats",
            percent_encode_rfc3986(bot_id)
        );
        self.send_json(Method::GET, &path, None::<&()>).await
    }

    pub async fn create_bot_chat(
        &self,
        bot_id: &str,
        req: &OrcaCreateBotChatRequest,
    ) -> Result<Value, OrcaClientError> {
        let path = format!(
            "/api/v1/orca/bots/{}/chats",
            percent_encode_rfc3986(bot_id)
        );
        self.send_json(Method::POST, &path, Some(req)).await
    }

    pub async fn bot_reply(
        &self,
        bot_id: &str,
        req: &OrcaBotChatReplyRequest,
    ) -> Result<Value, OrcaClientError> {
        let path = format!(
            "/api/v1/orca/bots/{}/reply",
            percent_encode_rfc3986(bot_id)
        );
        self.send_json(Method::POST, &path, Some(req)).await
    }

    // -----------------------------------------------------------------------
    // 6. Packages, E2EE Team Share, Import, Tool Relay & Telegram Fusion
    // -----------------------------------------------------------------------

    pub async fn package_export(
        &self,
        bot_id: &str,
        req: &OrcaPackageExportRequest,
    ) -> Result<Value, OrcaClientError> {
        let path = format!(
            "/api/v1/orca/bots/{}/package/export",
            percent_encode_rfc3986(bot_id)
        );
        self.send_json(Method::POST, &path, Some(req)).await
    }

    pub async fn package_import(
        &self,
        req: &OrcaPackageImportRequest,
    ) -> Result<Value, OrcaClientError> {
        self.send_json(Method::POST, "/api/v1/orca/bots/package/import", Some(req))
            .await
    }

    pub async fn share_identity(&self) -> Result<TeamSharePublicIdentity, OrcaClientError> {
        self.send_json(Method::GET, "/api/v1/orca/bots/share/identity", None::<&()>)
            .await
    }

    pub async fn share_seal(
        &self,
        bot_id: &str,
        req: &OrcaShareSealRequest,
    ) -> Result<Value, OrcaClientError> {
        let path = format!(
            "/api/v1/orca/bots/{}/share/seal",
            percent_encode_rfc3986(bot_id)
        );
        self.send_json(Method::POST, &path, Some(req)).await
    }

    pub async fn share_open(
        &self,
        req: &OrcaShareOpenRequest,
    ) -> Result<Value, OrcaClientError> {
        self.send_json(Method::POST, "/api/v1/orca/bots/share/open", Some(req))
            .await
    }

    pub async fn import_discover(&self) -> Result<Value, OrcaClientError> {
        self.send_json(
            Method::GET,
            "/api/v1/orca/bots/import/discover",
            None::<&()>,
        )
        .await
    }

    pub async fn import_discover_post(
        &self,
        req: &OrcaImportDiscoverRequest,
    ) -> Result<Value, OrcaClientError> {
        self.send_json(Method::POST, "/api/v1/orca/bots/import/discover", Some(req))
            .await
    }

    pub async fn relay_start(&self) -> Result<OrcaRelayStartResponse, OrcaClientError> {
        self.send_json(Method::POST, "/api/v1/orca/relay/start", None::<&()>)
            .await
    }

    pub async fn relay_status(&self) -> Result<OrcaRelayStatusResponse, OrcaClientError> {
        self.send_json(Method::GET, "/api/v1/orca/relay/status", None::<&()>)
            .await
    }

    pub async fn relay_register_tool(
        &self,
        req: &OrcaRelayRegisterToolRequest,
    ) -> Result<Value, OrcaClientError> {
        self.send_json(Method::POST, "/api/v1/orca/relay/tools", Some(req))
            .await
    }

    pub async fn relay_register_grant(
        &self,
        req: &OrcaRelayRegisterGrantRequest,
    ) -> Result<Value, OrcaClientError> {
        self.send_json(Method::POST, "/api/v1/orca/relay/grants", Some(req))
            .await
    }

    pub async fn relay_revoke(
        &self,
        req: &OrcaRelayRevokeRequest,
    ) -> Result<Value, OrcaClientError> {
        self.send_json(Method::POST, "/api/v1/orca/relay/revoke", Some(req))
            .await
    }

    pub async fn telegram_link_get(&self, bot_id: &str) -> Result<Value, OrcaClientError> {
        let path = format!(
            "/api/v1/orca/bots/{}/telegram/link",
            percent_encode_rfc3986(bot_id)
        );
        self.send_json(Method::GET, &path, None::<&()>).await
    }

    pub async fn telegram_link(
        &self,
        bot_id: &str,
        req: &OrcaTelegramLinkRequest,
    ) -> Result<Value, OrcaClientError> {
        let path = format!(
            "/api/v1/orca/bots/{}/telegram/link",
            percent_encode_rfc3986(bot_id)
        );
        self.send_json(Method::POST, &path, Some(req)).await
    }

    pub async fn telegram_post_card(
        &self,
        bot_id: &str,
        req: &OrcaTelegramCardPostRequest,
    ) -> Result<Value, OrcaClientError> {
        let path = format!(
            "/api/v1/orca/bots/{}/telegram/card",
            percent_encode_rfc3986(bot_id)
        );
        self.send_json(Method::POST, &path, Some(req)).await
    }

    // -----------------------------------------------------------------------
    // 7. Webhooks & Automations / Watch-PR
    // -----------------------------------------------------------------------

    pub async fn list_webhooks(
        &self,
        bot_id: Option<&str>,
    ) -> Result<Value, OrcaClientError> {
        let mut q = Vec::new();
        if let Some(b) = bot_id.filter(|s| !s.trim().is_empty()) {
            q.push(("bot_id", b.to_string()));
        }
        let path = build_query_path("/api/v1/orca/webhooks", &q);
        self.send_json(Method::GET, &path, None::<&()>).await
    }

    pub async fn create_webhook(
        &self,
        req: &OrcaWebhookCreateRequest,
    ) -> Result<Value, OrcaClientError> {
        self.send_json(Method::POST, "/api/v1/orca/webhooks", Some(req))
            .await
    }

    pub async fn show_webhook(&self, hook_id: &str) -> Result<Value, OrcaClientError> {
        let path = format!(
            "/api/v1/orca/webhooks/{}",
            percent_encode_rfc3986(hook_id)
        );
        self.send_json(Method::GET, &path, None::<&()>).await
    }

    pub async fn patch_webhook(
        &self,
        hook_id: &str,
        req: &OrcaWebhookPatchRequest,
    ) -> Result<Value, OrcaClientError> {
        let path = format!(
            "/api/v1/orca/webhooks/{}",
            percent_encode_rfc3986(hook_id)
        );
        self.send_json(Method::PATCH, &path, Some(req)).await
    }

    pub async fn rotate_webhook_secret(&self, hook_id: &str) -> Result<Value, OrcaClientError> {
        let path = format!(
            "/api/v1/orca/webhooks/{}/rotate-secret",
            percent_encode_rfc3986(hook_id)
        );
        self.send_json(Method::POST, &path, None::<&()>).await
    }

    pub async fn webhook_calls(
        &self,
        hook_id: &str,
        call_id: Option<&str>,
    ) -> Result<Value, OrcaClientError> {
        let mut q = Vec::new();
        if let Some(cid) = call_id.filter(|s| !s.trim().is_empty()) {
            q.push(("call_id", cid.to_string()));
        }
        let base = format!(
            "/api/v1/orca/webhooks/{}/calls",
            percent_encode_rfc3986(hook_id)
        );
        let path = build_query_path(&base, &q);
        self.send_json(Method::GET, &path, None::<&()>).await
    }

    pub async fn list_automations(
        &self,
        bot_id: Option<&str>,
        local_only: bool,
    ) -> Result<Value, OrcaClientError> {
        let mut q = Vec::new();
        if let Some(b) = bot_id.filter(|s| !s.trim().is_empty()) {
            q.push(("bot_id", b.to_string()));
        }
        if local_only {
            q.push(("local_only", "true".to_string()));
        }
        let path = build_query_path("/api/v1/orca/automations", &q);
        self.send_json(Method::GET, &path, None::<&()>).await
    }

    pub async fn create_automation(
        &self,
        req: &OrcaAutomationCreateRequest,
    ) -> Result<Value, OrcaClientError> {
        self.send_json(Method::POST, "/api/v1/orca/automations", Some(req))
            .await
    }

    pub async fn patch_automation(
        &self,
        id: &str,
        req: &OrcaAutomationPatchRequest,
    ) -> Result<Value, OrcaClientError> {
        let path = format!("/api/v1/orca/automations/{}", percent_encode_rfc3986(id));
        self.send_json(Method::PATCH, &path, Some(req)).await
    }

    pub async fn remove_automation(&self, id: &str) -> Result<Value, OrcaClientError> {
        let path = format!("/api/v1/orca/automations/{}", percent_encode_rfc3986(id));
        self.send_json(Method::DELETE, &path, None::<&()>).await
    }

    pub async fn watch_pr(&self, req: &OrcaWatchPrRequest) -> Result<Value, OrcaClientError> {
        self.send_json(Method::POST, "/api/v1/orca/automations/watch-pr", Some(req))
            .await
    }

    pub async fn watch_pr_get(&self, watch_id: &str) -> Result<Value, OrcaClientError> {
        let path = format!(
            "/api/v1/orca/automations/watch-pr/{}",
            percent_encode_rfc3986(watch_id)
        );
        self.send_json(Method::GET, &path, None::<&()>).await
    }

    pub async fn watch_pr_wake(
        &self,
        watch_id: &str,
        body: &Value,
    ) -> Result<Value, OrcaClientError> {
        let path = format!(
            "/api/v1/orca/automations/watch-pr/{}/wake",
            percent_encode_rfc3986(watch_id)
        );
        self.send_json(Method::POST, &path, Some(body)).await
    }

    // -----------------------------------------------------------------------
    // 8. Multi-Agent Orchestration & Federation
    // -----------------------------------------------------------------------

    pub async fn create_run(&self, body: &Value) -> Result<Value, OrcaClientError> {
        self.send_json(Method::POST, "/api/v1/orca/orchestration/runs", Some(body))
            .await
    }

    pub async fn list_runs(&self, status: Option<&str>) -> Result<Value, OrcaClientError> {
        let mut q = Vec::new();
        if let Some(st) = status.filter(|s| !s.trim().is_empty()) {
            q.push(("status", st.to_string()));
        }
        let path = build_query_path("/api/v1/orca/orchestration/runs", &q);
        self.send_json(Method::GET, &path, None::<&()>).await
    }

    pub async fn get_run(&self, run_id: &str) -> Result<Value, OrcaClientError> {
        let path = format!(
            "/api/v1/orca/orchestration/runs/{}",
            percent_encode_rfc3986(run_id)
        );
        self.send_json(Method::GET, &path, None::<&()>).await
    }

    pub async fn create_task(&self, body: &Value) -> Result<Value, OrcaClientError> {
        self.send_json(Method::POST, "/api/v1/orca/orchestration/tasks", Some(body))
            .await
    }

    pub async fn transition_task(
        &self,
        task_id: &str,
        body: &Value,
    ) -> Result<Value, OrcaClientError> {
        let path = format!(
            "/api/v1/orca/orchestration/tasks/{}",
            percent_encode_rfc3986(task_id)
        );
        self.send_json(Method::PATCH, &path, Some(body)).await
    }

    pub async fn start_worker(&self, body: &Value) -> Result<Value, OrcaClientError> {
        self.send_json(
            Method::POST,
            "/api/v1/orca/orchestration/workers/start",
            Some(body),
        )
        .await
    }

    pub async fn complete_worker(&self, body: &Value) -> Result<Value, OrcaClientError> {
        self.send_json(
            Method::POST,
            "/api/v1/orca/orchestration/workers/done",
            Some(body),
        )
        .await
    }

    pub async fn read_worker_archive(&self, dispatch_id: &str) -> Result<Value, OrcaClientError> {
        let path = format!(
            "/api/v1/orca/orchestration/workers/{}/archive",
            percent_encode_rfc3986(dispatch_id)
        );
        self.send_json(Method::GET, &path, None::<&()>).await
    }

    pub async fn release_worker(
        &self,
        dispatch_id: &str,
        now_ms: Option<i64>,
    ) -> Result<Value, OrcaClientError> {
        let path = format!(
            "/api/v1/orca/orchestration/workers/{}/release",
            percent_encode_rfc3986(dispatch_id)
        );
        let body = match now_ms {
            Some(ms) => json!({ "now_ms": ms }),
            None => json!({}),
        };
        self.send_json(Method::POST, &path, Some(&body)).await
    }

    pub async fn send_orchestration_message(
        &self,
        body: &Value,
    ) -> Result<Value, OrcaClientError> {
        self.send_json(
            Method::POST,
            "/api/v1/orca/orchestration/messages",
            Some(body),
        )
        .await
    }

    pub async fn check_mailbox(
        &self,
        recipient_id: &str,
        dispatch_id: Option<&str>,
        expected_generation: Option<u64>,
        now_ms: Option<i64>,
    ) -> Result<Value, OrcaClientError> {
        let mut q = vec![("recipient_id", recipient_id.to_string())];
        if let Some(d) = dispatch_id.filter(|s| !s.trim().is_empty()) {
            q.push(("dispatch_id", d.to_string()));
        }
        if let Some(g) = expected_generation {
            q.push(("expected_generation", g.to_string()));
        }
        if let Some(ms) = now_ms {
            q.push(("now_ms", ms.to_string()));
        }
        let path = build_query_path("/api/v1/orca/orchestration/mailbox", &q);
        self.send_json(Method::GET, &path, None::<&()>).await
    }

    pub async fn ask_question(&self, body: &Value) -> Result<Value, OrcaClientError> {
        self.send_json(
            Method::POST,
            "/api/v1/orca/orchestration/questions",
            Some(body),
        )
        .await
    }

    pub async fn create_gate(&self, body: &Value) -> Result<Value, OrcaClientError> {
        self.send_json(Method::POST, "/api/v1/orca/orchestration/gates", Some(body))
            .await
    }

    pub async fn resolve_gate(
        &self,
        gate_id: &str,
        body: &Value,
    ) -> Result<Value, OrcaClientError> {
        let path = format!(
            "/api/v1/orca/orchestration/gates/{}/resolve",
            percent_encode_rfc3986(gate_id)
        );
        self.send_json(Method::POST, &path, Some(body)).await
    }

    pub async fn list_hosts(&self) -> Result<Value, OrcaClientError> {
        self.send_json(
            Method::GET,
            "/api/v1/orca/orchestration/hosts",
            None::<&()>,
        )
        .await
    }

    pub async fn register_host(&self, body: &Value) -> Result<Value, OrcaClientError> {
        self.send_json(Method::POST, "/api/v1/orca/orchestration/hosts", Some(body))
            .await
    }

    pub async fn select_host(&self, body: &Value) -> Result<Value, OrcaClientError> {
        self.send_json(
            Method::POST,
            "/api/v1/orca/orchestration/hosts/select",
            Some(body),
        )
        .await
    }

    // -----------------------------------------------------------------------
    // 9. Agent IDE (`orca.yaml`, Worktrees, Diffs, Vault, Plugins, Artifacts,
    //    ComputerUse, Usage, Hibernation, E2EE v2 & ProfileState)
    // -----------------------------------------------------------------------

    pub async fn parse_orca_yaml(&self, yaml: &str) -> Result<OrcaYamlConfig, OrcaClientError> {
        let body = json!({ "yaml": yaml });
        self.send_json(Method::POST, "/api/v1/orca/ide/orca-yaml/parse", Some(&body))
            .await
    }

    pub async fn list_worktrees(&self) -> Result<Value, OrcaClientError> {
        self.send_json(Method::GET, "/api/v1/orca/ide/worktrees", None::<&()>)
            .await
    }

    pub async fn create_worktree(&self, body: &Value) -> Result<Value, OrcaClientError> {
        self.send_json(Method::POST, "/api/v1/orca/ide/worktrees", Some(body))
            .await
    }

    pub async fn set_worktree_checkpoint(
        &self,
        worktree_id: &str,
        body: &Value,
    ) -> Result<Value, OrcaClientError> {
        let path = format!(
            "/api/v1/orca/ide/worktrees/{}",
            percent_encode_rfc3986(worktree_id)
        );
        self.send_json(Method::PATCH, &path, Some(body)).await
    }

    pub async fn worktree_include_copy(&self, body: &Value) -> Result<Value, OrcaClientError> {
        self.send_json(
            Method::POST,
            "/api/v1/orca/ide/worktrees/include-copy",
            Some(body),
        )
        .await
    }

    pub async fn worktree_ports(&self) -> Result<Value, OrcaClientError> {
        self.send_json(Method::GET, "/api/v1/orca/ide/worktrees/ports", None::<&()>)
            .await
    }

    pub async fn worktree_cleanup(&self, body: &Value) -> Result<Value, OrcaClientError> {
        self.send_json(
            Method::POST,
            "/api/v1/orca/ide/worktrees/cleanup",
            Some(body),
        )
        .await
    }

    pub async fn list_diff_annotations(
        &self,
        file_path: Option<&str>,
        include_resolved: bool,
    ) -> Result<Vec<DiffAnnotation>, OrcaClientError> {
        let mut q = Vec::new();
        if let Some(fp) = file_path.filter(|s| !s.trim().is_empty()) {
            q.push(("file_path", fp.to_string()));
        }
        if include_resolved {
            q.push(("include_resolved", "true".to_string()));
        }
        let path = build_query_path("/api/v1/orca/ide/diff-annotations", &q);
        self.send_json(Method::GET, &path, None::<&()>).await
    }

    pub async fn diff_annotations_batch(
        &self,
        file_path: Option<&str>,
    ) -> Result<Value, OrcaClientError> {
        let mut q = Vec::new();
        if let Some(fp) = file_path.filter(|s| !s.trim().is_empty()) {
            q.push(("file_path", fp.to_string()));
        }
        let path = build_query_path("/api/v1/orca/ide/diff-annotations/batch", &q);
        self.send_json(Method::GET, &path, None::<&()>).await
    }

    pub async fn add_diff_annotation(
        &self,
        body: &Value,
    ) -> Result<DiffAnnotation, OrcaClientError> {
        self.send_json(Method::POST, "/api/v1/orca/ide/diff-annotations", Some(body))
            .await
    }

    pub async fn vault_search(
        &self,
        query: &str,
        workspace_id: Option<&str>,
        agent_id: Option<&str>,
        limit: Option<usize>,
    ) -> Result<Value, OrcaClientError> {
        let mut q = vec![("q", query.to_string())];
        if let Some(ws) = workspace_id.filter(|s| !s.trim().is_empty()) {
            q.push(("workspace_id", ws.to_string()));
        }
        if let Some(ag) = agent_id.filter(|s| !s.trim().is_empty()) {
            q.push(("agent_id", ag.to_string()));
        }
        if let Some(l) = limit {
            q.push(("limit", l.to_string()));
        }
        let path = build_query_path("/api/v1/orca/ide/vault/search", &q);
        self.send_json(Method::GET, &path, None::<&()>).await
    }

    pub async fn vault_index(&self, body: &Value) -> Result<Value, OrcaClientError> {
        self.send_json(Method::POST, "/api/v1/orca/ide/vault/index", Some(body))
            .await
    }

    pub async fn list_plugins(&self) -> Result<Value, OrcaClientError> {
        self.send_json(Method::GET, "/api/v1/orca/ide/plugins", None::<&()>)
            .await
    }

    pub async fn plugin_action(&self, body: &Value) -> Result<Value, OrcaClientError> {
        self.send_json(Method::POST, "/api/v1/orca/ide/plugins", Some(body))
            .await
    }

    pub async fn list_artifacts(
        &self,
        profile_id: Option<&str>,
        artifact_id: Option<&str>,
    ) -> Result<Value, OrcaClientError> {
        let mut q = Vec::new();
        if let Some(pid) = profile_id.filter(|s| !s.trim().is_empty()) {
            q.push(("profile_id", pid.to_string()));
        }
        if let Some(aid) = artifact_id.filter(|s| !s.trim().is_empty()) {
            q.push(("artifact_id", aid.to_string()));
        }
        let path = build_query_path("/api/v1/orca/ide/artifacts", &q);
        self.send_json(Method::GET, &path, None::<&()>).await
    }

    pub async fn publish_artifact(&self, body: &Value) -> Result<Value, OrcaClientError> {
        self.send_json(Method::POST, "/api/v1/orca/ide/artifacts", Some(body))
            .await
    }

    pub async fn computer_guard(
        &self,
        req: &ComputerUseRequest,
        policy: Option<&ComputerUsePolicy>,
    ) -> Result<ComputerUseGuardDecision, OrcaClientError> {
        let body = match policy {
            Some(p) => json!({ "request": req, "policy": p }),
            None => serde_json::to_value(req)?,
        };
        self.send_json(Method::POST, "/api/v1/orca/ide/computer/guard", Some(&body))
            .await
    }

    pub async fn usage_roster(&self, now_ms: Option<i64>) -> Result<Value, OrcaClientError> {
        let mut q = Vec::new();
        if let Some(ms) = now_ms {
            q.push(("now_ms", ms.to_string()));
        }
        let path = build_query_path("/api/v1/orca/ide/usage", &q);
        self.send_json(Method::GET, &path, None::<&()>).await
    }

    pub async fn hibernation_check(
        &self,
        body: &Value,
    ) -> Result<HibernationDecision, OrcaClientError> {
        self.send_json(
            Method::POST,
            "/api/v1/orca/ide/hibernation/check",
            Some(body),
        )
        .await
    }

    pub async fn e2ee_handshake(&self, body: &Value) -> Result<Value, OrcaClientError> {
        self.send_json(Method::POST, "/api/v1/orca/ide/e2ee/handshake", Some(body))
            .await
    }

    pub async fn get_profile_state(&self, domain: Option<&str>) -> Result<Value, OrcaClientError> {
        let mut q = Vec::new();
        if let Some(d) = domain.filter(|s| !s.trim().is_empty()) {
            q.push(("domain", d.to_string()));
        }
        let path = build_query_path("/api/v1/orca/ide/profile-state", &q);
        self.send_json(Method::GET, &path, None::<&()>).await
    }

    pub async fn put_profile_state(&self, body: &Value) -> Result<Value, OrcaClientError> {
        self.send_json(Method::PUT, "/api/v1/orca/ide/profile-state", Some(body))
            .await
    }
}
