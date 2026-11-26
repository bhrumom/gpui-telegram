//! Pure-Rust Bot Groups, sub-threads, remote bot members & asks, linear-time
//! code/quote/path-aware `@mention` parser, group dispatch router, and
//! 7-code `evaluate_bot_request_policy` (Deliverable #6).

use std::collections::{BTreeMap, HashMap, HashSet, VecDeque};
use std::fmt;

use serde::{Deserialize, Deserializer, Serialize};

use crate::bot_types::{BotFolderSlug, BotName, bot_folder_name};
use crate::{BotError, BotErrorCode};

// ---------------------------------------------------------------------------
// Constants & Group Folder Slug
// ---------------------------------------------------------------------------

pub const BOT_GROUP_MAX_LOCAL_MEMBERS: usize = 32;
pub const BOT_GROUP_MAX_REMOTE_MEMBERS: usize = 31;
pub const BOT_GROUP_THREAD_TITLE_MAX_LEN: usize = 80;
pub const BOT_GROUP_NAME_MAX_LEN: usize = 64;
pub const BOT_GROUP_FOLDER_PREFIX: &str = "group-";
pub const BOT_GROUP_SEND_TEXT_MAX_LEN: usize = 100_000;
pub const BOT_GROUP_SEND_MAX_IMAGES: usize = 20;

/// Validated Bot Group folder slug matching `^group-[a-z0-9]+(?:-[a-z0-9]+)*$`.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize)]
#[serde(transparent)]
pub struct BotGroupFolderSlug(String);

impl BotGroupFolderSlug {
    pub fn new(raw: &str) -> Result<Self, BotErrorCode> {
        let Some(rest) = raw.strip_prefix(BOT_GROUP_FOLDER_PREFIX) else {
            return Err(BotErrorCode::InvalidArgument);
        };
        BotFolderSlug::new(rest)?;
        Ok(Self(raw.to_string()))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    pub fn into_inner(self) -> String {
        self.0
    }
}

impl AsRef<str> for BotGroupFolderSlug {
    fn as_ref(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for BotGroupFolderSlug {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl<'de> Deserialize<'de> for BotGroupFolderSlug {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let raw = String::deserialize(deserializer)?;
        Self::new(&raw).map_err(serde::de::Error::custom)
    }
}

pub fn bot_group_folder_name(group_name: &str) -> BotGroupFolderSlug {
    let slug = bot_folder_name(group_name);
    BotGroupFolderSlug(format!("{BOT_GROUP_FOLDER_PREFIX}{}", slug.as_str()))
}

// ---------------------------------------------------------------------------
// Group Domain Types: ReplyMode, Thread, PeerScope, SyntheticRoot, Send
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum BotGroupReplyMode {
    Bot {
        #[serde(rename = "botId", alias = "bot_id")]
        bot_id: String,
    },
    All,
    Mentions,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum BotGroupSyntheticRoot {
    #[serde(rename = "fromItems", alias = "from_items")]
    FromItems,
    #[serde(rename = "fromReports", alias = "from_reports")]
    FromReports,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BotGroupPeerScope {
    #[serde(alias = "root_request_id")]
    pub root_request_id: String,
    #[serde(alias = "root_sequence")]
    pub root_sequence: u64,
    #[serde(alias = "source_session_id")]
    pub source_session_id: String,
    #[serde(alias = "target_session_id")]
    pub target_session_id: String,
}

impl BotGroupPeerScope {
    pub fn validate(&self) -> Result<(), BotError> {
        if self.root_request_id.trim().is_empty()
            || self.source_session_id.trim().is_empty()
            || self.target_session_id.trim().is_empty()
        {
            return Err(BotError::new(
                BotErrorCode::InvalidArgument,
                "Peer scope requires non-empty root_request_id, source_session_id, and target_session_id.",
            ));
        }
        if self.source_session_id.trim() == self.target_session_id.trim() {
            return Err(BotError::new(
                BotErrorCode::InvalidArgument,
                "Peer scope source_session_id and target_session_id must differ.",
            ));
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BotGroupImage {
    pub id: String,
    pub path: String,
    pub mime: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BotGroupSend {
    #[serde(default)]
    pub text: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub images: Vec<BotGroupImage>,
}

impl BotGroupSend {
    pub fn validate(&self) -> Result<(), BotError> {
        if self.text.trim().is_empty() && self.images.is_empty() {
            return Err(BotError::new(
                BotErrorCode::InvalidArgument,
                "Group message must include non-empty text or at least one image.",
            ));
        }
        if self.text.chars().count() > BOT_GROUP_SEND_TEXT_MAX_LEN {
            return Err(BotError::new(
                BotErrorCode::InvalidArgument,
                format!(
                    "Group message text exceeds {BOT_GROUP_SEND_TEXT_MAX_LEN} characters."
                ),
            ));
        }
        if self.images.len() > BOT_GROUP_SEND_MAX_IMAGES {
            return Err(BotError::new(
                BotErrorCode::InvalidArgument,
                format!(
                    "Group message cannot attach more than {BOT_GROUP_SEND_MAX_IMAGES} images."
                ),
            ));
        }
        Ok(())
    }
}

/// Derives a safe `<= 80`-char sub-thread title from a first post (UTF-8 char-boundary safe).
pub fn derive_group_thread_title(first_post: &str) -> String {
    let mut collapsed = String::new();
    let mut in_ws = false;
    let mut count = 0usize;
    for ch in first_post.chars() {
        if ch.is_control() || ch.is_whitespace() {
            if !collapsed.is_empty() && !in_ws {
                in_ws = true;
            }
        } else {
            if in_ws {
                if count + 1 >= BOT_GROUP_THREAD_TITLE_MAX_LEN {
                    break;
                }
                collapsed.push(' ');
                count += 1;
                in_ws = false;
            }
            if count >= BOT_GROUP_THREAD_TITLE_MAX_LEN {
                break;
            }
            collapsed.push(ch);
            count += 1;
        }
    }
    let trimmed = collapsed.trim();
    if trimmed.is_empty() {
        "New thread".to_string()
    } else {
        trimmed.to_string()
    }
}

pub fn validate_group_thread_title(raw: &str) -> Result<String, BotError> {
    if raw.chars().any(char::is_control) {
        return Err(BotError::new(
            BotErrorCode::InvalidArgument,
            "Thread title cannot contain control characters.",
        ));
    }
    let trimmed = raw.trim();
    let len = trimmed.chars().count();
    if len == 0 || len > BOT_GROUP_THREAD_TITLE_MAX_LEN {
        return Err(BotError::new(
            BotErrorCode::InvalidArgument,
            format!(
                "Thread title must be 1..={BOT_GROUP_THREAD_TITLE_MAX_LEN} characters."
            ),
        ));
    }
    Ok(trimmed.to_string())
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BotGroupThread {
    pub id: String,
    pub title: String,
    #[serde(default, alias = "session_ids")]
    pub session_ids: BTreeMap<String, String>,
    #[serde(default, alias = "former_session_ids")]
    pub former_session_ids: BTreeMap<String, Vec<String>>,
    #[serde(default, alias = "created_at_ms")]
    pub created_at_ms: i64,
    #[serde(default, alias = "updated_at_ms")]
    pub updated_at_ms: i64,
}

impl BotGroupThread {
    pub fn validate(&self) -> Result<(), BotError> {
        if self.id.trim().is_empty() {
            return Err(BotError::new(
                BotErrorCode::InvalidArgument,
                "Thread id must be non-empty.",
            ));
        }
        validate_group_thread_title(&self.title)?;
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// RemoteBotMember & RemoteAsk
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RemoteBotMember {
    #[serde(alias = "host_id")]
    pub host_id: String,
    #[serde(alias = "bot_id")]
    pub bot_id: String,
    pub name: String,
    #[serde(alias = "chat_session_id")]
    pub chat_session_id: String,
}

impl RemoteBotMember {
    /// Disambiguated member key that can never collide with a local bot id.
    pub fn member_key(&self) -> String {
        format!("remote:{}:{}", self.host_id, self.bot_id)
    }

    pub fn validate(&self) -> Result<(), BotError> {
        if self.host_id.is_empty()
            || self.bot_id.is_empty()
            || self.chat_session_id.trim().is_empty()
            || self.host_id.trim() != self.host_id
            || self.bot_id.trim() != self.bot_id
            || self.host_id.chars().any(char::is_control)
            || self.bot_id.chars().any(char::is_control)
            || self.host_id.contains(':')
            || self.bot_id.contains(':')
        {
            return Err(BotError::new(
                BotErrorCode::InvalidArgument,
                "Remote bot member requires non-empty trimmed host_id and bot_id without ':' or control characters, and non-empty pinned chat_session_id.",
            ));
        }
        BotName::new(&self.name).map_err(|code| {
            BotError::new(code, "Remote bot member name is invalid.")
        })?;
        Ok(())
    }

    /// Renames the remote member while keeping its pinned `chat_session_id` intact.
    pub fn rename(&mut self, new_name: &str) -> Result<(), BotError> {
        let valid = BotName::new(new_name)
            .map_err(|code| BotError::new(code, "Remote bot member name is invalid."))?;
        self.name = valid.into_inner();
        Ok(())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RemoteAskState {
    Sending,
    Sent,
    Failed,
    Unknown,
}

impl RemoteAskState {
    pub const ALL: [Self; 4] = [Self::Sending, Self::Sent, Self::Failed, Self::Unknown];

    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Sending => "sending",
            Self::Sent => "sent",
            Self::Failed => "failed",
            Self::Unknown => "unknown",
        }
    }

    pub fn parse(raw: &str) -> Option<Self> {
        match raw {
            "sending" => Some(Self::Sending),
            "sent" => Some(Self::Sent),
            "failed" => Some(Self::Failed),
            "unknown" => Some(Self::Unknown),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum RemoteAskReply {
    Answer {
        text: String,
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        truncated: bool,
    },
    Notice {
        text: String,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RemoteAsk {
    pub id: String,
    #[serde(alias = "host_id")]
    pub host_id: String,
    #[serde(alias = "bot_id")]
    pub bot_id: String,
    #[serde(alias = "chat_session_id")]
    pub chat_session_id: String,
    pub state: RemoteAskState,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reply: Option<RemoteAskReply>,
    #[serde(alias = "created_at_ms")]
    pub created_at_ms: i64,
    #[serde(default, alias = "updated_at_ms", skip_serializing_if = "Option::is_none")]
    pub updated_at_ms: Option<i64>,
}

impl RemoteAsk {
    /// Applies a reply to a remote ask: `Answer` may replace `Notice`,
    /// but `Notice` can NEVER overwrite an existing `Answer`.
    pub fn apply_reply(&mut self, reply: RemoteAskReply, now_ms: i64) -> Result<(), BotError> {
        if matches!(self.reply, Some(RemoteAskReply::Answer { .. }))
            && matches!(reply, RemoteAskReply::Notice { .. })
        {
            return Err(BotError::new(
                BotErrorCode::InvalidArgument,
                "A remote ask notice cannot overwrite an existing answer.",
            ));
        }
        self.reply = Some(reply);
        self.updated_at_ms = Some(now_ms);
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GroupSessionLookup {
    Active {
        thread_id: Option<String>,
        bot_id: String,
    },
    Former {
        thread_id: Option<String>,
        bot_id: String,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BotGroup {
    pub id: String,
    pub name: String,
    pub folder: BotGroupFolderSlug,
    #[serde(default, alias = "member_bot_ids")]
    pub member_bot_ids: Vec<String>,
    #[serde(default, alias = "session_ids")]
    pub session_ids: BTreeMap<String, String>,
    #[serde(default, alias = "former_session_ids")]
    pub former_session_ids: BTreeMap<String, Vec<String>>,
    #[serde(alias = "reply_mode")]
    pub reply_mode: BotGroupReplyMode,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub threads: Vec<BotGroupThread>,
    #[serde(default, alias = "remote_members", skip_serializing_if = "Vec::is_empty")]
    pub remote_members: Vec<RemoteBotMember>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub images: Vec<BotGroupImage>,
    #[serde(default, alias = "created_at_ms")]
    pub created_at_ms: i64,
    #[serde(default, alias = "updated_at_ms")]
    pub updated_at_ms: i64,
}

impl BotGroup {
    pub fn validate(&self) -> Result<(), BotError> {
        if self.id.trim().is_empty() {
            return Err(BotError::new(BotErrorCode::InvalidArgument, "Group id must be non-empty."));
        }
        BotName::new(&self.name)
            .map_err(|code| BotError::new(code, "Group name is invalid."))?;
        BotGroupFolderSlug::new(self.folder.as_str())
            .map_err(|code| BotError::new(code, "Group folder slug is invalid."))?;

        if self.member_bot_ids.is_empty() || self.member_bot_ids.len() > BOT_GROUP_MAX_LOCAL_MEMBERS
        {
            return Err(BotError::new(
                BotErrorCode::InvalidArgument,
                format!(
                    "Group must have between 1 and {BOT_GROUP_MAX_LOCAL_MEMBERS} local members."
                ),
            ));
        }
        let mut local_seen: HashSet<&str> = HashSet::with_capacity(self.member_bot_ids.len());
        for bid in &self.member_bot_ids {
            let trimmed = bid.trim();
            if trimmed.is_empty() || !local_seen.insert(trimmed) {
                return Err(BotError::new(
                    BotErrorCode::InvalidArgument,
                    "Group member_bot_ids must be non-empty and unique.",
                ));
            }
        }

        if let BotGroupReplyMode::Bot { ref bot_id } = self.reply_mode
            && !self.member_bot_ids.iter().any(|m| m == bot_id)
        {
            return Err(BotError::new(
                BotErrorCode::InvalidArgument,
                format!("Reply mode lead bot '{bot_id}' is not a member of this group."),
            ));
        }

        if self.remote_members.len() > BOT_GROUP_MAX_REMOTE_MEMBERS {
            return Err(BotError::new(
                BotErrorCode::InvalidArgument,
                format!(
                    "Group cannot have more than {BOT_GROUP_MAX_REMOTE_MEMBERS} remote members."
                ),
            ));
        }
        let mut remote_seen: HashSet<(&str, &str)> =
            HashSet::with_capacity(self.remote_members.len());
        for rm in &self.remote_members {
            rm.validate()?;
            if !remote_seen.insert((rm.host_id.trim(), rm.bot_id.trim())) {
                return Err(BotError::new(
                    BotErrorCode::InvalidArgument,
                    format!(
                        "Duplicate remote member '{}:{}' in group.",
                        rm.host_id, rm.bot_id
                    ),
                ));
            }
        }

        for thread in &self.threads {
            thread.validate()?;
        }
        Ok(())
    }

    /// Removes a local member, archiving its active root and thread sessions into
    /// `former_session_ids` (read-only history) and adjusting lead reply mode if needed.
    pub fn remove_member(&mut self, bot_id: &str) {
        self.member_bot_ids.retain(|id| id != bot_id);
        if let Some(sid) = self.session_ids.remove(bot_id) {
            let entry = self
                .former_session_ids
                .entry(bot_id.to_string())
                .or_default();
            if !entry.contains(&sid) {
                entry.push(sid);
            }
        }
        for thread in &mut self.threads {
            if let Some(sid) = thread.session_ids.remove(bot_id) {
                let entry = thread
                    .former_session_ids
                    .entry(bot_id.to_string())
                    .or_default();
                if !entry.contains(&sid) {
                    entry.push(sid);
                }
            }
        }
        if let BotGroupReplyMode::Bot { bot_id: ref lead } = self.reply_mode
            && lead == bot_id
        {
            self.reply_mode = self
                .member_bot_ids
                .first()
                .map(|first| BotGroupReplyMode::Bot {
                    bot_id: first.clone(),
                })
                .unwrap_or(BotGroupReplyMode::Mentions);
        }
    }

    /// Resolves which room/thread owns `session_id`, distinguishing active members from
    /// former (history-only) members.
    pub fn find_room_of_session(&self, session_id: &str) -> Option<GroupSessionLookup> {
        for (bot_id, sid) in &self.session_ids {
            if sid == session_id && self.member_bot_ids.iter().any(|m| m == bot_id) {
                return Some(GroupSessionLookup::Active {
                    thread_id: None,
                    bot_id: bot_id.clone(),
                });
            }
        }
        for thread in &self.threads {
            for (bot_id, sid) in &thread.session_ids {
                if sid == session_id && self.member_bot_ids.iter().any(|m| m == bot_id) {
                    return Some(GroupSessionLookup::Active {
                        thread_id: Some(thread.id.clone()),
                        bot_id: bot_id.clone(),
                    });
                }
            }
        }
        for (bot_id, sids) in &self.former_session_ids {
            if sids.iter().any(|s| s == session_id) {
                return Some(GroupSessionLookup::Former {
                    thread_id: None,
                    bot_id: bot_id.clone(),
                });
            }
        }
        for thread in &self.threads {
            for (bot_id, sids) in &thread.former_session_ids {
                if sids.iter().any(|s| s == session_id) {
                    return Some(GroupSessionLookup::Former {
                        thread_id: Some(thread.id.clone()),
                        bot_id: bot_id.clone(),
                    });
                }
            }
        }
        None
    }

    /// Returns `true` ONLY when `session_id` belongs to an active member session
    /// (former sessions return `false`).
    pub fn has_active_session_access(&self, session_id: &str) -> bool {
        matches!(
            self.find_room_of_session(session_id),
            Some(GroupSessionLookup::Active { .. })
        )
    }
}

// ---------------------------------------------------------------------------
// Linear-Time Code/Quote/Path/History-Aware @mention Parser & Group Router
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MentionCandidate {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub is_remote: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MentionOccurrenceKind {
    Member,
    Everyone,
    Ambiguous,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MentionOccurrence {
    pub kind: MentionOccurrenceKind,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub candidate: Option<MentionCandidate>,
    pub start: usize,
    pub end: usize,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ParsedBotMentions {
    pub members: Vec<MentionCandidate>,
    pub has_everyone: bool,
    pub ambiguous_everyone: bool,
    pub occurrences: Vec<MentionOccurrence>,
}

fn blank_range_preserve_newlines(buf: &mut [u8], start: usize, end: usize) {
    let clamped_end = end.min(buf.len());
    if start >= clamped_end {
        return;
    }
    for byte in &mut buf[start..clamped_end] {
        if *byte != b'\n' && *byte != b'\r' {
            *byte = b' ';
        }
    }
}

fn find_ascii_bytes(haystack: &[u8], needle: &[u8], from: usize) -> Option<usize> {
    if needle.is_empty() || from + needle.len() > haystack.len() {
        return None;
    }
    haystack[from..]
        .windows(needle.len())
        .position(|w| w == needle)
        .map(|rel| from + rel)
}

fn parse_fence_open(line: &[u8]) -> Option<(u8, usize)> {
    let mut i = 0;
    while i < line.len() && i < 3 && line[i] == b' ' {
        i += 1;
    }
    let ch = *line.get(i)?;
    if ch != b'`' && ch != b'~' {
        return None;
    }
    let start = i;
    while i < line.len() && line[i] == ch {
        i += 1;
    }
    let count = i - start;
    if count < 3 {
        return None;
    }
    if ch == b'`' && line[i..].contains(&b'`') {
        return None;
    }
    Some((ch, count))
}

fn is_fence_close(line: &[u8], fence_ch: u8, fence_len: usize) -> bool {
    let mut i = 0;
    while i < line.len() && i < 3 && line[i] == b' ' {
        i += 1;
    }
    let start = i;
    while i < line.len() && line[i] == fence_ch {
        i += 1;
    }
    let count = i - start;
    if count < fence_len {
        return false;
    }
    line[i..].iter().all(|b| matches!(b, b' ' | b'\t' | b'\r'))
}

fn is_blockquote_line(line: &[u8]) -> bool {
    let mut i = 0;
    while i < line.len() && i < 3 && line[i] == b' ' {
        i += 1;
    }
    line.get(i) == Some(&b'>')
}

/// Returns a valid UTF-8 string of identical byte length to `text` where non-mention
/// regions (`<quoted_room_history>`, fenced code blocks, blockquotes, inline code spans,
/// `"..."`/`“...”` quotes, and `@/...`/`@~/...`/`@C:\...` file paths) are replaced with spaces.
pub fn blank_non_mention_regions(text: &str) -> String {
    let mut buf = text.as_bytes().to_vec();
    let len = buf.len();

    // 1. <quoted_room_history>...</quoted_room_history> (ASCII case-insensitive)
    let lower_ascii: Vec<u8> = buf.iter().map(u8::to_ascii_lowercase).collect();
    const OPEN_TAG: &[u8] = b"<quoted_room_history>";
    const CLOSE_TAG: &[u8] = b"</quoted_room_history>";
    let mut pos = 0;
    while let Some(open_idx) = find_ascii_bytes(&lower_ascii, OPEN_TAG, pos) {
        let search_from = open_idx + OPEN_TAG.len();
        if let Some(close_idx) = find_ascii_bytes(&lower_ascii, CLOSE_TAG, search_from) {
            let end = close_idx + CLOSE_TAG.len();
            blank_range_preserve_newlines(&mut buf, open_idx, end);
            pos = end;
        } else {
            blank_range_preserve_newlines(&mut buf, open_idx, len);
            break;
        }
    }

    // 2. Line-level fenced code blocks (CommonMark §4.5; unclosed fence blanks to EOF) & blockquotes
    let mut active_fence: Option<(u8, usize)> = None;
    let mut line_start = 0;
    while line_start < len {
        let line_end = buf[line_start..]
            .iter()
            .position(|&b| b == b'\n')
            .map_or(len, |rel| line_start + rel);
        let line = &buf[line_start..line_end];
        if let Some((fch, flen)) = active_fence {
            let closes = is_fence_close(line, fch, flen);
            blank_range_preserve_newlines(&mut buf, line_start, line_end);
            if closes {
                active_fence = None;
            }
        } else if let Some(fence) = parse_fence_open(line) {
            active_fence = Some(fence);
            blank_range_preserve_newlines(&mut buf, line_start, line_end);
        } else if is_blockquote_line(line) {
            blank_range_preserve_newlines(&mut buf, line_start, line_end);
        }
        line_start = if line_end < len { line_end + 1 } else { len };
    }

    // 3. Inline code spans (CommonMark §6.1, strict O(n) pre-indexed runs; unclosed left intact)
    let mut runs_by_len: HashMap<usize, VecDeque<usize>> = HashMap::new();
    let mut i = 0;
    while i < len {
        if buf[i] == b'`' {
            let start = i;
            while i < len && buf[i] == b'`' {
                i += 1;
            }
            runs_by_len.entry(i - start).or_default().push_back(start);
        } else {
            i += 1;
        }
    }
    let mut cursor = 0;
    while cursor < len {
        if buf[cursor] == b'`' {
            let start = cursor;
            while cursor < len && buf[cursor] == b'`' {
                cursor += 1;
            }
            let rlen = cursor - start;
            if let Some(queue) = runs_by_len.get_mut(&rlen) {
                while queue.front().is_some_and(|&idx| idx <= start) {
                    queue.pop_front();
                }
                if let Some(&close_start) = queue.front() {
                    let close_end = close_start + rlen;
                    blank_range_preserve_newlines(&mut buf, start, close_end);
                    cursor = close_end;
                }
            }
        } else {
            cursor += 1;
        }
    }

    // 4. Quoted strings ("..." with backslash escapes, and smart quotes “...”) paired
    //    independently per line so an unclosed quote of one style never disables the other.
    let mut l_start = 0;
    while l_start < len {
        let l_end = buf[l_start..]
            .iter()
            .position(|&b| b == b'\n')
            .map_or(len, |rel| l_start + rel);

        // 4a. Straight double quotes "..." with backslash escapes (\", \\)
        let mut open_dq: Option<usize> = None;
        let mut p = l_start;
        while p < l_end {
            if open_dq.is_some() && buf[p] == b'\\' && p + 1 < l_end {
                p += 2;
                continue;
            }
            if buf[p] == b'"' {
                if let Some(o) = open_dq.take() {
                    blank_range_preserve_newlines(&mut buf, o, p + 1);
                } else {
                    open_dq = Some(p);
                }
            }
            p += 1;
        }

        // 4b. Smart double quotes “...”
        let mut open_smart: Option<usize> = None;
        let mut q = l_start;
        while q < l_end {
            if q + 3 <= l_end && buf[q..q + 3] == [0xE2, 0x80, 0x9C] {
                if open_smart.is_none() {
                    open_smart = Some(q);
                }
                q += 3;
            } else if q + 3 <= l_end && buf[q..q + 3] == [0xE2, 0x80, 0x9D] {
                if let Some(o) = open_smart.take() {
                    blank_range_preserve_newlines(&mut buf, o, q + 3);
                }
                q += 3;
            } else {
                q += 1;
            }
        }

        l_start = if l_end < len { l_end + 1 } else { len };
    }

    // 5. File paths starting with '@': @/..., @~/..., @~\..., @./..., @.\..., @../..., @..\..., @C:\..., @C:/...
    let mut path_spans: Vec<(usize, usize)> = Vec::new();
    if let Ok(text_view) = std::str::from_utf8(&buf) {
        let b = text_view.as_bytes();
        let mut k = 0;
        while k < len {
            if b[k] == b'@' {
                let after = k + 1;
                let is_path = if let Some(&next) = b.get(after) {
                    next == b'/'
                        || (next == b'~'
                            && matches!(b.get(after + 1), Some(b'/') | Some(b'\\')))
                        || (next == b'.'
                            && (matches!(b.get(after + 1), Some(b'/') | Some(b'\\'))
                                || (b.get(after + 1) == Some(&b'.')
                                    && matches!(b.get(after + 2), Some(b'/') | Some(b'\\')))))
                        || (next.is_ascii_alphabetic()
                            && b.get(after + 1) == Some(&b':')
                            && matches!(b.get(after + 2), Some(b'\\') | Some(b'/')))
                } else {
                    false
                };
                if is_path && let Some(tail) = text_view.get(after..) {
                    let end = tail
                        .char_indices()
                        .find(|&(_, ch)| is_file_path_terminator(ch))
                        .map_or(len, |(rel, _)| after + rel);
                    path_spans.push((k, end));
                    k = end;
                    continue;
                }
            }
            k += 1;
        }
    }
    for (start, end) in path_spans {
        blank_range_preserve_newlines(&mut buf, start, end);
    }

    String::from_utf8(buf).unwrap_or_else(|_| " ".repeat(text.len()))
}

fn is_file_path_terminator(ch: char) -> bool {
    ch.is_whitespace()
        || matches!(
            ch,
            '，' | '。'
                | '、'
                | '！'
                | '？'
                | '；'
                | '：'
                | '（'
                | '）'
                | '【'
                | '】'
                | '「'
                | '」'
                | ','
                | ';'
                | ')'
                | ']'
                | '>'
                | '"'
                | '\''
        )
}

fn is_valid_mention_left_boundary(text: &str, at_byte: usize) -> bool {
    let Some(before) = text.get(..at_byte) else {
        return false;
    };
    before
        .chars()
        .next_back()
        .is_none_or(|c| !(c.is_ascii_alphanumeric() || matches!(c, '_' | '.' | '+')))
}

fn is_valid_mention_right_boundary(text: &str, end_byte: usize) -> bool {
    let Some(after) = text.get(end_byte..) else {
        return false;
    };
    after
        .chars()
        .next()
        .is_none_or(|c| !c.is_alphanumeric() && c != '_' && c != '-')
}

fn match_name_at(text: &str, start_byte: usize, lower_chars: &[char]) -> Option<usize> {
    if lower_chars.is_empty() {
        return None;
    }
    let slice = text.get(start_byte..)?;
    let mut matched = 0usize;
    for (rel, ch) in slice.char_indices() {
        for lc in ch.to_lowercase() {
            if lower_chars.get(matched) != Some(&lc) {
                return None;
            }
            matched += 1;
        }
        if matched == lower_chars.len() {
            let end_byte = start_byte + rel + ch.len_utf8();
            if is_valid_mention_right_boundary(text, end_byte) {
                return Some(end_byte);
            }
            return None;
        }
        if matched > lower_chars.len() {
            return None;
        }
    }
    None
}

/// Linear-time, UTF-8-safe `@mention` parser supporting longest-match case-insensitive
/// candidates, `@everyone` ambiguity detection, and byte-accurate offsets after blanking.
pub fn parse_bot_mentions_detailed(
    text: &str,
    candidates: &[MentionCandidate],
) -> ParsedBotMentions {
    let blanked = blank_non_mention_regions(text);

    let mut prepared: Vec<(Vec<char>, &MentionCandidate, usize)> = candidates
        .iter()
        .enumerate()
        .filter_map(|(idx, cand)| {
            let trimmed = cand.name.trim();
            if trimmed.is_empty() {
                return None;
            }
            let lower: Vec<char> = trimmed.chars().flat_map(char::to_lowercase).collect();
            if lower.is_empty() {
                return None;
            }
            Some((lower, cand, idx))
        })
        .collect();

    prepared.sort_by(|a, b| {
        b.0.len()
            .cmp(&a.0.len())
            .then_with(|| b.1.name.len().cmp(&a.1.name.len()))
            .then_with(|| a.2.cmp(&b.2))
    });

    const EVERYONE_CHARS: [char; 8] = ['e', 'v', 'e', 'r', 'y', 'o', 'n', 'e'];

    let mut by_first: HashMap<char, Vec<usize>> = HashMap::new();
    for (pi, (lower, _, _)) in prepared.iter().enumerate() {
        if let Some(&first) = lower.first() {
            by_first.entry(first).or_default().push(pi);
        }
    }

    let mut result = ParsedBotMentions::default();
    let mut seen_ids: HashSet<String> = HashSet::new();
    let bytes = blanked.as_bytes();
    let len = bytes.len();
    let mut i = 0usize;

    while i < len {
        if bytes[i] != b'@' {
            i += 1;
            continue;
        }
        if !is_valid_mention_left_boundary(&blanked, i) {
            i += 1;
            continue;
        }
        let after_at = i + 1;
        let Some(first_ch) = blanked.get(after_at..).and_then(|s| s.chars().next()) else {
            break;
        };
        let first_lc = first_ch.to_lowercase().next().unwrap_or(first_ch);

        let mut best_member: Option<(&MentionCandidate, usize)> = None;
        if let Some(bucket) = by_first.get(&first_lc) {
            for &pi in bucket {
                if let Some((lower, cand, _)) = prepared.get(pi)
                    && let Some(end_byte) = match_name_at(&blanked, after_at, lower)
                {
                    best_member = Some((cand, end_byte));
                    break;
                }
            }
        }

        let everyone_end = if first_lc == 'e' {
            match_name_at(&blanked, after_at, &EVERYONE_CHARS)
        } else {
            None
        };

        match (best_member, everyone_end) {
            (Some((cand, m_end)), Some(e_end)) if m_end > e_end => {
                if seen_ids.insert(cand.id.clone()) {
                    result.members.push(cand.clone());
                }
                result.occurrences.push(MentionOccurrence {
                    kind: MentionOccurrenceKind::Member,
                    candidate: Some(cand.clone()),
                    start: i,
                    end: m_end,
                });
                i = m_end;
            }
            (Some(_), Some(e_end)) => {
                result.ambiguous_everyone = true;
                result.occurrences.push(MentionOccurrence {
                    kind: MentionOccurrenceKind::Ambiguous,
                    candidate: None,
                    start: i,
                    end: e_end,
                });
                i = e_end;
            }
            (Some((cand, m_end)), None) => {
                if seen_ids.insert(cand.id.clone()) {
                    result.members.push(cand.clone());
                }
                result.occurrences.push(MentionOccurrence {
                    kind: MentionOccurrenceKind::Member,
                    candidate: Some(cand.clone()),
                    start: i,
                    end: m_end,
                });
                i = m_end;
            }
            (None, Some(e_end)) => {
                result.has_everyone = true;
                result.occurrences.push(MentionOccurrence {
                    kind: MentionOccurrenceKind::Everyone,
                    candidate: None,
                    start: i,
                    end: e_end,
                });
                i = e_end;
            }
            (None, None) => {
                i += 1;
            }
        }
    }

    result
}

/// Extracts unique matched `@mention` candidates in first-mention order.
pub fn parse_bot_mentions(text: &str, candidates: &[MentionCandidate]) -> Vec<MentionCandidate> {
    parse_bot_mentions_detailed(text, candidates).members
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct GroupDispatchTargets {
    pub dispatched_bot_ids: Vec<String>,
    pub held_bot_ids: Vec<String>,
    pub remote_targets: Vec<RemoteBotMember>,
    pub handed_off: bool,
}

/// Routes a group message to active local members (partitioned into dispatched vs paused-held)
/// and pinned remote members according to `group.reply_mode` and `@mention` handoffs.
pub fn resolve_group_targets(
    group: &BotGroup,
    text: &str,
    sender_bot_id: Option<&str>,
    active_local_candidates: &[MentionCandidate],
    paused_bot_ids: &HashSet<String>,
) -> GroupDispatchTargets {
    let active_local_ids: HashSet<&str> = group.member_bot_ids.iter().map(String::as_str).collect();
    let mut combined_candidates: Vec<MentionCandidate> = active_local_candidates
        .iter()
        .filter(|c| !c.is_remote && active_local_ids.contains(c.id.as_str()))
        .cloned()
        .collect();
    for rm in &group.remote_members {
        combined_candidates.push(MentionCandidate {
            id: rm.member_key(),
            name: rm.name.clone(),
            is_remote: true,
        });
    }

    let parsed = parse_bot_mentions_detailed(text, &combined_candidates);
    let mut local_targets: Vec<String> = Vec::new();
    let mut remote_targets: Vec<RemoteBotMember> = Vec::new();

    let push_local = |list: &mut Vec<String>, bid: &str| {
        if sender_bot_id != Some(bid)
            && active_local_ids.contains(bid)
            && !list.iter().any(|x| x == bid)
        {
            list.push(bid.to_string());
        }
    };
    let push_remote = |list: &mut Vec<RemoteBotMember>, key: &str| {
        if let Some(rm) = group.remote_members.iter().find(|r| r.member_key() == key)
            && !list
                .iter()
                .any(|x| x.host_id == rm.host_id && x.bot_id == rm.bot_id)
        {
            list.push(rm.clone());
        }
    };

    match &group.reply_mode {
        BotGroupReplyMode::Bot { bot_id } => {
            if sender_bot_id == Some(bot_id.as_str()) {
                for m in &parsed.members {
                    if m.is_remote {
                        push_remote(&mut remote_targets, &m.id);
                    } else {
                        push_local(&mut local_targets, &m.id);
                    }
                }
            } else {
                push_local(&mut local_targets, bot_id);
            }
        }
        BotGroupReplyMode::All => {
            for bid in &group.member_bot_ids {
                push_local(&mut local_targets, bid);
            }
            for m in &parsed.members {
                if m.is_remote {
                    push_remote(&mut remote_targets, &m.id);
                }
            }
        }
        BotGroupReplyMode::Mentions => {
            if parsed.has_everyone {
                for bid in &group.member_bot_ids {
                    push_local(&mut local_targets, bid);
                }
            }
            for m in &parsed.members {
                if m.is_remote {
                    push_remote(&mut remote_targets, &m.id);
                } else {
                    push_local(&mut local_targets, &m.id);
                }
            }
        }
    }

    let mut dispatched_bot_ids = Vec::new();
    let mut held_bot_ids = Vec::new();
    for bid in local_targets {
        if paused_bot_ids.contains(&bid) {
            held_bot_ids.push(bid);
        } else {
            dispatched_bot_ids.push(bid);
        }
    }
    let handed_off = sender_bot_id.is_some()
        && (!dispatched_bot_ids.is_empty()
            || !held_bot_ids.is_empty()
            || !remote_targets.is_empty());

    GroupDispatchTargets {
        dispatched_bot_ids,
        held_bot_ids,
        remote_targets,
        handed_off,
    }
}

// ---------------------------------------------------------------------------
// BotRequestType & 7-Code `evaluate_bot_request_policy`
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BotRequestType {
    Status,
    Question,
    Handoff,
}

impl BotRequestType {
    pub const ALL: [Self; 3] = [Self::Status, Self::Question, Self::Handoff];

    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Status => "status",
            Self::Question => "question",
            Self::Handoff => "handoff",
        }
    }

    pub fn parse(raw: &str) -> Option<Self> {
        let trimmed = raw.trim();
        Self::ALL.into_iter().find(|t| t.as_str() == trimmed)
    }

    pub const fn awaits_answer(self) -> bool {
        matches!(self, Self::Question)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BotRequestCaller {
    Owner,
    Bot {
        bot_id: String,
        session_id: Option<String>,
    },
    Worker {
        parent_bot_id: Option<String>,
        parent_session_id: Option<String>,
        is_group_worker: bool,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BotRequestChatTarget {
    Default,
    New,
    Session(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LocalBotPolicyEntry {
    pub id: String,
    pub name: String,
    pub paused: bool,
    pub home_chat_session_id: Option<String>,
    pub known_chat_session_ids: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PairedBotEntry {
    pub id: String,
    pub name: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PairedComputerRoster {
    pub computer_name: String,
    pub host_id: String,
    pub bots: Vec<PairedBotEntry>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct BotRequestPolicyContext {
    pub bots_enabled: bool,
    pub local_bots: Vec<LocalBotPolicyEntry>,
    pub paired_rosters: Vec<PairedComputerRoster>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BotRequestPolicyInput {
    pub caller: BotRequestCaller,
    pub target_bot: String,
    pub chat_target: BotRequestChatTarget,
    pub request_type_raw: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ApprovedBotRequest {
    pub target_bot_id: String,
    pub request_type: BotRequestType,
    pub target_chat: BotRequestChatTarget,
    pub awaits_answer: bool,
}

fn find_local_bot<'a>(
    bots: &'a [LocalBotPolicyEntry],
    query: &str,
) -> Option<&'a LocalBotPolicyEntry> {
    let trimmed = query.trim();
    if trimmed.is_empty() {
        return None;
    }
    bots.iter()
        .find(|b| b.id == trimmed)
        .or_else(|| {
            let q_lower = trimmed.to_lowercase();
            bots.iter()
                .find(|b| b.name.trim().to_lowercase() == q_lower)
        })
}

fn find_paired_computer<'a>(
    rosters: &'a [PairedComputerRoster],
    query: &str,
) -> Option<&'a PairedComputerRoster> {
    let trimmed = query.trim();
    if trimmed.is_empty() {
        return None;
    }
    let q_lower = trimmed.to_lowercase();
    rosters.iter().find(|r| {
        r.bots
            .iter()
            .any(|b| b.id == trimmed || b.name.trim().to_lowercase() == q_lower)
    })
}

/// Evaluates bot-to-bot / worker-to-bot request authorization in exact upstream precedence:
/// `bots_disabled` > `bot_not_found` > `bot_chat_not_found` > `bot_self` > `bot_paused`
/// > `bot_request_type` > `bot_request_worker`.
pub fn evaluate_bot_request_policy(
    ctx: &BotRequestPolicyContext,
    input: &BotRequestPolicyInput,
) -> Result<ApprovedBotRequest, BotError> {
    // 1. bots_disabled
    if !ctx.bots_enabled {
        return Err(BotError::new(
            BotErrorCode::BotsDisabled,
            "Bots are disabled on this host. Enable bots in Settings before sending requests.",
        ));
    }

    // 2. bot_not_found (with paired computer `--on <computer>` suggestion)
    let Some(target) = find_local_bot(&ctx.local_bots, &input.target_bot) else {
        if let Some(paired) = find_paired_computer(&ctx.paired_rosters, &input.target_bot) {
            return Err(BotError::new(
                BotErrorCode::BotNotFound,
                format!(
                    "Bot '{}' was not found on this computer, but exists on paired computer '{}'. Retry with `--on {}`.",
                    input.target_bot, paired.computer_name, paired.computer_name
                ),
            ));
        }
        return Err(BotError::new(
            BotErrorCode::BotNotFound,
            format!(
                "Bot '{}' was not found on this computer or any paired computer. Run `orca bot list` to see available bots.",
                input.target_bot
            ),
        ));
    };

    // 3. bot_chat_not_found
    if let BotRequestChatTarget::Session(ref sid) = input.chat_target {
        let matches_home = target.home_chat_session_id.as_deref() == Some(sid.as_str());
        let matches_known = target.known_chat_session_ids.iter().any(|k| k == sid);
        if !matches_home && !matches_known {
            return Err(BotError::new(
                BotErrorCode::BotChatNotFound,
                format!(
                    "Chat session '{}' was not found for bot '{}'. Run `orca bot chats --bot {}` or pass `--chat new`.",
                    sid, target.name, target.id
                ),
            ));
        }
    }

    // 4. bot_self (direct bot self-send & worker self-send to parent chat; allows --chat new or other valid chat)
    match &input.caller {
        BotRequestCaller::Bot { bot_id, session_id } if bot_id == &target.id => {
            let to_other_chat = matches!(&input.chat_target, BotRequestChatTarget::New)
                || matches!(
                    &input.chat_target,
                    BotRequestChatTarget::Session(sid)
                        if session_id.as_deref().is_some_and(|cs| cs != sid)
                );
            if !to_other_chat {
                return Err(BotError::new(
                    BotErrorCode::BotSelf,
                    format!(
                        "Bot '{}' cannot send a request to its own current chat. Pass `--chat <id>` to target another chat or `--chat new` to start a new chat.",
                        target.name
                    ),
                ));
            }
        }
        BotRequestCaller::Worker {
            parent_bot_id: Some(pid),
            parent_session_id,
            ..
        } if pid == &target.id => {
            let to_other_chat = matches!(&input.chat_target, BotRequestChatTarget::New)
                || matches!(
                    &input.chat_target,
                    BotRequestChatTarget::Session(sid)
                        if parent_session_id.as_deref().is_some_and(|ps| ps != sid)
                );
            if !to_other_chat {
                return Err(BotError::new(
                    BotErrorCode::BotSelf,
                    format!(
                        "Worker cannot send a request to its parent bot '{}' current chat. Pass `--chat <id>` to target another chat or `--chat new` to start a new chat.",
                        target.name
                    ),
                ));
            }
        }
        _ => {}
    }

    // 5. bot_paused
    if target.paused {
        return Err(BotError::new(
            BotErrorCode::BotPaused,
            format!(
                "Bot '{}' is paused and cannot accept requests right now. Unpause the bot first.",
                target.name
            ),
        ));
    }

    // 6. bot_request_type
    let Some(request_type) = BotRequestType::parse(&input.request_type_raw) else {
        return Err(BotError::new(
            BotErrorCode::BotRequestType,
            format!(
                "Invalid bot request type '{}'. Allowed types are: status, question, handoff.",
                input.request_type_raw
            ),
        ));
    };

    // 7. bot_request_worker
    if let BotRequestCaller::Worker {
        parent_bot_id,
        is_group_worker,
        ..
    } = &input.caller
    {
        if request_type == BotRequestType::Handoff {
            return Err(BotError::new(
                BotErrorCode::BotRequestWorker,
                "Workers cannot hand off jobs to another bot. Report back to your parent bot so it can hand the job off.",
            ));
        }
        if *is_group_worker && parent_bot_id.as_deref() != Some(target.id.as_str()) {
            return Err(BotError::new(
                BotErrorCode::BotRequestWorker,
                "Group workers must report to their parent bot instead of messaging other bots directly.",
            ));
        }
    }

    Ok(ApprovedBotRequest {
        target_bot_id: target.id.clone(),
        request_type,
        target_chat: input.chat_target.clone(),
        awaits_answer: request_type.awaits_answer(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Instant;

    fn sample_group() -> BotGroup {
        let mut sids = BTreeMap::new();
        sids.insert("b1".to_string(), "sess-b1-root".to_string());
        sids.insert("b2".to_string(), "sess-b2-root".to_string());
        let mut tsids = BTreeMap::new();
        tsids.insert("b1".to_string(), "sess-b1-t1".to_string());
        tsids.insert("b2".to_string(), "sess-b2-t1".to_string());
        BotGroup {
            id: "grp-1".to_string(),
            name: "Incident Room".to_string(),
            folder: bot_group_folder_name("Incident Room"),
            member_bot_ids: vec!["b1".to_string(), "b2".to_string()],
            session_ids: sids,
            former_session_ids: BTreeMap::new(),
            reply_mode: BotGroupReplyMode::Bot {
                bot_id: "b1".to_string(),
            },
            threads: vec![BotGroupThread {
                id: "t1".to_string(),
                title: "Outage triage".to_string(),
                session_ids: tsids,
                former_session_ids: BTreeMap::new(),
                created_at_ms: 100,
                updated_at_ms: 100,
            }],
            remote_members: Vec::new(),
            images: Vec::new(),
            created_at_ms: 100,
            updated_at_ms: 100,
        }
    }

    #[test]
    fn bg_01_mention_linear_time_and_unclosed_fence() {
        let members: Vec<MentionCandidate> = (0..32)
            .map(|i| MentionCandidate {
                id: format!("b{i}"),
                name: format!("Scout{i}"),
                is_remote: false,
            })
            .collect();

        let t0 = Instant::now();
        let m1 = parse_bot_mentions(&"@".repeat(50_000), &members);
        assert!(m1.is_empty());
        assert!(t0.elapsed().as_millis() < 200);

        let t1 = Instant::now();
        let m2 = parse_bot_mentions(&"@unknown ".repeat(10_000), &members);
        assert!(m2.is_empty());
        assert!(t1.elapsed().as_millis() < 200);

        let t2 = Instant::now();
        let m3 = parse_bot_mentions(&"@a@b@c".repeat(10_000), &members);
        assert!(m3.is_empty());
        assert!(t2.elapsed().as_millis() < 200);

        let unclosed = format!("```\n{}", "@Scout0 ".repeat(10_000));
        let t3 = Instant::now();
        let m4 = parse_bot_mentions(&unclosed, &members);
        assert!(m4.is_empty());
        assert!(t3.elapsed().as_millis() < 200);
    }

    #[test]
    fn bg_02_mention_longest_match_everyone_clash_and_unicode() {
        let candidates = vec![
            MentionCandidate { id: "1".to_string(), name: "Ops".to_string(), is_remote: false },
            MentionCandidate { id: "2".to_string(), name: "Ops Bot".to_string(), is_remote: false },
            MentionCandidate { id: "3".to_string(), name: "Everyone".to_string(), is_remote: false },
            MentionCandidate { id: "4".to_string(), name: "Everyone Helper".to_string(), is_remote: false },
        ];

        assert_eq!(
            parse_bot_mentions("@Ops Bot check", &candidates)
                .iter()
                .map(|m| m.id.as_str())
                .collect::<Vec<_>>(),
            vec!["2"]
        );
        assert_eq!(
            parse_bot_mentions("@Ops, check", &candidates)
                .iter()
                .map(|m| m.id.as_str())
                .collect::<Vec<_>>(),
            vec!["1"]
        );
        assert!(parse_bot_mentions("@OpsBot", &candidates).is_empty());
        assert!(parse_bot_mentions("user@Ops.com", &candidates).is_empty());

        // @everyone clashes with member named "Everyone" -> ambiguous
        let amb = parse_bot_mentions_detailed("@everyone hi", &candidates);
        assert!(amb.ambiguous_everyone);
        assert!(!amb.has_everyone);
        assert!(amb.members.is_empty());
        assert_eq!(amb.occurrences[0].kind, MentionOccurrenceKind::Ambiguous);

        // Longer member name "Everyone Helper" wins over @everyone
        let eh = parse_bot_mentions_detailed("@Everyone Helper hi", &candidates);
        assert!(!eh.has_everyone);
        assert!(!eh.ambiguous_everyone);
        assert_eq!(eh.members.len(), 1);
        assert_eq!(eh.members[0].id, "4");

        // CJK + fullwidth punctuation boundaries (zero UTF-8 panic)
        let cjk = parse_bot_mentions_detailed("你好@Ops！", &candidates);
        assert_eq!(cjk.members.len(), 1);
        assert_eq!(cjk.members[0].id, "1");
        assert_eq!(&"你好@Ops！"[cjk.occurrences[0].start..cjk.occurrences[0].end], "@Ops");
    }

    #[test]
    fn bg_03_mention_blanking_code_quotes_paths_history_and_offsets() {
        let candidates = vec![MentionCandidate {
            id: "s".to_string(),
            name: "Scout".to_string(),
            is_remote: false,
        }];
        let samples = [
            "`@Scout`",
            "```\n@Scout\n```",
            "~~~\n@Scout\n~~~",
            "> @Scout",
            "\"@Scout\"",
            "\u{201c}@Scout\u{201d}",
            "<quoted_room_history>@Scout</quoted_room_history>",
            "@/Users/a/file.rs",
            "@~/notes.md",
            "@C:\\dir\\f.txt",
        ];
        for sample in samples {
            assert!(
                parse_bot_mentions(sample, &candidates).is_empty(),
                "expected no mention in {sample}"
            );
        }

        // Unclosed inline backtick does NOT blind subsequent mention
        let recovered = parse_bot_mentions("unclosed ` backtick then @Scout!", &candidates);
        assert_eq!(recovered.len(), 1);

        // Byte offsets after blanked multibyte block remain exact
        let msg = "<quoted_room_history>你好 @Scout</quoted_room_history>\nReal @Scout check";
        let detailed = parse_bot_mentions_detailed(msg, &candidates);
        assert_eq!(detailed.occurrences.len(), 1);
        let occ = &detailed.occurrences[0];
        assert_eq!(&msg[occ.start..occ.end], "@Scout");
    }

    #[test]
    fn bg_04_remote_cap_key_collision_and_remote_ask_precedence() {
        let mut g = sample_group();
        assert!(g.validate().is_ok());
        assert_eq!(g.folder.as_str(), "group-incident-room");
        assert!(BotGroupFolderSlug::new("../escape").is_err());

        // Local member caps: 0 -> err, 32 -> ok, 33 -> err
        g.member_bot_ids.clear();
        assert_eq!(g.validate().unwrap_err().code, BotErrorCode::InvalidArgument);
        g.member_bot_ids = (0..32).map(|i| format!("b{i}")).collect();
        g.reply_mode = BotGroupReplyMode::All;
        assert!(g.validate().is_ok());
        g.member_bot_ids.push("b32".to_string());
        assert_eq!(g.validate().unwrap_err().code, BotErrorCode::InvalidArgument);

        // Remote member caps: 31 -> ok, 32 -> InvalidArgument
        g.member_bot_ids = vec!["b1".to_string()];
        g.remote_members = (0..31)
            .map(|i| RemoteBotMember {
                host_id: "srv1".to_string(),
                bot_id: format!("rb{i}"),
                name: format!("Remote {i}"),
                chat_session_id: format!("pinned-{i}"),
            })
            .collect();
        assert!(g.validate().is_ok());
        g.remote_members.push(RemoteBotMember {
            host_id: "srv1".to_string(),
            bot_id: "rb31".to_string(),
            name: "Remote 31".to_string(),
            chat_session_id: "pinned-31".to_string(),
        });
        assert_eq!(g.validate().unwrap_err().code, BotErrorCode::InvalidArgument);

        // Key disambiguation & pinned chat_session_id preserved on rename
        let mut rm = RemoteBotMember {
            host_id: "srv1".to_string(),
            bot_id: "b1".to_string(),
            name: "Remote B1".to_string(),
            chat_session_id: "pinned-sess".to_string(),
        };
        assert_eq!(rm.member_key(), "remote:srv1:b1");
        assert_ne!(rm.member_key(), "b1");
        rm.rename("Renamed Remote").expect("rename");
        assert_eq!(rm.name, "Renamed Remote");
        assert_eq!(rm.chat_session_id, "pinned-sess");

        // RemoteAsk: Notice -> Answer allowed; Answer -> Notice forbidden
        let mut ask = RemoteAsk {
            id: "ra-1".to_string(),
            host_id: "srv1".to_string(),
            bot_id: "b1".to_string(),
            chat_session_id: "pinned-sess".to_string(),
            state: RemoteAskState::Sent,
            reply: None,
            created_at_ms: 100,
            updated_at_ms: None,
        };
        ask.apply_reply(RemoteAskReply::Notice { text: "queued".to_string() }, 200)
            .expect("notice");
        ask.apply_reply(
            RemoteAskReply::Answer { text: "done".to_string(), truncated: true },
            300,
        )
        .expect("answer replaces notice");
        assert!(matches!(ask.reply, Some(RemoteAskReply::Answer { truncated: true, .. })));
        let err = ask
            .apply_reply(RemoteAskReply::Notice { text: "late notice".to_string() }, 400)
            .unwrap_err();
        assert_eq!(err.code, BotErrorCode::InvalidArgument);
        assert!(matches!(ask.reply, Some(RemoteAskReply::Answer { .. })));
    }
}

#[cfg(test)]
mod policy_and_session_tests {
    use super::*;

    fn sample_group() -> BotGroup {
        let mut sids = BTreeMap::new();
        sids.insert("b1".to_string(), "sess-b1-root".to_string());
        sids.insert("b2".to_string(), "sess-b2-root".to_string());
        let mut tsids = BTreeMap::new();
        tsids.insert("b1".to_string(), "sess-b1-t1".to_string());
        tsids.insert("b2".to_string(), "sess-b2-t1".to_string());
        BotGroup {
            id: "grp-1".to_string(),
            name: "Incident Room".to_string(),
            folder: bot_group_folder_name("Incident Room"),
            member_bot_ids: vec!["b1".to_string(), "b2".to_string()],
            session_ids: sids,
            former_session_ids: BTreeMap::new(),
            reply_mode: BotGroupReplyMode::Mentions,
            threads: vec![BotGroupThread {
                id: "t1".to_string(),
                title: "Outage triage".to_string(),
                session_ids: tsids,
                former_session_ids: BTreeMap::new(),
                created_at_ms: 100,
                updated_at_ms: 100,
            }],
            remote_members: vec![RemoteBotMember {
                host_id: "mac-studio".to_string(),
                bot_id: "rb1".to_string(),
                name: "CloudOps".to_string(),
                chat_session_id: "remote-pinned-1".to_string(),
            }],
            images: Vec::new(),
            created_at_ms: 100,
            updated_at_ms: 100,
        }
    }

    #[test]
    fn bg_05_threads_former_sessions_send_limits_and_router() {
        // Thread title 80 CJK chars ok, 81 err, derive truncates safely
        assert!(validate_group_thread_title(&"测".repeat(80)).is_ok());
        assert_eq!(
            validate_group_thread_title(&"测".repeat(81)).unwrap_err().code,
            BotErrorCode::InvalidArgument
        );
        let derived = derive_group_thread_title(&format!("  {}  ", "🚀".repeat(120)));
        assert_eq!(derived.chars().count(), 80);

        // former_session_ids resolves history lookup but denies active access
        let mut g = sample_group();
        assert!(g.has_active_session_access("sess-b2-root"));
        assert!(g.has_active_session_access("sess-b2-t1"));
        g.remove_member("b2");
        assert!(!g.has_active_session_access("sess-b2-root"));
        assert!(!g.has_active_session_access("sess-b2-t1"));
        assert_eq!(
            g.find_room_of_session("sess-b2-root"),
            Some(GroupSessionLookup::Former {
                thread_id: None,
                bot_id: "b2".to_string(),
            })
        );
        assert_eq!(
            g.find_room_of_session("sess-b2-t1"),
            Some(GroupSessionLookup::Former {
                thread_id: Some("t1".to_string()),
                bot_id: "b2".to_string(),
            })
        );

        // PeerScope self-session rejection & synthetic root serde
        let bad_scope = BotGroupPeerScope {
            root_request_id: "req-1".to_string(),
            root_sequence: 1,
            source_session_id: "s1".to_string(),
            target_session_id: "s1".to_string(),
        };
        assert_eq!(bad_scope.validate().unwrap_err().code, BotErrorCode::InvalidArgument);
        let good_scope = BotGroupPeerScope {
            target_session_id: "s2".to_string(),
            ..bad_scope
        };
        assert!(good_scope.validate().is_ok());
        assert_eq!(
            serde_json::to_string(&BotGroupSyntheticRoot::FromItems).expect("ser"),
            "\"fromItems\""
        );
        assert_eq!(
            serde_json::to_string(&BotGroupSyntheticRoot::FromReports).expect("ser"),
            "\"fromReports\""
        );

        // BotGroupSend limits
        let img = BotGroupImage {
            id: "i1".to_string(),
            path: "a.png".to_string(),
            mime: "image/png".to_string(),
        };
        assert_eq!(
            BotGroupSend { text: "".to_string(), images: vec![] }.validate().unwrap_err().code,
            BotErrorCode::InvalidArgument
        );
        assert!(BotGroupSend { text: "   ".to_string(), images: vec![img.clone()] }.validate().is_ok());
        assert!(BotGroupSend { text: "a".repeat(100_000), images: vec![] }.validate().is_ok());
        assert_eq!(
            BotGroupSend { text: "a".repeat(100_001), images: vec![] }.validate().unwrap_err().code,
            BotErrorCode::InvalidArgument
        );
        assert!(BotGroupSend { text: "ok".to_string(), images: vec![img.clone(); 20] }.validate().is_ok());
        assert_eq!(
            BotGroupSend { text: "ok".to_string(), images: vec![img; 21] }.validate().unwrap_err().code,
            BotErrorCode::InvalidArgument
        );

        // Router partitions paused-held local targets, ignores former members, routes remote
        let mut g2 = sample_group();
        g2.reply_mode = BotGroupReplyMode::Mentions;
        let cands = vec![
            MentionCandidate { id: "b1".to_string(), name: "Alpha".to_string(), is_remote: false },
            MentionCandidate { id: "b2".to_string(), name: "Beta".to_string(), is_remote: false },
        ];
        let mut paused = HashSet::new();
        paused.insert("b2".to_string());
        let routed = resolve_group_targets(&g2, "@Alpha @Beta @CloudOps check", None, &cands, &paused);
        assert_eq!(routed.dispatched_bot_ids, vec!["b1"]);
        assert_eq!(routed.held_bot_ids, vec!["b2"]);
        assert_eq!(routed.remote_targets.len(), 1);
        assert_eq!(routed.remote_targets[0].chat_session_id, "remote-pinned-1");
        assert!(!routed.handed_off);
    }

    #[test]
    fn bg_06_all_seven_request_policy_codes_and_exact_precedence() {
        let base_ctx = BotRequestPolicyContext {
            bots_enabled: true,
            local_bots: vec![
                LocalBotPolicyEntry {
                    id: "b1".to_string(),
                    name: "Alpha".to_string(),
                    paused: false,
                    home_chat_session_id: Some("home-1".to_string()),
                    known_chat_session_ids: vec!["home-1".to_string(), "chat-alt".to_string()],
                },
                LocalBotPolicyEntry {
                    id: "b2".to_string(),
                    name: "PausedBot".to_string(),
                    paused: true,
                    home_chat_session_id: Some("home-2".to_string()),
                    known_chat_session_ids: vec!["home-2".to_string()],
                },
            ],
            paired_rosters: vec![PairedComputerRoster {
                computer_name: "studio-box".to_string(),
                host_id: "host-2".to_string(),
                bots: vec![PairedBotEntry {
                    id: "remote-1".to_string(),
                    name: "Builder".to_string(),
                }],
            }],
        };

        // Precedence 1: disabled + paused + self + bad type => BotsDisabled
        let mut disabled_ctx = base_ctx.clone();
        disabled_ctx.bots_enabled = false;
        let err = evaluate_bot_request_policy(
            &disabled_ctx,
            &BotRequestPolicyInput {
                caller: BotRequestCaller::Bot { bot_id: "b2".to_string(), session_id: Some("home-2".to_string()) },
                target_bot: "b2".to_string(),
                chat_target: BotRequestChatTarget::Default,
                request_type_raw: "invalid".to_string(),
            },
        )
        .unwrap_err();
        assert_eq!(err.code, BotErrorCode::BotsDisabled);

        // Precedence 2: BotNotFound (paired roster suggests `--on studio-box`; missing suggests list)
        let err_paired = evaluate_bot_request_policy(
            &base_ctx,
            &BotRequestPolicyInput {
                caller: BotRequestCaller::Owner,
                target_bot: "Builder".to_string(),
                chat_target: BotRequestChatTarget::Default,
                request_type_raw: "question".to_string(),
            },
        )
        .unwrap_err();
        assert_eq!(err_paired.code, BotErrorCode::BotNotFound);
        assert!(err_paired.message.contains("--on studio-box"));

        let err_missing = evaluate_bot_request_policy(
            &base_ctx,
            &BotRequestPolicyInput {
                caller: BotRequestCaller::Owner,
                target_bot: "Ghost".to_string(),
                chat_target: BotRequestChatTarget::Default,
                request_type_raw: "question".to_string(),
            },
        )
        .unwrap_err();
        assert_eq!(err_missing.code, BotErrorCode::BotNotFound);

        // Precedence 3: BotChatNotFound
        let err_chat = evaluate_bot_request_policy(
            &base_ctx,
            &BotRequestPolicyInput {
                caller: BotRequestCaller::Owner,
                target_bot: "b1".to_string(),
                chat_target: BotRequestChatTarget::Session("no-such-chat".to_string()),
                request_type_raw: "question".to_string(),
            },
        )
        .unwrap_err();
        assert_eq!(err_chat.code, BotErrorCode::BotChatNotFound);

        // Precedence 4: enabled + paused + self (!to_other_chat) => BotSelf (before BotPaused!)
        let err_self = evaluate_bot_request_policy(
            &base_ctx,
            &BotRequestPolicyInput {
                caller: BotRequestCaller::Bot { bot_id: "b2".to_string(), session_id: Some("home-2".to_string()) },
                target_bot: "b2".to_string(),
                chat_target: BotRequestChatTarget::Default,
                request_type_raw: "invalid".to_string(),
            },
        )
        .unwrap_err();
        assert_eq!(err_self.code, BotErrorCode::BotSelf);
        assert!(err_self.message.contains("--chat <id>"));
        assert!(err_self.message.contains("--chat new"));

        // Self-send allowed when targeting `--chat new` or another valid chat!
        assert!(evaluate_bot_request_policy(
            &base_ctx,
            &BotRequestPolicyInput {
                caller: BotRequestCaller::Bot { bot_id: "b1".to_string(), session_id: Some("home-1".to_string()) },
                target_bot: "b1".to_string(),
                chat_target: BotRequestChatTarget::New,
                request_type_raw: "question".to_string(),
            },
        ).is_ok());
        assert!(evaluate_bot_request_policy(
            &base_ctx,
            &BotRequestPolicyInput {
                caller: BotRequestCaller::Bot { bot_id: "b1".to_string(), session_id: Some("home-1".to_string()) },
                target_bot: "b1".to_string(),
                chat_target: BotRequestChatTarget::Session("chat-alt".to_string()),
                request_type_raw: "status".to_string(),
            },
        ).is_ok());

        // Worker self-send to parent chat => BotSelf
        let err_wself = evaluate_bot_request_policy(
            &base_ctx,
            &BotRequestPolicyInput {
                caller: BotRequestCaller::Worker {
                    parent_bot_id: Some("b1".to_string()),
                    parent_session_id: Some("home-1".to_string()),
                    is_group_worker: false,
                },
                target_bot: "b1".to_string(),
                chat_target: BotRequestChatTarget::Default,
                request_type_raw: "question".to_string(),
            },
        )
        .unwrap_err();
        assert_eq!(err_wself.code, BotErrorCode::BotSelf);

        // Precedence 5: enabled + paused + bad type (not self) => BotPaused
        let err_paused = evaluate_bot_request_policy(
            &base_ctx,
            &BotRequestPolicyInput {
                caller: BotRequestCaller::Owner,
                target_bot: "PausedBot".to_string(),
                chat_target: BotRequestChatTarget::Default,
                request_type_raw: "invalid".to_string(),
            },
        )
        .unwrap_err();
        assert_eq!(err_paused.code, BotErrorCode::BotPaused);

        // Precedence 6: enabled + unpaused + bad type + worker => BotRequestType
        let err_type = evaluate_bot_request_policy(
            &base_ctx,
            &BotRequestPolicyInput {
                caller: BotRequestCaller::Worker {
                    parent_bot_id: Some("b2".to_string()),
                    parent_session_id: None,
                    is_group_worker: false,
                },
                target_bot: "b1".to_string(),
                chat_target: BotRequestChatTarget::Default,
                request_type_raw: "bogus".to_string(),
            },
        )
        .unwrap_err();
        assert_eq!(err_type.code, BotErrorCode::BotRequestType);

        // Precedence 7: worker handoff refused (BotRequestWorker), ordinary worker question/status OK, group worker refused
        let err_wh = evaluate_bot_request_policy(
            &base_ctx,
            &BotRequestPolicyInput {
                caller: BotRequestCaller::Worker {
                    parent_bot_id: Some("b2".to_string()),
                    parent_session_id: None,
                    is_group_worker: false,
                },
                target_bot: "b1".to_string(),
                chat_target: BotRequestChatTarget::Default,
                request_type_raw: "handoff".to_string(),
            },
        )
        .unwrap_err();
        assert_eq!(err_wh.code, BotErrorCode::BotRequestWorker);
        assert!(err_wh.message.to_lowercase().contains("worker"));
        assert!(err_wh.message.to_lowercase().contains("hand"));
        assert!(err_wh.message.to_lowercase().contains("parent"));

        let ok_wq = evaluate_bot_request_policy(
            &base_ctx,
            &BotRequestPolicyInput {
                caller: BotRequestCaller::Worker {
                    parent_bot_id: Some("b2".to_string()),
                    parent_session_id: None,
                    is_group_worker: false,
                },
                target_bot: "b1".to_string(),
                chat_target: BotRequestChatTarget::Default,
                request_type_raw: "question".to_string(),
            },
        )
        .expect("ordinary worker question allowed");
        assert!(ok_wq.awaits_answer);

        let err_gw = evaluate_bot_request_policy(
            &base_ctx,
            &BotRequestPolicyInput {
                caller: BotRequestCaller::Worker {
                    parent_bot_id: Some("b2".to_string()),
                    parent_session_id: None,
                    is_group_worker: true,
                },
                target_bot: "b1".to_string(),
                chat_target: BotRequestChatTarget::Default,
                request_type_raw: "status".to_string(),
            },
        )
        .unwrap_err();
        assert_eq!(err_gw.code, BotErrorCode::BotRequestWorker);
    }

    #[test]
    fn bg_07_remediation_regressions() {
        // Finding #8: RemoteBotMember colon collision, leading/trailing whitespace, control chars
        let colon_host = RemoteBotMember {
            host_id: "a:b".to_string(),
            bot_id: "c".to_string(),
            name: "First".to_string(),
            chat_session_id: "c1".to_string(),
        };
        assert_eq!(colon_host.validate().unwrap_err().code, BotErrorCode::InvalidArgument);

        let colon_bot = RemoteBotMember {
            host_id: "a".to_string(),
            bot_id: "b:c".to_string(),
            name: "Second".to_string(),
            chat_session_id: "c2".to_string(),
        };
        assert_eq!(colon_bot.validate().unwrap_err().code, BotErrorCode::InvalidArgument);

        let padded_host = RemoteBotMember {
            host_id: " h1 ".to_string(),
            bot_id: "rb1".to_string(),
            name: "Padded".to_string(),
            chat_session_id: "c3".to_string(),
        };
        assert_eq!(padded_host.validate().unwrap_err().code, BotErrorCode::InvalidArgument);

        let ctrl_bot = RemoteBotMember {
            host_id: "h1".to_string(),
            bot_id: "rb\u{0000}1".to_string(),
            name: "Ctrl".to_string(),
            chat_session_id: "c4".to_string(),
        };
        assert_eq!(ctrl_bot.validate().unwrap_err().code, BotErrorCode::InvalidArgument);

        let mut g_dup = sample_group();
        g_dup.remote_members = vec![
            RemoteBotMember {
                host_id: "h1".to_string(),
                bot_id: "rb1".to_string(),
                name: "One".to_string(),
                chat_session_id: "c1".to_string(),
            },
            RemoteBotMember {
                host_id: " h1 ".to_string(),
                bot_id: "rb1".to_string(),
                name: "Two".to_string(),
                chat_session_id: "c2".to_string(),
            },
        ];
        assert_eq!(g_dup.validate().unwrap_err().code, BotErrorCode::InvalidArgument);

        // Finding #9: Escaped straight quotes, independent straight/smart quotes,
        // Windows relative backslash paths, and Unicode whitespace / CJK punctuation after paths
        let candidates = vec![MentionCandidate {
            id: "s".to_string(),
            name: "Scout".to_string(),
            is_remote: false,
        }];
        assert!(parse_bot_mentions(r#""hello \"@Scout\"""#, &candidates).is_empty());
        assert_eq!(
            parse_bot_mentions(r#""hello \\" @Scout"#, &candidates).len(),
            1
        );
        assert!(parse_bot_mentions("\"unclosed straight then \u{201c}@Scout\u{201d}", &candidates).is_empty());
        assert!(parse_bot_mentions("\u{201c}unclosed smart then \"@Scout\"", &candidates).is_empty());
        assert!(parse_bot_mentions(r"@.\Users\Scout", &candidates).is_empty());
        assert!(parse_bot_mentions(r"@..\Scout", &candidates).is_empty());

        for msg in [
            "@/tmp/file.rs\u{3000}@Scout",
            "@/tmp/file.rs\u{00A0}@Scout",
            "@/tmp/file.rs\u{ff0c}@Scout",
        ] {
            let parsed = parse_bot_mentions_detailed(msg, &candidates);
            assert_eq!(parsed.members.len(), 1, "expected @Scout preserved in {msg}");
            assert_eq!(parsed.members[0].id, "s");
            let occ = &parsed.occurrences[0];
            assert_eq!(&msg[occ.start..occ.end], "@Scout");
        }

        // Finding #10: Non-member with stale session_ids cannot gain Active access,
        // and RemoteAskState ALL/as_str/parse round-trip
        let mut g_stale = sample_group();
        g_stale.session_ids.insert("ghost-bot".to_string(), "stale-root-sess".to_string());
        g_stale.threads[0]
            .session_ids
            .insert("ghost-bot".to_string(), "stale-thread-sess".to_string());
        assert!(!g_stale.has_active_session_access("stale-root-sess"));
        assert!(!g_stale.has_active_session_access("stale-thread-sess"));
        assert_eq!(g_stale.find_room_of_session("stale-root-sess"), None);
        assert_eq!(g_stale.find_room_of_session("stale-thread-sess"), None);

        let expected_states = ["sending", "sent", "failed", "unknown"];
        assert_eq!(RemoteAskState::ALL.len(), 4);
        for (st, wire) in RemoteAskState::ALL.into_iter().zip(expected_states) {
            assert_eq!(st.as_str(), wire);
            assert_eq!(RemoteAskState::parse(wire), Some(st));
            let s = serde_json::to_string(&st).expect("ser");
            assert_eq!(s, format!("\"{wire}\""));
            let back: RemoteAskState = serde_json::from_str(&s).expect("de");
            assert_eq!(back, st);
        }
        assert_eq!(RemoteAskState::parse("bogus"), None);
    }
}
