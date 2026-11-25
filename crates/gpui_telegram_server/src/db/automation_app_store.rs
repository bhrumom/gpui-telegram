use crate::db::{ServerDatabase, SqliteResultExt};
use gpui_telegram_core::error::Result;
use rusqlite::{params, OptionalExtension};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

pub const MIGRATION_V41_AUTOMATION_APPS: &str = r#"
CREATE TABLE IF NOT EXISTS automation_apps (
    app_id TEXT PRIMARY KEY,
    slug TEXT NOT NULL,
    author_user_id INTEGER NOT NULL,
    name TEXT NOT NULL,
    description TEXT NOT NULL DEFAULT '',
    category TEXT NOT NULL DEFAULT 'general',
    target_domains_json TEXT NOT NULL DEFAULT '[]',
    latest_version TEXT NOT NULL,
    manifest_json TEXT NOT NULL,
    install_count INTEGER NOT NULL DEFAULT 0,
    run_count INTEGER NOT NULL DEFAULT 0,
    is_published INTEGER NOT NULL DEFAULT 1,
    created_at_ms INTEGER NOT NULL,
    updated_at_ms INTEGER NOT NULL
) STRICT;
CREATE INDEX IF NOT EXISTS idx_automation_apps_published_rank
    ON automation_apps(is_published, install_count DESC, run_count DESC, updated_at_ms DESC);
CREATE INDEX IF NOT EXISTS idx_automation_apps_category
    ON automation_apps(is_published, category, updated_at_ms DESC);
CREATE INDEX IF NOT EXISTS idx_automation_apps_author
    ON automation_apps(author_user_id, updated_at_ms DESC);
CREATE INDEX IF NOT EXISTS idx_automation_apps_slug
    ON automation_apps(slug);

CREATE TABLE IF NOT EXISTS automation_app_versions (
    app_id TEXT NOT NULL REFERENCES automation_apps(app_id) ON DELETE CASCADE,
    version TEXT NOT NULL,
    manifest_json TEXT NOT NULL,
    content_sha256 TEXT NOT NULL,
    created_at_ms INTEGER NOT NULL,
    PRIMARY KEY (app_id, version)
) STRICT;
CREATE INDEX IF NOT EXISTS idx_automation_app_versions_created
    ON automation_app_versions(app_id, created_at_ms DESC);

CREATE TABLE IF NOT EXISTS user_automation_apps (
    user_id INTEGER NOT NULL,
    app_id TEXT NOT NULL REFERENCES automation_apps(app_id) ON DELETE CASCADE,
    installed_version TEXT NOT NULL,
    installed_at_ms INTEGER NOT NULL,
    last_run_at_ms INTEGER,
    PRIMARY KEY (user_id, app_id)
) STRICT;
CREATE INDEX IF NOT EXISTS idx_user_automation_apps_user
    ON user_automation_apps(user_id, installed_at_ms DESC);

CREATE TABLE IF NOT EXISTS automation_app_runs (
    run_id INTEGER PRIMARY KEY AUTOINCREMENT,
    app_id TEXT NOT NULL REFERENCES automation_apps(app_id) ON DELETE CASCADE,
    user_id INTEGER NOT NULL,
    version TEXT NOT NULL,
    success INTEGER NOT NULL,
    steps_executed INTEGER NOT NULL DEFAULT 0,
    total_duration_ms INTEGER NOT NULL DEFAULT 0,
    used_llm INTEGER NOT NULL DEFAULT 0,
    error TEXT,
    recorded_at_ms INTEGER NOT NULL
) STRICT;
CREATE INDEX IF NOT EXISTS idx_automation_app_runs_app_time
    ON automation_app_runs(app_id, recorded_at_ms DESC);
CREATE INDEX IF NOT EXISTS idx_automation_app_runs_user_time
    ON automation_app_runs(user_id, recorded_at_ms DESC);
"#;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AutomationAppRecord {
    pub app_id: String,
    pub slug: String,
    pub author_user_id: i64,
    pub author_username: Option<String>,
    pub name: String,
    pub description: String,
    pub category: String,
    pub target_domains_json: String,
    pub latest_version: String,
    pub manifest_json: String,
    pub install_count: i64,
    pub run_count: i64,
    pub is_published: bool,
    pub created_at_ms: i64,
    pub updated_at_ms: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AutomationAppVersionRecord {
    pub app_id: String,
    pub version: String,
    pub manifest_json: String,
    pub content_sha256: String,
    pub created_at_ms: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct UserAutomationAppRecord {
    pub user_id: i64,
    pub app_id: String,
    pub installed_version: String,
    pub installed_at_ms: i64,
    pub last_run_at_ms: Option<i64>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AutomationAppRunRecord {
    pub run_id: i64,
    pub app_id: String,
    pub user_id: i64,
    pub version: String,
    pub success: bool,
    pub steps_executed: i64,
    pub total_duration_ms: i64,
    pub used_llm: bool,
    pub error: Option<String>,
    pub recorded_at_ms: i64,
}

#[derive(Debug, Clone)]
pub enum PublishAutomationAppOutcome {
    Published {
        app: AutomationAppRecord,
        version: AutomationAppVersionRecord,
        created_new_app: bool,
    },
    ForbiddenOwner,
    DuplicateVersion {
        app_id: String,
        version: String,
    },
}

#[derive(Debug, Clone)]
pub enum InstallAutomationAppOutcome {
    Installed {
        app: AutomationAppRecord,
        install: UserAutomationAppRecord,
        content_sha256: String,
        local_run_count: u64,
        newly_installed: bool,
    },
    NotFound,
}

#[derive(Debug, Clone)]
pub enum RecordAutomationRunOutcome {
    Recorded {
        app: AutomationAppRecord,
        last_run_at_ms: i64,
    },
    NotFound,
    VersionNotFound {
        version: String,
    },
}

pub(crate) fn normalize_domain_host(raw: &str) -> Option<String> {
    let mut s = raw.trim().to_ascii_lowercase();
    if s.is_empty() {
        return None;
    }
    if let Some((_, rest)) = s.split_once("://") {
        s = rest.to_string();
    }
    if let Some((host_port, _)) = s.split_once(['/', '?', '#']) {
        s = host_port.to_string();
    }
    if let Some((_, host_port)) = s.rsplit_once('@') {
        s = host_port.to_string();
    }
    if let Some((host, port)) = s.rsplit_once(':') {
        if port.chars().all(|c| c.is_ascii_digit()) {
            s = host.to_string();
        }
    }
    while let Some(stripped) = s.strip_prefix("*.") {
        s = stripped.to_string();
    }
    let trimmed = s.trim_matches('.').trim();
    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed.to_string())
    }
}

pub(crate) fn domain_matches_target_list(target_domains_json: &str, filter_domain: &str) -> bool {
    let Some(needle) = normalize_domain_host(filter_domain) else {
        return true;
    };
    let Ok(domains) = serde_json::from_str::<Vec<String>>(target_domains_json) else {
        return false;
    };
    domains.iter().any(|entry| {
        normalize_domain_host(entry).is_some_and(|dom| {
            dom == needle
                || dom.ends_with(&format!(".{needle}"))
                || needle.ends_with(&format!(".{dom}"))
        })
    })
}

fn row_to_app_record(row: &rusqlite::Row<'_>) -> rusqlite::Result<AutomationAppRecord> {
    Ok(AutomationAppRecord {
        app_id: row.get(0)?,
        slug: row.get(1)?,
        author_user_id: row.get(2)?,
        author_username: row.get(3)?,
        name: row.get(4)?,
        description: row.get(5)?,
        category: row.get(6)?,
        target_domains_json: row.get(7)?,
        latest_version: row.get(8)?,
        manifest_json: row.get(9)?,
        install_count: row.get(10)?,
        run_count: row.get(11)?,
        is_published: row.get::<_, i64>(12)? != 0,
        created_at_ms: row.get(13)?,
        updated_at_ms: row.get(14)?,
    })
}

fn load_app_record_conn(
    conn: &rusqlite::Connection,
    app_id: &str,
) -> Result<Option<AutomationAppRecord>> {
    conn.query_row(
        r#"
        SELECT a.app_id, a.slug, a.author_user_id, u.username, a.name, a.description,
               a.category, a.target_domains_json, a.latest_version, a.manifest_json,
               a.install_count, a.run_count, a.is_published, a.created_at_ms, a.updated_at_ms
        FROM automation_apps a
        LEFT JOIN users u ON u.user_id = a.author_user_id
        WHERE a.app_id = ?1
        "#,
        params![app_id],
        row_to_app_record,
    )
    .optional()
    .map_db()
}

impl ServerDatabase {
    #[allow(clippy::too_many_arguments)]
    pub fn publish_automation_app(
        &self,
        app_id: &str,
        slug: &str,
        author_user_id: i64,
        name: &str,
        description: &str,
        category: &str,
        target_domains_json: &str,
        version: &str,
        manifest_json: &str,
        is_published: bool,
        now_ms: i64,
    ) -> Result<PublishAutomationAppOutcome> {
        self.pool.with_transaction(|tx| {
            let existing: Option<(i64, i64)> = tx
                .query_row(
                    "SELECT author_user_id, created_at_ms FROM automation_apps WHERE app_id = ?1",
                    params![app_id],
                    |r| Ok((r.get(0)?, r.get(1)?)),
                )
                .optional()
                .map_db()?;

            let created_new_app = match existing {
                Some((existing_author, _)) => {
                    if existing_author != author_user_id {
                        return Ok(PublishAutomationAppOutcome::ForbiddenOwner);
                    }
                    false
                }
                None => true,
            };

            let dup_version: bool = tx
                .query_row(
                    "SELECT EXISTS(SELECT 1 FROM automation_app_versions WHERE app_id = ?1 AND version = ?2)",
                    params![app_id, version],
                    |r| r.get(0),
                )
                .map_db()?;
            if dup_version {
                return Ok(PublishAutomationAppOutcome::DuplicateVersion {
                    app_id: app_id.to_string(),
                    version: version.to_string(),
                });
            }

            let content_sha256 = crate::db::mcp_oauth_store::sha256_hex(manifest_json);
            let created_at_ms = existing.map(|(_, c)| c).unwrap_or(now_ms);
            let is_pub_i64 = if is_published { 1i64 } else { 0i64 };

            tx.execute(
                r#"
                INSERT INTO automation_apps (
                    app_id, slug, author_user_id, name, description, category,
                    target_domains_json, latest_version, manifest_json,
                    install_count, run_count, is_published, created_at_ms, updated_at_ms
                ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, 0, 0, ?10, ?11, ?12)
                ON CONFLICT(app_id) DO UPDATE SET
                    slug = excluded.slug,
                    name = excluded.name,
                    description = excluded.description,
                    category = excluded.category,
                    target_domains_json = excluded.target_domains_json,
                    latest_version = excluded.latest_version,
                    manifest_json = excluded.manifest_json,
                    is_published = excluded.is_published,
                    updated_at_ms = excluded.updated_at_ms
                "#,
                params![
                    app_id,
                    slug,
                    author_user_id,
                    name,
                    description,
                    category,
                    target_domains_json,
                    version,
                    manifest_json,
                    is_pub_i64,
                    created_at_ms,
                    now_ms,
                ],
            )
            .map_db()?;

            tx.execute(
                r#"
                INSERT INTO automation_app_versions (
                    app_id, version, manifest_json, content_sha256, created_at_ms
                ) VALUES (?1, ?2, ?3, ?4, ?5)
                "#,
                params![app_id, version, manifest_json, content_sha256, now_ms],
            )
            .map_db()?;

            let app = load_app_record_conn(tx, app_id)?.ok_or_else(|| {
                gpui_telegram_core::error::TelegramError::Database(
                    "automation_apps row missing immediately after publish".to_string(),
                )
            })?;
            let ver = AutomationAppVersionRecord {
                app_id: app_id.to_string(),
                version: version.to_string(),
                manifest_json: manifest_json.to_string(),
                content_sha256,
                created_at_ms: now_ms,
            };

            Ok(PublishAutomationAppOutcome::Published {
                app,
                version: ver,
                created_new_app,
            })
        })
    }

    pub fn search_automation_apps(
        &self,
        query: &str,
        category: Option<&str>,
        domain: Option<&str>,
        limit: usize,
        offset: usize,
    ) -> Result<(Vec<AutomationAppRecord>, usize)> {
        let q_lower = query.trim().to_ascii_lowercase();
        let cat_lower = category
            .map(str::trim)
            .filter(|c| !c.is_empty() && !c.eq_ignore_ascii_case("all"))
            .map(|c| c.to_ascii_lowercase());
        let dom_filter = domain.and_then(normalize_domain_host);

        self.pool.with_reader(|conn| {
            let mut stmt = conn
                .prepare(
                    r#"
                    SELECT a.app_id, a.slug, a.author_user_id, u.username, a.name, a.description,
                           a.category, a.target_domains_json, a.latest_version, a.manifest_json,
                           a.install_count, a.run_count, a.is_published, a.created_at_ms, a.updated_at_ms
                    FROM automation_apps a
                    LEFT JOIN users u ON u.user_id = a.author_user_id
                    WHERE a.is_published = 1
                    ORDER BY a.install_count DESC, a.run_count DESC, a.updated_at_ms DESC, a.app_id ASC
                    "#,
                )
                .map_db()?;

            let rows = stmt.query_map([], row_to_app_record).map_db()?;
            let mut filtered = Vec::new();
            for row in rows {
                let rec = row.map_db()?;
                if let Some(ref cat) = cat_lower {
                    if rec.category.trim().to_ascii_lowercase() != *cat {
                        continue;
                    }
                }
                if let Some(ref dom) = dom_filter {
                    if !domain_matches_target_list(&rec.target_domains_json, dom) {
                        continue;
                    }
                }
                if !q_lower.is_empty() {
                    let hay = format!(
                        "{} {} {} {} {} {}",
                        rec.app_id,
                        rec.slug,
                        rec.name,
                        rec.description,
                        rec.category,
                        rec.target_domains_json
                    )
                    .to_ascii_lowercase();
                    if !hay.contains(&q_lower) {
                        continue;
                    }
                }
                filtered.push(rec);
            }
            let total = filtered.len();
            let page = filtered.into_iter().skip(offset).take(limit).collect();
            Ok((page, total))
        })
    }

    pub fn get_automation_app(&self, app_id: &str) -> Result<Option<AutomationAppRecord>> {
        self.pool
            .with_reader(|conn| load_app_record_conn(conn, app_id))
    }

    pub fn list_automation_app_versions(
        &self,
        app_id: &str,
    ) -> Result<Vec<AutomationAppVersionRecord>> {
        self.pool.with_reader(|conn| {
            let mut stmt = conn
                .prepare(
                    r#"
                    SELECT app_id, version, manifest_json, content_sha256, created_at_ms
                    FROM automation_app_versions
                    WHERE app_id = ?1
                    ORDER BY created_at_ms DESC, rowid DESC
                    "#,
                )
                .map_db()?;
            let rows = stmt
                .query_map(params![app_id], |r| {
                    Ok(AutomationAppVersionRecord {
                        app_id: r.get(0)?,
                        version: r.get(1)?,
                        manifest_json: r.get(2)?,
                        content_sha256: r.get(3)?,
                        created_at_ms: r.get(4)?,
                    })
                })
                .map_db()?;
            rows.collect::<std::result::Result<Vec<_>, _>>().map_db()
        })
    }

    pub fn is_automation_app_installed_map(
        &self,
        user_id: i64,
    ) -> Result<HashMap<String, UserAutomationAppRecord>> {
        self.pool.with_reader(|conn| {
            let mut stmt = conn
                .prepare(
                    r#"
                    SELECT user_id, app_id, installed_version, installed_at_ms, last_run_at_ms
                    FROM user_automation_apps
                    WHERE user_id = ?1
                    "#,
                )
                .map_db()?;
            let rows = stmt
                .query_map(params![user_id], |r| {
                    Ok(UserAutomationAppRecord {
                        user_id: r.get(0)?,
                        app_id: r.get(1)?,
                        installed_version: r.get(2)?,
                        installed_at_ms: r.get(3)?,
                        last_run_at_ms: r.get(4)?,
                    })
                })
                .map_db()?;
            let mut out = HashMap::new();
            for row in rows {
                let rec = row.map_db()?;
                out.insert(rec.app_id.clone(), rec);
            }
            Ok(out)
        })
    }

    pub fn get_user_local_run_counts(&self, user_id: i64) -> Result<HashMap<String, u64>> {
        self.pool.with_reader(|conn| {
            let mut stmt = conn
                .prepare(
                    r#"
                    SELECT app_id, COUNT(*)
                    FROM automation_app_runs
                    WHERE user_id = ?1
                    GROUP BY app_id
                    "#,
                )
                .map_db()?;
            let rows = stmt
                .query_map(params![user_id], |r| {
                    Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?.max(0) as u64))
                })
                .map_db()?;
            let mut out = HashMap::new();
            for row in rows {
                let (app_id, cnt) = row.map_db()?;
                out.insert(app_id, cnt);
            }
            Ok(out)
        })
    }

    pub fn list_user_installed_automation_apps(
        &self,
        user_id: i64,
    ) -> Result<Vec<(AutomationAppRecord, UserAutomationAppRecord, String, u64)>> {
        let run_counts = self.get_user_local_run_counts(user_id)?;
        self.pool.with_reader(|conn| {
            let mut stmt = conn
                .prepare(
                    r#"
                    SELECT a.app_id, a.slug, a.author_user_id, usr.username, a.name, a.description,
                           a.category, a.target_domains_json, a.latest_version,
                           COALESCE(v.manifest_json, a.manifest_json),
                           a.install_count, a.run_count, a.is_published, a.created_at_ms, a.updated_at_ms,
                           ua.user_id, ua.app_id, ua.installed_version, ua.installed_at_ms, ua.last_run_at_ms,
                           COALESCE(v.content_sha256, '')
                    FROM user_automation_apps ua
                    JOIN automation_apps a ON a.app_id = ua.app_id
                    LEFT JOIN users usr ON usr.user_id = a.author_user_id
                    LEFT JOIN automation_app_versions v
                        ON v.app_id = ua.app_id AND v.version = ua.installed_version
                    WHERE ua.user_id = ?1
                    ORDER BY ua.installed_at_ms DESC, ua.app_id ASC
                    "#,
                )
                .map_db()?;
            let rows = stmt
                .query_map(params![user_id], |r| {
                    let app = row_to_app_record(r)?;
                    let install = UserAutomationAppRecord {
                        user_id: r.get(15)?,
                        app_id: r.get(16)?,
                        installed_version: r.get(17)?,
                        installed_at_ms: r.get(18)?,
                        last_run_at_ms: r.get(19)?,
                    };
                    let sha: String = r.get(20)?;
                    let content_sha256 = if sha.is_empty() {
                        crate::db::mcp_oauth_store::sha256_hex(&app.manifest_json)
                    } else {
                        sha
                    };
                    let local_runs = run_counts.get(&app.app_id).copied().unwrap_or(0);
                    Ok((app, install, content_sha256, local_runs))
                })
                .map_db()?;
            rows.collect::<std::result::Result<Vec<_>, _>>().map_db()
        })
    }

    pub fn install_automation_app(
        &self,
        user_id: i64,
        app_id: &str,
        now_ms: i64,
    ) -> Result<InstallAutomationAppOutcome> {
        self.pool.with_transaction(|tx| {
            let Some(app) = load_app_record_conn(tx, app_id)? else {
                return Ok(InstallAutomationAppOutcome::NotFound);
            };
            if !app.is_published && app.author_user_id != user_id {
                return Ok(InstallAutomationAppOutcome::NotFound);
            }

            let existing: Option<UserAutomationAppRecord> = tx
                .query_row(
                    r#"
                    SELECT user_id, app_id, installed_version, installed_at_ms, last_run_at_ms
                    FROM user_automation_apps
                    WHERE user_id = ?1 AND app_id = ?2
                    "#,
                    params![user_id, app_id],
                    |r| {
                        Ok(UserAutomationAppRecord {
                            user_id: r.get(0)?,
                            app_id: r.get(1)?,
                            installed_version: r.get(2)?,
                            installed_at_ms: r.get(3)?,
                            last_run_at_ms: r.get(4)?,
                        })
                    },
                )
                .optional()
                .map_db()?;

            let newly_installed = existing.is_none();
            if newly_installed {
                tx.execute(
                    r#"
                    INSERT INTO user_automation_apps (
                        user_id, app_id, installed_version, installed_at_ms, last_run_at_ms
                    ) VALUES (?1, ?2, ?3, ?4, NULL)
                    "#,
                    params![user_id, app_id, app.latest_version, now_ms],
                )
                .map_db()?;
                tx.execute(
                    "UPDATE automation_apps SET install_count = install_count + 1, updated_at_ms = ?1 WHERE app_id = ?2",
                    params![now_ms, app_id],
                )
                .map_db()?;
            } else {
                tx.execute(
                    "UPDATE user_automation_apps SET installed_version = ?1 WHERE user_id = ?2 AND app_id = ?3",
                    params![app.latest_version, user_id, app_id],
                )
                .map_db()?;
            }

            let updated_app = load_app_record_conn(tx, app_id)?.ok_or_else(|| {
                gpui_telegram_core::error::TelegramError::Database(
                    "automation_apps row missing after install".to_string(),
                )
            })?;
            let install = tx
                .query_row(
                    r#"
                    SELECT user_id, app_id, installed_version, installed_at_ms, last_run_at_ms
                    FROM user_automation_apps
                    WHERE user_id = ?1 AND app_id = ?2
                    "#,
                    params![user_id, app_id],
                    |r| {
                        Ok(UserAutomationAppRecord {
                            user_id: r.get(0)?,
                            app_id: r.get(1)?,
                            installed_version: r.get(2)?,
                            installed_at_ms: r.get(3)?,
                            last_run_at_ms: r.get(4)?,
                        })
                    },
                )
                .map_db()?;
            let content_sha256: String = tx
                .query_row(
                    "SELECT content_sha256 FROM automation_app_versions WHERE app_id = ?1 AND version = ?2",
                    params![app_id, install.installed_version],
                    |r| r.get(0),
                )
                .optional()
                .map_db()?
                .unwrap_or_else(|| {
                    crate::db::mcp_oauth_store::sha256_hex(&updated_app.manifest_json)
                });
            let local_run_count: u64 = tx
                .query_row(
                    "SELECT COUNT(*) FROM automation_app_runs WHERE user_id = ?1 AND app_id = ?2",
                    params![user_id, app_id],
                    |r| Ok(r.get::<_, i64>(0)?.max(0) as u64),
                )
                .map_db()?;

            Ok(InstallAutomationAppOutcome::Installed {
                app: updated_app,
                install,
                content_sha256,
                local_run_count,
                newly_installed,
            })
        })
    }

    pub fn uninstall_automation_app(
        &self,
        user_id: i64,
        app_id: &str,
        now_ms: i64,
    ) -> Result<Option<bool>> {
        self.pool.with_transaction(|tx| {
            let app_row: Option<(i64, bool)> = tx
                .query_row(
                    "SELECT author_user_id, is_published != 0 FROM automation_apps WHERE app_id = ?1",
                    params![app_id],
                    |r| Ok((r.get(0)?, r.get(1)?)),
                )
                .optional()
                .map_db()?;
            let Some((author_user_id, is_published)) = app_row else {
                return Ok(None);
            };
            let deleted = tx
                .execute(
                    "DELETE FROM user_automation_apps WHERE user_id = ?1 AND app_id = ?2",
                    params![user_id, app_id],
                )
                .map_db()?;
            if deleted > 0 {
                tx.execute(
                    "UPDATE automation_apps SET install_count = MAX(0, install_count - 1), updated_at_ms = ?1 WHERE app_id = ?2",
                    params![now_ms, app_id],
                )
                .map_db()?;
                return Ok(Some(true));
            }
            if !is_published && author_user_id != user_id {
                return Ok(None);
            }
            Ok(Some(false))
        })
    }

    #[allow(clippy::too_many_arguments)]
    pub fn record_automation_app_run(
        &self,
        user_id: i64,
        app_id: &str,
        version: Option<&str>,
        success: bool,
        steps_executed: usize,
        total_duration_ms: u64,
        used_llm: bool,
        error: Option<&str>,
        now_ms: i64,
    ) -> Result<RecordAutomationRunOutcome> {
        self.pool.with_transaction(|tx| {
            let Some(app) = load_app_record_conn(tx, app_id)? else {
                return Ok(RecordAutomationRunOutcome::NotFound);
            };
            if !app.is_published && app.author_user_id != user_id {
                return Ok(RecordAutomationRunOutcome::NotFound);
            }

            let run_version =
                if let Some(explicit_ver) = version.map(str::trim).filter(|v| !v.is_empty()) {
                    let exists: bool = tx
                        .query_row(
                            "SELECT EXISTS(SELECT 1 FROM automation_app_versions WHERE app_id = ?1 AND version = ?2)",
                            params![app_id, explicit_ver],
                            |r| r.get(0),
                        )
                        .map_db()?;
                    if !exists {
                        return Ok(RecordAutomationRunOutcome::VersionNotFound {
                            version: explicit_ver.to_string(),
                        });
                    }
                    explicit_ver
                } else {
                    app.latest_version.as_str()
                };

            tx.execute(
                r#"
                INSERT INTO automation_app_runs (
                    app_id, user_id, version, success, steps_executed,
                    total_duration_ms, used_llm, error, recorded_at_ms
                ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)
                "#,
                params![
                    app_id,
                    user_id,
                    run_version,
                    if success { 1i64 } else { 0i64 },
                    steps_executed.min(i64::MAX as usize) as i64,
                    total_duration_ms.min(i64::MAX as u64) as i64,
                    if used_llm { 1i64 } else { 0i64 },
                    error,
                    now_ms,
                ],
            )
            .map_db()?;

            tx.execute(
                "UPDATE automation_apps SET run_count = run_count + 1, updated_at_ms = ?1 WHERE app_id = ?2",
                params![now_ms, app_id],
            )
            .map_db()?;

            tx.execute(
                "UPDATE user_automation_apps SET last_run_at_ms = ?1 WHERE user_id = ?2 AND app_id = ?3",
                params![now_ms, user_id, app_id],
            )
            .map_db()?;

            let updated_app = load_app_record_conn(tx, app_id)?.ok_or_else(|| {
                gpui_telegram_core::error::TelegramError::Database(
                    "automation_apps row missing after run record".to_string(),
                )
            })?;

            Ok(RecordAutomationRunOutcome::Recorded {
                app: updated_app,
                last_run_at_ms: now_ms,
            })
        })
    }

    pub fn list_automation_app_runs(
        &self,
        app_id: &str,
        limit: usize,
    ) -> Result<Vec<AutomationAppRunRecord>> {
        self.pool.with_reader(|conn| {
            let mut stmt = conn
                .prepare(
                    r#"
                    SELECT run_id, app_id, user_id, version, success, steps_executed,
                           total_duration_ms, used_llm, error, recorded_at_ms
                    FROM automation_app_runs
                    WHERE app_id = ?1
                    ORDER BY recorded_at_ms DESC, run_id DESC
                    LIMIT ?2
                    "#,
                )
                .map_db()?;
            let rows = stmt
                .query_map(params![app_id, limit as i64], |r| {
                    Ok(AutomationAppRunRecord {
                        run_id: r.get(0)?,
                        app_id: r.get(1)?,
                        user_id: r.get(2)?,
                        version: r.get(3)?,
                        success: r.get::<_, i64>(4)? != 0,
                        steps_executed: r.get(5)?,
                        total_duration_ms: r.get(6)?,
                        used_llm: r.get::<_, i64>(7)? != 0,
                        error: r.get(8)?,
                        recorded_at_ms: r.get(9)?,
                    })
                })
                .map_db()?;
            rows.collect::<std::result::Result<Vec<_>, _>>().map_db()
        })
    }
}
