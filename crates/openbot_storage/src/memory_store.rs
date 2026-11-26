//! 3-Tier OpenBot Memory Persistence, FTS5 `bm25` + `>=6`-char Prefix + CJK/Script-Boundary Search,
//! `site` Domain Hierarchy Recall, `<openbot_saved_memories>` / `<openbot_saved_memories_update>`
//! Prompt Builders, and `StagedTurnMemories` Optimistic Turn Commit Coordinator.
//!
//! Direct 1:1 port of:
//! - `src/backend/memory-store.ts`
//! - `src/backend/agent-memory-store.ts`
//! - `src/backend/agent-memory-selection.ts`
//! - `src/backend/channel-memory-store.ts`
//! - `src/backend/messaging/messaging-memory-store.ts`
//! - `packages/contracts/src/agent-memory-context.ts`
//! - `src/backend/agent/developer-instructions.ts:L246-L313`
//! - `src/backend/agent/agent-memories.ts`

use std::collections::{BTreeMap, HashMap, HashSet};
use std::fmt;
use std::sync::Arc;

use chrono::{DateTime, Duration, SecondsFormat, Utc};
use openbot_contracts::conversation::{ConversationMemoryChange, ConversationMemoryChangeAction};
use openbot_contracts::limits::{INPUT_LIMITS, is_identifier, utf16_len};
use parking_lot::Mutex;
use rusqlite::{Connection, params, params_from_iter};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use uuid::Uuid;

use crate::database::{OpenBotDatabase, OrchestrationEventInput};
use crate::error::{StorageError, StorageResult};
use crate::redact::redact_secrets;

pub const AGENT_MEMORY_CONTEXT_BUDGET_BYTES: usize = 4_096;
pub const UPSTREAM_V78_MEMORY_BUDGET_BYTES: usize = 8_192;
pub const DEFAULT_AGENT_MEMORY_LIMIT: usize = 64;
pub const AGENT_MEMORY_LIMIT_CHOICES: [usize; 4] = [64, 128, 256, 512];
pub const MEMORY_SITE_MAX_LENGTH: usize = 253;
pub const SITE_MEMORY_LIMIT: usize = 5;
pub const MEMORY_LIST_PAGE_LIMIT: usize = 25;
pub const MEMORY_SELECTION_BATCH_LIMIT: usize = 25;

const MEMORY_LABEL: &str = "The following saved memories are the user's lasting preferences and facts. Follow them like the user's instructions, unless the user's current request or your Instructions say otherwise. A memory with a site is a note on how that website works: use it as a hint, not as a rule.";
const MEMORY_BLOCK_START: &str = "<openbot_saved_memories>";
const MEMORY_UPDATE_START: &str = "<openbot_saved_memories_update>";

pub const MEMORY_OFF_UPDATE: &str = "<openbot_saved_memories_update>\nThe user turned OpenBot memory off for you. The saved memories in earlier saved-memory blocks are no longer current: do not use them.\n</openbot_saved_memories_update>";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum AgentMemoryOrigin {
    Automatic,
    Manual,
}

impl AgentMemoryOrigin {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Automatic => "automatic",
            Self::Manual => "manual",
        }
    }

    pub fn parse(raw: &str) -> StorageResult<Self> {
        match raw {
            "automatic" => Ok(Self::Automatic),
            "manual" => Ok(Self::Manual),
            _ => Err(StorageError::Validation("Invalid memory origin.".to_string())),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum MemoryInclusion {
    Essential,
    Searchable,
}

impl MemoryInclusion {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Essential => "essential",
            Self::Searchable => "searchable",
        }
    }

    pub fn parse(raw: &str) -> StorageResult<Self> {
        match raw {
            "essential" => Ok(Self::Essential),
            "searchable" => Ok(Self::Searchable),
            _ => Err(StorageError::Validation(
                "Invalid memory inclusion.".to_string(),
            )),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum MemoryInclusionChangeValue {
    Essential,
    Searchable,
    Automatic,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum MemorySelectionActor {
    User,
    Agent,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum MemorySelectionErrorCode {
    Budget,
    Conflict,
    UserControlled,
    Query,
    Missing,
}

impl MemorySelectionErrorCode {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Budget => "budget",
            Self::Conflict => "conflict",
            Self::UserControlled => "user-controlled",
            Self::Query => "query",
            Self::Missing => "missing",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AgentMemorySelectionError {
    pub code: MemorySelectionErrorCode,
    pub message: String,
}

impl fmt::Display for AgentMemorySelectionError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.message)
    }
}

impl std::error::Error for AgentMemorySelectionError {}

impl From<AgentMemorySelectionError> for StorageError {
    fn from(err: AgentMemorySelectionError) -> Self {
        match err.code {
            MemorySelectionErrorCode::Budget => Self::LimitExceeded(err.message),
            MemorySelectionErrorCode::Conflict => Self::Conflict(err.message),
            MemorySelectionErrorCode::UserControlled => Self::Forbidden(err.message),
            MemorySelectionErrorCode::Query => Self::Validation(err.message),
            MemorySelectionErrorCode::Missing => Self::NotFound(err.message),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MemoryEntry {
    pub id: String,
    pub text: String,
    pub origin: AgentMemoryOrigin,
    #[serde(deserialize_with = "Option::deserialize")]
    pub source_turn_id: Option<String>,
    pub created_at: String,
    pub updated_at: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub site: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentMemory {
    #[serde(flatten)]
    pub entry: MemoryEntry,
    pub agent_id: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ChannelMemory {
    #[serde(flatten)]
    pub entry: MemoryEntry,
    pub channel_id: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentMemorySelection {
    pub memory_id: String,
    pub inclusion: MemoryInclusion,
    pub user_controlled: bool,
    pub revision: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentMemorySelectionChange {
    pub memory_id: String,
    pub inclusion: MemoryInclusionChangeValue,
    pub expected_revision: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentMemorySelectionState {
    pub selections: Vec<AgentMemorySelection>,
    pub used_bytes: usize,
    pub budget_bytes: usize,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RetrievedAgentMemory {
    #[serde(flatten)]
    pub memory: AgentMemory,
    pub inclusion: MemoryInclusion,
    pub user_controlled: bool,
    pub revision: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentMemorySearchResult {
    pub memories: Vec<RetrievedAgentMemory>,
    pub has_more: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentMemoryListPage {
    pub memories: Vec<RetrievedAgentMemory>,
    pub has_more: bool,
    #[serde(deserialize_with = "Option::deserialize")]
    pub next_cursor: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ContextMemory {
    pub id: String,
    pub text: String,
    pub origin: AgentMemoryOrigin,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub site: Option<String>,
}

impl From<&MemoryEntry> for ContextMemory {
    fn from(entry: &MemoryEntry) -> Self {
        Self {
            id: entry.id.clone(),
            text: entry.text.clone(),
            origin: entry.origin,
            site: entry.site.clone(),
        }
    }
}

impl From<&AgentMemory> for ContextMemory {
    fn from(mem: &AgentMemory) -> Self {
        Self::from(&mem.entry)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MemoryContext {
    pub memories: Vec<ContextMemory>,
    pub stored_count: usize,
    pub limit: usize,
    #[serde(skip, default = "default_budget_bytes")]
    pub budget_bytes: usize,
}

const fn default_budget_bytes() -> usize {
    AGENT_MEMORY_CONTEXT_BUDGET_BYTES
}

pub fn serialize_essential_memories(memories: &[ContextMemory]) -> String {
    let json = serde_json::to_string(memories).unwrap_or_else(|_| "[]".to_string());
    format!(
        "<agent_memories count=\"{}\">\n{json}\n</agent_memories>",
        memories.len()
    )
}

pub fn essential_memory_bytes(memories: &[ContextMemory]) -> usize {
    serialize_essential_memories(memories).len()
}

pub fn build_memory_context(
    memories: &[AgentMemory],
    memory_limit: Option<usize>,
    stored_memory_count: Option<usize>,
    budget_bytes: Option<usize>,
) -> MemoryContext {
    let budget = budget_bytes.unwrap_or(AGENT_MEMORY_CONTEXT_BUDGET_BYTES);
    let mut prompt_memories: Vec<ContextMemory> = Vec::new();
    for memory in memories {
        let safe = ContextMemory {
            id: memory.entry.id.clone(),
            text: redact_secrets(&memory.entry.text),
            origin: memory.entry.origin,
            site: memory.entry.site.clone(),
        };
        prompt_memories.push(safe);
        if essential_memory_bytes(&prompt_memories) > budget {
            prompt_memories.pop();
        }
    }
    MemoryContext {
        memories: prompt_memories,
        stored_count: stored_memory_count.unwrap_or(memories.len()),
        limit: memory_limit.unwrap_or(DEFAULT_AGENT_MEMORY_LIMIT),
        budget_bytes: budget,
    }
}

fn memory_count_line(context: &MemoryContext) -> String {
    let additional = context.stored_count.saturating_sub(context.memories.len());
    format!(
        "You have {} saved memories; {} additional entries are available through search. The storage limit is {}. The essential-memory prompt has a separate {}-byte limit.",
        context.stored_count, additional, context.limit, context.budget_bytes
    )
}

pub fn format_memory_context(context: &MemoryContext) -> String {
    format!(
        "{MEMORY_BLOCK_START}\n{MEMORY_LABEL}\n{}\n{}\n</openbot_saved_memories>",
        serialize_essential_memories(&context.memories),
        memory_count_line(context),
    )
}

pub fn memory_context_update(
    delivered: Option<&MemoryContext>,
    current: &MemoryContext,
) -> Option<String> {
    let full = format_memory_context(current);
    let Some(delivered) = delivered else {
        return Some(full);
    };
    let before: HashMap<&str, &ContextMemory> = delivered
        .memories
        .iter()
        .map(|m| (m.id.as_str(), m))
        .collect();
    let changed: Vec<ContextMemory> = current
        .memories
        .iter()
        .filter(|m| {
            before.get(m.id.as_str()).is_none_or(|old| {
                old.text != m.text || old.origin != m.origin || old.site != m.site
            })
        })
        .cloned()
        .collect();
    let kept: HashSet<&str> = current.memories.iter().map(|m| m.id.as_str()).collect();
    let removed: Vec<&str> = delivered
        .memories
        .iter()
        .filter(|m| !kept.contains(m.id.as_str()))
        .map(|m| m.id.as_str())
        .collect();
    let count = memory_count_line(current);
    if changed.is_empty() && removed.is_empty() && count == memory_count_line(delivered) {
        return None;
    }
    let mut lines = vec![
        MEMORY_UPDATE_START.to_string(),
        format!("Your essential saved memories changed after the last saved-memory block. {MEMORY_LABEL}"),
    ];
    if !changed.is_empty() {
        lines.push("Added or changed:".to_string());
        lines.push(serialize_essential_memories(&changed));
    }
    if !removed.is_empty() {
        lines.push(format!(
            "No longer essential (forgotten, or searchable only): {}",
            serde_json::to_string(&removed).unwrap_or_else(|_| "[]".to_string())
        ));
    }
    lines.push(count);
    lines.push("</openbot_saved_memories_update>".to_string());
    let update = lines.join("\n");
    if utf16_len(&update) < utf16_len(&full) {
        Some(update)
    } else {
        Some(full)
    }
}

fn now_iso() -> String {
    Utc::now().to_rfc3339_opts(SecondsFormat::Millis, true)
}

fn monotonic_updated_at(previous_updated_at: Option<&str>, now: &str) -> String {
    let Some(prev) = previous_updated_at else {
        return now.to_string();
    };
    if now > prev {
        return now.to_string();
    }
    if let Ok(parsed) = DateTime::parse_from_rfc3339(prev) {
        return (parsed.with_timezone(&Utc) + Duration::milliseconds(1))
            .to_rfc3339_opts(SecondsFormat::Millis, true);
    }
    now.to_string()
}

fn new_memory_event(
    aggregate_type: &str,
    aggregate_id: &str,
    event_type: &str,
    payload: Value,
) -> OrchestrationEventInput {
    OrchestrationEventInput {
        event_id: Uuid::new_v4().to_string(),
        aggregate_type: aggregate_type.to_string(),
        aggregate_id: aggregate_id.to_string(),
        event_type: event_type.to_string(),
        occurred_at: now_iso(),
        payload,
    }
}

pub fn validate_memory_text(value: &str) -> StorageResult<String> {
    let text = value.trim();
    if text.is_empty() {
        return Err(StorageError::Validation(
            "Memory text is required.".to_string(),
        ));
    }
    if utf16_len(text) > INPUT_LIMITS.agent_memory_text {
        return Err(StorageError::Validation(
            "Memory text is too long.".to_string(),
        ));
    }
    Ok(text.to_string())
}

#[inline]
pub fn normalize_memory_text(value: &str) -> String {
    value.to_string()
}

pub fn normalize_memory_site(value: &str) -> Option<String> {
    let trimmed = value.trim();
    if trimmed.is_empty() || utf16_len(trimmed) > 2_048 {
        return None;
    }
    let after_scheme = if let Some(sep_idx) = trimmed.find("://") {
        let scheme = &trimmed[..sep_idx];
        let valid_scheme = !scheme.is_empty()
            && scheme.as_bytes()[0].is_ascii_alphabetic()
            && scheme.bytes().all(|b| {
                b.is_ascii_alphanumeric() || matches!(b, b'+' | b'.' | b'-')
            });
        if valid_scheme {
            &trimmed[sep_idx + 3..]
        } else {
            trimmed
        }
    } else {
        trimmed
    };
    let auth_end = after_scheme
        .find(['/', '\\', '?', '#'])
        .unwrap_or(after_scheme.len());
    let authority = &after_scheme[..auth_end];
    if authority.is_empty() || authority.chars().any(char::is_whitespace) {
        return None;
    }
    let host_port = authority
        .rfind('@')
        .map_or(authority, |at| &authority[at + 1..]);
    if host_port.is_empty() {
        return None;
    }
    let host = if host_port.starts_with('[') {
        let close = host_port.find(']')?;
        let rest = &host_port[close + 1..];
        if !rest.is_empty() {
            let port_str = rest.strip_prefix(':')?;
            if !port_str.is_empty() {
                let _port: u16 = port_str.parse().ok()?;
            }
        }
        &host_port[..=close]
    } else if let Some((h, port_str)) = host_port.split_once(':') {
        if port_str.contains(':') {
            return None;
        }
        if !port_str.is_empty() {
            let _port: u16 = port_str.parse().ok()?;
        }
        if h.chars().any(|c| {
            matches!(
                c,
                '\0' | '#' | '%' | '<' | '>' | '[' | ']' | '^' | '|'
            )
        }) {
            return None;
        }
        h
    } else {
        if host_port.chars().any(|c| {
            matches!(
                c,
                '\0' | '#' | '%' | '<' | '>' | '[' | ']' | '^' | '|'
            )
        }) {
            return None;
        }
        host_port
    };
    let mut site = host.to_lowercase();
    if let Some(stripped) = site.strip_suffix('.') {
        site = stripped.to_string();
    }
    if let Some(stripped) = site.strip_prefix("www.") {
        site = stripped.to_string();
    }
    (!site.is_empty() && utf16_len(&site) <= MEMORY_SITE_MAX_LENGTH).then_some(site)
}

pub fn memory_sites_for_host(hostname: &str) -> Vec<String> {
    let Some(site) = normalize_memory_site(hostname) else {
        return Vec::new();
    };
    let labels: Vec<&str> = site.split('.').collect();
    if labels.len() < 2
        || site.chars().all(|c| c.is_ascii_digit() || c == '.')
        || site.contains(':')
    {
        return vec![site];
    }
    (0..labels.len() - 1)
        .map(|idx| labels[idx..].join("."))
        .collect()
}

pub fn memory_prompt_line(text: &str, block: &str) -> String {
    let collapsed = text.split_whitespace().collect::<Vec<_>>().join(" ");
    let lower_block = block.to_ascii_lowercase();
    let mut out = String::with_capacity(collapsed.len());
    let chars: Vec<char> = collapsed.chars().collect();
    let mut i = 0usize;
    while i < chars.len() {
        if chars[i] == '<' {
            let mut p = i + 1;
            while p < chars.len() && chars[p].is_whitespace() {
                p += 1;
            }
            if chars.get(p) == Some(&'/') {
                p += 1;
                while p < chars.len() && chars[p].is_whitespace() {
                    p += 1;
                }
                let cand: String = chars[p..]
                    .iter()
                    .take(lower_block.chars().count())
                    .collect();
                if cand.to_ascii_lowercase() == lower_block {
                    out.push_str("< /");
                    out.push_str(&cand);
                    i = p + lower_block.chars().count();
                    continue;
                }
            }
        }
        out.push(chars[i]);
        i += 1;
    }
    out.trim().to_string()
}

pub fn fold_memory_text(text: &str) -> String {
    let mut compat = String::with_capacity(text.len());
    for ch in text.chars() {
        match ch {
            '\u{3000}' | '\u{00A0}' | '\u{202F}' | '\u{2007}' => compat.push(' '),
            '\u{FF01}'..='\u{FF5E}' => {
                if let Some(ascii) = char::from_u32(ch as u32 - 0xFEE0) {
                    compat.push(ascii);
                }
            }
            '\u{2018}'..='\u{201F}' | '"' | '`' | '\u{2032}' => compat.push('\''),
            '\u{2010}'..='\u{2015}' | '\u{2212}' | '\u{FE58}' | '\u{FE63}' => compat.push('-'),
            '\u{2026}' => compat.push_str("..."),
            '\u{FB00}' => compat.push_str("ff"),
            '\u{FB01}' => compat.push_str("fi"),
            '\u{FB02}' => compat.push_str("fl"),
            '\u{FB03}' => compat.push_str("ffi"),
            '\u{FB04}' => compat.push_str("ffl"),
            '\u{00B9}' => compat.push('1'),
            '\u{00B2}' => compat.push('2'),
            '\u{00B3}' => compat.push('3'),
            '\u{2070}' => compat.push('0'),
            '\u{2074}'..='\u{2079}' => {
                if let Some(d) = char::from_u32(b'0' as u32 + (ch as u32 - 0x2070)) {
                    compat.push(d);
                }
            }
            other => compat.push(other),
        }
    }
    let normalized_quotes = compat
        .chars()
        .map(|c| match c {
            '"' | '`' => '\'',
            _ => c,
        })
        .collect::<String>();
    normalized_quotes
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .to_lowercase()
}

/// Exact pure-Rust unspaced script range check matching `UNSPACED_SCRIPT` in `agent-memory-store.ts:L18-L19`:
/// `Han`, `Hiragana`, `Katakana`, `Hangul`, `Thai`, `Lao`, `Khmer`, `Myanmar`.
pub fn is_unspaced_script(c: char) -> bool {
    let u = c as u32;
    matches!(
        u,
        // Han (CJK Unified Ideographs + Extensions A..I + Compatibility)
        0x3400..=0x4DBF | 0x4E00..=0x9FFF | 0xF900..=0xFAFF | 0x20000..=0x323AF
        // Hiragana & Katakana (including Phonetic Extensions & Halfwidth)
        | 0x3040..=0x309F | 0x30A0..=0x30FF | 0x31F0..=0x31FF | 0xFF65..=0xFF9F | 0x1B000..=0x1B16F
        // Hangul (Jamo, Compatibility Jamo, Syllables, Extended-A/B)
        | 0x1100..=0x11FF | 0x3130..=0x318F | 0xA960..=0xA97F | 0xAC00..=0xD7AF | 0xD7B0..=0xD7FF
        // Thai, Lao, Khmer, Myanmar (+ Extended-A/B/C)
        | 0x0E00..=0x0E7F | 0x0E80..=0x0EFF | 0x1780..=0x17FF | 0x19E0..=0x19FF
        | 0x1000..=0x109F | 0xAA60..=0xAA7F | 0xA9E0..=0xA9FF | 0x116D0..=0x116E3
    )
}

pub fn is_unicode_combining_mark(c: char) -> bool {
    let u = c as u32;
    matches!(
        u,
        0x0300..=0x036F
            | 0x0483..=0x0489
            | 0x0591..=0x05BD
            | 0x05BF
            | 0x05C1..=0x05C2
            | 0x05C4..=0x05C5
            | 0x05C7
            | 0x0610..=0x061A
            | 0x064B..=0x065F
            | 0x0670
            | 0x06D6..=0x06ED
            | 0x0901..=0x0903
            | 0x093A..=0x094F
            | 0x0951..=0x0957
            | 0x0962..=0x0963
            | 0x0E31
            | 0x0E34..=0x0E3A
            | 0x0E47..=0x0E4E
            | 0x0EB1
            | 0x0EB4..=0x0EBC
            | 0x0EC8..=0x0ECD
            | 0x102B..=0x103E
            | 0x1056..=0x1059
            | 0x105E..=0x1060
            | 0x1062..=0x1064
            | 0x1067..=0x106D
            | 0x1071..=0x1074
            | 0x1082..=0x108D
            | 0x108F
            | 0x109A..=0x109D
            | 0x17B4..=0x17D3
            | 0x17DD
            | 0x1DC0..=0x1DFF
            | 0x20D0..=0x20FF
            | 0x3099..=0x309A
            | 0xFE20..=0xFE2F
    )
}

#[inline]
fn is_word_char(c: char) -> bool {
    c.is_alphanumeric() || is_unicode_combining_mark(c)
}

pub fn extract_search_word_terms(query: &str) -> Vec<String> {
    let mut terms = Vec::new();
    let mut current = String::new();
    for ch in query.chars() {
        if is_word_char(ch) {
            current.push(ch);
        } else if !current.is_empty() {
            terms.push(std::mem::take(&mut current));
        }
    }
    if !current.is_empty() {
        terms.push(current);
    }
    terms
}

/// Exact port of `unindexedTermMatcher` (`agent-memory-store.ts:L21-L26`).
pub fn matches_unindexed_term(text: &str, term: &str) -> bool {
    let t_chars: Vec<char> = text.chars().flat_map(char::to_lowercase).collect();
    let q_chars: Vec<char> = term.chars().flat_map(char::to_lowercase).collect();
    if q_chars.is_empty() || q_chars.len() > t_chars.len() {
        return false;
    }
    let term_has_unspaced = term.chars().any(is_unspaced_script);
    for start in 0..=(t_chars.len() - q_chars.len()) {
        if t_chars[start..start + q_chars.len()] != q_chars[..] {
            continue;
        }
        if term_has_unspaced {
            return true;
        }
        let prev = start.checked_sub(1).map(|i| t_chars[i]);
        let next = t_chars.get(start + q_chars.len()).copied();
        let prev_unspaced = prev.is_some_and(is_unspaced_script);
        let next_unspaced = next.is_some_and(is_unspaced_script);
        let prev_edge = prev.is_none_or(|c| is_unspaced_script(c) || !is_word_char(c));
        let next_edge = next.is_none_or(|c| is_unspaced_script(c) || !is_word_char(c));
        if (prev_unspaced && next_edge) || (prev_edge && next_unspaced) {
            return true;
        }
    }
    false
}

fn forget_events_before_tx(
    conn: &Connection,
    aggregate_type: &str,
    memory_id: &str,
    deletion_sequence: i64,
) -> StorageResult<()> {
    conn.execute(
        "DELETE FROM orchestration_command_receipts WHERE command_id IN (
           SELECT DISTINCT command_id
           FROM orchestration_events
           WHERE aggregate_type = ?1 AND aggregate_id = ?2 AND sequence < ?3
         )",
        params![aggregate_type, memory_id, deletion_sequence],
    )?;
    conn.execute(
        "DELETE FROM orchestration_events
         WHERE aggregate_type = ?1 AND aggregate_id = ?2 AND sequence < ?3",
        params![aggregate_type, memory_id, deletion_sequence],
    )?;
    Ok(())
}

#[derive(Clone)]
pub struct MemoryTables {
    pub table: &'static str,
    pub owner_column: &'static str,
    pub owner_key: &'static str,
    pub aggregate_type: &'static str,
    pub limit_fn: Arc<dyn Fn() -> usize + Send + Sync>,
    pub limit_message_fn: Arc<dyn Fn(usize) -> String + Send + Sync>,
    pub site_column: bool,
}

#[derive(Debug, Clone)]
pub struct SaveAutomaticMemory {
    pub memory_id: Option<String>,
    pub text: String,
    pub source_turn_id: String,
    pub expected_updated_at: Option<Option<String>>,
    pub command_id: Option<String>,
    pub site: Option<String>,
}

#[derive(Debug, Clone)]
pub struct SaveMemoryInput {
    pub memory_id: Option<String>,
    pub text: String,
    pub origin: AgentMemoryOrigin,
    pub source_turn_id: Option<String>,
    pub command_id: Option<String>,
    pub uncapped: bool,
    /// `None` preserves existing `site` on update; `Some(None)` clears `site`; `Some(Some(s))` sets `site`.
    pub site: Option<Option<String>>,
}

fn agent_memory_tables(limit_fn: Arc<dyn Fn() -> usize + Send + Sync>) -> MemoryTables {
    MemoryTables {
        table: "projection_agent_memories",
        owner_column: "agent_id",
        owner_key: "agentId",
        aggregate_type: "agent-memory",
        limit_fn,
        limit_message_fn: Arc::new(|lim| format!("An agent can have up to {lim} memories.")),
        site_column: true,
    }
}

fn channel_memory_tables() -> MemoryTables {
    MemoryTables {
        table: "projection_channel_memories",
        owner_column: "channel_id",
        owner_key: "channelId",
        aggregate_type: "channel-memory",
        limit_fn: Arc::new(|| INPUT_LIMITS.channel_memories),
        limit_message_fn: Arc::new(|lim| format!("A channel can have up to {lim} memories.")),
        site_column: false,
    }
}

fn messaging_memory_tables() -> MemoryTables {
    MemoryTables {
        table: "projection_messaging_memories",
        owner_column: "connection_id",
        owner_key: "connectionId",
        aggregate_type: "messaging-memory",
        limit_fn: Arc::new(|| INPUT_LIMITS.messaging_memories),
        limit_message_fn: Arc::new(|lim| {
            format!("A connected workspace can have up to {lim} memories. Delete one first.")
        }),
        site_column: false,
    }
}

#[derive(Clone)]
pub struct MemoryStore {
    database: OpenBotDatabase,
    tables: MemoryTables,
    columns: String,
}

impl MemoryStore {
    pub fn new(database: OpenBotDatabase) -> Self {
        Self::for_tables(
            database,
            agent_memory_tables(Arc::new(|| DEFAULT_AGENT_MEMORY_LIMIT)),
        )
    }

    pub fn for_tables(database: OpenBotDatabase, tables: MemoryTables) -> Self {
        let columns = if tables.site_column {
            "memory_id, text, origin, source_turn_id, created_at, updated_at, site".to_string()
        } else {
            "memory_id, text, origin, source_turn_id, created_at, updated_at".to_string()
        };
        Self {
            database,
            tables,
            columns,
        }
    }

    pub fn agent(database: OpenBotDatabase) -> AgentMemoryStore {
        AgentMemoryStore::new(database)
    }

    pub fn channel(database: OpenBotDatabase) -> ChannelMemoryStore {
        ChannelMemoryStore::new(database)
    }

    pub fn messaging(database: OpenBotDatabase) -> MessagingMemoryStore {
        MessagingMemoryStore::new(database)
    }

    pub fn database(&self) -> &OpenBotDatabase {
        &self.database
    }

    fn decode_row(&self, row: &rusqlite::Row<'_>) -> StorageResult<MemoryEntry> {
        let origin_raw: String = row.get(2)?;
        let origin = AgentMemoryOrigin::parse(&origin_raw)?;
        let site = if self.tables.site_column {
            row.get::<_, Option<String>>(6)?.filter(|s| !s.is_empty())
        } else {
            None
        };
        Ok(MemoryEntry {
            id: row.get(0)?,
            text: row.get(1)?,
            origin,
            source_turn_id: row.get(3)?,
            created_at: row.get(4)?,
            updated_at: row.get(5)?,
            site,
        })
    }

    pub fn list(&self, owner_id: &str) -> StorageResult<Vec<MemoryEntry>> {
        self.database.with_conn(|conn| self.list_tx(conn, owner_id))
    }

    fn list_tx(&self, conn: &Connection, owner_id: &str) -> StorageResult<Vec<MemoryEntry>> {
        let sql = format!(
            "SELECT {} FROM {} WHERE {} = ?1 ORDER BY updated_at DESC, memory_id",
            self.columns, self.tables.table, self.tables.owner_column
        );
        let mut stmt = conn.prepare(&sql)?;
        let mut rows = stmt.query(params![owner_id])?;
        let mut out = Vec::new();
        while let Some(row) = rows.next()? {
            out.push(self.decode_row(row)?);
        }
        Ok(out)
    }

    pub fn get(&self, owner_id: &str, memory_id: &str) -> StorageResult<Option<MemoryEntry>> {
        self.database
            .with_conn(|conn| self.get_tx(conn, owner_id, memory_id))
    }

    fn get_tx(
        &self,
        conn: &Connection,
        owner_id: &str,
        memory_id: &str,
    ) -> StorageResult<Option<MemoryEntry>> {
        let sql = format!(
            "SELECT {} FROM {} WHERE {} = ?1 AND memory_id = ?2",
            self.columns, self.tables.table, self.tables.owner_column
        );
        let mut stmt = conn.prepare(&sql)?;
        let mut rows = stmt.query(params![owner_id, memory_id])?;
        match rows.next()? {
            Some(row) => Ok(Some(self.decode_row(row)?)),
            None => Ok(None),
        }
    }

    pub fn count(&self, owner_id: &str) -> StorageResult<usize> {
        self.database
            .with_conn(|conn| self.count_tx(conn, owner_id))
    }

    fn count_tx(&self, conn: &Connection, owner_id: &str) -> StorageResult<usize> {
        let sql = format!(
            "SELECT COUNT(*) FROM {} WHERE {} = ?1",
            self.tables.table, self.tables.owner_column
        );
        let c: i64 = conn.query_row(&sql, params![owner_id], |r| r.get(0))?;
        Ok(c.max(0) as usize)
    }

    pub fn limit(&self) -> usize {
        (self.tables.limit_fn)()
    }

    pub fn create_manual(&self, owner_id: &str, text: &str) -> StorageResult<MemoryEntry> {
        self.save_base(
            owner_id,
            SaveMemoryInput {
                memory_id: None,
                text: text.to_string(),
                origin: AgentMemoryOrigin::Manual,
                source_turn_id: None,
                command_id: None,
                uncapped: false,
                site: None,
            },
        )
    }

    pub fn update_manual(
        &self,
        owner_id: &str,
        memory_id: &str,
        text: &str,
    ) -> StorageResult<MemoryEntry> {
        self.save_base(
            owner_id,
            SaveMemoryInput {
                memory_id: Some(memory_id.to_string()),
                text: text.to_string(),
                origin: AgentMemoryOrigin::Manual,
                source_turn_id: None,
                command_id: None,
                uncapped: false,
                site: None,
            },
        )
    }
}

impl MemoryStore {
    pub fn duplicate(
        &self,
        source_owner_id: &str,
        target_owner_id: &str,
    ) -> StorageResult<Vec<MemoryEntry>> {
        let originals = self.list(source_owner_id)?;
        let mut out = Vec::with_capacity(originals.len());
        for mem in originals {
            out.push(self.save_base(
                target_owner_id,
                SaveMemoryInput {
                    memory_id: None,
                    text: mem.text,
                    origin: mem.origin,
                    source_turn_id: None,
                    command_id: None,
                    uncapped: true,
                    site: Some(mem.site),
                },
            )?);
        }
        Ok(out)
    }

    pub fn save_automatic_entry(
        &self,
        owner_id: &str,
        input: SaveAutomaticMemory,
    ) -> StorageResult<Option<MemoryEntry>> {
        if let (Some(mid), Some(expected)) = (&input.memory_id, &input.expected_updated_at) {
            let current = self.get(owner_id, mid)?;
            let Some(cur) = current else {
                return Ok(None);
            };
            if Some(cur.updated_at.as_str()) != expected.as_deref() {
                return Ok(None);
            }
        }
        let saved = self.save_base(
            owner_id,
            SaveMemoryInput {
                memory_id: input.memory_id,
                text: input.text,
                origin: AgentMemoryOrigin::Automatic,
                source_turn_id: Some(input.source_turn_id),
                command_id: input.command_id,
                uncapped: false,
                site: input.site.map(Some),
            },
        )?;
        Ok(Some(saved))
    }

    pub fn delete(&self, owner_id: &str, memory_id: &str) -> StorageResult<bool> {
        self.delete_with_expected(owner_id, memory_id, None)
    }

    pub fn delete_with_expected(
        &self,
        owner_id: &str,
        memory_id: &str,
        expected_updated_at: Option<Option<&str>>,
    ) -> StorageResult<bool> {
        let Some(current) = self.get(owner_id, memory_id)? else {
            return Ok(false);
        };
        if let Some(exp) = expected_updated_at
            && Some(current.updated_at.as_str()) != exp
        {
            return Ok(false);
        }
        let agg = self.tables.aggregate_type;
        let table = self.tables.table;
        let owner_col = self.tables.owner_column;
        let owner_key = self.tables.owner_key;
        let command_id = format!("{agg}:delete:{}", Uuid::new_v4());
        let mut payload_obj = serde_json::Map::new();
        payload_obj.insert(
            owner_key.to_string(),
            Value::String(owner_id.to_string()),
        );
        payload_obj.insert(
            "memoryId".to_string(),
            Value::String(memory_id.to_string()),
        );
        let event = new_memory_event(
            agg,
            memory_id,
            &format!("{agg}.deleted"),
            Value::Object(payload_obj),
        );
        self.database.dispatch_with_projection(
            &command_id,
            None,
            &[event],
            |conn, sequences| {
                forget_events_before_tx(
                    conn,
                    agg,
                    memory_id,
                    sequences.first().copied().unwrap_or(0),
                )?;
                conn.execute(
                    &format!("DELETE FROM {table} WHERE {owner_col} = ?1 AND memory_id = ?2"),
                    params![owner_id, memory_id],
                )?;
                Ok(true)
            },
        )?;
        Ok(true)
    }

    pub fn clear(&self, owner_id: &str) -> StorageResult<usize> {
        let memories = self.list(owner_id)?;
        if memories.is_empty() {
            return Ok(0);
        }
        let agg = self.tables.aggregate_type;
        let table = self.tables.table;
        let owner_col = self.tables.owner_column;
        let owner_key = self.tables.owner_key;
        let command_id = format!("{agg}:clear:{}", Uuid::new_v4());
        let events: Vec<OrchestrationEventInput> = memories
            .iter()
            .map(|mem| {
                let mut payload_obj = serde_json::Map::new();
                payload_obj.insert(
                    owner_key.to_string(),
                    Value::String(owner_id.to_string()),
                );
                payload_obj.insert("memoryId".to_string(), Value::String(mem.id.clone()));
                new_memory_event(
                    agg,
                    &mem.id,
                    &format!("{agg}.deleted"),
                    Value::Object(payload_obj),
                )
            })
            .collect();
        let count = memories.len();
        self.database.dispatch_with_projection(
            &command_id,
            None,
            &events,
            |conn, sequences| {
                for (idx, mem) in memories.iter().enumerate() {
                    forget_events_before_tx(
                        conn,
                        agg,
                        &mem.id,
                        sequences.get(idx).copied().unwrap_or(0),
                    )?;
                }
                conn.execute(
                    &format!("DELETE FROM {table} WHERE {owner_col} = ?1"),
                    params![owner_id],
                )?;
                Ok(count)
            },
        )?;
        Ok(count)
    }

    fn find_by_normalized_text(
        &self,
        owner_id: &str,
        normalized_text: &str,
    ) -> StorageResult<Option<MemoryEntry>> {
        self.database.with_conn(|conn| {
            let sql = format!(
                "SELECT {} FROM {} WHERE {} = ?1 AND normalized_text = ?2",
                self.columns, self.tables.table, self.tables.owner_column
            );
            let mut stmt = conn.prepare(&sql)?;
            let mut rows = stmt.query(params![owner_id, normalized_text])?;
            match rows.next()? {
                Some(row) => Ok(Some(self.decode_row(row)?)),
                None => Ok(None),
            }
        })
    }

    pub fn save_base(&self, owner_id: &str, input: SaveMemoryInput) -> StorageResult<MemoryEntry> {
        let text = validate_memory_text(&input.text)?;
        let normalized_text = normalize_memory_text(&text);
        if let Some(duplicate) = self.find_by_normalized_text(owner_id, &normalized_text)?
            && Some(duplicate.id.as_str()) != input.memory_id.as_deref()
        {
            if let Some(old_id) = &input.memory_id {
                self.delete(owner_id, old_id)?;
            }
            if self.tables.site_column
                && let Some(req_site) = input.site
                && req_site != duplicate.site
            {
                return self.save_base(
                    owner_id,
                    SaveMemoryInput {
                        memory_id: Some(duplicate.id),
                        text: duplicate.text,
                        origin: duplicate.origin,
                        source_turn_id: duplicate.source_turn_id,
                        command_id: None,
                        uncapped: false,
                        site: Some(req_site),
                    },
                );
            }
            return Ok(duplicate);
        }
        self.persist_memory_entry(owner_id, text, normalized_text, input)
    }
}

impl MemoryStore {
    fn persist_memory_entry(
        &self,
        owner_id: &str,
        text: String,
        normalized_text: String,
        input: SaveMemoryInput,
    ) -> StorageResult<MemoryEntry> {
        let previous = match &input.memory_id {
            Some(mid) => {
                let found = self.get(owner_id, mid)?;
                if found.is_none() {
                    return Err(StorageError::NotFound(
                        "This memory no longer exists.".to_string(),
                    ));
                }
                found
            }
            None => None,
        };
        if previous.is_none() && !input.uncapped {
            let lim = self.limit();
            if self.count(owner_id)? >= lim {
                return Err(StorageError::LimitExceeded(
                    (self.tables.limit_message_fn)(lim),
                ));
            }
        }
        let now = now_iso();
        let updated_at =
            monotonic_updated_at(previous.as_ref().map(|p| p.updated_at.as_str()), &now);
        let site = if self.tables.site_column {
            match input.site {
                None => previous.as_ref().and_then(|p| p.site.clone()),
                Some(opt) => opt.filter(|s| !s.is_empty()),
            }
        } else {
            None
        };
        let memory = MemoryEntry {
            id: previous
                .as_ref()
                .map_or_else(|| Uuid::new_v4().to_string(), |p| p.id.clone()),
            text,
            origin: input.origin,
            source_turn_id: input.source_turn_id,
            created_at: previous
                .as_ref()
                .map_or_else(|| now.clone(), |p| p.created_at.clone()),
            updated_at,
            site,
        };
        let agg = self.tables.aggregate_type;
        let table = self.tables.table;
        let owner_col = self.tables.owner_column;
        let owner_key = self.tables.owner_key;
        let site_col = self.tables.site_column;
        let event_type = format!(
            "{agg}.{}",
            if previous.is_some() { "updated" } else { "created" }
        );
        let command_id = input
            .command_id
            .unwrap_or_else(|| format!("{agg}:{event_type}:{}", Uuid::new_v4()));
        let mut mem_val = serde_json::to_value(&memory)?;
        if let Some(obj) = mem_val.as_object_mut() {
            obj.insert(
                owner_key.to_string(),
                Value::String(owner_id.to_string()),
            );
        }
        let event = new_memory_event(
            agg,
            &memory.id,
            &event_type,
            serde_json::json!({ "memory": mem_val }),
        );
        let receipt = self.database.dispatch_with_projection(
            &command_id,
            None,
            &[event],
            |conn, sequences| {
                let seq = sequences.first().copied().unwrap_or(0);
                if site_col {
                    conn.execute(
                        &format!(
                            "INSERT INTO {table} (
                               memory_id, {owner_col}, text, normalized_text, origin, source_turn_id,
                               created_at, updated_at, last_event_sequence, site
                             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)
                             ON CONFLICT(memory_id) DO UPDATE SET
                               text = excluded.text,
                               normalized_text = excluded.normalized_text,
                               origin = excluded.origin,
                               source_turn_id = excluded.source_turn_id,
                               updated_at = excluded.updated_at,
                               last_event_sequence = excluded.last_event_sequence,
                               site = excluded.site"
                        ),
                        params![
                            memory.id,
                            owner_id,
                            memory.text,
                            normalized_text,
                            memory.origin.as_str(),
                            memory.source_turn_id,
                            memory.created_at,
                            memory.updated_at,
                            seq,
                            memory.site,
                        ],
                    )?;
                } else {
                    conn.execute(
                        &format!(
                            "INSERT INTO {table} (
                               memory_id, {owner_col}, text, normalized_text, origin, source_turn_id,
                               created_at, updated_at, last_event_sequence
                             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)
                             ON CONFLICT(memory_id) DO UPDATE SET
                               text = excluded.text,
                               normalized_text = excluded.normalized_text,
                               origin = excluded.origin,
                               source_turn_id = excluded.source_turn_id,
                               updated_at = excluded.updated_at,
                               last_event_sequence = excluded.last_event_sequence"
                        ),
                        params![
                            memory.id,
                            owner_id,
                            memory.text,
                            normalized_text,
                            memory.origin.as_str(),
                            memory.source_turn_id,
                            memory.created_at,
                            memory.updated_at,
                            seq,
                        ],
                    )?;
                }
                Ok(memory.clone())
            },
        )?;
        Ok(receipt.result)
    }
}

#[derive(Clone)]
pub struct ChannelMemoryStore {
    inner: MemoryStore,
}

impl ChannelMemoryStore {
    pub fn new(database: OpenBotDatabase) -> Self {
        Self {
            inner: MemoryStore::for_tables(database, channel_memory_tables()),
        }
    }

    pub fn list(&self, channel_id: &str) -> StorageResult<Vec<ChannelMemory>> {
        Ok(self
            .inner
            .list(channel_id)?
            .into_iter()
            .map(|entry| ChannelMemory {
                entry,
                channel_id: channel_id.to_string(),
            })
            .collect())
    }

    pub fn get(&self, channel_id: &str, memory_id: &str) -> StorageResult<Option<ChannelMemory>> {
        Ok(self.inner.get(channel_id, memory_id)?.map(|entry| ChannelMemory {
            entry,
            channel_id: channel_id.to_string(),
        }))
    }

    pub fn count(&self, channel_id: &str) -> StorageResult<usize> {
        self.inner.count(channel_id)
    }

    pub fn limit(&self) -> usize {
        self.inner.limit()
    }

    pub fn create_manual(&self, channel_id: &str, text: &str) -> StorageResult<ChannelMemory> {
        Ok(ChannelMemory {
            entry: self.inner.create_manual(channel_id, text)?,
            channel_id: channel_id.to_string(),
        })
    }

    pub fn update_manual(
        &self,
        channel_id: &str,
        memory_id: &str,
        text: &str,
    ) -> StorageResult<ChannelMemory> {
        Ok(ChannelMemory {
            entry: self.inner.update_manual(channel_id, memory_id, text)?,
            channel_id: channel_id.to_string(),
        })
    }
}

impl ChannelMemoryStore {
    pub fn save_from_tool(
        &self,
        channel_id: &str,
        text: &str,
        source_turn_id: &str,
        command_id: &str,
    ) -> StorageResult<ChannelMemory> {
        let saved = self.inner.save_automatic_entry(
            channel_id,
            SaveAutomaticMemory {
                memory_id: None,
                text: text.to_string(),
                source_turn_id: source_turn_id.to_string(),
                expected_updated_at: None,
                command_id: Some(command_id.to_string()),
                site: None,
            },
        )?;
        let Some(entry) = saved else {
            return Err(StorageError::NotFound(
                "This memory no longer exists.".to_string(),
            ));
        };
        Ok(ChannelMemory {
            entry,
            channel_id: channel_id.to_string(),
        })
    }

    pub fn delete(&self, channel_id: &str, memory_id: &str) -> StorageResult<bool> {
        self.inner.delete(channel_id, memory_id)
    }

    pub fn clear(&self, channel_id: &str) -> StorageResult<usize> {
        self.inner.clear(channel_id)
    }

    pub fn delete_by_text(&self, channel_id: &str, text: &str) -> StorageResult<bool> {
        let target = text.trim();
        if target.is_empty() {
            return Ok(false);
        }
        let entries = self.list(channel_id)?;
        let target_lower = target.to_lowercase();
        let matched = entries
            .iter()
            .find(|e| e.entry.text == target)
            .or_else(|| {
                entries
                    .iter()
                    .find(|e| e.entry.text.to_lowercase() == target_lower)
            })
            .or_else(|| {
                let folded = fold_memory_text(target);
                let similar: Vec<&ChannelMemory> = entries
                    .iter()
                    .filter(|e| fold_memory_text(&e.entry.text) == folded)
                    .collect();
                (similar.len() == 1).then(|| similar[0])
            });
        match matched {
            Some(mem) => self.delete(channel_id, &mem.entry.id),
            None => Ok(false),
        }
    }
}

#[derive(Clone)]
pub struct MessagingMemoryStore {
    inner: MemoryStore,
}

impl MessagingMemoryStore {
    pub fn new(database: OpenBotDatabase) -> Self {
        Self {
            inner: MemoryStore::for_tables(database, messaging_memory_tables()),
        }
    }

    pub fn list(&self, connection_id: &str) -> StorageResult<Vec<MemoryEntry>> {
        self.inner.list(connection_id)
    }

    pub fn get(&self, connection_id: &str, memory_id: &str) -> StorageResult<Option<MemoryEntry>> {
        self.inner.get(connection_id, memory_id)
    }

    pub fn count(&self, connection_id: &str) -> StorageResult<usize> {
        self.inner.count(connection_id)
    }

    pub fn limit(&self) -> usize {
        self.inner.limit()
    }

    pub fn create_manual(&self, connection_id: &str, text: &str) -> StorageResult<MemoryEntry> {
        self.inner.create_manual(connection_id, text)
    }

    pub fn update_manual(
        &self,
        connection_id: &str,
        memory_id: &str,
        text: &str,
    ) -> StorageResult<MemoryEntry> {
        self.inner.update_manual(connection_id, memory_id, text)
    }

    pub fn save_from_tool(
        &self,
        connection_id: &str,
        text: &str,
        memory_id: Option<&str>,
        source_turn_id: &str,
        command_id: &str,
    ) -> StorageResult<MemoryEntry> {
        let saved = self.inner.save_automatic_entry(
            connection_id,
            SaveAutomaticMemory {
                memory_id: memory_id.map(str::to_string),
                text: text.to_string(),
                source_turn_id: source_turn_id.to_string(),
                expected_updated_at: None,
                command_id: Some(command_id.to_string()),
                site: None,
            },
        )?;
        saved.ok_or_else(|| StorageError::NotFound("This memory no longer exists.".to_string()))
    }

    pub fn delete(&self, connection_id: &str, memory_id: &str) -> StorageResult<bool> {
        self.inner.delete(connection_id, memory_id)
    }

    pub fn clear(&self, connection_id: &str) -> StorageResult<usize> {
        self.inner.clear(connection_id)
    }
}

/// Exact port of `initializeAgentMemorySelection` (`agent-memory-selection.ts:L6-L35`).
pub fn initialize_agent_memory_selection(
    conn: &Connection,
    agent_id: &str,
    budget_bytes: usize,
) -> StorageResult<()> {
    let mut stmt = conn.prepare(
        "SELECT m.memory_id, m.text, m.origin, m.site, s.inclusion, COALESCE(s.user_controlled, 0)
         FROM projection_agent_memories m
         LEFT JOIN agent_memory_selections s USING(memory_id)
         WHERE m.agent_id = ?1
         ORDER BY CASE m.origin WHEN 'manual' THEN 0 ELSE 1 END, m.updated_at DESC, m.memory_id",
    )?;
    let rows = stmt.query_map(params![agent_id], |r| {
        Ok((
            r.get::<_, String>(0)?,
            r.get::<_, String>(1)?,
            r.get::<_, String>(2)?,
            r.get::<_, Option<String>>(3)?,
            r.get::<_, Option<String>>(4)?,
            r.get::<_, i64>(5)? == 1,
        ))
    })?;
    let mut entries = Vec::new();
    for item in rows {
        let (id, text, origin_str, site, inc, user_ctrl) = item?;
        let origin = AgentMemoryOrigin::parse(&origin_str)?;
        entries.push((
            ContextMemory {
                id,
                text,
                origin,
                site: site.filter(|s| !s.is_empty()),
            },
            user_ctrl,
            inc.as_deref() == Some("essential"),
        ));
    }
    let mut selected: Vec<ContextMemory> = entries
        .iter()
        .filter(|(_, user_ctrl, essential)| *user_ctrl && *essential)
        .map(|(mem, _, _)| mem.clone())
        .collect();
    for (mem, user_ctrl, _) in entries {
        if user_ctrl {
            continue;
        }
        selected.push(mem.clone());
        let fits = essential_memory_bytes(&selected) <= budget_bytes;
        if !fits {
            selected.pop();
        }
        let inclusion_str = if fits { "essential" } else { "searchable" };
        conn.execute(
            "INSERT INTO agent_memory_selections(memory_id, inclusion) VALUES (?1, ?2)
             ON CONFLICT(memory_id) DO UPDATE SET inclusion = excluded.inclusion, revision = revision + 1
             WHERE user_controlled = 0",
            params![mem.id, inclusion_str],
        )?;
    }
    Ok(())
}

#[derive(Debug, Clone)]
pub struct SaveAutomaticMemoryInput {
    pub agent_id: String,
    pub memory_id: Option<String>,
    pub text: String,
    pub source_turn_id: String,
    pub expected_updated_at: Option<Option<String>>,
    pub expected_selection_revision: Option<i64>,
    pub site: Option<String>,
}

#[derive(Clone)]
pub struct AgentMemoryStore {
    inner: MemoryStore,
    budget_bytes: usize,
}

impl AgentMemoryStore {
    pub fn new(database: OpenBotDatabase) -> Self {
        Self::with_config(
            database,
            Arc::new(|| DEFAULT_AGENT_MEMORY_LIMIT),
            AGENT_MEMORY_CONTEXT_BUDGET_BYTES,
        )
    }

    pub fn with_config(
        database: OpenBotDatabase,
        limit_fn: Arc<dyn Fn() -> usize + Send + Sync>,
        budget_bytes: usize,
    ) -> Self {
        Self {
            inner: MemoryStore::for_tables(database, agent_memory_tables(limit_fn)),
            budget_bytes,
        }
    }

    pub fn database(&self) -> &OpenBotDatabase {
        self.inner.database()
    }

    pub fn limit(&self) -> usize {
        self.inner.limit()
    }

    pub fn budget_bytes(&self) -> usize {
        self.budget_bytes
    }

    pub fn count(&self, agent_id: &str) -> StorageResult<usize> {
        self.inner.count(agent_id)
    }

    pub fn list(&self, agent_id: &str) -> StorageResult<Vec<AgentMemory>> {
        Ok(self
            .inner
            .list(agent_id)?
            .into_iter()
            .map(|entry| AgentMemory {
                entry,
                agent_id: agent_id.to_string(),
            })
            .collect())
    }

    pub fn get(&self, agent_id: &str, memory_id: &str) -> StorageResult<Option<AgentMemory>> {
        Ok(self.inner.get(agent_id, memory_id)?.map(|entry| AgentMemory {
            entry,
            agent_id: agent_id.to_string(),
        }))
    }

    pub fn essential_for(&self, agent_id: &str) -> StorageResult<Vec<AgentMemory>> {
        let essential_ids: HashSet<String> = self
            .list_selections(agent_id)?
            .into_iter()
            .filter(|s| s.inclusion == MemoryInclusion::Essential)
            .map(|s| s.memory_id)
            .collect();
        Ok(self
            .list(agent_id)?
            .into_iter()
            .filter(|m| essential_ids.contains(&m.entry.id))
            .collect())
    }

    pub fn create_manual(&self, agent_id: &str, text: &str) -> StorageResult<AgentMemory> {
        let entry = self.save_agent_memory(
            agent_id,
            SaveMemoryInput {
                memory_id: None,
                text: text.to_string(),
                origin: AgentMemoryOrigin::Manual,
                source_turn_id: None,
                command_id: None,
                uncapped: false,
                site: None,
            },
        )?;
        Ok(AgentMemory {
            entry,
            agent_id: agent_id.to_string(),
        })
    }

    pub fn update_manual(
        &self,
        agent_id: &str,
        memory_id: &str,
        text: &str,
    ) -> StorageResult<AgentMemory> {
        let entry = self.save_agent_memory(
            agent_id,
            SaveMemoryInput {
                memory_id: Some(memory_id.to_string()),
                text: text.to_string(),
                origin: AgentMemoryOrigin::Manual,
                source_turn_id: None,
                command_id: None,
                uncapped: false,
                site: None,
            },
        )?;
        Ok(AgentMemory {
            entry,
            agent_id: agent_id.to_string(),
        })
    }

    pub fn save_automatic(
        &self,
        input: SaveAutomaticMemoryInput,
    ) -> StorageResult<Option<AgentMemory>> {
        if let (Some(mid), Some(exp_rev)) = (&input.memory_id, input.expected_selection_revision) {
            let cur_rev = self.get_selection(&input.agent_id, mid)?.map(|s| s.revision);
            if cur_rev != Some(exp_rev) {
                return Ok(None);
            }
        }
        if let (Some(mid), Some(exp_upd)) = (&input.memory_id, &input.expected_updated_at) {
            let cur_upd = self
                .get(&input.agent_id, mid)?
                .map(|m| m.entry.updated_at);
            if cur_upd.as_deref() != exp_upd.as_deref() {
                return Ok(None);
            }
        }
        let entry = self.save_agent_memory(
            &input.agent_id,
            SaveMemoryInput {
                memory_id: input.memory_id,
                text: input.text,
                origin: AgentMemoryOrigin::Automatic,
                source_turn_id: Some(input.source_turn_id),
                command_id: None,
                uncapped: false,
                site: input.site.map(Some),
            },
        )?;
        Ok(Some(AgentMemory {
            entry,
            agent_id: input.agent_id,
        }))
    }

    pub fn delete(&self, agent_id: &str, memory_id: &str) -> StorageResult<bool> {
        self.inner.delete(agent_id, memory_id)
    }

    pub fn delete_with_expected(
        &self,
        agent_id: &str,
        memory_id: &str,
        expected_updated_at: Option<Option<&str>>,
    ) -> StorageResult<bool> {
        self.inner
            .delete_with_expected(agent_id, memory_id, expected_updated_at)
    }

    pub fn clear(&self, agent_id: &str) -> StorageResult<usize> {
        self.inner.clear(agent_id)
    }

    pub fn duplicate(
        &self,
        source_agent_id: &str,
        target_agent_id: &str,
    ) -> StorageResult<Vec<AgentMemory>> {
        self.database().with_tx(|_| {
            let source_selections = self.list_selections(source_agent_id)?;
            let originals = self.list(source_agent_id)?;
            let copied = self.inner.duplicate(source_agent_id, target_agent_id)?;
            self.database().with_conn(|conn| {
                for (idx, mem) in copied.iter().enumerate() {
                    if let Some(orig) = originals.get(idx)
                        && let Some(sel) = source_selections
                            .iter()
                            .find(|s| s.memory_id == orig.entry.id)
                    {
                        conn.execute(
                            "UPDATE agent_memory_selections
                             SET inclusion = ?1, user_controlled = ?2
                             WHERE memory_id = ?3",
                            params![
                                sel.inclusion.as_str(),
                                i64::from(sel.user_controlled),
                                mem.id,
                            ],
                        )?;
                    }
                }
                Ok(())
            })?;
            self.assert_budget(target_agent_id)?;
            Ok(copied
                .into_iter()
                .map(|entry| AgentMemory {
                    entry,
                    agent_id: target_agent_id.to_string(),
                })
                .collect())
        })
    }
}

impl AgentMemoryStore {
    pub fn list_for_sites(
        &self,
        agent_id: &str,
        sites: &[String],
        limit: usize,
    ) -> StorageResult<Vec<AgentMemory>> {
        if sites.is_empty() || limit == 0 {
            return Ok(Vec::new());
        }
        let ids: Vec<String> = self.database().with_conn(|conn| {
            let placeholders = std::iter::repeat_n("?", sites.len())
                .collect::<Vec<_>>()
                .join(", ");
            let sql = format!(
                "SELECT m.memory_id FROM projection_agent_memories m
                 LEFT JOIN agent_memory_selections s ON s.memory_id = m.memory_id
                 WHERE m.agent_id = ? AND m.site IN ({placeholders})
                   AND COALESCE(s.inclusion, 'searchable') <> 'essential'
                 ORDER BY m.updated_at DESC, m.memory_id LIMIT ?"
            );
            let limit_i64 = limit as i64;
            let mut sql_params: Vec<&dyn rusqlite::ToSql> = Vec::with_capacity(2 + sites.len());
            sql_params.push(&agent_id);
            for s in sites {
                sql_params.push(s);
            }
            sql_params.push(&limit_i64);
            let mut stmt = conn.prepare(&sql)?;
            let rows = stmt.query_map(params_from_iter(sql_params), |r| r.get::<_, String>(0))?;
            let mut out = Vec::new();
            for item in rows {
                out.push(item?);
            }
            Ok(out)
        })?;
        let mut out = Vec::with_capacity(ids.len());
        for id in ids {
            if let Some(mem) = self.get(agent_id, &id)? {
                out.push(mem);
            }
        }
        Ok(out)
    }

    pub fn for_host(
        &self,
        agent_id: &str,
        hostname: &str,
        limit: usize,
    ) -> StorageResult<Vec<AgentMemory>> {
        self.list_for_sites(agent_id, &memory_sites_for_host(hostname), limit)
    }

    pub fn list_selections(&self, agent_id: &str) -> StorageResult<Vec<AgentMemorySelection>> {
        self.database().with_conn(|conn| {
            let mut stmt = conn.prepare(
                "SELECT s.memory_id, s.inclusion, s.user_controlled, s.revision
                 FROM agent_memory_selections s
                 JOIN projection_agent_memories m ON m.memory_id = s.memory_id
                 WHERE m.agent_id = ?1 ORDER BY s.memory_id",
            )?;
            let rows = stmt.query_map(params![agent_id], |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    r.get::<_, String>(1)?,
                    r.get::<_, i64>(2)? == 1,
                    r.get::<_, i64>(3)?,
                ))
            })?;
            let mut out = Vec::new();
            for item in rows {
                let (mid, inc_raw, user_ctrl, rev) = item?;
                out.push(AgentMemorySelection {
                    memory_id: mid,
                    inclusion: MemoryInclusion::parse(&inc_raw)?,
                    user_controlled: user_ctrl,
                    revision: rev,
                });
            }
            Ok(out)
        })
    }

    pub fn get_selection(
        &self,
        agent_id: &str,
        memory_id: &str,
    ) -> StorageResult<Option<AgentMemorySelection>> {
        Ok(self
            .list_selections(agent_id)?
            .into_iter()
            .find(|s| s.memory_id == memory_id))
    }

    pub fn selection_state(&self, agent_id: &str) -> StorageResult<AgentMemorySelectionState> {
        let selections = self.list_selections(agent_id)?;
        let essential_ids: HashSet<&str> = selections
            .iter()
            .filter(|s| s.inclusion == MemoryInclusion::Essential)
            .map(|s| s.memory_id.as_str())
            .collect();
        let essential_ctx: Vec<ContextMemory> = self
            .list(agent_id)?
            .iter()
            .filter(|m| essential_ids.contains(m.entry.id.as_str()))
            .map(ContextMemory::from)
            .collect();
        Ok(AgentMemorySelectionState {
            selections,
            used_bytes: essential_memory_bytes(&essential_ctx),
            budget_bytes: self.budget_bytes,
        })
    }

    pub fn initialize_selection(&self, agent_id: &str) -> StorageResult<()> {
        self.database().with_tx(|conn| {
            initialize_agent_memory_selection(conn, agent_id, self.budget_bytes)?;
            self.assert_budget(agent_id)
        })
    }

    pub fn set_inclusions(
        &self,
        agent_id: &str,
        changes: &[AgentMemorySelectionChange],
        actor: MemorySelectionActor,
    ) -> StorageResult<AgentMemorySelectionState> {
        self.set_inclusions_checked(agent_id, changes, actor)
            .map_err(|e| e.into())
    }

    pub fn set_inclusions_checked(
        &self,
        agent_id: &str,
        changes: &[AgentMemorySelectionChange],
        actor: MemorySelectionActor,
    ) -> Result<AgentMemorySelectionState, AgentMemorySelectionError> {
        let map_db_err = |e: StorageError| AgentMemorySelectionError {
            code: MemorySelectionErrorCode::Conflict,
            message: e.to_string(),
        };
        if changes.len() > MEMORY_SELECTION_BATCH_LIMIT {
            return Err(AgentMemorySelectionError {
                code: MemorySelectionErrorCode::Conflict,
                message: "The memory changed. Reload it and try again.".to_string(),
            });
        }
        let mut result_holder: Option<Result<AgentMemorySelectionState, AgentMemorySelectionError>> =
            None;
        let tx_res = self.database().with_tx(|conn| {
            let mut seen_ids = HashSet::new();
            let mut updates = Vec::with_capacity(changes.len());
            for change in changes {
                let current = self.get_selection(agent_id, &change.memory_id)?;
                let Some(current) = current else {
                    result_holder = Some(Err(AgentMemorySelectionError {
                        code: MemorySelectionErrorCode::Conflict,
                        message: "The memory changed. Reload it and try again.".to_string(),
                    }));
                    return Ok(());
                };
                if current.revision != change.expected_revision
                    || !seen_ids.insert(change.memory_id.clone())
                {
                    result_holder = Some(Err(AgentMemorySelectionError {
                        code: MemorySelectionErrorCode::Conflict,
                        message: "The memory changed. Reload it and try again.".to_string(),
                    }));
                    return Ok(());
                }
                if actor == MemorySelectionActor::Agent
                    && (current.user_controlled
                        || change.inclusion == MemoryInclusionChangeValue::Automatic)
                {
                    result_holder = Some(Err(AgentMemorySelectionError {
                        code: MemorySelectionErrorCode::UserControlled,
                        message: "The user controls this memory selection. Keep the user's choice."
                            .to_string(),
                    }));
                    return Ok(());
                }
                let next_inclusion = match change.inclusion {
                    MemoryInclusionChangeValue::Automatic => current.inclusion,
                    MemoryInclusionChangeValue::Essential => MemoryInclusion::Essential,
                    MemoryInclusionChangeValue::Searchable => MemoryInclusion::Searchable,
                };
                updates.push(AgentMemorySelection {
                    memory_id: change.memory_id.clone(),
                    inclusion: next_inclusion,
                    user_controlled: actor == MemorySelectionActor::User
                        && change.inclusion != MemoryInclusionChangeValue::Automatic,
                    revision: current.revision + 1,
                });
            }
            let updated_map: HashMap<&str, &AgentMemorySelection> = updates
                .iter()
                .map(|u| (u.memory_id.as_str(), u))
                .collect();
            let all_selections = self.list_selections(agent_id)?;
            let essential_ids: HashSet<&str> = all_selections
                .iter()
                .map(|s| updated_map.get(s.memory_id.as_str()).copied().unwrap_or(s))
                .filter(|s| s.inclusion == MemoryInclusion::Essential)
                .map(|s| s.memory_id.as_str())
                .collect();
            let proposed: Vec<ContextMemory> = self
                .list(agent_id)?
                .iter()
                .filter(|m| essential_ids.contains(m.entry.id.as_str()))
                .map(ContextMemory::from)
                .collect();
            if essential_memory_bytes(&proposed) > self.budget_bytes {
                result_holder = Some(Err(AgentMemorySelectionError {
                    code: MemorySelectionErrorCode::Budget,
                    message: "The essential memories exceed the prompt limit. Set an entry to search when needed, then try again.".to_string(),
                }));
                return Ok(());
            }
            for sel in &updates {
                conn.execute(
                    "UPDATE agent_memory_selections
                     SET inclusion = ?1, user_controlled = ?2, revision = ?3
                     WHERE memory_id = ?4",
                    params![
                        sel.inclusion.as_str(),
                        i64::from(sel.user_controlled),
                        sel.revision,
                        sel.memory_id,
                    ],
                )?;
            }
            result_holder = Some(Ok(self.selection_state(agent_id)?));
            Ok(())
        });
        tx_res.map_err(map_db_err)?;
        result_holder.unwrap_or_else(|| {
            Err(AgentMemorySelectionError {
                code: MemorySelectionErrorCode::Conflict,
                message: "The memory changed. Reload it and try again.".to_string(),
            })
        })
    }
}

impl AgentMemoryStore {
    pub fn search(
        &self,
        agent_id: &str,
        query: &str,
        limit: usize,
    ) -> StorageResult<AgentMemorySearchResult> {
        self.search_checked(agent_id, query, limit)
            .map_err(|e| e.into())
    }

    pub fn search_checked(
        &self,
        agent_id: &str,
        query: &str,
        limit: usize,
    ) -> Result<AgentMemorySearchResult, AgentMemorySelectionError> {
        let terms = extract_search_word_terms(query);
        if utf16_len(query) > 256 || terms.is_empty() || !(1..=10).contains(&limit) {
            return Err(AgentMemorySelectionError {
                code: MemorySelectionErrorCode::Query,
                message: "Enter one or more search words, up to 256 characters.".to_string(),
            });
        }
        let mut clauses: Vec<String> = Vec::new();
        let mut seen_clauses = HashSet::new();
        for term in &terms {
            let escaped = term.replace('"', "\"\"");
            let letters: Vec<char> = term.chars().collect();
            let exact = format!("\"{escaped}\"");
            if seen_clauses.insert(exact.clone()) {
                clauses.push(exact);
            }
            if letters.len() >= 6 {
                let prefix_len = usize::max(4, letters.len() - 3);
                let prefix: String = letters[..prefix_len].iter().collect();
                let prefix_escaped = prefix.replace('"', "\"\"");
                let prefix_clause = format!("\"{prefix_escaped}\"*");
                if seen_clauses.insert(prefix_clause.clone()) {
                    clauses.push(prefix_clause);
                }
            }
        }
        let expression = clauses.join(" OR ");
        let map_err = |e: StorageError| AgentMemorySelectionError {
            code: MemorySelectionErrorCode::Missing,
            message: e.to_string(),
        };

        let indexed: Vec<String> = self
            .database()
            .with_conn(|conn| {
                let mut stmt = conn.prepare(
                    "SELECT m.memory_id
                     FROM agent_memory_search
                     JOIN projection_agent_memories m ON m.rowid = agent_memory_search.rowid
                     WHERE agent_memory_search MATCH ?1 AND m.agent_id = ?2
                     ORDER BY bm25(agent_memory_search), m.updated_at DESC, m.memory_id
                     LIMIT ?3",
                )?;
                let rows = stmt.query_map(
                    params![expression, agent_id, (limit + 1) as i64],
                    |r| r.get::<_, String>(0),
                )?;
                let mut out = Vec::new();
                for item in rows {
                    out.push(item?);
                }
                Ok(out)
            })
            .map_err(map_err)?;

        let ids: Vec<String> = if indexed.len() > limit {
            indexed
        } else {
            let mut unique_terms: Vec<String> = Vec::new();
            let mut seen_t = HashSet::new();
            for t in &terms {
                if seen_t.insert(t.clone()) {
                    unique_terms.push(t.clone());
                }
            }
            let like_patterns: Vec<String> =
                unique_terms.iter().map(|t| format!("%{t}%")).collect();
            let fallback_ids: Vec<String> = self
                .database()
                .with_conn(|conn| {
                    let ors = std::iter::repeat_n("text LIKE ?", like_patterns.len())
                        .collect::<Vec<_>>()
                        .join(" OR ");
                    let sql = format!(
                        "SELECT memory_id, text FROM projection_agent_memories
                         WHERE agent_id = ? AND ({ors})
                         ORDER BY updated_at DESC, memory_id"
                    );
                    let mut sql_params: Vec<&dyn rusqlite::ToSql> =
                        Vec::with_capacity(1 + like_patterns.len());
                    sql_params.push(&agent_id);
                    for pat in &like_patterns {
                        sql_params.push(pat);
                    }
                    let mut stmt = conn.prepare(&sql)?;
                    let rows = stmt.query_map(params_from_iter(sql_params), |r| {
                        Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?))
                    })?;
                    let mut matched_ids = Vec::new();
                    for item in rows {
                        let (mid, text) = item?;
                        if unique_terms
                            .iter()
                            .any(|term| matches_unindexed_term(&text, term))
                        {
                            matched_ids.push(mid);
                        }
                    }
                    Ok(matched_ids)
                })
                .map_err(map_err)?;
            let mut combined = Vec::with_capacity(limit + 1);
            let mut seen_ids = HashSet::new();
            for id in indexed.into_iter().chain(fallback_ids) {
                if seen_ids.insert(id.clone()) {
                    combined.push(id);
                    if combined.len() >= limit + 1 {
                        break;
                    }
                }
            }
            combined
        };

        let mut result = AgentMemorySearchResult {
            memories: Vec::new(),
            has_more: ids.len() > limit,
        };
        for id in ids.into_iter().take(limit) {
            let mem = self.retrieved(agent_id, &id)?;
            let mut candidate = result.memories.clone();
            candidate.push(mem.clone());
            let probe = AgentMemorySearchResult {
                memories: candidate,
                has_more: false,
            };
            let byte_len = serde_json::to_string(&probe)
                .map_or(usize::MAX, |s| s.len());
            if byte_len > self.budget_bytes {
                result.has_more = true;
                break;
            }
            result.memories.push(mem);
        }
        Ok(result)
    }

    pub fn list_page(
        &self,
        agent_id: &str,
        after: Option<&str>,
    ) -> StorageResult<AgentMemoryListPage> {
        let ids: Vec<String> = self.database().with_conn(|conn| {
            let mut stmt = conn.prepare(
                "SELECT memory_id FROM projection_agent_memories
                 WHERE agent_id = ?1 AND (?2 IS NULL OR memory_id > ?3)
                 ORDER BY memory_id LIMIT 26",
            )?;
            let rows =
                stmt.query_map(params![agent_id, after, after], |r| r.get::<_, String>(0))?;
            let mut out = Vec::new();
            for item in rows {
                out.push(item?);
            }
            Ok(out)
        })?;
        let mut result = AgentMemoryListPage {
            memories: Vec::new(),
            has_more: ids.len() > MEMORY_LIST_PAGE_LIMIT,
            next_cursor: None,
        };
        for id in ids.into_iter().take(MEMORY_LIST_PAGE_LIMIT) {
            let mem = self
                .retrieved(agent_id, &id)
                .map_err(StorageError::from)?;
            let mut candidate = result.memories.clone();
            candidate.push(mem.clone());
            let probe = AgentMemoryListPage {
                memories: candidate,
                has_more: true,
                next_cursor: Some(id.clone()),
            };
            let byte_len = serde_json::to_string(&probe)
                .map_or(usize::MAX, |s| s.len());
            if byte_len > self.budget_bytes {
                result.has_more = true;
                break;
            }
            result.memories.push(mem);
        }
        result.next_cursor = if result.has_more {
            result.memories.last().map(|m| m.memory.entry.id.clone())
        } else {
            None
        };
        Ok(result)
    }
}

impl AgentMemoryStore {
    fn save_agent_memory(
        &self,
        agent_id: &str,
        input: SaveMemoryInput,
    ) -> StorageResult<MemoryEntry> {
        self.database().with_tx(|conn| {
            let trimmed = input.text.trim();
            let duplicate = match &input.memory_id {
                Some(mid) => self
                    .list(agent_id)?
                    .into_iter()
                    .find(|m| m.entry.id != *mid && m.entry.text == trimmed),
                None => None,
            };
            let merge_selection = match (&input.memory_id, &duplicate) {
                (Some(src_id), Some(dup)) => {
                    Some(self.merged_selection(agent_id, src_id, &dup.entry.id)?)
                }
                _ => None,
            };
            if duplicate.is_none()
                && let Some(mid) = &input.memory_id
                && self
                    .get_selection(agent_id, mid)?
                    .is_some_and(|s| s.inclusion == MemoryInclusion::Essential)
            {
                let essential_ids: HashSet<String> = self
                    .list_selections(agent_id)?
                    .into_iter()
                    .filter(|s| s.inclusion == MemoryInclusion::Essential)
                    .map(|s| s.memory_id)
                    .collect();
                let proposed: Vec<ContextMemory> = self
                    .list(agent_id)?
                    .into_iter()
                    .filter(|m| essential_ids.contains(&m.entry.id))
                    .map(|m| {
                        if m.entry.id == *mid {
                            ContextMemory {
                                id: m.entry.id,
                                text: trimmed.to_string(),
                                origin: input.origin,
                                site: match &input.site {
                                    None => m.entry.site,
                                    Some(opt) => opt.clone(),
                                },
                            }
                        } else {
                            ContextMemory::from(&m)
                        }
                    })
                    .collect();
                if essential_memory_bytes(&proposed) > self.budget_bytes {
                    return Err(AgentMemorySelectionError {
                        code: MemorySelectionErrorCode::Budget,
                        message: "The essential memories exceed the prompt limit. Set an entry to search when needed, then try again.".to_string(),
                    }
                    .into());
                }
            }
            let saved = self.inner.save_base(agent_id, input)?;
            if let Some(merged) = merge_selection {
                conn.execute(
                    "UPDATE agent_memory_selections
                     SET inclusion = ?1, user_controlled = ?2, revision = revision + 1
                     WHERE memory_id = ?3",
                    params![
                        merged.inclusion.as_str(),
                        i64::from(merged.user_controlled),
                        saved.id,
                    ],
                )?;
                self.assert_budget(agent_id)?;
            }
            Ok(saved)
        })
    }

    fn merged_selection(
        &self,
        agent_id: &str,
        source_id: &str,
        target_id: &str,
    ) -> StorageResult<AgentMemorySelection> {
        let source = self.get_selection(agent_id, source_id)?.ok_or_else(|| {
            AgentMemorySelectionError {
                code: MemorySelectionErrorCode::Missing,
                message: "This memory no longer exists.".to_string(),
            }
        })?;
        let target = self.get_selection(agent_id, target_id)?.ok_or_else(|| {
            AgentMemorySelectionError {
                code: MemorySelectionErrorCode::Missing,
                message: "This memory no longer exists.".to_string(),
            }
        })?;
        if source.user_controlled
            && target.user_controlled
            && source.inclusion != target.inclusion
        {
            return Err(AgentMemorySelectionError {
                code: MemorySelectionErrorCode::Conflict,
                message: "The memory changed. Reload it and try again.".to_string(),
            }
            .into());
        }
        if target.user_controlled {
            return Ok(target);
        }
        if source.user_controlled {
            return Ok(AgentMemorySelection {
                memory_id: target_id.to_string(),
                ..source
            });
        }
        Ok(AgentMemorySelection {
            inclusion: if source.inclusion == MemoryInclusion::Essential {
                MemoryInclusion::Essential
            } else {
                target.inclusion
            },
            ..target
        })
    }

    fn assert_budget(&self, agent_id: &str) -> StorageResult<()> {
        if self.selection_state(agent_id)?.used_bytes > self.budget_bytes {
            return Err(AgentMemorySelectionError {
                code: MemorySelectionErrorCode::Budget,
                message: "The essential memories exceed the prompt limit. Set an entry to search when needed, then try again.".to_string(),
            }
            .into());
        }
        Ok(())
    }

    fn retrieved(
        &self,
        agent_id: &str,
        memory_id: &str,
    ) -> Result<RetrievedAgentMemory, AgentMemorySelectionError> {
        let missing = || AgentMemorySelectionError {
            code: MemorySelectionErrorCode::Missing,
            message: "This memory no longer exists.".to_string(),
        };
        let memory = self
            .get(agent_id, memory_id)
            .map_err(|_| missing())?
            .ok_or_else(missing)?;
        let selection = self
            .get_selection(agent_id, memory_id)
            .map_err(|_| missing())?
            .ok_or_else(missing)?;
        Ok(RetrievedAgentMemory {
            memory,
            inclusion: selection.inclusion,
            user_controlled: selection.user_controlled,
            revision: selection.revision,
        })
    }
}

#[derive(Debug, Clone, Default)]
pub struct StageRememberInput {
    pub memory_id: Option<String>,
    pub text: String,
    pub inclusion: Option<MemoryInclusion>,
    pub site: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum StagedMemoryStatus {
    Staged,
    Saved,
    Forgotten,
    Unchanged,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StagedMemoryOutcome {
    pub status: StagedMemoryStatus,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub memory_id: Option<Option<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub inclusion: Option<MemoryInclusion>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub site: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
}

#[derive(Debug, Clone)]
enum PendingMemoryMutation {
    Selection {
        call_id: String,
        agent_id: String,
        epoch: u64,
        changes: Vec<AgentMemorySelectionChange>,
    },
    Remember {
        call_id: String,
        agent_id: String,
        epoch: u64,
        memory_id: Option<String>,
        text: String,
        source_turn_id: String,
        expected_updated_at: Option<Option<String>>,
        expected_selection_revision: Option<i64>,
        duplicate: Option<(String, i64)>,
        inclusion: MemoryInclusion,
        site: Option<String>,
    },
    Forget {
        call_id: String,
        agent_id: String,
        epoch: u64,
        memory_id: String,
        expected_updated_at: String,
        expected_selection_revision: i64,
    },
}

impl PendingMemoryMutation {
    fn call_id(&self) -> &str {
        match self {
            Self::Selection { call_id, .. }
            | Self::Remember { call_id, .. }
            | Self::Forget { call_id, .. } => call_id,
        }
    }

    fn agent_id(&self) -> &str {
        match self {
            Self::Selection { agent_id, .. }
            | Self::Remember { agent_id, .. }
            | Self::Forget { agent_id, .. } => agent_id,
        }
    }

    fn epoch(&self) -> u64 {
        match self {
            Self::Selection { epoch, .. }
            | Self::Remember { epoch, .. }
            | Self::Forget { epoch, .. } => *epoch,
        }
    }

    fn target_memory_id(&self) -> Option<&str> {
        match self {
            Self::Selection { .. } => None,
            Self::Remember { memory_id, .. } => memory_id.as_deref(),
            Self::Forget { memory_id, .. } => Some(memory_id.as_str()),
        }
    }
}

#[derive(Default)]
struct StagedTurnState {
    pending: BTreeMap<String, Vec<PendingMemoryMutation>>,
    epochs: HashMap<String, u64>,
}

/// Optimistic intra-turn memory staging coordinator (`src/backend/agent/agent-memories.ts:L31-L775`).
#[derive(Clone)]
pub struct StagedTurnMemories {
    store: AgentMemoryStore,
    state: Arc<Mutex<StagedTurnState>>,
}

impl StagedTurnMemories {
    pub fn new(store: AgentMemoryStore) -> Self {
        Self {
            store,
            state: Arc::new(Mutex::new(StagedTurnState::default())),
        }
    }

    pub fn store(&self) -> &AgentMemoryStore {
        &self.store
    }

    pub fn epoch(&self, agent_id: &str) -> u64 {
        self.state
            .lock()
            .epochs
            .get(agent_id)
            .copied()
            .unwrap_or(0)
    }

    pub fn bump_epoch(&self, agent_id: &str) -> u64 {
        let mut guard = self.state.lock();
        let next = guard.epochs.get(agent_id).copied().unwrap_or(0) + 1;
        guard.epochs.insert(agent_id.to_string(), next);
        next
    }

    pub fn clear_agent_memories(&self, agent_id: &str) -> StorageResult<usize> {
        self.bump_epoch(agent_id);
        self.store.clear(agent_id)
    }

    pub fn clear_pending(&self) {
        self.state.lock().pending.clear();
    }

    fn push_staged(&self, turn_id: &str, mutation: PendingMemoryMutation) {
        let mut guard = self.state.lock();
        let list = guard.pending.entry(turn_id.to_string()).or_default();
        if !list.iter().any(|c| c.call_id() == mutation.call_id()) {
            list.push(mutation);
        }
    }

    fn stored_state_fingerprint(&self, agent_id: &str) -> StorageResult<String> {
        let memories = self.store.list(agent_id)?;
        let selection = self.store.list_selections(agent_id)?;
        Ok(serde_json::to_string(&(memories, selection))?)
    }

    fn finalize_stage_outcome(
        &self,
        turn_id: &str,
        agent_id: &str,
        turn_running: bool,
        applied: StagedMemoryStatus,
        mut outcome: StagedMemoryOutcome,
    ) -> StorageResult<StagedMemoryOutcome> {
        if turn_running {
            outcome.status = StagedMemoryStatus::Staged;
            return Ok(outcome);
        }
        let before = self.stored_state_fingerprint(agent_id)?;
        self.finish_turn(turn_id, "completed");
        let after = self.stored_state_fingerprint(agent_id)?;
        outcome.status = if before == after {
            StagedMemoryStatus::Unchanged
        } else {
            applied
        };
        Ok(outcome)
    }

    pub fn stage_set_inclusion(
        &self,
        turn_id: &str,
        call_id: &str,
        agent_id: &str,
        changes: Vec<AgentMemorySelectionChange>,
        turn_running: bool,
    ) -> StorageResult<StagedMemoryOutcome> {
        if changes.is_empty() || changes.len() > MEMORY_SELECTION_BATCH_LIMIT {
            return Err(StorageError::Validation(
                "Provide 1 to 25 memory selections with their current revisions.".to_string(),
            ));
        }
        let (all, mut essentials) = self.planned_context(agent_id, turn_id, call_id)?;
        let mut seen = HashSet::new();
        for change in &changes {
            if change.inclusion == MemoryInclusionChangeValue::Automatic {
                return Err(StorageError::Forbidden(
                    "The user controls this memory selection. Keep the user's choice.".to_string(),
                ));
            }
            let sel = self.store.get_selection(agent_id, &change.memory_id)?;
            let mem = self.store.get(agent_id, &change.memory_id)?;
            let (Some(sel), Some(mem)) = (sel, mem) else {
                return Err(StorageError::Conflict(
                    "The memory changed. Reload it and try again.".to_string(),
                ));
            };
            if sel.revision != change.expected_revision || !seen.insert(change.memory_id.clone()) {
                return Err(StorageError::Conflict(
                    "The memory changed. Reload it and try again.".to_string(),
                ));
            }
            if sel.user_controlled {
                return Err(StorageError::Forbidden(
                    "The user controls this memory selection. Keep the user's choice.".to_string(),
                ));
            }
            if change.inclusion == MemoryInclusionChangeValue::Essential {
                let ctx = all
                    .get(&mem.entry.id)
                    .cloned()
                    .unwrap_or_else(|| ContextMemory::from(&mem));
                essentials.insert(mem.entry.id.clone(), ctx);
            } else {
                essentials.remove(&change.memory_id);
            }
        }
        let essential_vec: Vec<ContextMemory> = essentials.into_values().collect();
        if essential_memory_bytes(&essential_vec) > self.store.budget_bytes() {
            return Err(StorageError::LimitExceeded(
                "The essential memories exceed the prompt limit. Set an entry to search when needed, then try again.".to_string(),
            ));
        }
        let epoch = self.epoch(agent_id);
        self.push_staged(
            turn_id,
            PendingMemoryMutation::Selection {
                call_id: call_id.to_string(),
                agent_id: agent_id.to_string(),
                epoch,
                changes,
            },
        );
        self.finalize_stage_outcome(
            turn_id,
            agent_id,
            turn_running,
            StagedMemoryStatus::Saved,
            StagedMemoryOutcome {
                status: StagedMemoryStatus::Staged,
                memory_id: None,
                inclusion: None,
                site: None,
                note: None,
            },
        )
    }
}

impl StagedTurnMemories {
    pub fn stage_remember(
        &self,
        turn_id: &str,
        call_id: &str,
        agent_id: &str,
        input: StageRememberInput,
        turn_running: bool,
    ) -> StorageResult<StagedMemoryOutcome> {
        let text = input.text.trim();
        if text.is_empty() {
            return Err(StorageError::Validation(
                "Memory text is required.".to_string(),
            ));
        }
        let text_u16 = utf16_len(text);
        if text_u16 > INPUT_LIMITS.agent_memory_text {
            return Err(StorageError::Validation(format!(
                "Memory text is too long. A memory has at most {} characters, and this text has {text_u16}. Save each fact as its own shorter memory.",
                INPUT_LIMITS.agent_memory_text
            )));
        }
        let normalized_site = match &input.site {
            Some(raw_site) => {
                let Some(s) = normalize_memory_site(raw_site) else {
                    return Err(StorageError::Validation(
                        "site must be a host name or a URL.".to_string(),
                    ));
                };
                Some(s)
            }
            None => None,
        };
        if let Some(mid) = &input.memory_id
            && !is_identifier(mid)
        {
            return Err(StorageError::Validation("memoryId is invalid.".to_string()));
        }
        let current = match &input.memory_id {
            Some(mid) => {
                let found = self.store.get(agent_id, mid)?;
                if found.is_none() {
                    return Err(StorageError::NotFound(
                        "This memory no longer exists. Call list_memories to read your current memories. To keep the fact, save it without memoryId.".to_string(),
                    ));
                }
                found
            }
            None => None,
        };

        let (planned_all, planned_essentials) = self.planned_context(agent_id, turn_id, call_id)?;
        let duplicate_entry = planned_all
            .iter()
            .find(|(k, m)| Some(k.as_str()) != input.memory_id.as_deref() && m.text == text)
            .map(|(k, m)| (k.clone(), m.clone()));
        let source_sel = match &current {
            Some(cur) => self.store.get_selection(agent_id, &cur.entry.id)?,
            None => None,
        };
        let dup_sel = match &duplicate_entry {
            Some((_, dup)) => self.store.get_selection(agent_id, &dup.id)?,
            None => None,
        };
        let selected_id = duplicate_entry
            .as_ref()
            .map(|(_, d)| d.id.as_str())
            .or_else(|| current.as_ref().map(|c| c.entry.id.as_str()));
        let selected_key = duplicate_entry
            .as_ref()
            .map(|(k, _)| k.as_str())
            .or_else(|| current.as_ref().map(|c| c.entry.id.as_str()));
        let active_sel = dup_sel.as_ref().or(source_sel.as_ref());

        if duplicate_entry.is_some() && source_sel.as_ref().is_some_and(|s| s.user_controlled) {
            return Err(StorageError::Forbidden(
                "The user controls this memory selection. Keep the user's choice.".to_string(),
            ));
        }
        if let (Some(sel), Some(req_inc)) = (active_sel, input.inclusion)
            && sel.user_controlled
            && req_inc != sel.inclusion
        {
            return Err(StorageError::Forbidden(
                "The user controls this memory selection. Keep the user's choice.".to_string(),
            ));
        }
        if current.is_none()
            && let (Some((_, dup)), Some(sel)) = (&duplicate_entry, active_sel)
            && sel.user_controlled
        {
            return Ok(StagedMemoryOutcome {
                status: StagedMemoryStatus::Unchanged,
                memory_id: Some(Some(dup.id.clone())),
                inclusion: Some(sel.inclusion),
                site: None,
                note: None,
            });
        }

        let intended_inclusion = input.inclusion.unwrap_or_else(|| {
            if let Some(sel) = active_sel
                && sel.user_controlled
            {
                sel.inclusion
            } else if selected_key.is_some_and(|k| planned_essentials.contains_key(k)) {
                MemoryInclusion::Essential
            } else {
                MemoryInclusion::Searchable
            }
        });
        let mut inclusion = intended_inclusion;
        if inclusion == MemoryInclusion::Essential {
            let mut others: Vec<ContextMemory> = planned_essentials
                .iter()
                .filter(|(k, m)| {
                    Some(k.as_str()) != current.as_ref().map(|c| c.entry.id.as_str())
                        && Some(k.as_str()) != duplicate_entry.as_ref().map(|(dk, _)| dk.as_str())
                        && m.text != text
                })
                .map(|(_, m)| m.clone())
                .collect();
            others.push(ContextMemory {
                id: selected_id
                    .unwrap_or("00000000-0000-0000-0000-000000000000")
                    .to_string(),
                text: text.to_string(),
                origin: duplicate_entry
                    .as_ref()
                    .map_or(AgentMemoryOrigin::Automatic, |(_, d)| d.origin),
                site: normalized_site
                    .clone()
                    .or_else(|| current.as_ref().and_then(|c| c.entry.site.clone())),
            });
            if essential_memory_bytes(&others) > self.store.budget_bytes() {
                if current.is_some()
                    && source_sel
                        .as_ref()
                        .is_some_and(|s| s.inclusion == MemoryInclusion::Essential)
                {
                    return Err(StorageError::LimitExceeded(
                        "The essential memories exceed the prompt limit. Set an entry to search when needed, then try again.".to_string(),
                    ));
                }
                inclusion = MemoryInclusion::Searchable;
            }
        }

        if input.memory_id.is_none() {
            let projected = self.projected_memories(agent_id, turn_id, call_id)?;
            let normalized = normalize_memory_text(text);
            let lim = self.store.limit();
            let saved = projected.len();
            if saved >= lim && !projected.values().any(|(t, _)| t.as_deref() == Some(normalized.as_str())) {
                let msg = if saved > lim {
                    format!(
                        "You have {saved} memories, and the limit is {lim}. The user set the limit below the number of saved memories. Do not forget memories to make room. Tell the user that this memory was not saved."
                    )
                } else {
                    format!(
                        "You have {saved} of {lim} memories. To make room, update one memory by memoryId with the combined text of two related memories, then forget the other one, or forget a memory that is no longer true. Then try again."
                    )
                };
                return Err(StorageError::LimitExceeded(msg));
            }
        }

        let epoch = self.epoch(agent_id);
        self.push_staged(
            turn_id,
            PendingMemoryMutation::Remember {
                call_id: call_id.to_string(),
                agent_id: agent_id.to_string(),
                epoch,
                memory_id: input.memory_id.clone(),
                text: text.to_string(),
                source_turn_id: turn_id.to_string(),
                expected_updated_at: input
                    .memory_id
                    .as_ref()
                    .map(|_| current.as_ref().map(|c| c.entry.updated_at.clone())),
                expected_selection_revision: source_sel.as_ref().map(|s| s.revision),
                duplicate: duplicate_entry
                    .as_ref()
                    .zip(dup_sel.as_ref())
                    .map(|((_, d), s)| (d.id.clone(), s.revision)),
                inclusion,
                site: normalized_site.clone(),
            },
        );

        let note = (inclusion != intended_inclusion).then(|| {
            "The essential memories exceed the prompt limit. Set an entry to search when needed, then try again."
                .to_string()
        });
        self.finalize_stage_outcome(
            turn_id,
            agent_id,
            turn_running,
            StagedMemoryStatus::Saved,
            StagedMemoryOutcome {
                status: StagedMemoryStatus::Staged,
                memory_id: Some(input.memory_id),
                inclusion: Some(inclusion),
                site: normalized_site,
                note,
            },
        )
    }
}

impl StagedTurnMemories {
    pub fn stage_forget(
        &self,
        turn_id: &str,
        call_id: &str,
        agent_id: &str,
        memory_id: &str,
        turn_running: bool,
    ) -> StorageResult<StagedMemoryOutcome> {
        if !is_identifier(memory_id) {
            return Err(StorageError::Validation("memoryId is required.".to_string()));
        }
        let Some(current) = self.store.get(agent_id, memory_id)? else {
            return Err(StorageError::NotFound(
                "This memory no longer exists. There is nothing to delete. Call list_memories if you need your current memory IDs.".to_string(),
            ));
        };
        let selection = self.store.get_selection(agent_id, &current.entry.id)?;
        if selection.as_ref().is_some_and(|s| s.user_controlled) {
            return Err(StorageError::Forbidden(
                "The user controls this memory selection. Keep the user's choice. You cannot delete it. If the user asked to delete it, tell them in one sentence to delete it in your Memories settings: Settings, Memories, then Delete on this memory. Do not try another memory tool.".to_string(),
            ));
        }
        let epoch = self.epoch(agent_id);
        self.push_staged(
            turn_id,
            PendingMemoryMutation::Forget {
                call_id: call_id.to_string(),
                agent_id: agent_id.to_string(),
                epoch,
                memory_id: current.entry.id.clone(),
                expected_updated_at: current.entry.updated_at,
                expected_selection_revision: selection.map_or(-1, |s| s.revision),
            },
        );
        self.finalize_stage_outcome(
            turn_id,
            agent_id,
            turn_running,
            StagedMemoryStatus::Forgotten,
            StagedMemoryOutcome {
                status: StagedMemoryStatus::Staged,
                memory_id: Some(Some(current.entry.id)),
                inclusion: None,
                site: None,
                note: None,
            },
        )
    }

    pub fn finish_turn(
        &self,
        turn_id: &str,
        status: &str,
    ) -> BTreeMap<String, Vec<ConversationMemoryChange>> {
        let pending = self
            .state
            .lock()
            .pending
            .remove(turn_id)
            .unwrap_or_default();
        let mut changes: BTreeMap<String, Vec<ConversationMemoryChange>> = BTreeMap::new();
        if status != "completed" || pending.is_empty() {
            return changes;
        }
        let mut created_memories: HashSet<String> = HashSet::new();
        let mut own_revisions: HashMap<String, HashMap<i64, i64>> = HashMap::new();

        let revision_after_own =
            |revs: &HashMap<String, HashMap<i64, i64>>, agent_id: &str, mid: &str, rev: i64| -> i64 {
                let key = format!("{agent_id}:{mid}");
                let mut next = rev;
                if let Some(map) = revs.get(&key) {
                    let mut guard = 0usize;
                    while let Some(&mapped) = map.get(&next) {
                        if mapped == next || guard >= 64 {
                            break;
                        }
                        next = mapped;
                        guard += 1;
                    }
                }
                next
            };

        for mutation in pending {
            let agent_id = mutation.agent_id().to_string();
            if mutation.epoch() != self.epoch(&agent_id) {
                continue;
            }
            let Ok(before_fp) = self.stored_state_fingerprint(&agent_id) else {
                continue;
            };
            let prev_selections = self.store.list_selections(&agent_id).unwrap_or_default();
            let prev_memory = mutation
                .target_memory_id()
                .and_then(|mid| self.store.get(&agent_id, mid).ok().flatten());
            let prev_ids: Option<HashSet<String>> = match &mutation {
                PendingMemoryMutation::Remember {
                    memory_id: None, ..
                } => Some(
                    self.store
                        .list(&agent_id)
                        .unwrap_or_default()
                        .into_iter()
                        .map(|m| m.entry.id)
                        .collect(),
                ),
                _ => None,
            };

            let commit_res: StorageResult<()> = match &mutation {
                PendingMemoryMutation::Selection {
                    changes: sel_changes,
                    ..
                } => {
                    let chained: Vec<AgentMemorySelectionChange> = sel_changes
                        .iter()
                        .map(|c| AgentMemorySelectionChange {
                            memory_id: c.memory_id.clone(),
                            inclusion: c.inclusion,
                            expected_revision: revision_after_own(
                                &own_revisions,
                                &agent_id,
                                &c.memory_id,
                                c.expected_revision,
                            ),
                        })
                        .collect();
                    self.store
                        .set_inclusions(&agent_id, &chained, MemorySelectionActor::Agent)
                        .map(|_| ())
                }
                PendingMemoryMutation::Remember {
                    memory_id,
                    text,
                    source_turn_id,
                    expected_selection_revision,
                    duplicate,
                    inclusion,
                    site,
                    ..
                } => {
                    let chained_dup = duplicate.as_ref().map(|(did, drev)| {
                        (
                            did.clone(),
                            revision_after_own(&own_revisions, &agent_id, did, *drev),
                        )
                    });
                    let chained_src_rev = match (memory_id, expected_selection_revision) {
                        (Some(mid), Some(srev)) => {
                            Some(revision_after_own(&own_revisions, &agent_id, mid, *srev))
                        }
                        _ => *expected_selection_revision,
                    };
                    self.commit_remember_tx(
                        &agent_id,
                        memory_id.as_deref(),
                        text,
                        source_turn_id,
                        chained_src_rev,
                        chained_dup.as_ref(),
                        *inclusion,
                        site.as_deref(),
                        &created_memories,
                    )
                }
                PendingMemoryMutation::Forget {
                    memory_id,
                    expected_updated_at,
                    expected_selection_revision,
                    ..
                } => {
                    let chained_rev = revision_after_own(
                        &own_revisions,
                        &agent_id,
                        memory_id,
                        *expected_selection_revision,
                    );
                    let cur_sel = self.store.get_selection(&agent_id, memory_id).ok().flatten();
                    if cur_sel
                        .as_ref()
                        .is_some_and(|s| !s.user_controlled && s.revision == chained_rev)
                    {
                        self.store
                            .delete_with_expected(
                                &agent_id,
                                memory_id,
                                Some(Some(expected_updated_at.as_str())),
                            )
                            .map(|_| ())
                    } else {
                        Ok(())
                    }
                }
            };

            if commit_res.is_err() {
                let refused = self.refused_changes(&mutation, prev_memory.as_ref());
                if !refused.is_empty() {
                    changes.entry(agent_id).or_default().extend(refused);
                }
                continue;
            }

            for sel in self.store.list_selections(&agent_id).unwrap_or_default() {
                let prev = prev_selections.iter().find(|p| p.memory_id == sel.memory_id);
                let Some(prev) = prev else {
                    created_memories.insert(sel.memory_id);
                    continue;
                };
                if prev.revision != sel.revision {
                    own_revisions
                        .entry(format!("{agent_id}:{}", sel.memory_id))
                        .or_default()
                        .insert(prev.revision, sel.revision);
                }
            }

            let after_fp = self
                .stored_state_fingerprint(&agent_id)
                .unwrap_or_default();
            let recorded = if after_fp == before_fp {
                self.refused_changes(&mutation, prev_memory.as_ref())
            } else {
                self.committed_changes(&mutation, prev_memory.as_ref(), prev_ids.as_ref())
            };
            if !recorded.is_empty() {
                changes.entry(agent_id).or_default().extend(recorded);
            }
        }
        changes
    }
}

impl StagedTurnMemories {
    #[allow(clippy::too_many_arguments)]
    fn commit_remember_tx(
        &self,
        agent_id: &str,
        memory_id: Option<&str>,
        text: &str,
        source_turn_id: &str,
        expected_selection_revision: Option<i64>,
        duplicate: Option<&(String, i64)>,
        inclusion: MemoryInclusion,
        site: Option<&str>,
        created_memories: &HashSet<String>,
    ) -> StorageResult<()> {
        self.store.database().with_tx(|_| {
            let cur_dup = self
                .store
                .list(agent_id)?
                .into_iter()
                .find(|m| Some(m.entry.id.as_str()) != memory_id && m.entry.text == text);
            let dup_sel = match &cur_dup {
                Some(d) => self.store.get_selection(agent_id, &d.entry.id)?,
                None => None,
            };
            if let Some((exp_dup_id, exp_dup_rev)) = duplicate {
                if cur_dup.as_ref().map(|d| &d.entry.id) != Some(exp_dup_id)
                    || dup_sel.as_ref().map(|s| s.revision) != Some(*exp_dup_rev)
                {
                    return Ok(());
                }
            } else if let Some(d) = &cur_dup
                && !created_memories.contains(&d.entry.id)
            {
                return Ok(());
            }
            if let Some(ds) = &dup_sel
                && ds.user_controlled
                && ds.inclusion != inclusion
            {
                return Ok(());
            }
            if let Some(mid) = memory_id {
                let current = self.store.get(agent_id, mid)?;
                let sel = self.store.get_selection(agent_id, mid)?;
                let (Some(current), Some(sel)) = (current, sel) else {
                    return Ok(());
                };
                if Some(sel.revision) != expected_selection_revision
                    || (cur_dup.is_some() && sel.user_controlled)
                {
                    return Ok(());
                }
                if inclusion == MemoryInclusion::Searchable
                    && sel.inclusion == MemoryInclusion::Essential
                {
                    self.store.set_inclusions(
                        agent_id,
                        &[AgentMemorySelectionChange {
                            memory_id: current.entry.id,
                            inclusion: MemoryInclusionChangeValue::Searchable,
                            expected_revision: sel.revision,
                        }],
                        MemorySelectionActor::Agent,
                    )?;
                }
            }
            let saved = self.store.save_automatic(SaveAutomaticMemoryInput {
                agent_id: agent_id.to_string(),
                memory_id: memory_id.map(str::to_string),
                text: text.to_string(),
                source_turn_id: source_turn_id.to_string(),
                expected_updated_at: None,
                expected_selection_revision: None,
                site: site.map(str::to_string),
            })?;
            let Some(saved) = saved else {
                return Ok(());
            };
            let Some(sel) = self.store.get_selection(agent_id, &saved.entry.id)? else {
                return Ok(());
            };
            if sel.inclusion == inclusion || sel.user_controlled {
                return Ok(());
            }
            if inclusion == MemoryInclusion::Essential {
                let mut next: Vec<ContextMemory> = self
                    .store
                    .essential_for(agent_id)?
                    .iter()
                    .map(ContextMemory::from)
                    .collect();
                next.push(ContextMemory::from(&saved));
                if essential_memory_bytes(&next) > self.store.budget_bytes() {
                    return Ok(());
                }
            }
            let next_change = match inclusion {
                MemoryInclusion::Essential => MemoryInclusionChangeValue::Essential,
                MemoryInclusion::Searchable => MemoryInclusionChangeValue::Searchable,
            };
            self.store.set_inclusions(
                agent_id,
                &[AgentMemorySelectionChange {
                    memory_id: saved.entry.id,
                    inclusion: next_change,
                    expected_revision: sel.revision,
                }],
                MemorySelectionActor::Agent,
            )?;
            Ok(())
        })
    }

    fn refused_changes(
        &self,
        mutation: &PendingMemoryMutation,
        previous: Option<&AgentMemory>,
    ) -> Vec<ConversationMemoryChange> {
        match mutation {
            PendingMemoryMutation::Selection { .. } => Vec::new(),
            PendingMemoryMutation::Forget {
                agent_id,
                memory_id,
                ..
            } => {
                let Some(prev) = previous else {
                    return Vec::new();
                };
                if self.store.get(agent_id, memory_id).ok().flatten().is_none() {
                    return Vec::new();
                }
                vec![ConversationMemoryChange {
                    memory_id: prev.entry.id.clone(),
                    action: ConversationMemoryChangeAction::NotChanged,
                    text: prev.entry.text.clone(),
                    previous_text: None,
                }]
            }
            PendingMemoryMutation::Remember {
                call_id,
                agent_id,
                memory_id,
                text,
                duplicate,
                ..
            } => {
                if self
                    .store
                    .list(agent_id)
                    .unwrap_or_default()
                    .iter()
                    .any(|m| m.entry.text == *text)
                {
                    return Vec::new();
                }
                let raw_id = memory_id
                    .clone()
                    .or_else(|| duplicate.as_ref().map(|(id, _)| id.clone()))
                    .unwrap_or_else(|| format!("unsaved-{call_id}"));
                let truncated: String = raw_id
                    .chars()
                    .take(INPUT_LIMITS.identifier)
                    .collect();
                vec![ConversationMemoryChange {
                    memory_id: truncated,
                    action: ConversationMemoryChangeAction::NotChanged,
                    text: text.clone(),
                    previous_text: None,
                }]
            }
        }
    }

    fn committed_changes(
        &self,
        mutation: &PendingMemoryMutation,
        previous: Option<&AgentMemory>,
        previous_ids: Option<&HashSet<String>>,
    ) -> Vec<ConversationMemoryChange> {
        match mutation {
            PendingMemoryMutation::Selection { .. } => Vec::new(),
            PendingMemoryMutation::Forget {
                agent_id,
                memory_id,
                ..
            } => {
                let Some(prev) = previous else {
                    return Vec::new();
                };
                if self.store.get(agent_id, memory_id).ok().flatten().is_some() {
                    return Vec::new();
                }
                vec![ConversationMemoryChange {
                    memory_id: prev.entry.id.clone(),
                    action: ConversationMemoryChangeAction::Forgotten,
                    text: prev.entry.text.clone(),
                    previous_text: None,
                }]
            }
            PendingMemoryMutation::Remember { agent_id, .. } => {
                if let Some(prev) = previous {
                    let Some(cur) = self.store.get(agent_id, &prev.entry.id).ok().flatten() else {
                        return Vec::new();
                    };
                    if cur.entry.text == prev.entry.text {
                        return Vec::new();
                    }
                    return vec![ConversationMemoryChange {
                        memory_id: cur.entry.id,
                        action: ConversationMemoryChangeAction::Updated,
                        text: cur.entry.text,
                        previous_text: Some(prev.entry.text.clone()),
                    }];
                }
                self.store
                    .list(agent_id)
                    .unwrap_or_default()
                    .into_iter()
                    .filter(|m| previous_ids.is_none_or(|ids| !ids.contains(&m.entry.id)))
                    .map(|m| ConversationMemoryChange {
                        memory_id: m.entry.id,
                        action: ConversationMemoryChangeAction::Saved,
                        text: m.entry.text,
                        previous_text: None,
                    })
                    .collect()
            }
        }
    }
}

impl StagedTurnMemories {
    fn planned_context(
        &self,
        agent_id: &str,
        turn_id: &str,
        except_call_id: &str,
    ) -> StorageResult<(BTreeMap<String, ContextMemory>, BTreeMap<String, ContextMemory>)> {
        let mut all: BTreeMap<String, ContextMemory> = self
            .store
            .list(agent_id)?
            .iter()
            .map(|m| (m.entry.id.clone(), ContextMemory::from(m)))
            .collect();
        let mut selected: BTreeMap<String, ContextMemory> = self
            .store
            .essential_for(agent_id)?
            .iter()
            .map(|m| (m.entry.id.clone(), ContextMemory::from(m)))
            .collect();
        let cur_epoch = self.epoch(agent_id);
        let turn_pending = self
            .state
            .lock()
            .pending
            .get(turn_id)
            .cloned()
            .unwrap_or_default();
        for item in turn_pending {
            if item.agent_id() != agent_id
                || item.call_id() == except_call_id
                || item.epoch() != cur_epoch
            {
                continue;
            }
            match item {
                PendingMemoryMutation::Selection { changes, .. } => {
                    for ch in changes {
                        if ch.inclusion == MemoryInclusionChangeValue::Essential
                            && let Some(mem) = all.get(&ch.memory_id).cloned()
                        {
                            selected.insert(mem.id.clone(), mem);
                        } else {
                            selected.remove(&ch.memory_id);
                        }
                    }
                }
                PendingMemoryMutation::Forget { memory_id, .. } => {
                    selected.remove(&memory_id);
                    all.remove(&memory_id);
                }
                PendingMemoryMutation::Remember {
                    call_id,
                    memory_id,
                    text,
                    inclusion,
                    site,
                    ..
                } => {
                    let dup = all
                        .iter()
                        .find(|(_, m)| {
                            m.text == text && Some(m.id.as_str()) != memory_id.as_deref()
                        })
                        .map(|(k, m)| (k.clone(), m.clone()));
                    let key = dup
                        .as_ref()
                        .map(|(k, _)| k.clone())
                        .or_else(|| memory_id.clone())
                        .unwrap_or_else(|| format!("pending:{call_id}"));
                    if dup.is_some()
                        && let Some(mid) = &memory_id
                    {
                        selected.remove(mid);
                        all.remove(mid);
                    }
                    let mem = dup.map(|(_, m)| m).unwrap_or_else(|| ContextMemory {
                        id: memory_id
                            .unwrap_or_else(|| "00000000-0000-0000-0000-000000000000".to_string()),
                        text,
                        origin: AgentMemoryOrigin::Automatic,
                        site,
                    });
                    all.insert(key.clone(), mem.clone());
                    if inclusion == MemoryInclusion::Essential {
                        selected.insert(key, mem);
                    } else {
                        selected.remove(&key);
                    }
                }
            }
        }
        Ok((all, selected))
    }

    fn projected_memories(
        &self,
        agent_id: &str,
        turn_id: &str,
        except_call_id: &str,
    ) -> StorageResult<BTreeMap<String, (Option<String>, Option<String>)>> {
        let cur_epoch = self.epoch(agent_id);
        let (same_turn, others): (Vec<PendingMemoryMutation>, Vec<PendingMemoryMutation>) = {
            let guard = self.state.lock();
            let relevant = |m: &&PendingMemoryMutation| {
                m.agent_id() == agent_id && m.epoch() == cur_epoch && m.call_id() != except_call_id
            };
            let same = guard
                .pending
                .get(turn_id)
                .map(|v| v.iter().filter(relevant).cloned().collect())
                .unwrap_or_default();
            let oth = guard
                .pending
                .iter()
                .filter(|(tid, _)| tid.as_str() != turn_id)
                .flat_map(|(_, v)| v.iter().filter(relevant).cloned())
                .collect();
            (same, oth)
        };
        let changed_elsewhere: HashSet<String> = others
            .iter()
            .filter_map(|m| m.target_memory_id().map(str::to_string))
            .collect();
        let mut memories: BTreeMap<String, (Option<String>, Option<String>)> = self
            .store
            .list(agent_id)?
            .into_iter()
            .map(|m| {
                if changed_elsewhere.contains(&m.entry.id) {
                    (m.entry.id, (None, None))
                } else {
                    (
                        m.entry.id,
                        (
                            Some(normalize_memory_text(&m.entry.text)),
                            Some(m.entry.updated_at),
                        ),
                    )
                }
            })
            .collect();

        for item in same_turn {
            match item {
                PendingMemoryMutation::Selection { .. } => {}
                PendingMemoryMutation::Forget {
                    memory_id,
                    expected_updated_at,
                    ..
                } => {
                    if memories
                        .get(&memory_id)
                        .and_then(|(_, upd)| upd.as_deref())
                        == Some(expected_updated_at.as_str())
                    {
                        memories.remove(&memory_id);
                    }
                }
                PendingMemoryMutation::Remember {
                    call_id,
                    memory_id,
                    text,
                    expected_updated_at,
                    ..
                } => {
                    let norm = normalize_memory_text(&text);
                    let same_key = memories
                        .iter()
                        .find(|(_, (t, _))| t.as_deref() == Some(norm.as_str()))
                        .map(|(k, _)| k.clone());
                    let Some(mid) = memory_id else {
                        if same_key.is_none() {
                            memories.insert(format!("staged:{call_id}"), (Some(norm), None));
                        }
                        continue;
                    };
                    let Some((_, cur_upd)) = memories.get(&mid).cloned() else {
                        continue;
                    };
                    if changed_elsewhere.contains(&mid) {
                        continue;
                    }
                    if let (Some(u), Some(exp)) = (&cur_upd, &expected_updated_at)
                        && Some(u.as_str()) != exp.as_deref()
                    {
                        continue;
                    }
                    if let Some(sk) = same_key
                        && sk != mid
                    {
                        memories.remove(&mid);
                    } else {
                        memories.insert(mid, (Some(norm), None));
                    }
                }
            }
        }
        for item in others {
            if let PendingMemoryMutation::Remember {
                call_id,
                memory_id: None,
                ..
            } = item
            {
                memories.insert(format!("reserved:{call_id}"), (None, None));
            }
        }
        Ok(memories)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cjk_script_boundary_and_site_hierarchy_match_upstream() {
        assert!(matches_unindexed_term("プロジェクトのAPI設定を変更", "設定"));
        assert!(matches_unindexed_term("プロジェクトのAPI設定を変更", "api"));
        assert!(!matches_unindexed_term("rapidly changing", "api"));
        assert_eq!(
            memory_sites_for_host("https://www.sub.app.example.com:8443/login"),
            vec![
                "sub.app.example.com".to_string(),
                "app.example.com".to_string(),
                "example.com".to_string(),
            ]
        );
        assert_eq!(
            memory_sites_for_host("http://127.0.0.1:3000/test"),
            vec!["127.0.0.1".to_string()]
        );
    }
}
