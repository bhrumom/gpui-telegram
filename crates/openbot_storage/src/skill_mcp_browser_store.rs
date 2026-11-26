//! Browser Profiles, Browser Proxies, and MCP Server persistence stores
//! (`BrowserProfileStore`, `BrowserProxyStore`, `McpServerStore`).
//! Faithful 1:1 port of `src/backend/browser-profiles/{browser-profile-store,browser-proxy-store}.ts`,
//! `src/backend/mcp-server-store.ts`, and `packages/contracts/src/{ipc-browser-profiles,ipc-browser-proxies,ipc-mcp-servers}.ts`.

use std::collections::HashMap;
use std::fmt::Write as _;

use rusqlite::{Connection, OptionalExtension, params};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::crm_store::new_crm_id;
use crate::database::OpenBotDatabase;
use crate::error::{StorageError, StorageResult};
use crate::workflow_store::{ms_to_iso, now_millis};
use openbot_contracts::avatar::AvatarHue;
use openbot_contracts::limits::{INPUT_LIMITS, utf16_len};

// ---------------------------------------------------------------------------
// BrowserProfileStore (`browser_profiles`)
// ---------------------------------------------------------------------------

pub const BROWSER_PROFILE_NAME_MAX: usize = 60;
pub const BROWSER_PROFILE_LIMIT: usize = 100;

pub const BROWSER_PROFILE_HUES: [AvatarHue; 8] = [
    AvatarHue::Blue,
    AvatarHue::Orange,
    AvatarHue::Green,
    AvatarHue::Magenta,
    AvatarHue::Yellow,
    AvatarHue::Cyan,
    AvatarHue::Indigo,
    AvatarHue::Red,
];

pub fn next_browser_profile_hue(used: &[AvatarHue]) -> AvatarHue {
    let count_of = |h: AvatarHue| used.iter().filter(|&&c| c == h).count();
    let mut best = BROWSER_PROFILE_HUES[0];
    for &cand in &BROWSER_PROFILE_HUES[1..] {
        if count_of(cand) < count_of(best) {
            best = cand;
        }
    }
    best
}

pub fn is_browser_profile_id(value: &str) -> bool {
    value.len() == 32 && value.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))
}

pub fn browser_profile_name(value: &str) -> Option<String> {
    let trimmed = value.trim();
    (!trimmed.is_empty() && utf16_len(trimmed) <= BROWSER_PROFILE_NAME_MAX)
        .then(|| trimmed.to_string())
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BrowserProfileRecord {
    pub id: String,
    pub name: String,
    pub hue: AvatarHue,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "lowercase")]
pub enum AgentBrowserProfile {
    Shared,
    Agent,
    Profile {
        #[serde(rename = "profileId")]
        profile_id: String,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BrowserProfileSummary {
    pub id: String,
    pub name: String,
    pub hue: AvatarHue,
    pub agent_ids: Vec<String>,
    pub tab_count: usize,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BrowserProfileList {
    pub profiles: Vec<BrowserProfileSummary>,
    pub shared_agent_ids: Vec<String>,
}

#[derive(Clone)]
pub struct BrowserProfileStore {
    db: OpenBotDatabase,
}

impl BrowserProfileStore {
    pub fn new(db: OpenBotDatabase) -> Self {
        Self { db }
    }

    pub fn list(&self) -> StorageResult<Vec<BrowserProfileRecord>> {
        self.db.with_conn(list_profiles_on_conn)
    }

    pub fn get(&self, id: &str) -> StorageResult<Option<BrowserProfileRecord>> {
        self.db.with_conn(|conn| {
            conn.query_row(
                "SELECT profile_id, name, hue FROM browser_profiles WHERE profile_id = ?1",
                params![id],
                read_profile_row,
            )
            .optional()?
            .transpose()
        })
    }

    pub fn create(&self, name: &str, hue: AvatarHue) -> StorageResult<BrowserProfileRecord> {
        let valid_name = browser_profile_name(name)
            .ok_or_else(|| StorageError::Validation("invalid browser profile name".to_string()))?;
        let now = ms_to_iso(now_millis());
        self.db.with_tx(|conn| {
            let existing = list_profiles_on_conn(conn)?;
            if existing.len() >= BROWSER_PROFILE_LIMIT {
                return Err(StorageError::LimitExceeded(format!(
                    "browser profile limit ({BROWSER_PROFILE_LIMIT}) reached"
                )));
            }
            if existing
                .iter()
                .any(|p| p.name.to_lowercase() == valid_name.to_lowercase())
            {
                return Err(StorageError::Conflict(format!(
                    "browser profile name '{}' already taken",
                    valid_name
                )));
            }
            let id = new_crm_id();
            conn.execute(
                "INSERT INTO browser_profiles (profile_id, name, hue, created_at, updated_at) \
                 VALUES (?1, ?2, ?3, ?4, ?5)",
                params![id, valid_name, i64::from(hue.degrees()), now, now],
            )?;
            Ok(BrowserProfileRecord {
                id,
                name: valid_name,
                hue,
            })
        })
    }

    pub fn create_auto_hue(&self, name: &str) -> StorageResult<BrowserProfileRecord> {
        let used: Vec<AvatarHue> = self.list()?.into_iter().map(|p| p.hue).collect();
        let hue = next_browser_profile_hue(&used);
        self.create(name, hue)
    }

    pub fn rename(&self, id: &str, name: &str) -> StorageResult<()> {
        let valid_name = browser_profile_name(name)
            .ok_or_else(|| StorageError::Validation("invalid browser profile name".to_string()))?;
        let now = ms_to_iso(now_millis());
        self.db.with_tx(|conn| {
            let existing = list_profiles_on_conn(conn)?;
            if !existing.iter().any(|p| p.id == id) {
                return Err(StorageError::NotFound(format!(
                    "browser profile {id} not found"
                )));
            }
            if existing
                .iter()
                .any(|p| p.id != id && p.name.to_lowercase() == valid_name.to_lowercase())
            {
                return Err(StorageError::Conflict(format!(
                    "browser profile name '{}' already taken",
                    valid_name
                )));
            }
            conn.execute(
                "UPDATE browser_profiles SET name = ?1, updated_at = ?2 WHERE profile_id = ?3",
                params![valid_name, now, id],
            )?;
            Ok(())
        })
    }

    pub fn set_hue(&self, id: &str, hue: AvatarHue) -> StorageResult<()> {
        let now = ms_to_iso(now_millis());
        self.db.with_conn(|conn| {
            let changes = conn.execute(
                "UPDATE browser_profiles SET hue = ?1, updated_at = ?2 WHERE profile_id = ?3",
                params![i64::from(hue.degrees()), now, id],
            )?;
            if changes == 0 {
                return Err(StorageError::NotFound(format!(
                    "browser profile {id} not found"
                )));
            }
            Ok(())
        })
    }

    pub fn delete(&self, id: &str) -> StorageResult<bool> {
        self.db.with_conn(|conn| {
            let changes = conn.execute(
                "DELETE FROM browser_profiles WHERE profile_id = ?1",
                params![id],
            )?;
            Ok(changes > 0)
        })
    }
}

fn list_profiles_on_conn(conn: &Connection) -> StorageResult<Vec<BrowserProfileRecord>> {
    let mut stmt = conn.prepare(
        "SELECT profile_id, name, hue FROM browser_profiles \
         ORDER BY name COLLATE NOCASE, created_at, profile_id",
    )?;
    let rows = stmt.query_map([], read_profile_row)?;
    let mut out = Vec::new();
    for r in rows {
        out.push(r??);
    }
    Ok(out)
}

fn read_profile_row(
    row: &rusqlite::Row<'_>,
) -> rusqlite::Result<StorageResult<BrowserProfileRecord>> {
    let id: String = row.get(0)?;
    let name: String = row.get(1)?;
    let raw_hue: i64 = row.get(2)?;
    let Some(hue) = u16::try_from(raw_hue).ok().and_then(AvatarHue::from_degrees) else {
        return Ok(Err(StorageError::Validation(format!(
            "invalid browser profile hue: {raw_hue}"
        ))));
    };
    Ok(Ok(BrowserProfileRecord { id, name, hue }))
}

// ---------------------------------------------------------------------------
// BrowserProxyStore (`browser_proxies`, `browser_profile_proxies`)
// ---------------------------------------------------------------------------

pub const BROWSER_PROXY_LIMIT: usize = 50;
pub const BROWSER_PROXY_ADDRESS_MAX: usize = 2_048;
pub const SOCKS_SIGN_IN_MAX_BYTES: usize = 255;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum BrowserProxyScheme {
    Socks5,
    Http,
    Https,
}

impl BrowserProxyScheme {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Socks5 => "socks5",
            Self::Http => "http",
            Self::Https => "https",
        }
    }

    pub fn parse(raw: &str) -> Option<Self> {
        match raw {
            "socks5" => Some(Self::Socks5),
            "http" => Some(Self::Http),
            "https" => Some(Self::Https),
            _ => None,
        }
    }

    pub const fn default_port(self) -> u16 {
        match self {
            Self::Socks5 => 1080,
            Self::Http => 80,
            Self::Https => 443,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BrowserProxyAddress {
    pub scheme: BrowserProxyScheme,
    pub host: String,
    pub port: u16,
    pub username: Option<String>,
    pub password: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BrowserProxyAddressRecord {
    pub id: String,
    pub scheme: BrowserProxyScheme,
    pub host: String,
    pub port: u16,
    pub username: Option<String>,
    pub has_password: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BrowserProxyComputerRecord {
    pub id: String,
    pub member_id: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "lowercase")]
pub enum BrowserProxyRecord {
    Address(BrowserProxyAddressRecord),
    Computer(BrowserProxyComputerRecord),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BrowserProxyRow {
    pub scheme: BrowserProxyScheme,
    pub host: String,
    pub port: u16,
    pub username: Option<String>,
    pub password: Option<Vec<u8>>,
}

pub fn parse_browser_proxy_address(text: &str) -> Option<BrowserProxyAddress> {
    let val = text.trim();
    if val.is_empty() || utf16_len(val) > BROWSER_PROXY_ADDRESS_MAX || val.chars().any(char::is_whitespace) {
        return None;
    }
    let (scheme, rest, port_required) = match val.find("://") {
        Some(idx) => {
            let raw_scheme = &val[..idx];
            if raw_scheme.is_empty()
                || !raw_scheme.bytes().next()?.is_ascii_alphabetic()
                || !raw_scheme
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'+' | b'.' | b'-'))
            {
                return None;
            }
            let parsed = BrowserProxyScheme::parse(&raw_scheme.to_ascii_lowercase())?;
            (parsed, &val[idx + 3..], false)
        }
        None => (BrowserProxyScheme::Http, val, true),
    };
    let (raw_host, raw_port, raw_user, raw_pass) = split_proxy_parts(rest, port_required)?;
    let host = raw_host.to_ascii_lowercase();
    if !is_valid_proxy_host(&host) {
        return None;
    }
    let port: u16 = match raw_port {
        None => scheme.default_port(),
        Some(p) => {
            let parsed: u32 = p.parse().ok()?;
            if !(1..=65_535).contains(&parsed) {
                return None;
            }
            parsed as u16
        }
    };
    let username = decode_sign_in_part(raw_user);
    let password = decode_sign_in_part(raw_pass);
    if username.is_none() && password.is_some() {
        return None;
    }
    if scheme == BrowserProxyScheme::Socks5
        && (username.as_ref().is_some_and(|u| u.len() > SOCKS_SIGN_IN_MAX_BYTES)
            || password.as_ref().is_some_and(|p| p.len() > SOCKS_SIGN_IN_MAX_BYTES))
    {
        return None;
    }
    Some(BrowserProxyAddress {
        scheme,
        host,
        port,
        username,
        password,
    })
}

pub fn browser_proxy_address_text(
    scheme: BrowserProxyScheme,
    host: &str,
    port: u16,
    username: Option<&str>,
) -> String {
    match username {
        Some(u) => format!("{}://{}@{host}:{port}", scheme.as_str(), percent_encode_component(u)),
        None => format!("{}://{host}:{port}", scheme.as_str()),
    }
}

fn split_proxy_parts(
    rest: &str,
    port_required: bool,
) -> Option<(&str, Option<&str>, &str, &str)> {
    if !rest.starts_with('[') {
        let fields: Vec<&str> = rest.split(':').collect();
        if fields.len() >= 4
            && is_valid_proxy_host(&fields[0].to_ascii_lowercase())
            && is_port_digits(fields[1])
            && !fields[2].contains('@')
        {
            let pass_start = fields[0].len() + 1 + fields[1].len() + 1 + fields[2].len() + 1;
            return Some((fields[0], Some(fields[1]), fields[2], &rest[pass_start..]));
        }
    }
    let (sign_in, host_part) = match rest.rfind('@') {
        Some(at) => (&rest[..at], &rest[at + 1..]),
        None => ("", rest),
    };
    let (host, port) = split_host_and_port(host_part, port_required)?;
    let (user, pass) = match sign_in.find(':') {
        Some(c) => (&sign_in[..c], &sign_in[c + 1..]),
        None => (sign_in, ""),
    };
    Some((host, port, user, pass))
}

fn split_host_and_port(value: &str, port_required: bool) -> Option<(&str, Option<&str>)> {
    let text = value.strip_suffix('/').unwrap_or(value);
    let bracket_end = if text.starts_with('[') {
        text.find(']')
    } else {
        None
    };
    let colon_opt = match bracket_end {
        Some(b) => text[b..].find(':').map(|off| b + off),
        None => {
            let last = text.rfind(':');
            if text.find(':') != last {
                return None;
            }
            last
        }
    };
    match colon_opt {
        None if port_required => None,
        None => Some((text, None)),
        Some(c) => {
            let p = &text[c + 1..];
            is_port_digits(p).then_some((&text[..c], Some(p)))
        }
    }
}

fn is_port_digits(s: &str) -> bool {
    (1..=5).contains(&s.len()) && s.bytes().all(|b| b.is_ascii_digit())
}

fn is_valid_proxy_host(host: &str) -> bool {
    if let Some(inner) = host.strip_prefix('[').and_then(|s| s.strip_suffix(']')) {
        return !inner.is_empty()
            && inner
                .bytes()
                .all(|b| b.is_ascii_hexdigit() || b == b':' || b == b'.');
    }
    !host.is_empty()
        && host.split('.').all(|label| {
            let bytes = label.as_bytes();
            !bytes.is_empty()
                && bytes.len() <= 63
                && bytes[0].is_ascii_alphanumeric()
                && bytes[bytes.len() - 1].is_ascii_alphanumeric()
                && bytes.iter().all(|b| b.is_ascii_alphanumeric() || *b == b'-')
        })
}

fn decode_sign_in_part(raw: &str) -> Option<String> {
    if raw.is_empty() {
        return None;
    }
    let bytes = raw.as_bytes();
    let mut decoded = Vec::with_capacity(bytes.len());
    let mut idx = 0;
    while idx < bytes.len() {
        if bytes[idx] == b'%' && idx + 2 < bytes.len() {
            if let Ok(hex_byte) = u8::from_str_radix(&raw[idx + 1..idx + 3], 16) {
                decoded.push(hex_byte);
                idx += 3;
                continue;
            }
        }
        decoded.push(bytes[idx]);
        idx += 1;
    }
    Some(String::from_utf8(decoded).unwrap_or_else(|_| raw.to_string()))
}

fn percent_encode_component(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for &b in value.as_bytes() {
        if b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.' | b'!' | b'~' | b'*' | b'\'' | b'(' | b')') {
            out.push(b as char);
        } else {
            let _ = write!(&mut out, "%{b:02X}");
        }
    }
    out
}

const PROXY_RECORD_COLS: &str = "\
  proxy_id, kind, scheme, host, port, username, password IS NOT NULL AS has_password, member_id";

#[derive(Clone)]
pub struct BrowserProxyStore {
    db: OpenBotDatabase,
}

impl BrowserProxyStore {
    pub fn new(db: OpenBotDatabase) -> Self {
        Self { db }
    }

    pub fn list(&self) -> StorageResult<Vec<BrowserProxyRecord>> {
        self.db.with_conn(list_proxies_on_conn)
    }

    pub fn get(&self, id: &str) -> StorageResult<Option<BrowserProxyRecord>> {
        self.db.with_conn(|conn| {
            conn.query_row(
                &format!("SELECT {PROXY_RECORD_COLS} FROM browser_proxies WHERE proxy_id = ?1"),
                params![id],
                read_proxy_row,
            )
            .optional()?
            .transpose()
        })
    }

    pub fn ensure_computer(&self, member_id: &str) -> StorageResult<BrowserProxyComputerRecord> {
        if member_id.is_empty() || member_id.len() > 255 {
            return Err(StorageError::Validation("invalid proxy member_id".to_string()));
        }
        let now = ms_to_iso(now_millis());
        let cand_id = new_crm_id();
        self.db.with_tx(|conn| {
            conn.execute(
                "INSERT INTO browser_proxies (proxy_id, kind, member_id, created_at, updated_at) \
                 VALUES (?1, 'computer', ?2, ?3, ?4) \
                 ON CONFLICT(member_id) WHERE kind = 'computer' DO NOTHING",
                params![cand_id, member_id, now, now],
            )?;
            let id: String = conn.query_row(
                "SELECT proxy_id FROM browser_proxies WHERE kind = 'computer' AND member_id = ?1",
                params![member_id],
                |r| r.get(0),
            )?;
            Ok(BrowserProxyComputerRecord {
                id,
                member_id: member_id.to_string(),
            })
        })
    }

    pub fn password(&self, id: &str) -> StorageResult<Option<Vec<u8>>> {
        self.db.with_conn(|conn| {
            let pw: Option<Option<Vec<u8>>> = conn
                .query_row(
                    "SELECT password FROM browser_proxies WHERE proxy_id = ?1",
                    params![id],
                    |r| r.get(0),
                )
                .optional()?;
            Ok(pw.flatten())
        })
    }

    pub fn assignments(&self) -> StorageResult<HashMap<String, String>> {
        self.db.with_conn(|conn| {
            let mut stmt =
                conn.prepare("SELECT profile_id, proxy_id FROM browser_profile_proxies")?;
            let rows =
                stmt.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))?;
            Ok(rows.collect::<rusqlite::Result<HashMap<_, _>>>()?)
        })
    }

    pub fn proxy_of_profile(&self, profile_id: &str) -> StorageResult<Option<String>> {
        self.db.with_conn(|conn| {
            conn.query_row(
                "SELECT proxy_id FROM browser_profile_proxies WHERE profile_id = ?1",
                params![profile_id],
                |r| r.get(0),
            )
            .optional()
            .map_err(StorageError::from)
        })
    }

    pub fn create(&self, row: &BrowserProxyRow) -> StorageResult<String> {
        validate_proxy_row(row)?;
        let now = ms_to_iso(now_millis());
        self.db.with_tx(|conn| {
            let existing = list_proxies_on_conn(conn)?;
            let addr_list: Vec<&BrowserProxyAddressRecord> = existing
                .iter()
                .filter_map(|p| match p {
                    BrowserProxyRecord::Address(a) => Some(a),
                    BrowserProxyRecord::Computer(_) => None,
                })
                .collect();
            if addr_list.len() >= BROWSER_PROXY_LIMIT {
                return Err(StorageError::LimitExceeded(format!(
                    "browser proxy limit ({BROWSER_PROXY_LIMIT}) reached"
                )));
            }
            if addr_list.iter().any(|p| {
                p.scheme == row.scheme
                    && p.host.eq_ignore_ascii_case(&row.host)
                    && p.port == row.port
                    && p.username == row.username
            }) {
                return Err(StorageError::Conflict("browser proxy already exists".to_string()));
            }
            let id = new_crm_id();
            conn.execute(
                "INSERT INTO browser_proxies \
                   (proxy_id, scheme, host, port, username, password, created_at, updated_at) \
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
                params![
                    id,
                    row.scheme.as_str(),
                    row.host.to_ascii_lowercase(),
                    i64::from(row.port),
                    row.username,
                    row.password,
                    now,
                    now,
                ],
            )?;
            Ok(id)
        })
    }

    pub fn update(&self, id: &str, row: &BrowserProxyRow) -> StorageResult<()> {
        validate_proxy_row(row)?;
        let now = ms_to_iso(now_millis());
        self.db.with_tx(|conn| {
            let changes = conn.execute(
                "UPDATE browser_proxies SET scheme = ?1, host = ?2, port = ?3, username = ?4, \
                   password = ?5, updated_at = ?6 \
                 WHERE proxy_id = ?7 AND kind = 'address'",
                params![
                    row.scheme.as_str(),
                    row.host.to_ascii_lowercase(),
                    i64::from(row.port),
                    row.username,
                    row.password,
                    now,
                    id,
                ],
            )?;
            if changes == 0 {
                return Err(StorageError::NotFound(format!(
                    "browser address proxy {id} not found"
                )));
            }
            Ok(())
        })
    }

    pub fn delete(&self, id: &str) -> StorageResult<bool> {
        self.db.with_conn(|conn| {
            let changes = conn.execute(
                "DELETE FROM browser_proxies WHERE proxy_id = ?1",
                params![id],
            )?;
            Ok(changes > 0)
        })
    }

    pub fn assign(&self, profile_id: &str, proxy_id: Option<&str>) -> StorageResult<()> {
        self.db.with_conn(|conn| {
            match proxy_id {
                None => {
                    conn.execute(
                        "DELETE FROM browser_profile_proxies WHERE profile_id = ?1",
                        params![profile_id],
                    )?;
                }
                Some(pid) => {
                    conn.execute(
                        "INSERT INTO browser_profile_proxies (profile_id, proxy_id) VALUES (?1, ?2) \
                         ON CONFLICT(profile_id) DO UPDATE SET proxy_id = excluded.proxy_id",
                        params![profile_id, pid],
                    )?;
                }
            }
            Ok(())
        })
    }
}

fn validate_proxy_row(row: &BrowserProxyRow) -> StorageResult<()> {
    if row.port == 0 || !is_valid_proxy_host(&row.host.to_ascii_lowercase()) {
        return Err(StorageError::Validation(
            "invalid browser proxy host or port".to_string(),
        ));
    }
    if row.username.is_none() && row.password.is_some() {
        return Err(StorageError::Validation(
            "browser proxy password requires username".to_string(),
        ));
    }
    if row.scheme == BrowserProxyScheme::Socks5
        && row
            .username
            .as_ref()
            .is_some_and(|u| u.len() > SOCKS_SIGN_IN_MAX_BYTES)
    {
        return Err(StorageError::Validation(
            "socks5 proxy username exceeds 255 bytes".to_string(),
        ));
    }
    Ok(())
}

fn list_proxies_on_conn(conn: &Connection) -> StorageResult<Vec<BrowserProxyRecord>> {
    let mut stmt = conn.prepare(&format!(
        "SELECT {PROXY_RECORD_COLS} FROM browser_proxies \
         ORDER BY kind, host COLLATE NOCASE, port, created_at, proxy_id"
    ))?;
    let rows = stmt.query_map([], read_proxy_row)?;
    let mut out = Vec::new();
    for r in rows {
        out.push(r??);
    }
    Ok(out)
}

fn read_proxy_row(
    row: &rusqlite::Row<'_>,
) -> rusqlite::Result<StorageResult<BrowserProxyRecord>> {
    let id: String = row.get(0)?;
    let kind: String = row.get(1)?;
    if kind == "computer" {
        let member_id: Option<String> = row.get(7)?;
        let Some(member_id) = member_id else {
            return Ok(Err(StorageError::Validation(
                "computer proxy missing member_id".to_string(),
            )));
        };
        return Ok(Ok(BrowserProxyRecord::Computer(BrowserProxyComputerRecord {
            id,
            member_id,
        })));
    }
    if kind != "address" {
        return Ok(Err(StorageError::Validation(format!(
            "invalid browser proxy kind: {kind}"
        ))));
    }
    let scheme_s: Option<String> = row.get(2)?;
    let host: Option<String> = row.get(3)?;
    let port: Option<i64> = row.get(4)?;
    let username: Option<String> = row.get(5)?;
    let has_pw: i64 = row.get(6)?;
    let (Some(scheme), Some(host), Some(port)) = (
        scheme_s.as_deref().and_then(BrowserProxyScheme::parse),
        host,
        port.and_then(|p| u16::try_from(p).ok()).filter(|&p| p >= 1),
    ) else {
        return Ok(Err(StorageError::Validation(
            "invalid browser address proxy columns".to_string(),
        )));
    };
    Ok(Ok(BrowserProxyRecord::Address(BrowserProxyAddressRecord {
        id,
        scheme,
        host,
        port,
        username,
        has_password: has_pw == 1,
    })))
}

// ---------------------------------------------------------------------------
// McpServerStore (`projection_mcp_servers`)
// ---------------------------------------------------------------------------

pub const RESERVED_MCP_SERVER_NAMES: [&str; 2] = ["openbot", "openbot_browser"];
pub const COMPUTER_USE_MCP_SERVER_NAME: &str = "computer_use";
pub const COMPUTER_USE_MCP_SERVER_ID: &str = "openbot-computer-use";
pub const GITHUB_CONNECTOR_MCP_SERVER_ID: &str = "openbot-github";
pub const GITHUB_CONNECTOR_MCP_SERVER_NAME: &str = "github";
pub const GITHUB_CONNECTOR_MCP_SERVER_URL: &str = "https://api.githubcopilot.com/mcp/";

const MCP_REMOTE_BRIDGES: [(&str, &str, &str); 6] = [
    ("canva", "https://mcp.canva.com/mcp", "https://mcp.canva.com/mcp"),
    ("linear", "https://mcp.linear.app/sse", "https://mcp.linear.app/mcp"),
    ("notion", "https://mcp.notion.com/mcp", "https://mcp.notion.com/mcp"),
    ("figma", "https://mcp.figma.com/mcp", "https://mcp.figma.com/mcp"),
    ("sentry", "https://mcp.sentry.dev/mcp", "https://mcp.sentry.dev/mcp"),
    ("stripe", "https://mcp.stripe.com", "https://mcp.stripe.com"),
];

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum McpTransport {
    Stdio,
    Http,
}

impl McpTransport {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Stdio => "stdio",
            Self::Http => "http",
        }
    }

    pub fn parse(raw: &str) -> Option<Self> {
        match raw {
            "stdio" => Some(Self::Stdio),
            "http" => Some(Self::Http),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct McpKeyValue {
    pub key: String,
    pub value: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct McpServerConfig {
    pub id: String,
    pub name: String,
    pub transport: McpTransport,
    pub enabled: bool,
    pub command: String,
    pub args: Vec<String>,
    pub env: Vec<McpKeyValue>,
    pub env_passthrough: Vec<String>,
    pub working_directory: String,
    pub url: String,
    pub headers: Vec<McpKeyValue>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct McpConfigErrors {
    pub name: Option<String>,
    pub command: Option<String>,
    pub url: Option<String>,
    pub headers: Option<String>,
}

impl McpConfigErrors {
    pub fn first_error(&self) -> Option<&str> {
        self.name
            .as_deref()
            .or(self.command.as_deref())
            .or(self.url.as_deref())
            .or(self.headers.as_deref())
    }
}

pub fn create_mcp_server_id() -> String {
    format!("mcp-{}", Uuid::new_v4())
}

pub fn is_reserved_mcp_server_name(name: &str) -> bool {
    let lower = name.trim().to_ascii_lowercase();
    RESERVED_MCP_SERVER_NAMES.contains(&lower.as_str())
}

pub fn is_mcp_header_name(key: &str) -> bool {
    let trimmed = key.trim();
    trimmed.is_empty()
        || trimmed.bytes().all(|b| {
            b.is_ascii_alphanumeric()
                || matches!(
                    b,
                    b'!' | b'#' | b'$' | b'%' | b'&' | b'\'' | b'*' | b'+' | b'-' | b'.' | b'^'
                        | b'_' | b'`' | b'|' | b'~'
                )
        })
}

pub fn normalize_mcp_config(config: &McpServerConfig) -> McpServerConfig {
    let stdio = config.transport == McpTransport::Stdio;
    let norm_pairs = |pairs: &[McpKeyValue]| -> Vec<McpKeyValue> {
        pairs
            .iter()
            .map(|p| McpKeyValue {
                key: p.key.trim().to_string(),
                value: p.value.clone(),
            })
            .filter(|p| !p.key.is_empty())
            .collect()
    };
    McpServerConfig {
        id: config.id.clone(),
        name: config.name.trim().to_string(),
        transport: config.transport,
        enabled: config.enabled,
        command: if stdio {
            config.command.trim().to_string()
        } else {
            String::new()
        },
        args: if stdio {
            config
                .args
                .iter()
                .filter(|a| !a.trim().is_empty())
                .cloned()
                .collect()
        } else {
            Vec::new()
        },
        env: if stdio { norm_pairs(&config.env) } else { Vec::new() },
        env_passthrough: if stdio {
            config
                .env_passthrough
                .iter()
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty())
                .collect()
        } else {
            Vec::new()
        },
        working_directory: if stdio {
            config.working_directory.trim().to_string()
        } else {
            String::new()
        },
        url: if stdio {
            String::new()
        } else {
            config.url.trim().to_string()
        },
        headers: if stdio {
            Vec::new()
        } else {
            norm_pairs(&config.headers)
        },
    }
}

fn is_http_or_https_url(raw: &str) -> bool {
    let t = raw.trim();
    if t.is_empty() || utf16_len(t) > INPUT_LIMITS.mcp_url || t.chars().any(char::is_whitespace) {
        return false;
    }
    let Some(rest) = t
        .strip_prefix("http://")
        .or_else(|| t.strip_prefix("https://"))
    else {
        return false;
    };
    let host_port = rest.split(['/', '?', '#']).next().unwrap_or("");
    !host_port.is_empty()
}

pub fn mcp_config_errors(config: &McpServerConfig) -> McpConfigErrors {
    let mut errors = McpConfigErrors::default();
    let name = config.name.trim();
    if name.is_empty() {
        errors.name = Some("Enter a name for this MCP server.".to_string());
    } else if utf16_len(name) > INPUT_LIMITS.mcp_server_name {
        errors.name = Some(format!(
            "Use {} characters or fewer for the name.",
            INPUT_LIMITS.mcp_server_name
        ));
    } else if is_reserved_mcp_server_name(name)
        || name.eq_ignore_ascii_case(COMPUTER_USE_MCP_SERVER_NAME)
    {
        errors.name = Some(format!("OpenBot already uses the name {name}."));
    }
    if config.transport == McpTransport::Stdio {
        if config.command.trim().is_empty() {
            errors.command = Some("Enter the command that launches this server.".to_string());
        }
        return errors;
    }
    if !is_http_or_https_url(&config.url) {
        errors.url = Some("Enter an http or https address.".to_string());
    }
    if config.headers.iter().any(|h| !is_mcp_header_name(&h.key)) {
        errors.headers = Some("Invalid MCP HTTP header name.".to_string());
    }
    errors
}

pub fn is_mcp_server_config(config: &McpServerConfig) -> bool {
    utf16_len(&config.id) <= INPUT_LIMITS.identifier
        && utf16_len(&config.name) <= INPUT_LIMITS.mcp_server_name
        && utf16_len(&config.command) <= INPUT_LIMITS.mcp_command
        && config.args.len() <= INPUT_LIMITS.mcp_args
        && config
            .args
            .iter()
            .all(|a| utf16_len(a) <= INPUT_LIMITS.mcp_arg_value)
        && config.env.len() <= INPUT_LIMITS.mcp_env_variables
        && config.env.iter().all(|p| {
            utf16_len(&p.key) <= INPUT_LIMITS.mcp_env_name
                && utf16_len(&p.value) <= INPUT_LIMITS.mcp_env_value
        })
        && config.env_passthrough.len() <= INPUT_LIMITS.mcp_env_variables
        && config
            .env_passthrough
            .iter()
            .all(|k| utf16_len(k) <= INPUT_LIMITS.mcp_env_name)
        && utf16_len(&config.working_directory) <= INPUT_LIMITS.path
        && utf16_len(&config.url) <= INPUT_LIMITS.mcp_url
        && config.headers.len() <= INPUT_LIMITS.mcp_headers
        && config.headers.iter().all(|p| {
            utf16_len(&p.key) <= INPUT_LIMITS.mcp_env_name
                && utf16_len(&p.value) <= INPUT_LIMITS.mcp_header_value
        })
}

#[derive(Clone)]
pub struct McpServerStore {
    db: OpenBotDatabase,
}

impl McpServerStore {
    pub fn new(db: OpenBotDatabase) -> Self {
        Self { db }
    }

    pub fn list(&self) -> StorageResult<Vec<McpServerConfig>> {
        self.db.with_conn(|conn| {
            let mut stmt = conn.prepare(
                "SELECT mcp_server_id, name, transport, enabled, command, args_json, env_json, \
                        env_passthrough_json, working_directory, url, headers_json \
                 FROM projection_mcp_servers ORDER BY position, mcp_server_id",
            )?;
            let rows = stmt.query_map([], read_raw_mcp_row)?;
            let mut out = Vec::new();
            for r in rows {
                if let Some(stored) = decode_stored_mcp_server(r?) {
                    out.push(stored.config);
                }
            }
            Ok(out)
        })
    }

    pub fn list_enabled(&self) -> StorageResult<Vec<McpServerConfig>> {
        Ok(self.list()?.into_iter().filter(|c| c.enabled).collect())
    }

    pub fn get(&self, mcp_server_id: &str) -> StorageResult<Option<McpServerConfig>> {
        self.db
            .with_conn(|conn| Ok(read_stored_mcp_on_conn(conn, mcp_server_id)?.map(|s| s.config)))
    }

    pub fn save(
        &self,
        config: &McpServerConfig,
        now_iso: Option<&str>,
    ) -> StorageResult<McpServerConfig> {
        let normalized = normalize_mcp_config(config);
        let errs = mcp_config_errors(&normalized);
        if let Some(first_err) = errs.first_error() {
            return Err(StorageError::Validation(first_err.to_string()));
        }
        if !is_mcp_server_config(&normalized) {
            return Err(StorageError::Validation(
                "MCP server configuration exceeds field limits".to_string(),
            ));
        }
        let now = now_iso
            .map(str::to_string)
            .unwrap_or_else(|| ms_to_iso(now_millis()));

        self.db.with_tx(|conn| {
            let existing = if normalized.id.is_empty() {
                None
            } else {
                let Some(found) = read_stored_mcp_on_conn(conn, &normalized.id)? else {
                    return Err(StorageError::NotFound(format!(
                        "MCP server {} gone",
                        normalized.id
                    )));
                };
                Some(found.config)
            };
            if existing.is_none() {
                let cnt: i64 = conn.query_row(
                    "SELECT COUNT(*) FROM projection_mcp_servers",
                    [],
                    |r| r.get(0),
                )?;
                if (cnt as usize) >= INPUT_LIMITS.mcp_servers {
                    return Err(StorageError::LimitExceeded(format!(
                        "MCP server limit ({}) reached",
                        INPUT_LIMITS.mcp_servers
                    )));
                }
            }
            let except_id = existing.as_ref().map(|e| e.id.as_str());
            if mcp_name_taken_on_conn(conn, &normalized.name, except_id)? {
                return Err(StorageError::Conflict(format!(
                    "MCP server name '{}' is already taken",
                    normalized.name
                )));
            }
            let id = existing
                .as_ref()
                .map_or_else(create_mcp_server_id, |e| e.id.clone());
            let position = if existing.is_some() {
                conn.query_row(
                    "SELECT position FROM projection_mcp_servers WHERE mcp_server_id = ?1",
                    params![id],
                    |r| r.get::<_, i64>(0),
                )
                .optional()?
                .unwrap_or(0)
            } else {
                conn.query_row(
                    "SELECT COALESCE(MAX(position) + 1, 0) FROM projection_mcp_servers",
                    [],
                    |r| r.get::<_, i64>(0),
                )?
            };
            let stored = McpServerConfig {
                id,
                ..normalized
            };
            let args_json = serde_json::to_string(&stored.args)?;
            let env_json = serde_json::to_string(&stored.env)?;
            let passthrough_json = serde_json::to_string(&stored.env_passthrough)?;
            let headers_json = serde_json::to_string(&stored.headers)?;
            conn.execute(
                "INSERT INTO projection_mcp_servers \
                   (mcp_server_id, name, transport, enabled, command, args_json, env_json, \
                    env_passthrough_json, working_directory, url, headers_json, position, created_at, updated_at) \
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14) \
                 ON CONFLICT(mcp_server_id) DO UPDATE SET \
                   name = excluded.name, transport = excluded.transport, enabled = excluded.enabled, \
                   command = excluded.command, args_json = excluded.args_json, env_json = excluded.env_json, \
                   env_passthrough_json = excluded.env_passthrough_json, \
                   working_directory = excluded.working_directory, url = excluded.url, \
                   headers_json = excluded.headers_json, updated_at = excluded.updated_at",
                params![
                    stored.id,
                    stored.name,
                    stored.transport.as_str(),
                    i64::from(stored.enabled),
                    stored.command,
                    args_json,
                    env_json,
                    passthrough_json,
                    stored.working_directory,
                    stored.url,
                    headers_json,
                    position,
                    now,
                    now,
                ],
            )?;
            Ok(stored)
        })
    }

    pub fn remove(&self, mcp_server_id: &str) -> StorageResult<bool> {
        self.db.with_conn(|conn| {
            let changes = conn.execute(
                "DELETE FROM projection_mcp_servers WHERE mcp_server_id = ?1",
                params![mcp_server_id],
            )?;
            Ok(changes > 0)
        })
    }

    pub fn set_enabled(
        &self,
        mcp_server_id: &str,
        enabled: bool,
        now_iso: Option<&str>,
    ) -> StorageResult<McpServerConfig> {
        let now = now_iso
            .map(str::to_string)
            .unwrap_or_else(|| ms_to_iso(now_millis()));
        let stored = self
            .db
            .with_conn(|conn| read_stored_mcp_on_conn(conn, mcp_server_id))?
            .ok_or_else(|| StorageError::NotFound(format!("MCP server {mcp_server_id} gone")))?;
        let mut updated = stored.config;
        updated.enabled = enabled;
        if enabled && !stored.unreadable.is_empty() {
            return self.save(&updated, Some(&now));
        }
        self.db.with_conn(|conn| {
            conn.execute(
                "UPDATE projection_mcp_servers SET enabled = ?1, updated_at = ?2 WHERE mcp_server_id = ?3",
                params![i64::from(enabled), now, mcp_server_id],
            )?;
            Ok(updated)
        })
    }

    pub fn migrate_catalog_rows(&self, now_iso: Option<&str>) -> StorageResult<usize> {
        let now = now_iso
            .map(str::to_string)
            .unwrap_or_else(|| ms_to_iso(now_millis()));
        self.db.with_tx(|conn| {
            let rows: Vec<(String, String, String, String, String)> = {
                let mut stmt = conn.prepare(
                    "SELECT mcp_server_id, name, transport, command, args_json \
                     FROM projection_mcp_servers",
                )?;
                stmt.query_map([], |r| {
                    Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?))
                })?
                .collect::<rusqlite::Result<Vec<_>>>()?
            };
            let mut converted = 0usize;
            for (id, name, transport, command, args_json) in rows {
                if transport != "stdio" || command != "npx" {
                    continue;
                }
                let Ok(args) = serde_json::from_str::<Vec<String>>(&args_json) else {
                    continue;
                };
                if name == "apple-apps" && args == ["-y", "mcp-icloud@2.1.3"] {
                    let next_args = serde_json::to_string(&["-y", "mcp-icloud@2.1.4"])?;
                    conn.execute(
                        "UPDATE projection_mcp_servers SET args_json = ?1, updated_at = ?2 \
                         WHERE mcp_server_id = ?3",
                        params![next_args, now, id],
                    )?;
                    converted += 1;
                    continue;
                }
                if let Some(&(_, _, target_url)) = MCP_REMOTE_BRIDGES.iter().find(|&&(b_name, rem, _)| {
                    b_name == name && args == ["-y", "mcp-remote@latest", rem]
                }) {
                    conn.execute(
                        "UPDATE projection_mcp_servers \
                         SET transport = 'http', command = '', args_json = '[]', url = ?1, updated_at = ?2 \
                         WHERE mcp_server_id = ?3",
                        params![target_url, now, id],
                    )?;
                    converted += 1;
                }
            }
            Ok(converted)
        })
    }
}

struct RawMcpRow {
    id: String,
    name: String,
    transport: String,
    enabled: i64,
    command: String,
    args_json: String,
    env_json: String,
    env_passthrough_json: String,
    working_directory: String,
    url: String,
    headers_json: String,
}

struct StoredMcpServer {
    config: McpServerConfig,
    unreadable: Vec<&'static str>,
}

fn read_raw_mcp_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<RawMcpRow> {
    Ok(RawMcpRow {
        id: row.get(0)?,
        name: row.get(1)?,
        transport: row.get(2)?,
        enabled: row.get(3)?,
        command: row.get(4)?,
        args_json: row.get(5)?,
        env_json: row.get(6)?,
        env_passthrough_json: row.get(7)?,
        working_directory: row.get(8)?,
        url: row.get(9)?,
        headers_json: row.get(10)?,
    })
}

fn read_stored_mcp_on_conn(
    conn: &Connection,
    mcp_server_id: &str,
) -> StorageResult<Option<StoredMcpServer>> {
    let raw = conn
        .query_row(
            "SELECT mcp_server_id, name, transport, enabled, command, args_json, env_json, \
                    env_passthrough_json, working_directory, url, headers_json \
             FROM projection_mcp_servers WHERE mcp_server_id = ?1",
            params![mcp_server_id],
            read_raw_mcp_row,
        )
        .optional()?;
    Ok(raw.and_then(decode_stored_mcp_server))
}

fn mcp_name_taken_on_conn(
    conn: &Connection,
    name: &str,
    except_id: Option<&str>,
) -> StorageResult<bool> {
    if is_reserved_mcp_server_name(name) {
        return Ok(true);
    }
    let found: Option<i64> = conn
        .query_row(
            "SELECT 1 FROM projection_mcp_servers WHERE name = ?1 AND mcp_server_id IS NOT ?2",
            params![name, except_id],
            |r| r.get(0),
        )
        .optional()?;
    Ok(found.is_some())
}

fn decode_stored_mcp_server(raw: RawMcpRow) -> Option<StoredMcpServer> {
    let transport = McpTransport::parse(&raw.transport)?;
    if utf16_len(&raw.id) > INPUT_LIMITS.identifier
        || utf16_len(&raw.name) > INPUT_LIMITS.mcp_server_name
    {
        return None;
    }
    let mut unreadable = Vec::new();
    let command = if utf16_len(&raw.command) <= INPUT_LIMITS.mcp_command {
        raw.command
    } else {
        unreadable.push("command");
        String::new()
    };
    let working_directory = if utf16_len(&raw.working_directory) <= INPUT_LIMITS.path {
        raw.working_directory
    } else {
        unreadable.push("working_directory");
        String::new()
    };
    let url = if utf16_len(&raw.url) <= INPUT_LIMITS.mcp_url {
        raw.url
    } else {
        unreadable.push("url");
        String::new()
    };
    let args = match serde_json::from_str::<Vec<String>>(&raw.args_json) {
        Ok(list)
            if list.len() <= INPUT_LIMITS.mcp_args
                && list.iter().all(|a| utf16_len(a) <= INPUT_LIMITS.mcp_arg_value) =>
        {
            list
        }
        _ => {
            unreadable.push("args_json");
            Vec::new()
        }
    };
    let env = match serde_json::from_str::<Vec<McpKeyValue>>(&raw.env_json) {
        Ok(list)
            if list.len() <= INPUT_LIMITS.mcp_env_variables
                && list.iter().all(|p| {
                    utf16_len(&p.key) <= INPUT_LIMITS.mcp_env_name
                        && utf16_len(&p.value) <= INPUT_LIMITS.mcp_env_value
                }) =>
        {
            list
        }
        _ => {
            unreadable.push("env_json");
            Vec::new()
        }
    };
    let env_passthrough = match serde_json::from_str::<Vec<String>>(&raw.env_passthrough_json) {
        Ok(list)
            if list.len() <= INPUT_LIMITS.mcp_env_variables
                && list.iter().all(|k| utf16_len(k) <= INPUT_LIMITS.mcp_env_name) =>
        {
            list
        }
        _ => {
            unreadable.push("env_passthrough_json");
            Vec::new()
        }
    };
    let headers = match serde_json::from_str::<Vec<McpKeyValue>>(&raw.headers_json) {
        Ok(list)
            if list.len() <= INPUT_LIMITS.mcp_headers
                && list.iter().all(|p| {
                    utf16_len(&p.key) <= INPUT_LIMITS.mcp_env_name
                        && utf16_len(&p.value) <= INPUT_LIMITS.mcp_header_value
                }) =>
        {
            list
        }
        _ => {
            unreadable.push("headers_json");
            Vec::new()
        }
    };
    let enabled = raw.enabled == 1 && unreadable.is_empty();
    Some(StoredMcpServer {
        config: McpServerConfig {
            id: raw.id,
            name: raw.name,
            transport,
            enabled,
            command,
            args,
            env,
            env_passthrough,
            working_directory,
            url,
            headers,
        },
        unreadable,
    })
}
