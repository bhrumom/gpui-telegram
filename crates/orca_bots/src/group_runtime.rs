//! Multi-Bot Group Runtime (`BotGroupRuntime`):
//! - SQLite WAL store with all 7 upstream tables (`bot_groups`, `bot_group_messages`,
//!   `bot_group_operations`, `bot_group_pending`, `bot_group_worker_operations`,
//!   `bot_group_sidebar`, `bot_group_remote_asks`).
//! - Rollback-safe SQLite `AFTER UPDATE` triggers (`trg_bot_groups_preserve_newer_fields`,
//!   `trg_bot_group_messages_preserve_newer_fields`) preserving `"remoteMembers"`,
//!   `"images"`, and `"remoteAsks"` if an older build rewrites `payload_json` omitting them.
//! - Hidden group home folder `<bots_root>/group-<slug>` with atomic directory reservation.
//! - Sub-threads (`threads`, title <= 80), active vs `former_session_ids` isolation,
//!   3 reply modes (`Bot`, `All`, `Mentions`), `@mention` handoffs, `BotGroupPeerScope`,
//!   synthetic roots (`fromItems`, `fromReports`), paused-bot `bot_group_pending` hold/flush,
//!   cross-machine `RemoteBotMember` (max 31) + `RemoteAsk`, and 7-code inter-bot policy.

use std::collections::{BTreeMap, HashSet};
use std::fs::{self, File};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use orca_core::bot_groups::{
    ApprovedBotRequest, BOT_GROUP_FOLDER_PREFIX, BotGroup, BotGroupFolderSlug, BotGroupImage,
    BotGroupPeerScope, BotGroupReplyMode, BotGroupSend, BotGroupSyntheticRoot, BotGroupThread,
    BotRequestPolicyContext, BotRequestPolicyInput, GroupDispatchTargets, GroupSessionLookup,
    MentionCandidate, RemoteAsk, RemoteAskReply, RemoteAskState, RemoteBotMember,
    bot_group_folder_name, derive_group_thread_title, evaluate_bot_request_policy,
    resolve_group_targets, validate_group_thread_title,
};
use orca_core::BotErrorCode;
use rusqlite::{Connection, OptionalExtension, params};
use serde::{Deserialize, Serialize};
use serde_json::{Map as JsonMap, Value as JsonValue};
use uuid::Uuid;

use crate::{OrcaBotsError, Result, now_epoch_ms};

const MAX_GROUP_FOLDER_SLUG_LEN: usize = 46;
const MAX_UNIQUE_GROUP_FOLDER_ATTEMPTS: usize = 10_000;
const GROUP_MANIFEST_FILE: &str = "group.json";

const GROUP_SCHEMA_SQL: &str = r#"
CREATE TABLE IF NOT EXISTS bot_groups (
    id TEXT PRIMARY KEY NOT NULL,
    name TEXT NOT NULL,
    folder TEXT NOT NULL UNIQUE,
    reply_mode_json TEXT NOT NULL,
    payload_json TEXT NOT NULL,
    created_at_ms INTEGER NOT NULL,
    updated_at_ms INTEGER NOT NULL
);

CREATE TABLE IF NOT EXISTS bot_group_messages (
    id TEXT PRIMARY KEY NOT NULL,
    group_id TEXT NOT NULL,
    thread_id TEXT,
    sequence INTEGER NOT NULL,
    sender_kind TEXT NOT NULL,
    sender_bot_id TEXT,
    text TEXT NOT NULL,
    requester_json TEXT,
    answered_by_json TEXT NOT NULL,
    mailed_json TEXT NOT NULL,
    handed_off INTEGER NOT NULL DEFAULT 0,
    peer_scope_json TEXT,
    synthetic_root TEXT,
    payload_json TEXT NOT NULL,
    created_at_ms INTEGER NOT NULL,
    FOREIGN KEY (group_id) REFERENCES bot_groups(id) ON DELETE CASCADE
);
CREATE UNIQUE INDEX IF NOT EXISTS idx_bgm_room_seq
    ON bot_group_messages(group_id, COALESCE(thread_id, ''), sequence);
CREATE INDEX IF NOT EXISTS idx_bgm_group_created
    ON bot_group_messages(group_id, created_at_ms);

CREATE TABLE IF NOT EXISTS bot_group_operations (
    id TEXT PRIMARY KEY NOT NULL,
    group_id TEXT NOT NULL,
    thread_id TEXT,
    message_id TEXT,
    bot_id TEXT NOT NULL,
    session_id TEXT NOT NULL,
    kind TEXT NOT NULL,
    state TEXT NOT NULL,
    peer_scope_json TEXT,
    synthetic_root TEXT,
    started_at_ms INTEGER NOT NULL,
    finished_at_ms INTEGER,
    FOREIGN KEY (group_id) REFERENCES bot_groups(id) ON DELETE CASCADE
);
CREATE INDEX IF NOT EXISTS idx_bgo_group_state
    ON bot_group_operations(group_id, state);

CREATE TABLE IF NOT EXISTS bot_group_pending (
    id TEXT PRIMARY KEY NOT NULL,
    group_id TEXT NOT NULL,
    thread_id TEXT,
    message_id TEXT NOT NULL,
    bot_id TEXT NOT NULL,
    session_id TEXT NOT NULL,
    reason TEXT NOT NULL,
    payload_json TEXT NOT NULL,
    created_at_ms INTEGER NOT NULL,
    delivered_at_ms INTEGER,
    FOREIGN KEY (group_id) REFERENCES bot_groups(id) ON DELETE CASCADE,
    FOREIGN KEY (message_id) REFERENCES bot_group_messages(id) ON DELETE CASCADE
);
CREATE INDEX IF NOT EXISTS idx_bgp_bot_undelivered
    ON bot_group_pending(bot_id, delivered_at_ms);

CREATE TABLE IF NOT EXISTS bot_group_worker_operations (
    id TEXT PRIMARY KEY NOT NULL,
    group_id TEXT NOT NULL,
    thread_id TEXT,
    parent_bot_id TEXT NOT NULL,
    parent_session_id TEXT NOT NULL,
    worker_dispatch_id TEXT NOT NULL,
    state TEXT NOT NULL,
    report_subject TEXT,
    report_body TEXT,
    created_at_ms INTEGER NOT NULL,
    updated_at_ms INTEGER NOT NULL,
    FOREIGN KEY (group_id) REFERENCES bot_groups(id) ON DELETE CASCADE
);
CREATE INDEX IF NOT EXISTS idx_bgwo_group_state
    ON bot_group_worker_operations(group_id, state);

CREATE TABLE IF NOT EXISTS bot_group_sidebar (
    group_id TEXT PRIMARY KEY NOT NULL,
    sidebar_order INTEGER NOT NULL DEFAULT 0,
    pinned INTEGER NOT NULL DEFAULT 0,
    unread_count INTEGER NOT NULL DEFAULT 0,
    last_read_sequence INTEGER NOT NULL DEFAULT 0,
    last_message_at_ms INTEGER,
    updated_at_ms INTEGER NOT NULL,
    FOREIGN KEY (group_id) REFERENCES bot_groups(id) ON DELETE CASCADE
);

CREATE TABLE IF NOT EXISTS bot_group_remote_asks (
    id TEXT PRIMARY KEY NOT NULL,
    group_id TEXT NOT NULL,
    thread_id TEXT,
    message_id TEXT NOT NULL,
    host_id TEXT NOT NULL,
    bot_id TEXT NOT NULL,
    chat_session_id TEXT NOT NULL,
    state TEXT NOT NULL,
    reply_json TEXT,
    created_at_ms INTEGER NOT NULL,
    updated_at_ms INTEGER,
    FOREIGN KEY (group_id) REFERENCES bot_groups(id) ON DELETE CASCADE,
    FOREIGN KEY (message_id) REFERENCES bot_group_messages(id) ON DELETE CASCADE
);
CREATE INDEX IF NOT EXISTS idx_bgra_group_msg
    ON bot_group_remote_asks(group_id, message_id);

CREATE TRIGGER IF NOT EXISTS trg_bot_groups_preserve_newer_fields
AFTER UPDATE OF payload_json ON bot_groups
FOR EACH ROW
WHEN json_valid(OLD.payload_json)
 AND json_valid(NEW.payload_json)
 AND (
     (json_type(OLD.payload_json, '$.remoteMembers') IS NOT NULL
      AND json_type(NEW.payload_json, '$.remoteMembers') IS NULL)
  OR (json_type(OLD.payload_json, '$.images') IS NOT NULL
      AND json_type(NEW.payload_json, '$.images') IS NULL)
 )
BEGIN
    UPDATE bot_groups
    SET payload_json = CASE
        WHEN json_type(OLD.payload_json, '$.remoteMembers') IS NOT NULL
         AND json_type(NEW.payload_json, '$.remoteMembers') IS NULL
         AND json_type(OLD.payload_json, '$.images') IS NOT NULL
         AND json_type(NEW.payload_json, '$.images') IS NULL
        THEN json_set(
            NEW.payload_json,
            '$.remoteMembers', json(json_extract(OLD.payload_json, '$.remoteMembers')),
            '$.images', json(json_extract(OLD.payload_json, '$.images'))
        )
        WHEN json_type(OLD.payload_json, '$.remoteMembers') IS NOT NULL
         AND json_type(NEW.payload_json, '$.remoteMembers') IS NULL
        THEN json_set(
            NEW.payload_json,
            '$.remoteMembers', json(json_extract(OLD.payload_json, '$.remoteMembers'))
        )
        ELSE json_set(
            NEW.payload_json,
            '$.images', json(json_extract(OLD.payload_json, '$.images'))
        )
    END
    WHERE id = NEW.id;
END;

CREATE TRIGGER IF NOT EXISTS trg_bot_group_messages_preserve_newer_fields
AFTER UPDATE OF payload_json ON bot_group_messages
FOR EACH ROW
WHEN json_valid(OLD.payload_json)
 AND json_valid(NEW.payload_json)
 AND (
     (json_type(OLD.payload_json, '$.images') IS NOT NULL
      AND json_type(NEW.payload_json, '$.images') IS NULL)
  OR (json_type(OLD.payload_json, '$.remoteAsks') IS NOT NULL
      AND json_type(NEW.payload_json, '$.remoteAsks') IS NULL)
 )
BEGIN
    UPDATE bot_group_messages
    SET payload_json = CASE
        WHEN json_type(OLD.payload_json, '$.images') IS NOT NULL
         AND json_type(NEW.payload_json, '$.images') IS NULL
         AND json_type(OLD.payload_json, '$.remoteAsks') IS NOT NULL
         AND json_type(NEW.payload_json, '$.remoteAsks') IS NULL
        THEN json_set(
            NEW.payload_json,
            '$.images', json(json_extract(OLD.payload_json, '$.images')),
            '$.remoteAsks', json(json_extract(OLD.payload_json, '$.remoteAsks'))
        )
        WHEN json_type(OLD.payload_json, '$.images') IS NOT NULL
         AND json_type(NEW.payload_json, '$.images') IS NULL
        THEN json_set(
            NEW.payload_json,
            '$.images', json(json_extract(OLD.payload_json, '$.images'))
        )
        ELSE json_set(
            NEW.payload_json,
            '$.remoteAsks', json(json_extract(OLD.payload_json, '$.remoteAsks'))
        )
    END
    WHERE id = NEW.id;
END;
"#;

/// Sender classification for a group room message.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum GroupMessageSender {
    Owner,
    Bot { bot_id: String },
    RemoteBot { host_id: String, bot_id: String },
    System,
}

impl GroupMessageSender {
    fn kind_str(&self) -> &'static str {
        match self {
            Self::Owner => "owner",
            Self::Bot { .. } => "bot",
            Self::RemoteBot { .. } => "remote_bot",
            Self::System => "system",
        }
    }

    fn bot_id(&self) -> Option<&str> {
        match self {
            Self::Bot { bot_id } | Self::RemoteBot { bot_id, .. } => Some(bot_id.as_str()),
            Self::Owner | Self::System => None,
        }
    }
}

/// Who initiated a group message or synthetic request root.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum GroupMessageRequester {
    Owner,
    Bot {
        bot_id: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        session_id: Option<String>,
    },
    FromItems {
        item_uid: String,
        item_number: u64,
    },
    FromReports {
        worker_dispatch_id: String,
        parent_bot_id: String,
    },
}

/// Request to create a new multi-bot group room and reserve its hidden folder.
#[derive(Debug, Clone)]
pub struct CreateBotGroupRequest {
    pub name: String,
    pub folder: Option<BotGroupFolderSlug>,
    pub member_bot_ids: Vec<String>,
    pub reply_mode: BotGroupReplyMode,
    pub remote_members: Vec<RemoteBotMember>,
}

/// Request to post a message into a group room or sub-thread.
#[derive(Debug, Clone)]
pub struct PostGroupMessageRequest {
    pub group_id: String,
    pub thread_id: Option<String>,
    pub sender: GroupMessageSender,
    pub send: BotGroupSend,
    pub active_local_candidates: Vec<MentionCandidate>,
    pub paused_bot_ids: HashSet<String>,
    pub requester: Option<GroupMessageRequester>,
    pub peer_scope: Option<BotGroupPeerScope>,
    pub synthetic_root: Option<BotGroupSyntheticRoot>,
}

/// Persisted group message row hydrated with `images` and `remote_asks`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GroupMessageRecord {
    pub id: String,
    pub group_id: String,
    pub thread_id: Option<String>,
    pub sequence: u64,
    pub sender_kind: String,
    pub sender_bot_id: Option<String>,
    pub text: String,
    pub requester: Option<GroupMessageRequester>,
    pub answered_by: Vec<String>,
    pub mailed: Vec<String>,
    pub handed_off: bool,
    pub peer_scope: Option<BotGroupPeerScope>,
    pub synthetic_root: Option<BotGroupSyntheticRoot>,
    pub images: Vec<BotGroupImage>,
    pub remote_asks: Vec<RemoteAsk>,
    pub created_at_ms: i64,
}

/// Outcome of posting a message to a group room.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PostedGroupMessage {
    pub message: GroupMessageRecord,
    pub targets: GroupDispatchTargets,
    pub operation_ids: Vec<String>,
    pub pending_ids: Vec<String>,
    pub remote_asks: Vec<RemoteAsk>,
}

/// Active or completed operation tracked in `bot_group_operations`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GroupOperationRecord {
    pub id: String,
    pub group_id: String,
    pub thread_id: Option<String>,
    pub message_id: Option<String>,
    pub bot_id: String,
    pub session_id: String,
    pub kind: String,
    pub state: String,
    pub peer_scope: Option<BotGroupPeerScope>,
    pub synthetic_root: Option<BotGroupSyntheticRoot>,
    pub started_at_ms: i64,
    pub finished_at_ms: Option<i64>,
}

/// Queued mail held in `bot_group_pending` while a target member bot is paused.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GroupPendingMail {
    pub id: String,
    pub group_id: String,
    pub thread_id: Option<String>,
    pub message_id: String,
    pub bot_id: String,
    pub session_id: String,
    pub reason: String,
    pub text: String,
    pub created_at_ms: i64,
    pub delivered_at_ms: Option<i64>,
}

/// Worker execution row tracked in `bot_group_worker_operations`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GroupWorkerOperation {
    pub id: String,
    pub group_id: String,
    pub thread_id: Option<String>,
    pub parent_bot_id: String,
    pub parent_session_id: String,
    pub worker_dispatch_id: String,
    pub state: String,
    pub report_subject: Option<String>,
    pub report_body: Option<String>,
    pub created_at_ms: i64,
    pub updated_at_ms: i64,
}

/// Sidebar ordering, pin, and unread tracking row from `bot_group_sidebar`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GroupSidebarEntry {
    pub group_id: String,
    pub sidebar_order: i64,
    pub pinned: bool,
    pub unread_count: u64,
    pub last_read_sequence: u64,
    pub last_message_at_ms: Option<i64>,
    pub updated_at_ms: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct GroupFolderManifest {
    id: String,
    name: String,
    folder: String,
    created_at_ms: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct GroupMessagePayload {
    images: Vec<BotGroupImage>,
    remote_asks: Vec<RemoteAsk>,
}

fn serialize_group_payload(group: &BotGroup) -> Result<String> {
    let mut value = serde_json::to_value(group)?;
    if let Some(obj) = value.as_object_mut() {
        if !obj.contains_key("remoteMembers") {
            obj.insert("remoteMembers".to_string(), JsonValue::Array(Vec::new()));
        }
        if !obj.contains_key("images") {
            obj.insert("images".to_string(), JsonValue::Array(Vec::new()));
        }
        if !obj.contains_key("threads") {
            obj.insert("threads".to_string(), JsonValue::Array(Vec::new()));
        }
    }
    Ok(serde_json::to_string(&value)?)
}

fn serialize_message_payload(images: &[BotGroupImage], remote_asks: &[RemoteAsk]) -> Result<String> {
    let mut obj = JsonMap::new();
    obj.insert("images".to_string(), serde_json::to_value(images)?);
    obj.insert("remoteAsks".to_string(), serde_json::to_value(remote_asks)?);
    Ok(serde_json::to_string(&JsonValue::Object(obj))?)
}

fn synthetic_root_to_str(root: BotGroupSyntheticRoot) -> &'static str {
    match root {
        BotGroupSyntheticRoot::FromItems => "fromItems",
        BotGroupSyntheticRoot::FromReports => "fromReports",
    }
}

fn synthetic_root_from_str(raw: &str) -> Option<BotGroupSyntheticRoot> {
    match raw {
        "fromItems" | "from_items" => Some(BotGroupSyntheticRoot::FromItems),
        "fromReports" | "from_reports" => Some(BotGroupSyntheticRoot::FromReports),
        _ => None,
    }
}

fn atomic_write_bytes(path: &Path, bytes: &[u8]) -> Result<()> {
    if let Some(parent) = path.parent()
        && !parent.as_os_str().is_empty()
    {
        fs::create_dir_all(parent)?;
    }
    let tmp_name = format!(
        ".{}.tmp-{}",
        path.file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("group"),
        Uuid::new_v4()
    );
    let tmp_path = path.with_file_name(tmp_name);
    {
        let mut file = File::create(&tmp_path)?;
        file.write_all(bytes)?;
        file.sync_all()?;
    }
    if let Err(err) = fs::rename(&tmp_path, path) {
        let _ = fs::remove_file(&tmp_path);
        return Err(OrcaBotsError::Io(err));
    }
    Ok(())
}

/// Persistent SQLite WAL runtime for multi-bot group rooms, sub-threads,
/// paused-bot mail queues, remote asks, worker reports, and inter-bot policy.
#[derive(Clone)]
pub struct BotGroupRuntime {
    conn: Arc<Mutex<Connection>>,
    bots_root: PathBuf,
}

impl std::fmt::Debug for BotGroupRuntime {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("BotGroupRuntime")
            .field("bots_root", &self.bots_root)
            .finish()
    }
}

impl BotGroupRuntime {
    /// Opens or creates a persistent WAL SQLite group database alongside `bots_root`.
    pub fn open(db_path: impl AsRef<Path>, bots_root: impl Into<PathBuf>) -> Result<Self> {
        let db_path = db_path.as_ref();
        if let Some(parent) = db_path.parent()
            && !parent.as_os_str().is_empty()
        {
            fs::create_dir_all(parent)?;
        }
        let conn = Connection::open(db_path)?;
        Self::init_connection(conn, bots_root.into())
    }

    /// Opens an in-memory SQLite database backed by `bots_root` for isolated testing.
    pub fn open_in_memory(bots_root: impl Into<PathBuf>) -> Result<Self> {
        let conn = Connection::open_in_memory()?;
        Self::init_connection(conn, bots_root.into())
    }

    fn init_connection(conn: Connection, bots_root: PathBuf) -> Result<Self> {
        conn.execute_batch(
            "PRAGMA journal_mode = WAL;
             PRAGMA foreign_keys = ON;
             PRAGMA busy_timeout = 5000;",
        )?;
        conn.execute_batch(GROUP_SCHEMA_SQL)?;
        Ok(Self {
            conn: Arc::new(Mutex::new(conn)),
            bots_root,
        })
    }

    fn lock_conn(&self) -> Result<std::sync::MutexGuard<'_, Connection>> {
        self.conn.lock().map_err(|_| {
            OrcaBotsError::bot(
                BotErrorCode::ItemsUnavailable,
                "BotGroupRuntime SQLite mutex poisoned.",
            )
        })
    }

    /// Returns the configured bots root where hidden `group-<slug>` folders live.
    pub fn bots_root(&self) -> &Path {
        &self.bots_root
    }

    /// Resolves `<bots_root>/<folder>` for a validated `BotGroupFolderSlug`.
    pub fn group_home_dir(&self, folder: &BotGroupFolderSlug) -> PathBuf {
        self.bots_root.join(folder.as_str())
    }

    /// Atomically reserves a hidden `<bots_root>/group-<slug>` directory using `create_dir`.
    pub fn reserve_group_folder(&self, slug: &BotGroupFolderSlug) -> Result<PathBuf> {
        BotGroupFolderSlug::new(slug.as_str())?;
        fs::create_dir_all(&self.bots_root)?;
        let dir = self.group_home_dir(slug);
        fs::create_dir(&dir)?;
        Ok(dir)
    }

    /// Atomically reserves a unique `group-<slug>` directory derived from `group_name`.
    pub fn reserve_unique_group_folder(&self, group_name: &str) -> Result<(BotGroupFolderSlug, PathBuf)> {
        let base = bot_group_folder_name(group_name);
        match self.reserve_group_folder(&base) {
            Ok(dir) => return Ok((base, dir)),
            Err(OrcaBotsError::Io(ref err)) if err.kind() == io::ErrorKind::AlreadyExists => {}
            Err(err) => return Err(err),
        }

        let base_tail = base
            .as_str()
            .strip_prefix(BOT_GROUP_FOLDER_PREFIX)
            .unwrap_or("room");
        for n in 2..=MAX_UNIQUE_GROUP_FOLDER_ATTEMPTS {
            let suffix = format!("-{n}");
            let max_tail = MAX_GROUP_FOLDER_SLUG_LEN
                .saturating_sub(BOT_GROUP_FOLDER_PREFIX.len() + suffix.len());
            let trimmed = base_tail
                .chars()
                .take(max_tail)
                .collect::<String>()
                .trim_end_matches('-')
                .to_string();
            let candidate = if trimmed.is_empty() {
                format!("{BOT_GROUP_FOLDER_PREFIX}room{suffix}")
            } else {
                format!("{BOT_GROUP_FOLDER_PREFIX}{trimmed}{suffix}")
            };
            let slug = BotGroupFolderSlug::new(&candidate)?;
            match self.reserve_group_folder(&slug) {
                Ok(dir) => return Ok((slug, dir)),
                Err(OrcaBotsError::Io(ref err)) if err.kind() == io::ErrorKind::AlreadyExists => {
                    continue;
                }
                Err(err) => return Err(err),
            }
        }
        Err(OrcaBotsError::bot(
            BotErrorCode::Full,
            format!("Unable to reserve unique group folder for '{group_name}'."),
        ))
    }

    /// Creates a new multi-bot group room, reserves its hidden folder, spawns fresh
    /// member sessions, and inserts `bot_groups` + `bot_group_sidebar` rows.
    pub fn create_group(&self, req: CreateBotGroupRequest) -> Result<BotGroup> {
        let now = now_epoch_ms();
        let group_id = format!("grp-{}", Uuid::new_v4());
        let (folder, dir) = match req.folder {
            Some(explicit) => {
                let d = self.reserve_group_folder(&explicit)?;
                (explicit, d)
            }
            None => self.reserve_unique_group_folder(&req.name)?,
        };

        let mut session_ids = BTreeMap::new();
        for bot_id in &req.member_bot_ids {
            session_ids.insert(bot_id.clone(), format!("grp-sess-{}", Uuid::new_v4()));
        }

        let group = BotGroup {
            id: group_id.clone(),
            name: req.name.trim().to_string(),
            folder: folder.clone(),
            member_bot_ids: req.member_bot_ids,
            session_ids,
            former_session_ids: BTreeMap::new(),
            reply_mode: req.reply_mode,
            threads: Vec::new(),
            remote_members: req.remote_members,
            images: Vec::new(),
            created_at_ms: now,
            updated_at_ms: now,
        };
        if let Err(err) = group.validate() {
            let _ = fs::remove_dir(&dir);
            return Err(OrcaBotsError::Bot(err));
        }

        let manifest = GroupFolderManifest {
            id: group.id.clone(),
            name: group.name.clone(),
            folder: group.folder.as_str().to_string(),
            created_at_ms: now,
        };
        let manifest_bytes = serde_json::to_vec_pretty(&manifest)?;
        if let Err(err) = atomic_write_bytes(&dir.join(GROUP_MANIFEST_FILE), &manifest_bytes) {
            let _ = fs::remove_dir_all(&dir);
            return Err(err);
        }

        let reply_mode_json = serde_json::to_string(&group.reply_mode)?;
        let payload_json = serialize_group_payload(&group)?;

        let conn = self.lock_conn()?;
        let res = (|| -> Result<()> {
            conn.execute(
                "INSERT INTO bot_groups (id, name, folder, reply_mode_json, payload_json, created_at_ms, updated_at_ms)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
                params![
                    group.id,
                    group.name,
                    group.folder.as_str(),
                    reply_mode_json,
                    payload_json,
                    group.created_at_ms,
                    group.updated_at_ms
                ],
            )?;
            conn.execute(
                "INSERT INTO bot_group_sidebar (group_id, sidebar_order, pinned, unread_count, last_read_sequence, last_message_at_ms, updated_at_ms)
                 VALUES (?1, 0, 0, 0, 0, NULL, ?2)",
                params![group.id, now],
            )?;
            Ok(())
        })();

        if let Err(err) = res {
            let _ = fs::remove_dir_all(&dir);
            return Err(err);
        }
        Ok(group)
    }

    fn load_group_locked(conn: &Connection, group_id: &str) -> Result<Option<BotGroup>> {
        let row: Option<String> = conn
            .query_row(
                "SELECT payload_json FROM bot_groups WHERE id = ?1",
                params![group_id],
                |r| r.get(0),
            )
            .optional()?;
        match row {
            Some(payload) => Ok(Some(serde_json::from_str(&payload)?)),
            None => Ok(None),
        }
    }

    fn require_group_locked(conn: &Connection, group_id: &str) -> Result<BotGroup> {
        Self::load_group_locked(conn, group_id)?.ok_or_else(|| {
            OrcaBotsError::bot(
                BotErrorCode::NotFound,
                format!("Bot group '{group_id}' was not found."),
            )
        })
    }

    fn persist_group_locked(conn: &Connection, group: &BotGroup) -> Result<()> {
        group.validate()?;
        let reply_mode_json = serde_json::to_string(&group.reply_mode)?;
        let payload_json = serialize_group_payload(group)?;
        conn.execute(
            "UPDATE bot_groups
             SET name = ?1, reply_mode_json = ?2, payload_json = ?3, updated_at_ms = ?4
             WHERE id = ?5",
            params![
                group.name,
                reply_mode_json,
                payload_json,
                group.updated_at_ms,
                group.id
            ],
        )?;
        Ok(())
    }

    /// Loads a group by ID, returning `Ok(None)` if not present.
    pub fn get_group(&self, group_id: &str) -> Result<Option<BotGroup>> {
        let conn = self.lock_conn()?;
        Self::load_group_locked(&conn, group_id)
    }

    /// Lists all persisted bot groups ordered by sidebar order then creation time.
    pub fn list_groups(&self) -> Result<Vec<BotGroup>> {
        let conn = self.lock_conn()?;
        let mut stmt = conn.prepare(
            "SELECT g.payload_json
             FROM bot_groups g
             LEFT JOIN bot_group_sidebar s ON s.group_id = g.id
             ORDER BY COALESCE(s.pinned, 0) DESC, COALESCE(s.sidebar_order, 0) ASC, g.created_at_ms ASC",
        )?;
        let mut rows = stmt.query([])?;
        let mut groups = Vec::new();
        while let Some(row) = rows.next()? {
            let payload: String = row.get(0)?;
            groups.push(serde_json::from_str(&payload)?);
        }
        Ok(groups)
    }

    /// Deletes a group and releases its hidden folder only if `group.json` matches `group_id`.
    pub fn delete_group(&self, group_id: &str) -> Result<bool> {
        let folder = {
            let conn = self.lock_conn()?;
            let Some(group) = Self::load_group_locked(&conn, group_id)? else {
                return Ok(false);
            };
            conn.execute("DELETE FROM bot_groups WHERE id = ?1", params![group_id])?;
            group.folder
        };

        let dir = self.group_home_dir(&folder);
        let manifest_path = dir.join(GROUP_MANIFEST_FILE);
        if let Ok(raw) = fs::read_to_string(&manifest_path)
            && let Ok(manifest) = serde_json::from_str::<GroupFolderManifest>(&raw)
            && manifest.id == group_id
        {
            let _ = fs::remove_dir_all(&dir);
        }
        Ok(true)
    }

    /// Updates a group's reply mode (`Bot { bot_id }`, `All`, or `Mentions`).
    pub fn update_reply_mode(
        &self,
        group_id: &str,
        reply_mode: BotGroupReplyMode,
    ) -> Result<BotGroup> {
        let conn = self.lock_conn()?;
        let mut group = Self::require_group_locked(&conn, group_id)?;
        group.reply_mode = reply_mode;
        group.updated_at_ms = now_epoch_ms();
        Self::persist_group_locked(&conn, &group)?;
        Ok(group)
    }

    /// Adds a local member bot and spawns fresh root + sub-thread session IDs for it.
    pub fn add_member(&self, group_id: &str, bot_id: impl Into<String>) -> Result<BotGroup> {
        let bot_id = bot_id.into();
        let conn = self.lock_conn()?;
        let mut group = Self::require_group_locked(&conn, group_id)?;
        if !group.member_bot_ids.iter().any(|m| m == &bot_id) {
            group.member_bot_ids.push(bot_id.clone());
        }
        group
            .session_ids
            .entry(bot_id.clone())
            .or_insert_with(|| format!("grp-sess-{}", Uuid::new_v4()));
        for thread in &mut group.threads {
            thread
                .session_ids
                .entry(bot_id.clone())
                .or_insert_with(|| format!("grp-thr-sess-{}", Uuid::new_v4()));
        }
        group.updated_at_ms = now_epoch_ms();
        Self::persist_group_locked(&conn, &group)?;
        Ok(group)
    }

    /// Removes a local member bot, archiving active root & thread sessions into
    /// `former_session_ids` (read-only history, zero active access).
    pub fn remove_member(&self, group_id: &str, bot_id: &str) -> Result<BotGroup> {
        let conn = self.lock_conn()?;
        let mut group = Self::require_group_locked(&conn, group_id)?;
        group.remove_member(bot_id);
        group.updated_at_ms = now_epoch_ms();
        Self::persist_group_locked(&conn, &group)?;
        Ok(group)
    }

    /// Adds a pinned cross-machine `RemoteBotMember` (enforces max 31 & unique `(host_id, bot_id)`).
    pub fn add_remote_member(&self, group_id: &str, member: RemoteBotMember) -> Result<BotGroup> {
        member.validate()?;
        let conn = self.lock_conn()?;
        let mut group = Self::require_group_locked(&conn, group_id)?;
        group.remote_members.push(member);
        group.updated_at_ms = now_epoch_ms();
        Self::persist_group_locked(&conn, &group)?;
        Ok(group)
    }

    /// Renames a remote member while preserving its pinned `chat_session_id`.
    pub fn rename_remote_member(
        &self,
        group_id: &str,
        host_id: &str,
        bot_id: &str,
        new_name: &str,
    ) -> Result<BotGroup> {
        let conn = self.lock_conn()?;
        let mut group = Self::require_group_locked(&conn, group_id)?;
        let Some(rm) = group
            .remote_members
            .iter_mut()
            .find(|r| r.host_id == host_id && r.bot_id == bot_id)
        else {
            return Err(OrcaBotsError::bot(
                BotErrorCode::BotNotFound,
                format!("Remote member '{host_id}:{bot_id}' not found in group '{group_id}'."),
            ));
        };
        rm.rename(new_name)?;
        group.updated_at_ms = now_epoch_ms();
        Self::persist_group_locked(&conn, &group)?;
        Ok(group)
    }

    /// Removes a remote member from the group cleanly (writes explicit `"remoteMembers": [...]`).
    pub fn remove_remote_member(
        &self,
        group_id: &str,
        host_id: &str,
        bot_id: &str,
    ) -> Result<BotGroup> {
        let conn = self.lock_conn()?;
        let mut group = Self::require_group_locked(&conn, group_id)?;
        group
            .remote_members
            .retain(|r| !(r.host_id == host_id && r.bot_id == bot_id));
        group.updated_at_ms = now_epoch_ms();
        Self::persist_group_locked(&conn, &group)?;
        Ok(group)
    }

    /// Opens a sub-thread with title (`<= 80` chars) derived from `first_post`, spawning
    /// fresh session IDs for every currently active local member bot.
    pub fn create_thread(&self, group_id: &str, first_post: &str) -> Result<BotGroupThread> {
        let title = validate_group_thread_title(&derive_group_thread_title(first_post))?;
        let now = now_epoch_ms();
        let conn = self.lock_conn()?;
        let mut group = Self::require_group_locked(&conn, group_id)?;

        let mut session_ids = BTreeMap::new();
        for bot_id in &group.member_bot_ids {
            session_ids.insert(bot_id.clone(), format!("grp-thr-sess-{}", Uuid::new_v4()));
        }
        let thread = BotGroupThread {
            id: format!("thr-{}", Uuid::new_v4()),
            title,
            session_ids,
            former_session_ids: BTreeMap::new(),
            created_at_ms: now,
            updated_at_ms: now,
        };
        thread.validate()?;
        group.threads.push(thread.clone());
        group.updated_at_ms = now;
        Self::persist_group_locked(&conn, &group)?;
        Ok(thread)
    }

    /// Lists all sub-threads in a group.
    pub fn list_threads(&self, group_id: &str) -> Result<Vec<BotGroupThread>> {
        let conn = self.lock_conn()?;
        let group = Self::require_group_locked(&conn, group_id)?;
        Ok(group.threads)
    }

    /// Resolves whether a session belongs to an active or former member in this group.
    pub fn session_lookup(&self, group_id: &str, session_id: &str) -> Result<Option<GroupSessionLookup>> {
        let conn = self.lock_conn()?;
        let group = Self::require_group_locked(&conn, group_id)?;
        Ok(group.find_room_of_session(session_id))
    }

    /// Returns `true` only if `session_id` has active (non-former) access to `group_id`.
    pub fn has_active_session_access(&self, group_id: &str, session_id: &str) -> Result<bool> {
        let conn = self.lock_conn()?;
        let group = Self::require_group_locked(&conn, group_id)?;
        Ok(group.has_active_session_access(session_id))
    }

    fn room_session_for_bot(
        group: &BotGroup,
        thread_id: Option<&str>,
        bot_id: &str,
    ) -> Result<String> {
        let map = match thread_id {
            Some(tid) => {
                let thread = group.threads.iter().find(|t| t.id == tid).ok_or_else(|| {
                    OrcaBotsError::bot(
                        BotErrorCode::NotFound,
                        format!("Thread '{tid}' was not found in group '{}'.", group.id),
                    )
                })?;
                &thread.session_ids
            }
            None => &group.session_ids,
        };
        map.get(bot_id).cloned().ok_or_else(|| {
            OrcaBotsError::bot(
                BotErrorCode::BotChatNotFound,
                format!("No active session for bot '{bot_id}' in group '{}'.", group.id),
            )
        })
    }

    /// Posts a message into a group room or sub-thread, resolves routing across all
    /// 3 reply modes + `@mention` handoffs, dispatches active targets, holds paused
    /// targets in `bot_group_pending`, and tracks pinned `bot_group_remote_asks`.
    pub fn post_message(&self, req: PostGroupMessageRequest) -> Result<PostedGroupMessage> {
        req.send.validate()?;
        if let Some(ref scope) = req.peer_scope {
            scope.validate()?;
        }

        let now = now_epoch_ms();
        let conn = self.lock_conn()?;
        let group = Self::require_group_locked(&conn, &req.group_id)?;
        if let Some(ref tid) = req.thread_id
            && !group.threads.iter().any(|t| &t.id == tid)
        {
            return Err(OrcaBotsError::bot(
                BotErrorCode::NotFound,
                format!("Thread '{tid}' was not found in group '{}'.", group.id),
            ));
        }

        let sender_bot_id = match &req.sender {
            GroupMessageSender::Bot { bot_id } => Some(bot_id.as_str()),
            _ => None,
        };
        let targets = resolve_group_targets(
            &group,
            &req.send.text,
            sender_bot_id,
            &req.active_local_candidates,
            &req.paused_bot_ids,
        );

        let room_key = req.thread_id.as_deref().unwrap_or("");
        let last_seq: i64 = conn.query_row(
            "SELECT COALESCE(MAX(sequence), 0)
             FROM bot_group_messages
             WHERE group_id = ?1 AND COALESCE(thread_id, '') = ?2",
            params![group.id, room_key],
            |r| r.get(0),
        )?;
        let next_seq = (last_seq as u64).saturating_add(1);

        let message_id = format!("gmsg-{}", Uuid::new_v4());
        let mut remote_asks = Vec::with_capacity(targets.remote_targets.len());
        for rm in &targets.remote_targets {
            remote_asks.push(RemoteAsk {
                id: format!("rask-{}", Uuid::new_v4()),
                host_id: rm.host_id.clone(),
                bot_id: rm.bot_id.clone(),
                chat_session_id: rm.chat_session_id.clone(),
                state: RemoteAskState::Sending,
                reply: None,
                created_at_ms: now,
                updated_at_ms: None,
            });
        }

        let requester = req.requester.or_else(|| match &req.sender {
            GroupMessageSender::Owner => Some(GroupMessageRequester::Owner),
            GroupMessageSender::Bot { bot_id } => Some(GroupMessageRequester::Bot {
                bot_id: bot_id.clone(),
                session_id: Self::room_session_for_bot(&group, req.thread_id.as_deref(), bot_id)
                    .ok(),
            }),
            GroupMessageSender::RemoteBot { .. } | GroupMessageSender::System => None,
        });

        let mut mailed = Vec::new();
        for bid in &targets.dispatched_bot_ids {
            mailed.push(bid.clone());
        }
        for rm in &targets.remote_targets {
            mailed.push(rm.member_key());
        }

        let requester_json = requester
            .as_ref()
            .map(serde_json::to_string)
            .transpose()?;
        let answered_by: Vec<String> = Vec::new();
        let answered_by_json = serde_json::to_string(&answered_by)?;
        let mailed_json = serde_json::to_string(&mailed)?;
        let peer_scope_json = req
            .peer_scope
            .as_ref()
            .map(serde_json::to_string)
            .transpose()?;
        let synthetic_root_str = req.synthetic_root.map(synthetic_root_to_str);
        let payload_json = serialize_message_payload(&req.send.images, &remote_asks)?;

        conn.execute(
            "INSERT INTO bot_group_messages (
                id, group_id, thread_id, sequence, sender_kind, sender_bot_id, text,
                requester_json, answered_by_json, mailed_json, handed_off,
                peer_scope_json, synthetic_root, payload_json, created_at_ms
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15)",
            params![
                message_id,
                group.id,
                req.thread_id,
                next_seq as i64,
                req.sender.kind_str(),
                req.sender.bot_id(),
                req.send.text,
                requester_json,
                answered_by_json,
                mailed_json,
                if targets.handed_off { 1_i64 } else { 0_i64 },
                peer_scope_json,
                synthetic_root_str,
                payload_json,
                now
            ],
        )?;

        let op_kind = match req.synthetic_root {
            Some(BotGroupSyntheticRoot::FromItems) => "synthetic_item",
            Some(BotGroupSyntheticRoot::FromReports) => "synthetic_report",
            None if req.peer_scope.is_some() => "peer_request",
            None if targets.handed_off => "handoff",
            None => "reply",
        };

        let mut operation_ids = Vec::with_capacity(targets.dispatched_bot_ids.len());
        for bid in &targets.dispatched_bot_ids {
            let sid = Self::room_session_for_bot(&group, req.thread_id.as_deref(), bid)?;
            let op_id = format!("gop-{}", Uuid::new_v4());
            conn.execute(
                "INSERT INTO bot_group_operations (
                    id, group_id, thread_id, message_id, bot_id, session_id,
                    kind, state, peer_scope_json, synthetic_root, started_at_ms, finished_at_ms
                 ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, 'running', ?8, ?9, ?10, NULL)",
                params![
                    op_id,
                    group.id,
                    req.thread_id,
                    message_id,
                    bid,
                    sid,
                    op_kind,
                    peer_scope_json,
                    synthetic_root_str,
                    now
                ],
            )?;
            operation_ids.push(op_id);
        }

        let mut pending_ids = Vec::with_capacity(targets.held_bot_ids.len());
        for bid in &targets.held_bot_ids {
            let sid = Self::room_session_for_bot(&group, req.thread_id.as_deref(), bid)?;
            let pending_id = format!("gpend-{}", Uuid::new_v4());
            let pending_payload = serde_json::json!({
                "text": req.send.text,
                "messageId": message_id,
                "opKind": op_kind,
            })
            .to_string();
            conn.execute(
                "INSERT INTO bot_group_pending (
                    id, group_id, thread_id, message_id, bot_id, session_id,
                    reason, payload_json, created_at_ms, delivered_at_ms
                 ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, 'paused', ?7, ?8, NULL)",
                params![
                    pending_id,
                    group.id,
                    req.thread_id,
                    message_id,
                    bid,
                    sid,
                    pending_payload,
                    now
                ],
            )?;
            pending_ids.push(pending_id);
        }

        for ask in &remote_asks {
            conn.execute(
                "INSERT INTO bot_group_remote_asks (
                    id, group_id, thread_id, message_id, host_id, bot_id,
                    chat_session_id, state, reply_json, created_at_ms, updated_at_ms
                 ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, NULL, ?9, NULL)",
                params![
                    ask.id,
                    group.id,
                    req.thread_id,
                    message_id,
                    ask.host_id,
                    ask.bot_id,
                    ask.chat_session_id,
                    ask.state.as_str(),
                    ask.created_at_ms
                ],
            )?;
        }

        conn.execute(
            "UPDATE bot_group_sidebar
             SET unread_count = unread_count + 1,
                 last_message_at_ms = ?1,
                 updated_at_ms = ?1
             WHERE group_id = ?2",
            params![now, group.id],
        )?;

        let message = GroupMessageRecord {
            id: message_id,
            group_id: group.id,
            thread_id: req.thread_id,
            sequence: next_seq,
            sender_kind: req.sender.kind_str().to_string(),
            sender_bot_id: req.sender.bot_id().map(ToOwned::to_owned),
            text: req.send.text,
            requester,
            answered_by,
            mailed,
            handed_off: targets.handed_off,
            peer_scope: req.peer_scope,
            synthetic_root: req.synthetic_root,
            images: req.send.images,
            remote_asks: remote_asks.clone(),
            created_at_ms: now,
        };

        Ok(PostedGroupMessage {
            message,
            targets,
            operation_ids,
            pending_ids,
            remote_asks,
        })
    }

    /// Posts a synthetic `fromItems` request root (external item answer landing in a group).
    #[allow(clippy::too_many_arguments)]
    pub fn post_from_items(
        &self,
        group_id: impl Into<String>,
        thread_id: Option<String>,
        item_uid: impl Into<String>,
        item_number: u64,
        text: impl Into<String>,
        active_local_candidates: Vec<MentionCandidate>,
        paused_bot_ids: HashSet<String>,
    ) -> Result<PostedGroupMessage> {
        self.post_message(PostGroupMessageRequest {
            group_id: group_id.into(),
            thread_id,
            sender: GroupMessageSender::System,
            send: BotGroupSend {
                text: text.into(),
                images: Vec::new(),
            },
            active_local_candidates,
            paused_bot_ids,
            requester: Some(GroupMessageRequester::FromItems {
                item_uid: item_uid.into(),
                item_number,
            }),
            peer_scope: None,
            synthetic_root: Some(BotGroupSyntheticRoot::FromItems),
        })
    }

    /// Posts a synthetic `fromReports` request root (e.g., worker report landing after Stop).
    #[allow(clippy::too_many_arguments)]
    pub fn post_from_reports(
        &self,
        group_id: impl Into<String>,
        thread_id: Option<String>,
        worker_dispatch_id: impl Into<String>,
        parent_bot_id: impl Into<String>,
        text: impl Into<String>,
        active_local_candidates: Vec<MentionCandidate>,
        paused_bot_ids: HashSet<String>,
    ) -> Result<PostedGroupMessage> {
        self.post_message(PostGroupMessageRequest {
            group_id: group_id.into(),
            thread_id,
            sender: GroupMessageSender::System,
            send: BotGroupSend {
                text: text.into(),
                images: Vec::new(),
            },
            active_local_candidates,
            paused_bot_ids,
            requester: Some(GroupMessageRequester::FromReports {
                worker_dispatch_id: worker_dispatch_id.into(),
                parent_bot_id: parent_bot_id.into(),
            }),
            peer_scope: None,
            synthetic_root: Some(BotGroupSyntheticRoot::FromReports),
        })
    }

    fn hydrate_message_row(row: &rusqlite::Row<'_>) -> Result<GroupMessageRecord> {
        let requester_raw: Option<String> = row.get(7)?;
        let answered_raw: String = row.get(8)?;
        let mailed_raw: String = row.get(9)?;
        let handed_off_int: i64 = row.get(10)?;
        let peer_scope_raw: Option<String> = row.get(11)?;
        let synthetic_raw: Option<String> = row.get(12)?;
        let payload_raw: String = row.get(13)?;

        let payload: GroupMessagePayload = serde_json::from_str(&payload_raw)?;
        let requester = requester_raw
            .as_deref()
            .map(serde_json::from_str)
            .transpose()?;
        let answered_by: Vec<String> = serde_json::from_str(&answered_raw)?;
        let mailed: Vec<String> = serde_json::from_str(&mailed_raw)?;
        let peer_scope = peer_scope_raw
            .as_deref()
            .map(serde_json::from_str)
            .transpose()?;
        let synthetic_root = synthetic_raw.as_deref().and_then(synthetic_root_from_str);

        Ok(GroupMessageRecord {
            id: row.get(0)?,
            group_id: row.get(1)?,
            thread_id: row.get(2)?,
            sequence: row.get::<_, i64>(3)? as u64,
            sender_kind: row.get(4)?,
            sender_bot_id: row.get(5)?,
            text: row.get(6)?,
            requester,
            answered_by,
            mailed,
            handed_off: handed_off_int != 0,
            peer_scope,
            synthetic_root,
            images: payload.images,
            remote_asks: payload.remote_asks,
            created_at_ms: row.get(14)?,
        })
    }

    /// Loads a single group message by ID.
    pub fn get_message(&self, message_id: &str) -> Result<Option<GroupMessageRecord>> {
        let conn = self.lock_conn()?;
        let mut stmt = conn.prepare(
            "SELECT id, group_id, thread_id, sequence, sender_kind, sender_bot_id, text,
                    requester_json, answered_by_json, mailed_json, handed_off,
                    peer_scope_json, synthetic_root, payload_json, created_at_ms
             FROM bot_group_messages WHERE id = ?1",
        )?;
        let mut rows = stmt.query(params![message_id])?;
        if let Some(row) = rows.next()? {
            Ok(Some(Self::hydrate_message_row(row)?))
        } else {
            Ok(None)
        }
    }

    /// Lists messages in a group's root room (`thread_id = None`) or sub-thread in sequence order.
    pub fn list_messages(
        &self,
        group_id: &str,
        thread_id: Option<&str>,
    ) -> Result<Vec<GroupMessageRecord>> {
        let conn = self.lock_conn()?;
        let room_key = thread_id.unwrap_or("");
        let mut stmt = conn.prepare(
            "SELECT id, group_id, thread_id, sequence, sender_kind, sender_bot_id, text,
                    requester_json, answered_by_json, mailed_json, handed_off,
                    peer_scope_json, synthetic_root, payload_json, created_at_ms
             FROM bot_group_messages
             WHERE group_id = ?1 AND COALESCE(thread_id, '') = ?2
             ORDER BY sequence ASC",
        )?;
        let mut rows = stmt.query(params![group_id, room_key])?;
        let mut out = Vec::new();
        while let Some(row) = rows.next()? {
            out.push(Self::hydrate_message_row(row)?);
        }
        Ok(out)
    }

    /// Records that `bot_id` answered `message_id` and marks its running operation completed.
    pub fn record_message_answer(&self, message_id: &str, bot_id: &str) -> Result<GroupMessageRecord> {
        let now = now_epoch_ms();
        let conn = self.lock_conn()?;
        let current_raw: Option<String> = conn
            .query_row(
                "SELECT answered_by_json FROM bot_group_messages WHERE id = ?1",
                params![message_id],
                |r| r.get(0),
            )
            .optional()?;
        let Some(answered_raw) = current_raw else {
            return Err(OrcaBotsError::bot(
                BotErrorCode::NotFound,
                format!("Group message '{message_id}' was not found."),
            ));
        };
        let mut answered_by: Vec<String> = serde_json::from_str(&answered_raw)?;
        if !answered_by.iter().any(|b| b == bot_id) {
            answered_by.push(bot_id.to_string());
        }
        let updated_json = serde_json::to_string(&answered_by)?;
        conn.execute(
            "UPDATE bot_group_messages SET answered_by_json = ?1 WHERE id = ?2",
            params![updated_json, message_id],
        )?;
        conn.execute(
            "UPDATE bot_group_operations
             SET state = 'completed', finished_at_ms = ?1
             WHERE message_id = ?2 AND bot_id = ?3 AND state = 'running'",
            params![now, message_id, bot_id],
        )?;
        drop(conn);
        self.get_message(message_id)?.ok_or_else(|| {
            OrcaBotsError::bot(
                BotErrorCode::NotFound,
                format!("Group message '{message_id}' disappeared."),
            )
        })
    }

    /// Stops all currently running group operations & worker operations in a room/thread.
    pub fn stop_group_operations(&self, group_id: &str, thread_id: Option<&str>) -> Result<usize> {
        let now = now_epoch_ms();
        let conn = self.lock_conn()?;
        let room_key = thread_id.unwrap_or("");
        let ops_stopped = conn.execute(
            "UPDATE bot_group_operations
             SET state = 'stopped', finished_at_ms = ?1
             WHERE group_id = ?2 AND COALESCE(thread_id, '') = ?3 AND state = 'running'",
            params![now, group_id, room_key],
        )?;
        let workers_stopped = conn.execute(
            "UPDATE bot_group_worker_operations
             SET state = 'stopped', updated_at_ms = ?1
             WHERE group_id = ?2 AND COALESCE(thread_id, '') = ?3 AND state = 'running'",
            params![now, group_id, room_key],
        )?;
        Ok(ops_stopped + workers_stopped)
    }

    /// Lists operations recorded in `bot_group_operations` for `group_id`.
    pub fn list_operations(&self, group_id: &str) -> Result<Vec<GroupOperationRecord>> {
        let conn = self.lock_conn()?;
        let mut stmt = conn.prepare(
            "SELECT id, group_id, thread_id, message_id, bot_id, session_id, kind, state,
                    peer_scope_json, synthetic_root, started_at_ms, finished_at_ms
             FROM bot_group_operations
             WHERE group_id = ?1
             ORDER BY started_at_ms ASC",
        )?;
        let mut rows = stmt.query(params![group_id])?;
        let mut out = Vec::new();
        while let Some(row) = rows.next()? {
            let scope_raw: Option<String> = row.get(8)?;
            let synth_raw: Option<String> = row.get(9)?;
            out.push(GroupOperationRecord {
                id: row.get(0)?,
                group_id: row.get(1)?,
                thread_id: row.get(2)?,
                message_id: row.get(3)?,
                bot_id: row.get(4)?,
                session_id: row.get(5)?,
                kind: row.get(6)?,
                state: row.get(7)?,
                peer_scope: scope_raw.as_deref().map(serde_json::from_str).transpose()?,
                synthetic_root: synth_raw.as_deref().and_then(synthetic_root_from_str),
                started_at_ms: row.get(10)?,
                finished_at_ms: row.get(11)?,
            });
        }
        Ok(out)
    }

    /// Lists all undelivered mail rows queued in `bot_group_pending` for `bot_id`.
    pub fn list_pending_for_bot(&self, bot_id: &str) -> Result<Vec<GroupPendingMail>> {
        let conn = self.lock_conn()?;
        let mut stmt = conn.prepare(
            "SELECT id, group_id, thread_id, message_id, bot_id, session_id, reason,
                    payload_json, created_at_ms, delivered_at_ms
             FROM bot_group_pending
             WHERE bot_id = ?1 AND delivered_at_ms IS NULL
             ORDER BY created_at_ms ASC",
        )?;
        let mut rows = stmt.query(params![bot_id])?;
        let mut out = Vec::new();
        while let Some(row) = rows.next()? {
            let payload_raw: String = row.get(7)?;
            let payload: JsonValue = serde_json::from_str(&payload_raw)?;
            let text = payload
                .get("text")
                .and_then(JsonValue::as_str)
                .unwrap_or_default()
                .to_string();
            out.push(GroupPendingMail {
                id: row.get(0)?,
                group_id: row.get(1)?,
                thread_id: row.get(2)?,
                message_id: row.get(3)?,
                bot_id: row.get(4)?,
                session_id: row.get(5)?,
                reason: row.get(6)?,
                text,
                created_at_ms: row.get(8)?,
                delivered_at_ms: row.get(9)?,
            });
        }
        Ok(out)
    }

    /// Drains all held mail in `bot_group_pending` for `bot_id` once unpaused, marking
    /// each row delivered, adding `bot_id` to parent message's `mailed`, and starting
    /// corresponding `bot_group_operations`.
    pub fn flush_pending_for_bot(&self, bot_id: &str) -> Result<Vec<GroupPendingMail>> {
        let now = now_epoch_ms();
        let conn = self.lock_conn()?;
        let mut stmt = conn.prepare(
            "SELECT id, group_id, thread_id, message_id, bot_id, session_id, reason,
                    payload_json, created_at_ms
             FROM bot_group_pending
             WHERE bot_id = ?1 AND delivered_at_ms IS NULL
             ORDER BY created_at_ms ASC",
        )?;
        let mut rows = stmt.query(params![bot_id])?;
        let mut drained = Vec::new();
        while let Some(row) = rows.next()? {
            let payload_raw: String = row.get(7)?;
            let payload: JsonValue = serde_json::from_str(&payload_raw)?;
            let text = payload
                .get("text")
                .and_then(JsonValue::as_str)
                .unwrap_or_default()
                .to_string();
            drained.push(GroupPendingMail {
                id: row.get(0)?,
                group_id: row.get(1)?,
                thread_id: row.get(2)?,
                message_id: row.get(3)?,
                bot_id: row.get(4)?,
                session_id: row.get(5)?,
                reason: row.get(6)?,
                text,
                created_at_ms: row.get(8)?,
                delivered_at_ms: Some(now),
            });
        }
        drop(rows);
        drop(stmt);

        for item in &drained {
            conn.execute(
                "UPDATE bot_group_pending SET delivered_at_ms = ?1 WHERE id = ?2",
                params![now, item.id],
            )?;
            let op_id = format!("gop-{}", Uuid::new_v4());
            conn.execute(
                "INSERT INTO bot_group_operations (
                    id, group_id, thread_id, message_id, bot_id, session_id,
                    kind, state, peer_scope_json, synthetic_root, started_at_ms, finished_at_ms
                 ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, 'pending_flush', 'running', NULL, NULL, ?7, NULL)",
                params![
                    op_id,
                    item.group_id,
                    item.thread_id,
                    item.message_id,
                    item.bot_id,
                    item.session_id,
                    now
                ],
            )?;
            if let Some(mailed_raw) = conn
                .query_row(
                    "SELECT mailed_json FROM bot_group_messages WHERE id = ?1",
                    params![item.message_id],
                    |r| r.get::<_, String>(0),
                )
                .optional()?
            {
                let mut mailed: Vec<String> = serde_json::from_str(&mailed_raw)?;
                if !mailed.iter().any(|m| m == &item.bot_id) {
                    mailed.push(item.bot_id.clone());
                    let updated = serde_json::to_string(&mailed)?;
                    conn.execute(
                        "UPDATE bot_group_messages SET mailed_json = ?1 WHERE id = ?2",
                        params![updated, item.message_id],
                    )?;
                }
            }
        }
        Ok(drained)
    }

    /// Registers a child worker operation spawned by a member bot in a group room.
    pub fn register_worker_operation(
        &self,
        group_id: &str,
        thread_id: Option<&str>,
        parent_bot_id: &str,
        worker_dispatch_id: &str,
    ) -> Result<GroupWorkerOperation> {
        let now = now_epoch_ms();
        let conn = self.lock_conn()?;
        let group = Self::require_group_locked(&conn, group_id)?;
        let parent_session_id = Self::room_session_for_bot(&group, thread_id, parent_bot_id)?;
        let id = format!("gwop-{}", Uuid::new_v4());
        conn.execute(
            "INSERT INTO bot_group_worker_operations (
                id, group_id, thread_id, parent_bot_id, parent_session_id,
                worker_dispatch_id, state, report_subject, report_body, created_at_ms, updated_at_ms
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, 'running', NULL, NULL, ?7, ?7)",
            params![
                id,
                group_id,
                thread_id,
                parent_bot_id,
                parent_session_id,
                worker_dispatch_id,
                now
            ],
        )?;
        Ok(GroupWorkerOperation {
            id,
            group_id: group_id.to_string(),
            thread_id: thread_id.map(ToOwned::to_owned),
            parent_bot_id: parent_bot_id.to_string(),
            parent_session_id,
            worker_dispatch_id: worker_dispatch_id.to_string(),
            state: "running".to_string(),
            report_subject: None,
            report_body: None,
            created_at_ms: now,
            updated_at_ms: now,
        })
    }

    /// Records a worker report and, if the worker operation had already been stopped,
    /// emits a synthetic `fromReports` message into the group room.
    pub fn complete_worker_report(
        &self,
        worker_op_id: &str,
        subject: &str,
        body: &str,
        active_local_candidates: Vec<MentionCandidate>,
        paused_bot_ids: HashSet<String>,
    ) -> Result<(GroupWorkerOperation, Option<PostedGroupMessage>)> {
        let now = now_epoch_ms();
        let (mut op, was_stopped) = {
            let conn = self.lock_conn()?;
            let mut stmt = conn.prepare(
                "SELECT id, group_id, thread_id, parent_bot_id, parent_session_id,
                        worker_dispatch_id, state, report_subject, report_body,
                        created_at_ms, updated_at_ms
                 FROM bot_group_worker_operations WHERE id = ?1",
            )?;
            let mut rows = stmt.query(params![worker_op_id])?;
            let Some(row) = rows.next()? else {
                return Err(OrcaBotsError::bot(
                    BotErrorCode::NotFound,
                    format!("Worker operation '{worker_op_id}' was not found."),
                ));
            };
            let op = GroupWorkerOperation {
                id: row.get(0)?,
                group_id: row.get(1)?,
                thread_id: row.get(2)?,
                parent_bot_id: row.get(3)?,
                parent_session_id: row.get(4)?,
                worker_dispatch_id: row.get(5)?,
                state: row.get(6)?,
                report_subject: row.get(7)?,
                report_body: row.get(8)?,
                created_at_ms: row.get(9)?,
                updated_at_ms: row.get(10)?,
            };
            let was_stopped = op.state == "stopped";
            conn.execute(
                "UPDATE bot_group_worker_operations
                 SET state = 'completed', report_subject = ?1, report_body = ?2, updated_at_ms = ?3
                 WHERE id = ?4",
                params![subject, body, now, worker_op_id],
            )?;
            (op, was_stopped)
        };
        op.state = "completed".to_string();
        op.report_subject = Some(subject.to_string());
        op.report_body = Some(body.to_string());
        op.updated_at_ms = now;

        let synthetic_msg = if was_stopped {
            let text = if subject.trim().is_empty() {
                body.to_string()
            } else {
                format!("{subject}\n\n{body}")
            };
            Some(self.post_from_reports(
                op.group_id.clone(),
                op.thread_id.clone(),
                op.worker_dispatch_id.clone(),
                op.parent_bot_id.clone(),
                text,
                active_local_candidates,
                paused_bot_ids,
            )?)
        } else {
            None
        };
        Ok((op, synthetic_msg))
    }

    /// Lists worker operations recorded in `bot_group_worker_operations` for `group_id`.
    pub fn list_worker_operations(&self, group_id: &str) -> Result<Vec<GroupWorkerOperation>> {
        let conn = self.lock_conn()?;
        let mut stmt = conn.prepare(
            "SELECT id, group_id, thread_id, parent_bot_id, parent_session_id,
                    worker_dispatch_id, state, report_subject, report_body,
                    created_at_ms, updated_at_ms
             FROM bot_group_worker_operations
             WHERE group_id = ?1
             ORDER BY created_at_ms ASC",
        )?;
        let mut rows = stmt.query(params![group_id])?;
        let mut out = Vec::new();
        while let Some(row) = rows.next()? {
            out.push(GroupWorkerOperation {
                id: row.get(0)?,
                group_id: row.get(1)?,
                thread_id: row.get(2)?,
                parent_bot_id: row.get(3)?,
                parent_session_id: row.get(4)?,
                worker_dispatch_id: row.get(5)?,
                state: row.get(6)?,
                report_subject: row.get(7)?,
                report_body: row.get(8)?,
                created_at_ms: row.get(9)?,
                updated_at_ms: row.get(10)?,
            });
        }
        Ok(out)
    }

    fn sync_message_remote_asks_locked(conn: &Connection, message_id: &str) -> Result<()> {
        let payload_raw: Option<String> = conn
            .query_row(
                "SELECT payload_json FROM bot_group_messages WHERE id = ?1",
                params![message_id],
                |r| r.get(0),
            )
            .optional()?;
        let Some(raw) = payload_raw else {
            return Ok(());
        };
        let mut payload: GroupMessagePayload = serde_json::from_str(&raw)?;
        let mut stmt = conn.prepare(
            "SELECT id, host_id, bot_id, chat_session_id, state, reply_json, created_at_ms, updated_at_ms
             FROM bot_group_remote_asks
             WHERE message_id = ?1
             ORDER BY created_at_ms ASC",
        )?;
        let mut rows = stmt.query(params![message_id])?;
        let mut asks = Vec::new();
        while let Some(row) = rows.next()? {
            let state_raw: String = row.get(4)?;
            let reply_raw: Option<String> = row.get(5)?;
            asks.push(RemoteAsk {
                id: row.get(0)?,
                host_id: row.get(1)?,
                bot_id: row.get(2)?,
                chat_session_id: row.get(3)?,
                state: RemoteAskState::parse(&state_raw).unwrap_or(RemoteAskState::Unknown),
                reply: reply_raw.as_deref().map(serde_json::from_str).transpose()?,
                created_at_ms: row.get(6)?,
                updated_at_ms: row.get(7)?,
            });
        }
        drop(rows);
        drop(stmt);
        payload.remote_asks = asks;
        let updated = serialize_message_payload(&payload.images, &payload.remote_asks)?;
        conn.execute(
            "UPDATE bot_group_messages SET payload_json = ?1 WHERE id = ?2",
            params![updated, message_id],
        )?;
        Ok(())
    }

    fn load_remote_ask_locked(conn: &Connection, ask_id: &str) -> Result<Option<(String, RemoteAsk)>> {
        let mut stmt = conn.prepare(
            "SELECT message_id, id, host_id, bot_id, chat_session_id, state, reply_json,
                    created_at_ms, updated_at_ms
             FROM bot_group_remote_asks WHERE id = ?1",
        )?;
        let mut rows = stmt.query(params![ask_id])?;
        if let Some(row) = rows.next()? {
            let msg_id: String = row.get(0)?;
            let state_raw: String = row.get(5)?;
            let reply_raw: Option<String> = row.get(6)?;
            let ask = RemoteAsk {
                id: row.get(1)?,
                host_id: row.get(2)?,
                bot_id: row.get(3)?,
                chat_session_id: row.get(4)?,
                state: RemoteAskState::parse(&state_raw).unwrap_or(RemoteAskState::Unknown),
                reply: reply_raw.as_deref().map(serde_json::from_str).transpose()?,
                created_at_ms: row.get(7)?,
                updated_at_ms: row.get(8)?,
            };
            Ok(Some((msg_id, ask)))
        } else {
            Ok(None)
        }
    }

    /// Updates a cross-machine remote ask delivery state (`sending | sent | failed | unknown`).
    pub fn update_remote_ask_state(&self, ask_id: &str, state: RemoteAskState) -> Result<RemoteAsk> {
        let now = now_epoch_ms();
        let conn = self.lock_conn()?;
        let Some((msg_id, mut ask)) = Self::load_remote_ask_locked(&conn, ask_id)? else {
            return Err(OrcaBotsError::bot(
                BotErrorCode::NotFound,
                format!("Remote ask '{ask_id}' was not found."),
            ));
        };
        ask.state = state;
        ask.updated_at_ms = Some(now);
        conn.execute(
            "UPDATE bot_group_remote_asks SET state = ?1, updated_at_ms = ?2 WHERE id = ?3",
            params![ask.state.as_str(), now, ask_id],
        )?;
        Self::sync_message_remote_asks_locked(&conn, &msg_id)?;
        Ok(ask)
    }

    /// Applies an `Answer { text, truncated }` or `Notice { text }` reply to a remote ask,
    /// enforcing that a `Notice` can never overwrite an existing `Answer`.
    pub fn apply_remote_ask_reply(&self, ask_id: &str, reply: RemoteAskReply) -> Result<RemoteAsk> {
        let now = now_epoch_ms();
        let conn = self.lock_conn()?;
        let Some((msg_id, mut ask)) = Self::load_remote_ask_locked(&conn, ask_id)? else {
            return Err(OrcaBotsError::bot(
                BotErrorCode::NotFound,
                format!("Remote ask '{ask_id}' was not found."),
            ));
        };
        ask.apply_reply(reply, now)?;
        ask.state = RemoteAskState::Sent;
        let reply_json = ask.reply.as_ref().map(serde_json::to_string).transpose()?;
        conn.execute(
            "UPDATE bot_group_remote_asks
             SET state = ?1, reply_json = ?2, updated_at_ms = ?3
             WHERE id = ?4",
            params![ask.state.as_str(), reply_json, now, ask_id],
        )?;
        Self::sync_message_remote_asks_locked(&conn, &msg_id)?;
        Ok(ask)
    }

    /// Loads a remote ask by ID.
    pub fn get_remote_ask(&self, ask_id: &str) -> Result<Option<RemoteAsk>> {
        let conn = self.lock_conn()?;
        Ok(Self::load_remote_ask_locked(&conn, ask_id)?.map(|(_, ask)| ask))
    }

    /// Lists all remote asks associated with `group_id`.
    pub fn list_remote_asks(&self, group_id: &str) -> Result<Vec<RemoteAsk>> {
        let conn = self.lock_conn()?;
        let mut stmt = conn.prepare(
            "SELECT id, host_id, bot_id, chat_session_id, state, reply_json,
                    created_at_ms, updated_at_ms
             FROM bot_group_remote_asks
             WHERE group_id = ?1
             ORDER BY created_at_ms ASC",
        )?;
        let mut rows = stmt.query(params![group_id])?;
        let mut out = Vec::new();
        while let Some(row) = rows.next()? {
            let state_raw: String = row.get(4)?;
            let reply_raw: Option<String> = row.get(5)?;
            out.push(RemoteAsk {
                id: row.get(0)?,
                host_id: row.get(1)?,
                bot_id: row.get(2)?,
                chat_session_id: row.get(3)?,
                state: RemoteAskState::parse(&state_raw).unwrap_or(RemoteAskState::Unknown),
                reply: reply_raw.as_deref().map(serde_json::from_str).transpose()?,
                created_at_ms: row.get(6)?,
                updated_at_ms: row.get(7)?,
            });
        }
        Ok(out)
    }

    /// Loads sidebar metadata from `bot_group_sidebar`.
    pub fn get_sidebar(&self, group_id: &str) -> Result<Option<GroupSidebarEntry>> {
        let conn = self.lock_conn()?;
        let row = conn
            .query_row(
                "SELECT group_id, sidebar_order, pinned, unread_count, last_read_sequence,
                        last_message_at_ms, updated_at_ms
                 FROM bot_group_sidebar WHERE group_id = ?1",
                params![group_id],
                |r| {
                    Ok(GroupSidebarEntry {
                        group_id: r.get(0)?,
                        sidebar_order: r.get(1)?,
                        pinned: r.get::<_, i64>(2)? != 0,
                        unread_count: r.get::<_, i64>(3)? as u64,
                        last_read_sequence: r.get::<_, i64>(4)? as u64,
                        last_message_at_ms: r.get(5)?,
                        updated_at_ms: r.get(6)?,
                    })
                },
            )
            .optional()?;
        Ok(row)
    }

    /// Marks a group as read up to `last_read_sequence`, resetting `unread_count` to `0`.
    pub fn mark_group_read(&self, group_id: &str, last_read_sequence: u64) -> Result<GroupSidebarEntry> {
        let now = now_epoch_ms();
        let conn = self.lock_conn()?;
        let changed = conn.execute(
            "UPDATE bot_group_sidebar
             SET unread_count = 0, last_read_sequence = ?1, updated_at_ms = ?2
             WHERE group_id = ?3",
            params![last_read_sequence as i64, now, group_id],
        )?;
        drop(conn);
        if changed == 0 {
            return Err(OrcaBotsError::bot(
                BotErrorCode::NotFound,
                format!("Group sidebar entry '{group_id}' was not found."),
            ));
        }
        self.get_sidebar(group_id)?.ok_or_else(|| {
            OrcaBotsError::bot(
                BotErrorCode::NotFound,
                format!("Group sidebar entry '{group_id}' disappeared."),
            )
        })
    }

    /// Sets sidebar ordering and pinned flag for `group_id`.
    pub fn set_sidebar_order_and_pin(
        &self,
        group_id: &str,
        sidebar_order: i64,
        pinned: bool,
    ) -> Result<GroupSidebarEntry> {
        let now = now_epoch_ms();
        let conn = self.lock_conn()?;
        let changed = conn.execute(
            "UPDATE bot_group_sidebar
             SET sidebar_order = ?1, pinned = ?2, updated_at_ms = ?3
             WHERE group_id = ?4",
            params![sidebar_order, if pinned { 1_i64 } else { 0_i64 }, now, group_id],
        )?;
        drop(conn);
        if changed == 0 {
            return Err(OrcaBotsError::bot(
                BotErrorCode::NotFound,
                format!("Group sidebar entry '{group_id}' was not found."),
            ));
        }
        self.get_sidebar(group_id)?.ok_or_else(|| {
            OrcaBotsError::bot(
                BotErrorCode::NotFound,
                format!("Group sidebar entry '{group_id}' disappeared."),
            )
        })
    }

    /// Evaluates inter-bot / worker-to-bot request policy via `orca_core::bot_groups`,
    /// covering all 7 refusal codes (`bots_disabled`, `bot_not_found` with `--on <computer>`,
    /// `bot_chat_not_found`, `bot_paused`, `bot_self` with `--chat new`, `bot_request_type`,
    /// `bot_request_worker`).
    pub fn evaluate_request_policy(
        &self,
        ctx: &BotRequestPolicyContext,
        input: &BotRequestPolicyInput,
    ) -> Result<ApprovedBotRequest> {
        evaluate_bot_request_policy(ctx, input).map_err(OrcaBotsError::Bot)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use orca_core::bot_groups::{
        BOT_GROUP_MAX_REMOTE_MEMBERS, BotRequestCaller, BotRequestChatTarget, LocalBotPolicyEntry,
        PairedBotEntry, PairedComputerRoster,
    };
    use tempfile::TempDir;

    fn sample_candidates() -> Vec<MentionCandidate> {
        vec![
            MentionCandidate {
                id: "bot-alpha".into(),
                name: "Alpha".into(),
                is_remote: false,
            },
            MentionCandidate {
                id: "bot-beta".into(),
                name: "Beta".into(),
                is_remote: false,
            },
            MentionCandidate {
                id: "bot-gamma".into(),
                name: "Gamma".into(),
                is_remote: false,
            },
        ]
    }

    #[test]
    fn sqlite_wal_seven_tables_and_rollback_safe_after_update_triggers() {
        let tmp = TempDir::new().unwrap();
        let db_path = tmp.path().join("groups.sqlite3");
        let bots_root = tmp.path().join("bots");
        let rt = BotGroupRuntime::open(&db_path, &bots_root).unwrap();

        // Verify WAL mode and all 7 upstream tables exist in sqlite_master.
        let raw_conn = Connection::open(&db_path).unwrap();
        let journal_mode: String = raw_conn
            .query_row("PRAGMA journal_mode", [], |r| r.get(0))
            .unwrap();
        assert_eq!(journal_mode.to_lowercase(), "wal");

        let mut stmt = raw_conn
            .prepare("SELECT name FROM sqlite_master WHERE type = 'table' ORDER BY name")
            .unwrap();
        let tables: HashSet<String> = stmt
            .query_map([], |r| r.get(0))
            .unwrap()
            .map(|r| r.unwrap())
            .collect();
        for expected in [
            "bot_groups",
            "bot_group_messages",
            "bot_group_operations",
            "bot_group_pending",
            "bot_group_worker_operations",
            "bot_group_sidebar",
            "bot_group_remote_asks",
        ] {
            assert!(tables.contains(expected), "missing table {expected}");
        }

        // Create group with remote member + message with image & remote ask.
        let remote = RemoteBotMember {
            host_id: "mac-studio".into(),
            bot_id: "bot-remote".into(),
            name: "StudioBot".into(),
            chat_session_id: "pinned-sess-42".into(),
        };
        let group = rt
            .create_group(CreateBotGroupRequest {
                name: "Release Room".into(),
                folder: None,
                member_bot_ids: vec!["bot-alpha".into(), "bot-beta".into()],
                reply_mode: BotGroupReplyMode::Mentions,
                remote_members: vec![remote.clone()],
            })
            .unwrap();
        assert!(group.folder.as_str().starts_with("group-"));
        assert!(bots_root.join(group.folder.as_str()).is_dir());

        let posted = rt
            .post_message(PostGroupMessageRequest {
                group_id: group.id.clone(),
                thread_id: None,
                sender: GroupMessageSender::Owner,
                send: BotGroupSend {
                    text: "Please check @StudioBot".into(),
                    images: vec![BotGroupImage {
                        id: "img-1".into(),
                        path: "uploads/screenshot.png".into(),
                        mime: "image/png".into(),
                    }],
                },
                active_local_candidates: sample_candidates(),
                paused_bot_ids: HashSet::new(),
                requester: None,
                peer_scope: None,
                synthetic_root: None,
            })
            .unwrap();
        assert_eq!(posted.remote_asks.len(), 1);
        assert_eq!(posted.remote_asks[0].chat_session_id, "pinned-sess-42");

        // Simulate an OLDER build running raw SQL UPDATE omitting "remoteMembers", "images", "remoteAsks"!
        let legacy_group_payload = serde_json::json!({
            "id": group.id,
            "name": "Renamed By Legacy Build",
            "folder": group.folder.as_str(),
            "memberBotIds": group.member_bot_ids,
            "sessionIds": group.session_ids,
            "formerSessionIds": {},
            "replyMode": { "kind": "mentions" },
            "threads": [],
            "createdAtMs": group.created_at_ms,
            "updatedAtMs": group.updated_at_ms + 100
        })
        .to_string();
        raw_conn
            .execute(
                "UPDATE bot_groups SET name = 'Renamed By Legacy Build', payload_json = ?1 WHERE id = ?2",
                params![legacy_group_payload, group.id],
            )
            .unwrap();

        let legacy_msg_payload = serde_json::json!({ "legacyFlag": true }).to_string();
        raw_conn
            .execute(
                "UPDATE bot_group_messages SET payload_json = ?1 WHERE id = ?2",
                params![legacy_msg_payload, posted.message.id],
            )
            .unwrap();

        // Reload via BotGroupRuntime: triggers MUST have preserved remoteMembers, images, remoteAsks!
        let reloaded_group = rt.get_group(&group.id).unwrap().unwrap();
        assert_eq!(reloaded_group.name, "Renamed By Legacy Build");
        assert_eq!(reloaded_group.remote_members, vec![remote.clone()]);

        let reloaded_msg = rt.get_message(&posted.message.id).unwrap().unwrap();
        assert_eq!(reloaded_msg.images.len(), 1);
        assert_eq!(reloaded_msg.images[0].id, "img-1");
        assert_eq!(reloaded_msg.remote_asks.len(), 1);
        assert_eq!(reloaded_msg.remote_asks[0].chat_session_id, "pinned-sess-42");

        // Intentional removal by current build writes explicit empty "remoteMembers": [] so it clears cleanly.
        let cleared = rt
            .remove_remote_member(&group.id, "mac-studio", "bot-remote")
            .unwrap();
        assert!(cleared.remote_members.is_empty());
    }

    #[test]
    fn threads_former_sessions_reply_modes_handoff_pending_flush_and_synthetic_roots() {
        let tmp = TempDir::new().unwrap();
        let rt = BotGroupRuntime::open_in_memory(tmp.path().join("bots")).unwrap();

        let group = rt
            .create_group(CreateBotGroupRequest {
                name: "Platform Ops".into(),
                folder: None,
                member_bot_ids: vec!["bot-alpha".into(), "bot-beta".into(), "bot-gamma".into()],
                reply_mode: BotGroupReplyMode::Bot {
                    bot_id: "bot-alpha".into(),
                },
                remote_members: Vec::new(),
            })
            .unwrap();

        // Create sub-thread with >80 char post -> truncated cleanly <=80, fresh sessions per member.
        let long_post = "Investigate staging latency spike across regions when background workers flush metrics continuously every minute";
        let thread = rt.create_thread(&group.id, long_post).unwrap();
        assert!(thread.title.chars().count() <= 80);
        assert_eq!(thread.session_ids.len(), 3);
        assert_ne!(
            thread.session_ids.get("bot-alpha"),
            group.session_ids.get("bot-alpha")
        );
        assert_eq!(rt.list_threads(&group.id).unwrap().len(), 1);

        // Remove bot-gamma: its root & thread sessions move to former_session_ids (no active access).
        let gamma_root_sess = group.session_ids.get("bot-gamma").unwrap().clone();
        let gamma_thr_sess = thread.session_ids.get("bot-gamma").unwrap().clone();
        let updated = rt.remove_member(&group.id, "bot-gamma").unwrap();
        assert!(!updated.has_active_session_access(&gamma_root_sess));
        assert!(!updated.has_active_session_access(&gamma_thr_sess));
        assert_eq!(
            rt.session_lookup(&group.id, &gamma_thr_sess).unwrap(),
            Some(GroupSessionLookup::Former {
                thread_id: Some(thread.id.clone()),
                bot_id: "bot-gamma".into(),
            })
        );

        // Mode 1: Bot lead (bot-alpha) receives owner post; then lead hands off via @Beta with PeerScope.
        let m1 = rt
            .post_message(PostGroupMessageRequest {
                group_id: group.id.clone(),
                thread_id: Some(thread.id.clone()),
                sender: GroupMessageSender::Owner,
                send: BotGroupSend {
                    text: "Check deploy logs".into(),
                    images: Vec::new(),
                },
                active_local_candidates: sample_candidates(),
                paused_bot_ids: HashSet::new(),
                requester: None,
                peer_scope: None,
                synthetic_root: None,
            })
            .unwrap();
        assert_eq!(m1.targets.dispatched_bot_ids, vec!["bot-alpha".to_string()]);
        assert!(!m1.message.handed_off);

        let peer_scope = BotGroupPeerScope {
            root_request_id: m1.message.id.clone(),
            root_sequence: m1.message.sequence,
            source_session_id: thread.session_ids["bot-alpha"].clone(),
            target_session_id: thread.session_ids["bot-beta"].clone(),
        };
        let mut paused = HashSet::new();
        paused.insert("bot-beta".to_string());
        let handoff = rt
            .post_message(PostGroupMessageRequest {
                group_id: group.id.clone(),
                thread_id: Some(thread.id.clone()),
                sender: GroupMessageSender::Bot {
                    bot_id: "bot-alpha".into(),
                },
                send: BotGroupSend {
                    text: "@Beta please inspect the worker crash".into(),
                    images: Vec::new(),
                },
                active_local_candidates: sample_candidates(),
                paused_bot_ids: paused,
                requester: None,
                peer_scope: Some(peer_scope.clone()),
                synthetic_root: None,
            })
            .unwrap();
        assert!(handoff.message.handed_off);
        assert!(handoff.targets.dispatched_bot_ids.is_empty());
        assert_eq!(handoff.targets.held_bot_ids, vec!["bot-beta".to_string()]);
        assert_eq!(handoff.pending_ids.len(), 1);
        assert_eq!(handoff.message.peer_scope, Some(peer_scope));

        // Paused bot-beta mail is held in bot_group_pending until unpaused and flushed.
        assert_eq!(rt.list_pending_for_bot("bot-beta").unwrap().len(), 1);
        let flushed = rt.flush_pending_for_bot("bot-beta").unwrap();
        assert_eq!(flushed.len(), 1);
        assert!(flushed[0].delivered_at_ms.is_some());
        assert!(rt.list_pending_for_bot("bot-beta").unwrap().is_empty());
        let answered = rt
            .record_message_answer(&handoff.message.id, "bot-beta")
            .unwrap();
        assert_eq!(answered.answered_by, vec!["bot-beta".to_string()]);
        assert!(answered.mailed.contains(&"bot-beta".to_string()));

        // Mode 2: All
        rt.update_reply_mode(&group.id, BotGroupReplyMode::All)
            .unwrap();
        let all_post = rt
            .post_message(PostGroupMessageRequest {
                group_id: group.id.clone(),
                thread_id: None,
                sender: GroupMessageSender::Owner,
                send: BotGroupSend {
                    text: "Status update?".into(),
                    images: Vec::new(),
                },
                active_local_candidates: sample_candidates(),
                paused_bot_ids: HashSet::new(),
                requester: None,
                peer_scope: None,
                synthetic_root: None,
            })
            .unwrap();
        assert_eq!(
            all_post.targets.dispatched_bot_ids,
            vec!["bot-alpha".to_string(), "bot-beta".to_string()]
        );

        // Synthetic roots: fromItems & worker report landing after Stop (fromReports).
        let from_items = rt
            .post_from_items(
                &group.id,
                None,
                "item-uid-7",
                7,
                "Owner answered item #7",
                sample_candidates(),
                HashSet::new(),
            )
            .unwrap();
        assert_eq!(
            from_items.message.synthetic_root,
            Some(BotGroupSyntheticRoot::FromItems)
        );

        let wop = rt
            .register_worker_operation(&group.id, None, "bot-alpha", "disp-99")
            .unwrap();
        assert!(rt.stop_group_operations(&group.id, None).unwrap() >= 1);
        let (done_wop, report_msg) = rt
            .complete_worker_report(
                &wop.id,
                "Worker finished",
                "Patched retry loop",
                sample_candidates(),
                HashSet::new(),
            )
            .unwrap();
        assert_eq!(done_wop.state, "completed");
        let report_msg = report_msg.expect("stopped worker report emits fromReports root");
        assert_eq!(
            report_msg.message.synthetic_root,
            Some(BotGroupSyntheticRoot::FromReports)
        );
    }

    #[test]
    fn remote_member_pinning_cap_31_remote_asks_and_all_seven_policy_refusals() {
        let tmp = TempDir::new().unwrap();
        let rt = BotGroupRuntime::open_in_memory(tmp.path().join("bots")).unwrap();

        let mut remotes = Vec::new();
        for i in 0..BOT_GROUP_MAX_REMOTE_MEMBERS {
            remotes.push(RemoteBotMember {
                host_id: format!("host-{i}"),
                bot_id: format!("rbot-{i}"),
                name: format!("Remote{i}"),
                chat_session_id: format!("pinned-sess-{i}"),
            });
        }
        let group = rt
            .create_group(CreateBotGroupRequest {
                name: "Remote Mesh".into(),
                folder: None,
                member_bot_ids: vec!["bot-alpha".into()],
                reply_mode: BotGroupReplyMode::Mentions,
                remote_members: remotes,
            })
            .unwrap();

        // 32nd remote member rejected.
        let extra_err = rt
            .add_remote_member(
                &group.id,
                RemoteBotMember {
                    host_id: "host-extra".into(),
                    bot_id: "rbot-extra".into(),
                    name: "Extra".into(),
                    chat_session_id: "pinned-extra".into(),
                },
            )
            .unwrap_err();
        assert_eq!(extra_err.code(), BotErrorCode::InvalidArgument);

        // Rename preserves pinned chat_session_id.
        let renamed = rt
            .rename_remote_member(&group.id, "host-0", "rbot-0", "RenamedZero")
            .unwrap();
        assert_eq!(renamed.remote_members[0].chat_session_id, "pinned-sess-0");
        assert_eq!(renamed.remote_members[0].name, "RenamedZero");

        // Post mentioning @RenamedZero creates RemoteAsk; Notice cannot overwrite Answer.
        let posted = rt
            .post_message(PostGroupMessageRequest {
                group_id: group.id.clone(),
                thread_id: None,
                sender: GroupMessageSender::Owner,
                send: BotGroupSend {
                    text: "Hey @RenamedZero check metrics".into(),
                    images: Vec::new(),
                },
                active_local_candidates: sample_candidates(),
                paused_bot_ids: HashSet::new(),
                requester: None,
                peer_scope: None,
                synthetic_root: None,
            })
            .unwrap();
        let ask_id = &posted.remote_asks[0].id;
        rt.update_remote_ask_state(ask_id, RemoteAskState::Sent)
            .unwrap();
        let answered = rt
            .apply_remote_ask_reply(
                ask_id,
                RemoteAskReply::Answer {
                    text: "All green".into(),
                    truncated: true,
                },
            )
            .unwrap();
        assert!(matches!(
            answered.reply,
            Some(RemoteAskReply::Answer { truncated: true, .. })
        ));
        let overwrite_err = rt
            .apply_remote_ask_reply(
                ask_id,
                RemoteAskReply::Notice {
                    text: "late notice".into(),
                },
            )
            .unwrap_err();
        assert_eq!(overwrite_err.code(), BotErrorCode::InvalidArgument);

        // All 7 inter-bot request policy refusal codes.
        let mut ctx = BotRequestPolicyContext {
            bots_enabled: false,
            local_bots: vec![
                LocalBotPolicyEntry {
                    id: "bot-alpha".into(),
                    name: "Alpha".into(),
                    paused: false,
                    home_chat_session_id: Some("sess-a".into()),
                    known_chat_session_ids: vec!["sess-a-2".into()],
                },
                LocalBotPolicyEntry {
                    id: "bot-paused".into(),
                    name: "PausedBot".into(),
                    paused: true,
                    home_chat_session_id: Some("sess-p".into()),
                    known_chat_session_ids: Vec::new(),
                },
            ],
            paired_rosters: vec![PairedComputerRoster {
                computer_name: "mac-studio".into(),
                host_id: "host-studio".into(),
                bots: vec![PairedBotEntry {
                    id: "bot-remote".into(),
                    name: "StudioBot".into(),
                }],
            }],
        };

        // 1. bots_disabled
        let e1 = rt
            .evaluate_request_policy(
                &ctx,
                &BotRequestPolicyInput {
                    caller: BotRequestCaller::Owner,
                    target_bot: "Alpha".into(),
                    chat_target: BotRequestChatTarget::Default,
                    request_type_raw: "question".into(),
                },
            )
            .unwrap_err();
        assert_eq!(e1.code(), BotErrorCode::BotsDisabled);
        ctx.bots_enabled = true;

        // 2. bot_not_found with paired --on <computer> hint
        let e2 = rt
            .evaluate_request_policy(
                &ctx,
                &BotRequestPolicyInput {
                    caller: BotRequestCaller::Owner,
                    target_bot: "StudioBot".into(),
                    chat_target: BotRequestChatTarget::Default,
                    request_type_raw: "question".into(),
                },
            )
            .unwrap_err();
        assert_eq!(e2.code(), BotErrorCode::BotNotFound);
        assert!(e2.to_string().contains("--on mac-studio"));

        // 3. bot_chat_not_found
        let e3 = rt
            .evaluate_request_policy(
                &ctx,
                &BotRequestPolicyInput {
                    caller: BotRequestCaller::Owner,
                    target_bot: "Alpha".into(),
                    chat_target: BotRequestChatTarget::Session("missing-chat".into()),
                    request_type_raw: "status".into(),
                },
            )
            .unwrap_err();
        assert_eq!(e3.code(), BotErrorCode::BotChatNotFound);

        // 4. bot_self with --chat new hint
        let e4 = rt
            .evaluate_request_policy(
                &ctx,
                &BotRequestPolicyInput {
                    caller: BotRequestCaller::Bot {
                        bot_id: "bot-alpha".into(),
                        session_id: Some("sess-a".into()),
                    },
                    target_bot: "Alpha".into(),
                    chat_target: BotRequestChatTarget::Default,
                    request_type_raw: "question".into(),
                },
            )
            .unwrap_err();
        assert_eq!(e4.code(), BotErrorCode::BotSelf);
        assert!(e4.to_string().contains("--chat new"));

        // 5. bot_paused
        let e5 = rt
            .evaluate_request_policy(
                &ctx,
                &BotRequestPolicyInput {
                    caller: BotRequestCaller::Owner,
                    target_bot: "PausedBot".into(),
                    chat_target: BotRequestChatTarget::Default,
                    request_type_raw: "status".into(),
                },
            )
            .unwrap_err();
        assert_eq!(e5.code(), BotErrorCode::BotPaused);

        // 6. bot_request_type
        let e6 = rt
            .evaluate_request_policy(
                &ctx,
                &BotRequestPolicyInput {
                    caller: BotRequestCaller::Owner,
                    target_bot: "Alpha".into(),
                    chat_target: BotRequestChatTarget::Default,
                    request_type_raw: "unknown_kind".into(),
                },
            )
            .unwrap_err();
        assert_eq!(e6.code(), BotErrorCode::BotRequestType);

        // 7. bot_request_worker (worker cannot handoff)
        let e7 = rt
            .evaluate_request_policy(
                &ctx,
                &BotRequestPolicyInput {
                    caller: BotRequestCaller::Worker {
                        parent_bot_id: Some("bot-alpha".into()),
                        parent_session_id: Some("sess-a".into()),
                        is_group_worker: true,
                    },
                    target_bot: "Alpha".into(),
                    chat_target: BotRequestChatTarget::New,
                    request_type_raw: "handoff".into(),
                },
            )
            .unwrap_err();
        assert_eq!(e7.code(), BotErrorCode::BotRequestWorker);
    }
}
