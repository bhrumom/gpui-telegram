//! Real SQLite persistence for the server-published OpenBot Marketplace:
//! - `marketplace_apps`: all 266 official OpenBot connectors & MCP plugins + user-published apps
//! - `marketplace_agents`: all 29 OpenBot marketplace agents with real install counts
//! - `marketplace_skills`: all 40 OpenBot marketplace skills with real `SKILL.md` & install counts
//! - `marketplace_user_connections`: per-user live verified connector/MCP connections with
//!   ChaCha20-Poly1305 AEAD encrypted credentials at rest (never returned to clients).

use std::collections::BTreeMap;

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD as BASE64_STANDARD;
use chrono::Utc;
use gpui_telegram_core::error::{Result, TelegramError};
use gpui_telegram_core::types::UserId;
use gpui_telegram_proto::marketplace::{
    MarketplaceAgentListingDto, MarketplaceAppListingDto, MarketplaceAuthFieldDto,
    MarketplaceConnectionDto, MarketplaceEnvPairDto, MarketplacePromptDto,
    MarketplaceSkillListingDto, MarketplaceToolSchemaDto, PublishMarketplaceAppRequest,
};
use openbot_runtime::platform::connector_driver::native_connector_real_endpoint;
use openbot_runtime::platform::marketplace::openbot_marketplace_catalog;
use rusqlite::{OptionalExtension, params};
use serde::{Deserialize, Serialize};
use zeroize::Zeroizing;

use crate::db::computer_store::{
    decrypt_computer_secret, encrypt_computer_secret, resolve_computer_master_key,
};
use crate::db::{ServerDatabase, SqliteResultExt};

pub const MIGRATION_V44_MARKETPLACE_CONNECTORS: &str = r#"
CREATE TABLE IF NOT EXISTS marketplace_apps (
    app_id TEXT PRIMARY KEY,
    slug TEXT NOT NULL UNIQUE COLLATE NOCASE,
    kind TEXT NOT NULL,
    name TEXT NOT NULL,
    tagline TEXT NOT NULL,
    zh_tagline TEXT,
    description TEXT NOT NULL,
    zh_description TEXT,
    category TEXT NOT NULL,
    creator_name TEXT NOT NULL,
    version TEXT NOT NULL DEFAULT '1.0.0',
    brand_hex TEXT NOT NULL DEFAULT '#3F3F46',
    website_url TEXT,
    privacy_policy_url TEXT,
    terms_url TEXT,
    server_name TEXT NOT NULL,
    transport TEXT NOT NULL,
    url TEXT,
    command TEXT,
    args_json TEXT NOT NULL DEFAULT '[]',
    env_json TEXT NOT NULL DEFAULT '[]',
    auth_kind TEXT NOT NULL DEFAULT 'open',
    auth_label TEXT,
    auth_fields_json TEXT NOT NULL DEFAULT '[]',
    auth_steps_json TEXT NOT NULL DEFAULT '[]',
    auth_note TEXT,
    auth_docs_url TEXT,
    auth_docs_label TEXT,
    prompts_json TEXT NOT NULL DEFAULT '[]',
    setup_kind TEXT,
    verified INTEGER NOT NULL DEFAULT 1,
    install_count INTEGER NOT NULL DEFAULT 0,
    publisher_user_id INTEGER,
    published_at_ms INTEGER NOT NULL,
    updated_at_ms INTEGER NOT NULL
) STRICT;
CREATE INDEX IF NOT EXISTS idx_marketplace_apps_category ON marketplace_apps(category, kind);

CREATE TABLE IF NOT EXISTS marketplace_agents (
    slug TEXT PRIMARY KEY,
    name TEXT NOT NULL,
    title TEXT NOT NULL,
    description TEXT NOT NULL,
    category TEXT NOT NULL,
    creator_name TEXT NOT NULL,
    avatar_hue INTEGER NOT NULL DEFAULT 220,
    skills_json TEXT NOT NULL DEFAULT '[]',
    verified INTEGER NOT NULL DEFAULT 1,
    featured INTEGER NOT NULL DEFAULT 0,
    version INTEGER NOT NULL DEFAULT 1,
    install_count INTEGER NOT NULL DEFAULT 0,
    updated_at_ms INTEGER NOT NULL
) STRICT;

CREATE TABLE IF NOT EXISTS marketplace_skills (
    slug TEXT PRIMARY KEY,
    name TEXT NOT NULL,
    description TEXT NOT NULL,
    category TEXT NOT NULL,
    creator_name TEXT NOT NULL,
    source TEXT NOT NULL DEFAULT 'openbot',
    verified INTEGER NOT NULL DEFAULT 1,
    featured INTEGER NOT NULL DEFAULT 0,
    version INTEGER NOT NULL DEFAULT 1,
    install_count INTEGER NOT NULL DEFAULT 0,
    skill_md TEXT NOT NULL,
    updated_at_ms INTEGER NOT NULL
) STRICT;

CREATE TABLE IF NOT EXISTS marketplace_user_connections (
    user_id INTEGER NOT NULL,
    app_id TEXT NOT NULL,
    kind TEXT NOT NULL,
    name TEXT NOT NULL,
    status TEXT NOT NULL,
    account_label TEXT,
    endpoint TEXT,
    transport TEXT NOT NULL DEFAULT 'http',
    mcp_server_id TEXT,
    credentials_enc_b64 TEXT NOT NULL DEFAULT '',
    credentials_nonce_b64 TEXT NOT NULL DEFAULT '',
    metadata_json TEXT NOT NULL DEFAULT '{}',
    discovered_tools_json TEXT NOT NULL DEFAULT '[]',
    last_error TEXT,
    connected_at_ms INTEGER NOT NULL,
    verified_at_ms INTEGER,
    updated_at_ms INTEGER NOT NULL,
    PRIMARY KEY (user_id, app_id)
) STRICT;
CREATE INDEX IF NOT EXISTS idx_marketplace_user_connections_status
    ON marketplace_user_connections(status, updated_at_ms DESC);
"#;

/// Encrypted secret payload stored inside `marketplace_user_connections`.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StoredConnectorSecret {
    #[serde(default)]
    pub credential: String,
    #[serde(default)]
    pub headers: Vec<MarketplaceEnvPairDto>,
    #[serde(default)]
    pub env: Vec<MarketplaceEnvPairDto>,
    #[serde(default)]
    pub command: Option<String>,
    #[serde(default)]
    pub args: Vec<String>,
}

#[derive(Debug, Clone)]
pub struct HydratedMarketplaceConnection {
    pub user_id: u64,
    pub dto: MarketplaceConnectionDto,
    pub secret: StoredConnectorSecret,
}

fn sanitize_slug(raw: &str) -> String {
    let mut out = String::with_capacity(raw.len());
    for ch in raw.trim().chars() {
        if ch.is_ascii_alphanumeric() {
            out.push(ch.to_ascii_lowercase());
        } else if (ch == '-' || ch == '_' || ch == ' ') && !out.ends_with('-') {
            out.push('-');
        }
    }
    out.trim_matches('-').to_string()
}

fn encrypt_secret_payload(owner_user_id: u64, secret: &StoredConnectorSecret) -> Result<(String, String)> {
    let master = resolve_computer_master_key();
    let json_str = Zeroizing::new(
        serde_json::to_string(secret)
            .map_err(|e| TelegramError::Internal(format!("Serialize connector secret: {e}")))?,
    );
    let (ct, nonce) = encrypt_computer_secret(&master, owner_user_id, &json_str)?;
    Ok((BASE64_STANDARD.encode(ct), BASE64_STANDARD.encode(nonce)))
}

fn decrypt_secret_payload(
    owner_user_id: u64,
    ct_b64: &str,
    nonce_b64: &str,
) -> StoredConnectorSecret {
    if ct_b64.trim().is_empty() || nonce_b64.trim().is_empty() {
        return StoredConnectorSecret::default();
    }
    let master = resolve_computer_master_key();
    let Ok(ct) = BASE64_STANDARD.decode(ct_b64.trim()) else {
        return StoredConnectorSecret::default();
    };
    let Ok(nonce) = BASE64_STANDARD.decode(nonce_b64.trim()) else {
        return StoredConnectorSecret::default();
    };
    let Ok(plain) = decrypt_computer_secret(&master, owner_user_id, &ct, &nonce) else {
        return StoredConnectorSecret::default();
    };
    serde_json::from_str(&plain).unwrap_or_default()
}

fn row_to_app_listing(r: &rusqlite::Row<'_>) -> rusqlite::Result<MarketplaceAppListingDto> {
    let args_json: String = r.get(19)?;
    let env_json: String = r.get(20)?;
    let auth_fields_json: String = r.get(23)?;
    let auth_steps_json: String = r.get(24)?;
    let prompts_json: String = r.get(28)?;
    let verified_int: i64 = r.get(30)?;
    let install_count: i64 = r.get(31)?;
    let pub_uid: Option<i64> = r.get(32)?;
    Ok(MarketplaceAppListingDto {
        app_id: r.get(0)?,
        slug: r.get(1)?,
        kind: r.get(2)?,
        name: r.get(3)?,
        tagline: r.get(4)?,
        zh_tagline: r.get(5)?,
        description: r.get(6)?,
        zh_description: r.get(7)?,
        category: r.get(8)?,
        creator_name: r.get(9)?,
        version: r.get(10)?,
        brand_hex: r.get(11)?,
        website_url: r.get(12)?,
        privacy_policy_url: r.get(13)?,
        terms_url: r.get(14)?,
        server_name: r.get(15)?,
        transport: r.get(16)?,
        url: r.get(17)?,
        command: r.get(18)?,
        args: serde_json::from_str(&args_json).unwrap_or_default(),
        env: serde_json::from_str(&env_json).unwrap_or_default(),
        auth_kind: r.get(21)?,
        auth_label: r.get(22)?,
        auth_fields: serde_json::from_str(&auth_fields_json).unwrap_or_default(),
        auth_steps: serde_json::from_str(&auth_steps_json).unwrap_or_default(),
        auth_note: r.get(25)?,
        auth_docs_url: r.get(26)?,
        auth_docs_label: r.get(27)?,
        prompts: serde_json::from_str(&prompts_json).unwrap_or_default(),
        setup_kind: r.get(29)?,
        verified: verified_int != 0,
        install_count: install_count.max(0) as u64,
        publisher_user_id: pub_uid.filter(|&v| v > 0).map(|v| v as u64),
        published_at_ms: r.get(33)?,
    })
}

fn row_to_connection_dto(r: &rusqlite::Row<'_>) -> rusqlite::Result<MarketplaceConnectionDto> {
    let meta_json: String = r.get(8)?;
    let tools_json: String = r.get(9)?;
    Ok(MarketplaceConnectionDto {
        app_id: r.get(0)?,
        kind: r.get(1)?,
        name: r.get(2)?,
        status: r.get(3)?,
        account_label: r.get(4)?,
        endpoint: r.get(5)?,
        transport: r.get(6)?,
        mcp_server_id: r.get(7)?,
        metadata: serde_json::from_str(&meta_json).unwrap_or_default(),
        discovered_tools: serde_json::from_str(&tools_json).unwrap_or_default(),
        last_error: r.get(10)?,
        connected_at_ms: r.get(11)?,
        verified_at_ms: r.get(12)?,
    })
}

const APP_SELECT_COLS: &str = "app_id, slug, kind, name, tagline, zh_tagline, description, zh_description, category, creator_name, version, brand_hex, website_url, privacy_policy_url, terms_url, server_name, transport, url, command, args_json, env_json, auth_kind, auth_label, auth_fields_json, auth_steps_json, auth_note, auth_docs_url, auth_docs_label, prompts_json, setup_kind, verified, install_count, publisher_user_id, published_at_ms";

impl ServerDatabase {
    /// Idempotently publishes the full authentic OpenBot Marketplace catalog (9 native connectors,
    /// 257 MCP plugins, 29 agents, 40 skills) into SQLite while preserving real accumulated
    /// `install_count` and any user-published connector apps. Zero fake install counts!
    pub fn publish_openbot_marketplace_catalog(&self) -> Result<(usize, usize, usize)> {
        let catalog = openbot_marketplace_catalog();
        let now_ms = Utc::now().timestamp_millis();

        self.pool.with_transaction(|tx| {
            let mut apps_count = 0usize;
            for conn in &catalog.connectors {
                let real_endpoint = native_connector_real_endpoint(&conn.connector_id);
                let (transport, url, cmd) = match conn.connector_id.as_str() {
                    "onepassword" => ("stdio", None, Some("op")),
                    "bitwarden" => ("stdio", None, Some("bw")),
                    "protonpass" => ("stdio", None, Some("pass-cli")),
                    _ => ("http", Some(real_endpoint), None),
                };
                tx.execute(
                    r#"INSERT INTO marketplace_apps (
                        app_id, slug, kind, name, tagline, zh_tagline, description, zh_description,
                        category, creator_name, version, brand_hex, website_url, privacy_policy_url,
                        terms_url, server_name, transport, url, command, args_json, env_json,
                        auth_kind, auth_label, auth_fields_json, auth_steps_json, auth_note,
                        auth_docs_url, auth_docs_label, prompts_json, setup_kind, verified,
                        install_count, publisher_user_id, published_at_ms, updated_at_ms
                    ) VALUES (
                        ?1, ?2, 'connector', ?3, ?4, ?5, ?6, ?7,
                        ?8, ?9, '1.0.0', ?10, ?11, ?12,
                        ?13, ?14, ?15, ?16, ?17, '[]', '[]',
                        ?18, NULL, '[]', '[]', NULL,
                        NULL, NULL, '[]', ?18, 1,
                        0, NULL, ?19, ?19
                    )
                    ON CONFLICT(app_id) DO UPDATE SET
                        name = excluded.name,
                        tagline = excluded.tagline,
                        zh_tagline = excluded.zh_tagline,
                        description = excluded.description,
                        zh_description = excluded.zh_description,
                        category = excluded.category,
                        creator_name = excluded.creator_name,
                        brand_hex = excluded.brand_hex,
                        website_url = excluded.website_url,
                        privacy_policy_url = excluded.privacy_policy_url,
                        terms_url = excluded.terms_url,
                        server_name = excluded.server_name,
                        transport = excluded.transport,
                        url = excluded.url,
                        command = excluded.command,
                        auth_kind = excluded.auth_kind,
                        setup_kind = excluded.setup_kind,
                        verified = 1,
                        updated_at_ms = excluded.updated_at_ms"#,
                    params![
                        conn.id,
                        conn.slug,
                        conn.name,
                        conn.tagline,
                        conn.zh_tagline,
                        conn.description,
                        conn.zh_description,
                        conn.category,
                        conn.creator_name,
                        conn.brand_hex,
                        conn.website_url,
                        conn.privacy_policy_url,
                        conn.terms_url,
                        format!("connector-{}", conn.connector_id),
                        transport,
                        url,
                        cmd,
                        conn.setup_kind,
                        now_ms,
                    ],
                )
                .map_db()?;
                apps_count += 1;
            }

            for plug in &catalog.plugins {
                let args_json = serde_json::to_string(&plug.args).unwrap_or_else(|_| "[]".into());
                let env_dtos: Vec<MarketplaceEnvPairDto> = plug
                    .env
                    .iter()
                    .map(|e| MarketplaceEnvPairDto {
                        key: e.key.clone(),
                        value: e.value.clone(),
                    })
                    .collect();
                let env_json = serde_json::to_string(&env_dtos).unwrap_or_else(|_| "[]".into());
                let field_dtos: Vec<MarketplaceAuthFieldDto> = plug
                    .auth_fields
                    .iter()
                    .map(|f| MarketplaceAuthFieldDto {
                        id: f.id.clone(),
                        label: f.label.clone(),
                        env: f.env.clone(),
                        header: f.header.clone(),
                        prefix: f.prefix.clone(),
                        placeholder: f.placeholder.clone(),
                        hint: f.hint.clone(),
                        optional: f.optional,
                        url: f.url,
                    })
                    .collect();
                let auth_fields_json =
                    serde_json::to_string(&field_dtos).unwrap_or_else(|_| "[]".into());
                let auth_steps_json =
                    serde_json::to_string(&plug.auth_steps).unwrap_or_else(|_| "[]".into());
                let prompt_dtos: Vec<MarketplacePromptDto> = plug
                    .prompts
                    .iter()
                    .map(|p| MarketplacePromptDto {
                        id: p.id.clone(),
                        text: p.text.clone(),
                        zh_text: p.zh_text.clone(),
                    })
                    .collect();
                let prompts_json =
                    serde_json::to_string(&prompt_dtos).unwrap_or_else(|_| "[]".into());

                tx.execute(
                    r#"INSERT INTO marketplace_apps (
                        app_id, slug, kind, name, tagline, zh_tagline, description, zh_description,
                        category, creator_name, version, brand_hex, website_url, privacy_policy_url,
                        terms_url, server_name, transport, url, command, args_json, env_json,
                        auth_kind, auth_label, auth_fields_json, auth_steps_json, auth_note,
                        auth_docs_url, auth_docs_label, prompts_json, setup_kind, verified,
                        install_count, publisher_user_id, published_at_ms, updated_at_ms
                    ) VALUES (
                        ?1, ?2, 'plugin', ?3, ?4, ?5, ?6, ?7,
                        ?8, ?9, ?10, ?11, ?12, ?13,
                        ?14, ?15, ?16, ?17, ?18, ?19, ?20,
                        ?21, ?22, ?23, ?24, ?25,
                        ?26, ?27, ?28, NULL, 1,
                        0, NULL, ?29, ?29
                    )
                    ON CONFLICT(app_id) DO UPDATE SET
                        name = excluded.name,
                        tagline = excluded.tagline,
                        zh_tagline = excluded.zh_tagline,
                        description = excluded.description,
                        zh_description = excluded.zh_description,
                        category = excluded.category,
                        creator_name = excluded.creator_name,
                        version = excluded.version,
                        brand_hex = excluded.brand_hex,
                        website_url = excluded.website_url,
                        privacy_policy_url = excluded.privacy_policy_url,
                        terms_url = excluded.terms_url,
                        server_name = excluded.server_name,
                        transport = excluded.transport,
                        url = excluded.url,
                        command = excluded.command,
                        args_json = excluded.args_json,
                        env_json = excluded.env_json,
                        auth_kind = excluded.auth_kind,
                        auth_label = excluded.auth_label,
                        auth_fields_json = excluded.auth_fields_json,
                        auth_steps_json = excluded.auth_steps_json,
                        auth_note = excluded.auth_note,
                        auth_docs_url = excluded.auth_docs_url,
                        auth_docs_label = excluded.auth_docs_label,
                        prompts_json = excluded.prompts_json,
                        verified = 1,
                        updated_at_ms = excluded.updated_at_ms"#,
                    params![
                        plug.id,
                        plug.slug,
                        plug.name,
                        plug.tagline,
                        plug.zh_tagline,
                        plug.description,
                        plug.zh_description,
                        plug.category,
                        plug.creator_name,
                        plug.version,
                        plug.brand_hex,
                        plug.website_url,
                        plug.privacy_policy_url,
                        plug.terms_url,
                        plug.server_name,
                        plug.transport,
                        plug.url,
                        plug.command,
                        args_json,
                        env_json,
                        plug.auth_kind,
                        plug.auth_label,
                        auth_fields_json,
                        auth_steps_json,
                        plug.auth_note,
                        plug.auth_docs_url,
                        plug.auth_docs_label,
                        prompts_json,
                        now_ms,
                    ],
                )
                .map_db()?;
                apps_count += 1;
            }

            let mut agents_count = 0usize;
            for ag in &catalog.agents {
                let skills_json = serde_json::to_string(&ag.skills).unwrap_or_else(|_| "[]".into());
                tx.execute(
                    r#"INSERT INTO marketplace_agents (
                        slug, name, title, description, category, creator_name,
                        avatar_hue, skills_json, verified, featured, version,
                        install_count, updated_at_ms
                    ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, 0, ?12)
                    ON CONFLICT(slug) DO UPDATE SET
                        name = excluded.name,
                        title = excluded.title,
                        description = excluded.description,
                        category = excluded.category,
                        creator_name = excluded.creator_name,
                        avatar_hue = excluded.avatar_hue,
                        skills_json = excluded.skills_json,
                        verified = excluded.verified,
                        featured = excluded.featured,
                        version = excluded.version,
                        updated_at_ms = excluded.updated_at_ms"#,
                    params![
                        ag.slug,
                        ag.name,
                        ag.title,
                        ag.description,
                        ag.category,
                        ag.creator_name,
                        ag.avatar_hue as i64,
                        skills_json,
                        i64::from(ag.verified),
                        i64::from(ag.featured),
                        ag.version as i64,
                        now_ms,
                    ],
                )
                .map_db()?;
                agents_count += 1;
            }

            let mut skills_count = 0usize;
            for sk in &catalog.skills {
                tx.execute(
                    r#"INSERT INTO marketplace_skills (
                        slug, name, description, category, creator_name, source,
                        verified, featured, version, install_count, skill_md, updated_at_ms
                    ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, 0, ?10, ?11)
                    ON CONFLICT(slug) DO UPDATE SET
                        name = excluded.name,
                        description = excluded.description,
                        category = excluded.category,
                        creator_name = excluded.creator_name,
                        source = excluded.source,
                        verified = excluded.verified,
                        featured = excluded.featured,
                        version = excluded.version,
                        skill_md = excluded.skill_md,
                        updated_at_ms = excluded.updated_at_ms"#,
                    params![
                        sk.slug,
                        sk.name,
                        sk.description,
                        sk.category,
                        sk.creator_name,
                        sk.source,
                        i64::from(sk.verified),
                        i64::from(sk.featured),
                        sk.version as i64,
                        sk.skill_md,
                        now_ms,
                    ],
                )
                .map_db()?;
                skills_count += 1;
            }

            Ok((apps_count, agents_count, skills_count))
        })
    }

    pub fn list_marketplace_apps(
        &self,
        category: Option<&str>,
        query: Option<&str>,
    ) -> Result<Vec<MarketplaceAppListingDto>> {
        let cat_filter = category.map(str::trim).filter(|s| !s.is_empty());
        let q_lower = query
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::to_lowercase);
        self.pool.with_reader(|conn| {
            let sql = format!(
                "SELECT {APP_SELECT_COLS} FROM marketplace_apps ORDER BY CASE kind WHEN 'connector' THEN 0 WHEN 'custom_published' THEN 1 ELSE 2 END, rowid ASC"
            );
            let mut stmt = conn.prepare(&sql).map_db()?;
            let rows = stmt.query_map([], row_to_app_listing).map_db()?;
            let all = rows.collect::<std::result::Result<Vec<_>, _>>().map_db()?;
            Ok(all
                .into_iter()
                .filter(|item| {
                    if let Some(cat) = cat_filter {
                        if !item.category.eq_ignore_ascii_case(cat) {
                            return false;
                        }
                    }
                    if let Some(ref q) = q_lower {
                        let hay = format!(
                            "{} {} {} {} {} {}",
                            item.name,
                            item.slug,
                            item.tagline,
                            item.zh_tagline.as_deref().unwrap_or(""),
                            item.description,
                            item.creator_name
                        )
                        .to_lowercase();
                        if !hay.contains(q.as_str()) {
                            return false;
                        }
                    }
                    true
                })
                .collect())
        })
    }

    pub fn get_marketplace_app(&self, app_id_or_slug: &str) -> Result<Option<MarketplaceAppListingDto>> {
        let needle = app_id_or_slug.trim();
        self.pool.with_reader(|conn| {
            let sql = format!(
                "SELECT {APP_SELECT_COLS} FROM marketplace_apps WHERE app_id = ?1 OR slug = ?1 COLLATE NOCASE LIMIT 1"
            );
            conn.query_row(&sql, params![needle], row_to_app_listing)
                .optional()
                .map_db()
        })
    }

    /// Publishes a brand-new real connector / MCP app into `marketplace_apps` so all users can
    /// discover and connect it immediately.
    pub fn publish_custom_marketplace_app(
        &self,
        publisher_user_id: UserId,
        creator_name: &str,
        req: &PublishMarketplaceAppRequest,
    ) -> Result<MarketplaceAppListingDto> {
        let name = req.name.trim();
        if name.is_empty() || name.len() > 80 {
            return Err(TelegramError::Validation(
                "Connector name must be 1..=80 characters.".into(),
            ));
        }
        let raw_slug = req
            .slug
            .as_deref()
            .filter(|s| !s.trim().is_empty())
            .unwrap_or(name);
        let slug = sanitize_slug(raw_slug);
        if slug.is_empty() {
            return Err(TelegramError::Validation(
                "Connector slug must contain alphanumeric characters.".into(),
            ));
        }
        let transport = req.transport.trim().to_ascii_lowercase();
        if transport != "http" && transport != "stdio" {
            return Err(TelegramError::Validation(
                "transport must be 'http' or 'stdio'.".into(),
            ));
        }
        let url = req
            .url
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::to_string);
        let command = req
            .command
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::to_string);
        if transport == "http" {
            let Some(ref u) = url else {
                return Err(TelegramError::Validation(
                    "HTTP MCP connector requires a valid https:// or http:// url.".into(),
                ));
            };
            if !u.starts_with("https://") && !u.starts_with("http://") {
                return Err(TelegramError::Validation(
                    "HTTP MCP url must start with https:// or http://.".into(),
                ));
            }
        } else if command.is_none() {
            return Err(TelegramError::Validation(
                "STDIO MCP connector requires a command executable.".into(),
            ));
        }

        let app_id = format!("published:{slug}");
        let tagline = req.tagline.trim();
        let tagline_final = if tagline.is_empty() {
            format!("{name} MCP connector")
        } else {
            tagline.to_string()
        };
        let description_final = req
            .description
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::to_string)
            .unwrap_or_else(|| tagline_final.clone());
        let category = req
            .category
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .unwrap_or("other")
            .to_ascii_lowercase();
        let brand_hex = req
            .brand_hex
            .as_deref()
            .map(str::trim)
            .filter(|s| s.starts_with('#') && (s.len() == 7 || s.len() == 4))
            .unwrap_or("#3F3F46")
            .to_string();
        let auth_kind = req
            .auth_kind
            .as_deref()
            .map(str::trim)
            .filter(|s| matches!(*s, "open" | "key" | "link" | "local"))
            .unwrap_or("open")
            .to_string();
        let args_json = serde_json::to_string(&req.args).unwrap_or_else(|_| "[]".into());
        let env_json = serde_json::to_string(&req.env).unwrap_or_else(|_| "[]".into());
        let auth_fields_json =
            serde_json::to_string(&req.auth_fields).unwrap_or_else(|_| "[]".into());
        let now_ms = Utc::now().timestamp_millis();

        self.pool.with_writer(|conn| {
            conn.execute(
                r#"INSERT INTO marketplace_apps (
                    app_id, slug, kind, name, tagline, zh_tagline, description, zh_description,
                    category, creator_name, version, brand_hex, website_url, privacy_policy_url,
                    terms_url, server_name, transport, url, command, args_json, env_json,
                    auth_kind, auth_label, auth_fields_json, auth_steps_json, auth_note,
                    auth_docs_url, auth_docs_label, prompts_json, setup_kind, verified,
                    install_count, publisher_user_id, published_at_ms, updated_at_ms
                ) VALUES (
                    ?1, ?2, 'custom_published', ?3, ?4, ?5, ?6, ?7,
                    ?8, ?9, '1.0.0', ?10, ?11, NULL,
                    NULL, ?2, ?12, ?13, ?14, ?15, ?16,
                    ?17, NULL, ?18, '[]', NULL,
                    NULL, NULL, '[]', NULL, 0,
                    0, ?19, ?20, ?20
                )
                ON CONFLICT(app_id) DO UPDATE SET
                    name = excluded.name,
                    tagline = excluded.tagline,
                    zh_tagline = excluded.zh_tagline,
                    description = excluded.description,
                    zh_description = excluded.zh_description,
                    category = excluded.category,
                    brand_hex = excluded.brand_hex,
                    website_url = excluded.website_url,
                    transport = excluded.transport,
                    url = excluded.url,
                    command = excluded.command,
                    args_json = excluded.args_json,
                    env_json = excluded.env_json,
                    auth_kind = excluded.auth_kind,
                    auth_fields_json = excluded.auth_fields_json,
                    updated_at_ms = excluded.updated_at_ms"#,
                params![
                    app_id,
                    slug,
                    name,
                    tagline_final,
                    req.zh_tagline,
                    description_final,
                    req.zh_description,
                    category,
                    creator_name,
                    brand_hex,
                    req.website_url,
                    transport,
                    url,
                    command,
                    args_json,
                    env_json,
                    auth_kind,
                    auth_fields_json,
                    publisher_user_id.0 as i64,
                    now_ms,
                ],
            )
            .map_db()?;
            Ok(())
        })?;

        self.get_marketplace_app(&app_id)?.ok_or_else(|| {
            TelegramError::Internal("Published marketplace app missing after insert".into())
        })
    }

    pub fn increment_marketplace_app_installs(&self, app_id: &str) -> Result<u64> {
        self.pool.with_writer(|conn| {
            conn.execute(
                "UPDATE marketplace_apps SET install_count = install_count + 1 WHERE app_id = ?1 OR slug = ?1 COLLATE NOCASE",
                params![app_id],
            )
            .map_db()?;
            let count: Option<i64> = conn
                .query_row(
                    "SELECT install_count FROM marketplace_apps WHERE app_id = ?1 OR slug = ?1 COLLATE NOCASE LIMIT 1",
                    params![app_id],
                    |r| r.get(0),
                )
                .optional()
                .map_db()?;
            Ok(count.unwrap_or(0).max(0) as u64)
        })
    }

    pub fn list_marketplace_agents(&self) -> Result<Vec<MarketplaceAgentListingDto>> {
        self.pool.with_reader(|conn| {
            let mut stmt = conn
                .prepare(
                    "SELECT slug, name, title, description, category, creator_name, avatar_hue, skills_json, verified, featured, version, install_count FROM marketplace_agents ORDER BY rowid ASC",
                )
                .map_db()?;
            let rows = stmt
                .query_map([], |r| {
                    let skills_json: String = r.get(7)?;
                    Ok(MarketplaceAgentListingDto {
                        slug: r.get(0)?,
                        name: r.get(1)?,
                        title: r.get(2)?,
                        description: r.get(3)?,
                        category: r.get(4)?,
                        creator_name: r.get(5)?,
                        avatar_hue: r.get::<_, i64>(6)?.clamp(0, 359) as u16,
                        skills: serde_json::from_str(&skills_json).unwrap_or_default(),
                        verified: r.get::<_, i64>(8)? != 0,
                        featured: r.get::<_, i64>(9)? != 0,
                        version: r.get::<_, i64>(10)?.max(1) as u32,
                        install_count: r.get::<_, i64>(11)?.max(0) as u64,
                    })
                })
                .map_db()?;
            rows.collect::<std::result::Result<Vec<_>, _>>().map_db()
        })
    }

    pub fn increment_marketplace_agent_installs(&self, slug: &str) -> Result<u64> {
        self.pool.with_writer(|conn| {
            conn.execute(
                "UPDATE marketplace_agents SET install_count = install_count + 1 WHERE slug = ?1 COLLATE NOCASE",
                params![slug],
            )
            .map_db()?;
            let count: Option<i64> = conn
                .query_row(
                    "SELECT install_count FROM marketplace_agents WHERE slug = ?1 COLLATE NOCASE LIMIT 1",
                    params![slug],
                    |r| r.get(0),
                )
                .optional()
                .map_db()?;
            Ok(count.unwrap_or(0).max(0) as u64)
        })
    }

    pub fn list_marketplace_skills(&self) -> Result<Vec<MarketplaceSkillListingDto>> {
        self.pool.with_reader(|conn| {
            let mut stmt = conn
                .prepare(
                    "SELECT slug, name, description, category, creator_name, source, verified, featured, version, install_count, skill_md FROM marketplace_skills ORDER BY rowid ASC",
                )
                .map_db()?;
            let rows = stmt
                .query_map([], |r| {
                    Ok(MarketplaceSkillListingDto {
                        slug: r.get(0)?,
                        name: r.get(1)?,
                        description: r.get(2)?,
                        category: r.get(3)?,
                        creator_name: r.get(4)?,
                        source: r.get(5)?,
                        verified: r.get::<_, i64>(6)? != 0,
                        featured: r.get::<_, i64>(7)? != 0,
                        version: r.get::<_, i64>(8)?.max(1) as u32,
                        install_count: r.get::<_, i64>(9)?.max(0) as u64,
                        skill_md: r.get(10)?,
                    })
                })
                .map_db()?;
            rows.collect::<std::result::Result<Vec<_>, _>>().map_db()
        })
    }

    pub fn increment_marketplace_skill_installs(&self, slug: &str) -> Result<u64> {
        self.pool.with_writer(|conn| {
            conn.execute(
                "UPDATE marketplace_skills SET install_count = install_count + 1 WHERE slug = ?1 COLLATE NOCASE",
                params![slug],
            )
            .map_db()?;
            let count: Option<i64> = conn
                .query_row(
                    "SELECT install_count FROM marketplace_skills WHERE slug = ?1 COLLATE NOCASE LIMIT 1",
                    params![slug],
                    |r| r.get(0),
                )
                .optional()
                .map_db()?;
            Ok(count.unwrap_or(0).max(0) as u64)
        })
    }

    #[allow(clippy::too_many_arguments)]
    pub fn upsert_marketplace_user_connection(
        &self,
        user_id: UserId,
        app_id: &str,
        kind: &str,
        name: &str,
        status: &str,
        account_label: Option<&str>,
        endpoint: Option<&str>,
        transport: &str,
        mcp_server_id: Option<&str>,
        secret: &StoredConnectorSecret,
        metadata: &BTreeMap<String, String>,
        discovered_tools: &[MarketplaceToolSchemaDto],
        last_error: Option<&str>,
        verified_at_ms: Option<i64>,
    ) -> Result<MarketplaceConnectionDto> {
        let now_ms = Utc::now().timestamp_millis();
        let (ct_b64, nonce_b64) = encrypt_secret_payload(user_id.0, secret)?;
        let meta_json = serde_json::to_string(metadata).unwrap_or_else(|_| "{}".into());
        let tools_json = serde_json::to_string(discovered_tools).unwrap_or_else(|_| "[]".into());

        self.pool.with_writer(|conn| {
            conn.execute(
                r#"INSERT INTO marketplace_user_connections (
                    user_id, app_id, kind, name, status, account_label, endpoint,
                    transport, mcp_server_id, credentials_enc_b64, credentials_nonce_b64,
                    metadata_json, discovered_tools_json, last_error,
                    connected_at_ms, verified_at_ms, updated_at_ms
                ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?15)
                ON CONFLICT(user_id, app_id) DO UPDATE SET
                    kind = excluded.kind,
                    name = excluded.name,
                    status = excluded.status,
                    account_label = excluded.account_label,
                    endpoint = excluded.endpoint,
                    transport = excluded.transport,
                    mcp_server_id = excluded.mcp_server_id,
                    credentials_enc_b64 = excluded.credentials_enc_b64,
                    credentials_nonce_b64 = excluded.credentials_nonce_b64,
                    metadata_json = excluded.metadata_json,
                    discovered_tools_json = excluded.discovered_tools_json,
                    last_error = excluded.last_error,
                    verified_at_ms = excluded.verified_at_ms,
                    updated_at_ms = excluded.updated_at_ms"#,
                params![
                    user_id.0 as i64,
                    app_id,
                    kind,
                    name,
                    status,
                    account_label,
                    endpoint,
                    transport,
                    mcp_server_id,
                    ct_b64,
                    nonce_b64,
                    meta_json,
                    tools_json,
                    last_error,
                    now_ms,
                    verified_at_ms,
                ],
            )
            .map_db()?;
            Ok(())
        })?;

        Ok(MarketplaceConnectionDto {
            app_id: app_id.to_string(),
            kind: kind.to_string(),
            name: name.to_string(),
            status: status.to_string(),
            account_label: account_label.map(str::to_string),
            endpoint: endpoint.map(str::to_string),
            transport: transport.to_string(),
            mcp_server_id: mcp_server_id.map(str::to_string),
            discovered_tools: discovered_tools.to_vec(),
            last_error: last_error.map(str::to_string),
            metadata: metadata.clone(),
            connected_at_ms: now_ms,
            verified_at_ms,
        })
    }

    pub fn list_marketplace_user_connections(
        &self,
        user_id: UserId,
    ) -> Result<Vec<MarketplaceConnectionDto>> {
        self.pool.with_reader(|conn| {
            let mut stmt = conn
                .prepare(
                    "SELECT app_id, kind, name, status, account_label, endpoint, transport, mcp_server_id, metadata_json, discovered_tools_json, last_error, connected_at_ms, verified_at_ms FROM marketplace_user_connections WHERE user_id = ?1 ORDER BY updated_at_ms DESC",
                )
                .map_db()?;
            let rows = stmt
                .query_map(params![user_id.0 as i64], row_to_connection_dto)
                .map_db()?;
            rows.collect::<std::result::Result<Vec<_>, _>>().map_db()
        })
    }

    pub fn get_marketplace_user_connection_with_secret(
        &self,
        user_id: UserId,
        app_id: &str,
    ) -> Result<Option<HydratedMarketplaceConnection>> {
        self.pool.with_reader(|conn| {
            conn.query_row(
                "SELECT app_id, kind, name, status, account_label, endpoint, transport, mcp_server_id, metadata_json, discovered_tools_json, last_error, connected_at_ms, verified_at_ms, credentials_enc_b64, credentials_nonce_b64 FROM marketplace_user_connections WHERE user_id = ?1 AND app_id = ?2 LIMIT 1",
                params![user_id.0 as i64, app_id],
                |r| {
                    let dto = row_to_connection_dto(r)?;
                    let ct: String = r.get(13)?;
                    let nonce: String = r.get(14)?;
                    let secret = decrypt_secret_payload(user_id.0, &ct, &nonce);
                    Ok(HydratedMarketplaceConnection {
                        user_id: user_id.0,
                        dto,
                        secret,
                    })
                },
            )
            .optional()
            .map_db()
        })
    }

    pub fn list_all_active_marketplace_connections_with_secrets(
        &self,
    ) -> Result<Vec<HydratedMarketplaceConnection>> {
        self.pool.with_reader(|conn| {
            let mut stmt = conn
                .prepare(
                    "SELECT app_id, kind, name, status, account_label, endpoint, transport, mcp_server_id, metadata_json, discovered_tools_json, last_error, connected_at_ms, verified_at_ms, credentials_enc_b64, credentials_nonce_b64, user_id FROM marketplace_user_connections WHERE status = 'connected' ORDER BY updated_at_ms ASC",
                )
                .map_db()?;
            let rows = stmt
                .query_map([], |r| {
                    let dto = row_to_connection_dto(r)?;
                    let ct: String = r.get(13)?;
                    let nonce: String = r.get(14)?;
                    let uid: i64 = r.get(15)?;
                    let u = uid.max(0) as u64;
                    let secret = decrypt_secret_payload(u, &ct, &nonce);
                    Ok(HydratedMarketplaceConnection {
                        user_id: u,
                        dto,
                        secret,
                    })
                })
                .map_db()?;
            rows.collect::<std::result::Result<Vec<_>, _>>().map_db()
        })
    }

    pub fn delete_marketplace_user_connection(
        &self,
        user_id: UserId,
        app_id: &str,
    ) -> Result<Option<String>> {
        self.pool.with_writer(|conn| {
            let mcp_server_id: Option<String> = conn
                .query_row(
                    "SELECT mcp_server_id FROM marketplace_user_connections WHERE user_id = ?1 AND app_id = ?2",
                    params![user_id.0 as i64, app_id],
                    |r| r.get(0),
                )
                .optional()
                .map_db()?
                .flatten();
            let removed = conn
                .execute(
                    "DELETE FROM marketplace_user_connections WHERE user_id = ?1 AND app_id = ?2",
                    params![user_id.0 as i64, app_id],
                )
                .map_db()?;
            if removed > 0 {
                Ok(Some(mcp_server_id.unwrap_or_default()))
            } else {
                Ok(None)
            }
        })
    }
}
