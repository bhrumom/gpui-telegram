use crate::db::{ServerDatabase, SqliteResultExt};
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine as _;
use gpui_telegram_core::error::Result;
use rusqlite::{params, OptionalExtension};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::{BTreeSet, HashMap};
use subtle::ConstantTimeEq;

pub const MCP_ACCESS_TOKEN_TTL_MS: i64 = 3_600_000; // 1 hour
pub const MCP_REFRESH_TOKEN_TTL_MS: i64 = 30 * 24 * 3_600_000; // 30 days
pub const MCP_AUTH_CODE_TTL_MS: i64 = 5 * 60_000; // 5 minutes

pub const MIGRATION_V39_MCP_OAUTH: &str = r#"
CREATE TABLE IF NOT EXISTS mcp_oauth_clients (
    client_id TEXT PRIMARY KEY,
    client_name TEXT NOT NULL,
    redirect_uris_json TEXT NOT NULL,
    token_endpoint_auth_method TEXT NOT NULL DEFAULT 'none',
    client_secret_hash TEXT,
    is_cimd INTEGER NOT NULL DEFAULT 0,
    created_at_ms INTEGER NOT NULL
) STRICT;

CREATE TABLE IF NOT EXISTS mcp_oauth_codes (
    code_hash TEXT PRIMARY KEY,
    client_id TEXT NOT NULL,
    user_id INTEGER NOT NULL,
    redirect_uri TEXT NOT NULL,
    resource TEXT NOT NULL DEFAULT '',
    code_challenge TEXT NOT NULL,
    code_challenge_method TEXT NOT NULL DEFAULT 'S256',
    scopes_json TEXT NOT NULL,
    allowed_node_ids_json TEXT NOT NULL DEFAULT '[]',
    issued_family_id TEXT,
    expires_at_ms INTEGER NOT NULL,
    consumed_at_ms INTEGER
) STRICT;
CREATE INDEX IF NOT EXISTS idx_mcp_oauth_codes_expires ON mcp_oauth_codes(expires_at_ms);

CREATE TABLE IF NOT EXISTS mcp_oauth_tokens (
    token_id TEXT PRIMARY KEY,
    family_id TEXT NOT NULL,
    token_kind TEXT NOT NULL,
    token_hash TEXT NOT NULL UNIQUE,
    client_id TEXT NOT NULL,
    user_id INTEGER NOT NULL,
    resource TEXT NOT NULL DEFAULT '',
    scopes_json TEXT NOT NULL,
    allowed_node_ids_json TEXT NOT NULL DEFAULT '[]',
    created_at_ms INTEGER NOT NULL,
    expires_at_ms INTEGER NOT NULL,
    last_used_at_ms INTEGER,
    revoked_at_ms INTEGER
) STRICT;
CREATE INDEX IF NOT EXISTS idx_mcp_oauth_tokens_family ON mcp_oauth_tokens(family_id);
CREATE INDEX IF NOT EXISTS idx_mcp_oauth_tokens_user ON mcp_oauth_tokens(user_id, revoked_at_ms);
CREATE INDEX IF NOT EXISTS idx_mcp_oauth_tokens_hash ON mcp_oauth_tokens(token_hash);

CREATE TABLE IF NOT EXISTS mcp_tool_audit_log (
    audit_id TEXT PRIMARY KEY,
    user_id INTEGER NOT NULL,
    client_id TEXT NOT NULL,
    family_id TEXT NOT NULL DEFAULT '',
    session_id TEXT NOT NULL DEFAULT '',
    tool_name TEXT NOT NULL,
    target_node_id TEXT,
    args_summary TEXT NOT NULL DEFAULT '',
    ok INTEGER NOT NULL,
    error_code TEXT,
    duration_ms INTEGER NOT NULL DEFAULT 0,
    created_at_ms INTEGER NOT NULL
) STRICT;
CREATE INDEX IF NOT EXISTS idx_mcp_tool_audit_user_time ON mcp_tool_audit_log(user_id, created_at_ms DESC);
"#;

pub fn sha256_hex(input: &str) -> String {
    let digest = Sha256::digest(input.as_bytes());
    let mut out = String::with_capacity(64);
    for b in digest {
        use std::fmt::Write;
        let _ = write!(&mut out, "{b:02x}");
    }
    out
}

pub fn ct_str_eq(a: &str, b: &str) -> bool {
    a.as_bytes().len() == b.as_bytes().len()
        && bool::from(a.as_bytes().ct_eq(b.as_bytes()))
}

pub fn random_hex(num_bytes: usize) -> String {
    let mut buf = vec![0u8; num_bytes];
    if getrandom::getrandom(&mut buf).is_err() {
        let u = uuid::Uuid::new_v4();
        let u2 = uuid::Uuid::new_v4();
        let mut combined = Vec::with_capacity(32);
        combined.extend_from_slice(u.as_bytes());
        combined.extend_from_slice(u2.as_bytes());
        buf.copy_from_slice(&combined[..num_bytes.min(32)]);
    }
    let mut out = String::with_capacity(buf.len() * 2);
    for b in buf {
        use std::fmt::Write;
        let _ = write!(&mut out, "{b:02x}");
    }
    out
}

pub fn random_b64url(num_bytes: usize) -> String {
    let mut buf = vec![0u8; num_bytes];
    if getrandom::getrandom(&mut buf).is_err() {
        let u = uuid::Uuid::new_v4();
        let u2 = uuid::Uuid::new_v4();
        let mut combined = Vec::with_capacity(32);
        combined.extend_from_slice(u.as_bytes());
        combined.extend_from_slice(u2.as_bytes());
        buf.copy_from_slice(&combined[..num_bytes.min(32)]);
    }
    URL_SAFE_NO_PAD.encode(buf)
}

pub fn is_valid_pkce_verifier(verifier: &str) -> bool {
    let len = verifier.len();
    (43..=128).contains(&len)
        && verifier
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'.' | b'_' | b'~'))
}

pub fn verify_pkce_s256(verifier: &str, expected_challenge: &str) -> bool {
    if !is_valid_pkce_verifier(verifier) || expected_challenge.is_empty() {
        return false;
    }
    let digest = Sha256::digest(verifier.as_bytes());
    let computed = URL_SAFE_NO_PAD.encode(digest);
    ct_str_eq(&computed, expected_challenge.trim())
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct McpOAuthClientRecord {
    pub client_id: String,
    pub client_name: String,
    pub redirect_uris: Vec<String>,
    pub token_endpoint_auth_method: String,
    pub client_secret_hash: Option<String>,
    pub is_cimd: bool,
    pub created_at_ms: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct McpOAuthCodeRecord {
    pub code_hash: String,
    pub client_id: String,
    pub user_id: u64,
    pub redirect_uri: String,
    pub resource: String,
    pub code_challenge: String,
    pub code_challenge_method: String,
    pub scopes: Vec<String>,
    pub allowed_node_ids: Vec<String>,
    pub issued_family_id: Option<String>,
    pub expires_at_ms: i64,
    pub consumed_at_ms: Option<i64>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct McpIssuedTokenPair {
    pub family_id: String,
    pub access_token: String,
    pub access_token_id: String,
    pub expires_in_secs: i64,
    pub refresh_token: String,
    pub refresh_token_id: String,
    pub scopes: Vec<String>,
    pub allowed_node_ids: Vec<String>,
    pub resource: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct McpVerifiedAccessToken {
    pub token_id: String,
    pub family_id: String,
    pub client_id: String,
    pub client_name: String,
    pub user_id: u64,
    pub resource: String,
    pub scopes: Vec<String>,
    pub allowed_node_ids: Vec<String>,
    pub expires_at_ms: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct McpGrantRecord {
    pub family_id: String,
    pub client_id: String,
    pub client_name: String,
    pub scopes: Vec<String>,
    pub allowed_node_ids: Vec<String>,
    pub created_at_ms: i64,
    pub last_used_ms: Option<i64>,
    pub expires_at_ms: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct McpToolAuditRecord {
    pub audit_id: String,
    pub user_id: u64,
    pub client_id: String,
    pub family_id: String,
    pub session_id: String,
    pub tool_name: String,
    pub target_node_id: Option<String>,
    pub args_summary: String,
    pub ok: bool,
    pub error_code: Option<String>,
    pub duration_ms: i64,
    pub created_at_ms: i64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum McpCodeExchangeOutcome {
    Issued(McpIssuedTokenPair),
    InvalidGrant(String),
    InvalidClient(String),
    InvalidTarget(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum McpRefreshOutcome {
    Rotated(McpIssuedTokenPair),
    InvalidGrant(String),
    InvalidClient(String),
    InvalidScope(String),
    InvalidTarget(String),
}

#[derive(Debug)]
struct TokenRow {
    token_id: String,
    family_id: String,
    token_kind: String,
    token_hash: String,
    client_id: String,
    user_id: u64,
    resource: String,
    scopes: Vec<String>,
    allowed_node_ids: Vec<String>,
    expires_at_ms: i64,
    revoked_at_ms: Option<i64>,
}

fn parse_token_lookup_id(raw_token: &str) -> Option<String> {
    let trimmed = raw_token.trim();
    if let Some(rest) = trimmed.strip_prefix("fbmcp_at_") {
        if let Some((id_hex, secret)) = rest.split_once('.') {
            if !id_hex.is_empty() && !secret.is_empty() {
                return Some(format!("at_{id_hex}"));
            }
        }
    }
    if let Some(rest) = trimmed.strip_prefix("fbmcp_rt_") {
        if let Some((id_hex, secret)) = rest.split_once('.') {
            if !id_hex.is_empty() && !secret.is_empty() {
                return Some(format!("rt_{id_hex}"));
            }
        }
    }
    None
}

fn normalize_resource_str(r: &str) -> String {
    r.trim().trim_end_matches('/').to_string()
}

fn resource_matches(bound: &str, requested: &str) -> bool {
    let b = normalize_resource_str(bound);
    let req = normalize_resource_str(requested);
    if b.is_empty() || req.is_empty() {
        return true;
    }
    b == req
}

impl ServerDatabase {
    pub fn upsert_mcp_oauth_client(&self, client: &McpOAuthClientRecord) -> Result<()> {
        let redirect_uris_json = serde_json::to_string(&client.redirect_uris)?;
        self.pool.with_conn(|conn| {
            conn.execute(
                r#"
                INSERT INTO mcp_oauth_clients (
                    client_id, client_name, redirect_uris_json,
                    token_endpoint_auth_method, client_secret_hash, is_cimd, created_at_ms
                ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)
                ON CONFLICT(client_id) DO UPDATE SET
                    client_name = excluded.client_name,
                    redirect_uris_json = excluded.redirect_uris_json,
                    token_endpoint_auth_method = excluded.token_endpoint_auth_method,
                    client_secret_hash = COALESCE(excluded.client_secret_hash, mcp_oauth_clients.client_secret_hash),
                    is_cimd = excluded.is_cimd
                "#,
                params![
                    client.client_id,
                    client.client_name,
                    redirect_uris_json,
                    client.token_endpoint_auth_method,
                    client.client_secret_hash,
                    if client.is_cimd { 1i64 } else { 0i64 },
                    client.created_at_ms,
                ],
            )
            .map_db()?;
            Ok(())
        })
    }

    pub fn get_mcp_oauth_client(&self, client_id: &str) -> Result<Option<McpOAuthClientRecord>> {
        let trimmed = client_id.trim();
        if trimmed.is_empty() {
            return Ok(None);
        }
        self.pool.with_reader(|conn| {
            conn.query_row(
                r#"
                SELECT client_id, client_name, redirect_uris_json,
                       token_endpoint_auth_method, client_secret_hash, is_cimd, created_at_ms
                FROM mcp_oauth_clients
                WHERE client_id = ?1
                LIMIT 1
                "#,
                params![trimmed],
                |row| {
                    let uris_json: String = row.get(2)?;
                    let redirect_uris: Vec<String> =
                        serde_json::from_str(&uris_json).unwrap_or_default();
                    Ok(McpOAuthClientRecord {
                        client_id: row.get(0)?,
                        client_name: row.get(1)?,
                        redirect_uris,
                        token_endpoint_auth_method: row.get(3)?,
                        client_secret_hash: row.get(4)?,
                        is_cimd: row.get::<_, i64>(5)? != 0,
                        created_at_ms: row.get(6)?,
                    })
                },
            )
            .optional()
            .map_db()
        })
    }

    pub fn create_mcp_oauth_code(&self, rec: &McpOAuthCodeRecord) -> Result<()> {
        let scopes_json = serde_json::to_string(&rec.scopes)?;
        let allowed_json = serde_json::to_string(&rec.allowed_node_ids)?;
        self.pool.with_conn(|conn| {
            conn.execute(
                r#"
                INSERT INTO mcp_oauth_codes (
                    code_hash, client_id, user_id, redirect_uri, resource,
                    code_challenge, code_challenge_method, scopes_json,
                    allowed_node_ids_json, issued_family_id, expires_at_ms, consumed_at_ms
                ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)
                "#,
                params![
                    rec.code_hash,
                    rec.client_id,
                    rec.user_id as i64,
                    rec.redirect_uri,
                    normalize_resource_str(&rec.resource),
                    rec.code_challenge,
                    rec.code_challenge_method,
                    scopes_json,
                    allowed_json,
                    rec.issued_family_id,
                    rec.expires_at_ms,
                    rec.consumed_at_ms,
                ],
            )
            .map_db()?;
            Ok(())
        })
    }

    #[allow(clippy::too_many_arguments)]
    pub fn exchange_mcp_oauth_code(
        &self,
        raw_code: &str,
        client_id: &str,
        redirect_uri: &str,
        code_verifier: &str,
        requested_resource: Option<&str>,
        now_ms: i64,
    ) -> Result<McpCodeExchangeOutcome> {
        let candidate_hash = sha256_hex(raw_code.trim());
        self.pool.with_transaction(|tx| {
            let row: Option<McpOAuthCodeRecord> = tx
                .query_row(
                    r#"
                    SELECT code_hash, client_id, user_id, redirect_uri, resource,
                           code_challenge, code_challenge_method, scopes_json,
                           allowed_node_ids_json, issued_family_id, expires_at_ms, consumed_at_ms
                    FROM mcp_oauth_codes
                    WHERE code_hash = ?1
                    LIMIT 1
                    "#,
                    params![&candidate_hash],
                    |r| {
                        let scopes_s: String = r.get(7)?;
                        let allowed_s: String = r.get(8)?;
                        Ok(McpOAuthCodeRecord {
                            code_hash: r.get(0)?,
                            client_id: r.get(1)?,
                            user_id: r.get::<_, i64>(2)?.max(0) as u64,
                            redirect_uri: r.get(3)?,
                            resource: r.get(4)?,
                            code_challenge: r.get(5)?,
                            code_challenge_method: r.get(6)?,
                            scopes: serde_json::from_str(&scopes_s).unwrap_or_default(),
                            allowed_node_ids: serde_json::from_str(&allowed_s).unwrap_or_default(),
                            issued_family_id: r.get(9)?,
                            expires_at_ms: r.get(10)?,
                            consumed_at_ms: r.get(11)?,
                        })
                    },
                )
                .optional()
                .map_db()?;

            let Some(rec) = row else {
                return Ok(McpCodeExchangeOutcome::InvalidGrant(
                    "Unknown authorization code".to_string(),
                ));
            };

            if !ct_str_eq(&candidate_hash, &rec.code_hash) {
                return Ok(McpCodeExchangeOutcome::InvalidGrant(
                    "Invalid authorization code".to_string(),
                ));
            }

            if rec.consumed_at_ms.is_some() {
                if let Some(ref fam) = rec.issued_family_id {
                    tx.execute(
                        "UPDATE mcp_oauth_tokens SET revoked_at_ms = COALESCE(revoked_at_ms, ?1) WHERE family_id = ?2",
                        params![now_ms, fam],
                    )
                    .map_db()?;
                }
                return Ok(McpCodeExchangeOutcome::InvalidGrant(
                    "Authorization code already consumed".to_string(),
                ));
            }

            if rec.client_id != client_id.trim() {
                return Ok(McpCodeExchangeOutcome::InvalidClient(
                    "client_id mismatch on authorization code".to_string(),
                ));
            }

            if rec.redirect_uri != redirect_uri.trim() {
                return Ok(McpCodeExchangeOutcome::InvalidGrant(
                    "redirect_uri mismatch".to_string(),
                ));
            }

            if now_ms > rec.expires_at_ms {
                return Ok(McpCodeExchangeOutcome::InvalidGrant(
                    "Authorization code expired".to_string(),
                ));
            }

            if let Some(req_res) = requested_resource.map(normalize_resource_str).filter(|s| !s.is_empty()) {
                if !rec.resource.is_empty() && !resource_matches(&rec.resource, &req_res) {
                    return Ok(McpCodeExchangeOutcome::InvalidTarget(
                        "resource parameter does not match authorization grant".to_string(),
                    ));
                }
            }

            let consumed = tx
                .execute(
                    "UPDATE mcp_oauth_codes SET consumed_at_ms = ?1 WHERE code_hash = ?2 AND consumed_at_ms IS NULL",
                    params![now_ms, &rec.code_hash],
                )
                .map_db()?;
            if consumed == 0 {
                return Ok(McpCodeExchangeOutcome::InvalidGrant(
                    "Authorization code already consumed".to_string(),
                ));
            }

            if !rec.code_challenge_method.eq_ignore_ascii_case("S256")
                || !verify_pkce_s256(code_verifier, &rec.code_challenge)
            {
                return Ok(McpCodeExchangeOutcome::InvalidGrant(
                    "PKCE S256 verification failed".to_string(),
                ));
            }

            let family_id = format!("fam_{}", random_hex(12));
            tx.execute(
                "UPDATE mcp_oauth_codes SET issued_family_id = ?1 WHERE code_hash = ?2",
                params![&family_id, &rec.code_hash],
            )
            .map_db()?;

            let effective_resource = if !rec.resource.is_empty() {
                rec.resource.clone()
            } else {
                requested_resource
                    .map(normalize_resource_str)
                    .unwrap_or_default()
            };

            let pair = Self::insert_token_pair_tx(
                tx,
                &family_id,
                &rec.client_id,
                rec.user_id,
                &effective_resource,
                &rec.scopes,
                &rec.allowed_node_ids,
                now_ms,
            )?;

            Ok(McpCodeExchangeOutcome::Issued(pair))
        })
    }

    #[allow(clippy::too_many_arguments)]
    fn insert_token_pair_tx(
        tx: &rusqlite::Transaction<'_>,
        family_id: &str,
        client_id: &str,
        user_id: u64,
        resource: &str,
        scopes: &[String],
        allowed_node_ids: &[String],
        now_ms: i64,
    ) -> Result<McpIssuedTokenPair> {
        let at_id_hex = random_hex(12);
        let access_token_id = format!("at_{at_id_hex}");
        let access_token = format!("fbmcp_at_{at_id_hex}.{}", random_b64url(32));
        let access_hash = sha256_hex(&access_token);

        let rt_id_hex = random_hex(12);
        let refresh_token_id = format!("rt_{rt_id_hex}");
        let refresh_token = format!("fbmcp_rt_{rt_id_hex}.{}", random_b64url(32));
        let refresh_hash = sha256_hex(&refresh_token);

        let scopes_json = serde_json::to_string(scopes)?;
        let allowed_json = serde_json::to_string(allowed_node_ids)?;
        let res_norm = normalize_resource_str(resource);

        let at_exp = now_ms.saturating_add(MCP_ACCESS_TOKEN_TTL_MS);
        let rt_exp = now_ms.saturating_add(MCP_REFRESH_TOKEN_TTL_MS);

        tx.execute(
            r#"
            INSERT INTO mcp_oauth_tokens (
                token_id, family_id, token_kind, token_hash, client_id, user_id,
                resource, scopes_json, allowed_node_ids_json, created_at_ms,
                expires_at_ms, last_used_at_ms, revoked_at_ms
            ) VALUES (?1, ?2, 'access', ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, NULL, NULL)
            "#,
            params![
                &access_token_id,
                family_id,
                &access_hash,
                client_id,
                user_id as i64,
                &res_norm,
                &scopes_json,
                &allowed_json,
                now_ms,
                at_exp,
            ],
        )
        .map_db()?;

        tx.execute(
            r#"
            INSERT INTO mcp_oauth_tokens (
                token_id, family_id, token_kind, token_hash, client_id, user_id,
                resource, scopes_json, allowed_node_ids_json, created_at_ms,
                expires_at_ms, last_used_at_ms, revoked_at_ms
            ) VALUES (?1, ?2, 'refresh', ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, NULL, NULL)
            "#,
            params![
                &refresh_token_id,
                family_id,
                &refresh_hash,
                client_id,
                user_id as i64,
                &res_norm,
                &scopes_json,
                &allowed_json,
                now_ms,
                rt_exp,
            ],
        )
        .map_db()?;

        Ok(McpIssuedTokenPair {
            family_id: family_id.to_string(),
            access_token,
            access_token_id,
            expires_in_secs: MCP_ACCESS_TOKEN_TTL_MS / 1000,
            refresh_token,
            refresh_token_id,
            scopes: scopes.to_vec(),
            allowed_node_ids: allowed_node_ids.to_vec(),
            resource: res_norm,
        })
    }

    fn find_token_row_conn(conn: &rusqlite::Connection, raw_token: &str) -> Result<Option<TokenRow>> {
        let trimmed = raw_token.trim();
        if trimmed.is_empty() {
            return Ok(None);
        }
        let candidate_hash = sha256_hex(trimmed);
        let lookup_id = parse_token_lookup_id(trimmed);

        let map_row = |r: &rusqlite::Row<'_>| -> rusqlite::Result<TokenRow> {
            let scopes_s: String = r.get(7)?;
            let allowed_s: String = r.get(8)?;
            Ok(TokenRow {
                token_id: r.get(0)?,
                family_id: r.get(1)?,
                token_kind: r.get(2)?,
                token_hash: r.get(3)?,
                client_id: r.get(4)?,
                user_id: r.get::<_, i64>(5)?.max(0) as u64,
                resource: r.get(6)?,
                scopes: serde_json::from_str(&scopes_s).unwrap_or_default(),
                allowed_node_ids: serde_json::from_str(&allowed_s).unwrap_or_default(),
                expires_at_ms: r.get(9)?,
                revoked_at_ms: r.get(10)?,
            })
        };

        let row = if let Some(tid) = lookup_id {
            conn.query_row(
                r#"
                SELECT token_id, family_id, token_kind, token_hash, client_id, user_id,
                       resource, scopes_json, allowed_node_ids_json, expires_at_ms, revoked_at_ms
                FROM mcp_oauth_tokens
                WHERE token_id = ?1
                LIMIT 1
                "#,
                params![tid],
                map_row,
            )
            .optional()
            .map_db()?
        } else {
            conn.query_row(
                r#"
                SELECT token_id, family_id, token_kind, token_hash, client_id, user_id,
                       resource, scopes_json, allowed_node_ids_json, expires_at_ms, revoked_at_ms
                FROM mcp_oauth_tokens
                WHERE token_hash = ?1
                LIMIT 1
                "#,
                params![&candidate_hash],
                map_row,
            )
            .optional()
            .map_db()?
        };

        let Some(found) = row else {
            return Ok(None);
        };
        if !ct_str_eq(&candidate_hash, &found.token_hash) {
            return Ok(None);
        }
        Ok(Some(found))
    }

    pub fn rotate_mcp_refresh_token(
        &self,
        raw_refresh_token: &str,
        client_id: Option<&str>,
        requested_scopes: Option<&[String]>,
        requested_resource: Option<&str>,
        now_ms: i64,
    ) -> Result<McpRefreshOutcome> {
        self.pool.with_transaction(|tx| {
            let Some(row) = Self::find_token_row_conn(tx, raw_refresh_token)? else {
                return Ok(McpRefreshOutcome::InvalidGrant(
                    "Invalid refresh token".to_string(),
                ));
            };
            if row.token_kind != "refresh" {
                return Ok(McpRefreshOutcome::InvalidGrant(
                    "Token is not a refresh token".to_string(),
                ));
            }
            if let Some(cid) = client_id.map(str::trim).filter(|s| !s.is_empty()) {
                if row.client_id != cid {
                    return Ok(McpRefreshOutcome::InvalidClient(
                        "client_id mismatch for refresh token".to_string(),
                    ));
                }
            }

            if row.revoked_at_ms.is_some() {
                tx.execute(
                    "UPDATE mcp_oauth_tokens SET revoked_at_ms = COALESCE(revoked_at_ms, ?1) WHERE family_id = ?2",
                    params![now_ms, &row.family_id],
                )
                .map_db()?;
                return Ok(McpRefreshOutcome::InvalidGrant(
                    "Refresh token reuse detected; entire token family revoked".to_string(),
                ));
            }

            if now_ms > row.expires_at_ms {
                return Ok(McpRefreshOutcome::InvalidGrant(
                    "Refresh token expired".to_string(),
                ));
            }

            if let Some(req_res) = requested_resource.map(normalize_resource_str).filter(|s| !s.is_empty()) {
                if !row.resource.is_empty() && !resource_matches(&row.resource, &req_res) {
                    return Ok(McpRefreshOutcome::InvalidTarget(
                        "resource parameter does not match original grant".to_string(),
                    ));
                }
            }

            let effective_scopes = match requested_scopes {
                Some(req_sc) if !req_sc.is_empty() => {
                    let allowed_set: BTreeSet<&str> = row.scopes.iter().map(String::as_str).collect();
                    for s in req_sc {
                        if !allowed_set.contains(s.as_str()) {
                            return Ok(McpRefreshOutcome::InvalidScope(format!(
                                "Requested scope `{s}` exceeds original grant"
                            )));
                        }
                    }
                    req_sc.to_vec()
                }
                _ => row.scopes.clone(),
            };

            let updated = tx
                .execute(
                    "UPDATE mcp_oauth_tokens SET revoked_at_ms = ?1, last_used_at_ms = ?1 WHERE token_id = ?2 AND revoked_at_ms IS NULL",
                    params![now_ms, &row.token_id],
                )
                .map_db()?;
            if updated == 0 {
                tx.execute(
                    "UPDATE mcp_oauth_tokens SET revoked_at_ms = COALESCE(revoked_at_ms, ?1) WHERE family_id = ?2",
                    params![now_ms, &row.family_id],
                )
                .map_db()?;
                return Ok(McpRefreshOutcome::InvalidGrant(
                    "Refresh token reuse detected; entire token family revoked".to_string(),
                ));
            }

            let effective_resource = if !row.resource.is_empty() {
                row.resource.clone()
            } else {
                requested_resource
                    .map(normalize_resource_str)
                    .unwrap_or_default()
            };

            let pair = Self::insert_token_pair_tx(
                tx,
                &row.family_id,
                &row.client_id,
                row.user_id,
                &effective_resource,
                &effective_scopes,
                &row.allowed_node_ids,
                now_ms,
            )?;

            Ok(McpRefreshOutcome::Rotated(pair))
        })
    }

    pub fn verify_mcp_access_token(
        &self,
        raw_access_token: &str,
        now_ms: i64,
    ) -> Result<Option<McpVerifiedAccessToken>> {
        let verified = self.pool.with_reader(|conn| {
            let Some(row) = Self::find_token_row_conn(conn, raw_access_token)? else {
                return Ok(None);
            };
            if row.token_kind != "access" || row.revoked_at_ms.is_some() || now_ms > row.expires_at_ms {
                return Ok(None);
            }
            let client_name: String = conn
                .query_row(
                    "SELECT client_name FROM mcp_oauth_clients WHERE client_id = ?1 LIMIT 1",
                    params![&row.client_id],
                    |r| r.get(0),
                )
                .optional()
                .map_db()?
                .unwrap_or_else(|| row.client_id.clone());

            Ok(Some(McpVerifiedAccessToken {
                token_id: row.token_id,
                family_id: row.family_id,
                client_id: row.client_id,
                client_name,
                user_id: row.user_id,
                resource: row.resource,
                scopes: row.scopes,
                allowed_node_ids: row.allowed_node_ids,
                expires_at_ms: row.expires_at_ms,
            }))
        })?;

        if let Some(ref tok) = verified {
            let tid = tok.token_id.clone();
            let _ = self.pool.with_conn(|conn| {
                conn.execute(
                    "UPDATE mcp_oauth_tokens SET last_used_at_ms = ?1 WHERE token_id = ?2",
                    params![now_ms, tid],
                )
                .map_db()?;
                Ok(())
            });
        }

        Ok(verified)
    }

    pub fn revoke_mcp_token(&self, raw_token: &str, now_ms: i64) -> Result<bool> {
        self.pool.with_transaction(|tx| {
            let Some(row) = Self::find_token_row_conn(tx, raw_token)? else {
                return Ok(false);
            };
            let changed = if row.token_kind == "refresh" {
                tx.execute(
                    "UPDATE mcp_oauth_tokens SET revoked_at_ms = COALESCE(revoked_at_ms, ?1) WHERE family_id = ?2",
                    params![now_ms, &row.family_id],
                )
                .map_db()?
            } else {
                tx.execute(
                    "UPDATE mcp_oauth_tokens SET revoked_at_ms = COALESCE(revoked_at_ms, ?1) WHERE token_id = ?2",
                    params![now_ms, &row.token_id],
                )
                .map_db()?
            };
            Ok(changed > 0)
        })
    }

    pub fn list_mcp_oauth_grants(&self, user_id: u64, now_ms: i64) -> Result<Vec<McpGrantRecord>> {
        self.pool.with_reader(|conn| {
            let mut stmt = conn
                .prepare(
                    r#"
                    SELECT t.family_id, t.client_id, COALESCE(c.client_name, t.client_id),
                           t.scopes_json, t.allowed_node_ids_json,
                           t.created_at_ms, t.last_used_at_ms, t.expires_at_ms, t.token_kind
                    FROM mcp_oauth_tokens t
                    LEFT JOIN mcp_oauth_clients c ON c.client_id = t.client_id
                    WHERE t.user_id = ?1
                      AND t.revoked_at_ms IS NULL
                      AND t.expires_at_ms > ?2
                    ORDER BY t.created_at_ms DESC
                    "#,
                )
                .map_db()?;
            let rows = stmt
                .query_map(params![user_id as i64, now_ms], |r| {
                    let scopes_s: String = r.get(3)?;
                    let allowed_s: String = r.get(4)?;
                    Ok((
                        r.get::<_, String>(0)?,
                        r.get::<_, String>(1)?,
                        r.get::<_, String>(2)?,
                        serde_json::from_str::<Vec<String>>(&scopes_s).unwrap_or_default(),
                        serde_json::from_str::<Vec<String>>(&allowed_s).unwrap_or_default(),
                        r.get::<_, i64>(5)?,
                        r.get::<_, Option<i64>>(6)?,
                        r.get::<_, i64>(7)?,
                        r.get::<_, String>(8)?,
                    ))
                })
                .map_db()?;

            let mut by_family: HashMap<String, McpGrantRecord> = HashMap::new();
            for item in rows {
                let (family_id, client_id, client_name, scopes, allowed_node_ids, created_at_ms, last_used_ms, expires_at_ms, kind) =
                    item.map_db()?;
                let entry = by_family.entry(family_id.clone()).or_insert_with(|| McpGrantRecord {
                    family_id,
                    client_id,
                    client_name,
                    scopes: scopes.clone(),
                    allowed_node_ids: allowed_node_ids.clone(),
                    created_at_ms,
                    last_used_ms,
                    expires_at_ms,
                });
                entry.created_at_ms = entry.created_at_ms.min(created_at_ms);
                entry.expires_at_ms = entry.expires_at_ms.max(expires_at_ms);
                if let Some(lu) = last_used_ms {
                    entry.last_used_ms = Some(entry.last_used_ms.map_or(lu, |cur| cur.max(lu)));
                }
                if kind == "access" {
                    entry.scopes = scopes;
                    entry.allowed_node_ids = allowed_node_ids;
                }
            }
            let mut grants: Vec<McpGrantRecord> = by_family.into_values().collect();
            grants.sort_by(|a, b| b.created_at_ms.cmp(&a.created_at_ms));
            Ok(grants)
        })
    }

    pub fn revoke_mcp_grant_family(
        &self,
        user_id: u64,
        family_id: &str,
        now_ms: i64,
    ) -> Result<bool> {
        self.pool.with_conn(|conn| {
            let changed = conn
                .execute(
                    "UPDATE mcp_oauth_tokens SET revoked_at_ms = COALESCE(revoked_at_ms, ?1) WHERE user_id = ?2 AND family_id = ?3 AND revoked_at_ms IS NULL",
                    params![now_ms, user_id as i64, family_id.trim()],
                )
                .map_db()?;
            Ok(changed > 0)
        })
    }

    pub fn record_mcp_tool_audit(&self, rec: &McpToolAuditRecord) -> Result<()> {
        self.pool.with_conn(|conn| {
            conn.execute(
                r#"
                INSERT INTO mcp_tool_audit_log (
                    audit_id, user_id, client_id, family_id, session_id, tool_name,
                    target_node_id, args_summary, ok, error_code, duration_ms, created_at_ms
                ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)
                "#,
                params![
                    rec.audit_id,
                    rec.user_id as i64,
                    rec.client_id,
                    rec.family_id,
                    rec.session_id,
                    rec.tool_name,
                    rec.target_node_id,
                    rec.args_summary,
                    if rec.ok { 1i64 } else { 0i64 },
                    rec.error_code,
                    rec.duration_ms,
                    rec.created_at_ms,
                ],
            )
            .map_db()?;
            Ok(())
        })
    }

    pub fn list_mcp_tool_audit(
        &self,
        user_id: u64,
        limit: usize,
    ) -> Result<Vec<McpToolAuditRecord>> {
        let lim = limit.clamp(1, 500) as i64;
        self.pool.with_reader(|conn| {
            let mut stmt = conn
                .prepare(
                    r#"
                    SELECT audit_id, user_id, client_id, family_id, session_id, tool_name,
                           target_node_id, args_summary, ok, error_code, duration_ms, created_at_ms
                    FROM mcp_tool_audit_log
                    WHERE user_id = ?1
                    ORDER BY created_at_ms DESC
                    LIMIT ?2
                    "#,
                )
                .map_db()?;
            let rows = stmt
                .query_map(params![user_id as i64, lim], |r| {
                    Ok(McpToolAuditRecord {
                        audit_id: r.get(0)?,
                        user_id: r.get::<_, i64>(1)?.max(0) as u64,
                        client_id: r.get(2)?,
                        family_id: r.get(3)?,
                        session_id: r.get(4)?,
                        tool_name: r.get(5)?,
                        target_node_id: r.get(6)?,
                        args_summary: r.get(7)?,
                        ok: r.get::<_, i64>(8)? != 0,
                        error_code: r.get(9)?,
                        duration_ms: r.get(10)?,
                        created_at_ms: r.get(11)?,
                    })
                })
                .map_db()?;
            rows.collect::<std::result::Result<Vec<_>, _>>().map_db()
        })
    }
}
