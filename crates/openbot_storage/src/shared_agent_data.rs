//! `SharedAgentDataDb` (`<data_dir>/Shared/Data/agent-data.db`) — Supervised multi-agent SQLite
//! database (`<= 64` tables via `INPUT_LIMITS.shared_tables`, `256 MiB` `max_page_count = 65536`,
//! `5_000ms` `progress_handler` deadline, `SQLITE_DBCONFIG_DEFENSIVE`, stateful SQL/parameter
//! scanner, and `rusqlite::Connection::authorizer` enforcing `owner_agent_id` via dual-synced
//! `_openbot_tables` + `openbot_metadata`).
//!
//! Mirrors `src/backend/agent-data/{agent-tables.ts,agent-database-rules.ts,agent-database-protocol.ts,agent-database-host.ts}`.

use std::collections::{HashMap, HashSet};
use std::fmt;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use openbot_contracts::{INPUT_LIMITS, utf16_len};
use parking_lot::Mutex;
use rusqlite::config::DbConfig;
use rusqlite::hooks::{AuthAction, AuthContext, Authorization};
use rusqlite::types::{ToSqlOutput, Value as SqlValue, ValueRef};
use rusqlite::{Connection, ErrorCode, OptionalExtension, ToSql, params, params_from_iter};
use serde::de::Error as DeError;
use serde::ser::SerializeMap;
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use serde_json::Value as JsonValue;

use crate::database::now_rfc3339_millis;
use crate::error::{StorageError, StorageResult};
use crate::schema_v69::assert_quick_check;

pub const AGENT_DATABASE_SCHEMA_VERSION: i64 = 1;
pub const PRIMARY_METADATA_TABLE: &str = "_openbot_tables";
pub const UPSTREAM_METADATA_TABLE: &str = "openbot_metadata";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AgentDatabaseLimits {
    pub max_sql_length: usize,
    pub max_parameters: usize,
    pub max_parameter_bytes: usize,
    pub max_rows: usize,
    pub max_result_bytes: usize,
    pub max_cell_chars: usize,
    pub max_page_count: usize,
    pub statement_deadline_ms: u64,
    pub busy_timeout_ms: u64,
    pub max_shared_tables: usize,
}

pub const AGENT_DATABASE_LIMITS: AgentDatabaseLimits = AgentDatabaseLimits {
    max_sql_length: 20_000,
    max_parameters: 100,
    max_parameter_bytes: 1_000_000,
    max_rows: 500,
    max_result_bytes: 256_000,
    max_cell_chars: 2_000,
    max_page_count: 65_536,
    statement_deadline_ms: 5_000,
    busy_timeout_ms: 2_000,
    max_shared_tables: INPUT_LIMITS.shared_tables,
};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum AgentSqlMode {
    Read,
    Write,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum AgentSchemaChange {
    Create,
    Drop,
    Alter,
}

#[derive(Clone, Debug, PartialEq)]
pub enum AgentSqlParam {
    Null,
    Integer(i64),
    Real(f64),
    Text(String),
}

impl ToSql for AgentSqlParam {
    fn to_sql(&self) -> rusqlite::Result<ToSqlOutput<'_>> {
        match self {
            Self::Null => Ok(ToSqlOutput::Owned(SqlValue::Null)),
            Self::Integer(v) => Ok(ToSqlOutput::Owned(SqlValue::Integer(*v))),
            Self::Real(v) => Ok(ToSqlOutput::Owned(SqlValue::Real(*v))),
            Self::Text(s) => Ok(ToSqlOutput::Borrowed(ValueRef::Text(s.as_bytes()))),
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum AgentCellValue {
    Null,
    Integer(i64),
    Real(f64),
    Text(String),
    Blob { blob_bytes: usize },
}

impl Serialize for AgentCellValue {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        match self {
            Self::Null => serializer.serialize_none(),
            Self::Integer(v) => serializer.serialize_i64(*v),
            Self::Real(v) => serializer.serialize_f64(*v),
            Self::Text(s) => serializer.serialize_str(s),
            Self::Blob { blob_bytes } => {
                let mut map = serializer.serialize_map(Some(1))?;
                map.serialize_entry("blobBytes", blob_bytes)?;
                map.end()
            }
        }
    }
}

impl<'de> Deserialize<'de> for AgentCellValue {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let value = JsonValue::deserialize(deserializer)?;
        match value {
            JsonValue::Null => Ok(Self::Null),
            JsonValue::Number(num) => {
                if let Some(i) = num.as_i64() {
                    Ok(Self::Integer(i))
                } else if let Some(f) = num.as_f64() {
                    Ok(Self::Real(f))
                } else {
                    Err(D::Error::custom("unsupported numeric cell value"))
                }
            }
            JsonValue::String(s) => Ok(Self::Text(s)),
            JsonValue::Object(obj) if obj.len() == 1 && obj.contains_key("blobBytes") => {
                let n = obj
                    .get("blobBytes")
                    .and_then(JsonValue::as_u64)
                    .ok_or_else(|| D::Error::custom("blobBytes must be a non-negative integer"))?;
                Ok(Self::Blob {
                    blob_bytes: n as usize,
                })
            }
            _ => Err(D::Error::custom("invalid AgentCellValue JSON representation")),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentTableSummary {
    pub name: String,
    pub owner_agent_id: Option<String>,
    pub row_count: Option<i64>,
    pub sql: String,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentQueryResult {
    pub columns: Vec<String>,
    pub rows: Vec<Vec<AgentCellValue>>,
    pub truncated: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentWriteResult {
    pub changes: usize,
    pub last_insert_rowid: Option<i64>,
}

struct ScannedStatement {
    words: Vec<String>,
    ddl_header_identifiers: Vec<String>,
    has_trailing_statement: bool,
    unterminated: bool,
    placeholders: Option<usize>,
}

const TOK_SEMI: usize = 0;
const STATE_START: usize = 1;
const TOK_OTHER: usize = 2;
const TOK_EXPLAIN: usize = 3;
const TOK_CREATE: usize = 4;
const TOK_TEMP: usize = 5;
const TOK_TRIGGER: usize = 6;
const TOK_END: usize = 7;

const TRANSITIONS: [[usize; 8]; 8] = [
    [1, 0, 2, 3, 4, 2, 2, 2],
    [1, 1, 2, 3, 4, 2, 2, 2],
    [1, 2, 2, 2, 2, 2, 2, 2],
    [1, 3, 3, 2, 4, 2, 2, 2],
    [1, 4, 2, 2, 2, 4, 5, 2],
    [6, 5, 5, 5, 5, 5, 5, 5],
    [6, 6, 5, 5, 5, 5, 5, 7],
    [1, 7, 5, 5, 5, 5, 5, 5],
];

fn keyword_token(word: &str) -> usize {
    match word {
        "explain" => TOK_EXPLAIN,
        "create" => TOK_CREATE,
        "temp" | "temporary" => TOK_TEMP,
        "trigger" => TOK_TRIGGER,
        "end" => TOK_END,
        _ => TOK_OTHER,
    }
}

fn is_sql_ident_start(ch: char) -> bool {
    ch.is_ascii_alphabetic() || ch == '_' || (ch as u32) >= 0x80
}

fn is_sql_ident_continue(ch: char) -> bool {
    is_sql_ident_start(ch) || ch.is_ascii_digit() || ch == '$'
}

fn scan_statement(sql: &str) -> ScannedStatement {
    let chars: Vec<char> = sql.chars().collect();
    let len = chars.len();
    let mut words = Vec::new();
    let mut ddl_header_identifiers = Vec::new();
    let mut in_ddl_header = true;
    let mut depth = 0usize;
    let mut index = 0usize;
    let mut state = 0usize;
    let mut ended = false;
    let mut has_trailing_statement = false;
    let mut placeholders: Option<usize> = Some(0);

    let mut see = |token: usize| {
        if ended && token != TOK_SEMI {
            has_trailing_statement = true;
        }
        state = TRANSITIONS[state][token];
        if state == STATE_START {
            ended = true;
        }
    };

    while index < len {
        let ch = chars[index];
        if ch == '?' {
            if let Some(count) = placeholders {
                if chars.get(index + 1).is_some_and(char::is_ascii_digit) {
                    placeholders = None;
                } else {
                    placeholders = Some(count + 1);
                }
            }
            see(TOK_OTHER);
            index += 1;
            continue;
        }
        if matches!(ch, ':' | '@' | '$') {
            placeholders = None;
        }
        if ch == '-' && chars.get(index + 1) == Some(&'-') {
            index += 2;
            while index < len && chars[index] != '\n' {
                index += 1;
            }
            if index < len {
                index += 1;
            }
            continue;
        }
        if ch == '/' && chars.get(index + 1) == Some(&'*') {
            index += 2;
            let mut closed = false;
            while index + 1 < len {
                if chars[index] == '*' && chars[index + 1] == '/' {
                    index += 2;
                    closed = true;
                    break;
                }
                index += 1;
            }
            if !closed {
                return ScannedStatement {
                    words,
                    ddl_header_identifiers,
                    has_trailing_statement,
                    unterminated: true,
                    placeholders: None,
                };
            }
            continue;
        }
        if matches!(ch, 'x' | 'X') && chars.get(index + 1) == Some(&'\'') {
            index += 2;
            let mut closed = false;
            while index < len {
                if chars[index] == '\'' {
                    index += 1;
                    closed = true;
                    break;
                }
                index += 1;
            }
            if !closed {
                return ScannedStatement {
                    words,
                    ddl_header_identifiers,
                    has_trailing_statement,
                    unterminated: true,
                    placeholders: None,
                };
            }
            see(TOK_OTHER);
            continue;
        }
        if matches!(ch, '\'' | '"' | '`') {
            let quote = ch;
            index += 1;
            let mut ident = String::new();
            let mut closed = false;
            while index < len {
                if chars[index] == quote {
                    if chars.get(index + 1) == Some(&quote) {
                        ident.push(quote);
                        index += 2;
                        continue;
                    }
                    index += 1;
                    closed = true;
                    break;
                }
                ident.push(chars[index]);
                index += 1;
            }
            if !closed {
                return ScannedStatement {
                    words,
                    ddl_header_identifiers,
                    has_trailing_statement,
                    unterminated: true,
                    placeholders: None,
                };
            }
            if depth == 0 && in_ddl_header && matches!(quote, '"' | '`') {
                ddl_header_identifiers.push(ident.to_ascii_lowercase());
            }
            see(TOK_OTHER);
            continue;
        }
        if ch == '[' {
            index += 1;
            let start = index;
            while index < len && chars[index] != ']' {
                index += 1;
            }
            if index >= len {
                return ScannedStatement {
                    words,
                    ddl_header_identifiers,
                    has_trailing_statement,
                    unterminated: true,
                    placeholders: None,
                };
            }
            if depth == 0 && in_ddl_header {
                let ident: String = chars[start..index].iter().collect();
                ddl_header_identifiers.push(ident.to_ascii_lowercase());
            }
            index += 1;
            see(TOK_OTHER);
            continue;
        }
        if ch == '(' {
            if depth == 0 {
                in_ddl_header = false;
            }
            see(TOK_OTHER);
            depth += 1;
            index += 1;
            continue;
        }
        if ch == ')' {
            see(TOK_OTHER);
            depth = depth.saturating_sub(1);
            index += 1;
            continue;
        }
        if ch == ';' {
            in_ddl_header = false;
            see(TOK_SEMI);
            index += 1;
            continue;
        }
        if is_sql_ident_start(ch) {
            let start = index;
            while index < len && is_sql_ident_continue(chars[index]) {
                index += 1;
            }
            let word: String = chars[start..index].iter().collect::<String>().to_lowercase();
            see(keyword_token(&word));
            if depth == 0 {
                if matches!(word.as_str(), "as" | "using" | "begin") {
                    in_ddl_header = false;
                } else if in_ddl_header {
                    ddl_header_identifiers.push(word.clone());
                }
                words.push(word);
            }
            continue;
        }
        if !ch.is_whitespace() {
            see(TOK_OTHER);
        }
        index += 1;
    }

    ScannedStatement {
        words,
        ddl_header_identifiers,
        has_trailing_statement,
        unterminated: false,
        placeholders,
    }
}

fn refusal_message(keyword: &str) -> Option<&'static str> {
    match keyword {
        "attach" => {
            Some("ATTACH is not available. Every table is already in the one shared database.")
        }
        "detach" => Some("DETACH is not available."),
        "vacuum" => Some("VACUUM is not available."),
        "pragma" => Some("PRAGMA is not available. Use list_tables to read the schema of a table."),
        "analyze" => Some("ANALYZE is not available."),
        "reindex" => Some("REINDEX is not available."),
        "begin" | "commit" | "end" | "rollback" | "savepoint" | "release" => {
            Some("Each call runs one statement in its own transaction.")
        }
        _ => None,
    }
}

fn is_read_keyword(word: &str) -> bool {
    matches!(word, "select" | "values" | "explain")
}

fn is_write_keyword(word: &str) -> bool {
    matches!(
        word,
        "insert" | "replace" | "update" | "delete" | "create" | "alter" | "drop"
    )
}

fn is_with_target(word: &str) -> bool {
    matches!(
        word,
        "select" | "values" | "insert" | "replace" | "update" | "delete"
    )
}

fn is_metadata_table(name: &str) -> bool {
    name.eq_ignore_ascii_case(PRIMARY_METADATA_TABLE)
        || name.eq_ignore_ascii_case(UPSTREAM_METADATA_TABLE)
}

fn is_internal_schema_catalog(name: &str) -> bool {
    matches!(
        name.to_ascii_lowercase().as_str(),
        "sqlite_master" | "sqlite_schema" | "sqlite_temp_master" | "sqlite_temp_schema"
    )
}

pub fn check_agent_sql(sql: &str, mode: AgentSqlMode) -> StorageResult<String> {
    if sql.trim().is_empty() {
        return Err(StorageError::validation("Send a SQL statement."));
    }
    if utf16_len(sql) > AGENT_DATABASE_LIMITS.max_sql_length {
        return Err(StorageError::limit_exceeded(format!(
            "A statement may be at most {} characters.",
            AGENT_DATABASE_LIMITS.max_sql_length
        )));
    }

    let scanned = scan_statement(sql);
    if scanned.unterminated {
        return Err(StorageError::validation(
            "This statement has an unterminated string, identifier, or comment.",
        ));
    }
    if scanned.has_trailing_statement {
        return Err(StorageError::validation(
            "Send one SQL statement per call. Everything after the first statement would be ignored.",
        ));
    }

    let Some(first) = scanned.words.first().map(String::as_str) else {
        return Err(StorageError::validation("Send a SQL statement."));
    };
    if let Some(refusal) = refusal_message(first) {
        return Err(StorageError::forbidden(refusal));
    }

    let effective = if first == "with" {
        scanned
            .words
            .iter()
            .skip(1)
            .map(String::as_str)
            .find(|w| is_with_target(w))
    } else {
        Some(first)
    };
    let Some(effective) = effective else {
        return Err(StorageError::validation(
            "A WITH clause must end in SELECT, INSERT, UPDATE, or DELETE.",
        ));
    };

    match mode {
        AgentSqlMode::Read => {
            if is_read_keyword(effective) {
                return Ok(sql.to_string());
            }
            if is_write_keyword(effective) {
                return Err(StorageError::validation(format!(
                    "Use execute_data for {}. query_data only reads.",
                    effective.to_ascii_uppercase()
                )));
            }
            return Err(StorageError::validation(format!(
                "{} is not available. query_data runs SELECT.",
                effective.to_ascii_uppercase()
            )));
        }
        AgentSqlMode::Write => {
            if is_write_keyword(effective) {
                if matches!(first, "create" | "alter" | "drop") {
                    for ident in &scanned.ddl_header_identifiers {
                        if is_metadata_table(ident) || ident.starts_with("sqlite_") {
                            return Err(StorageError::forbidden(
                                "SQLite internal and OpenBot metadata tables cannot be created, altered, or dropped.",
                            ));
                        }
                    }
                }
                return Ok(sql.to_string());
            }
            if is_read_keyword(effective) {
                return Err(StorageError::validation(
                    "Use query_data to read rows. execute_data changes data.",
                ));
            }
            return Err(StorageError::validation(format!(
                "{} is not available.",
                effective.to_ascii_uppercase()
            )));
        }
    }
}

pub fn schema_change(sql: &str) -> Option<AgentSchemaChange> {
    match scan_statement(sql).words.first().map(String::as_str) {
        Some("create") => Some(AgentSchemaChange::Create),
        Some("drop") => Some(AgentSchemaChange::Drop),
        Some("alter") => Some(AgentSchemaChange::Alter),
        _ => None,
    }
}

pub fn check_agent_parameters(
    value: Option<&JsonValue>,
    sql: &str,
) -> StorageResult<Vec<AgentSqlParam>> {
    let items: &[JsonValue] = match value {
        None | Some(JsonValue::Null) => &[],
        Some(JsonValue::Array(arr)) => arr.as_slice(),
        Some(_) => {
            return Err(StorageError::validation(
                "params must be an array of values, one for each ? placeholder in order.",
            ));
        }
    };
    if items.len() > AGENT_DATABASE_LIMITS.max_parameters {
        return Err(StorageError::limit_exceeded(format!(
            "A statement may take at most {} parameters.",
            AGENT_DATABASE_LIMITS.max_parameters
        )));
    }

    let mut params_out = Vec::with_capacity(items.len());
    let mut total_units = 0usize;
    for entry in items {
        match entry {
            JsonValue::Null => params_out.push(AgentSqlParam::Null),
            JsonValue::Bool(b) => params_out.push(AgentSqlParam::Integer(i64::from(*b))),
            JsonValue::Number(num) => {
                if let Some(i) = num.as_i64() {
                    params_out.push(AgentSqlParam::Integer(i));
                } else if let Some(f) = num.as_f64()
                    && f.is_finite()
                {
                    params_out.push(AgentSqlParam::Real(f));
                } else {
                    return Err(StorageError::validation("A number parameter must be finite."));
                }
            }
            JsonValue::String(s) => {
                total_units += utf16_len(s);
                params_out.push(AgentSqlParam::Text(s.clone()));
            }
            JsonValue::Array(_) | JsonValue::Object(_) => {
                return Err(StorageError::validation(
                    "Parameters must be text, numbers, booleans, or null.",
                ));
            }
        }
    }
    if total_units > AGENT_DATABASE_LIMITS.max_parameter_bytes {
        return Err(StorageError::limit_exceeded(
            "The parameters together are too large. Write fewer rows per call.",
        ));
    }

    let scanned = scan_statement(sql);
    let expected = if scanned.unterminated || scanned.has_trailing_statement {
        None
    } else {
        scanned.placeholders
    };
    if let Some(exp) = expected
        && exp != params_out.len()
    {
        let p_label = if exp == 1 {
            "1 ? placeholder".to_string()
        } else {
            format!("{exp} ? placeholders")
        };
        let v_label = if params_out.len() == 1 {
            "1 value".to_string()
        } else {
            format!("{} values", params_out.len())
        };
        return Err(StorageError::validation(format!(
            "This statement has {p_label}, but params has {v_label}. Give one value for each ? in order."
        )));
    }
    Ok(params_out)
}

#[derive(Clone, Debug, Default)]
pub enum AuthorizerContext {
    #[default]
    DenyAll,
    HostAdmin,
    Agent {
        agent_id: String,
        mode: AgentSqlMode,
        in_ddl: bool,
        table_owners: HashMap<String, String>,
    },
}

fn is_forbidden_sqlite_dml_table(name: &str, in_ddl: bool) -> bool {
    let lower = name.to_ascii_lowercase();
    if !lower.starts_with("sqlite_") {
        return false;
    }
    if in_ddl && (is_internal_schema_catalog(&lower) || lower == "sqlite_sequence") {
        return false;
    }
    true
}

pub fn install_shared_db_guards(
    conn: &Connection,
    auth_state: Arc<Mutex<AuthorizerContext>>,
    deadline_state: Arc<Mutex<Option<Instant>>>,
) -> StorageResult<()> {
    conn.set_db_config(DbConfig::SQLITE_DBCONFIG_ENABLE_FKEY, true)?;
    conn.set_db_config(DbConfig::SQLITE_DBCONFIG_DEFENSIVE, true)?;
    conn.set_db_config(DbConfig::SQLITE_DBCONFIG_DQS_DML, false)?;
    conn.set_db_config(DbConfig::SQLITE_DBCONFIG_DQS_DDL, false)?;
    conn.set_db_config(DbConfig::SQLITE_DBCONFIG_ENABLE_FTS3_TOKENIZER, false)?;
    conn.set_db_config(DbConfig::SQLITE_DBCONFIG_TRUSTED_SCHEMA, false)?;
    conn.busy_timeout(Duration::from_millis(AGENT_DATABASE_LIMITS.busy_timeout_ms))?;

    conn.progress_handler(
        1_000,
        Some(move || deadline_state.lock().is_some_and(|dl| Instant::now() >= dl)),
    )?;

    conn.authorizer(Some(move |ctx: AuthContext<'_>| -> Authorization {
        let guard = auth_state.lock();
        let (caller_id, mode, in_ddl, owners) = match &*guard {
            AuthorizerContext::HostAdmin => return Authorization::Allow,
            AuthorizerContext::DenyAll => return Authorization::Deny,
            AuthorizerContext::Agent {
                agent_id,
                mode,
                in_ddl,
                table_owners,
            } => (agent_id.as_str(), *mode, *in_ddl, table_owners),
        };

        let owned_by_other = |table: &str| -> bool {
            let lower = table.to_ascii_lowercase();
            is_metadata_table(&lower)
                || lower.starts_with("sqlite_")
                || owners
                    .get(&lower)
                    .is_some_and(|owner| owner != caller_id)
        };

        match ctx.action {
            AuthAction::Attach { .. } | AuthAction::Detach { .. } => Authorization::Deny,
            AuthAction::CreateTempTrigger { .. } | AuthAction::Analyze { .. } => {
                Authorization::Deny
            }
            AuthAction::Reindex { .. } => {
                if in_ddl && mode == AgentSqlMode::Write {
                    Authorization::Allow
                } else {
                    Authorization::Deny
                }
            }
            AuthAction::Pragma {
                pragma_name,
                pragma_value,
            } => {
                if pragma_value.is_none()
                    && (pragma_name.eq_ignore_ascii_case("data_version")
                        || pragma_name.eq_ignore_ascii_case("page_size"))
                {
                    Authorization::Allow
                } else {
                    Authorization::Deny
                }
            }
            AuthAction::Function { function_name } => {
                if matches!(
                    function_name.to_ascii_lowercase().as_str(),
                    "load_extension" | "readfile" | "writefile" | "edit" | "fts3_tokenizer"
                ) {
                    Authorization::Deny
                } else {
                    Authorization::Allow
                }
            }
            AuthAction::Select | AuthAction::Recursive => Authorization::Allow,
            AuthAction::Read { table_name, .. } => {
                if is_metadata_table(table_name) {
                    Authorization::Deny
                } else {
                    Authorization::Allow
                }
            }
            AuthAction::Insert { table_name }
            | AuthAction::Update { table_name, .. }
            | AuthAction::Delete { table_name } => {
                if mode == AgentSqlMode::Read
                    || is_metadata_table(table_name)
                    || is_forbidden_sqlite_dml_table(table_name, in_ddl)
                {
                    Authorization::Deny
                } else {
                    Authorization::Allow
                }
            }
            AuthAction::CreateTable { table_name } => {
                let lower = table_name.to_ascii_lowercase();
                let internal_seq = in_ddl && lower == "sqlite_sequence";
                if mode == AgentSqlMode::Read
                    || is_metadata_table(&lower)
                    || (lower.starts_with("sqlite_") && !internal_seq)
                {
                    Authorization::Deny
                } else {
                    Authorization::Allow
                }
            }
            AuthAction::CreateTempTable { table_name }
            | AuthAction::CreateView {
                view_name: table_name,
            }
            | AuthAction::CreateTempView {
                view_name: table_name,
            } => {
                let lower = table_name.to_ascii_lowercase();
                if mode == AgentSqlMode::Read
                    || is_metadata_table(&lower)
                    || lower.starts_with("sqlite_")
                {
                    Authorization::Deny
                } else {
                    Authorization::Allow
                }
            }
            AuthAction::CreateVtable {
                table_name,
                module_name,
            } => {
                let lower = table_name.to_ascii_lowercase();
                if mode == AgentSqlMode::Read
                    || is_metadata_table(&lower)
                    || lower.starts_with("sqlite_")
                    || !matches!(
                        module_name.to_ascii_lowercase().as_str(),
                        "fts5" | "fts5vocab" | "rtree" | "rtreecheck"
                    )
                {
                    Authorization::Deny
                } else {
                    Authorization::Allow
                }
            }
            AuthAction::AlterTable { table_name, .. }
            | AuthAction::DropTable { table_name }
            | AuthAction::DropTempTable { table_name }
            | AuthAction::DropVtable { table_name, .. }
            | AuthAction::DropView {
                view_name: table_name,
            }
            | AuthAction::DropTempView {
                view_name: table_name,
            }
            | AuthAction::CreateIndex { table_name, .. }
            | AuthAction::DropIndex { table_name, .. }
            | AuthAction::CreateTempIndex { table_name, .. }
            | AuthAction::DropTempIndex { table_name, .. }
            | AuthAction::CreateTrigger { table_name, .. }
            | AuthAction::DropTrigger { table_name, .. }
            | AuthAction::DropTempTrigger { table_name, .. } => {
                if mode == AgentSqlMode::Read || owned_by_other(table_name) {
                    Authorization::Deny
                } else {
                    Authorization::Allow
                }
            }
            AuthAction::Transaction { .. } | AuthAction::Savepoint { .. } => Authorization::Deny,
            _ => Authorization::Deny,
        }
    }))?;

    Ok(())
}

const HIDDEN_TABLES_PREDICATE: &str =
    "name NOT LIKE 'sqlite~_%' ESCAPE '~' AND lower(name) NOT IN ('_openbot_tables', 'openbot_metadata')";

struct SharedAgentDataInner {
    db_path: PathBuf,
    conn: Mutex<Connection>,
    auth_state: Arc<Mutex<AuthorizerContext>>,
    deadline_state: Arc<Mutex<Option<Instant>>>,
}

#[derive(Clone)]
pub struct SharedAgentDataDb {
    inner: Arc<SharedAgentDataInner>,
}

impl fmt::Debug for SharedAgentDataDb {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("SharedAgentDataDb")
            .field("db_path", &self.inner.db_path)
            .finish()
    }
}

struct ExecutionScopeGuard<'a> {
    auth_state: &'a Mutex<AuthorizerContext>,
    deadline_state: &'a Mutex<Option<Instant>>,
}

impl Drop for ExecutionScopeGuard<'_> {
    fn drop(&mut self) {
        *self.deadline_state.lock() = None;
        *self.auth_state.lock() = AuthorizerContext::DenyAll;
    }
}

fn resolve_agent_data_path(path: &Path) -> PathBuf {
    if path.is_file() {
        return path.to_path_buf();
    }
    let looks_like_db = path
        .extension()
        .and_then(|ext| ext.to_str())
        .is_some_and(|ext| {
            ext.eq_ignore_ascii_case("db")
                || ext.eq_ignore_ascii_case("sqlite")
                || ext.eq_ignore_ascii_case("sqlite3")
        });
    if looks_like_db {
        return path.to_path_buf();
    }
    let file_name = path.file_name().and_then(|n| n.to_str());
    if file_name == Some("Shared") {
        return path.join("Data").join("agent-data.db");
    }
    if file_name == Some("Data")
        && path.parent().and_then(|p| p.file_name()).and_then(|n| n.to_str()) == Some("Shared")
    {
        return path.join("agent-data.db");
    }
    path.join("Shared").join("Data").join("agent-data.db")
}

fn create_secure_dir(dir: &Path) -> StorageResult<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::{DirBuilderExt, PermissionsExt};
        let existed = dir.exists();
        let mut builder = fs::DirBuilder::new();
        builder.recursive(true).mode(0o700);
        builder.create(dir)?;
        if !existed {
            let _ = fs::set_permissions(dir, fs::Permissions::from_mode(0o700));
        }
    }
    #[cfg(not(unix))]
    {
        fs::create_dir_all(dir)?;
    }
    Ok(())
}

fn set_secure_file_mode(path: &Path) -> StorageResult<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(path, fs::Permissions::from_mode(0o600))?;
    }
    #[cfg(not(unix))]
    {
        let _ = path;
    }
    Ok(())
}

fn provision_metadata_tables(conn: &Connection) -> StorageResult<()> {
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS _openbot_tables (\
           table_name TEXT PRIMARY KEY COLLATE NOCASE,\
           owner_agent_id TEXT NOT NULL,\
           created_at TEXT NOT NULL\
         );\
         CREATE TABLE IF NOT EXISTS openbot_metadata (\
           table_name TEXT PRIMARY KEY COLLATE NOCASE,\
           owner_agent_id TEXT NOT NULL,\
           created_at TEXT NOT NULL\
         );\
         INSERT OR IGNORE INTO _openbot_tables (table_name, owner_agent_id, created_at)\
           SELECT table_name, owner_agent_id, created_at FROM openbot_metadata;\
         INSERT OR IGNORE INTO openbot_metadata (table_name, owner_agent_id, created_at)\
           SELECT table_name, owner_agent_id, created_at FROM _openbot_tables;",
    )?;
    Ok(())
}

impl SharedAgentDataDb {
    pub fn open(path: impl AsRef<Path>) -> StorageResult<Self> {
        let db_path = resolve_agent_data_path(path.as_ref());
        if let Some(parent) = db_path.parent()
            && !parent.as_os_str().is_empty()
        {
            if let Some(grand) = parent.parent()
                && grand.file_name().and_then(|n| n.to_str()) == Some("Shared")
            {
                create_secure_dir(grand)?;
            }
            create_secure_dir(parent)?;
        }

        let conn = Connection::open(&db_path)?;
        set_secure_file_mode(&db_path)?;

        let user_version: i64 = conn.query_row("PRAGMA user_version", [], |r| r.get(0))?;
        if user_version > AGENT_DATABASE_SCHEMA_VERSION {
            return Err(StorageError::NewerDatabase {
                found: user_version,
                supported: AGENT_DATABASE_SCHEMA_VERSION,
            });
        }
        if user_version < AGENT_DATABASE_SCHEMA_VERSION {
            conn.execute_batch(&format!(
                "PRAGMA user_version = {AGENT_DATABASE_SCHEMA_VERSION};"
            ))?;
        }
        conn.execute_batch(
            "PRAGMA journal_mode = WAL;\
             PRAGMA synchronous = NORMAL;\
             PRAGMA foreign_keys = ON;\
             PRAGMA trusted_schema = OFF;\
             PRAGMA max_page_count = 65536;",
        )?;
        provision_metadata_tables(&conn)?;

        let auth_state = Arc::new(Mutex::new(AuthorizerContext::DenyAll));
        let deadline_state = Arc::new(Mutex::new(None));
        install_shared_db_guards(&conn, Arc::clone(&auth_state), Arc::clone(&deadline_state))?;

        Ok(Self {
            inner: Arc::new(SharedAgentDataInner {
                db_path,
                conn: Mutex::new(conn),
                auth_state,
                deadline_state,
            }),
        })
    }

    pub fn open_in_memory() -> StorageResult<Self> {
        let conn = Connection::open_in_memory()?;
        conn.execute_batch(
            "PRAGMA user_version = 1;\
             PRAGMA foreign_keys = ON;\
             PRAGMA trusted_schema = OFF;\
             PRAGMA max_page_count = 65536;",
        )?;
        provision_metadata_tables(&conn)?;

        let auth_state = Arc::new(Mutex::new(AuthorizerContext::DenyAll));
        let deadline_state = Arc::new(Mutex::new(None));
        install_shared_db_guards(&conn, Arc::clone(&auth_state), Arc::clone(&deadline_state))?;

        Ok(Self {
            inner: Arc::new(SharedAgentDataInner {
                db_path: PathBuf::from(":memory:"),
                conn: Mutex::new(conn),
                auth_state,
                deadline_state,
            }),
        })
    }

    #[inline]
    pub fn db_path(&self) -> &Path {
        &self.inner.db_path
    }
}

fn quote_identifier(name: &str) -> String {
    name.replace('"', "\"\"")
}

fn truncate_cell_utf16(text: &str, max_chars: usize) -> String {
    let units: Vec<u16> = text.encode_utf16().collect();
    if units.len() <= max_chars {
        return text.to_string();
    }
    let prefix = String::from_utf16_lossy(&units[..max_chars]);
    format!("{prefix}…")
}

fn format_cell_value(value: ValueRef<'_>, max_cell_chars: usize) -> AgentCellValue {
    match value {
        ValueRef::Null => AgentCellValue::Null,
        ValueRef::Integer(i) => AgentCellValue::Integer(i),
        ValueRef::Real(f) => {
            if f.is_finite() {
                AgentCellValue::Real(f)
            } else {
                AgentCellValue::Null
            }
        }
        ValueRef::Text(bytes) => {
            let text = String::from_utf8_lossy(bytes);
            AgentCellValue::Text(truncate_cell_utf16(&text, max_cell_chars))
        }
        ValueRef::Blob(bytes) => AgentCellValue::Blob {
            blob_bytes: bytes.len(),
        },
    }
}

fn map_agent_sqlite_error(err: rusqlite::Error, db_path: &Path) -> StorageError {
    if let rusqlite::Error::SqliteFailure(sqlite_err, ref msg_opt) = err {
        let raw_msg = msg_opt.as_deref().unwrap_or("");
        if sqlite_err.code == ErrorCode::AuthorizationForStatementDenied
            || raw_msg.to_ascii_lowercase().contains("not authorized")
        {
            return StorageError::forbidden(
                "SQLite refused part of this statement. ATTACH, DETACH, PRAGMA, VACUUM, extensions, and TEMP triggers are not available, and a table another agent created cannot be dropped, altered, indexed, or given a trigger.",
            );
        }
        if sqlite_err.code == ErrorCode::DiskFull {
            return StorageError::limit_exceeded(
                "The shared database reached its 256 MB limit. Delete rows or tables that are no longer needed.",
            );
        }
        if sqlite_err.code == ErrorCode::OperationInterrupted {
            return StorageError::limit_exceeded(format!(
                "Statement exceeded the {}ms execution time limit.",
                AGENT_DATABASE_LIMITS.statement_deadline_ms
            ));
        }
        if raw_msg.contains("may not be modified") || raw_msg.contains("may not be dropped") {
            return StorageError::forbidden(raw_msg.to_string());
        }
    }
    let msg = err.to_string();
    if let Some(path_str) = db_path.to_str()
        && !path_str.is_empty()
        && path_str != ":memory:"
        && msg.contains(path_str)
    {
        let base = db_path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("agent-data.db");
        return StorageError::validation(msg.replace(path_str, base));
    }
    StorageError::Sqlite(err)
}

fn read_table_owners_admin(conn: &Connection) -> StorageResult<HashMap<String, String>> {
    let mut owners = HashMap::new();
    for table in [UPSTREAM_METADATA_TABLE, PRIMARY_METADATA_TABLE] {
        let mut stmt = conn.prepare(&format!(
            "SELECT table_name, owner_agent_id FROM {table}"
        ))?;
        let mut rows = stmt.query([])?;
        while let Some(row) = rows.next()? {
            let name: String = row.get(0)?;
            let owner: String = row.get(1)?;
            if !owner.is_empty() {
                owners.insert(name.to_ascii_lowercase(), owner);
            }
        }
    }
    Ok(owners)
}

fn read_visible_table_names_admin(conn: &Connection) -> StorageResult<Vec<String>> {
    let sql = format!(
        "SELECT name FROM sqlite_master WHERE type = 'table' AND {HIDDEN_TABLES_PREDICATE} ORDER BY name"
    );
    let mut stmt = conn.prepare(&sql)?;
    let rows = stmt.query_map([], |row| row.get::<_, String>(0))?;
    let mut names = Vec::new();
    for row in rows {
        names.push(row?);
    }
    Ok(names)
}

fn forget_removed_tables_admin(conn: &Connection) -> StorageResult<()> {
    conn.execute_batch(
        "DELETE FROM _openbot_tables \
         WHERE lower(table_name) NOT IN (SELECT lower(name) FROM sqlite_master WHERE type = 'table');\
         DELETE FROM openbot_metadata \
         WHERE lower(table_name) NOT IN (SELECT lower(name) FROM sqlite_master WHERE type = 'table');",
    )?;
    Ok(())
}

fn claim_new_tables_admin(
    conn: &Connection,
    agent_id: &str,
    existing_before: &[String],
) -> StorageResult<()> {
    let before_lower: HashSet<String> = existing_before
        .iter()
        .map(|name| name.to_ascii_lowercase())
        .collect();
    let current = read_visible_table_names_admin(conn)?;
    let now = now_rfc3339_millis();
    let mut ins_primary = conn.prepare(
        "INSERT OR IGNORE INTO _openbot_tables (table_name, owner_agent_id, created_at) \
         VALUES (?1, ?2, ?3)",
    )?;
    let mut ins_upstream = conn.prepare(
        "INSERT OR IGNORE INTO openbot_metadata (table_name, owner_agent_id, created_at) \
         VALUES (?1, ?2, ?3)",
    )?;
    for name in current {
        if !before_lower.contains(&name.to_ascii_lowercase()) {
            ins_primary.execute(params![name, agent_id, now])?;
            ins_upstream.execute(params![name, agent_id, now])?;
        }
    }
    Ok(())
}

impl SharedAgentDataDb {
    fn enter_scope(&self, context: AuthorizerContext) -> ExecutionScopeGuard<'_> {
        *self.inner.auth_state.lock() = context;
        *self.inner.deadline_state.lock() = Some(
            Instant::now() + Duration::from_millis(AGENT_DATABASE_LIMITS.statement_deadline_ms),
        );
        ExecutionScopeGuard {
            auth_state: &self.inner.auth_state,
            deadline_state: &self.inner.deadline_state,
        }
    }

    pub fn list_tables(&self) -> StorageResult<Vec<AgentTableSummary>> {
        let conn = self.inner.conn.lock();
        let _scope = self.enter_scope(AuthorizerContext::HostAdmin);

        let owners = read_table_owners_admin(&conn)?;
        let sql = format!(
            "SELECT name, coalesce(sql, '') FROM sqlite_master \
             WHERE type = 'table' AND {HIDDEN_TABLES_PREDICATE} ORDER BY name"
        );
        let mut stmt = conn.prepare(&sql)?;
        let entries: Vec<(String, String)> = stmt
            .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))?
            .collect::<rusqlite::Result<Vec<_>>>()?;

        let mut summaries = Vec::with_capacity(entries.len());
        for (name, create_sql) in entries {
            let quoted = quote_identifier(&name);
            let row_count: Option<i64> = conn
                .query_row(
                    &format!("SELECT COUNT(*) FROM \"{quoted}\""),
                    [],
                    |r| r.get(0),
                )
                .ok();
            let owner_agent_id = owners.get(&name.to_ascii_lowercase()).cloned();
            summaries.push(AgentTableSummary {
                name,
                owner_agent_id,
                row_count,
                sql: create_sql,
            });
        }
        Ok(summaries)
    }

    pub fn table_owner(&self, name: &str) -> StorageResult<Option<String>> {
        let conn = self.inner.conn.lock();
        let _scope = self.enter_scope(AuthorizerContext::HostAdmin);
        conn.query_row(
            "SELECT owner_agent_id FROM _openbot_tables WHERE lower(table_name) = lower(?1)",
            params![name.trim()],
            |row| row.get(0),
        )
        .optional()
        .map_err(StorageError::from)
    }

    pub fn query(
        &self,
        agent_id: &str,
        sql: &str,
        params: &[JsonValue],
    ) -> StorageResult<AgentQueryResult> {
        let arr = JsonValue::Array(params.to_vec());
        self.query_checked(agent_id, sql, Some(&arr))
    }

    pub fn query_checked(
        &self,
        agent_id: &str,
        sql: &str,
        params: Option<&JsonValue>,
    ) -> StorageResult<AgentQueryResult> {
        let checked_sql = check_agent_sql(sql, AgentSqlMode::Read)?;
        let bound_params = check_agent_parameters(params, &checked_sql)?;

        let conn = self.inner.conn.lock();
        let owners = {
            let _admin = self.enter_scope(AuthorizerContext::HostAdmin);
            read_table_owners_admin(&conn)?
        };
        let _agent_scope = self.enter_scope(AuthorizerContext::Agent {
            agent_id: agent_id.to_string(),
            mode: AgentSqlMode::Read,
            in_ddl: false,
            table_owners: owners,
        });

        let mut stmt = conn
            .prepare(&checked_sql)
            .map_err(|e| map_agent_sqlite_error(e, &self.inner.db_path))?;
        let columns: Vec<String> = stmt
            .column_names()
            .into_iter()
            .enumerate()
            .map(|(idx, col)| {
                if col.is_empty() {
                    format!("column{}", idx + 1)
                } else {
                    col.to_string()
                }
            })
            .collect();
        let col_count = columns.len();
        let mut rows_cursor = stmt
            .query(params_from_iter(bound_params.iter()))
            .map_err(|e| map_agent_sqlite_error(e, &self.inner.db_path))?;

        let mut rows_out = Vec::new();
        let mut total_bytes = 0usize;
        let mut truncated = false;
        while let Some(row) = rows_cursor
            .next()
            .map_err(|e| map_agent_sqlite_error(e, &self.inner.db_path))?
        {
            if rows_out.len() >= AGENT_DATABASE_LIMITS.max_rows
                || total_bytes >= AGENT_DATABASE_LIMITS.max_result_bytes
            {
                truncated = true;
                break;
            }
            let mut cells = Vec::with_capacity(col_count);
            for idx in 0..col_count {
                let raw = row
                    .get_ref(idx)
                    .map_err(|e| map_agent_sqlite_error(e, &self.inner.db_path))?;
                cells.push(format_cell_value(raw, AGENT_DATABASE_LIMITS.max_cell_chars));
            }
            total_bytes += serde_json::to_string(&cells)?.len();
            rows_out.push(cells);
        }

        Ok(AgentQueryResult {
            columns,
            rows: rows_out,
            truncated,
        })
    }

    pub fn execute(
        &self,
        agent_id: &str,
        sql: &str,
        params: &[JsonValue],
    ) -> StorageResult<AgentWriteResult> {
        let arr = JsonValue::Array(params.to_vec());
        self.execute_checked(agent_id, sql, Some(&arr))
    }

    pub fn execute_checked(
        &self,
        agent_id: &str,
        sql: &str,
        params: Option<&JsonValue>,
    ) -> StorageResult<AgentWriteResult> {
        if agent_id.trim().is_empty() {
            return Err(StorageError::validation("agent_id must not be empty"));
        }
        let checked_sql = check_agent_sql(sql, AgentSqlMode::Write)?;
        let bound_params = check_agent_parameters(params, &checked_sql)?;
        let ddl_change = schema_change(&checked_sql);

        let conn = self.inner.conn.lock();
        let (existing_before, owners) = {
            let _admin = self.enter_scope(AuthorizerContext::HostAdmin);
            let existing = read_visible_table_names_admin(&conn)?;
            if ddl_change == Some(AgentSchemaChange::Create)
                && existing.len() >= AGENT_DATABASE_LIMITS.max_shared_tables
            {
                return Err(StorageError::limit_exceeded(format!(
                    "There are already {} tables. Add columns or rows to a table that exists, or remove one you created.",
                    AGENT_DATABASE_LIMITS.max_shared_tables
                )));
            }
            let owners = read_table_owners_admin(&conn)?;
            (existing, owners)
        };

        let (changes, last_insert_rowid) = {
            let _agent_scope = self.enter_scope(AuthorizerContext::Agent {
                agent_id: agent_id.to_string(),
                mode: AgentSqlMode::Write,
                in_ddl: ddl_change.is_some(),
                table_owners: owners,
            });
            let mut stmt = conn
                .prepare(&checked_sql)
                .map_err(|e| map_agent_sqlite_error(e, &self.inner.db_path))?;
            let changed = stmt
                .execute(params_from_iter(bound_params.iter()))
                .map_err(|e| map_agent_sqlite_error(e, &self.inner.db_path))?;
            let rowid = conn.last_insert_rowid();
            (changed, (changed > 0 || ddl_change.is_none()).then_some(rowid))
        };

        if ddl_change.is_some() {
            let _admin = self.enter_scope(AuthorizerContext::HostAdmin);
            forget_removed_tables_admin(&conn)?;
            claim_new_tables_admin(&conn, agent_id, &existing_before)?;
        }

        Ok(AgentWriteResult {
            changes,
            last_insert_rowid,
        })
    }
}

impl SharedAgentDataDb {
    pub fn remove_table(&self, agent_id: &str, name: &str) -> StorageResult<String> {
        let wanted = name.trim();
        if wanted.is_empty() {
            return Err(StorageError::validation("Provide a table name."));
        }
        let tables = self.list_tables()?;
        let Some(found) = tables
            .into_iter()
            .find(|entry| entry.name.eq_ignore_ascii_case(wanted))
        else {
            return Err(StorageError::not_found(format!(
                "There is no table called {name}."
            )));
        };

        if let Some(ref owner) = found.owner_agent_id
            && owner != agent_id
        {
            return Err(StorageError::forbidden(format!(
                "{} was created by another agent. Ask that agent to remove it, or the user can remove it in agent settings.",
                found.name
            )));
        }

        let conn = self.inner.conn.lock();
        let _admin = self.enter_scope(AuthorizerContext::HostAdmin);
        let quoted = quote_identifier(&found.name);
        conn.execute_batch(&format!("DROP TABLE \"{quoted}\";"))?;
        forget_removed_tables_admin(&conn)?;
        Ok(found.name)
    }

    pub fn remove_table_as_user(&self, name: &str) -> StorageResult<()> {
        let wanted = name.trim();
        if wanted.is_empty() {
            return Err(StorageError::validation("Provide a table name."));
        }
        let tables = self.list_tables()?;
        let Some(found) = tables
            .into_iter()
            .find(|entry| entry.name.eq_ignore_ascii_case(wanted))
        else {
            return Err(StorageError::not_found(format!(
                "There is no table called {name}."
            )));
        };

        let conn = self.inner.conn.lock();
        let _admin = self.enter_scope(AuthorizerContext::HostAdmin);
        let quoted = quote_identifier(&found.name);
        conn.execute_batch(&format!("DROP TABLE \"{quoted}\";"))?;
        forget_removed_tables_admin(&conn)?;
        Ok(())
    }

    pub fn quick_check(&self) -> StorageResult<()> {
        let conn = self.inner.conn.lock();
        let _admin = self.enter_scope(AuthorizerContext::HostAdmin);
        assert_quick_check(&conn)
    }
}
