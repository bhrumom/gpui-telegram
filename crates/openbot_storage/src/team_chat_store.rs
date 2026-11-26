//! `TeamChatStore`, `MessagingStore`, and `MessagingHistoryStore` —
//! Member-to-member direct messaging (`dm-<32hex>`, `projection_direct_threads`,
//! `projection_direct_messages`, `projection_direct_reads`) and external platform bridges
//! (`projection_messaging_{connections,threads,history,pings,allowed_roles,allowed_senders}`).
//! Mirrors `src/backend/team-chat-store.ts` and `src/backend/messaging/{messaging-store,messaging-history-store}.ts`.

use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use openbot_contracts::limits::{INPUT_LIMITS, utf16_len};
use rusqlite::{OptionalExtension, params};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use uuid::Uuid;

use crate::agent_store::{
    delete_aggregate_history, delete_thread_attachments, hex_lower, now_iso,
};
use crate::database::{OpenBotDatabase, OrchestrationEventInput};
use crate::error::{StorageError, StorageResult};

pub const DIRECT_PAGE_LIMIT: usize = 100;
pub const MESSAGING_LINK_LIST_LIMIT: usize = 200;
pub const MESSAGING_CONTEXT_MESSAGES: usize = 200;
const MESSAGING_QUERY_WORDS: usize = 8;

/// Deterministic sorted pair `(min, max)` and 35-char `"dm-<32hex>"` SHA-256 thread ID
/// (`team-chat-store.ts:L379-L383`, `L437-L439`).
pub fn sorted_member_ids<'a>(left: &'a str, right: &'a str) -> (&'a str, &'a str) {
    if left <= right {
        (left, right)
    } else {
        (right, left)
    }
}

pub fn direct_thread_id(left_member_id: &str, right_member_id: &str) -> String {
    let (a, b) = sorted_member_ids(left_member_id, right_member_id);
    let mut hasher = Sha256::new();
    hasher.update(format!("{a}\0{b}").as_bytes());
    let digest = hasher.finalize();
    format!("dm-{}", hex_lower(&digest[..16]))
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DirectMessage {
    pub id: String,
    pub thread_id: String,
    pub sender_member_id: String,
    pub recipient_member_id: String,
    pub text: String,
    pub created_at: String,
    pub sequence: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DirectThreadSummary {
    pub thread_id: String,
    pub other_member_id: String,
    pub last_message: DirectMessage,
    pub unread_count: u64,
    pub updated_at: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DirectConversationReadState {
    pub unread_count: u64,
    #[serde(deserialize_with = "Option::deserialize")]
    pub first_unread_message_id: Option<String>,
    pub through_sequence: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DirectConversationSnapshot {
    pub thread_id: String,
    pub other_member_id: String,
    pub messages: Vec<DirectMessage>,
    pub revision: u64,
    pub read_state: DirectConversationReadState,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "lowercase")]
pub enum DirectConversationPageAnchor {
    Latest,
    Before {
        cursor: String,
    },
    Around {
        #[serde(rename = "messageId")]
        message_id: String,
    },
}

impl Default for DirectConversationPageAnchor {
    fn default() -> Self {
        Self::Latest
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DirectConversationPageInfo {
    pub has_older: bool,
    #[serde(deserialize_with = "Option::deserialize")]
    pub older_cursor: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub older_count: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub oldest_at: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DirectConversationPage {
    pub thread_id: String,
    pub other_member_id: String,
    pub messages: Vec<DirectMessage>,
    pub revision: u64,
    pub page_info: DirectConversationPageInfo,
    pub read_state: DirectConversationReadState,
}

#[derive(Debug, Clone)]
pub struct SendDirectMessageInput {
    pub client_message_id: String,
    pub sender_member_id: String,
    pub recipient_member_id: String,
    pub text: String,
    pub created_at: Option<String>,
}

pub fn encode_direct_cursor(sequence: u64) -> StorageResult<String> {
    Ok(URL_SAFE_NO_PAD.encode(serde_json::to_vec(
        &serde_json::json!({ "version": 1, "sequence": sequence }),
    )?))
}

pub fn decode_direct_cursor(value: &str) -> StorageResult<u64> {
    let bytes = URL_SAFE_NO_PAD
        .decode(value.trim_end_matches('='))
        .map_err(|_| StorageError::Validation("The direct-message page cursor is invalid.".to_string()))?;
    let parsed: serde_json::Value = serde_json::from_slice(&bytes)
        .map_err(|_| StorageError::Validation("The direct-message page cursor is invalid.".to_string()))?;
    if parsed.get("version").and_then(|v| v.as_u64()) != Some(1) {
        return Err(StorageError::Validation(
            "The direct-message page cursor is invalid.".to_string(),
        ));
    }
    parsed
        .get("sequence")
        .and_then(|v| v.as_u64())
        .ok_or_else(|| StorageError::Validation("The direct-message page cursor is invalid.".to_string()))
}

/// `TeamChatStore` over `projection_direct_threads`, `projection_direct_messages`, and
/// `projection_direct_reads` (`src/backend/team-chat-store.ts`).
#[derive(Clone)]
pub struct TeamChatStore {
    database: OpenBotDatabase,
}

impl TeamChatStore {
    pub fn new(database: OpenBotDatabase) -> Self {
        Self { database }
    }

    pub fn database(&self) -> &OpenBotDatabase {
        &self.database
    }

    pub fn list_threads(&self, member_id: &str) -> StorageResult<Vec<DirectThreadSummary>> {
        self.database.with_conn(|conn| {
            let mut stmt = conn.prepare_cached(
                "SELECT
                   thread.thread_id,
                   thread.member_a_id,
                   thread.member_b_id,
                   thread.updated_at,
                   message.message_json,
                   (
                     SELECT COUNT(*)
                     FROM projection_direct_messages unread
                     WHERE unread.thread_id = thread.thread_id
                       AND unread.sender_member_id != ?1
                       AND unread.last_event_sequence > COALESCE(read_state.last_read_sequence, 0)
                   ) AS unread_count
                 FROM projection_direct_threads thread
                 JOIN projection_direct_messages message
                   ON message.message_id = thread.last_message_id
                 LEFT JOIN projection_direct_reads read_state
                   ON read_state.thread_id = thread.thread_id AND read_state.member_id = ?1
                 WHERE thread.member_a_id = ?1 OR thread.member_b_id = ?1
                 ORDER BY thread.updated_at DESC, thread.last_event_sequence DESC",
            )?;
            let rows = stmt.query_map(params![member_id], |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    r.get::<_, String>(1)?,
                    r.get::<_, String>(2)?,
                    r.get::<_, String>(3)?,
                    r.get::<_, String>(4)?,
                    r.get::<_, i64>(5)?,
                ))
            })?;
            let mut summaries = Vec::new();
            for row in rows {
                let (tid, ma, mb, updated_at, msg_json, unread) = row?;
                let other = if ma == member_id { mb } else { ma };
                let last_message: DirectMessage = serde_json::from_str(&msg_json)?;
                summaries.push(DirectThreadSummary {
                    thread_id: tid,
                    other_member_id: other,
                    last_message,
                    unread_count: unread.max(0) as u64,
                    updated_at,
                });
            }
            Ok(summaries)
        })
    }

    pub fn read_conversation(
        &self,
        member_id: &str,
        other_member_id: &str,
    ) -> StorageResult<DirectConversationSnapshot> {
        let thread_id = direct_thread_id(member_id, other_member_id);
        self.database.with_conn(|conn| {
            let rev_opt: Option<i64> = conn
                .prepare_cached(
                    "SELECT last_event_sequence FROM projection_direct_threads
                     WHERE thread_id = ?1 AND (member_a_id = ?2 OR member_b_id = ?2)",
                )?
                .query_row(params![thread_id, member_id], |r| r.get(0))
                .optional()?;
            let mut stmt = conn.prepare_cached(
                "SELECT message_json FROM projection_direct_messages
                 WHERE thread_id = ?1 ORDER BY last_event_sequence, message_id",
            )?;
            let rows = stmt.query_map(params![thread_id], |r| r.get::<_, String>(0))?;
            let mut messages = Vec::new();
            for raw in rows {
                messages.push(serde_json::from_str::<DirectMessage>(&raw?)?);
            }
            let through: i64 = conn
                .prepare_cached(
                    "SELECT last_read_sequence FROM projection_direct_reads
                     WHERE thread_id = ?1 AND member_id = ?2",
                )?
                .query_row(params![thread_id, member_id], |r| r.get(0))
                .optional()?
                .unwrap_or(0);
            let through_u64 = through.max(0) as u64;
            let unread: Vec<&DirectMessage> = messages
                .iter()
                .filter(|m| m.sender_member_id != member_id && m.sequence > through_u64)
                .collect();
            Ok(DirectConversationSnapshot {
                thread_id,
                other_member_id: other_member_id.to_string(),
                revision: rev_opt.unwrap_or(0).max(0) as u64,
                read_state: DirectConversationReadState {
                    unread_count: unread.len() as u64,
                    first_unread_message_id: unread.first().map(|m| m.id.clone()),
                    through_sequence: through_u64,
                },
                messages,
            })
        })
    }

    pub fn read_conversation_page(
        &self,
        member_id: &str,
        other_member_id: &str,
        anchor: &DirectConversationPageAnchor,
        requested_limit: usize,
    ) -> StorageResult<DirectConversationPage> {
        if requested_limit == 0 {
            return Err(StorageError::Validation(
                "The direct-message page limit is invalid.".to_string(),
            ));
        }
        let limit = requested_limit.min(DIRECT_PAGE_LIMIT);
        let thread_id = direct_thread_id(member_id, other_member_id);
        self.database.with_conn(|conn| {
            let rev_opt: Option<i64> = conn
                .prepare_cached(
                    "SELECT last_event_sequence FROM projection_direct_threads
                     WHERE thread_id = ?1 AND (member_a_id = ?2 OR member_b_id = ?2)",
                )?
                .query_row(params![thread_id, member_id], |r| r.get(0))
                .optional()?;

            let rows: Vec<(String, i64)> = match anchor {
                DirectConversationPageAnchor::Latest => {
                    let mut v = conn
                        .prepare_cached(
                            "SELECT message_json, last_event_sequence FROM projection_direct_messages
                             WHERE thread_id = ?1 ORDER BY last_event_sequence DESC, message_id DESC LIMIT ?2",
                        )?
                        .query_map(params![thread_id, limit as i64], |r| Ok((r.get(0)?, r.get(1)?)))?
                        .collect::<Result<Vec<_>, _>>()?;
                    v.reverse();
                    v
                }
                DirectConversationPageAnchor::Before { cursor } => {
                    let seq = decode_direct_cursor(cursor)? as i64;
                    let mut v = conn
                        .prepare_cached(
                            "SELECT message_json, last_event_sequence FROM projection_direct_messages
                             WHERE thread_id = ?1 AND last_event_sequence < ?2
                             ORDER BY last_event_sequence DESC, message_id DESC LIMIT ?3",
                        )?
                        .query_map(params![thread_id, seq, limit as i64], |r| {
                            Ok((r.get(0)?, r.get(1)?))
                        })?
                        .collect::<Result<Vec<_>, _>>()?;
                    v.reverse();
                    v
                }
                DirectConversationPageAnchor::Around { message_id } => {
                    let anchor_seq: Option<i64> = conn
                        .prepare_cached(
                            "SELECT last_event_sequence FROM projection_direct_messages
                             WHERE thread_id = ?1 AND message_id = ?2",
                        )?
                        .query_row(params![thread_id, message_id], |r| r.get(0))
                        .optional()?;
                    match anchor_seq {
                        None => Vec::new(),
                        Some(seq) => {
                            let older_lim = (limit / 2) + 1;
                            let mut older = conn
                                .prepare_cached(
                                    "SELECT message_json, last_event_sequence FROM projection_direct_messages
                                     WHERE thread_id = ?1 AND last_event_sequence <= ?2
                                     ORDER BY last_event_sequence DESC, message_id DESC LIMIT ?3",
                                )?
                                .query_map(params![thread_id, seq, older_lim as i64], |r| {
                                    Ok((r.get(0)?, r.get(1)?))
                                })?
                                .collect::<Result<Vec<_>, _>>()?;
                            older.reverse();
                            let newer_lim = limit.saturating_sub(older.len());
                            let newer = conn
                                .prepare_cached(
                                    "SELECT message_json, last_event_sequence FROM projection_direct_messages
                                     WHERE thread_id = ?1 AND last_event_sequence > ?2
                                     ORDER BY last_event_sequence, message_id LIMIT ?3",
                                )?
                                .query_map(params![thread_id, seq, newer_lim as i64], |r| {
                                    Ok((r.get(0)?, r.get(1)?))
                                })?
                                .collect::<Result<Vec<_>, _>>()?;
                            older.extend(newer);
                            older
                        }
                    }
                }
            };

            let first_seq = rows.first().map(|(_, s)| *s).unwrap_or(0);
            let mut messages = Vec::with_capacity(rows.len());
            for (mj, _) in rows {
                messages.push(serde_json::from_str::<DirectMessage>(&mj)?);
            }
            let (older_count, oldest_at) = if first_seq > 0 {
                let (cnt, oa): (i64, Option<String>) = conn
                    .prepare_cached(
                        "SELECT COUNT(*),
                           (SELECT created_at FROM projection_direct_messages
                            WHERE thread_id = ?1 ORDER BY last_event_sequence LIMIT 1)
                         FROM projection_direct_messages
                         WHERE thread_id = ?1 AND last_event_sequence < ?2",
                    )?
                    .query_row(params![thread_id, first_seq], |r| Ok((r.get(0)?, r.get(1)?)))?;
                (cnt.max(0) as u64, oa)
            } else {
                (0, None)
            };
            let has_older = older_count > 0;
            let read_state = read_direct_state_from_db(conn, member_id, &thread_id)?;
            Ok(DirectConversationPage {
                thread_id,
                other_member_id: other_member_id.to_string(),
                messages,
                revision: rev_opt.unwrap_or(0).max(0) as u64,
                page_info: DirectConversationPageInfo {
                    has_older,
                    older_cursor: has_older
                        .then(|| encode_direct_cursor(first_seq as u64))
                        .transpose()?,
                    older_count: has_older.then_some(older_count),
                    oldest_at,
                },
                read_state,
            })
        })
    }

    pub fn send_message(&self, input: &SendDirectMessageInput) -> StorageResult<DirectMessage> {
        let text = input.text.trim();
        if text.is_empty() {
            return Err(StorageError::Validation(
                "Direct message text is required.".to_string(),
            ));
        }
        if utf16_len(text) > INPUT_LIMITS.direct_message_text {
            return Err(StorageError::LimitExceeded(format!(
                "Direct message exceeds {} characters.",
                INPUT_LIMITS.direct_message_text
            )));
        }
        if input.sender_member_id == input.recipient_member_id {
            return Err(StorageError::Validation(
                "A member cannot send a direct message to themselves.".to_string(),
            ));
        }
        let (member_a, member_b) =
            sorted_member_ids(&input.sender_member_id, &input.recipient_member_id);
        let thread_id = direct_thread_id(member_a, member_b);
        let created_at = input.created_at.clone().unwrap_or_else(now_iso);
        let draft = DirectMessage {
            id: input.client_message_id.clone(),
            thread_id: thread_id.clone(),
            sender_member_id: input.sender_member_id.clone(),
            recipient_member_id: input.recipient_member_id.clone(),
            text: text.to_string(),
            created_at: created_at.clone(),
            sequence: 0,
        };
        let command_id = format!("direct-message:{}:{}", draft.sender_member_id, draft.id);
        let events = [OrchestrationEventInput {
            event_id: Uuid::new_v4().to_string(),
            aggregate_type: "direct-thread".to_string(),
            aggregate_id: thread_id.clone(),
            event_type: "direct-message.sent".to_string(),
            occurred_at: created_at.clone(),
            payload: serde_json::to_value(&draft)?,
        }];
        let receipt = self.database.dispatch_with_projection(
            &command_id,
            None,
            &events,
            |conn, sequences| {
                let seq = sequences.first().copied().unwrap_or(0);
                let mut stored = draft.clone();
                stored.sequence = seq.max(0) as u64;
                conn.execute(
                    "INSERT INTO projection_direct_threads (
                       thread_id, member_a_id, member_b_id, created_at, updated_at,
                       last_message_id, last_event_sequence
                     ) VALUES (?1, ?2, ?3, ?4, ?4, ?5, ?6)
                     ON CONFLICT(thread_id) DO UPDATE SET
                       updated_at = excluded.updated_at,
                       last_message_id = excluded.last_message_id,
                       last_event_sequence = excluded.last_event_sequence",
                    params![thread_id, member_a, member_b, created_at, stored.id, seq],
                )?;
                conn.execute(
                    "INSERT INTO projection_direct_messages (
                       message_id, thread_id, sender_member_id, recipient_member_id, text,
                       created_at, message_json, last_event_sequence
                     ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
                    params![
                        stored.id,
                        stored.thread_id,
                        stored.sender_member_id,
                        stored.recipient_member_id,
                        stored.text,
                        stored.created_at,
                        serde_json::to_string(&stored)?,
                        seq
                    ],
                )?;
                Ok(stored)
            },
        )?;
        Ok(receipt.result)
    }

    pub fn mark_read(
        &self,
        member_id: &str,
        other_member_id: &str,
        through_sequence: u64,
    ) -> StorageResult<DirectConversationReadState> {
        let thread_id = direct_thread_id(member_id, other_member_id);
        let state_opt: Option<(i64, Option<i64>)> = self.database.with_conn(|conn| {
            Ok(conn
                .prepare_cached(
                    "SELECT thread.last_event_sequence, read_state.last_read_sequence
                     FROM projection_direct_threads thread
                     LEFT JOIN projection_direct_reads read_state
                       ON read_state.thread_id = thread.thread_id AND read_state.member_id = ?1
                     WHERE thread.thread_id = ?2 AND (thread.member_a_id = ?1 OR thread.member_b_id = ?1)",
                )?
                .query_row(params![member_id, thread_id], |r| Ok((r.get(0)?, r.get(1)?)))
                .optional()?)
        })?;
        let Some((last_event_seq, last_read_opt)) = state_opt else {
            return Ok(DirectConversationReadState {
                unread_count: 0,
                first_unread_message_id: None,
                through_sequence: 0,
            });
        };
        let last_event_u64 = last_event_seq.max(0) as u64;
        let last_read_u64 = last_read_opt.unwrap_or(0).max(0) as u64;
        let next_seq = last_read_u64.max(through_sequence.min(last_event_u64));
        if last_read_u64 >= next_seq {
            return self
                .database
                .with_conn(|c| read_direct_state_from_db(c, member_id, &thread_id));
        }
        let updated_at = now_iso();
        let command_id = format!("direct-read:{thread_id}:{member_id}:{next_seq}");
        let events = [OrchestrationEventInput {
            event_id: Uuid::new_v4().to_string(),
            aggregate_type: "direct-thread".to_string(),
            aggregate_id: thread_id.clone(),
            event_type: "direct-thread.read".to_string(),
            occurred_at: updated_at.clone(),
            payload: serde_json::json!({
                "memberId": member_id,
                "throughSequence": next_seq,
            }),
        }];
        let _ = self.database.dispatch_with_projection(
            &command_id,
            None,
            &events,
            |conn, _seq| {
                conn.execute(
                    "INSERT INTO projection_direct_reads
                       (thread_id, member_id, last_read_sequence, updated_at)
                     VALUES (?1, ?2, ?3, ?4)
                     ON CONFLICT(thread_id, member_id) DO UPDATE SET
                       last_read_sequence = MAX(last_read_sequence, excluded.last_read_sequence),
                       updated_at = excluded.updated_at",
                    params![thread_id, member_id, next_seq as i64, updated_at],
                )?;
                Ok(serde_json::Value::Null)
            },
        )?;
        self.database
            .with_conn(|c| read_direct_state_from_db(c, member_id, &thread_id))
    }
}

fn read_direct_state_from_db(
    conn: &rusqlite::Connection,
    member_id: &str,
    thread_id: &str,
) -> StorageResult<DirectConversationReadState> {
    let through: i64 = conn
        .prepare_cached(
            "SELECT last_read_sequence FROM projection_direct_reads
             WHERE thread_id = ?1 AND member_id = ?2",
        )?
        .query_row(params![thread_id, member_id], |r| r.get(0))
        .optional()?
        .unwrap_or(0);
    let unread_count: i64 = conn
        .prepare_cached(
            "SELECT COUNT(*) FROM projection_direct_messages
             WHERE thread_id = ?1 AND sender_member_id != ?2 AND last_event_sequence > ?3",
        )?
        .query_row(params![thread_id, member_id, through], |r| r.get(0))?;
    let first_unread_message_id: Option<String> = conn
        .prepare_cached(
            "SELECT message_id FROM projection_direct_messages
             WHERE thread_id = ?1 AND sender_member_id != ?2 AND last_event_sequence > ?3
             ORDER BY last_event_sequence, message_id LIMIT 1",
        )?
        .query_row(params![thread_id, member_id, through], |r| r.get(0))
        .optional()?;
    Ok(DirectConversationReadState {
        unread_count: unread_count.max(0) as u64,
        first_unread_message_id,
        through_sequence: through.max(0) as u64,
    })
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum MessagingPlatform {
    Slack,
    Discord,
    Telegram,
    Whatsapp,
}

impl MessagingPlatform {
    pub const ALL: [Self; 4] = [
        Self::Slack,
        Self::Discord,
        Self::Telegram,
        Self::Whatsapp,
    ];

    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Slack => "slack",
            Self::Discord => "discord",
            Self::Telegram => "telegram",
            Self::Whatsapp => "whatsapp",
        }
    }

    pub fn parse(raw: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|p| p.as_str() == raw)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MessagingConnectionRecord {
    pub connection_id: String,
    pub platform: MessagingPlatform,
    pub workspace_id: String,
    pub workspace_name: String,
    pub enabled: bool,
    pub bot_user_id: Option<String>,
    pub app_id: Option<String>,
    pub orchestrator_agent_id: Option<String>,
    pub last_error_code: Option<String>,
}

#[derive(Debug, Clone, Default)]
pub struct MessagingConnectionPatch {
    pub enabled: Option<bool>,
    pub workspace_name: Option<String>,
    pub bot_user_id: Option<Option<String>>,
    pub app_id: Option<Option<String>>,
    pub orchestrator_agent_id: Option<Option<String>>,
    pub last_error_code: Option<Option<String>>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MessagingLink {
    pub link_id: String,
    pub connection_id: String,
    pub agent_id: String,
    pub platform_channel_id: String,
    pub thread_key: String,
    pub is_direct: bool,
    pub thread_id: String,
    pub title: String,
    pub history_cursor: Option<String>,
    pub updated_at: String,
}

#[derive(Debug, Clone)]
pub struct MessagingLinkInput {
    pub connection_id: String,
    pub agent_id: String,
    pub platform_channel_id: String,
    pub thread_key: String,
    pub is_direct: bool,
    pub title: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MessagingAllowedRole {
    pub id: String,
    pub name: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum MessagingSenderKind {
    Contact,
    Group,
}

impl MessagingSenderKind {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Contact => "contact",
            Self::Group => "group",
        }
    }

    pub fn parse(raw: &str) -> Option<Self> {
        match raw {
            "contact" => Some(Self::Contact),
            "group" => Some(Self::Group),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MessagingAllowedSender {
    pub id: String,
    pub kind: MessagingSenderKind,
    pub name: String,
    pub read_only: bool,
}

#[derive(Debug, Clone)]
pub struct MessagingPingInput {
    pub connection_id: String,
    pub platform_channel_id: String,
    pub thread_key: String,
    pub user_ids: Vec<String>,
    pub agent_id: String,
    pub text: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MessagingPing {
    pub user_id: String,
    pub agent_id: String,
    pub text: String,
    pub created_at: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct InboundFile {
    pub id: String,
    pub name: String,
    pub mime_type: String,
    pub size: u64,
    pub url: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MessagingContextEntry {
    pub id: String,
    pub author_name: String,
    pub text: String,
    pub sent_at: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub files: Option<Vec<InboundFile>>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MessagingThreadedEntry {
    #[serde(flatten)]
    pub entry: MessagingContextEntry,
    pub thread_key: String,
}

#[derive(Debug, Clone, Default)]
pub struct MessagingHistoryFilter {
    pub query: Option<String>,
    pub before: Option<String>,
}

/// `MessagingStore` over `projection_messaging_connections`, `projection_messaging_threads`,
/// `projection_messaging_pings`, `projection_messaging_allowed_roles`, and
/// `projection_messaging_allowed_senders` (`src/backend/messaging/messaging-store.ts`).
#[derive(Clone)]
pub struct MessagingStore {
    database: OpenBotDatabase,
}

impl MessagingStore {
    pub fn new(database: OpenBotDatabase) -> Self {
        Self { database }
    }

    pub fn connections(&self) -> StorageResult<Vec<MessagingConnectionRecord>> {
        self.database.with_conn(|conn| {
            let mut stmt = conn.prepare_cached(
                "SELECT connection_id, platform, workspace_id, workspace_name, enabled,
                        bot_user_id, app_id, orchestrator_agent_id, last_error_code
                 FROM projection_messaging_connections ORDER BY created_at",
            )?;
            let rows = stmt.query_map([], decode_connection_row)?;
            let mut out = Vec::new();
            for item in rows {
                if let Some(rec) = item? {
                    out.push(rec);
                }
            }
            Ok(out)
        })
    }

    pub fn connection_for_workspace(
        &self,
        platform: MessagingPlatform,
        workspace_id: &str,
    ) -> StorageResult<Option<MessagingConnectionRecord>> {
        self.database.with_conn(|conn| {
            let row = conn
                .prepare_cached(
                    "SELECT connection_id, platform, workspace_id, workspace_name, enabled,
                            bot_user_id, app_id, orchestrator_agent_id, last_error_code
                     FROM projection_messaging_connections
                     WHERE platform = ?1 AND workspace_id = ?2",
                )?
                .query_row(params![platform.as_str(), workspace_id], decode_connection_row)
                .optional()?;
            Ok(row.flatten())
        })
    }

    pub fn connection(&self, connection_id: &str) -> StorageResult<Option<MessagingConnectionRecord>> {
        self.database.with_conn(|conn| {
            let row = conn
                .prepare_cached(
                    "SELECT connection_id, platform, workspace_id, workspace_name, enabled,
                            bot_user_id, app_id, orchestrator_agent_id, last_error_code
                     FROM projection_messaging_connections WHERE connection_id = ?1",
                )?
                .query_row(params![connection_id], decode_connection_row)
                .optional()?;
            Ok(row.flatten())
        })
    }

    pub fn ensure_connection(
        &self,
        platform: MessagingPlatform,
        workspace_id: &str,
        workspace_name: &str,
    ) -> StorageResult<MessagingConnectionRecord> {
        if let Some(existing) = self.connection_for_workspace(platform, workspace_id)? {
            return Ok(existing);
        }
        let now = now_iso();
        let conn_id = format!("messaging-{}", Uuid::new_v4());
        self.database.with_conn(|conn| {
            conn.execute(
                "INSERT INTO projection_messaging_connections
                   (connection_id, platform, workspace_id, workspace_name, enabled, created_at, updated_at)
                 VALUES (?1, ?2, ?3, ?4, 0, ?5, ?5)",
                params![conn_id, platform.as_str(), workspace_id, workspace_name, now],
            )?;
            Ok(())
        })?;
        self.connection_for_workspace(platform, workspace_id)?
            .ok_or_else(|| StorageError::NotFound("The messaging connection was not stored.".to_string()))
    }

    pub fn update_connection(
        &self,
        connection_id: &str,
        patch: &MessagingConnectionPatch,
    ) -> StorageResult<()> {
        let Some(current) = self.connection(connection_id)? else {
            return Ok(());
        };
        let enabled = patch.enabled.unwrap_or(current.enabled);
        let workspace_name = patch
            .workspace_name
            .clone()
            .unwrap_or(current.workspace_name);
        let bot_user_id = patch.bot_user_id.clone().unwrap_or(current.bot_user_id);
        let app_id = patch.app_id.clone().unwrap_or(current.app_id);
        let orchestrator = patch
            .orchestrator_agent_id
            .clone()
            .unwrap_or(current.orchestrator_agent_id);
        let last_err = patch
            .last_error_code
            .clone()
            .unwrap_or(current.last_error_code);
        self.database.with_conn(|conn| {
            conn.execute(
                "UPDATE projection_messaging_connections
                 SET enabled = ?1, workspace_name = ?2, bot_user_id = ?3, app_id = ?4,
                     orchestrator_agent_id = ?5, last_error_code = ?6, updated_at = ?7
                 WHERE connection_id = ?8",
                params![
                    i64::from(enabled),
                    workspace_name,
                    bot_user_id,
                    app_id,
                    orchestrator,
                    last_err,
                    now_iso(),
                    connection_id
                ],
            )?;
            Ok(())
        })
    }

    pub fn allowed_roles(&self, connection_id: &str) -> StorageResult<Vec<MessagingAllowedRole>> {
        self.database.with_conn(|conn| {
            let mut stmt = conn.prepare_cached(
                "SELECT role_id, role_name FROM projection_messaging_allowed_roles
                 WHERE connection_id = ?1 ORDER BY created_at, role_name, role_id",
            )?;
            let rows = stmt.query_map(params![connection_id], |r| {
                Ok(MessagingAllowedRole {
                    id: r.get(0)?,
                    name: r.get(1)?,
                })
            })?;
            Ok(rows.collect::<Result<Vec<_>, _>>()?)
        })
    }

    pub fn replace_allowed_roles(
        &self,
        connection_id: &str,
        roles: &[MessagingAllowedRole],
    ) -> StorageResult<()> {
        let current = self.allowed_roles(connection_id)?;
        let kept: std::collections::HashSet<&str> = roles.iter().map(|r| r.id.as_str()).collect();
        let now = now_iso();
        self.database.with_tx(|conn| {
            for old in &current {
                if !kept.contains(old.id.as_str()) {
                    conn.execute(
                        "DELETE FROM projection_messaging_allowed_roles
                         WHERE connection_id = ?1 AND role_id = ?2",
                        params![connection_id, old.id],
                    )?;
                }
            }
            for role in roles {
                conn.execute(
                    "INSERT INTO projection_messaging_allowed_roles
                       (connection_id, role_id, role_name, created_at)
                     VALUES (?1, ?2, ?3, ?4)
                     ON CONFLICT(connection_id, role_id) DO UPDATE SET role_name = excluded.role_name",
                    params![connection_id, role.id, role.name, now],
                )?;
            }
            Ok(())
        })
    }

    pub fn allowed_senders(
        &self,
        connection_id: &str,
    ) -> StorageResult<Vec<MessagingAllowedSender>> {
        self.database.with_conn(|conn| {
            let mut stmt = conn.prepare_cached(
                "SELECT sender_id, kind, name, mode FROM projection_messaging_allowed_senders
                 WHERE connection_id = ?1 ORDER BY created_at, name, sender_id",
            )?;
            let rows = stmt.query_map(params![connection_id], |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    r.get::<_, String>(1)?,
                    r.get::<_, String>(2)?,
                    r.get::<_, String>(3)?,
                ))
            })?;
            let mut out = Vec::new();
            for row in rows {
                let (id, kind_str, name, mode) = row?;
                if let Some(kind) = MessagingSenderKind::parse(&kind_str) {
                    out.push(MessagingAllowedSender {
                        id,
                        kind,
                        name,
                        read_only: mode == "read",
                    });
                }
            }
            Ok(out)
        })
    }

    pub fn set_allowed_sender_read_only(
        &self,
        connection_id: &str,
        sender_id: &str,
        read_only: bool,
    ) -> StorageResult<bool> {
        let mode = if read_only { "read" } else { "reply" };
        self.database.with_conn(|conn| {
            let changes = conn.execute(
                "UPDATE projection_messaging_allowed_senders SET mode = ?1
                 WHERE connection_id = ?2 AND sender_id = ?3",
                params![mode, connection_id, sender_id],
            )?;
            Ok(changes > 0)
        })
    }

    pub fn add_allowed_sender(
        &self,
        connection_id: &str,
        sender_id: &str,
        kind: MessagingSenderKind,
        name: &str,
    ) -> StorageResult<()> {
        self.database.with_conn(|conn| {
            conn.execute(
                "INSERT INTO projection_messaging_allowed_senders
                   (connection_id, sender_id, kind, name, created_at)
                 VALUES (?1, ?2, ?3, ?4, ?5)
                 ON CONFLICT(connection_id, sender_id) DO UPDATE SET name = excluded.name",
                params![connection_id, sender_id, kind.as_str(), name, now_iso()],
            )?;
            Ok(())
        })
    }

    pub fn remove_allowed_sender(&self, connection_id: &str, sender_id: &str) -> StorageResult<()> {
        self.database.with_conn(|conn| {
            conn.execute(
                "DELETE FROM projection_messaging_allowed_senders
                 WHERE connection_id = ?1 AND sender_id = ?2",
                params![connection_id, sender_id],
            )?;
            Ok(())
        })
    }

    pub fn save_pings(&self, input: &MessagingPingInput) -> StorageResult<()> {
        let now = now_iso();
        self.database.with_tx(|conn| {
            let mut stmt = conn.prepare_cached(
                "INSERT INTO projection_messaging_pings
                   (connection_id, platform_channel_id, thread_key, user_id, agent_id, text, created_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)
                 ON CONFLICT(connection_id, platform_channel_id, thread_key, user_id)
                 DO UPDATE SET agent_id = excluded.agent_id, text = excluded.text, created_at = excluded.created_at",
            )?;
            for uid in &input.user_ids {
                stmt.execute(params![
                    input.connection_id,
                    input.platform_channel_id,
                    input.thread_key,
                    uid,
                    input.agent_id,
                    input.text,
                    now
                ])?;
            }
            Ok(())
        })
    }

    pub fn pings(
        &self,
        connection_id: &str,
        platform_channel_id: &str,
        thread_key: &str,
    ) -> StorageResult<Vec<MessagingPing>> {
        self.database.with_conn(|conn| {
            let mut stmt = conn.prepare_cached(
                "SELECT user_id, agent_id, text, created_at FROM projection_messaging_pings
                 WHERE connection_id = ?1 AND platform_channel_id = ?2 AND thread_key = ?3",
            )?;
            let rows = stmt.query_map(
                params![connection_id, platform_channel_id, thread_key],
                |r| {
                    Ok(MessagingPing {
                        user_id: r.get(0)?,
                        agent_id: r.get(1)?,
                        text: r.get(2)?,
                        created_at: r.get(3)?,
                    })
                },
            )?;
            Ok(rows.collect::<Result<Vec<_>, _>>()?)
        })
    }

    pub fn link(&self, link_id: &str) -> StorageResult<Option<MessagingLink>> {
        self.database.with_conn(|conn| {
            Ok(conn
                .prepare_cached(
                    "SELECT link_id, connection_id, agent_id, platform_channel_id, thread_key,
                            is_direct, thread_id, title, history_cursor, updated_at
                     FROM projection_messaging_threads WHERE link_id = ?1",
                )?
                .query_row(params![link_id], decode_link_row)
                .optional()?)
        })
    }

    pub fn link_by_key(
        &self,
        connection_id: &str,
        platform_channel_id: &str,
        thread_key: &str,
    ) -> StorageResult<Option<MessagingLink>> {
        self.database.with_conn(|conn| {
            Ok(conn
                .prepare_cached(
                    "SELECT link_id, connection_id, agent_id, platform_channel_id, thread_key,
                            is_direct, thread_id, title, history_cursor, updated_at
                     FROM projection_messaging_threads
                     WHERE connection_id = ?1 AND platform_channel_id = ?2 AND thread_key = ?3",
                )?
                .query_row(
                    params![connection_id, platform_channel_id, thread_key],
                    decode_link_row,
                )
                .optional()?)
        })
    }

    pub fn link_for_thread(&self, thread_id: &str) -> StorageResult<Option<MessagingLink>> {
        self.database.with_conn(|conn| {
            Ok(conn
                .prepare_cached(
                    "SELECT link_id, connection_id, agent_id, platform_channel_id, thread_key,
                            is_direct, thread_id, title, history_cursor, updated_at
                     FROM projection_messaging_threads WHERE thread_id = ?1",
                )?
                .query_row(params![thread_id], decode_link_row)
                .optional()?)
        })
    }

    pub fn links(&self, agent_id: &str) -> StorageResult<Vec<MessagingLink>> {
        self.database.with_conn(|conn| {
            let mut stmt = conn.prepare_cached(
                "SELECT link_id, connection_id, agent_id, platform_channel_id, thread_key,
                        is_direct, thread_id, title, history_cursor, updated_at
                 FROM projection_messaging_threads
                 WHERE agent_id = ?1 ORDER BY updated_at DESC, link_id LIMIT ?2",
            )?;
            let rows = stmt.query_map(
                params![agent_id, MESSAGING_LINK_LIST_LIMIT as i64],
                decode_link_row,
            )?;
            Ok(rows.collect::<Result<Vec<_>, _>>()?)
        })
    }

    pub fn execution_threads(&self) -> StorageResult<Vec<(String, String)>> {
        self.database.with_conn(|conn| {
            let mut stmt =
                conn.prepare_cached("SELECT agent_id, thread_id FROM projection_messaging_threads")?;
            let rows = stmt.query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?;
            Ok(rows.collect::<Result<Vec<_>, _>>()?)
        })
    }

    pub fn ensure_link(&self, input: &MessagingLinkInput) -> StorageResult<MessagingLink> {
        if let Some(existing) = self.link_by_key(
            &input.connection_id,
            &input.platform_channel_id,
            &input.thread_key,
        )? {
            return Ok(existing);
        }
        let link_id = format!("messaging-link-{}", Uuid::new_v4());
        let thread_id = format!("openbot-thread-{}", Uuid::new_v4());
        let now = now_iso();
        let events = [OrchestrationEventInput {
            event_id: Uuid::new_v4().to_string(),
            aggregate_type: "messaging-thread".to_string(),
            aggregate_id: link_id.clone(),
            event_type: "messaging.thread-created".to_string(),
            occurred_at: now.clone(),
            payload: serde_json::json!({ "agentId": input.agent_id, "threadId": thread_id }),
        }];
        let _ = self.database.dispatch_with_projection(
            &format!("messaging-thread:{link_id}"),
            None,
            &events,
            |conn, sequences| {
                let seq = sequences.first().copied().unwrap_or(0);
                conn.execute(
                    "INSERT INTO projection_threads VALUES (?1, ?2, ?3, NULL, ?4, ?4, ?5)",
                    params![thread_id, input.agent_id, input.title, now, seq],
                )?;
                conn.execute(
                    "INSERT INTO projection_messaging_threads
                       (link_id, connection_id, agent_id, platform_channel_id, thread_key,
                        is_direct, thread_id, title, history_cursor, created_at, updated_at)
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, NULL, ?9, ?9)",
                    params![
                        link_id,
                        input.connection_id,
                        input.agent_id,
                        input.platform_channel_id,
                        input.thread_key,
                        i64::from(input.is_direct),
                        thread_id,
                        input.title,
                        now
                    ],
                )?;
                Ok(serde_json::Value::Null)
            },
        )?;
        self.link(&link_id)?
            .ok_or_else(|| StorageError::NotFound("The messaging thread was not stored.".to_string()))
    }

    pub fn touch(&self, link_id: &str, history_cursor: Option<&str>) -> StorageResult<()> {
        let now = now_iso();
        self.database.with_conn(|conn| {
            match history_cursor {
                None => {
                    conn.execute(
                        "UPDATE projection_messaging_threads SET updated_at = ?1 WHERE link_id = ?2",
                        params![now, link_id],
                    )?;
                }
                Some(cursor) => {
                    conn.execute(
                        "UPDATE projection_messaging_threads
                         SET updated_at = ?1, history_cursor = ?2 WHERE link_id = ?3",
                        params![now, cursor, link_id],
                    )?;
                }
            }
            Ok(())
        })
    }

    pub fn delete_for_agent(&self, agent_id: &str) -> StorageResult<Vec<String>> {
        let (orchestrator_of, links): (Vec<String>, Vec<(String, String)>) =
            self.database.with_conn(|conn| {
                let orch: Vec<String> = conn
                    .prepare_cached(
                        "SELECT connection_id FROM projection_messaging_connections
                         WHERE orchestrator_agent_id = ?1",
                    )?
                    .query_map(params![agent_id], |r| r.get(0))?
                    .collect::<Result<Vec<_>, _>>()?;
                let lnks: Vec<(String, String)> = conn
                    .prepare_cached(
                        "SELECT link_id, thread_id FROM projection_messaging_threads WHERE agent_id = ?1",
                    )?
                    .query_map(params![agent_id], |r| Ok((r.get(0)?, r.get(1)?)))?
                    .collect::<Result<Vec<_>, _>>()?;
                Ok((orch, lnks))
            })?;

        let thread_ids: Vec<String> = links.iter().map(|(_, tid)| tid.clone()).collect();
        let link_ids: Vec<String> = links.iter().map(|(lid, _)| lid.clone()).collect();
        let events = [OrchestrationEventInput {
            event_id: Uuid::new_v4().to_string(),
            aggregate_type: "messaging-agent".to_string(),
            aggregate_id: agent_id.to_string(),
            event_type: "messaging.agent-deleted".to_string(),
            occurred_at: now_iso(),
            payload: serde_json::json!({ "threadIds": thread_ids }),
        }];
        let _ = self.database.dispatch_with_projection(
            &format!("messaging-agent-deleted:{agent_id}:{}", Uuid::new_v4()),
            None,
            &events,
            |conn, _seq| {
                delete_aggregate_history(conn, "messaging-thread", &link_ids)?;
                delete_aggregate_history(conn, "thread", &thread_ids)?;
                for tid in &thread_ids {
                    delete_thread_attachments(conn, tid)?;
                }
                conn.execute(
                    "DELETE FROM projection_messaging_pings WHERE EXISTS (
                       SELECT 1 FROM projection_messaging_threads AS link
                       WHERE link.agent_id = ?1
                         AND link.connection_id = projection_messaging_pings.connection_id
                         AND link.platform_channel_id = projection_messaging_pings.platform_channel_id
                         AND link.thread_key = projection_messaging_pings.thread_key
                     )",
                    params![agent_id],
                )?;
                conn.execute(
                    "DELETE FROM projection_messaging_pings WHERE agent_id = ?1 AND NOT EXISTS (
                       SELECT 1 FROM projection_messaging_threads AS link
                       WHERE link.connection_id = projection_messaging_pings.connection_id
                         AND link.platform_channel_id = projection_messaging_pings.platform_channel_id
                         AND link.thread_key = projection_messaging_pings.thread_key
                     )",
                    params![agent_id],
                )?;
                conn.execute(
                    "DELETE FROM projection_messaging_threads WHERE agent_id = ?1",
                    params![agent_id],
                )?;
                for tid in &thread_ids {
                    conn.execute(
                        "DELETE FROM projection_threads WHERE thread_id = ?1",
                        params![tid],
                    )?;
                }
                conn.execute(
                    "UPDATE projection_messaging_connections
                     SET orchestrator_agent_id = NULL WHERE orchestrator_agent_id = ?1",
                    params![agent_id],
                )?;
                Ok(serde_json::Value::Null)
            },
        )?;
        Ok(orchestrator_of)
    }

    pub fn restore_orchestrator(
        &self,
        agent_id: &str,
        connection_ids: &[String],
    ) -> StorageResult<()> {
        for cid in connection_ids {
            if let Some(rec) = self.connection(cid)?
                && rec.orchestrator_agent_id.is_none()
            {
                self.update_connection(
                    cid,
                    &MessagingConnectionPatch {
                        orchestrator_agent_id: Some(Some(agent_id.to_string())),
                        ..Default::default()
                    },
                )?;
            }
        }
        Ok(())
    }

    pub fn thread_ids_for_agent(&self, agent_id: &str) -> StorageResult<Vec<String>> {
        self.database.with_conn(|conn| {
            let mut stmt = conn.prepare_cached(
                "SELECT thread_id FROM projection_messaging_threads WHERE agent_id = ?1",
            )?;
            let rows = stmt.query_map(params![agent_id], |r| r.get(0))?;
            Ok(rows.collect::<Result<Vec<_>, _>>()?)
        })
    }
}

fn decode_connection_row(
    row: &rusqlite::Row<'_>,
) -> rusqlite::Result<Option<MessagingConnectionRecord>> {
    let platform_raw: String = row.get(1)?;
    let Some(platform) = MessagingPlatform::parse(&platform_raw) else {
        return Ok(None);
    };
    Ok(Some(MessagingConnectionRecord {
        connection_id: row.get(0)?,
        platform,
        workspace_id: row.get(2)?,
        workspace_name: row.get(3)?,
        enabled: row.get::<_, i64>(4)? == 1,
        bot_user_id: row.get(5)?,
        app_id: row.get(6)?,
        orchestrator_agent_id: row.get(7)?,
        last_error_code: row.get(8)?,
    }))
}

fn decode_link_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<MessagingLink> {
    Ok(MessagingLink {
        link_id: row.get(0)?,
        connection_id: row.get(1)?,
        agent_id: row.get(2)?,
        platform_channel_id: row.get(3)?,
        thread_key: row.get(4)?,
        is_direct: row.get::<_, i64>(5)? == 1,
        thread_id: row.get(6)?,
        title: row.get(7)?,
        history_cursor: row.get(8)?,
        updated_at: row.get(9)?,
    })
}

/// `MessagingHistoryStore` over `projection_messaging_history`
/// (`src/backend/messaging/messaging-history-store.ts`).
#[derive(Clone)]
pub struct MessagingHistoryStore {
    database: OpenBotDatabase,
}

impl MessagingHistoryStore {
    pub fn new(database: OpenBotDatabase) -> Self {
        Self { database }
    }

    pub fn save(
        &self,
        connection_id: &str,
        platform_channel_id: &str,
        thread_key: &str,
        author_id: &str,
        entry: &MessagingContextEntry,
        saved_at: &str,
    ) -> StorageResult<bool> {
        let files_json =
            serde_json::to_string(entry.files.as_deref().unwrap_or(&[]))?;
        self.database.with_conn(|conn| {
            let changes = conn.execute(
                "INSERT INTO projection_messaging_history
                   (connection_id, platform_channel_id, message_id, thread_key, author_id,
                    author_name, text, sent_at, files_json, saved_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)
                 ON CONFLICT(connection_id, platform_channel_id, message_id) DO NOTHING",
                params![
                    connection_id,
                    platform_channel_id,
                    entry.id,
                    thread_key,
                    author_id,
                    entry.author_name,
                    entry.text,
                    entry.sent_at,
                    files_json,
                    saved_at
                ],
            )?;
            Ok(changes > 0)
        })
    }

    pub fn between(
        &self,
        connection_id: &str,
        platform_channel_id: &str,
        thread_key: &str,
        after_id: Option<&str>,
        before_id: &str,
    ) -> StorageResult<Vec<MessagingContextEntry>> {
        self.database.with_conn(|conn| {
            let seq_of = |mid: &str| -> StorageResult<Option<i64>> {
                Ok(conn
                    .prepare_cached(
                        "SELECT seq FROM projection_messaging_history
                         WHERE connection_id = ?1 AND platform_channel_id = ?2
                           AND message_id = ?3 AND thread_key = ?4",
                    )?
                    .query_row(
                        params![connection_id, platform_channel_id, mid, thread_key],
                        |r| r.get(0),
                    )
                    .optional()?)
            };
            let after = match after_id {
                Some(aid) => seq_of(aid)?.unwrap_or(0),
                None => 0,
            };
            let before = seq_of(before_id)?.unwrap_or(i64::MAX);
            let mut stmt = conn.prepare_cached(
                "SELECT message_id, author_name, text, sent_at, files_json
                 FROM projection_messaging_history
                 WHERE connection_id = ?1 AND platform_channel_id = ?2 AND thread_key = ?3
                   AND seq > ?4 AND seq < ?5
                 ORDER BY seq DESC LIMIT ?6",
            )?;
            let mut entries = stmt
                .query_map(
                    params![
                        connection_id,
                        platform_channel_id,
                        thread_key,
                        after,
                        before,
                        MESSAGING_CONTEXT_MESSAGES as i64
                    ],
                    decode_context_entry_row,
                )?
                .collect::<Result<Vec<_>, _>>()?;
            entries.reverse();
            Ok(entries)
        })
    }

    pub fn latest(
        &self,
        connection_id: &str,
        platform_channel_id: &str,
        limit: usize,
    ) -> StorageResult<Vec<MessagingThreadedEntry>> {
        self.database.with_conn(|conn| {
            let mut stmt = conn.prepare_cached(
                "SELECT message_id, author_name, text, sent_at, files_json, thread_key
                 FROM projection_messaging_history
                 WHERE connection_id = ?1 AND platform_channel_id = ?2
                 ORDER BY seq DESC LIMIT ?3",
            )?;
            let mut rows = stmt
                .query_map(
                    params![connection_id, platform_channel_id, limit as i64],
                    |r| {
                        Ok(MessagingThreadedEntry {
                            entry: decode_context_entry_row(r)?,
                            thread_key: r.get(5)?,
                        })
                    },
                )?
                .collect::<Result<Vec<_>, _>>()?;
            rows.reverse();
            Ok(rows)
        })
    }

    pub fn recent(
        &self,
        connection_id: &str,
        platform_channel_id: &str,
        thread_key: Option<&str>,
        limit: usize,
        filter: &MessagingHistoryFilter,
    ) -> StorageResult<Vec<MessagingContextEntry>> {
        self.database.with_conn(|conn| {
            let mut stmt = conn.prepare_cached(
                "SELECT message_id, author_name, text, sent_at, files_json
                 FROM projection_messaging_history
                 WHERE connection_id = ?1 AND platform_channel_id = ?2
                   AND (?3 IS NULL OR thread_key = ?3)
                   AND (?4 IS NULL OR sent_at < ?4)
                 ORDER BY sent_at DESC, seq DESC",
            )?;
            let rows = stmt.query_map(
                params![connection_id, platform_channel_id, thread_key, filter.before],
                decode_context_entry_row,
            )?;
            let mut found: Vec<MessagingContextEntry> = Vec::new();
            for row in rows {
                let msg = row?;
                if found.len() >= limit
                    && found.last().is_some_and(|last| msg.sent_at != last.sent_at)
                {
                    break;
                }
                if matches_history_filter(&msg, filter) {
                    found.push(msg);
                }
            }
            found.reverse();
            Ok(found)
        })
    }

    pub fn save_file_path(
        &self,
        connection_id: &str,
        platform_channel_id: &str,
        message_id: &str,
        file_id: &str,
        path: &str,
        size: u64,
    ) -> StorageResult<bool> {
        let Some(mut files) = self.read_files(connection_id, platform_channel_id, message_id)?
        else {
            return Ok(false);
        };
        if !files.iter().any(|f| f.id == file_id) {
            return Ok(false);
        }
        for f in &mut files {
            if f.id == file_id {
                f.path = Some(path.to_string());
                f.size = size;
            }
        }
        let updated_json = serde_json::to_string(&files)?;
        self.database.with_conn(|conn| {
            let changes = conn.execute(
                "UPDATE projection_messaging_history SET files_json = ?1
                 WHERE connection_id = ?2 AND platform_channel_id = ?3 AND message_id = ?4",
                params![updated_json, connection_id, platform_channel_id, message_id],
            )?;
            Ok(changes > 0)
        })
    }

    pub fn saved_file(
        &self,
        connection_id: &str,
        platform_channel_id: &str,
        message_id: &str,
        file_id: &str,
    ) -> StorageResult<Option<String>> {
        Ok(self
            .read_files(connection_id, platform_channel_id, message_id)?
            .and_then(|files| {
                files
                    .into_iter()
                    .find(|f| f.id == file_id)
                    .and_then(|f| f.path)
            }))
    }

    pub fn prune(&self, saved_before: &str) -> StorageResult<Vec<String>> {
        self.database.with_conn(|conn| {
            let mut stmt = conn.prepare_cached(
                "DELETE FROM projection_messaging_history
                 WHERE saved_at < ?1 RETURNING files_json",
            )?;
            let rows = stmt.query_map(params![saved_before], |r| r.get::<_, String>(0))?;
            collect_file_paths(rows)
        })
    }

    pub fn forget_message(
        &self,
        connection_id: &str,
        platform_channel_id: &str,
        message_id: &str,
        author_id: &str,
    ) -> StorageResult<Vec<String>> {
        self.database.with_conn(|conn| {
            let mut stmt = conn.prepare_cached(
                "DELETE FROM projection_messaging_history
                 WHERE connection_id = ?1 AND platform_channel_id = ?2
                   AND message_id = ?3 AND author_id = ?4
                 RETURNING files_json",
            )?;
            let rows = stmt.query_map(
                params![connection_id, platform_channel_id, message_id, author_id],
                |r| r.get::<_, String>(0),
            )?;
            collect_file_paths(rows)
        })
    }

    pub fn edit_message(
        &self,
        connection_id: &str,
        platform_channel_id: &str,
        message_id: &str,
        author_id: &str,
        text: &str,
    ) -> StorageResult<()> {
        self.database.with_conn(|conn| {
            conn.execute(
                "UPDATE projection_messaging_history SET text = ?1
                 WHERE connection_id = ?2 AND platform_channel_id = ?3
                   AND message_id = ?4 AND author_id = ?5",
                params![text, connection_id, platform_channel_id, message_id, author_id],
            )?;
            Ok(())
        })
    }

    /// FIFO file eviction ordered by `seq ASC` until `total <= max_bytes`
    /// (`messaging-history-store.ts:L195-L220`).
    pub fn trim_files(&self, connection_id: &str, max_bytes: u64) -> StorageResult<Vec<String>> {
        self.database.with_tx(|conn| {
            let rows: Vec<(i64, Vec<InboundFile>)> = {
                let mut stmt = conn.prepare_cached(
                    "SELECT seq, files_json FROM projection_messaging_history
                     WHERE connection_id = ?1 AND files_json LIKE '%\"path\":%'
                     ORDER BY seq",
                )?;
                let iter = stmt.query_map(params![connection_id], |r| {
                    let seq: i64 = r.get(0)?;
                    let fj: String = r.get(1)?;
                    Ok((seq, decode_inbound_files(&fj)))
                })?;
                iter.collect::<Result<Vec<_>, _>>()?
            };
            let saved_bytes = |files: &[InboundFile]| -> u64 {
                files
                    .iter()
                    .map(|f| if f.path.is_some() { f.size } else { 0 })
                    .sum()
            };
            let mut total: u64 = rows.iter().map(|(_, f)| saved_bytes(f)).sum();
            let mut evicted = Vec::new();
            let mut update = conn.prepare_cached(
                "UPDATE projection_messaging_history SET files_json = ?1 WHERE seq = ?2",
            )?;
            for (seq, mut files) in rows {
                if total <= max_bytes {
                    break;
                }
                total = total.saturating_sub(saved_bytes(&files));
                for f in &mut files {
                    if let Some(p) = f.path.take() {
                        evicted.push(p);
                    }
                }
                update.execute(params![serde_json::to_string(&files)?, seq])?;
            }
            Ok(evicted)
        })
    }

    pub fn forget(&self, connection_id: &str) -> StorageResult<Vec<String>> {
        self.database.with_conn(|conn| {
            let mut stmt = conn.prepare_cached(
                "DELETE FROM projection_messaging_history
                 WHERE connection_id = ?1 RETURNING files_json",
            )?;
            let rows = stmt.query_map(params![connection_id], |r| r.get::<_, String>(0))?;
            collect_file_paths(rows)
        })
    }

    fn read_files(
        &self,
        connection_id: &str,
        platform_channel_id: &str,
        message_id: &str,
    ) -> StorageResult<Option<Vec<InboundFile>>> {
        self.database.with_conn(|conn| {
            let raw_opt: Option<String> = conn
                .prepare_cached(
                    "SELECT files_json FROM projection_messaging_history
                     WHERE connection_id = ?1 AND platform_channel_id = ?2 AND message_id = ?3",
                )?
                .query_row(
                    params![connection_id, platform_channel_id, message_id],
                    |r| r.get(0),
                )
                .optional()?;
            Ok(raw_opt.map(|raw| decode_inbound_files(&raw)))
        })
    }
}

pub fn matches_history_filter(
    entry: &MessagingContextEntry,
    filter: &MessagingHistoryFilter,
) -> bool {
    if let Some(before) = &filter.before {
        let entry_ts = chrono::DateTime::parse_from_rfc3339(&entry.sent_at).ok();
        let before_ts = chrono::DateTime::parse_from_rfc3339(before).ok();
        match (entry_ts, before_ts) {
            (Some(et), Some(bt)) if et < bt => {}
            _ if entry.sent_at < *before => {}
            _ => return false,
        }
    }
    let haystack = format!("{}\n{}", entry.author_name, entry.text).to_lowercase();
    filter
        .query
        .as_deref()
        .unwrap_or("")
        .split_whitespace()
        .take(MESSAGING_QUERY_WORDS)
        .all(|word| haystack.contains(&word.to_lowercase()))
}

fn decode_context_entry_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<MessagingContextEntry> {
    let files_raw: String = row.get(4)?;
    let files = decode_inbound_files(&files_raw);
    Ok(MessagingContextEntry {
        id: row.get(0)?,
        author_name: row.get(1)?,
        text: row.get(2)?,
        sent_at: row.get(3)?,
        files: (!files.is_empty()).then_some(files),
    })
}

fn decode_inbound_files(json: &str) -> Vec<InboundFile> {
    let Ok(serde_json::Value::Array(arr)) = serde_json::from_str::<serde_json::Value>(json) else {
        return Vec::new();
    };
    arr.into_iter()
        .filter_map(|v| serde_json::from_value::<InboundFile>(v).ok())
        .collect()
}

fn collect_file_paths<I>(rows: I) -> StorageResult<Vec<String>>
where
    I: Iterator<Item = rusqlite::Result<String>>,
{
    let mut paths = Vec::new();
    for raw in rows {
        for f in decode_inbound_files(&raw?) {
            if let Some(p) = f.path {
                paths.push(p);
            }
        }
    }
    Ok(paths)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    struct ScopedTestDir(PathBuf);

    impl ScopedTestDir {
        fn new(prefix: &str) -> Self {
            let dir = std::env::temp_dir().join(format!("{prefix}_{}", Uuid::new_v4()));
            std::fs::create_dir_all(&dir).expect("create temp dir");
            Self(dir)
        }
    }

    impl Drop for ScopedTestDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn team_chat_and_messaging_stores_roundtrip_threads_reads_modes_and_fifo_eviction() {
        let tid_ab = direct_thread_id("member-b", "member-a");
        let tid_ba = direct_thread_id("member-a", "member-b");
        assert_eq!(tid_ab, tid_ba);
        assert_eq!(tid_ab.len(), 35);
        assert!(tid_ab.starts_with("dm-"));

        let tmp = ScopedTestDir::new("openbot_team_chat");
        let db = OpenBotDatabase::open(tmp.0.join("data").join("openbot.db")).unwrap();
        let chat = TeamChatStore::new(db.clone());

        assert!(chat
            .send_message(&SendDirectMessageInput {
                client_message_id: "self-1".to_string(),
                sender_member_id: "alice".to_string(),
                recipient_member_id: "alice".to_string(),
                text: "hi".to_string(),
                created_at: None,
            })
            .is_err());

        let m1 = chat
            .send_message(&SendDirectMessageInput {
                client_message_id: "dm-msg-1".to_string(),
                sender_member_id: "alice".to_string(),
                recipient_member_id: "bob".to_string(),
                text: "Hey Bob!".to_string(),
                created_at: Some("2026-11-25T01:00:00.000Z".to_string()),
            })
            .unwrap();
        let m2 = chat
            .send_message(&SendDirectMessageInput {
                client_message_id: "dm-msg-2".to_string(),
                sender_member_id: "alice".to_string(),
                recipient_member_id: "bob".to_string(),
                text: "Ready to ship?".to_string(),
                created_at: Some("2026-11-25T01:01:00.000Z".to_string()),
            })
            .unwrap();
        assert!(m2.sequence > m1.sequence);

        let bob_threads = chat.list_threads("bob").unwrap();
        assert_eq!(bob_threads.len(), 1);
        assert_eq!(bob_threads[0].unread_count, 2);
        assert_eq!(bob_threads[0].other_member_id, "alice");

        let page1 = chat
            .read_conversation_page("bob", "alice", &DirectConversationPageAnchor::Latest, 1)
            .unwrap();
        assert_eq!(page1.messages.len(), 1);
        assert!(page1.page_info.has_older);
        let page2 = chat
            .read_conversation_page(
                "bob",
                "alice",
                &DirectConversationPageAnchor::Before {
                    cursor: page1.page_info.older_cursor.unwrap(),
                },
                10,
            )
            .unwrap();
        assert_eq!(page2.messages.len(), 1);
        assert_eq!(page2.messages[0].id, "dm-msg-1");

        let read_state = chat.mark_read("bob", "alice", m2.sequence).unwrap();
        assert_eq!(read_state.unread_count, 0);

        let msg_store = MessagingStore::new(db.clone());
        let conn_rec = msg_store
            .ensure_connection(MessagingPlatform::Telegram, "tg-ws-1", "Telegram Bot")
            .unwrap();
        msg_store
            .add_allowed_sender(&conn_rec.connection_id, "u-1", MessagingSenderKind::Contact, "Sam")
            .unwrap();
        assert!(!msg_store.allowed_senders(&conn_rec.connection_id).unwrap()[0].read_only);
        assert!(msg_store
            .set_allowed_sender_read_only(&conn_rec.connection_id, "u-1", true)
            .unwrap());
        assert!(msg_store.allowed_senders(&conn_rec.connection_id).unwrap()[0].read_only);

        let hist = MessagingHistoryStore::new(db);
        for idx in 1..=2 {
            hist.save(
                &conn_rec.connection_id,
                "chat-1",
                "main",
                "u-1",
                &MessagingContextEntry {
                    id: format!("hm-{idx}"),
                    author_name: "Sam".to_string(),
                    text: format!("file {idx}"),
                    sent_at: format!("2026-11-25T02:0{idx}:00.000Z"),
                    files: Some(vec![InboundFile {
                        id: format!("f-{idx}"),
                        name: format!("doc{idx}.pdf"),
                        mime_type: "application/pdf".to_string(),
                        size: 100,
                        url: "https://example.invalid".to_string(),
                        path: Some(format!("/managed/f-{idx}.pdf")),
                    }]),
                },
                "2026-11-25T02:05:00.000Z",
            )
            .unwrap();
        }
        let evicted = hist.trim_files(&conn_rec.connection_id, 100).unwrap();
        assert_eq!(evicted, vec!["/managed/f-1.pdf".to_string()]);
        assert!(hist
            .saved_file(&conn_rec.connection_id, "chat-1", "hm-1", "f-1")
            .unwrap()
            .is_none());
        assert_eq!(
            hist.saved_file(&conn_rec.connection_id, "chat-1", "hm-2", "f-2")
                .unwrap()
                .as_deref(),
            Some("/managed/f-2.pdf")
        );
    }
}
