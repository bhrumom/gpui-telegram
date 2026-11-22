use crate::error::CliError;
use serde_json::{json, Value};

pub const DEFAULT_OFFICIAL_MCP_URL: &str = "https://mcp.ombhrum.com/mcp";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum McpClientTarget {
    Antigravity,
    Claude,
    Cursor,
    VsCode,
    All,
}

impl McpClientTarget {
    pub fn parse(raw: &str) -> Result<Self, CliError> {
        match raw.trim().to_ascii_lowercase().as_str() {
            "antigravity" | "agy" => Ok(Self::Antigravity),
            "claude" | "claude-desktop" | "claude-code" => Ok(Self::Claude),
            "cursor" => Ok(Self::Cursor),
            "vscode" | "vs-code" | "code" => Ok(Self::VsCode),
            "all" => Ok(Self::All),
            other => Err(CliError::Validation(format!(
                "Unknown MCP client '{other}' (expected antigravity, claude, cursor, vscode, or all)"
            ))),
        }
    }

    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Antigravity => "antigravity",
            Self::Claude => "claude",
            Self::Cursor => "cursor",
            Self::VsCode => "vscode",
            Self::All => "all",
        }
    }
}

pub fn normalize_mcp_url(raw: Option<&str>, server_url: &str, server_explicit: bool) -> String {
    if let Some(explicit) = raw.map(str::trim).filter(|s| !s.is_empty()) {
        let trimmed = explicit.trim_end_matches('/');
        if trimmed.ends_with("/mcp") {
            return trimmed.to_string();
        }
        return format!("{trimmed}/mcp");
    }
    if server_explicit {
        let base = server_url.trim_end_matches('/');
        if base.ends_with("/mcp") {
            return base.to_string();
        }
        return format!("{base}/mcp");
    }
    if let Ok(env_url) = std::env::var("FABUSHI_MCP_URL") {
        let trimmed = env_url.trim().trim_end_matches('/');
        if !trimmed.is_empty() {
            if trimmed.ends_with("/mcp") {
                return trimmed.to_string();
            }
            return format!("{trimmed}/mcp");
        }
    }
    DEFAULT_OFFICIAL_MCP_URL.to_string()
}

pub fn build_client_config(client: McpClientTarget, mcp_url: &str) -> Value {
    match client {
        McpClientTarget::Antigravity => json!({
            "client": "antigravity",
            "config_path": "~/.gemini/antigravity/mcp_config.json",
            "auth": "OAuth 2.1 (RFC 9728 + PKCE S256)",
            "config": {
                "mcpServers": {
                    "fabushi": {
                        "serverUrl": mcp_url
                    }
                }
            }
        }),
        McpClientTarget::Claude => json!({
            "client": "claude",
            "config_path": "~/.claude.json",
            "auth": "OAuth 2.1 (RFC 9728 + PKCE S256)",
            "config": {
                "mcpServers": {
                    "fabushi": {
                        "type": "http",
                        "url": mcp_url
                    }
                }
            }
        }),
        McpClientTarget::Cursor => json!({
            "client": "cursor",
            "config_path": "~/.cursor/mcp.json",
            "auth": "OAuth 2.1 (RFC 9728 + PKCE S256)",
            "config": {
                "mcpServers": {
                    "fabushi": {
                        "url": mcp_url
                    }
                }
            }
        }),
        McpClientTarget::VsCode => json!({
            "client": "vscode",
            "config_path": ".vscode/mcp.json",
            "auth": "OAuth 2.1 (RFC 9728 + PKCE S256)",
            "config": {
                "servers": {
                    "fabushi": {
                        "type": "http",
                        "url": mcp_url
                    }
                }
            }
        }),
        McpClientTarget::All => json!({
            "mcp_url": mcp_url,
            "auth": "OAuth 2.1 (RFC 9728 + PKCE S256)",
            "clients": {
                "antigravity": build_client_config(McpClientTarget::Antigravity, mcp_url),
                "claude": build_client_config(McpClientTarget::Claude, mcp_url),
                "cursor": build_client_config(McpClientTarget::Cursor, mcp_url),
                "vscode": build_client_config(McpClientTarget::VsCode, mcp_url),
            }
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_mcp_client_config_generation() {
        let url = normalize_mcp_url(Some("https://mcp.ombhrum.com"), "http://127.0.0.1:18080", false);
        assert_eq!(url, "https://mcp.ombhrum.com/mcp");

        let agy = build_client_config(McpClientTarget::Antigravity, &url);
        assert_eq!(agy["config"]["mcpServers"]["fabushi"]["serverUrl"], url);

        let claude = build_client_config(McpClientTarget::Claude, &url);
        assert_eq!(claude["config"]["mcpServers"]["fabushi"]["type"], "http");
        assert_eq!(claude["config"]["mcpServers"]["fabushi"]["url"], url);

        let cursor = build_client_config(McpClientTarget::Cursor, &url);
        assert_eq!(cursor["config"]["mcpServers"]["fabushi"]["url"], url);

        let vscode = build_client_config(McpClientTarget::VsCode, &url);
        assert_eq!(vscode["config"]["servers"]["fabushi"]["type"], "http");
        assert_eq!(vscode["config"]["servers"]["fabushi"]["url"], url);

        let all = build_client_config(McpClientTarget::All, &url);
        assert!(all["clients"]["antigravity"].is_object());
        assert!(all["clients"]["claude"].is_object());
        assert!(all["clients"]["cursor"].is_object());
        assert!(all["clients"]["vscode"].is_object());
    }
}
