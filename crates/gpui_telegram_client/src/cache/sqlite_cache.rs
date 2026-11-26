use crate::net::hermes_client::{HermesRichChatDto, HermesRichMessageDto};
use crate::net::http_client::{ChatSummary, HistoricalMessage};
use gpui_telegram_core::error::{Result, TelegramError};
use gpui_telegram_core::types::{ChatId, MessageId};
use gpui_telegram_proto::automation_apps::{AutomationAppDetail, InstalledAutomationApp};
use gpui_telegram_proto::contracts::auth::ContactDto;
use gpui_telegram_proto::contracts::{ChatDetailDto, ChatFolderDto, DraftDto};
use gpui_telegram_proto::creations::{
    CreationKind, CreationReviewStatus, PublicCatalogItemDto, UserCreationDto,
    UserCreationStateDto,
};
use gpui_telegram_proto::message::{MessageItem, ReactionCount};
use parking_lot::Mutex;
use rusqlite::{params, Connection, OptionalExtension, TransactionBehavior};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::fmt::Write as _;
use std::path::{Path, PathBuf};

pub(crate) const META_CREATIONS_LAST_REVISION: &str = "creations_last_revision";

#[derive(Debug, Clone)]
pub struct CachedMediaEntry {
    pub file_id: String,
    pub sha256: String,
    pub mime_type: Option<String>,
    pub size_bytes: u64,
    pub local_path: PathBuf,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct CachedChatHistorySnapshot {
    pub messages: Vec<HistoricalMessage>,
    #[serde(default)]
    pub rich_messages: Vec<HermesRichMessageDto>,
    #[serde(default)]
    pub updated_at_ms: i64,
}

pub(crate) struct SqliteClientCache {
    pub(crate) conn: Mutex<Connection>,
    base_dir: PathBuf,
    media_dir: PathBuf,
}

impl std::fmt::Debug for SqliteClientCache {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SqliteClientCache")
            .field("media_dir", &self.media_dir)
            .finish()
    }
}

pub(crate) fn sha256_hex(bytes: &[u8]) -> String {
    let digest = Sha256::digest(bytes);
    let mut out = String::with_capacity(64);
    for b in digest {
        let _ = write!(&mut out, "{:02x}", b);
    }
    out
}

pub(crate) fn db_err(err: rusqlite::Error) -> TelegramError {
    TelegramError::Database(err.to_string())
}

const CLIENT_SCHEMA_SQL: &str = "
    PRAGMA journal_mode=WAL;
    PRAGMA synchronous=NORMAL;
    CREATE TABLE IF NOT EXISTS messages (
        chat_id INTEGER NOT NULL,
        msg_id INTEGER NOT NULL,
        date_ms INTEGER NOT NULL,
        payload_json TEXT NOT NULL,
        PRIMARY KEY (chat_id, msg_id)
    );
    CREATE INDEX IF NOT EXISTS idx_messages_chat_date ON messages(chat_id, date_ms, msg_id);
    CREATE TABLE IF NOT EXISTS chats (
        chat_id INTEGER PRIMARY KEY,
        payload_json TEXT NOT NULL
    );
    CREATE TABLE IF NOT EXISTS drafts (
        chat_id INTEGER PRIMARY KEY,
        payload_json TEXT NOT NULL
    );
    CREATE TABLE IF NOT EXISTS read_states (
        chat_id INTEGER PRIMARY KEY,
        read_inbox_max INTEGER NOT NULL DEFAULT 0,
        read_outbox_max INTEGER NOT NULL DEFAULT 0
    );
    CREATE TABLE IF NOT EXISTS media_cas (
        file_id TEXT PRIMARY KEY,
        sha256 TEXT NOT NULL,
        mime_type TEXT,
        size_bytes INTEGER NOT NULL,
        local_path TEXT NOT NULL
    );
    CREATE TABLE IF NOT EXISTS message_reactions (
        chat_id INTEGER NOT NULL,
        msg_id INTEGER NOT NULL,
        payload_json TEXT NOT NULL,
        PRIMARY KEY (chat_id, msg_id)
    );
    CREATE TABLE IF NOT EXISTS cache_meta (
        key TEXT PRIMARY KEY,
        value TEXT NOT NULL
    );
    CREATE TABLE IF NOT EXISTS outbox_messages (
        client_nonce INTEGER PRIMARY KEY,
        chat_id INTEGER NOT NULL,
        payload_json TEXT NOT NULL,
        text_preview TEXT NOT NULL DEFAULT '',
        status TEXT NOT NULL,
        retry_count INTEGER NOT NULL DEFAULT 0,
        next_retry_at_ms INTEGER NOT NULL DEFAULT 0,
        last_error TEXT,
        server_message_id INTEGER,
        created_at_ms INTEGER NOT NULL,
        updated_at_ms INTEGER NOT NULL
    );
    CREATE INDEX IF NOT EXISTS idx_outbox_status_retry
        ON outbox_messages(status, next_retry_at_ms, client_nonce);
    CREATE TABLE IF NOT EXISTS file_transfers (
        transfer_id TEXT PRIMARY KEY,
        upload_id TEXT,
        direction TEXT NOT NULL DEFAULT 'upload',
        file_path TEXT NOT NULL,
        filename TEXT NOT NULL,
        mime_type TEXT NOT NULL,
        total_size INTEGER NOT NULL,
        part_size INTEGER NOT NULL,
        total_parts INTEGER NOT NULL,
        file_sha256 TEXT NOT NULL,
        chunk_hashes_json TEXT NOT NULL,
        uploaded_parts_json TEXT NOT NULL,
        status TEXT NOT NULL,
        server_file_id TEXT,
        last_error TEXT,
        created_at_ms INTEGER NOT NULL,
        updated_at_ms INTEGER NOT NULL
    );
    CREATE INDEX IF NOT EXISTS idx_file_transfers_status
        ON file_transfers(status, updated_at_ms);
    CREATE TABLE IF NOT EXISTS folders (
        folder_id INTEGER PRIMARY KEY,
        payload_json TEXT NOT NULL
    );
    CREATE TABLE IF NOT EXISTS chat_ttls (
        chat_id INTEGER PRIMARY KEY,
        ttl_seconds INTEGER NOT NULL
    );
    CREATE TABLE IF NOT EXISTS e2ee_identity (
        id INTEGER PRIMARY KEY CHECK (id = 1),
        x25519_sec BLOB NOT NULL,
        x25519_pub BLOB NOT NULL,
        ed25519_sec BLOB NOT NULL,
        ed25519_pub BLOB NOT NULL,
        next_spk_id INTEGER NOT NULL,
        next_otpk_id INTEGER NOT NULL,
        created_at_ms INTEGER NOT NULL
    );
    CREATE TABLE IF NOT EXISTS e2ee_signed_prekeys (
        key_id INTEGER PRIMARY KEY,
        secret BLOB NOT NULL,
        public BLOB NOT NULL,
        signature BLOB NOT NULL,
        created_at_ms INTEGER NOT NULL
    );
    CREATE TABLE IF NOT EXISTS e2ee_one_time_prekeys (
        key_id INTEGER PRIMARY KEY,
        secret BLOB NOT NULL,
        public BLOB NOT NULL,
        consumed INTEGER NOT NULL DEFAULT 0,
        created_at_ms INTEGER NOT NULL
    );
    CREATE TABLE IF NOT EXISTS e2ee_sessions (
        peer_user_id INTEGER NOT NULL,
        peer_device_id TEXT NOT NULL,
        state_json TEXT NOT NULL,
        updated_at_ms INTEGER NOT NULL,
        PRIMARY KEY (peer_user_id, peer_device_id)
    );
    CREATE TABLE IF NOT EXISTS e2ee_sender_keys (
        chat_id INTEGER NOT NULL,
        sender_user_id INTEGER NOT NULL,
        sender_device_id TEXT NOT NULL,
        state_json TEXT NOT NULL,
        updated_at_ms INTEGER NOT NULL,
        PRIMARY KEY (chat_id, sender_user_id, sender_device_id)
    );
    CREATE TABLE IF NOT EXISTS e2ee_sender_key_history (
        chat_id INTEGER NOT NULL,
        sender_user_id INTEGER NOT NULL,
        sender_device_id TEXT NOT NULL,
        chain_id INTEGER NOT NULL,
        state_json TEXT NOT NULL,
        updated_at_ms INTEGER NOT NULL,
        PRIMARY KEY (chat_id, sender_user_id, sender_device_id, chain_id)
    );
    CREATE TABLE IF NOT EXISTS e2ee_distributed_keys (
        chat_id INTEGER NOT NULL,
        chain_id INTEGER NOT NULL,
        peer_user_id INTEGER NOT NULL,
        peer_device_id TEXT NOT NULL,
        distributed_at_ms INTEGER NOT NULL,
        PRIMARY KEY (chat_id, chain_id, peer_user_id, peer_device_id)
    );
    CREATE TABLE IF NOT EXISTS user_creations_cache (
        creation_id TEXT PRIMARY KEY,
        cloud_revision INTEGER NOT NULL,
        kind TEXT NOT NULL,
        review_status TEXT NOT NULL,
        payload_json TEXT NOT NULL,
        deleted_at_ms INTEGER,
        updated_at_ms INTEGER NOT NULL
    );
    CREATE INDEX IF NOT EXISTS idx_user_creations_cache_kind_updated
        ON user_creations_cache(kind, updated_at_ms DESC);
    CREATE TABLE IF NOT EXISTS user_creation_states_cache (
        creation_id TEXT PRIMARY KEY,
        state_json TEXT NOT NULL,
        updated_at_ms INTEGER NOT NULL
    );
    CREATE TABLE IF NOT EXISTS public_catalog_cache (
        creation_id TEXT PRIMARY KEY,
        kind TEXT NOT NULL,
        payload_json TEXT NOT NULL,
        cached_at_ms INTEGER NOT NULL
    );
    CREATE INDEX IF NOT EXISTS idx_public_catalog_cache_kind_cached
        ON public_catalog_cache(kind, cached_at_ms DESC);
    CREATE TABLE IF NOT EXISTS installed_automation_apps (
        app_id TEXT PRIMARY KEY,
        slug TEXT NOT NULL,
        name TEXT NOT NULL,
        installed_version TEXT NOT NULL,
        installed_at_ms INTEGER NOT NULL,
        last_run_at_ms INTEGER,
        payload_json TEXT NOT NULL,
        updated_at_ms INTEGER NOT NULL
    );
    CREATE INDEX IF NOT EXISTS idx_installed_automation_apps_updated
        ON installed_automation_apps(updated_at_ms DESC, app_id);
    CREATE TABLE IF NOT EXISTS automation_app_cache (
        app_id TEXT PRIMARY KEY,
        slug TEXT NOT NULL,
        latest_version TEXT NOT NULL,
        payload_json TEXT NOT NULL,
        cached_at_ms INTEGER NOT NULL
    );
    CREATE TABLE IF NOT EXISTS code_scenes_cache (
        scene_id TEXT PRIMARY KEY,
        kind TEXT NOT NULL,
        author_user_id INTEGER NOT NULL,
        creator_bot_id INTEGER,
        title TEXT NOT NULL,
        source_sha256 TEXT NOT NULL,
        payload_json TEXT NOT NULL,
        is_local_only INTEGER NOT NULL DEFAULT 0,
        updated_at_ms INTEGER NOT NULL,
        cached_at_ms INTEGER NOT NULL
    );
    CREATE INDEX IF NOT EXISTS idx_code_scenes_cache_kind_updated
        ON code_scenes_cache(kind, updated_at_ms DESC, scene_id);
    CREATE INDEX IF NOT EXISTS idx_code_scenes_cache_author
        ON code_scenes_cache(author_user_id, updated_at_ms DESC);

    CREATE TABLE IF NOT EXISTS code_scene_local_profile (
        user_id INTEGER PRIMARY KEY,
        schema_version INTEGER NOT NULL DEFAULT 1,
        revision INTEGER NOT NULL DEFAULT 0,
        profile_json TEXT NOT NULL,
        updated_at_ms INTEGER NOT NULL
    );

    CREATE TABLE IF NOT EXISTS code_scene_telemetry_outbox (
        outbox_id INTEGER PRIMARY KEY AUTOINCREMENT,
        event_id TEXT NOT NULL UNIQUE,
        user_id INTEGER NOT NULL,
        scene_id TEXT NOT NULL,
        event_kind TEXT NOT NULL,
        payload_json TEXT NOT NULL,
        retry_count INTEGER NOT NULL DEFAULT 0,
        last_error TEXT,
        created_at_ms INTEGER NOT NULL,
        updated_at_ms INTEGER NOT NULL
    );
    CREATE INDEX IF NOT EXISTS idx_code_scene_telemetry_outbox_user
        ON code_scene_telemetry_outbox(user_id, outbox_id ASC);

    CREATE TABLE IF NOT EXISTS code_scene_local_drafts (
        draft_id TEXT PRIMARY KEY,
        user_id INTEGER NOT NULL,
        kind TEXT NOT NULL,
        title TEXT NOT NULL,
        remix_parent_id TEXT,
        prompt TEXT NOT NULL DEFAULT '',
        payload_json TEXT NOT NULL,
        created_at_ms INTEGER NOT NULL,
        updated_at_ms INTEGER NOT NULL
    );
    CREATE INDEX IF NOT EXISTS idx_code_scene_local_drafts_user
        ON code_scene_local_drafts(user_id, updated_at_ms DESC, draft_id);

    CREATE TABLE IF NOT EXISTS code_scene_local_social (
        user_id INTEGER NOT NULL,
        relation_kind TEXT NOT NULL,
        target_id TEXT NOT NULL,
        active INTEGER NOT NULL,
        pending_sync INTEGER NOT NULL DEFAULT 0,
        updated_at_ms INTEGER NOT NULL,
        PRIMARY KEY (user_id, relation_kind, target_id)
    );
    CREATE INDEX IF NOT EXISTS idx_code_scene_local_social_pending
        ON code_scene_local_social(user_id, pending_sync, updated_at_ms ASC);

    CREATE TABLE IF NOT EXISTS code_scene_local_comments (
        comment_id TEXT PRIMARY KEY,
        scene_id TEXT NOT NULL,
        author_user_id INTEGER NOT NULL,
        parent_comment_id TEXT,
        likes_count INTEGER NOT NULL DEFAULT 0,
        created_at_ms INTEGER NOT NULL,
        pending_sync INTEGER NOT NULL DEFAULT 0,
        payload_json TEXT NOT NULL
    );
    CREATE INDEX IF NOT EXISTS idx_code_scene_local_comments_scene_created
        ON code_scene_local_comments(scene_id, created_at_ms DESC, comment_id);
    CREATE INDEX IF NOT EXISTS idx_code_scene_local_comments_scene_hot
        ON code_scene_local_comments(scene_id, likes_count DESC, created_at_ms DESC);

    CREATE TABLE IF NOT EXISTS cached_dialog_summaries (
        chat_id INTEGER PRIMARY KEY,
        sort_order INTEGER NOT NULL,
        payload_json TEXT NOT NULL
    );
    CREATE INDEX IF NOT EXISTS idx_cached_dialog_summaries_order
        ON cached_dialog_summaries(sort_order ASC, chat_id ASC);
    CREATE TABLE IF NOT EXISTS cached_rich_chats (
        chat_id INTEGER PRIMARY KEY,
        payload_json TEXT NOT NULL
    );
    CREATE TABLE IF NOT EXISTS cached_chat_histories (
        chat_id INTEGER PRIMARY KEY,
        payload_json TEXT NOT NULL,
        updated_at_ms INTEGER NOT NULL
    );
    CREATE TABLE IF NOT EXISTS cached_contacts (
        user_id INTEGER PRIMARY KEY,
        sort_order INTEGER NOT NULL,
        payload_json TEXT NOT NULL
    );
    CREATE INDEX IF NOT EXISTS idx_cached_contacts_order
        ON cached_contacts(sort_order ASC, user_id ASC);
";

impl SqliteClientCache {
    pub(crate) fn open(base_dir: &Path) -> Result<Self> {
        let media_dir = base_dir.join("media");
        std::fs::create_dir_all(&media_dir)?;
        let db_path = base_dir.join("client_cache.db");
        let conn = Connection::open(&db_path).map_err(db_err)?;
        conn.execute_batch(CLIENT_SCHEMA_SQL).map_err(db_err)?;
        Ok(Self {
            conn: Mutex::new(conn),
            base_dir: base_dir.to_path_buf(),
            media_dir,
        })
    }

    pub(crate) fn open_in_memory() -> Result<Self> {
        let conn = Connection::open_in_memory().map_err(db_err)?;
        conn.execute_batch(CLIENT_SCHEMA_SQL).map_err(db_err)?;
        Ok(Self {
            conn: Mutex::new(conn),
            base_dir: PathBuf::new(),
            media_dir: PathBuf::new(),
        })
    }

    pub(crate) fn base_dir(&self) -> &Path {
        &self.base_dir
    }

    pub(crate) fn load_owner_scope(&self) -> Option<(String, u64)> {
        let conn = self.conn.lock();
        let uid_str: Option<String> = conn
            .query_row(
                "SELECT value FROM cache_meta WHERE key = 'owner_user_id'",
                [],
                |r| r.get(0),
            )
            .ok();
        let uid = uid_str.and_then(|s| s.parse::<u64>().ok()).filter(|&id| id > 0)?;
        let srv: String = conn
            .query_row(
                "SELECT value FROM cache_meta WHERE key = 'owner_server_url'",
                [],
                |r| r.get(0),
            )
            .unwrap_or_default();
        Some((srv, uid))
    }

    pub(crate) fn save_owner_scope(&self, server_url: &str, user_id: u64) {
        let conn = self.conn.lock();
        let _ = conn.execute(
            "INSERT OR REPLACE INTO cache_meta (key, value) VALUES ('owner_user_id', ?1)",
            params![user_id.to_string()],
        );
        let _ = conn.execute(
            "INSERT OR REPLACE INTO cache_meta (key, value) VALUES ('owner_server_url', ?1)",
            params![server_url],
        );
    }

    pub(crate) fn load_message_reactions(&self) -> Result<HashMap<(ChatId, MessageId), Vec<ReactionCount>>> {
        let conn = self.conn.lock();
        let mut stmt = conn
            .prepare("SELECT chat_id, msg_id, payload_json FROM message_reactions")
            .map_err(db_err)?;
        let rows = stmt
            .query_map([], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, i64>(1)?, r.get::<_, String>(2)?)))
            .map_err(db_err)?;
        let mut out = HashMap::new();
        for row in rows {
            let (cid, mid, json) = row.map_err(db_err)?;
            if let Ok(reactions) = serde_json::from_str::<Vec<ReactionCount>>(&json) {
                if !reactions.is_empty() {
                    out.insert((ChatId(cid), MessageId(mid)), reactions);
                }
            }
        }
        Ok(out)
    }

    pub(crate) fn upsert_message_reactions(
        &self,
        chat_id: ChatId,
        msg_id: MessageId,
        reactions: &[ReactionCount],
    ) {
        let conn = self.conn.lock();
        if reactions.is_empty() {
            let _ = conn.execute(
                "DELETE FROM message_reactions WHERE chat_id = ?1 AND msg_id = ?2",
                params![chat_id.0, msg_id.0],
            );
        } else if let Ok(json) = serde_json::to_string(reactions) {
            let _ = conn.execute(
                "INSERT OR REPLACE INTO message_reactions (chat_id, msg_id, payload_json) VALUES (?1, ?2, ?3)",
                params![chat_id.0, msg_id.0, json],
            );
        }
    }

    pub(crate) fn clear_chat_reactions(&self, chat_id: ChatId) {
        let conn = self.conn.lock();
        let _ = conn.execute(
            "DELETE FROM message_reactions WHERE chat_id = ?1",
            params![chat_id.0],
        );
    }

    pub(crate) fn clear_all(&self) {
        let mut conn = self.conn.lock();
        let Ok(tx) = conn.transaction_with_behavior(TransactionBehavior::Immediate) else {
            return;
        };
        const TABLES: &[&str] = &[
            "messages",
            "message_reactions",
            "chats",
            "drafts",
            "read_states",
            "cache_meta",
            "outbox_messages",
            "file_transfers",
            "folders",
            "chat_ttls",
            "e2ee_identity",
            "e2ee_signed_prekeys",
            "e2ee_one_time_prekeys",
            "e2ee_sessions",
            "e2ee_sender_keys",
            "e2ee_sender_key_history",
            "e2ee_distributed_keys",
            "user_creations_cache",
            "user_creation_states_cache",
            "public_catalog_cache",
            "installed_automation_apps",
            "automation_app_cache",
            "code_scenes_cache",
            "code_scene_local_profile",
            "code_scene_telemetry_outbox",
            "code_scene_local_drafts",
            "code_scene_local_social",
            "code_scene_local_comments",
            "cached_dialog_summaries",
            "cached_rich_chats",
            "cached_chat_histories",
            "cached_contacts",
        ];
        for table in TABLES {
            let sql = format!("DELETE FROM {table}");
            if tx.execute(&sql, []).is_err() {
                return;
            }
        }
        let _ = tx.commit();
    }

    pub(crate) fn max_outbox_nonce(&self) -> u64 {
        let conn = self.conn.lock();
        conn.query_row(
            "SELECT COALESCE(MAX(client_nonce), 0) FROM outbox_messages",
            [],
            |r| r.get::<_, i64>(0),
        )
        .unwrap_or(0)
        .max(0) as u64
    }

    pub(crate) fn load_folders(&self) -> Result<Vec<ChatFolderDto>> {
        let conn = self.conn.lock();
        let mut stmt = conn
            .prepare("SELECT payload_json FROM folders ORDER BY folder_id ASC")
            .map_err(db_err)?;
        let rows = stmt
            .query_map([], |r| r.get::<_, String>(0))
            .map_err(db_err)?;
        let mut out = Vec::new();
        for row in rows {
            let json = row.map_err(db_err)?;
            if let Ok(folder) = serde_json::from_str::<ChatFolderDto>(&json) {
                out.push(folder);
            }
        }
        Ok(out)
    }

    pub(crate) fn replace_folders(&self, folders: &[ChatFolderDto]) {
        let conn = self.conn.lock();
        let _ = conn.execute("DELETE FROM folders", []);
        for f in folders {
            if let Ok(json) = serde_json::to_string(f) {
                let _ = conn.execute(
                    "INSERT OR REPLACE INTO folders (folder_id, payload_json) VALUES (?1, ?2)",
                    params![f.id, json],
                );
            }
        }
    }

    pub(crate) fn upsert_folder(&self, folder: &ChatFolderDto) {
        if let Ok(json) = serde_json::to_string(folder) {
            let conn = self.conn.lock();
            let _ = conn.execute(
                "INSERT OR REPLACE INTO folders (folder_id, payload_json) VALUES (?1, ?2)",
                params![folder.id, json],
            );
        }
    }

    pub(crate) fn delete_folder(&self, folder_id: i64) {
        let conn = self.conn.lock();
        let _ = conn.execute("DELETE FROM folders WHERE folder_id = ?1", params![folder_id]);
    }

    pub(crate) fn load_chat_ttls(&self) -> Result<HashMap<ChatId, u32>> {
        let conn = self.conn.lock();
        let mut stmt = conn
            .prepare("SELECT chat_id, ttl_seconds FROM chat_ttls")
            .map_err(db_err)?;
        let rows = stmt
            .query_map([], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, i64>(1)?)))
            .map_err(db_err)?;
        let mut out = HashMap::new();
        for row in rows {
            let (cid, ttl) = row.map_err(db_err)?;
            if ttl > 0 {
                out.insert(ChatId(cid), ttl as u32);
            }
        }
        Ok(out)
    }

    pub(crate) fn upsert_chat_ttl(&self, chat_id: ChatId, ttl_seconds: u32) {
        let conn = self.conn.lock();
        if ttl_seconds == 0 {
            let _ = conn.execute("DELETE FROM chat_ttls WHERE chat_id = ?1", params![chat_id.0]);
        } else {
            let _ = conn.execute(
                "INSERT OR REPLACE INTO chat_ttls (chat_id, ttl_seconds) VALUES (?1, ?2)",
                params![chat_id.0, ttl_seconds as i64],
            );
        }
    }

    pub(crate) fn load_messages(&self) -> Result<HashMap<ChatId, Vec<MessageItem>>> {
        let conn = self.conn.lock();
        let mut stmt = conn
            .prepare("SELECT payload_json FROM messages ORDER BY date_ms ASC, msg_id ASC")
            .map_err(db_err)?;
        let rows = stmt
            .query_map([], |r| r.get::<_, String>(0))
            .map_err(db_err)?;
        let mut out: HashMap<ChatId, Vec<MessageItem>> = HashMap::new();
        for row in rows {
            let json = row.map_err(db_err)?;
            if let Ok(item) = serde_json::from_str::<MessageItem>(&json) {
                out.entry(item.chat_id).or_default().push(item);
            }
        }
        Ok(out)
    }

    pub(crate) fn load_chats(&self) -> Result<HashMap<ChatId, ChatDetailDto>> {
        let conn = self.conn.lock();
        let mut stmt = conn.prepare("SELECT payload_json FROM chats").map_err(db_err)?;
        let rows = stmt.query_map([], |r| r.get::<_, String>(0)).map_err(db_err)?;
        let mut out = HashMap::new();
        for row in rows {
            let json = row.map_err(db_err)?;
            if let Ok(chat) = serde_json::from_str::<ChatDetailDto>(&json) {
                out.insert(ChatId(chat.chat_id), chat);
            }
        }
        Ok(out)
    }

    pub(crate) fn load_drafts(&self) -> Result<HashMap<ChatId, DraftDto>> {
        let conn = self.conn.lock();
        let mut stmt = conn.prepare("SELECT payload_json FROM drafts").map_err(db_err)?;
        let rows = stmt.query_map([], |r| r.get::<_, String>(0)).map_err(db_err)?;
        let mut out = HashMap::new();
        for row in rows {
            let json = row.map_err(db_err)?;
            if let Ok(draft) = serde_json::from_str::<DraftDto>(&json) {
                out.insert(ChatId(draft.chat_id), draft);
            }
        }
        Ok(out)
    }

    pub(crate) fn load_read_states(&self) -> Result<(HashMap<ChatId, MessageId>, HashMap<ChatId, MessageId>)> {
        let conn = self.conn.lock();
        let mut stmt = conn
            .prepare("SELECT chat_id, read_inbox_max, read_outbox_max FROM read_states")
            .map_err(db_err)?;
        let rows = stmt
            .query_map([], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, i64>(1)?, r.get::<_, i64>(2)?)))
            .map_err(db_err)?;
        let (mut inbox, mut outbox) = (HashMap::new(), HashMap::new());
        for row in rows {
            let (cid, r_in, r_out) = row.map_err(db_err)?;
            if r_in > 0 {
                inbox.insert(ChatId(cid), MessageId(r_in));
            }
            if r_out > 0 {
                outbox.insert(ChatId(cid), MessageId(r_out));
            }
        }
        Ok((inbox, outbox))
    }

    pub(crate) fn load_media_index(&self) -> Result<HashMap<String, CachedMediaEntry>> {
        let conn = self.conn.lock();
        let mut stmt = conn
            .prepare("SELECT file_id, sha256, mime_type, size_bytes, local_path FROM media_cas")
            .map_err(db_err)?;
        let rows = stmt
            .query_map([], |r| {
                Ok(CachedMediaEntry {
                    file_id: r.get(0)?,
                    sha256: r.get(1)?,
                    mime_type: r.get(2)?,
                    size_bytes: r.get::<_, i64>(3)?.max(0) as u64,
                    local_path: PathBuf::from(r.get::<_, String>(4)?),
                })
            })
            .map_err(db_err)?;
        let mut out = HashMap::new();
        for row in rows {
            let entry = row.map_err(db_err)?;
            out.insert(entry.file_id.clone(), entry);
        }
        Ok(out)
    }

    pub(crate) fn upsert_message(&self, msg: &MessageItem) {
        if let Ok(json) = serde_json::to_string(msg) {
            let conn = self.conn.lock();
            let _ = conn.execute(
                "INSERT OR REPLACE INTO messages (chat_id, msg_id, date_ms, payload_json) VALUES (?1, ?2, ?3, ?4)",
                params![msg.chat_id.0, msg.id.0, msg.date.timestamp_millis(), json],
            );
        }
    }

    pub(crate) fn delete_message(&self, chat_id: ChatId, msg_id: MessageId) {
        let conn = self.conn.lock();
        let _ = conn.execute(
            "DELETE FROM messages WHERE chat_id = ?1 AND msg_id = ?2",
            params![chat_id.0, msg_id.0],
        );
        let _ = conn.execute(
            "DELETE FROM message_reactions WHERE chat_id = ?1 AND msg_id = ?2",
            params![chat_id.0, msg_id.0],
        );
    }

    pub(crate) fn upsert_chat(&self, chat: &ChatDetailDto) {
        if let Ok(json) = serde_json::to_string(chat) {
            let conn = self.conn.lock();
            let _ = conn.execute(
                "INSERT OR REPLACE INTO chats (chat_id, payload_json) VALUES (?1, ?2)",
                params![chat.chat_id, json],
            );
        }
    }

    pub(crate) fn set_draft(&self, chat_id: ChatId, draft: Option<&DraftDto>) {
        let conn = self.conn.lock();
        if let Some(d) = draft {
            if let Ok(json) = serde_json::to_string(d) {
                let _ = conn.execute(
                    "INSERT OR REPLACE INTO drafts (chat_id, payload_json) VALUES (?1, ?2)",
                    params![chat_id.0, json],
                );
            }
        } else {
            let _ = conn.execute("DELETE FROM drafts WHERE chat_id = ?1", params![chat_id.0]);
        }
    }

    pub(crate) fn upsert_read_state(&self, chat_id: ChatId, inbox: i64, outbox: i64) {
        let conn = self.conn.lock();
        let _ = conn.execute(
            "INSERT OR REPLACE INTO read_states (chat_id, read_inbox_max, read_outbox_max) VALUES (?1, ?2, ?3)",
            params![chat_id.0, inbox, outbox],
        );
    }

    pub(crate) fn write_media_cas(
        &self,
        file_id: &str,
        bytes: &[u8],
        mime_type: Option<&str>,
    ) -> Result<CachedMediaEntry> {
        let sha = sha256_hex(bytes);
        let final_path = self.media_dir.join(format!("{}.bin", sha));
        if !final_path.exists() {
            let tmp_path = self
                .media_dir
                .join(format!(".tmp-{}-{}", sha, std::process::id()));
            std::fs::write(&tmp_path, bytes)?;
            if let Err(err) = std::fs::rename(&tmp_path, &final_path) {
                let _ = std::fs::remove_file(&tmp_path);
                return Err(err.into());
            }
        }
        let entry = CachedMediaEntry {
            file_id: file_id.to_string(),
            sha256: sha.clone(),
            mime_type: mime_type.map(|m| m.to_string()),
            size_bytes: bytes.len() as u64,
            local_path: final_path.clone(),
        };
        let conn = self.conn.lock();
        conn.execute(
            "INSERT OR REPLACE INTO media_cas (file_id, sha256, mime_type, size_bytes, local_path) VALUES (?1, ?2, ?3, ?4, ?5)",
            params![
                entry.file_id,
                entry.sha256,
                entry.mime_type,
                entry.size_bytes as i64,
                final_path.to_string_lossy().as_ref()
            ],
        )
        .map_err(db_err)?;
        Ok(entry)
    }

    pub(crate) fn load_installed_automation_apps(
        &self,
    ) -> Result<HashMap<String, InstalledAutomationApp>> {
        let conn = self.conn.lock();
        let mut stmt = conn
            .prepare(
                "SELECT payload_json, last_run_at_ms FROM installed_automation_apps ORDER BY installed_at_ms DESC, app_id ASC",
            )
            .map_err(db_err)?;
        let rows = stmt
            .query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, Option<i64>>(1)?)))
            .map_err(db_err)?;
        let mut out = HashMap::new();
        for row in rows {
            let (json, last_run_col) = row.map_err(db_err)?;
            if let Ok(mut entry) = serde_json::from_str::<InstalledAutomationApp>(&json) {
                if entry.last_run_at_ms.is_none() {
                    entry.last_run_at_ms = last_run_col;
                }
                out.insert(entry.app_id.clone(), entry);
            }
        }
        Ok(out)
    }

    pub(crate) fn replace_installed_automation_apps(
        &self,
        apps: &[InstalledAutomationApp],
        now_ms: i64,
    ) {
        let mut conn = self.conn.lock();
        if let Ok(tx) = conn.transaction() {
            let _ = tx.execute("DELETE FROM installed_automation_apps", []);
            for entry in apps {
                if let Ok(json) = serde_json::to_string(entry) {
                    let _ = tx.execute(
                        r#"
                        INSERT OR REPLACE INTO installed_automation_apps (
                            app_id, slug, name, installed_version, installed_at_ms,
                            last_run_at_ms, payload_json, updated_at_ms
                        ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)
                        "#,
                        params![
                            entry.app_id,
                            entry.slug,
                            entry.name,
                            entry.installed_version,
                            entry.installed_at_ms,
                            entry.last_run_at_ms,
                            json,
                            now_ms,
                        ],
                    );
                }
            }
            let _ = tx.commit();
        }
    }

    pub(crate) fn upsert_installed_automation_app(
        &self,
        entry: &InstalledAutomationApp,
        now_ms: i64,
    ) {
        if let Ok(json) = serde_json::to_string(entry) {
            let conn = self.conn.lock();
            let _ = conn.execute(
                r#"
                INSERT OR REPLACE INTO installed_automation_apps (
                    app_id, slug, name, installed_version, installed_at_ms,
                    last_run_at_ms, payload_json, updated_at_ms
                ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)
                "#,
                params![
                    entry.app_id,
                    entry.slug,
                    entry.name,
                    entry.installed_version,
                    entry.installed_at_ms,
                    entry.last_run_at_ms,
                    json,
                    now_ms,
                ],
            );
        }
    }

    pub(crate) fn delete_installed_automation_app(&self, app_id: &str) {
        let conn = self.conn.lock();
        let _ = conn.execute(
            "DELETE FROM installed_automation_apps WHERE app_id = ?1",
            params![app_id],
        );
    }

    pub(crate) fn load_cached_automation_apps(
        &self,
    ) -> Result<HashMap<String, AutomationAppDetail>> {
        let conn = self.conn.lock();
        let mut stmt = conn
            .prepare("SELECT payload_json FROM automation_app_cache")
            .map_err(db_err)?;
        let rows = stmt
            .query_map([], |r| r.get::<_, String>(0))
            .map_err(db_err)?;
        let mut out = HashMap::new();
        for row in rows {
            let json = row.map_err(db_err)?;
            if let Ok(detail) = serde_json::from_str::<AutomationAppDetail>(&json) {
                out.insert(detail.app_id.clone(), detail);
            }
        }
        Ok(out)
    }

    pub(crate) fn upsert_cached_automation_app(
        &self,
        detail: &AutomationAppDetail,
        cached_at_ms: i64,
    ) {
        if let Ok(json) = serde_json::to_string(detail) {
            let conn = self.conn.lock();
            let _ = conn.execute(
                r#"
                INSERT OR REPLACE INTO automation_app_cache (
                    app_id, slug, latest_version, payload_json, cached_at_ms
                ) VALUES (?1, ?2, ?3, ?4, ?5)
                "#,
                params![
                    detail.app_id,
                    detail.slug,
                    detail.latest_version,
                    json,
                    cached_at_ms,
                ],
            );
        }
    }

    pub(crate) fn load_meta_value(&self, key: &str) -> Option<String> {
        let conn = self.conn.lock();
        conn.query_row(
            "SELECT value FROM cache_meta WHERE key = ?1",
            params![key],
            |r| r.get::<_, String>(0),
        )
        .ok()
    }

    pub(crate) fn save_meta_value(&self, key: &str, value: Option<&str>) {
        let conn = self.conn.lock();
        match value {
            Some(v) => {
                let _ = conn.execute(
                    "INSERT OR REPLACE INTO cache_meta (key, value) VALUES (?1, ?2)",
                    params![key, v],
                );
            }
            None => {
                let _ = conn.execute("DELETE FROM cache_meta WHERE key = ?1", params![key]);
            }
        }
    }

    pub(crate) fn load_cached_dialog_summaries(&self) -> Result<Vec<ChatSummary>> {
        let conn = self.conn.lock();
        let mut stmt = conn
            .prepare("SELECT payload_json FROM cached_dialog_summaries ORDER BY sort_order ASC, chat_id ASC")
            .map_err(db_err)?;
        let rows = stmt
            .query_map([], |r| r.get::<_, String>(0))
            .map_err(db_err)?;
        let mut out = Vec::new();
        for row in rows {
            let json = row.map_err(db_err)?;
            if let Ok(chat) = serde_json::from_str::<ChatSummary>(&json) {
                out.push(chat);
            }
        }
        Ok(out)
    }

    pub(crate) fn replace_cached_dialog_summaries(&self, chats: &[ChatSummary]) {
        let mut conn = self.conn.lock();
        if let Ok(tx) = conn.transaction() {
            let _ = tx.execute("DELETE FROM cached_dialog_summaries", []);
            for (idx, chat) in chats.iter().enumerate() {
                if let Ok(json) = serde_json::to_string(chat) {
                    let _ = tx.execute(
                        "INSERT OR REPLACE INTO cached_dialog_summaries (chat_id, sort_order, payload_json) VALUES (?1, ?2, ?3)",
                        params![chat.chat_id as i64, idx as i64, json],
                    );
                }
            }
            let _ = tx.commit();
        }
    }

    pub(crate) fn delete_cached_dialog_summary(&self, chat_id: u64) {
        let conn = self.conn.lock();
        let _ = conn.execute(
            "DELETE FROM cached_dialog_summaries WHERE chat_id = ?1",
            params![chat_id as i64],
        );
    }

    pub(crate) fn load_cached_rich_chats(&self) -> Result<HashMap<u64, HermesRichChatDto>> {
        let conn = self.conn.lock();
        let mut stmt = conn
            .prepare("SELECT chat_id, payload_json FROM cached_rich_chats")
            .map_err(db_err)?;
        let rows = stmt
            .query_map([], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?)))
            .map_err(db_err)?;
        let mut out = HashMap::new();
        for row in rows {
            let (cid, json) = row.map_err(db_err)?;
            if cid > 0 {
                if let Ok(rc) = serde_json::from_str::<HermesRichChatDto>(&json) {
                    out.insert(cid as u64, rc);
                }
            }
        }
        Ok(out)
    }

    pub(crate) fn replace_cached_rich_chats(&self, chats: &[HermesRichChatDto]) {
        let mut conn = self.conn.lock();
        if let Ok(tx) = conn.transaction() {
            let _ = tx.execute("DELETE FROM cached_rich_chats", []);
            for rc in chats {
                if rc.chat_id > 0 {
                    if let Ok(json) = serde_json::to_string(rc) {
                        let _ = tx.execute(
                            "INSERT OR REPLACE INTO cached_rich_chats (chat_id, payload_json) VALUES (?1, ?2)",
                            params![rc.chat_id, json],
                        );
                    }
                }
            }
            let _ = tx.commit();
        }
    }

    pub(crate) fn upsert_cached_rich_chat(&self, rc: &HermesRichChatDto) {
        if rc.chat_id <= 0 {
            return;
        }
        if let Ok(json) = serde_json::to_string(rc) {
            let conn = self.conn.lock();
            let _ = conn.execute(
                "INSERT OR REPLACE INTO cached_rich_chats (chat_id, payload_json) VALUES (?1, ?2)",
                params![rc.chat_id, json],
            );
        }
    }

    pub(crate) fn delete_cached_rich_chat(&self, chat_id: u64) {
        let conn = self.conn.lock();
        let _ = conn.execute(
            "DELETE FROM cached_rich_chats WHERE chat_id = ?1",
            params![chat_id as i64],
        );
    }

    pub(crate) fn load_cached_chat_histories(&self) -> Result<HashMap<u64, CachedChatHistorySnapshot>> {
        let conn = self.conn.lock();
        let mut stmt = conn
            .prepare("SELECT chat_id, payload_json FROM cached_chat_histories")
            .map_err(db_err)?;
        let rows = stmt
            .query_map([], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?)))
            .map_err(db_err)?;
        let mut out = HashMap::new();
        for row in rows {
            let (cid, json) = row.map_err(db_err)?;
            if cid > 0 {
                if let Ok(snap) = serde_json::from_str::<CachedChatHistorySnapshot>(&json) {
                    out.insert(cid as u64, snap);
                }
            }
        }
        Ok(out)
    }

    pub(crate) fn upsert_cached_chat_history(&self, chat_id: u64, snap: &CachedChatHistorySnapshot) {
        if chat_id == 0 {
            return;
        }
        if let Ok(json) = serde_json::to_string(snap) {
            let conn = self.conn.lock();
            let _ = conn.execute(
                "INSERT OR REPLACE INTO cached_chat_histories (chat_id, payload_json, updated_at_ms) VALUES (?1, ?2, ?3)",
                params![chat_id as i64, json, snap.updated_at_ms],
            );
        }
    }

    pub(crate) fn delete_cached_chat_history(&self, chat_id: u64) {
        let conn = self.conn.lock();
        let _ = conn.execute(
            "DELETE FROM cached_chat_histories WHERE chat_id = ?1",
            params![chat_id as i64],
        );
    }

    pub(crate) fn load_cached_contacts(&self) -> Result<Vec<ContactDto>> {
        let conn = self.conn.lock();
        let mut stmt = conn
            .prepare("SELECT payload_json FROM cached_contacts ORDER BY sort_order ASC, user_id ASC")
            .map_err(db_err)?;
        let rows = stmt
            .query_map([], |r| r.get::<_, String>(0))
            .map_err(db_err)?;
        let mut out = Vec::new();
        for row in rows {
            let json = row.map_err(db_err)?;
            if let Ok(contact) = serde_json::from_str::<ContactDto>(&json) {
                out.push(contact);
            }
        }
        Ok(out)
    }

    pub(crate) fn replace_cached_contacts(&self, contacts: &[ContactDto]) {
        let mut conn = self.conn.lock();
        if let Ok(tx) = conn.transaction() {
            let _ = tx.execute("DELETE FROM cached_contacts", []);
            for (idx, c) in contacts.iter().enumerate() {
                if let Ok(json) = serde_json::to_string(c) {
                    let _ = tx.execute(
                        "INSERT OR REPLACE INTO cached_contacts (user_id, sort_order, payload_json) VALUES (?1, ?2, ?3)",
                        params![c.user_id as i64, idx as i64, json],
                    );
                }
            }
            let _ = tx.commit();
        }
    }
}

fn json_cache_err(err: serde_json::Error) -> TelegramError {
    TelegramError::Database(format!("cache serialization error: {err}"))
}

pub(crate) fn sort_user_creations(items: &mut [UserCreationDto]) {
    items.sort_by(|a, b| {
        b.updated_at_ms
            .cmp(&a.updated_at_ms)
            .then_with(|| a.creation_id.cmp(&b.creation_id))
    });
}

pub(crate) fn sort_user_creation_states(items: &mut [UserCreationStateDto]) {
    items.sort_by(|a, b| {
        b.updated_at_ms
            .cmp(&a.updated_at_ms)
            .then_with(|| a.creation_id.cmp(&b.creation_id))
    });
}

pub(crate) fn sort_public_catalog_items(items: &mut [PublicCatalogItemDto]) {
    items.sort_by(|a, b| {
        let a_rank = a.featured_rank.unwrap_or(u32::MAX);
        let b_rank = b.featured_rank.unwrap_or(u32::MAX);
        a_rank
            .cmp(&b_rank)
            .then_with(|| b.published_at_ms.cmp(&a.published_at_ms))
            .then_with(|| a.creation_id.cmp(&b.creation_id))
    });
}

const UPSERT_USER_CREATION_SQL: &str = "
    INSERT INTO user_creations_cache (
        creation_id,
        cloud_revision,
        kind,
        review_status,
        payload_json,
        deleted_at_ms,
        updated_at_ms
    ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)
    ON CONFLICT(creation_id) DO UPDATE SET
        cloud_revision = excluded.cloud_revision,
        kind = excluded.kind,
        review_status = excluded.review_status,
        payload_json = excluded.payload_json,
        deleted_at_ms = excluded.deleted_at_ms,
        updated_at_ms = excluded.updated_at_ms
    WHERE excluded.cloud_revision > user_creations_cache.cloud_revision
       OR (
            excluded.cloud_revision = user_creations_cache.cloud_revision
            AND (user_creations_cache.deleted_at_ms IS NULL OR excluded.deleted_at_ms IS NOT NULL)
          )
";

impl SqliteClientCache {
    pub(crate) fn get_meta(&self, key: &str) -> Result<Option<String>> {
        let conn = self.conn.lock();
        conn.query_row(
            "SELECT value FROM cache_meta WHERE key = ?1",
            params![key],
            |r| r.get::<_, String>(0),
        )
        .optional()
        .map_err(db_err)
    }

    pub(crate) fn set_meta(&self, key: &str, value: &str) -> Result<()> {
        let conn = self.conn.lock();
        conn.execute(
            "INSERT OR REPLACE INTO cache_meta (key, value) VALUES (?1, ?2)",
            params![key, value],
        )
        .map_err(db_err)?;
        Ok(())
    }

    pub(crate) fn delete_meta(&self, key: &str) -> Result<()> {
        let conn = self.conn.lock();
        conn.execute("DELETE FROM cache_meta WHERE key = ?1", params![key])
            .map_err(db_err)?;
        Ok(())
    }

    pub(crate) fn load_creations_last_revision(&self) -> Result<i64> {
        let raw = self.get_meta(META_CREATIONS_LAST_REVISION)?;
        Ok(raw
            .and_then(|s| s.trim().parse::<i64>().ok())
            .unwrap_or(0)
            .max(0))
    }

    pub(crate) fn save_creations_last_revision(&self, revision: i64) -> Result<()> {
        let clamped = revision.max(0);
        let mut conn = self.conn.lock();
        let tx = conn
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(db_err)?;
        let prev_rev: i64 = tx
            .query_row(
                "SELECT value FROM cache_meta WHERE key = ?1",
                params![META_CREATIONS_LAST_REVISION],
                |r| r.get::<_, String>(0),
            )
            .optional()
            .map_err(db_err)?
            .and_then(|s| s.trim().parse::<i64>().ok())
            .unwrap_or(0)
            .max(0);
        let next_rev = prev_rev.max(clamped);
        tx.execute(
            "INSERT OR REPLACE INTO cache_meta (key, value) VALUES (?1, ?2)",
            params![META_CREATIONS_LAST_REVISION, next_rev.to_string()],
        )
        .map_err(db_err)?;
        tx.commit().map_err(db_err)?;
        Ok(())
    }

    pub(crate) fn creations_last_revision(&self) -> Result<i64> {
        self.load_creations_last_revision()
    }

    pub(crate) fn upsert_user_creations(&self, items: &[UserCreationDto]) -> Result<()> {
        if items.is_empty() {
            return Ok(());
        }
        let mut conn = self.conn.lock();
        let tx = conn
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(db_err)?;
        {
            let mut stmt = tx.prepare(UPSERT_USER_CREATION_SQL).map_err(db_err)?;
            for item in items {
                let json = serde_json::to_string(item).map_err(json_cache_err)?;
                stmt.execute(params![
                    item.creation_id,
                    item.cloud_revision,
                    item.kind.as_str(),
                    item.review_status.as_str(),
                    json,
                    item.deleted_at_ms,
                    item.updated_at_ms
                ])
                .map_err(db_err)?;
            }
        }
        tx.commit().map_err(db_err)?;
        Ok(())
    }

    pub(crate) fn apply_creations_sync_delta(
        &self,
        latest_revision: i64,
        items: &[UserCreationDto],
    ) -> Result<i64> {
        let mut conn = self.conn.lock();
        let tx = conn
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(db_err)?;
        let mut max_item_rev: i64 = 0;
        {
            let mut stmt = tx.prepare(UPSERT_USER_CREATION_SQL).map_err(db_err)?;
            for item in items {
                max_item_rev = max_item_rev.max(item.cloud_revision.max(0));
                let json = serde_json::to_string(item).map_err(json_cache_err)?;
                stmt.execute(params![
                    item.creation_id,
                    item.cloud_revision,
                    item.kind.as_str(),
                    item.review_status.as_str(),
                    json,
                    item.deleted_at_ms,
                    item.updated_at_ms
                ])
                .map_err(db_err)?;
            }
        }
        let prev_rev: i64 = tx
            .query_row(
                "SELECT value FROM cache_meta WHERE key = ?1",
                params![META_CREATIONS_LAST_REVISION],
                |r| r.get::<_, String>(0),
            )
            .optional()
            .map_err(db_err)?
            .and_then(|s| s.trim().parse::<i64>().ok())
            .unwrap_or(0)
            .max(0);
        let next_rev = prev_rev.max(latest_revision.max(0)).max(max_item_rev);
        tx.execute(
            "INSERT OR REPLACE INTO cache_meta (key, value) VALUES (?1, ?2)",
            params![META_CREATIONS_LAST_REVISION, next_rev.to_string()],
        )
        .map_err(db_err)?;
        tx.commit().map_err(db_err)?;
        Ok(next_rev)
    }

    pub(crate) fn load_all_user_creations(&self) -> Result<HashMap<String, UserCreationDto>> {
        let conn = self.conn.lock();
        let mut stmt = conn
            .prepare("SELECT payload_json FROM user_creations_cache")
            .map_err(db_err)?;
        let rows = stmt
            .query_map([], |r| r.get::<_, String>(0))
            .map_err(db_err)?;
        let mut out = HashMap::new();
        for row in rows {
            let json = row.map_err(db_err)?;
            if let Ok(item) = serde_json::from_str::<UserCreationDto>(&json) {
                out.insert(item.creation_id.clone(), item);
            }
        }
        Ok(out)
    }

    pub(crate) fn load_user_creations(
        &self,
        kind: Option<CreationKind>,
        status: Option<CreationReviewStatus>,
    ) -> Result<Vec<UserCreationDto>> {
        let conn = self.conn.lock();
        let kind_str = kind.map(CreationKind::as_str);
        let status_str = status.map(CreationReviewStatus::as_str);
        let mut stmt = conn
            .prepare(
                "SELECT payload_json
                 FROM user_creations_cache
                 WHERE deleted_at_ms IS NULL
                   AND (?1 IS NULL OR kind = ?1)
                   AND (?2 IS NULL OR review_status = ?2)
                 ORDER BY updated_at_ms DESC, creation_id ASC",
            )
            .map_err(db_err)?;
        let rows = stmt
            .query_map(params![kind_str, status_str], |r| r.get::<_, String>(0))
            .map_err(db_err)?;
        let mut out = Vec::new();
        for row in rows {
            let json = row.map_err(db_err)?;
            if let Ok(item) = serde_json::from_str::<UserCreationDto>(&json) {
                if item.deleted_at_ms.is_none() {
                    out.push(item);
                }
            }
        }
        sort_user_creations(&mut out);
        Ok(out)
    }

    pub(crate) fn get_user_creation(&self, creation_id: &str) -> Result<Option<UserCreationDto>> {
        let conn = self.conn.lock();
        let raw: Option<String> = conn
            .query_row(
                "SELECT payload_json FROM user_creations_cache WHERE creation_id = ?1 AND deleted_at_ms IS NULL",
                params![creation_id],
                |r| r.get::<_, String>(0),
            )
            .optional()
            .map_err(db_err)?;
        Ok(raw
            .and_then(|json| serde_json::from_str::<UserCreationDto>(&json).ok())
            .filter(|item| item.deleted_at_ms.is_none()))
    }

    pub(crate) fn upsert_user_creation_state(&self, state: &UserCreationStateDto) -> Result<()> {
        let json = serde_json::to_string(state).map_err(json_cache_err)?;
        let conn = self.conn.lock();
        conn.execute(
            "INSERT INTO user_creation_states_cache (creation_id, state_json, updated_at_ms)
             VALUES (?1, ?2, ?3)
             ON CONFLICT(creation_id) DO UPDATE SET
                state_json = excluded.state_json,
                updated_at_ms = excluded.updated_at_ms
             WHERE excluded.updated_at_ms >= user_creation_states_cache.updated_at_ms",
            params![state.creation_id, json, state.updated_at_ms],
        )
        .map_err(db_err)?;
        Ok(())
    }

    pub(crate) fn upsert_user_creation_states(&self, states: &[UserCreationStateDto]) -> Result<()> {
        if states.is_empty() {
            return Ok(());
        }
        let mut conn = self.conn.lock();
        let tx = conn
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(db_err)?;
        {
            let mut stmt = tx
                .prepare(
                    "INSERT INTO user_creation_states_cache (creation_id, state_json, updated_at_ms)
                     VALUES (?1, ?2, ?3)
                     ON CONFLICT(creation_id) DO UPDATE SET
                        state_json = excluded.state_json,
                        updated_at_ms = excluded.updated_at_ms
                     WHERE excluded.updated_at_ms >= user_creation_states_cache.updated_at_ms",
                )
                .map_err(db_err)?;
            for state in states {
                let json = serde_json::to_string(state).map_err(json_cache_err)?;
                stmt.execute(params![state.creation_id, json, state.updated_at_ms])
                    .map_err(db_err)?;
            }
        }
        tx.commit().map_err(db_err)?;
        Ok(())
    }

    pub(crate) fn load_user_creation_states_map(
        &self,
    ) -> Result<HashMap<String, UserCreationStateDto>> {
        let conn = self.conn.lock();
        let mut stmt = conn
            .prepare("SELECT state_json FROM user_creation_states_cache")
            .map_err(db_err)?;
        let rows = stmt
            .query_map([], |r| r.get::<_, String>(0))
            .map_err(db_err)?;
        let mut out = HashMap::new();
        for row in rows {
            let json = row.map_err(db_err)?;
            if let Ok(state) = serde_json::from_str::<UserCreationStateDto>(&json) {
                out.insert(state.creation_id.clone(), state);
            }
        }
        Ok(out)
    }

    pub(crate) fn load_user_creation_states(&self) -> Result<Vec<UserCreationStateDto>> {
        let mut items: Vec<UserCreationStateDto> =
            self.load_user_creation_states_map()?.into_values().collect();
        sort_user_creation_states(&mut items);
        Ok(items)
    }

    pub(crate) fn get_user_creation_state(
        &self,
        creation_id: &str,
    ) -> Result<Option<UserCreationStateDto>> {
        let conn = self.conn.lock();
        let raw: Option<String> = conn
            .query_row(
                "SELECT state_json FROM user_creation_states_cache WHERE creation_id = ?1",
                params![creation_id],
                |r| r.get::<_, String>(0),
            )
            .optional()
            .map_err(db_err)?;
        Ok(raw.and_then(|json| serde_json::from_str::<UserCreationStateDto>(&json).ok()))
    }

    pub(crate) fn upsert_public_catalog_items(
        &self,
        items: &[PublicCatalogItemDto],
        cached_at_ms: i64,
    ) -> Result<()> {
        if items.is_empty() {
            return Ok(());
        }
        let mut conn = self.conn.lock();
        let tx = conn
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(db_err)?;
        {
            let mut stmt = tx
                .prepare(
                    "INSERT OR REPLACE INTO public_catalog_cache (creation_id, kind, payload_json, cached_at_ms)
                     VALUES (?1, ?2, ?3, ?4)",
                )
                .map_err(db_err)?;
            for item in items {
                let json = serde_json::to_string(item).map_err(json_cache_err)?;
                stmt.execute(params![
                    item.creation_id,
                    item.kind.as_str(),
                    json,
                    cached_at_ms
                ])
                .map_err(db_err)?;
            }
        }
        tx.commit().map_err(db_err)?;
        Ok(())
    }

    pub(crate) fn remove_public_catalog_item(&self, creation_id: &str) -> Result<bool> {
        let conn = self.conn.lock();
        let deleted = conn
            .execute(
                "DELETE FROM public_catalog_cache WHERE creation_id = ?1",
                params![creation_id],
            )
            .map_err(db_err)?;
        Ok(deleted > 0)
    }

    pub(crate) fn load_public_catalog_entries(
        &self,
    ) -> Result<HashMap<String, (PublicCatalogItemDto, i64)>> {
        let conn = self.conn.lock();
        let mut stmt = conn
            .prepare("SELECT payload_json, cached_at_ms FROM public_catalog_cache")
            .map_err(db_err)?;
        let rows = stmt
            .query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?)))
            .map_err(db_err)?;
        let mut out = HashMap::new();
        for row in rows {
            let (json, cached_at_ms) = row.map_err(db_err)?;
            if let Ok(item) = serde_json::from_str::<PublicCatalogItemDto>(&json) {
                out.insert(item.creation_id.clone(), (item, cached_at_ms));
            }
        }
        Ok(out)
    }

    pub(crate) fn load_public_catalog_items(
        &self,
        kind: Option<CreationKind>,
    ) -> Result<Vec<PublicCatalogItemDto>> {
        let conn = self.conn.lock();
        let kind_str = kind.map(CreationKind::as_str);
        let mut stmt = conn
            .prepare(
                "SELECT payload_json
                 FROM public_catalog_cache
                 WHERE (?1 IS NULL OR kind = ?1)
                 ORDER BY cached_at_ms DESC, creation_id ASC",
            )
            .map_err(db_err)?;
        let rows = stmt
            .query_map(params![kind_str], |r| r.get::<_, String>(0))
            .map_err(db_err)?;
        let mut out = Vec::new();
        for row in rows {
            let json = row.map_err(db_err)?;
            if let Ok(item) = serde_json::from_str::<PublicCatalogItemDto>(&json) {
                out.push(item);
            }
        }
        sort_public_catalog_items(&mut out);
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui_telegram_proto::creations::{
        BookChapter, BookPage, BookPayload, BookRichBlock, CoverArtwork, CreationPayload,
        CreationVisibility,
    };

    fn test_creation(id: &str, rev: i64, deleted_at_ms: Option<i64>) -> UserCreationDto {
        UserCreationDto {
            creation_id: id.to_string(),
            owner_user_id: 42,
            creator_bot_user_id: None,
            creator_chat_id: None,
            kind: CreationKind::Book,
            title: format!("Book {id} r{rev}"),
            subtitle: String::new(),
            summary: "Summary".to_string(),
            author_display_name: "Author".to_string(),
            language: "en".to_string(),
            genres: vec!["sci-fi".to_string()],
            tags: vec![],
            cover: CoverArtwork::default(),
            age_rating: "4+".to_string(),
            payload: CreationPayload::Book(BookPayload {
                author: "Author".to_string(),
                series_name: None,
                series_index: None,
                blurb: "Blurb".to_string(),
                estimated_read_mins: 5,
                word_count: 100,
                chapters: vec![BookChapter {
                    chapter_id: "c1".to_string(),
                    chapter_number: 1,
                    title: "Ch 1".to_string(),
                    subtitle: None,
                    pages: vec![BookPage {
                        page_number: 1,
                        blocks: vec![BookRichBlock::Paragraph {
                            text: "Sample paragraph content for sqlite cache unit test.".to_string(),
                        }],
                    }],
                }],
            }),
            version: 1,
            cloud_revision: rev,
            review_status: CreationReviewStatus::Draft,
            visibility: CreationVisibility::Private,
            has_live_published_version: false,
            published_version: None,
            rejection_reason: None,
            review_notes: None,
            submitted_at_ms: None,
            reviewed_at_ms: None,
            published_at_ms: None,
            created_at_ms: 100,
            updated_at_ms: rev * 1000,
            deleted_at_ms,
            rating_avg: 0.0,
            rating_count: 0,
            engagement_count: 0,
            save_or_install_count: 0,
        }
    }

    #[test]
    fn sqlite_cache_enforces_monotonic_revision_tombstone_state_guard_and_clear_all() {
        let db = SqliteClientCache::open_in_memory().unwrap();
        assert_eq!(db.creations_last_revision().unwrap(), 0);

        db.save_creations_last_revision(7).unwrap();
        assert_eq!(db.creations_last_revision().unwrap(), 7);
        db.save_creations_last_revision(3).unwrap();
        assert_eq!(db.creations_last_revision().unwrap(), 7);

        let rev = db
            .apply_creations_sync_delta(8, &[test_creation("cr_a", 8, None)])
            .unwrap();
        assert_eq!(rev, 8);
        assert_eq!(db.load_user_creations(None, None).unwrap().len(), 1);

        // Tombstone at rev 9, then attempt stale resurrection at rev 8 and same-rev 9 non-deleted
        db.apply_creations_sync_delta(9, &[test_creation("cr_a", 9, Some(9_000))])
            .unwrap();
        assert!(db.get_user_creation("cr_a").unwrap().is_none());
        db.upsert_user_creations(&[
            test_creation("cr_a", 8, None),
            test_creation("cr_a", 9, None),
        ])
        .unwrap();
        assert!(db.get_user_creation("cr_a").unwrap().is_none());

        // User creation state stale-timestamp guard
        let newer_state = UserCreationStateDto {
            creation_id: "cr_a".to_string(),
            user_id: 42,
            kind: CreationKind::Book,
            in_library: true,
            favorite: true,
            progress_percent: 80.0,
            state_json: serde_json::json!({"page": 8}),
            updated_at_ms: 5_000,
        };
        let stale_state = UserCreationStateDto {
            progress_percent: 10.0,
            state_json: serde_json::json!({"page": 1}),
            updated_at_ms: 4_000,
            ..newer_state.clone()
        };
        db.upsert_user_creation_state(&newer_state).unwrap();
        db.upsert_user_creation_state(&stale_state).unwrap();
        let loaded = db.get_user_creation_state("cr_a").unwrap().unwrap();
        assert_eq!(loaded.progress_percent, 80.0);

        // Atomic clear_all wipes creations, states, catalog, and creations_last_revision
        db.clear_all();
        assert_eq!(db.creations_last_revision().unwrap(), 0);
        assert!(db.load_all_user_creations().unwrap().is_empty());
        assert!(db.load_user_creation_states().unwrap().is_empty());
        assert!(db.load_public_catalog_items(None).unwrap().is_empty());
    }
}

