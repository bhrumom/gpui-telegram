//! SQLite WAL Bot Items store (`items`, `item_links`, `item_events`) with forward-only
//! `PRAGMA user_version` read-only downgrade protection, 6-state machine, CAS versioning,
//! owner-cancel protection, snooze/stale lifecycle, multi-decision `--ask` quick-replies,
//! priority-folded `"Needs you"` inbox, and automatic closed-item monthly journal append.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use chrono::{DateTime, Utc};
use orca_core::bot_items::{
    AskSelection, BOT_ITEM_LIMITS, BotItem, BotItemActor, BotItemAsk, BotItemEvent,
    BotItemEventData, BotItemEventKind, BotItemLink, BotItemLinkKind, BotItemLinkMeta,
    BotItemMailState, BotItemStatus, BotItemsStale, ItemCommentRequest, ItemTransitionOutcome,
    NEEDS_YOU_FOLD_CAP, NeedsYouFoldResult, StatusTransitionRequest, add_item_link,
    apply_item_comment, apply_snooze, apply_status_transition, apply_unsnooze,
    bot_item_links_one_per_chain, check_items_schema_version, check_item_write_permission,
    filter_and_sort_needs_you_items, format_ask_reply, is_item_stale, is_snooze_active,
    needs_you_fold, normalize_list_page_limit, was_last_canceled_by_owner,
};
use orca_core::bot_memory::{
    JournalAsker, JournalCloseAttribution, JournalCloseStatus, JournalEntryInput,
};
use orca_core::bot_types::BotFolderSlug;
use orca_core::{BotError, BotErrorCode};
use rusqlite::{Connection, OpenFlags, OptionalExtension, params};
use uuid::Uuid;

use crate::memory_service::BotMemoryService;
use crate::{OrcaBotsError, Result};

/// Current forward-only SQLite schema version (`PRAGMA user_version`).
pub const ITEMS_SCHEMA_VERSION: u32 = 1;

const ITEMS_SCHEMA_V1_SQL: &str = "
CREATE TABLE IF NOT EXISTS items (
    uid TEXT PRIMARY KEY NOT NULL,
    number INTEGER UNIQUE NOT NULL,
    owner_bot_id TEXT NOT NULL,
    chat_id TEXT,
    group_id TEXT,
    source TEXT,
    made_by_json TEXT,
    title TEXT NOT NULL,
    description TEXT,
    done_means TEXT,
    status TEXT NOT NULL,
    question_event INTEGER,
    ask_json TEXT,
    wait_event INTEGER,
    wait_note TEXT,
    close_note TEXT,
    version INTEGER NOT NULL,
    last_event_id INTEGER NOT NULL,
    others_last_event_id INTEGER,
    snoozed_until_ms INTEGER,
    last_activity_at_ms INTEGER NOT NULL,
    created_at_ms INTEGER NOT NULL,
    updated_at_ms INTEGER NOT NULL,
    closed_at_ms INTEGER
);
CREATE INDEX IF NOT EXISTS idx_items_owner_status ON items(owner_bot_id, status);
CREATE INDEX IF NOT EXISTS idx_items_number ON items(number);

CREATE TABLE IF NOT EXISTS item_links (
    item_uid TEXT NOT NULL,
    kind TEXT NOT NULL,
    ref TEXT NOT NULL,
    added_event INTEGER NOT NULL,
    meta_json TEXT NOT NULL,
    PRIMARY KEY (item_uid, kind, ref),
    FOREIGN KEY (item_uid) REFERENCES items(uid) ON DELETE CASCADE
);
CREATE INDEX IF NOT EXISTS idx_item_links_uid ON item_links(item_uid);

CREATE TABLE IF NOT EXISTS item_events (
    item_uid TEXT NOT NULL,
    id INTEGER NOT NULL,
    kind TEXT NOT NULL,
    actor_json TEXT NOT NULL,
    data_json TEXT NOT NULL,
    mail TEXT,
    action TEXT,
    created_at_ms INTEGER NOT NULL,
    PRIMARY KEY (item_uid, id),
    FOREIGN KEY (item_uid) REFERENCES items(uid) ON DELETE CASCADE
);
CREATE INDEX IF NOT EXISTS idx_item_events_mail ON item_events(mail) WHERE mail = 'owed';
";

/// Input payload for creating a new Bot Item.
#[derive(Debug, Clone)]
pub struct CreateBotItemRequest {
    pub owner_bot_id: String,
    pub actor: BotItemActor,
    pub title: String,
    pub description: Option<String>,
    pub done_means: Option<String>,
    pub status: BotItemStatus,
    pub ask: Option<BotItemAsk>,
    pub wait_note: Option<String>,
    pub chat_id: Option<String>,
    pub group_id: Option<String>,
    pub source: Option<String>,
    pub made_by: Option<BotItemActor>,
    pub links: Vec<BotItemLink>,
}

impl CreateBotItemRequest {
    pub fn new(
        owner_bot_id: impl Into<String>,
        actor: BotItemActor,
        title: impl Into<String>,
    ) -> Self {
        Self {
            owner_bot_id: owner_bot_id.into(),
            actor,
            title: title.into(),
            description: None,
            done_means: None,
            status: BotItemStatus::Planned,
            ask: None,
            wait_note: None,
            chat_id: None,
            group_id: None,
            source: None,
            made_by: None,
            links: Vec::new(),
        }
    }
}

/// Filter options for listing Bot Items.
#[derive(Debug, Clone, Default)]
pub struct ListBotItemsFilter {
    pub owner_bot_id: Option<String>,
    pub statuses: Option<Vec<BotItemStatus>>,
    pub include_closed: bool,
    pub include_snoozed: bool,
    pub now_ms: Option<i64>,
    pub limit: Option<usize>,
    pub offset: usize,
}

/// Patch request for editing an item's title, description, or `done_means`.
#[derive(Debug, Clone)]
pub struct UpdateBotItemFieldsRequest {
    pub item_uid: String,
    pub actor: BotItemActor,
    pub expected_version: Option<u64>,
    pub title: Option<String>,
    pub description: Option<Option<String>>,
    pub done_means: Option<Option<String>>,
}

/// Quick-reply answer request for an open `NeedsYou` question.
#[derive(Debug, Clone)]
pub struct AnswerQuestionRequest {
    pub actor: BotItemActor,
    pub question_event: u64,
    pub selections: Option<Vec<AskSelection>>,
    pub reply_text: Option<String>,
    pub files: Vec<String>,
}

/// Explicit journal target passed when closing an item (`Done` / `Canceled`).
#[derive(Debug, Clone)]
pub struct ItemCloseJournalTarget<'a> {
    pub memory_service: &'a BotMemoryService,
    pub folder: &'a BotFolderSlug,
    pub attribution: Option<JournalCloseAttribution>,
}

struct ItemsDbInner {
    conn: Connection,
    read_only: bool,
    schema_version: u32,
    memory_service: Option<BotMemoryService>,
    bot_folders: HashMap<String, BotFolderSlug>,
}

/// Persistent SQLite WAL Bot Items store.
#[derive(Clone)]
pub struct BotItemsStore {
    db_path: Option<PathBuf>,
    inner: Arc<Mutex<ItemsDbInner>>,
}

impl BotItemsStore {
    /// Opens a persistent SQLite WAL Bot Items database at `db_path`.
    /// If the on-disk `PRAGMA user_version` is strictly newer than `ITEMS_SCHEMA_VERSION`,
    /// opens strictly read-only (`is_read_only() == true`) so older builds never corrupt data.
    pub fn open(db_path: impl Into<PathBuf>) -> Result<Self> {
        let path = db_path.into();
        if let Some(parent) = path.parent()
            && !parent.as_os_str().is_empty()
        {
            std::fs::create_dir_all(parent)?;
        }

        if path.exists() {
            let probe = Connection::open_with_flags(&path, OpenFlags::SQLITE_OPEN_READ_ONLY)?;
            let existing_version = read_user_version(&probe)?;
            if existing_version > ITEMS_SCHEMA_VERSION {
                return Ok(Self {
                    db_path: Some(path),
                    inner: Arc::new(Mutex::new(ItemsDbInner {
                        conn: probe,
                        read_only: true,
                        schema_version: existing_version,
                        memory_service: None,
                        bot_folders: HashMap::new(),
                    })),
                });
            }
            drop(probe);
        }

        let conn = Connection::open_with_flags(
            &path,
            OpenFlags::SQLITE_OPEN_READ_WRITE | OpenFlags::SQLITE_OPEN_CREATE,
        )?;
        initialize_writable_db(&conn)?;
        let schema_version = read_user_version(&conn)?;

        Ok(Self {
            db_path: Some(path),
            inner: Arc::new(Mutex::new(ItemsDbInner {
                conn,
                read_only: false,
                schema_version,
                memory_service: None,
                bot_folders: HashMap::new(),
            })),
        })
    }

    /// Opens an in-memory SQLite Bot Items store (useful for ephemeral runs & unit tests).
    pub fn open_in_memory() -> Result<Self> {
        let conn = Connection::open_in_memory()?;
        initialize_writable_db(&conn)?;
        let schema_version = read_user_version(&conn)?;
        Ok(Self {
            db_path: None,
            inner: Arc::new(Mutex::new(ItemsDbInner {
                conn,
                read_only: false,
                schema_version,
                memory_service: None,
                bot_folders: HashMap::new(),
            })),
        })
    }

    /// Attaches a `BotMemoryService` so closing items automatically appends deterministic
    /// monthly journal lines for registered bot folders.
    pub fn set_memory_service(&self, service: BotMemoryService) -> Result<()> {
        let mut guard = self.lock_inner()?;
        guard.memory_service = Some(service);
        Ok(())
    }

    /// Registers a bot's home folder slug for automatic closed-item journal appends.
    pub fn register_bot_folder(&self, bot_id: impl Into<String>, folder: BotFolderSlug) -> Result<()> {
        let mut guard = self.lock_inner()?;
        guard.bot_folders.insert(bot_id.into(), folder);
        Ok(())
    }

    /// Returns the filesystem path of the SQLite DB, if file-backed.
    pub fn db_path(&self) -> Option<&Path> {
        self.db_path.as_deref()
    }

    /// Returns `true` when the database was opened read-only due to a newer `PRAGMA user_version`.
    pub fn is_read_only(&self) -> Result<bool> {
        Ok(self.lock_inner()?.read_only)
    }

    /// Returns the observed SQLite `PRAGMA user_version`.
    pub fn schema_version(&self) -> Result<u32> {
        Ok(self.lock_inner()?.schema_version)
    }

    fn lock_inner(&self) -> Result<std::sync::MutexGuard<'_, ItemsDbInner>> {
        self.inner.lock().map_err(|_| {
            OrcaBotsError::bot(BotErrorCode::ItemsUnavailable, "BotItemsStore mutex poisoned.")
        })
    }

    /// Creates a new Bot Item with monotonic `number`, validates `BOT_ITEM_LIMITS`, persists
    /// initial links and `Created` event atomically in SQLite.
    pub fn create_item(
        &self,
        req: CreateBotItemRequest,
        now_ms: i64,
    ) -> Result<(BotItem, BotItemEvent)> {
        let mut guard = self.lock_inner()?;
        ensure_writable(&guard)?;

        if req.owner_bot_id.trim().is_empty() {
            return Err(OrcaBotsError::bot(
                BotErrorCode::InvalidArgument,
                "owner_bot_id cannot be empty.",
            ));
        }
        if let Some(actor_bot) = req.actor.bot_id()
            && actor_bot != req.owner_bot_id
        {
            return Err(OrcaBotsError::bot(
                BotErrorCode::NotOwner,
                format!(
                    "Bot '{actor_bot}' cannot create item owned by '{}'.",
                    req.owner_bot_id
                ),
            ));
        }

        let tx = guard.conn.transaction()?;
        let next_num: i64 = tx.query_row(
            "SELECT COALESCE(MAX(number), 0) + 1 FROM items",
            [],
            |row| row.get(0),
        )?;
        let number = u64::try_from(next_num).map_err(|_| {
            OrcaBotsError::bot(BotErrorCode::ItemsUnavailable, "Item number overflow.")
        })?;

        let uid = Uuid::new_v4().to_string();
        let initial_event_id = 1u64;
        let mut links: Vec<BotItemLink> = Vec::new();
        for mut link in req.links {
            if link.added_event == 0 {
                link.added_event = initial_event_id;
            }
            add_item_link(&mut links, link)?;
        }

        let item = BotItem {
            uid: uid.clone(),
            number,
            owner_bot_id: req.owner_bot_id,
            chat_id: req.chat_id,
            group_id: req.group_id,
            source: req.source,
            made_by: req.made_by.or_else(|| Some(req.actor.clone())),
            title: req.title.trim().to_string(),
            description: req.description,
            done_means: req.done_means,
            status: req.status,
            question_event: if req.status == BotItemStatus::NeedsYou {
                Some(initial_event_id)
            } else {
                None
            },
            ask: if req.status == BotItemStatus::NeedsYou {
                req.ask
            } else {
                None
            },
            wait_event: if req.status == BotItemStatus::Waiting {
                Some(initial_event_id)
            } else {
                None
            },
            wait_note: if req.status == BotItemStatus::Waiting {
                req.wait_note
            } else {
                None
            },
            close_note: None,
            version: 1,
            last_event_id: initial_event_id,
            others_last_event_id: None,
            snoozed_until_ms: None,
            last_activity_at_ms: now_ms,
            created_at_ms: now_ms,
            updated_at_ms: now_ms,
            closed_at_ms: if req.status.is_closed() {
                Some(now_ms)
            } else {
                None
            },
            links,
        };
        item.validate()?;

        let created_event = BotItemEvent {
            id: initial_event_id,
            item_uid: uid,
            kind: BotItemEventKind::Created,
            actor: req.actor,
            data: BotItemEventData::Created,
            mail: None,
            action: None,
            created_at_ms: now_ms,
        };

        insert_item_row(&tx, &item)?;
        for link in &item.links {
            upsert_item_link_row(&tx, &item.uid, link)?;
        }
        insert_item_event_row(&tx, &created_event)?;
        tx.commit()?;
        Ok((item, created_event))
    }

    /// Fetches an item by its `uid`, failing with `BotErrorCode::NotFound` if absent.
    pub fn get_item_by_uid(&self, uid: &str) -> Result<BotItem> {
        let guard = self.lock_inner()?;
        load_item_by_uid(&guard.conn, uid)
    }

    /// Fetches an item by its monotonic `number`, failing with `BotErrorCode::NotFound` if absent.
    pub fn get_item_by_number(&self, number: u64) -> Result<BotItem> {
        let guard = self.lock_inner()?;
        let num_i64 = i64::try_from(number).map_err(|_| {
            OrcaBotsError::bot(BotErrorCode::InvalidArgument, "Invalid item number.")
        })?;
        let uid: Option<String> = guard
            .conn
            .query_row(
                "SELECT uid FROM items WHERE number = ?1",
                params![num_i64],
                |row| row.get(0),
            )
            .optional()?;
        let uid = uid.ok_or_else(|| {
            OrcaBotsError::bot(BotErrorCode::NotFound, format!("Item #{number} not found."))
        })?;
        load_item_by_uid(&guard.conn, &uid)
    }

    /// Lists items matching `filter` with pagination bounds from `BOT_ITEM_LIMITS`.
    pub fn list_items(&self, filter: ListBotItemsFilter) -> Result<Vec<BotItem>> {
        let guard = self.lock_inner()?;
        let limit = normalize_list_page_limit(filter.limit);
        let mut all = load_all_items(&guard.conn, filter.owner_bot_id.as_deref())?;

        if let Some(ref statuses) = filter.statuses {
            all.retain(|it| statuses.contains(&it.status));
        } else if !filter.include_closed {
            all.retain(|it| it.status.is_open());
        }

        if !filter.include_snoozed
            && let Some(now_ms) = filter.now_ms
        {
            all.retain(|it| !is_snooze_active(it.snoozed_until_ms, now_ms));
        }

        Ok(all.into_iter().skip(filter.offset).take(limit).collect())
    }

    /// Lists events for `item_uid`, ordered by ascending `id`.
    pub fn list_events(
        &self,
        item_uid: &str,
        limit: Option<usize>,
        after_id: Option<u64>,
    ) -> Result<Vec<BotItemEvent>> {
        let guard = self.lock_inner()?;
        let _ = load_item_by_uid(&guard.conn, item_uid)?;
        let cap = limit
            .unwrap_or(BOT_ITEM_LIMITS.events_page)
            .clamp(1, BOT_ITEM_LIMITS.list_page_max);
        let events = load_item_events(&guard.conn, item_uid)?;
        Ok(events
            .into_iter()
            .filter(|ev| after_id.is_none_or(|a| ev.id > a))
            .take(cap)
            .collect())
    }

    /// Updates mutable metadata fields (`title`, `description`, `done_means`) with CAS versioning
    /// and owner-cancel protection.
    pub fn update_item_fields(
        &self,
        req: UpdateBotItemFieldsRequest,
        now_ms: i64,
    ) -> Result<(BotItem, BotItemEvent)> {
        let mut guard = self.lock_inner()?;
        ensure_writable(&guard)?;
        let tx = guard.conn.transaction()?;
        let mut item = load_item_by_uid(&tx, &req.item_uid)?;
        let history = load_item_events(&tx, &req.item_uid)?;

        check_item_write_permission(&item, &req.actor)?;
        if let Some(exp) = req.expected_version
            && exp != item.version
        {
            return Err(OrcaBotsError::Bot(BotError::new(
                BotErrorCode::ItemChanged,
                format!(
                    "Item #{} changed concurrently (expected version {exp}, current {}).",
                    item.number, item.version
                ),
            )));
        }
        if item.status == BotItemStatus::Canceled
            && !req.actor.is_owner()
            && was_last_canceled_by_owner(&history)
        {
            return Err(OrcaBotsError::Bot(BotError::new(
                BotErrorCode::ItemClosed,
                format!("Item #{} was canceled by the owner.", item.number),
            )));
        }

        let mut fields = Vec::new();
        if let Some(title) = req.title {
            item.title = title.trim().to_string();
            fields.push("title".to_string());
        }
        if let Some(desc) = req.description {
            item.description = desc;
            fields.push("description".to_string());
        }
        if let Some(done_means) = req.done_means {
            item.done_means = done_means;
            fields.push("done_means".to_string());
        }
        if fields.is_empty() {
            return Err(OrcaBotsError::bot(
                BotErrorCode::InvalidArgument,
                "At least one field must be provided to update.",
            ));
        }

        item.version = item.version.saturating_add(1);
        let event_id = item.last_event_id.saturating_add(1);
        item.last_event_id = event_id;
        item.updated_at_ms = now_ms;
        item.last_activity_at_ms = now_ms;
        item.validate()?;

        let event = BotItemEvent {
            id: event_id,
            item_uid: item.uid.clone(),
            kind: BotItemEventKind::Updated,
            actor: req.actor,
            data: BotItemEventData::Updated { fields },
            mail: None,
            action: None,
            created_at_ms: now_ms,
        };

        update_item_row(&tx, &item)?;
        insert_item_event_row(&tx, &event)?;
        tx.commit()?;
        Ok((item, event))
    }

    /// Transitions an item's status using `orca_core::bot_items::apply_status_transition`.
    /// When transitioning to `Done` or `Canceled`, automatically appends a deterministic
    /// `memory/journal/YYYY-MM.md` entry if a `BotMemoryService` and bot folder are registered.
    pub fn set_status(
        &self,
        item_uid: &str,
        req: StatusTransitionRequest,
        now_ms: i64,
    ) -> Result<(BotItem, ItemTransitionOutcome)> {
        self.set_status_with_journal(item_uid, req, now_ms, None)
    }

    /// Transitions an item's status and optionally appends to a specified closed-item journal target.
    pub fn set_status_with_journal(
        &self,
        item_uid: &str,
        req: StatusTransitionRequest,
        now_ms: i64,
        explicit_journal: Option<&ItemCloseJournalTarget<'_>>,
    ) -> Result<(BotItem, ItemTransitionOutcome)> {
        let (item, outcome, registered_journal) = {
            let mut guard = self.lock_inner()?;
            ensure_writable(&guard)?;
            let tx = guard.conn.transaction()?;
            let mut item = load_item_by_uid(&tx, item_uid)?;
            let history = load_item_events(&tx, item_uid)?;

            let outcome = apply_status_transition(&mut item, &history, req, now_ms)?;
            update_item_row(&tx, &item)?;
            for ev in &outcome.emitted_events {
                insert_item_event_row(&tx, ev)?;
            }
            tx.commit()?;

            let reg = if explicit_journal.is_none() && item.status.is_closed() {
                match (&guard.memory_service, guard.bot_folders.get(&item.owner_bot_id)) {
                    (Some(svc), Some(folder)) => Some((svc.clone(), folder.clone())),
                    _ => None,
                }
            } else {
                None
            };
            (item, outcome, reg)
        };

        if item.status.is_closed() {
            if let Some(target) = explicit_journal {
                let entry = build_closed_journal_entry(&item, now_ms, target.attribution.clone());
                target
                    .memory_service
                    .append_closed_item_journal(target.folder, &item.owner_bot_id, &entry)?;
            } else if let Some((svc, folder)) = registered_journal {
                let entry = build_closed_journal_entry(&item, now_ms, None);
                svc.append_closed_item_journal(&folder, &item.owner_bot_id, &entry)?;
            }
        }

        Ok((item, outcome))
    }

    /// Appends a comment event to an item via `orca_core::bot_items::apply_item_comment`.
    pub fn comment_item(
        &self,
        item_uid: &str,
        req: ItemCommentRequest,
        now_ms: i64,
    ) -> Result<(BotItem, BotItemEvent)> {
        let mut guard = self.lock_inner()?;
        ensure_writable(&guard)?;
        let tx = guard.conn.transaction()?;
        let mut item = load_item_by_uid(&tx, item_uid)?;
        let event = apply_item_comment(&mut item, req, now_ms)?;
        update_item_row(&tx, &item)?;
        insert_item_event_row(&tx, &event)?;
        tx.commit()?;
        Ok((item, event))
    }

    /// Handles an owner's quick-reply or free-text answer to an open `NeedsYou` question:
    /// validates the open question (`QuestionClosed` otherwise), formats `--ask/--option/--multi`
    /// selections if provided, posts a `Commented` event with `mail = 'owed'`, and transitions
    /// the item back to `Working`.
    pub fn answer_question(
        &self,
        item_uid: &str,
        req: AnswerQuestionRequest,
        now_ms: i64,
    ) -> Result<(BotItem, BotItemEvent)> {
        let mut guard = self.lock_inner()?;
        ensure_writable(&guard)?;
        let tx = guard.conn.transaction()?;
        let mut item = load_item_by_uid(&tx, item_uid)?;

        if item.status != BotItemStatus::NeedsYou || item.question_event != Some(req.question_event)
        {
            return Err(OrcaBotsError::Bot(BotError::new(
                BotErrorCode::QuestionClosed,
                format!(
                    "Question event {} on item #{} is already closed.",
                    req.question_event, item.number
                ),
            )));
        }

        let text = match (req.selections, req.reply_text) {
            (Some(selections), _) => {
                let ask = item.ask.as_ref().ok_or_else(|| {
                    OrcaBotsError::bot(
                        BotErrorCode::InvalidArgument,
                        "Item has no structured ask payload for quick-reply selections.",
                    )
                })?;
                format_ask_reply(ask, &selections)?
            }
            (None, Some(reply)) => reply,
            (None, None) => {
                return Err(OrcaBotsError::bot(
                    BotErrorCode::InvalidArgument,
                    "Either selections or reply_text must be provided to answer a question.",
                ));
            }
        };

        let event = apply_item_comment(
            &mut item,
            ItemCommentRequest {
                actor: req.actor,
                text,
                files: req.files,
                question_event: Some(req.question_event),
            },
            now_ms,
        )?;
        update_item_row(&tx, &item)?;
        insert_item_event_row(&tx, &event)?;
        tx.commit()?;
        Ok((item, event))
    }

    /// Snoozes an open item until `until_ms`.
    pub fn snooze_item(
        &self,
        item_uid: &str,
        actor: BotItemActor,
        until_ms: i64,
        now_ms: i64,
    ) -> Result<(BotItem, BotItemEvent)> {
        let mut guard = self.lock_inner()?;
        ensure_writable(&guard)?;
        let tx = guard.conn.transaction()?;
        let mut item = load_item_by_uid(&tx, item_uid)?;
        let event = apply_snooze(&mut item, actor, until_ms, now_ms)?;
        update_item_row(&tx, &item)?;
        insert_item_event_row(&tx, &event)?;
        tx.commit()?;
        Ok((item, event))
    }

    /// Unsnoozes an item, recording `early: true` if snooze had not yet expired.
    pub fn unsnooze_item(
        &self,
        item_uid: &str,
        actor: BotItemActor,
        now_ms: i64,
    ) -> Result<(BotItem, BotItemEvent)> {
        let mut guard = self.lock_inner()?;
        ensure_writable(&guard)?;
        let tx = guard.conn.transaction()?;
        let mut item = load_item_by_uid(&tx, item_uid)?;
        let event = apply_unsnooze(&mut item, actor, now_ms)?;
        update_item_row(&tx, &item)?;
        insert_item_event_row(&tx, &event)?;
        tx.commit()?;
        Ok((item, event))
    }

    /// Adds or updates a typed link (cap 1,000) and appends a `Linked` event.
    pub fn add_link(
        &self,
        item_uid: &str,
        actor: BotItemActor,
        mut link: BotItemLink,
        now_ms: i64,
    ) -> Result<(BotItem, BotItemEvent)> {
        let mut guard = self.lock_inner()?;
        ensure_writable(&guard)?;
        let tx = guard.conn.transaction()?;
        let mut item = load_item_by_uid(&tx, item_uid)?;
        check_item_write_permission(&item, &actor)?;

        let event_id = item.last_event_id.saturating_add(1);
        link.added_event = event_id;
        let kind = link.kind;
        let reference = link.reference.clone();
        add_item_link(&mut item.links, link.clone())?;

        item.last_event_id = event_id;
        item.updated_at_ms = now_ms;
        item.last_activity_at_ms = now_ms;

        let event = BotItemEvent {
            id: event_id,
            item_uid: item.uid.clone(),
            kind: BotItemEventKind::Linked,
            actor,
            data: BotItemEventData::Linked { kind, reference },
            mail: None,
            action: None,
            created_at_ms: now_ms,
        };

        upsert_item_link_row(&tx, &item.uid, &link)?;
        update_item_row(&tx, &item)?;
        insert_item_event_row(&tx, &event)?;
        tx.commit()?;
        Ok((item, event))
    }

    /// Returns lineage-deduplicated links (`one_per_chain`) for `item_uid`.
    pub fn links_one_per_chain(&self, item_uid: &str) -> Result<Vec<BotItemLink>> {
        let item = self.get_item_by_uid(item_uid)?;
        Ok(bot_item_links_one_per_chain(&item.links))
    }

    /// Lists events with pending `mail = 'owed'` outbox delivery.
    pub fn list_owed_mail_events(&self, owner_bot_id: Option<&str>) -> Result<Vec<BotItemEvent>> {
        let guard = self.lock_inner()?;
        let mut out = Vec::new();
        if let Some(bot_id) = owner_bot_id {
            let mut stmt = guard.conn.prepare(
                "SELECT e.item_uid, e.id, e.kind, e.actor_json, e.data_json, e.mail, e.action, e.created_at_ms
                 FROM item_events e
                 INNER JOIN items i ON i.uid = e.item_uid
                 WHERE e.mail = 'owed' AND i.owner_bot_id = ?1
                 ORDER BY e.created_at_ms ASC, e.id ASC",
            )?;
            let rows = stmt.query_map(params![bot_id], row_to_event)?;
            for r in rows {
                out.push(r??);
            }
        } else {
            let mut stmt = guard.conn.prepare(
                "SELECT item_uid, id, kind, actor_json, data_json, mail, action, created_at_ms
                 FROM item_events
                 WHERE mail = 'owed'
                 ORDER BY created_at_ms ASC, id ASC",
            )?;
            let rows = stmt.query_map([], row_to_event)?;
            for r in rows {
                out.push(r??);
            }
        }
        Ok(out)
    }

    /// Marks an `owed` outbox event as `sent`.
    pub fn mark_mail_sent(&self, item_uid: &str, event_id: u64) -> Result<()> {
        let guard = self.lock_inner()?;
        ensure_writable(&guard)?;
        let eid = i64::try_from(event_id).map_err(|_| {
            OrcaBotsError::bot(BotErrorCode::InvalidArgument, "Invalid event_id.")
        })?;
        guard.conn.execute(
            "UPDATE item_events SET mail = 'sent' WHERE item_uid = ?1 AND id = ?2 AND mail = 'owed'",
            params![item_uid, eid],
        )?;
        Ok(())
    }

    /// Checks whether `item` is stale under `policy` (3x longer for `Planned`).
    pub fn is_stale(&self, item: &BotItem, policy: BotItemsStale, now_ms: i64) -> bool {
        is_item_stale(item, policy, now_ms)
    }

    /// Returns all open items that are currently stale under `policy`.
    pub fn stale_items(
        &self,
        owner_bot_id: Option<&str>,
        policy: BotItemsStale,
        now_ms: i64,
    ) -> Result<Vec<BotItem>> {
        let guard = self.lock_inner()?;
        let items = load_all_items(&guard.conn, owner_bot_id)?;
        Ok(items
            .into_iter()
            .filter(|it| is_item_stale(it, policy, now_ms))
            .collect())
    }

    /// Builds the priority-folded `"Needs you"` inbox using `orca_core::bot_items::needs_you_fold`.
    pub fn needs_you_inbox(
        &self,
        owner_bot_id: Option<&str>,
        focused_uid: Option<&str>,
        cap: Option<usize>,
        now_ms: i64,
    ) -> Result<NeedsYouFoldResult<BotItem>> {
        self.needs_you_inbox_with_predicate(owner_bot_id, focused_uid, cap, now_ms, |it| {
            it.status == BotItemStatus::NeedsYou && it.ask.is_some()
        })
    }

    /// Builds the priority-folded `"Needs you"` inbox with a custom approval classifier.
    pub fn needs_you_inbox_with_predicate<F>(
        &self,
        owner_bot_id: Option<&str>,
        focused_uid: Option<&str>,
        cap: Option<usize>,
        now_ms: i64,
        is_approval: F,
    ) -> Result<NeedsYouFoldResult<BotItem>>
    where
        F: Fn(&BotItem) -> bool,
    {
        let guard = self.lock_inner()?;
        let all = load_all_items(&guard.conn, owner_bot_id)?;
        let sorted: Vec<BotItem> = filter_and_sort_needs_you_items(&all, now_ms)
            .into_iter()
            .cloned()
            .collect();
        Ok(needs_you_fold(
            &sorted,
            focused_uid,
            cap.unwrap_or(NEEDS_YOU_FOLD_CAP),
            |it| it.uid.as_str(),
            is_approval,
        ))
    }
}

fn read_user_version(conn: &Connection) -> Result<u32> {
    let ver: i64 = conn.pragma_query_value(None, "user_version", |row| row.get(0))?;
    u32::try_from(ver).map_err(|_| {
        OrcaBotsError::bot(BotErrorCode::ItemsUnavailable, "Negative SQLite user_version.")
    })
}

fn initialize_writable_db(conn: &Connection) -> Result<()> {
    conn.execute_batch(
        "PRAGMA journal_mode = WAL;
         PRAGMA foreign_keys = ON;
         PRAGMA busy_timeout = 5000;",
    )?;
    let ver = read_user_version(conn)?;
    check_items_schema_version(ver, ITEMS_SCHEMA_VERSION)?;
    if ver < ITEMS_SCHEMA_VERSION {
        conn.execute_batch(&format!(
            "BEGIN IMMEDIATE;
             {ITEMS_SCHEMA_V1_SQL}
             PRAGMA user_version = {ITEMS_SCHEMA_VERSION};
             COMMIT;"
        ))?;
    }
    Ok(())
}

fn ensure_writable(inner: &ItemsDbInner) -> Result<()> {
    if inner.read_only {
        return Err(OrcaBotsError::Bot(BotError::new(
            BotErrorCode::ItemsReadOnly,
            format!(
                "Bot items database schema v{} is newer than supported v{}; opened read-only.",
                inner.schema_version, ITEMS_SCHEMA_VERSION
            ),
        )));
    }
    check_items_schema_version(inner.schema_version, ITEMS_SCHEMA_VERSION)?;
    Ok(())
}

fn to_sql_i64(val: u64) -> Result<i64> {
    i64::try_from(val).map_err(|_| {
        OrcaBotsError::bot(BotErrorCode::InvalidArgument, "Integer exceeds SQLite i64 range.")
    })
}

fn from_sql_u64(val: i64) -> Result<u64> {
    u64::try_from(val).map_err(|_| {
        OrcaBotsError::bot(BotErrorCode::ItemsUnavailable, "Negative integer in items DB.")
    })
}

fn event_kind_to_str(kind: BotItemEventKind) -> &'static str {
    match kind {
        BotItemEventKind::Created => "created",
        BotItemEventKind::StatusChanged => "status_changed",
        BotItemEventKind::Commented => "commented",
        BotItemEventKind::Linked => "linked",
        BotItemEventKind::Unlinked => "unlinked",
        BotItemEventKind::Snoozed => "snoozed",
        BotItemEventKind::Unsnoozed => "unsnoozed",
        BotItemEventKind::Updated => "updated",
    }
}

fn parse_event_kind(raw: &str) -> Result<BotItemEventKind> {
    match raw {
        "created" => Ok(BotItemEventKind::Created),
        "status_changed" => Ok(BotItemEventKind::StatusChanged),
        "commented" => Ok(BotItemEventKind::Commented),
        "linked" => Ok(BotItemEventKind::Linked),
        "unlinked" => Ok(BotItemEventKind::Unlinked),
        "snoozed" => Ok(BotItemEventKind::Snoozed),
        "unsnoozed" => Ok(BotItemEventKind::Unsnoozed),
        "updated" => Ok(BotItemEventKind::Updated),
        other => Err(OrcaBotsError::bot(
            BotErrorCode::ItemsUnavailable,
            format!("Unknown item event kind '{other}'."),
        )),
    }
}

fn mail_state_to_str(mail: Option<BotItemMailState>) -> Option<&'static str> {
    match mail {
        Some(BotItemMailState::Owed) => Some("owed"),
        Some(BotItemMailState::Sent) => Some("sent"),
        None => None,
    }
}

fn parse_mail_state(raw: Option<String>) -> Result<Option<BotItemMailState>> {
    match raw.as_deref() {
        None => Ok(None),
        Some("owed") => Ok(Some(BotItemMailState::Owed)),
        Some("sent") => Ok(Some(BotItemMailState::Sent)),
        Some(other) => Err(OrcaBotsError::bot(
            BotErrorCode::ItemsUnavailable,
            format!("Unknown item mail state '{other}'."),
        )),
    }
}

fn insert_item_row(conn: &Connection, item: &BotItem) -> Result<()> {
    let made_by_json = item
        .made_by
        .as_ref()
        .map(serde_json::to_string)
        .transpose()?;
    let ask_json = item.ask.as_ref().map(serde_json::to_string).transpose()?;
    conn.execute(
        "INSERT INTO items (
            uid, number, owner_bot_id, chat_id, group_id, source, made_by_json,
            title, description, done_means, status, question_event, ask_json,
            wait_event, wait_note, close_note, version, last_event_id,
            others_last_event_id, snoozed_until_ms, last_activity_at_ms,
            created_at_ms, updated_at_ms, closed_at_ms
        ) VALUES (
            ?1, ?2, ?3, ?4, ?5, ?6, ?7,
            ?8, ?9, ?10, ?11, ?12, ?13,
            ?14, ?15, ?16, ?17, ?18,
            ?19, ?20, ?21,
            ?22, ?23, ?24
        )",
        params![
            item.uid,
            to_sql_i64(item.number)?,
            item.owner_bot_id,
            item.chat_id,
            item.group_id,
            item.source,
            made_by_json,
            item.title,
            item.description,
            item.done_means,
            item.status.as_str(),
            item.question_event.map(to_sql_i64).transpose()?,
            ask_json,
            item.wait_event.map(to_sql_i64).transpose()?,
            item.wait_note,
            item.close_note,
            to_sql_i64(item.version)?,
            to_sql_i64(item.last_event_id)?,
            item.others_last_event_id.map(to_sql_i64).transpose()?,
            item.snoozed_until_ms,
            item.last_activity_at_ms,
            item.created_at_ms,
            item.updated_at_ms,
            item.closed_at_ms,
        ],
    )?;
    Ok(())
}

fn update_item_row(conn: &Connection, item: &BotItem) -> Result<()> {
    let made_by_json = item
        .made_by
        .as_ref()
        .map(serde_json::to_string)
        .transpose()?;
    let ask_json = item.ask.as_ref().map(serde_json::to_string).transpose()?;
    conn.execute(
        "UPDATE items SET
            chat_id = ?2, group_id = ?3, source = ?4, made_by_json = ?5,
            title = ?6, description = ?7, done_means = ?8, status = ?9,
            question_event = ?10, ask_json = ?11, wait_event = ?12, wait_note = ?13,
            close_note = ?14, version = ?15, last_event_id = ?16,
            others_last_event_id = ?17, snoozed_until_ms = ?18,
            last_activity_at_ms = ?19, updated_at_ms = ?20, closed_at_ms = ?21
         WHERE uid = ?1",
        params![
            item.uid,
            item.chat_id,
            item.group_id,
            item.source,
            made_by_json,
            item.title,
            item.description,
            item.done_means,
            item.status.as_str(),
            item.question_event.map(to_sql_i64).transpose()?,
            ask_json,
            item.wait_event.map(to_sql_i64).transpose()?,
            item.wait_note,
            item.close_note,
            to_sql_i64(item.version)?,
            to_sql_i64(item.last_event_id)?,
            item.others_last_event_id.map(to_sql_i64).transpose()?,
            item.snoozed_until_ms,
            item.last_activity_at_ms,
            item.updated_at_ms,
            item.closed_at_ms,
        ],
    )?;
    Ok(())
}

fn upsert_item_link_row(conn: &Connection, item_uid: &str, link: &BotItemLink) -> Result<()> {
    let meta_json = serde_json::to_string(&link.meta)?;
    conn.execute(
        "INSERT INTO item_links (item_uid, kind, ref, added_event, meta_json)
         VALUES (?1, ?2, ?3, ?4, ?5)
         ON CONFLICT(item_uid, kind, ref) DO UPDATE SET
            added_event = excluded.added_event,
            meta_json = excluded.meta_json",
        params![
            item_uid,
            link.kind.as_str(),
            link.reference,
            to_sql_i64(link.added_event)?,
            meta_json,
        ],
    )?;
    Ok(())
}

fn insert_item_event_row(conn: &Connection, event: &BotItemEvent) -> Result<()> {
    let actor_json = serde_json::to_string(&event.actor)?;
    let data_json = serde_json::to_string(&event.data)?;
    conn.execute(
        "INSERT INTO item_events (
            item_uid, id, kind, actor_json, data_json, mail, action, created_at_ms
        ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
        params![
            event.item_uid,
            to_sql_i64(event.id)?,
            event_kind_to_str(event.kind),
            actor_json,
            data_json,
            mail_state_to_str(event.mail),
            event.action,
            event.created_at_ms,
        ],
    )?;
    Ok(())
}

fn load_item_by_uid(conn: &Connection, uid: &str) -> Result<BotItem> {
    let mut stmt = conn.prepare(
        "SELECT uid, number, owner_bot_id, chat_id, group_id, source, made_by_json,
                title, description, done_means, status, question_event, ask_json,
                wait_event, wait_note, close_note, version, last_event_id,
                others_last_event_id, snoozed_until_ms, last_activity_at_ms,
                created_at_ms, updated_at_ms, closed_at_ms
         FROM items WHERE uid = ?1",
    )?;
    let item_opt = stmt
        .query_row(params![uid], row_to_item)
        .optional()?;
    let mut item = match item_opt {
        Some(res) => res?,
        None => {
            return Err(OrcaBotsError::bot(
                BotErrorCode::NotFound,
                format!("Item '{uid}' not found."),
            ));
        }
    };
    item.links = load_item_links(conn, &item.uid)?;
    Ok(item)
}

fn load_all_items(conn: &Connection, owner_bot_id: Option<&str>) -> Result<Vec<BotItem>> {
    let mut items = Vec::new();
    if let Some(bot_id) = owner_bot_id {
        let mut stmt = conn.prepare(
            "SELECT uid, number, owner_bot_id, chat_id, group_id, source, made_by_json,
                    title, description, done_means, status, question_event, ask_json,
                    wait_event, wait_note, close_note, version, last_event_id,
                    others_last_event_id, snoozed_until_ms, last_activity_at_ms,
                    created_at_ms, updated_at_ms, closed_at_ms
             FROM items WHERE owner_bot_id = ?1
             ORDER BY number ASC",
        )?;
        let rows = stmt.query_map(params![bot_id], row_to_item)?;
        for r in rows {
            items.push(r??);
        }
    } else {
        let mut stmt = conn.prepare(
            "SELECT uid, number, owner_bot_id, chat_id, group_id, source, made_by_json,
                    title, description, done_means, status, question_event, ask_json,
                    wait_event, wait_note, close_note, version, last_event_id,
                    others_last_event_id, snoozed_until_ms, last_activity_at_ms,
                    created_at_ms, updated_at_ms, closed_at_ms
             FROM items ORDER BY number ASC",
        )?;
        let rows = stmt.query_map([], row_to_item)?;
        for r in rows {
            items.push(r??);
        }
    }
    for item in &mut items {
        item.links = load_item_links(conn, &item.uid)?;
    }
    Ok(items)
}

fn load_item_links(conn: &Connection, item_uid: &str) -> Result<Vec<BotItemLink>> {
    let mut stmt = conn.prepare(
        "SELECT kind, ref, added_event, meta_json
         FROM item_links WHERE item_uid = ?1
         ORDER BY added_event ASC, kind ASC, ref ASC",
    )?;
    let rows = stmt.query_map(params![item_uid], |row| {
        let kind_str: String = row.get(0)?;
        let reference: String = row.get(1)?;
        let added_event: i64 = row.get(2)?;
        let meta_json: String = row.get(3)?;
        Ok((kind_str, reference, added_event, meta_json))
    })?;

    let mut out = Vec::new();
    for r in rows {
        let (kind_str, reference, added_event, meta_json) = r?;
        let kind = BotItemLinkKind::parse(&kind_str).ok_or_else(|| {
            OrcaBotsError::bot(
                BotErrorCode::ItemsUnavailable,
                format!("Unknown item link kind '{kind_str}'."),
            )
        })?;
        let meta: BotItemLinkMeta = serde_json::from_str(&meta_json)?;
        out.push(BotItemLink {
            kind,
            reference,
            added_event: from_sql_u64(added_event)?,
            meta,
        });
    }
    Ok(out)
}

fn load_item_events(conn: &Connection, item_uid: &str) -> Result<Vec<BotItemEvent>> {
    let mut stmt = conn.prepare(
        "SELECT item_uid, id, kind, actor_json, data_json, mail, action, created_at_ms
         FROM item_events WHERE item_uid = ?1
         ORDER BY id ASC",
    )?;
    let rows = stmt.query_map(params![item_uid], row_to_event)?;
    let mut out = Vec::new();
    for r in rows {
        out.push(r??);
    }
    Ok(out)
}

fn row_to_item(row: &rusqlite::Row<'_>) -> rusqlite::Result<Result<BotItem>> {
    let uid: String = row.get(0)?;
    let number: i64 = row.get(1)?;
    let owner_bot_id: String = row.get(2)?;
    let chat_id: Option<String> = row.get(3)?;
    let group_id: Option<String> = row.get(4)?;
    let source: Option<String> = row.get(5)?;
    let made_by_json: Option<String> = row.get(6)?;
    let title: String = row.get(7)?;
    let description: Option<String> = row.get(8)?;
    let done_means: Option<String> = row.get(9)?;
    let status_str: String = row.get(10)?;
    let question_event: Option<i64> = row.get(11)?;
    let ask_json: Option<String> = row.get(12)?;
    let wait_event: Option<i64> = row.get(13)?;
    let wait_note: Option<String> = row.get(14)?;
    let close_note: Option<String> = row.get(15)?;
    let version: i64 = row.get(16)?;
    let last_event_id: i64 = row.get(17)?;
    let others_last_event_id: Option<i64> = row.get(18)?;
    let snoozed_until_ms: Option<i64> = row.get(19)?;
    let last_activity_at_ms: i64 = row.get(20)?;
    let created_at_ms: i64 = row.get(21)?;
    let updated_at_ms: i64 = row.get(22)?;
    let closed_at_ms: Option<i64> = row.get(23)?;

    Ok((|| -> Result<BotItem> {
        let status = BotItemStatus::parse_strict(&status_str).ok_or_else(|| {
            OrcaBotsError::bot(
                BotErrorCode::ItemsUnavailable,
                format!("Unknown item status '{status_str}'."),
            )
        })?;
        let made_by = made_by_json
            .as_deref()
            .map(serde_json::from_str)
            .transpose()?;
        let ask = ask_json.as_deref().map(serde_json::from_str).transpose()?;
        Ok(BotItem {
            uid,
            number: from_sql_u64(number)?,
            owner_bot_id,
            chat_id,
            group_id,
            source,
            made_by,
            title,
            description,
            done_means,
            status,
            question_event: question_event.map(from_sql_u64).transpose()?,
            ask,
            wait_event: wait_event.map(from_sql_u64).transpose()?,
            wait_note,
            close_note,
            version: from_sql_u64(version)?,
            last_event_id: from_sql_u64(last_event_id)?,
            others_last_event_id: others_last_event_id.map(from_sql_u64).transpose()?,
            snoozed_until_ms,
            last_activity_at_ms,
            created_at_ms,
            updated_at_ms,
            closed_at_ms,
            links: Vec::new(),
        })
    })())
}

fn row_to_event(row: &rusqlite::Row<'_>) -> rusqlite::Result<Result<BotItemEvent>> {
    let item_uid: String = row.get(0)?;
    let id: i64 = row.get(1)?;
    let kind_str: String = row.get(2)?;
    let actor_json: String = row.get(3)?;
    let data_json: String = row.get(4)?;
    let mail_str: Option<String> = row.get(5)?;
    let action: Option<String> = row.get(6)?;
    let created_at_ms: i64 = row.get(7)?;

    Ok((|| -> Result<BotItemEvent> {
        Ok(BotItemEvent {
            id: from_sql_u64(id)?,
            item_uid,
            kind: parse_event_kind(&kind_str)?,
            actor: serde_json::from_str(&actor_json)?,
            data: serde_json::from_str(&data_json)?,
            mail: parse_mail_state(mail_str)?,
            action,
            created_at_ms,
        })
    })())
}

fn build_closed_journal_entry(
    item: &BotItem,
    now_ms: i64,
    override_attribution: Option<JournalCloseAttribution>,
) -> JournalEntryInput {
    let date = DateTime::<Utc>::from_timestamp_millis(now_ms)
        .map(|dt| dt.format("%Y-%m-%d").to_string())
        .unwrap_or_else(|| "1970-01-01".to_string());
    let status = if item.status == BotItemStatus::Canceled {
        JournalCloseStatus::Canceled
    } else {
        JournalCloseStatus::Done
    };
    let prs: Vec<String> = item
        .links
        .iter()
        .filter(|l| l.kind == BotItemLinkKind::Pr)
        .map(|l| l.reference.clone())
        .collect();
    let attribution = override_attribution.unwrap_or_else(|| match &item.made_by {
        Some(BotItemActor::Person {
            source,
            user_id,
            name,
        }) => JournalCloseAttribution::ExternalPeople(vec![JournalAsker {
            source_name: source.clone(),
            user_id: user_id.clone(),
            name: name.clone(),
        }]),
        Some(BotItemActor::Owner | BotItemActor::Bot { .. } | BotItemActor::System) => {
            JournalCloseAttribution::Owner
        }
        None => JournalCloseAttribution::Unknown,
    });

    JournalEntryInput {
        date,
        item_number: item.number,
        status,
        title: item.title.clone(),
        prs,
        attribution,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use orca_core::bot_items::{
        BotItemAskOption, BotItemAskQuestion, DispatchLinkMeta, DispatchLiveState,
    };
    use orca_core::bot_types::{
        BotAgentId, BotAvatarSpec, BotConnectorLinks, BotMascotId, BotName, BotNotifications,
        OrcaBotRecord,
    };
    use orca_core::LOCAL_EXECUTION_HOST_ID;
    use tempfile::TempDir;

    use crate::home_store::BotHomeStore;

    fn sample_ask() -> BotItemAsk {
        BotItemAsk {
            context: Some("Deploy target confirmation".to_string()),
            questions: vec![
                BotItemAskQuestion {
                    prompt: "Environment".to_string(),
                    options: vec![
                        BotItemAskOption {
                            label: "Staging".to_string(),
                            detail: Some("Low risk".to_string()),
                        },
                        BotItemAskOption {
                            label: "Production".to_string(),
                            detail: None,
                        },
                    ],
                    recommended: Some(0),
                    multi: false,
                },
                BotItemAskQuestion {
                    prompt: "Regions".to_string(),
                    options: vec![
                        BotItemAskOption {
                            label: "us-east".to_string(),
                            detail: None,
                        },
                        BotItemAskOption {
                            label: "eu-west".to_string(),
                            detail: None,
                        },
                    ],
                    recommended: None,
                    multi: true,
                },
            ],
        }
    }

    #[test]
    fn six_state_lifecycle_cas_owner_cancel_snooze_and_stale_3x_planned() {
        let tmp = TempDir::new().unwrap();
        let db_path = tmp.path().join("items.sqlite");
        let store = BotItemsStore::open(&db_path).unwrap();
        assert!(!store.is_read_only().unwrap());
        assert_eq!(store.schema_version().unwrap(), ITEMS_SCHEMA_VERSION);

        let bot_actor = BotItemActor::Bot {
            bot_id: "bot-1".to_string(),
        };
        let t0 = 1_764_000_000_000i64;
        let (item, _) = store
            .create_item(
                CreateBotItemRequest::new("bot-1", bot_actor.clone(), "Ship v1.7"),
                t0,
            )
            .unwrap();
        assert_eq!(item.number, 1);
        assert_eq!(item.status, BotItemStatus::Planned);

        // Planned waits 3x longer for stale (Days5 -> 15 days vs 5 days)
        let day_ms = 86_400_000i64;
        assert!(!store.is_stale(&item, BotItemsStale::Days5, t0 + 10 * day_ms));
        assert!(store.is_stale(&item, BotItemsStale::Days5, t0 + 16 * day_ms));
        assert!(!store.is_stale(&item, BotItemsStale::Never, t0 + 100 * day_ms));

        // Snooze, then move to Waiting (remains snoozed), then move to NeedsYou (early unsnooze!)
        let (_, _) = store
            .snooze_item(&item.uid, BotItemActor::Owner, t0 + 5 * day_ms, t0 + 1_000)
            .unwrap();
        let (waiting, _) = store
            .set_status(
                &item.uid,
                StatusTransitionRequest {
                    actor: bot_actor.clone(),
                    to_status: BotItemStatus::Waiting,
                    expected_version: Some(1),
                    wait_note: Some("Waiting on CI".to_string()),
                    close_note: None,
                    note: None,
                    ask: None,
                },
                t0 + 2_000,
            )
            .unwrap();
        assert_eq!(waiting.version, 2);
        assert!(waiting.snoozed_until_ms.is_some());

        // Stale expected_version fails with ItemChanged
        let cas_err = store
            .set_status(
                &item.uid,
                StatusTransitionRequest {
                    actor: bot_actor.clone(),
                    to_status: BotItemStatus::Working,
                    expected_version: Some(1),
                    wait_note: None,
                    close_note: None,
                    note: None,
                    ask: None,
                },
                t0 + 3_000,
            )
            .unwrap_err();
        assert_eq!(cas_err.code(), BotErrorCode::ItemChanged);

        // Transition to NeedsYou auto-ends snooze with Unsnoozed { early: true }
        let (needs_you, outcome) = store
            .set_status(
                &item.uid,
                StatusTransitionRequest {
                    actor: bot_actor.clone(),
                    to_status: BotItemStatus::NeedsYou,
                    expected_version: Some(2),
                    wait_note: None,
                    close_note: None,
                    note: Some("Need deploy approval".to_string()),
                    ask: Some(sample_ask()),
                },
                t0 + 4_000,
            )
            .unwrap();
        assert!(outcome.unsnoozed_early);
        assert!(needs_you.snoozed_until_ms.is_none());
        assert!(outcome.emitted_events.iter().any(|e| e.is_early_unsnooze()));

        // Owner quick-reply answers multi-decision ask, sets mail='owed', transitions to Working
        let qid = needs_you.question_event.unwrap();
        let (working, reply_ev) = store
            .answer_question(
                &item.uid,
                AnswerQuestionRequest {
                    actor: BotItemActor::Owner,
                    question_event: qid,
                    selections: Some(vec![
                        AskSelection {
                            question_index: 0,
                            labels: vec!["Staging".to_string()],
                        },
                        AskSelection {
                            question_index: 1,
                            labels: vec!["us-east".to_string(), "eu-west".to_string()],
                        },
                    ]),
                    reply_text: None,
                    files: Vec::new(),
                },
                t0 + 5_000,
            )
            .unwrap();
        assert_eq!(working.status, BotItemStatus::Working);
        assert_eq!(reply_ev.mail, Some(BotItemMailState::Owed));
        assert_eq!(store.list_owed_mail_events(Some("bot-1")).unwrap().len(), 1);
        store.mark_mail_sent(&item.uid, reply_ev.id).unwrap();
        assert!(store.list_owed_mail_events(Some("bot-1")).unwrap().is_empty());

        // Duplicate answer fails with QuestionClosed
        let closed_q = store
            .answer_question(
                &item.uid,
                AnswerQuestionRequest {
                    actor: BotItemActor::Owner,
                    question_event: qid,
                    selections: None,
                    reply_text: Some("Staging".to_string()),
                    files: Vec::new(),
                },
                t0 + 6_000,
            )
            .unwrap_err();
        assert_eq!(closed_q.code(), BotErrorCode::QuestionClosed);

        // Owner cancels item -> bot CANNOT reopen (ItemClosed), owner CAN reopen
        let (canceled, _) = store
            .set_status(
                &item.uid,
                StatusTransitionRequest {
                    actor: BotItemActor::Owner,
                    to_status: BotItemStatus::Canceled,
                    expected_version: None,
                    wait_note: None,
                    close_note: Some("Superseded".to_string()),
                    note: None,
                    ask: None,
                },
                t0 + 7_000,
            )
            .unwrap();
        assert_eq!(canceled.status, BotItemStatus::Canceled);

        let reopen_err = store
            .set_status(
                &item.uid,
                StatusTransitionRequest {
                    actor: bot_actor,
                    to_status: BotItemStatus::Working,
                    expected_version: None,
                    wait_note: None,
                    close_note: None,
                    note: None,
                    ask: None,
                },
                t0 + 8_000,
            )
            .unwrap_err();
        assert_eq!(reopen_err.code(), BotErrorCode::ItemClosed);
    }

    #[test]
    fn newer_user_version_opens_read_only_and_close_writes_journal_and_links_fold() {
        let tmp = TempDir::new().unwrap();
        let db_path = tmp.path().join("items.sqlite");
        let home = BotHomeStore::new(tmp.path().join("bots"));
        let mem_svc = BotMemoryService::new(home.clone(), tmp.path().join("mem_state"));
        let bot = OrcaBotRecord {
            id: "bot-j".to_string(),
            name: BotName::new("Journaler").unwrap(),
            host_id: LOCAL_EXECUTION_HOST_ID.to_string(),
            folder: BotFolderSlug::new("journaler").unwrap(),
            folder_workspace_id: None,
            agent: BotAgentId::Claude,
            avatar_spec: BotAvatarSpec::new_with_mascot(BotMascotId::B1),
            paused: false,
            permission_mode: None,
            launch: None,
            notifications: BotNotifications::default(),
            members: Vec::new(),
            connectors: BotConnectorLinks::default(),
            skill_projects: None,
            sidebar_order: None,
            chat_session_id: None,
            look_around: None,
            tidy_up: None,
            items_stale: None,
            items_seen: None,
            first_run: false,
            accepted_offers: None,
            offer_answers: None,
            handed_paired_workspaces: None,
            cancelled_waits: None,
            imported_from: None,
            shared_from: None,
            template_id: None,
            created_at: 1_000,
            updated_at: 1_000,
        };
        let bot_dir = home.provision_bot_home(&bot, None).unwrap();

        let store = BotItemsStore::open(&db_path).unwrap();
        store.set_memory_service(mem_svc).unwrap();
        store.register_bot_folder(&bot.id, bot.folder.clone()).unwrap();

        let t0 = 1_764_072_000_000i64; // 2025-11-25 UTC
        let (it, _) = store
            .create_item(
                CreateBotItemRequest::new(&bot.id, BotItemActor::Owner, "Patch webhook"),
                t0,
            )
            .unwrap();

        // Add two dispatch links on the same chain + one PR link
        store
            .add_link(
                &it.uid,
                BotItemActor::Owner,
                BotItemLink {
                    kind: BotItemLinkKind::Dispatch,
                    reference: "disp-1".to_string(),
                    added_event: 0,
                    meta: BotItemLinkMeta::Dispatch(DispatchLinkMeta {
                        chain_id: Some("chain-A".to_string()),
                        state: Some(DispatchLiveState::Completed),
                        ..Default::default()
                    }),
                },
                t0 + 100,
            )
            .unwrap();
        store
            .add_link(
                &it.uid,
                BotItemActor::Owner,
                BotItemLink {
                    kind: BotItemLinkKind::Dispatch,
                    reference: "disp-2".to_string(),
                    added_event: 0,
                    meta: BotItemLinkMeta::Dispatch(DispatchLinkMeta {
                        chain_id: Some("chain-A".to_string()),
                        parent_dispatch_id: Some("disp-1".to_string()),
                        state: Some(DispatchLiveState::Completed),
                        ..Default::default()
                    }),
                },
                t0 + 200,
            )
            .unwrap();
        store
            .add_link(
                &it.uid,
                BotItemActor::Owner,
                BotItemLink {
                    kind: BotItemLinkKind::Pr,
                    reference: "acme/app#77".to_string(),
                    added_event: 0,
                    meta: BotItemLinkMeta::None,
                },
                t0 + 300,
            )
            .unwrap();
        assert_eq!(store.links_one_per_chain(&it.uid).unwrap().len(), 2);

        // Close item -> automatic deterministic journal append
        store
            .set_status(
                &it.uid,
                StatusTransitionRequest {
                    actor: BotItemActor::Owner,
                    to_status: BotItemStatus::Done,
                    expected_version: None,
                    wait_note: None,
                    close_note: Some("Merged".to_string()),
                    note: None,
                    ask: None,
                },
                t0 + 400,
            )
            .unwrap();
        let month = DateTime::<Utc>::from_timestamp_millis(t0 + 400)
            .unwrap()
            .format("%Y-%m")
            .to_string();
        let journal =
            std::fs::read_to_string(bot_dir.join(format!("memory/journal/{month}.md"))).unwrap();
        assert!(journal.contains("item #1"));
        assert!(journal.contains("PR #77") || journal.contains("acme/app#77"));

        // Needs-you priority folding
        let inbox = store.needs_you_inbox(Some(&bot.id), None, Some(5), t0 + 500).unwrap();
        assert!(inbox.visible.is_empty());

        // Simulate newer schema user_version = 99 -> opens strictly read-only
        drop(store);
        {
            let conn = Connection::open(&db_path).unwrap();
            conn.pragma_update(None, "user_version", 99u32).unwrap();
        }
        let ro_store = BotItemsStore::open(&db_path).unwrap();
        assert!(ro_store.is_read_only().unwrap());
        assert_eq!(ro_store.get_item_by_number(1).unwrap().title, "Patch webhook");
        let ro_err = ro_store
            .create_item(
                CreateBotItemRequest::new(&bot.id, BotItemActor::Owner, "Blocked"),
                t0 + 600,
            )
            .unwrap_err();
        assert_eq!(ro_err.code(), BotErrorCode::ItemsReadOnly);
    }
}
