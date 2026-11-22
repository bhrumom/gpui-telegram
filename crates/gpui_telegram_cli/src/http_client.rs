use crate::error::CliError;
use reqwest::{Client, Method};
use serde_json::{json, Value};
use std::time::Duration;

#[derive(Debug, Clone)]
pub enum ActionHttpOutcome {
    Executed(Value),
    ApprovalRequired {
        gate_id: String,
        expires_at_ms: u64,
        message: String,
    },
}

#[derive(Clone)]
pub struct ActionHttpClient {
    base_url: String,
    client: Client,
}

impl ActionHttpClient {
    pub fn new(base_url: &str) -> Result<Self, CliError> {
        let client = Client::builder()
            .no_proxy()
            .timeout(Duration::from_secs(30))
            .build()
            .map_err(|e| CliError::Http(format!("Failed to initialize HTTP client: {e}")))?;

        Ok(Self {
            base_url: base_url.trim_end_matches('/').to_string(),
            client,
        })
    }

    pub fn base_url(&self) -> &str {
        &self.base_url
    }

    pub async fn request_json(
        &self,
        method: Method,
        path_or_url: &str,
        token: Option<&str>,
        body: Option<&Value>,
        query: &[(&str, String)],
    ) -> Result<Value, CliError> {
        let url = if path_or_url.starts_with("http://") || path_or_url.starts_with("https://") {
            path_or_url.to_string()
        } else {
            format!("{}{}", self.base_url, path_or_url)
        };

        let mut req = self.client.request(method.clone(), &url);
        if !query.is_empty() {
            req = req.query(query);
        }
        if let Some(tok) = token {
            req = req.bearer_auth(tok);
        }
        if let Some(payload) = body {
            req = req.json(payload);
        }

        let resp = req
            .send()
            .await
            .map_err(|e| CliError::Http(format!("{method} {url} failed: {e}")))?;

        let status = resp.status().as_u16();
        let text = resp
            .text()
            .await
            .map_err(|e| CliError::Http(format!("Failed reading response from {url}: {e}")))?;

        let parsed_body: Value = if text.trim().is_empty() {
            json!({})
        } else {
            serde_json::from_str(&text).unwrap_or_else(|_| json!({ "raw": text }))
        };

        if (200..300).contains(&status) {
            return Ok(parsed_body);
        }
        if status == 401 {
            let msg = parsed_body
                .get("message")
                .or_else(|| parsed_body.get("error_description"))
                .and_then(|v| v.as_str())
                .unwrap_or("Authentication required or token rejected")
                .to_string();
            return Err(CliError::Unauthorized(msg));
        }
        if status == 403 {
            let msg = parsed_body
                .get("message")
                .and_then(|v| v.as_str())
                .map(String::from)
                .unwrap_or_else(|| parsed_body.to_string());
            return Err(CliError::Forbidden(msg));
        }
        if status == 404 {
            let msg = parsed_body
                .get("message")
                .and_then(|v| v.as_str())
                .unwrap_or("Resource or endpoint not found")
                .to_string();
            return Err(CliError::ServerError {
                status,
                body: json!({
                    "error": parsed_body.get("error").cloned().unwrap_or_else(|| json!("not_found")),
                    "message": msg,
                }),
            });
        }
        Err(CliError::ServerError {
            status,
            body: parsed_body,
        })
    }

    pub async fn get_json(
        &self,
        path: &str,
        token: Option<&str>,
        query: &[(&str, String)],
    ) -> Result<Value, CliError> {
        self.request_json(Method::GET, path, token, None, query)
            .await
    }

    pub async fn post_json(
        &self,
        path: &str,
        token: Option<&str>,
        body: &Value,
    ) -> Result<Value, CliError> {
        self.request_json(Method::POST, path, token, Some(body), &[])
            .await
    }

    pub async fn put_json(
        &self,
        path: &str,
        token: Option<&str>,
        body: &Value,
    ) -> Result<Value, CliError> {
        self.request_json(Method::PUT, path, token, Some(body), &[])
            .await
    }

    pub async fn delete_json(
        &self,
        path: &str,
        token: Option<&str>,
        query: &[(&str, String)],
    ) -> Result<Value, CliError> {
        self.request_json(Method::DELETE, path, token, None, query)
            .await
    }

    pub async fn list_actions(&self) -> Result<Value, CliError> {
        self.get_json("/api/v1/actions", None, &[]).await
    }

    pub async fn get_action_schema(&self, action_name: &str) -> Result<Value, CliError> {
        let path = format!("/api/v1/actions/{action_name}/schema");
        match self.get_json(&path, None, &[]).await {
            Ok(v) => Ok(v),
            Err(CliError::ServerError { status: 404, body }) => {
                Err(CliError::UnknownCommand(
                    body.get("message")
                        .and_then(|v| v.as_str())
                        .unwrap_or("Action not found")
                        .to_string(),
                ))
            }
            Err(e) => Err(e),
        }
    }

    pub async fn post_action(
        &self,
        action_name: &str,
        token: Option<&str>,
        approval_token: Option<&str>,
        input: &Value,
    ) -> Result<ActionHttpOutcome, CliError> {
        let url = format!("{}/api/v1/actions/{}", self.base_url, action_name);
        let mut req = self.client.post(&url).json(input);
        if let Some(tok) = token {
            req = req.bearer_auth(tok);
        }
        if let Some(app_tok) = approval_token {
            req = req.header("X-Approval-Token", app_tok);
        }

        let resp = req
            .send()
            .await
            .map_err(|e| CliError::Http(format!("POST {url} failed: {e}")))?;

        let status = resp.status().as_u16();
        let body: Value = resp
            .json()
            .await
            .unwrap_or_else(|_| json!({ "error": "invalid_response_body" }));

        if (200..300).contains(&status) {
            return Ok(ActionHttpOutcome::Executed(body));
        }

        if status == 401 {
            let msg = body
                .get("message")
                .and_then(|v| v.as_str())
                .unwrap_or("Authentication required or token rejected")
                .to_string();
            return Err(CliError::Unauthorized(msg));
        }

        if status == 403 {
            if body.get("error").and_then(|v| v.as_str()) == Some("Approval required") {
                if let Some(gate_id) = body.get("gate_id").and_then(|v| v.as_str()) {
                    let expires_at_ms = body
                        .get("expires_at_ms")
                        .and_then(|v| v.as_u64())
                        .unwrap_or(0);
                    let message = body
                        .get("message")
                        .and_then(|v| v.as_str())
                        .unwrap_or("Action requires explicit approval")
                        .to_string();
                    return Ok(ActionHttpOutcome::ApprovalRequired {
                        gate_id: gate_id.to_string(),
                        expires_at_ms,
                        message,
                    });
                }
            }
            return Err(CliError::Forbidden(body.to_string()));
        }

        Err(CliError::ServerError { status, body })
    }

    pub async fn approve_gate(
        &self,
        gate_id: &str,
        token: &str,
        decision: &str,
        reason: Option<&str>,
    ) -> Result<Value, CliError> {
        let path = format!("/api/v1/actions/approvals/{gate_id}/approve");
        let mut payload = json!({ "decision": decision });
        if let Some(r) = reason {
            payload["reason"] = Value::String(r.to_string());
        }
        self.post_json(&path, Some(token), &payload).await
    }

    pub async fn post_mcp_jsonrpc(
        &self,
        mcp_url: &str,
        token: Option<&str>,
        session_id: Option<&str>,
        rpc_req: &Value,
    ) -> Result<(Option<String>, Value), CliError> {
        let mut req = self
            .client
            .post(mcp_url)
            .header("Accept", "application/json, text/event-stream")
            .header("MCP-Protocol-Version", "2025-11-25")
            .json(rpc_req);
        if let Some(tok) = token {
            req = req.bearer_auth(tok);
        }
        if let Some(sid) = session_id {
            req = req.header("Mcp-Session-Id", sid);
        }

        let resp = req
            .send()
            .await
            .map_err(|e| CliError::Http(format!("POST {mcp_url} failed: {e}")))?;
        let status = resp.status().as_u16();
        let next_sid = resp
            .headers()
            .get("Mcp-Session-Id")
            .or_else(|| resp.headers().get("mcp-session-id"))
            .and_then(|v| v.to_str().ok())
            .map(String::from);
        let text = resp
            .text()
            .await
            .map_err(|e| CliError::Http(format!("Failed reading MCP response: {e}")))?;

        if status == 401 {
            return Err(CliError::Unauthorized(
                "MCP endpoint rejected Bearer token (401 Unauthorized)".into(),
            ));
        }
        if status == 403 {
            return Err(CliError::Forbidden(format!(
                "MCP endpoint denied request (403): {text}"
            )));
        }
        if !(200..300).contains(&status) {
            let body = serde_json::from_str(&text).unwrap_or_else(|_| json!({ "raw": text }));
            return Err(CliError::ServerError { status, body });
        }

        let val: Value = if text.trim_start().starts_with("data:") {
            let data_line = text
                .lines()
                .filter_map(|l| l.strip_prefix("data:"))
                .map(str::trim)
                .filter(|l| !l.is_empty() && *l != "[DONE]")
                .last()
                .unwrap_or("{}");
            serde_json::from_str(data_line).unwrap_or_else(|_| json!({ "raw": text }))
        } else if text.trim().is_empty() {
            json!({})
        } else {
            serde_json::from_str(&text).unwrap_or_else(|_| json!({ "raw": text }))
        };

        Ok((next_sid, val))
    }
}
