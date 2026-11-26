//! External chat bridge orchestrator (`OB-F13`): WhatsApp sender ACLs (`<= 200`) + Brazilian
//! 9th-digit normalization + pairing requests (`<= 3` pending, 1h TTL), Discord role ACLs (`<= 50`),
//! Slack workspace/channel routing, Telegram bidirectional hook & dispatch, `2_000`-entry dedup cache,
//! and `MessagingStore` / `MessagingHistoryStore` persistence (`<= 32` connections).
//!
//! Mirrors `src/backend/messaging/{messaging-service,whatsapp/whatsapp-senders,discord/discord-driver}.ts`
//! and `packages/contracts/src/ipc-messaging.ts`.

use std::collections::{HashMap, HashSet, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::{Arc, LazyLock};

use chrono::{SecondsFormat, TimeZone, Utc};
use openbot_contracts::limits::{is_bounded_string, is_identifier, utf16_len};
use openbot_storage::{
    MessagingAllowedRole, MessagingConnectionRecord, MessagingContextEntry, MessagingHistoryStore,
    MessagingLink, MessagingLinkInput, MessagingPlatform, MessagingSenderKind, MessagingStore,
    OpenBotDatabase,
};
use parking_lot::Mutex;
use regex::Regex;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::error::{OpenBotRuntimeError, RuntimeResult};

pub const MESSAGING_LIMIT_NAME: usize = 256;
pub const MESSAGING_LIMIT_CONNECTIONS: usize = 32;
pub const MESSAGING_LIMIT_ALLOWED_ROLES: usize = 50;
pub const MESSAGING_LIMIT_ALLOWED_SENDERS: usize = 200;
pub const MESSAGING_LIMIT_PENDING_REQUESTS: usize = 3;
pub const MESSAGING_LIMIT_ROLE_NAME: usize = 100;
pub const WHATSAPP_PAIRING_TTL_MS: i64 = 60 * 60 * 1_000;
pub const MESSAGING_DEDUP_CACHE_LIMIT: usize = 2_000;

const PAIRING_CODE_ALPHABET: &[u8; 32] = b"ABCDEFGHJKLMNPQRSTUVWXYZ23456789";

static BRAZIL_NUMBER_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^55([1-9]{2})(9?)([6-9][0-9]{7})$").expect("valid brazil whatsapp regex")
});

static DISCORD_ROLE_ID_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^[0-9]{1,24}$").expect("valid discord role id regex"));

static WHATSAPP_NUMBER_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^[0-9]{5,20}$").expect("valid whatsapp number regex"));

static WHATSAPP_GROUP_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^[0-9-]{5,64}@g\.us$").expect("valid whatsapp group regex"));

static WHATSAPP_CODE_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^[A-Z0-9]{6}$").expect("valid whatsapp pairing code regex"));

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum WhatsAppDecision {
    Allow,
    Pair,
    Save,
    Ignore,
}

impl WhatsAppDecision {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Allow => "allow",
            Self::Pair => "pair",
            Self::Save => "save",
            Self::Ignore => "ignore",
        }
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WhatsAppSenderRules {
    pub own_number: bool,
    pub contacts: Vec<String>,
    pub groups: Vec<String>,
    pub read_only: Vec<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WhatsAppSenderFacts {
    pub is_self_chat: bool,
    pub is_group: bool,
    pub from_me: bool,
    pub chat_jid: String,
    pub contact_number: Option<String>,
    pub author_number: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BridgeEvalRequest {
    pub platform: String, // "whatsapp" | "discord" | "slack" | "telegram"
    pub workspace_id: Option<String>,
    pub sender: Option<String>,
    pub text: Option<String>,
    pub is_group: bool,
    pub is_self_chat: bool,
    pub from_me: bool,
    pub own_number: Option<bool>,
    pub allowed_senders: Option<Vec<serde_json::Value>>,
    pub pending_requests_count: Option<usize>,
    pub allowed_roles: Option<Vec<serde_json::Value>>,
    pub author_roles: Option<Vec<String>>,
    pub channel_id: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BridgeEvalOutcome {
    pub platform: String,
    pub decision: String, // "allow" | "save" | "pair" | "ignore"
    pub allowed: bool,
    pub pairing_allowed: Option<bool>,
    pub pairing_code: Option<String>,
    pub pending_requests_capped: Option<bool>,
    pub normalized_forms: Option<Vec<String>>,
    pub dedup_key: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WhatsAppPairingRequest {
    pub code: String,
    pub sender_id: String,
    pub name: String,
    pub expires_at: String,
    pub expires_at_ms: i64,
}

#[derive(Debug, Default)]
pub struct WhatsAppPairingRequests {
    by_workspace: HashMap<String, Vec<WhatsAppPairingRequest>>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum TelegramBridgeDirection {
    Inbound,
    Outbound,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TelegramBridgeEvent {
    pub direction: TelegramBridgeDirection,
    pub chat_id: String,
    pub sender_id: Option<String>,
    pub text: String,
    pub timestamp: String,
}

pub type TelegramBridgeHook = Arc<dyn Fn(&TelegramBridgeEvent) + Send + Sync>;

#[derive(Debug, Default)]
struct DedupCache {
    seen: HashSet<String>,
    order: VecDeque<String>,
}

pub struct ExternalBridgeOrchestrator {
    database: OpenBotDatabase,
    data_dir: PathBuf,
    pairing: Mutex<WhatsAppPairingRequests>,
    dedup: Mutex<DedupCache>,
    telegram_hook: Mutex<Option<TelegramBridgeHook>>,
}

impl WhatsAppPairingRequests {
    pub fn list(&mut self, workspace_id: &str) -> Vec<WhatsAppPairingRequest> {
        self.list_at(workspace_id, Utc::now().timestamp_millis())
    }

    pub fn list_at(&mut self, workspace_id: &str, now_ms: i64) -> Vec<WhatsAppPairingRequest> {
        let entry = self
            .by_workspace
            .entry(workspace_id.to_string())
            .or_default();
        entry.retain(|req| req.expires_at_ms > now_ms);
        entry.clone()
    }

    pub fn request(
        &mut self,
        workspace_id: &str,
        sender_id: &str,
        name: &str,
    ) -> Option<(String, bool)> {
        self.request_at(workspace_id, sender_id, name, Utc::now().timestamp_millis())
    }

    pub fn request_at(
        &mut self,
        workspace_id: &str,
        sender_id: &str,
        name: &str,
        now_ms: i64,
    ) -> Option<(String, bool)> {
        let live = self.list_at(workspace_id, now_ms);
        if let Some(existing) = live.iter().find(|req| req.sender_id == sender_id) {
            return Some((existing.code.clone(), false));
        }
        if live.len() >= MESSAGING_LIMIT_PENDING_REQUESTS {
            return None;
        }
        let code = generate_pairing_code();
        let expires_at_ms = now_ms + WHATSAPP_PAIRING_TTL_MS;
        let expires_at = Utc
            .timestamp_millis_opt(expires_at_ms)
            .single()
            .unwrap_or_else(Utc::now)
            .to_rfc3339_opts(SecondsFormat::Millis, true);
        let clipped_name: String = name.trim().chars().take(MESSAGING_LIMIT_NAME).collect();
        let display_name = if clipped_name.is_empty() {
            sender_id.to_string()
        } else {
            clipped_name
        };
        let req = WhatsAppPairingRequest {
            code: code.clone(),
            sender_id: sender_id.to_string(),
            name: display_name,
            expires_at,
            expires_at_ms,
        };
        self.by_workspace
            .entry(workspace_id.to_string())
            .or_default()
            .push(req);
        Some((code, true))
    }

    pub fn take(&mut self, workspace_id: &str, code: &str) -> Option<WhatsAppPairingRequest> {
        self.take_at(workspace_id, code, Utc::now().timestamp_millis())
    }

    pub fn take_at(
        &mut self,
        workspace_id: &str,
        code: &str,
        now_ms: i64,
    ) -> Option<WhatsAppPairingRequest> {
        let entry = self
            .by_workspace
            .entry(workspace_id.to_string())
            .or_default();
        entry.retain(|req| req.expires_at_ms > now_ms);
        let pos = entry.iter().position(|req| req.code == code)?;
        Some(entry.remove(pos))
    }

    pub fn clear(&mut self, workspace_id: &str) {
        self.by_workspace.remove(workspace_id);
    }
}

fn generate_pairing_code() -> String {
    let mut bytes = [0u8; 6];
    if getrandom::getrandom(&mut bytes).is_err() {
        let fallback = uuid::Uuid::new_v4();
        bytes.copy_from_slice(&fallback.as_bytes()[..6]);
    }
    bytes
        .into_iter()
        .map(|b| PAIRING_CODE_ALPHABET[(b & 31) as usize] as char)
        .collect()
}

impl DedupCache {
    fn record(&mut self, key: String) -> bool {
        if self.seen.contains(&key) {
            return true;
        }
        while self.order.len() >= MESSAGING_DEDUP_CACHE_LIMIT {
            if let Some(evicted) = self.order.pop_front() {
                self.seen.remove(&evicted);
            }
        }
        self.seen.insert(key.clone());
        self.order.push_back(key);
        false
    }
}

impl ExternalBridgeOrchestrator {
    pub fn open(database: OpenBotDatabase, data_dir: impl AsRef<Path>) -> RuntimeResult<Arc<Self>> {
        Ok(Arc::new(Self {
            database,
            data_dir: data_dir.as_ref().to_path_buf(),
            pairing: Mutex::new(WhatsAppPairingRequests::default()),
            dedup: Mutex::new(DedupCache::default()),
            telegram_hook: Mutex::new(None),
        }))
    }

    pub fn data_dir(&self) -> &Path {
        &self.data_dir
    }

    /// Brazilian 9th-digit normalization returning both 12- and 13-digit forms
    /// (`src/backend/messaging/whatsapp/whatsapp-senders.ts:L50-L55`).
    pub fn whatsapp_number_forms(number: &str) -> Vec<String> {
        let Some(caps) = BRAZIL_NUMBER_RE.captures(number) else {
            return vec![number.to_string()];
        };
        let area = caps.get(1).map_or("", |m| m.as_str());
        let nine = caps.get(2).map_or("", |m| m.as_str());
        let local = caps.get(3).map_or("", |m| m.as_str());
        let alt = if nine.is_empty() {
            format!("55{area}9{local}")
        } else {
            format!("55{area}{local}")
        };
        vec![number.to_string(), alt]
    }

    /// Exact 1:1 port of `senderDecision` (`whatsapp-senders.ts:L25-L38`).
    pub fn whatsapp_sender_decision(
        rules: &WhatsAppSenderRules,
        facts: &WhatsAppSenderFacts,
    ) -> WhatsAppDecision {
        if facts.is_self_chat {
            return WhatsAppDecision::Allow;
        }
        let allowed_group = facts.is_group && rules.groups.iter().any(|g| g == &facts.chat_jid);
        let allowed_contact = !facts.is_group
            && facts
                .contact_number
                .as_deref()
                .is_some_and(|num| allows_whatsapp_contact(rules, num));
        if facts.from_me {
            return if allowed_group || allowed_contact {
                WhatsAppDecision::Save
            } else {
                WhatsAppDecision::Ignore
            };
        }
        if facts.is_group {
            if !allowed_group {
                return WhatsAppDecision::Ignore;
            }
            return if rules.read_only.iter().any(|r| r == &facts.chat_jid) {
                WhatsAppDecision::Save
            } else {
                WhatsAppDecision::Allow
            };
        }
        if let Some(author) = facts.author_number.as_deref().filter(|s| !s.is_empty())
            && allows_whatsapp_contact(rules, author)
        {
            return if is_read_only_whatsapp_contact(rules, author) {
                WhatsAppDecision::Save
            } else {
                WhatsAppDecision::Allow
            };
        }
        if rules.own_number || facts.author_number.as_deref().is_none_or(str::is_empty) {
            return WhatsAppDecision::Ignore;
        }
        WhatsAppDecision::Pair
    }

    pub fn is_read_only_whatsapp_chat(rules: &WhatsAppSenderRules, chat_jid: &str) -> bool {
        let mut parts = chat_jid.splitn(2, '@');
        let user = parts.next().unwrap_or("");
        let domain = parts.next().unwrap_or("");
        if domain == "g.us" {
            return rules.read_only.iter().any(|r| r == chat_jid);
        }
        let number = user.split(':').next().unwrap_or("");
        is_read_only_whatsapp_contact(rules, number)
    }
}

impl ExternalBridgeOrchestrator {
    pub fn register_telegram_hook(&self, hook: TelegramBridgeHook) {
        *self.telegram_hook.lock() = Some(hook);
    }

    pub fn clear_telegram_hook(&self) {
        *self.telegram_hook.lock() = None;
    }

    pub fn dispatch_telegram_outbound(&self, chat_id: &str, text: &str) -> RuntimeResult<bool> {
        let trimmed_chat = chat_id.trim();
        if trimmed_chat.is_empty() || !is_bounded_string(trimmed_chat, MESSAGING_LIMIT_NAME) {
            return Err(OpenBotRuntimeError::Validation(
                "Invalid Telegram chat id.".to_string(),
            ));
        }
        let hook = self.telegram_hook.lock().clone();
        if let Some(cb) = hook {
            let event = TelegramBridgeEvent {
                direction: TelegramBridgeDirection::Outbound,
                chat_id: trimmed_chat.to_string(),
                sender_id: None,
                text: text.to_string(),
                timestamp: Utc::now().to_rfc3339_opts(SecondsFormat::Millis, true),
            };
            cb(&event);
            Ok(true)
        } else {
            Ok(false)
        }
    }

    pub fn record_inbound_dedup(&self, connection_id: &str, message_key: &str) -> (String, bool) {
        let full_key = format!("messaging:{connection_id}:{message_key}");
        let duplicate = self.dedup.lock().record(full_key.clone());
        (full_key, duplicate)
    }

    pub fn ensure_connection(
        &self,
        platform: MessagingPlatform,
        workspace_id: &str,
        workspace_name: &str,
    ) -> RuntimeResult<MessagingConnectionRecord> {
        let ws_id = workspace_id.trim();
        if !is_identifier(ws_id) {
            return Err(OpenBotRuntimeError::Validation(
                "Invalid messaging workspaceId.".to_string(),
            ));
        }
        let ws_name = workspace_name.trim();
        if ws_name.is_empty() || !is_bounded_string(ws_name, MESSAGING_LIMIT_NAME) {
            return Err(OpenBotRuntimeError::Validation(
                "Invalid messaging workspaceName.".to_string(),
            ));
        }
        let store = MessagingStore::new(self.database.clone());
        if let Some(existing) = store.connection_for_workspace(platform, ws_id)? {
            return Ok(existing);
        }
        let all = store.connections()?;
        if all.len() >= MESSAGING_LIMIT_CONNECTIONS {
            return Err(OpenBotRuntimeError::LimitExceeded(format!(
                "Messaging connections limit of {MESSAGING_LIMIT_CONNECTIONS} reached."
            )));
        }
        Ok(store.ensure_connection(platform, ws_id, ws_name)?)
    }

    pub fn ensure_link(&self, input: &MessagingLinkInput) -> RuntimeResult<MessagingLink> {
        Ok(MessagingStore::new(self.database.clone()).ensure_link(input)?)
    }

    pub fn record_history(
        &self,
        connection_id: &str,
        platform_channel_id: &str,
        thread_key: &str,
        author_id: &str,
        entry: &MessagingContextEntry,
    ) -> RuntimeResult<bool> {
        let saved_at = Utc::now().to_rfc3339_opts(SecondsFormat::Millis, true);
        Ok(MessagingHistoryStore::new(self.database.clone()).save(
            connection_id,
            platform_channel_id,
            thread_key,
            author_id,
            entry,
            &saved_at,
        )?)
    }

    pub fn pending_whatsapp_pairings(&self, workspace_id: &str) -> Vec<WhatsAppPairingRequest> {
        self.pairing.lock().list(workspace_id)
    }

    pub fn answer_whatsapp_pairing(
        &self,
        workspace_id: &str,
        code: &str,
        approve: bool,
    ) -> RuntimeResult<bool> {
        let ws_id = workspace_id.trim();
        if !is_identifier(ws_id) {
            return Err(OpenBotRuntimeError::Validation(
                "Invalid WhatsApp workspaceId.".to_string(),
            ));
        }
        let normalized_code = code.trim().to_ascii_uppercase();
        if !WHATSAPP_CODE_RE.is_match(&normalized_code) {
            return Err(OpenBotRuntimeError::Validation(
                "Invalid WhatsApp pairing code.".to_string(),
            ));
        }
        let Some(pending) = self.pairing.lock().take(ws_id, &normalized_code) else {
            return Ok(false);
        };
        if !approve {
            return Ok(true);
        }
        let conn = self.ensure_connection(MessagingPlatform::Whatsapp, ws_id, ws_id)?;
        let store = MessagingStore::new(self.database.clone());
        let existing = store.allowed_senders(&conn.connection_id)?;
        if !existing.iter().any(|s| s.id == pending.sender_id)
            && existing.len() >= MESSAGING_LIMIT_ALLOWED_SENDERS
        {
            return Err(OpenBotRuntimeError::LimitExceeded(format!(
                "WhatsApp allowed senders limit of {MESSAGING_LIMIT_ALLOWED_SENDERS} reached."
            )));
        }
        store.add_allowed_sender(
            &conn.connection_id,
            &pending.sender_id,
            MessagingSenderKind::Contact,
            &pending.name,
        )?;
        Ok(true)
    }

    pub fn eval_bridge(&self, req: BridgeEvalRequest) -> RuntimeResult<BridgeEvalOutcome> {
        let platform_str = req.platform.trim().to_ascii_lowercase();
        let platform = MessagingPlatform::parse(&platform_str).ok_or_else(|| {
            OpenBotRuntimeError::Validation(format!("Unknown messaging platform: {}", req.platform))
        })?;
        match platform {
            MessagingPlatform::Whatsapp => self.eval_whatsapp(req),
            MessagingPlatform::Discord => self.eval_discord(req),
            MessagingPlatform::Slack => self.eval_slack(req),
            MessagingPlatform::Telegram => self.eval_telegram(req),
        }
    }
}

impl ExternalBridgeOrchestrator {
    fn eval_whatsapp(&self, req: BridgeEvalRequest) -> RuntimeResult<BridgeEvalOutcome> {
        let workspace_id = req
            .workspace_id
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .unwrap_or("whatsapp-default");
        if !is_identifier(workspace_id) {
            return Err(OpenBotRuntimeError::Validation(
                "Invalid WhatsApp workspaceId.".to_string(),
            ));
        }

        let mut rules = if let Some(ref raw_senders) = req.allowed_senders {
            parse_whatsapp_allowed_senders(raw_senders)?
        } else {
            let store = MessagingStore::new(self.database.clone());
            let mut r = WhatsAppSenderRules::default();
            if let Some(conn) =
                store.connection_for_workspace(MessagingPlatform::Whatsapp, workspace_id)?
            {
                for sender in store.allowed_senders(&conn.connection_id)? {
                    match sender.kind {
                        MessagingSenderKind::Contact => r.contacts.push(sender.id.clone()),
                        MessagingSenderKind::Group => r.groups.push(sender.id.clone()),
                    }
                    if sender.read_only {
                        r.read_only.push(sender.id);
                    }
                }
            }
            r
        };
        rules.own_number = req.own_number.unwrap_or(false);

        let cleaned_sender = req
            .sender
            .as_deref()
            .filter(|s| !s.trim().ends_with("@g.us"))
            .and_then(normalize_whatsapp_number);
        let chat_jid = if req.is_group {
            req.channel_id
                .as_deref()
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .map(str::to_string)
                .or_else(|| {
                    req.sender
                        .as_deref()
                        .map(str::trim)
                        .filter(|s| s.ends_with("@g.us"))
                        .map(str::to_string)
                })
                .unwrap_or_default()
        } else {
            req.channel_id
                .as_deref()
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .map(str::to_string)
                .unwrap_or_else(|| {
                    cleaned_sender
                        .as_deref()
                        .map(|num| format!("{num}@s.whatsapp.net"))
                        .unwrap_or_default()
                })
        };

        let facts = WhatsAppSenderFacts {
            is_self_chat: req.is_self_chat,
            is_group: req.is_group,
            from_me: req.from_me,
            chat_jid: chat_jid.clone(),
            contact_number: if req.is_group {
                None
            } else {
                cleaned_sender.clone()
            },
            author_number: if req.from_me {
                None
            } else {
                cleaned_sender.clone()
            },
        };

        let normalized_forms = cleaned_sender.as_deref().map(Self::whatsapp_number_forms);
        let decision = Self::whatsapp_sender_decision(&rules, &facts);

        let (pairing_allowed, pairing_code, pending_requests_capped) =
            if decision == WhatsAppDecision::Pair {
                let capped_by_input = req
                    .pending_requests_count
                    .is_some_and(|count| count >= MESSAGING_LIMIT_PENDING_REQUESTS);
                if capped_by_input {
                    (Some(false), None, Some(true))
                } else {
                    let sender_id = facts.author_number.as_deref().unwrap_or("");
                    match self.pairing.lock().request(workspace_id, sender_id, sender_id) {
                        Some((code, _created)) => (Some(true), Some(code), Some(false)),
                        None => (Some(false), None, Some(true)),
                    }
                }
            } else {
                (Some(false), None, Some(false))
            };

        let msg_key = format!("{chat_jid}:{}", short_text_digest(req.text.as_deref()));
        let (dedup_key, _) = self.record_inbound_dedup(workspace_id, &msg_key);

        Ok(BridgeEvalOutcome {
            platform: "whatsapp".to_string(),
            decision: decision.as_str().to_string(),
            allowed: decision == WhatsAppDecision::Allow,
            pairing_allowed,
            pairing_code,
            pending_requests_capped,
            normalized_forms,
            dedup_key: Some(dedup_key),
        })
    }

    fn eval_discord(&self, req: BridgeEvalRequest) -> RuntimeResult<BridgeEvalOutcome> {
        let workspace_id = req
            .workspace_id
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .unwrap_or("discord-default");
        if !is_identifier(workspace_id) {
            return Err(OpenBotRuntimeError::Validation(
                "Invalid Discord workspaceId.".to_string(),
            ));
        }

        let allowed_roles: Vec<MessagingAllowedRole> = if let Some(ref raw_roles) = req.allowed_roles
        {
            parse_discord_allowed_roles(raw_roles)?
        } else {
            let store = MessagingStore::new(self.database.clone());
            if let Some(conn) =
                store.connection_for_workspace(MessagingPlatform::Discord, workspace_id)?
            {
                store.allowed_roles(&conn.connection_id)?
            } else {
                Vec::new()
            }
        };

        let allowed = if allowed_roles.is_empty() {
            true
        } else {
            let author_roles: HashSet<&str> = req
                .author_roles
                .as_deref()
                .unwrap_or(&[])
                .iter()
                .map(|r| r.trim())
                .collect();
            allowed_roles
                .iter()
                .any(|role| author_roles.contains(role.id.as_str()))
        };

        let channel_scope = req.channel_id.as_deref().unwrap_or("discord");
        let msg_key = format!("{channel_scope}:{}", short_text_digest(req.text.as_deref()));
        let (dedup_key, _) = self.record_inbound_dedup(workspace_id, &msg_key);

        Ok(BridgeEvalOutcome {
            platform: "discord".to_string(),
            decision: if allowed { "allow" } else { "ignore" }.to_string(),
            allowed,
            pairing_allowed: None,
            pairing_code: None,
            pending_requests_capped: None,
            normalized_forms: None,
            dedup_key: Some(dedup_key),
        })
    }

    fn eval_slack(&self, req: BridgeEvalRequest) -> RuntimeResult<BridgeEvalOutcome> {
        if let Some(ws) = req.workspace_id.as_deref()
            && !is_identifier(ws.trim())
        {
            return Err(OpenBotRuntimeError::Validation(
                "Invalid Slack workspaceId.".to_string(),
            ));
        }
        if let Some(ch) = req.channel_id.as_deref()
            && (ch.trim().is_empty() || !is_bounded_string(ch.trim(), MESSAGING_LIMIT_NAME))
        {
            return Err(OpenBotRuntimeError::Validation(
                "Invalid Slack channelId.".to_string(),
            ));
        }
        let has_scope = req.workspace_id.as_deref().is_some_and(|s| !s.trim().is_empty())
            || req.channel_id.as_deref().is_some_and(|s| !s.trim().is_empty());
        let ws = req
            .workspace_id
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .unwrap_or("slack-default");
        let ch = req
            .channel_id
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .unwrap_or("general");
        let msg_key = format!("{ch}:{}", short_text_digest(req.text.as_deref()));
        let (dedup_key, _) = self.record_inbound_dedup(ws, &msg_key);

        Ok(BridgeEvalOutcome {
            platform: "slack".to_string(),
            decision: if has_scope { "allow" } else { "ignore" }.to_string(),
            allowed: has_scope,
            pairing_allowed: None,
            pairing_code: None,
            pending_requests_capped: None,
            normalized_forms: None,
            dedup_key: Some(dedup_key),
        })
    }
}

impl ExternalBridgeOrchestrator {
    fn eval_telegram(&self, req: BridgeEvalRequest) -> RuntimeResult<BridgeEvalOutcome> {
        if let Some(ws) = req.workspace_id.as_deref()
            && !is_identifier(ws.trim())
        {
            return Err(OpenBotRuntimeError::Validation(
                "Invalid Telegram workspaceId.".to_string(),
            ));
        }
        if let Some(ch) = req.channel_id.as_deref()
            && (ch.trim().is_empty() || !is_bounded_string(ch.trim(), MESSAGING_LIMIT_NAME))
        {
            return Err(OpenBotRuntimeError::Validation(
                "Invalid Telegram chat/channel id.".to_string(),
            ));
        }
        let chat_id = req
            .channel_id
            .as_deref()
            .or(req.workspace_id.as_deref())
            .map(str::trim)
            .filter(|s| !s.is_empty());
        let has_scope = chat_id.is_some()
            || req.sender.as_deref().is_some_and(|s| !s.trim().is_empty());
        let resolved_chat = chat_id.unwrap_or("telegram-default");
        let ws = req
            .workspace_id
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .unwrap_or(resolved_chat);
        let msg_key = format!("{resolved_chat}:{}", short_text_digest(req.text.as_deref()));
        let (dedup_key, _) = self.record_inbound_dedup(ws, &msg_key);

        if has_scope
            && let Some(hook) = self.telegram_hook.lock().clone()
        {
            let event = TelegramBridgeEvent {
                direction: TelegramBridgeDirection::Inbound,
                chat_id: resolved_chat.to_string(),
                sender_id: req.sender.clone(),
                text: req.text.clone().unwrap_or_default(),
                timestamp: Utc::now().to_rfc3339_opts(SecondsFormat::Millis, true),
            };
            hook(&event);
        }

        Ok(BridgeEvalOutcome {
            platform: "telegram".to_string(),
            decision: if has_scope { "allow" } else { "ignore" }.to_string(),
            allowed: has_scope,
            pairing_allowed: None,
            pairing_code: None,
            pending_requests_capped: None,
            normalized_forms: None,
            dedup_key: Some(dedup_key),
        })
    }
}

fn allows_whatsapp_contact(rules: &WhatsAppSenderRules, number: &str) -> bool {
    let forms = ExternalBridgeOrchestrator::whatsapp_number_forms(number);
    forms
        .iter()
        .any(|form| rules.contacts.iter().any(|c| c == form))
}

fn is_read_only_whatsapp_contact(rules: &WhatsAppSenderRules, number: &str) -> bool {
    let forms = ExternalBridgeOrchestrator::whatsapp_number_forms(number);
    forms
        .iter()
        .any(|form| rules.read_only.iter().any(|r| r == form))
}

fn normalize_whatsapp_number(raw: &str) -> Option<String> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return None;
    }
    let user_part = trimmed.split('@').next().unwrap_or(trimmed);
    let base_user = user_part.split(':').next().unwrap_or(user_part);
    let digits: String = base_user
        .chars()
        .filter(|c| !matches!(c, ' ' | '\t' | '(' | ')' | '+' | '-'))
        .collect();
    if digits.is_empty() {
        None
    } else {
        Some(digits)
    }
}

fn parse_whatsapp_allowed_senders(
    raw_senders: &[serde_json::Value],
) -> RuntimeResult<WhatsAppSenderRules> {
    if raw_senders.len() > MESSAGING_LIMIT_ALLOWED_SENDERS {
        return Err(OpenBotRuntimeError::LimitExceeded(format!(
            "WhatsApp allowed senders count {} exceeds maximum of {MESSAGING_LIMIT_ALLOWED_SENDERS}.",
            raw_senders.len()
        )));
    }
    let mut rules = WhatsAppSenderRules::default();
    for entry in raw_senders {
        if let Some(s) = entry.as_str() {
            let trimmed = s.trim();
            if trimmed.ends_with("@g.us") {
                if !WHATSAPP_GROUP_RE.is_match(trimmed) {
                    return Err(OpenBotRuntimeError::Validation(format!(
                        "Invalid WhatsApp group JID: {trimmed}"
                    )));
                }
                rules.groups.push(trimmed.to_string());
            } else {
                let num = normalize_whatsapp_number(trimmed).unwrap_or_default();
                if !WHATSAPP_NUMBER_RE.is_match(&num) {
                    return Err(OpenBotRuntimeError::Validation(format!(
                        "Invalid WhatsApp contact number: {trimmed}"
                    )));
                }
                rules.contacts.push(num);
            }
            continue;
        }
        let Some(obj) = entry.as_object() else {
            return Err(OpenBotRuntimeError::Validation(
                "WhatsApp allowed sender entry must be an object or string.".to_string(),
            ));
        };
        let Some(id_raw) = obj.get("id").and_then(serde_json::Value::as_str) else {
            return Err(OpenBotRuntimeError::Validation(
                "WhatsApp allowed sender missing string id.".to_string(),
            ));
        };
        let kind = obj
            .get("kind")
            .and_then(serde_json::Value::as_str)
            .unwrap_or(if id_raw.trim().ends_with("@g.us") {
                "group"
            } else {
                "contact"
            });
        if let Some(name) = obj.get("name").and_then(serde_json::Value::as_str)
            && !is_bounded_string(name, MESSAGING_LIMIT_NAME)
        {
            return Err(OpenBotRuntimeError::Validation(
                "WhatsApp allowed sender name exceeds limit.".to_string(),
            ));
        }
        let read_only = obj
            .get("readOnly")
            .or_else(|| obj.get("read_only"))
            .and_then(serde_json::Value::as_bool)
            .unwrap_or(false);
        match kind {
            "contact" => {
                let num = normalize_whatsapp_number(id_raw).unwrap_or_default();
                if !WHATSAPP_NUMBER_RE.is_match(&num) {
                    return Err(OpenBotRuntimeError::Validation(format!(
                        "Invalid WhatsApp contact number: {id_raw}"
                    )));
                }
                rules.contacts.push(num.clone());
                if read_only {
                    rules.read_only.push(num);
                }
            }
            "group" => {
                let jid = id_raw.trim();
                if !WHATSAPP_GROUP_RE.is_match(jid) {
                    return Err(OpenBotRuntimeError::Validation(format!(
                        "Invalid WhatsApp group JID: {jid}"
                    )));
                }
                rules.groups.push(jid.to_string());
                if read_only {
                    rules.read_only.push(jid.to_string());
                }
            }
            other => {
                return Err(OpenBotRuntimeError::Validation(format!(
                    "Invalid WhatsApp sender kind: {other}"
                )));
            }
        }
    }
    Ok(rules)
}

fn parse_discord_allowed_roles(
    raw_roles: &[serde_json::Value],
) -> RuntimeResult<Vec<MessagingAllowedRole>> {
    if raw_roles.len() > MESSAGING_LIMIT_ALLOWED_ROLES {
        return Err(OpenBotRuntimeError::LimitExceeded(format!(
            "Discord allowed roles count {} exceeds maximum of {MESSAGING_LIMIT_ALLOWED_ROLES}.",
            raw_roles.len()
        )));
    }
    let mut roles = Vec::with_capacity(raw_roles.len());
    let mut seen_ids = HashSet::with_capacity(raw_roles.len());
    for entry in raw_roles {
        let (id, name) = match entry {
            serde_json::Value::String(raw_id) => {
                let id = raw_id.trim();
                (id, id)
            }
            serde_json::Value::Object(obj) => {
                let Some(id) = obj.get("id").and_then(serde_json::Value::as_str) else {
                    return Err(OpenBotRuntimeError::Validation(
                        "Discord allowed role missing string id.".to_string(),
                    ));
                };
                let name = match obj.get("name") {
                    Some(serde_json::Value::String(n)) => n.as_str(),
                    None => id,
                    Some(_) => {
                        return Err(OpenBotRuntimeError::Validation(
                            "Discord allowed role name must be a string.".to_string(),
                        ));
                    }
                };
                (id, name)
            }
            _ => {
                return Err(OpenBotRuntimeError::Validation(
                    "Discord allowed role must be a role ID string or an object with id and name."
                        .to_string(),
                ));
            }
        };
        if !DISCORD_ROLE_ID_RE.is_match(id) {
            return Err(OpenBotRuntimeError::Validation(format!(
                "Invalid Discord role id: {id}"
            )));
        }
        if utf16_len(name) > MESSAGING_LIMIT_ROLE_NAME {
            return Err(OpenBotRuntimeError::Validation(format!(
                "Discord role name exceeds {MESSAGING_LIMIT_ROLE_NAME} characters."
            )));
        }
        if !seen_ids.insert(id.to_string()) {
            return Err(OpenBotRuntimeError::Validation(format!(
                "Duplicate Discord allowed role id: {id}"
            )));
        }
        roles.push(MessagingAllowedRole {
            id: id.to_string(),
            name: name.to_string(),
        });
    }
    Ok(roles)
}

fn short_text_digest(text: Option<&str>) -> String {
    let mut hasher = Sha256::new();
    hasher.update(text.unwrap_or("").as_bytes());
    let digest = hasher.finalize();
    hex::encode(&digest[..8])
}
