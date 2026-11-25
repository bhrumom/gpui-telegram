use crate::db::schema::ALL_MIGRATIONS;
use gpui_telegram_core::error::{Result, TelegramError};
use rusqlite::Connection;

/// Runs all database schema migrations sequentially and idempotently.
pub fn run_migrations(conn: &Connection) -> Result<()> {
    conn.execute_batch(
        r#"
        CREATE TABLE IF NOT EXISTS _schema_migrations (
            version INTEGER PRIMARY KEY,
            applied_at_ms INTEGER NOT NULL
        );
        "#,
    )
    .map_err(|e| TelegramError::Database(format!("Failed to create _schema_migrations: {}", e)))?;

    let essential_already_expected: bool = conn
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM _schema_migrations WHERE version = 1)",
            [],
            |row| row.get(0),
        )
        .unwrap_or(false);

    if essential_already_expected {
        const CORE_TABLES: &[&str] = &[
            "users",
            "sessions",
            "chats",
            "chat_members",
            "forum_topics",
            "messages",
            "pts_sequences",
            "pts_updates",
            "message_dedup",
            "read_states",
        ];
        for table in CORE_TABLES {
            let exists: bool = conn
                .query_row(
                    "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = ?1)",
                    [table],
                    |row| row.get(0),
                )
                .map_err(|e| TelegramError::Database(format!("Schema check failed: {}", e)))?;
            if !exists {
                return Err(TelegramError::Database(format!(
                    "Critical table '{}' is missing from existing database",
                    table
                )));
            }
        }
    }

    let extra_migrations: &[(i64, &str)] = &[
        (39, crate::db::mcp_oauth_store::MIGRATION_V39_MCP_OAUTH),
        (
            40,
            crate::db::remote_assist_store::MIGRATION_V40_REMOTE_ASSIST,
        ),
        (
            41,
            crate::db::creations_store::MIGRATION_V41_CREATIONS_AND_CATALOG,
        ),
    ];

    for (idx, &migration_sql) in ALL_MIGRATIONS.iter().enumerate() {
        let version = (idx + 1) as i64;
        apply_single_migration(conn, version, migration_sql)?;
    }

    for &(version, migration_sql) in extra_migrations {
        apply_single_migration(conn, version, migration_sql)?;
    }

    Ok(())
}

fn apply_single_migration(conn: &Connection, version: i64, migration_sql: &str) -> Result<()> {
    let already_applied: bool = conn
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM _schema_migrations WHERE version = ?1)",
            [version],
            |row| row.get(0),
        )
        .unwrap_or(false);

    if !already_applied {
        if let Err(e) = conn.execute_batch(migration_sql) {
            let err_str = e.to_string();
            if err_str.contains("duplicate column name") {
                for stmt in migration_sql.split(';') {
                    let clean = stmt.trim();
                    if clean.is_empty() {
                        continue;
                    }
                    if let Err(stmt_err) = conn.execute_batch(clean) {
                        if !stmt_err.to_string().contains("duplicate column name") {
                            return Err(TelegramError::Database(format!(
                                "Migration v{} statement failed: {}",
                                version, stmt_err
                            )));
                        }
                    }
                }
            } else {
                return Err(TelegramError::Database(format!(
                    "Migration v{} failed: {}",
                    version, e
                )));
            }
        }

        let now_ms = chrono::Utc::now().timestamp_millis();
        conn.execute(
            "INSERT INTO _schema_migrations (version, applied_at_ms) VALUES (?1, ?2)",
            (version, now_ms),
        )
        .map_err(|e| TelegramError::Database(format!("Failed to record migration v{}: {}", version, e)))?;
    }
    Ok(())
}
