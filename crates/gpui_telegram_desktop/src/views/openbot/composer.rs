//! OpenBot Rich `@`/`$` Token Composer (`OB-F16`).
//!
//! Mirrors `ComposerEditor.tsx`, `ConversationComposer.tsx`, `ComposerAddMenu.tsx`,
//! `ContextUsageMeter.tsx`, and `packages/contracts/src/input-limits.ts`:
//! - `@` picker with 5 ordered sections (`agents`, `files`, `tabs`, `threads`, `messages`),
//!   NFD-folded `agent_mention_rank`, channel-only `@everyone`, and 150ms debounced >=3-char thread search
//! - `$` picker ranking enabled skills (`status == "ok"`, duplicate-name slug disambiguation)
//!   and enabled MCP servers, with separate inline `skill_problems` repair warnings
//! - Exact >4,000-char paste-to-file (`pasted-text.txt`), 100,000-char message cap,
//!   100 MiB per-file / 250 MiB total / 10 message / 50 draft attachment limits
//! - Text-only custom `pi` endpoint image attachment guard with 1-click `Allow Images` patch
//! - GPUI `0.2.2` `render_composer(...) -> Div` with deferred popovers, extension-toned
//!   attachment chips, 80% `ContextUsageMeter`, and tri-state `ComposerSendControl`.

use std::collections::HashMap;

use gpui::prelude::FluentBuilder;
use gpui::*;
use openbot_contracts::limits::{ATTACHMENT_LIMITS, INPUT_LIMITS};
use openbot_contracts::mentions::{MentionableAgentInfo, agent_mention_rank};
use openbot_contracts::queue::AttachmentKind;
use openbot_contracts::tags::{ChatTagKind, EVERYONE_MENTION, serialize_chat_tag_reference};
use serde::{Deserialize, Serialize};

use crate::theme::openbot_palette::{
    OpenBotPalette, file_reference_badge, file_reference_tone, parse_css_color_to_rgba,
};
use crate::theme::{IconName, tg_icon_rgba};

/// Paste length threshold (`4_000` chars) above which pasted text auto-attaches as `pasted-text.txt`.
pub const COMPOSER_PASTED_TEXT_LIMIT: usize = INPUT_LIMITS.pasted_text;
/// Maximum message text length (`100_000` chars).
pub const COMPOSER_MESSAGE_TEXT_LIMIT: usize = INPUT_LIMITS.message_text;
/// Maximum single attachment file size (`100 MiB` = `104_857_600` bytes).
pub const COMPOSER_ATTACHMENT_FILE_BYTES: usize = ATTACHMENT_LIMITS.file_bytes;
/// Maximum total attachment payload (`250 MiB` = `262_144_000` bytes).
pub const COMPOSER_ATTACHMENT_TOTAL_BYTES: usize = ATTACHMENT_LIMITS.total_bytes;
/// Maximum attachments allowed on a single sent message (`10`).
pub const COMPOSER_ATTACHMENTS_LIMIT: usize = INPUT_LIMITS.attachments;
/// Maximum draft attachments held in the composer (`50`).
pub const COMPOSER_DRAFT_ATTACHMENTS_LIMIT: usize = INPUT_LIMITS.draft_attachments;
/// Debounce delay in milliseconds before querying historical thread messages in the `@` picker.
pub const THREAD_SEARCH_DELAY_MS: u64 = 150;
/// Minimum query character count to activate the debounced `messages` section in the `@` picker.
pub const THREAD_SEARCH_MIN_QUERY: usize = 3;
/// Maximum thread message hits displayed in the `@` picker `messages` section.
pub const THREAD_SEARCH_ROWS: usize = 5;
/// Exit animation fallback duration for the deferred `@`/`$` picker popover.
pub const PICKER_EXIT_FALLBACK_MS: u64 = 300;
/// Canonical filename used when pasting >4,000 characters converts the paste to a file attachment.
pub const PASTED_TEXT_FILE_NAME: &str = "pasted-text.txt";
/// Auto-compaction notice threshold (`80%`) displayed on `ContextUsageMeter`.
pub const CONTEXT_COMPACTION_THRESHOLD: f32 = 0.80;
/// Ephemeral side-question tangent maximum turns (`20`).
pub const SIDE_QUESTION_MAX_TURNS: usize = INPUT_LIMITS.side_question_turns;
/// Ephemeral side-question maximum answer characters (`32_000`).
pub const SIDE_QUESTION_MAX_ANSWER_CHARS: usize = INPUT_LIMITS.side_question_answer;

/// Ordered canonical `@` picker sections when debounced message search is eligible.
pub const AT_PICKER_ALL_SECTIONS: [&str; 5] = ["agents", "files", "tabs", "threads", "messages"];
/// Ordered `@` picker sections when a non-empty query has fewer than 3 characters.
pub const AT_PICKER_SHORT_QUERY_SECTIONS: [&str; 4] = ["agents", "files", "tabs", "threads"];

/// Active token trigger inside the rich composer.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum ComposerTrigger {
    #[serde(rename = "@")]
    At,
    #[serde(rename = "$")]
    Dollar,
}

impl ComposerTrigger {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::At => "@",
            Self::Dollar => "$",
        }
    }

    pub fn parse(raw: &str) -> Option<Self> {
        match raw.trim() {
            "@" | "at" => Some(Self::At),
            "$" | "dollar" => Some(Self::Dollar),
            _ => None,
        }
    }
}

/// Skill candidate supplied to the `$` picker.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ComposerSkillEntry {
    pub slug: String,
    pub name: String,
    pub status: String,
}

/// MCP server candidate supplied to the `$` picker.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ComposerMcpEntry {
    pub id: String,
    pub name: String,
    pub enabled: bool,
}

/// Draft file or image attachment queued in the composer.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ComposerAttachmentDraft {
    pub name: String,
    pub size_bytes: u64,
    pub kind: AttachmentKind,
}

impl ComposerAttachmentDraft {
    pub fn new(name: impl Into<String>, size_bytes: u64, kind: AttachmentKind) -> Self {
        Self {
            name: name.into(),
            size_bytes,
            kind,
        }
    }

    pub fn from_raw(name: impl Into<String>, size_bytes: u64, kind: &str) -> Self {
        let parsed_kind = if kind.trim().eq_ignore_ascii_case("image") {
            AttachmentKind::Image
        } else {
            AttachmentKind::File
        };
        Self::new(name, size_bytes, parsed_kind)
    }

    pub const fn is_image(&self) -> bool {
        matches!(self.kind, AttachmentKind::Image)
    }
}

/// File reference candidate for the `@` picker `files` section.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ComposerFileCandidate {
    pub id: String,
    pub name: String,
    pub path: String,
}

/// Open browser tab candidate for the `@` picker `tabs` section.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ComposerTabCandidate {
    pub id: String,
    pub title: String,
    pub url: String,
}

/// Agent side thread candidate for the `@` picker `threads` section.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ComposerThreadCandidate {
    pub id: String,
    pub title: String,
}

/// Historical thread message search hit for the debounced `@` picker `messages` section.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ComposerMessageHit {
    pub id: String,
    pub author: String,
    pub preview: String,
}

/// Ranked agent entry emitted by [`resolve_at_picker`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AtPickerAgentItem {
    pub id: String,
    pub name: String,
    pub rank: u8,
    pub is_everyone: bool,
}

/// Complete resolved `@` token picker model.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AtPickerResolution {
    pub query: String,
    pub at_sections: Vec<String>,
    pub everyone_shown: bool,
    pub messages_debounce_ms: u64,
    pub messages_min_query: usize,
    pub agents: Vec<AtPickerAgentItem>,
    pub files: Vec<ComposerFileCandidate>,
    pub tabs: Vec<ComposerTabCandidate>,
    pub threads: Vec<ComposerThreadCandidate>,
    pub messages: Vec<ComposerMessageHit>,
}

/// Item category in the `$` token picker.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum DollarPickerItemKind {
    Skill,
    Mcp,
}

impl DollarPickerItemKind {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Skill => "skill",
            Self::Mcp => "mcp",
        }
    }
}

/// Selectable entry returned by [`resolve_dollar_picker`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DollarPickerItem {
    pub kind: DollarPickerItemKind,
    pub id: String,
    pub name: String,
    pub display_label: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub slug: Option<String>,
    pub rank: u8,
}

/// Non-selectable broken skill surfaced as an inline repair warning in the `$` picker.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SkillProblemEntry {
    pub slug: String,
    pub name: String,
    pub status: String,
    pub repair_hint: String,
}

/// Complete resolved `$` token picker model.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DollarPickerResolution {
    pub query: String,
    pub dollar_items: Vec<DollarPickerItem>,
    pub skill_problems: Vec<SkillProblemEntry>,
}

/// Result of pasting text into the composer (`> 4_000` chars converts to `pasted-text.txt`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PasteResult {
    pub pasted_as_file: bool,
    pub pasted_file_name: Option<String>,
    pub inline_text_length: usize,
}

fn substring_rank(needle_lower: &str, primary: &str, secondary: &str) -> Option<u8> {
    if needle_lower.is_empty() {
        return Some(0);
    }
    let p_lower = primary.to_lowercase();
    let s_lower = secondary.to_lowercase();
    if p_lower == needle_lower || s_lower == needle_lower {
        Some(0)
    } else if p_lower.starts_with(needle_lower) || s_lower.starts_with(needle_lower) {
        Some(1)
    } else if p_lower.contains(needle_lower) {
        Some(2)
    } else if s_lower.contains(needle_lower) {
        Some(3)
    } else {
        None
    }
}

/// Resolves the 5-section `@` picker using `openbot_contracts::mentions::agent_mention_rank`.
///
/// Section order invariant:
/// - `query.is_empty() || query.chars().count() >= 3`: `["agents", "files", "tabs", "threads", "messages"]`
/// - `0 < query.chars().count() < 3`: omits `"messages"` (`["agents", "files", "tabs", "threads"]`)
/// - `@everyone` is shown iff `in_channel` and `query` is empty or a prefix of `"everyone"`.
pub fn resolve_at_picker(
    query: &str,
    in_channel: bool,
    agents: &[(String, String)],
    files: &[ComposerFileCandidate],
    tabs: &[ComposerTabCandidate],
    threads: &[ComposerThreadCandidate],
    messages: &[ComposerMessageHit],
) -> AtPickerResolution {
    let trimmed = query.trim();
    let query_lower = trimmed.to_lowercase();
    let query_chars = trimmed.chars().count();
    let include_messages = query_chars == 0 || query_chars >= THREAD_SEARCH_MIN_QUERY;

    let at_sections: Vec<String> = if include_messages {
        AT_PICKER_ALL_SECTIONS.iter().map(|s| (*s).to_string()).collect()
    } else {
        AT_PICKER_SHORT_QUERY_SECTIONS
            .iter()
            .map(|s| (*s).to_string())
            .collect()
    };

    let everyone_shown = in_channel
        && agents.len() > 1
        && (query_lower.is_empty() || "everyone".starts_with(&query_lower));

    let mut ranked_agents: Vec<AtPickerAgentItem> = agents
        .iter()
        .filter_map(|(id, name)| {
            let info = MentionableAgentInfo {
                name: name.clone(),
                title: String::new(),
                description: String::new(),
            };
            agent_mention_rank(&info, trimmed).map(|rank| AtPickerAgentItem {
                id: id.clone(),
                name: name.clone(),
                rank,
                is_everyone: false,
            })
        })
        .collect();
    ranked_agents.sort_by(|a, b| {
        a.rank
            .cmp(&b.rank)
            .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
            .then_with(|| a.id.cmp(&b.id))
    });

    if everyone_shown {
        ranked_agents.insert(
            0,
            AtPickerAgentItem {
                id: "everyone".to_string(),
                name: EVERYONE_MENTION.to_string(),
                rank: 0,
                is_everyone: true,
            },
        );
    }

    let matched_files: Vec<ComposerFileCandidate> = files
        .iter()
        .filter(|f| {
            query_lower.is_empty()
                || f.name.to_lowercase().contains(&query_lower)
                || f.path.to_lowercase().contains(&query_lower)
        })
        .cloned()
        .collect();

    let matched_tabs: Vec<ComposerTabCandidate> = tabs
        .iter()
        .filter(|t| {
            query_lower.is_empty()
                || t.title.to_lowercase().contains(&query_lower)
                || t.url.to_lowercase().contains(&query_lower)
        })
        .cloned()
        .collect();

    let matched_threads: Vec<ComposerThreadCandidate> = threads
        .iter()
        .filter(|t| query_lower.is_empty() || t.title.to_lowercase().contains(&query_lower))
        .cloned()
        .collect();

    let matched_messages: Vec<ComposerMessageHit> = if include_messages {
        messages
            .iter()
            .filter(|m| {
                query_lower.is_empty()
                    || m.preview.to_lowercase().contains(&query_lower)
                    || m.author.to_lowercase().contains(&query_lower)
            })
            .take(THREAD_SEARCH_ROWS)
            .cloned()
            .collect()
    } else {
        Vec::new()
    };

    AtPickerResolution {
        query: trimmed.to_string(),
        at_sections,
        everyone_shown,
        messages_debounce_ms: THREAD_SEARCH_DELAY_MS,
        messages_min_query: THREAD_SEARCH_MIN_QUERY,
        agents: ranked_agents,
        files: matched_files,
        tabs: matched_tabs,
        threads: matched_threads,
        messages: matched_messages,
    }
}

/// Resolves the `$` picker, ranking ONLY `status == "ok"` skills (with duplicate-name slug
/// disambiguation) and `enabled == true` MCP servers into `dollar_items`, while collecting
/// broken skills (`status != "ok"`) into `skill_problems`.
pub fn resolve_dollar_picker(
    query: &str,
    skills: &[ComposerSkillEntry],
    mcp_servers: &[ComposerMcpEntry],
) -> DollarPickerResolution {
    let trimmed = query.trim();
    let query_lower = trimmed.to_lowercase();

    let mut ok_name_counts: HashMap<String, usize> = HashMap::new();
    for skill in skills.iter().filter(|s| s.status == "ok") {
        *ok_name_counts.entry(skill.name.to_lowercase()).or_insert(0) += 1;
    }

    let mut dollar_items: Vec<DollarPickerItem> = Vec::new();
    let mut skill_problems: Vec<SkillProblemEntry> = Vec::new();

    for skill in skills {
        if skill.status == "ok" {
            if let Some(rank) = substring_rank(&query_lower, &skill.name, &skill.slug) {
                let is_duplicate_name = ok_name_counts
                    .get(&skill.name.to_lowercase())
                    .copied()
                    .unwrap_or(0)
                    > 1;
                let display_label = if is_duplicate_name {
                    format!("{} ({})", skill.name, skill.slug)
                } else {
                    skill.name.clone()
                };
                dollar_items.push(DollarPickerItem {
                    kind: DollarPickerItemKind::Skill,
                    id: skill.slug.clone(),
                    name: skill.name.clone(),
                    display_label,
                    slug: Some(skill.slug.clone()),
                    rank,
                });
            }
        } else if query_lower.is_empty()
            || skill.name.to_lowercase().contains(&query_lower)
            || skill.slug.to_lowercase().contains(&query_lower)
        {
            skill_problems.push(SkillProblemEntry {
                slug: skill.slug.clone(),
                name: skill.name.clone(),
                status: skill.status.clone(),
                repair_hint: format!("Skill '{}' needs repair before use.", skill.slug),
            });
        }
    }

    for mcp in mcp_servers.iter().filter(|m| m.enabled) {
        if let Some(rank) = substring_rank(&query_lower, &mcp.name, &mcp.id) {
            dollar_items.push(DollarPickerItem {
                kind: DollarPickerItemKind::Mcp,
                id: mcp.id.clone(),
                name: mcp.name.clone(),
                display_label: mcp.name.clone(),
                slug: None,
                rank,
            });
        }
    }

    dollar_items.sort_by(|a, b| {
        a.rank
            .cmp(&b.rank)
            .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
            .then_with(|| a.id.cmp(&b.id))
    });

    DollarPickerResolution {
        query: trimmed.to_string(),
        dollar_items,
        skill_problems,
    }
}

/// Handles a text paste of `text_len` characters:
/// - `text_len > 4_000`: converts to an attached `pasted-text.txt` file with `inline_text_length = 0`
/// - `text_len <= 4_000`: inserts inline (capped at `100_000`).
pub fn handle_paste(text_len: usize) -> PasteResult {
    if text_len > COMPOSER_PASTED_TEXT_LIMIT {
        PasteResult {
            pasted_as_file: true,
            pasted_file_name: Some(PASTED_TEXT_FILE_NAME.to_string()),
            inline_text_length: 0,
        }
    } else {
        PasteResult {
            pasted_as_file: false,
            pasted_file_name: None,
            inline_text_length: text_len.min(COMPOSER_MESSAGE_TEXT_LIMIT),
        }
    }
}

/// Validates attachment count (`<= 10` per message, `<= 50` draft), per-file cap (`<= 100 MiB`),
/// and aggregate size cap (`<= 250 MiB`).
pub fn validate_attachments(
    attachments: &[ComposerAttachmentDraft],
    is_draft: bool,
) -> Result<(), String> {
    let max_count = if is_draft {
        COMPOSER_DRAFT_ATTACHMENTS_LIMIT
    } else {
        COMPOSER_ATTACHMENTS_LIMIT
    };
    if attachments.len() > max_count {
        return Err(format!(
            "Attachment count {} exceeds maximum limit of {} attachments.",
            attachments.len(),
            max_count
        ));
    }

    let max_file = COMPOSER_ATTACHMENT_FILE_BYTES as u64;
    let max_total = COMPOSER_ATTACHMENT_TOTAL_BYTES as u128;
    let mut total_bytes: u128 = 0;

    for att in attachments {
        if att.size_bytes > max_file {
            return Err(format!(
                "Attachment '{}' size ({} bytes) exceeds 100 MiB maximum single-file limit.",
                att.name, att.size_bytes
            ));
        }
        total_bytes = total_bytes.saturating_add(att.size_bytes as u128);
    }

    if total_bytes > max_total {
        return Err(format!(
            "Total attachment size ({} bytes) exceeds 250 MiB maximum total limit.",
            total_bytes
        ));
    }

    Ok(())
}

/// Formats a picked `@`/`$` reference into canonical OpenBot wire syntax (`@[Name](kind:id)`).
pub fn format_picker_token(kind: ChatTagKind, name: &str, id: &str) -> String {
    serialize_chat_tag_reference(kind, name, id)
}

/// Formats byte sizes cleanly for attachment chips.
pub fn format_attachment_size(bytes: u64) -> String {
    const KIB: f64 = 1024.0;
    const MIB: f64 = 1024.0 * 1024.0;
    if bytes as f64 >= MIB {
        format!("{:.1} MB", (bytes as f64) / MIB)
    } else if bytes as f64 >= KIB {
        format!("{:.1} KB", (bytes as f64) / KIB)
    } else {
        format!("{bytes} B")
    }
}

/// Tri-state primary action rendered by `ComposerSendControl`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ComposerSendMode {
    Send,
    Stop,
    LiveVoice,
}

/// Action items inside the `+` `ComposerAddMenu` popover (`UI-F12-03`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ComposerAddMenuItem {
    AttachFiles,
    PickSkill,
    TeachBrowserTask,
    AskSideQuestion,
    VoiceConversation,
    StartNewChat,
}

impl ComposerAddMenuItem {
    pub const ALL: [Self; 6] = [
        Self::AttachFiles,
        Self::PickSkill,
        Self::TeachBrowserTask,
        Self::AskSideQuestion,
        Self::VoiceConversation,
        Self::StartNewChat,
    ];

    pub const fn id(self) -> &'static str {
        match self {
            Self::AttachFiles => "attach",
            Self::PickSkill => "skill",
            Self::TeachBrowserTask => "teach",
            Self::AskSideQuestion => "side_question",
            Self::VoiceConversation => "voice",
            Self::StartNewChat => "new_chat",
        }
    }

    pub fn items_for_peer(is_bot: bool, is_group: bool) -> Vec<Self> {
        if is_bot && !is_group {
            Self::ALL.to_vec()
        } else if is_group {
            vec![
                Self::AttachFiles,
                Self::PickSkill,
                Self::AskSideQuestion,
                Self::VoiceConversation,
                Self::StartNewChat,
            ]
        } else {
            vec![Self::AttachFiles, Self::VoiceConversation]
        }
    }

    pub const fn label(self) -> &'static str {
        match self {
            Self::AttachFiles => "Attach files",
            Self::PickSkill => "Pick $ skill or MCP",
            Self::TeachBrowserTask => "Teach browser task",
            Self::AskSideQuestion => "Ask side question",
            Self::VoiceConversation => "Voice conversation",
            Self::StartNewChat => "Start new chat",
        }
    }

    pub const fn detail(self) -> &'static str {
        match self {
            Self::AttachFiles => "Up to 10 files · 100 MiB each",
            Self::PickSkill => "Insert enabled skill or MCP server",
            Self::TeachBrowserTask => "Record steps in Browser pane",
            Self::AskSideQuestion => "Ephemeral tangent · up to 20 turns",
            Self::VoiceConversation => "Start live voice conversation",
            Self::StartNewChat => "Branch a fresh thread",
        }
    }

    pub const fn icon(self) -> IconName {
        match self {
            Self::AttachFiles => IconName::Paperclip,
            Self::PickSkill => IconName::TerminalMcp,
            Self::TeachBrowserTask => IconName::ComputerDesktop,
            Self::AskSideQuestion => IconName::Search,
            Self::VoiceConversation => IconName::Mic,
            Self::StartNewChat => IconName::ComposePen,
        }
    }
}

/// Context window consumption displayed by `ContextUsageMeter`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct ContextUsageState {
    pub used_tokens: u64,
    pub window_tokens: u64,
    pub auto_compacting: bool,
}

impl Default for ContextUsageState {
    fn default() -> Self {
        Self {
            used_tokens: 0,
            window_tokens: 200_000,
            auto_compacting: false,
        }
    }
}

impl ContextUsageState {
    pub fn usage_ratio(self) -> f32 {
        if self.window_tokens == 0 {
            0.0
        } else {
            ((self.used_tokens as f64) / (self.window_tokens as f64)).clamp(0.0, 1.0) as f32
        }
    }

    pub fn usage_percent(self) -> u8 {
        (self.usage_ratio() * 100.0).round().clamp(0.0, 100.0) as u8
    }

    pub fn shows_compaction_notice(self) -> bool {
        self.auto_compacting || self.usage_ratio() >= CONTEXT_COMPACTION_THRESHOLD
    }
}

/// Full interactive state for the OpenBot rich `@`/`$` composer.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ComposerState {
    pub text: String,
    pub in_channel: bool,
    pub channel_archived_read_only: bool,
    pub active_trigger: Option<ComposerTrigger>,
    pub picker_query: String,
    pub add_menu_open: bool,
    pub send_on_enter: bool,
    pub active_turn_running: bool,
    pub live_voice_available: bool,
    pub attachments: Vec<ComposerAttachmentDraft>,
    pub custom_pi_text_only: bool,
    pub custom_endpoint_inputs: Vec<String>,
    pub image_inputs_enabled: bool,
    pub allow_images_toast: Option<String>,
    pub validation_error: Option<String>,
    pub context_usage: ContextUsageState,
    pub available_agents: Vec<(String, String)>,
    pub available_files: Vec<ComposerFileCandidate>,
    pub available_tabs: Vec<ComposerTabCandidate>,
    pub available_threads: Vec<ComposerThreadCandidate>,
    pub available_messages: Vec<ComposerMessageHit>,
    pub available_skills: Vec<ComposerSkillEntry>,
    pub available_mcp_servers: Vec<ComposerMcpEntry>,
}

impl Default for ComposerState {
    fn default() -> Self {
        Self {
            text: String::new(),
            in_channel: false,
            channel_archived_read_only: false,
            active_trigger: None,
            picker_query: String::new(),
            add_menu_open: false,
            send_on_enter: true,
            active_turn_running: false,
            live_voice_available: true,
            attachments: Vec::new(),
            custom_pi_text_only: false,
            custom_endpoint_inputs: vec!["text".to_string(), "image".to_string()],
            image_inputs_enabled: true,
            allow_images_toast: None,
            validation_error: None,
            context_usage: ContextUsageState::default(),
            available_agents: Vec::new(),
            available_files: Vec::new(),
            available_tabs: Vec::new(),
            available_threads: Vec::new(),
            available_messages: Vec::new(),
            available_skills: Vec::new(),
            available_mcp_servers: Vec::new(),
        }
    }
}

impl ComposerState {
    pub fn new(in_channel: bool) -> Self {
        Self {
            in_channel,
            ..Self::default()
        }
    }

    /// Configures whether the active agent uses a custom text-only `pi` endpoint.
    pub fn set_custom_pi_text_only(&mut self, text_only: bool) {
        self.custom_pi_text_only = text_only;
        if text_only {
            self.custom_endpoint_inputs = vec!["text".to_string()];
            self.image_inputs_enabled = false;
        } else {
            self.custom_endpoint_inputs = vec!["text".to_string(), "image".to_string()];
            self.image_inputs_enabled = true;
        }
    }

    pub fn has_image_attachment(&self) -> bool {
        self.attachments.iter().any(ComposerAttachmentDraft::is_image)
    }

    /// Returns `true` when the text-only custom `pi` endpoint image warning condition is met
    /// (`!in_channel && custom_pi_text_only && has_image_attachment`).
    pub fn image_model_warning(&self) -> bool {
        !self.in_channel && self.custom_pi_text_only && self.has_image_attachment()
    }

    /// Returns `true` while the warning banner itself is still unaddressed in the composer UI.
    pub fn image_warning_banner_visible(&self) -> bool {
        self.image_model_warning() && !self.image_inputs_enabled
    }

    /// 1-click `Allow Images` handler: patches `custom_endpoint_inputs` to include `"image"`
    /// and sets `image_inputs_enabled = true`.
    pub fn click_allow_images(&mut self) {
        if !self.custom_endpoint_inputs.iter().any(|s| s == "image") {
            self.custom_endpoint_inputs.push("image".to_string());
        }
        self.image_inputs_enabled = true;
        self.allow_images_toast =
            Some("Patched custom pi model inputs with \"image\" — ready to send.".to_string());
    }

    /// Applies paste text: pastes `> 4_000` chars attach `pasted-text.txt`, otherwise inline
    /// text is appended up to the `100_000`-character message cap.
    pub fn paste_text(&mut self, pasted: &str) -> PasteResult {
        if self.channel_archived_read_only {
            return PasteResult {
                pasted_as_file: false,
                pasted_file_name: None,
                inline_text_length: 0,
            };
        }
        let char_count = pasted.chars().count();
        let result = handle_paste(char_count);
        if result.pasted_as_file {
            let draft = ComposerAttachmentDraft::new(
                PASTED_TEXT_FILE_NAME,
                pasted.len() as u64,
                AttachmentKind::File,
            );
            if let Err(err) = self.add_attachment(draft, true) {
                self.validation_error = Some(err);
            }
        } else {
            let current_chars = self.text.chars().count();
            let remaining = COMPOSER_MESSAGE_TEXT_LIMIT.saturating_sub(current_chars);
            let appended: String = pasted.chars().take(remaining).collect();
            self.text.push_str(&appended);
        }
        result
    }

    /// Validates and adds an attachment draft to the composer.
    pub fn add_attachment(
        &mut self,
        attachment: ComposerAttachmentDraft,
        is_draft: bool,
    ) -> Result<(), String> {
        let mut next = self.attachments.clone();
        next.push(attachment);
        validate_attachments(&next, is_draft)?;
        self.attachments = next;
        self.validation_error = None;
        Ok(())
    }

    /// Validates current attachments for either draft staging (`<= 50`) or send (`<= 10`).
    pub fn validate_current_attachments(&mut self, is_draft: bool) -> Result<(), String> {
        match validate_attachments(&self.attachments, is_draft) {
            Ok(()) => {
                self.validation_error = None;
                Ok(())
            }
            Err(err) => {
                self.validation_error = Some(err.clone());
                Err(err)
            }
        }
    }

    pub fn remove_attachment(&mut self, index: usize) -> Option<ComposerAttachmentDraft> {
        if index < self.attachments.len() {
            let removed = self.attachments.remove(index);
            self.validation_error = None;
            Some(removed)
        } else {
            None
        }
    }

    pub fn open_picker(&mut self, trigger: ComposerTrigger, query: impl Into<String>) {
        self.active_trigger = Some(trigger);
        self.picker_query = query.into();
        self.add_menu_open = false;
    }

    pub fn close_picker(&mut self) {
        self.active_trigger = None;
        self.picker_query.clear();
    }

    /// Toggles the composer `+` add menu unless explicitly disabled or in an archived read-only channel (`UI-F12`).
    pub fn try_toggle_add_menu(&mut self, disabled: bool) -> bool {
        if disabled || self.channel_archived_read_only {
            self.add_menu_open = false;
            return false;
        }
        self.add_menu_open = !self.add_menu_open;
        if self.add_menu_open {
            self.active_trigger = None;
        }
        self.add_menu_open
    }

    pub fn toggle_add_menu(&mut self) {
        self.try_toggle_add_menu(false);
    }

    /// Inserts a serialized `@[Name](kind:id)` reference into `self.text` and closes the picker.
    pub fn insert_tag_token(&mut self, kind: ChatTagKind, name: &str, id: &str) -> String {
        let wire = format_picker_token(kind, name, id);
        if !self.text.is_empty() && !self.text.ends_with(char::is_whitespace) {
            self.text.push(' ');
        }
        self.text.push_str(&wire);
        self.text.push(' ');
        self.close_picker();
        wire
    }

    /// Inserts an atomic `@everyone` mention token and closes the active picker (`UI-F13-02`).
    pub fn accept_at_everyone(&mut self) -> String {
        let token = "@everyone";
        if !self.text.is_empty() && !self.text.ends_with(char::is_whitespace) {
            self.text.push(' ');
        }
        self.text.push_str(token);
        self.text.push(' ');
        self.close_picker();
        token.to_string()
    }

    /// Accepts a `$` picker item (`Skill` -> `$skill:<slug> `, `Mcp` -> `$mcp:<slug> `, or tag token)
    /// and closes the active picker (`UI-F13-03`).
    pub fn accept_dollar_item(&mut self, item: &DollarPickerItem) -> String {
        let token = match item.kind {
            DollarPickerItemKind::Skill => format!("$skill:{}", item.id),
            DollarPickerItemKind::Mcp => format!("$mcp:{}", item.id),
        };
        if !self.text.is_empty() && !self.text.ends_with(char::is_whitespace) {
            self.text.push(' ');
        }
        self.text.push_str(&token);
        self.text.push(' ');
        self.close_picker();
        token
    }

    pub fn resolve_active_at_picker(&self) -> AtPickerResolution {
        resolve_at_picker(
            &self.picker_query,
            self.in_channel && self.available_agents.len() > 1,
            &self.available_agents,
            &self.available_files,
            &self.available_tabs,
            &self.available_threads,
            &self.available_messages,
        )
    }

    pub fn resolve_active_dollar_picker(&self) -> DollarPickerResolution {
        resolve_dollar_picker(
            &self.picker_query,
            &self.available_skills,
            &self.available_mcp_servers,
        )
    }

    /// Computes the tri-state control (`Send` / `Stop` / `LiveVoice`) for the send button.
    pub fn send_mode(&self) -> ComposerSendMode {
        let has_payload = !self.text.trim().is_empty() || !self.attachments.is_empty();
        if self.active_turn_running && !has_payload {
            ComposerSendMode::Stop
        } else if has_payload {
            ComposerSendMode::Send
        } else if self.live_voice_available {
            ComposerSendMode::LiveVoice
        } else {
            ComposerSendMode::Send
        }
    }

    pub fn can_send(&self) -> bool {
        if self.channel_archived_read_only {
            return false;
        }
        let has_payload = !self.text.trim().is_empty() || !self.attachments.is_empty();
        has_payload && validate_attachments(&self.attachments, false).is_ok()
    }
}

fn palette_color(palette: &OpenBotPalette, token: &str, fallback: u32) -> Rgba {
    palette.rgba(token).unwrap_or_else(|| rgb(fallback))
}

fn render_context_usage_ring(usage: ContextUsageState, palette: &OpenBotPalette) -> Div {
    let ratio = usage.usage_ratio();
    let track_color = palette_color(palette, "--openbot-border-strong", 0x3a3a3c);
    let fill_color = if usage.shows_compaction_notice() {
        palette_color(palette, "--openbot-warning", 0xff9412)
    } else {
        palette_color(palette, "--openbot-accent", 0x007cf7)
    };

    div().w(px(18.0)).h(px(18.0)).flex_shrink_0().child(
        canvas(
            |_, _, _| {},
            move |bounds, _, window, _cx| {
                let cx = f32::from(bounds.origin.x) + f32::from(bounds.size.width) * 0.5;
                let cy = f32::from(bounds.origin.y) + f32::from(bounds.size.height) * 0.5;
                let radius = (f32::from(bounds.size.width).min(f32::from(bounds.size.height)) * 0.5
                    - 2.0)
                    .max(2.0);
                let steps = 28usize;

                let mut track = PathBuilder::stroke(px(2.0));
                for i in 0..=steps {
                    let angle = -std::f32::consts::FRAC_PI_2
                        + (i as f32 / steps as f32) * std::f32::consts::TAU;
                    let pt = point(px(cx + radius * angle.cos()), px(cy + radius * angle.sin()));
                    if i == 0 {
                        track.move_to(pt);
                    } else {
                        track.line_to(pt);
                    }
                }
                if let Ok(path) = track.build() {
                    window.paint_path(path, track_color);
                }

                let arc_steps = ((steps as f32) * ratio).ceil() as usize;
                if arc_steps >= 1 {
                    let mut active = PathBuilder::stroke(px(2.2));
                    for i in 0..=arc_steps {
                        let frac = (i as f32 / steps as f32).min(ratio);
                        let angle = -std::f32::consts::FRAC_PI_2 + frac * std::f32::consts::TAU;
                        let pt =
                            point(px(cx + radius * angle.cos()), px(cy + radius * angle.sin()));
                        if i == 0 {
                            active.move_to(pt);
                        } else {
                            active.line_to(pt);
                        }
                    }
                    if let Ok(path) = active.build() {
                        window.paint_path(path, fill_color);
                    }
                }
            },
        )
        .size_full(),
    )
}

fn render_picker_popover(
    state: &ComposerState,
    trigger: ComposerTrigger,
    palette: &OpenBotPalette,
) -> AnyElement {
    let surface = palette_color(palette, "--openbot-bg-surface-raised", 0x242424);
    let border = palette_color(palette, "--openbot-dialog-ring", 0x3a3a3c);
    let hover_bg = palette_color(palette, "--openbot-hover", 0x2e2e30);
    let text_primary = palette_color(palette, "--openbot-text-primary", 0xffffff);
    let text_muted = palette_color(palette, "--openbot-text-muted", 0x979797);
    let accent = palette_color(palette, "--openbot-accent", 0x007cf7);
    let accent_soft = palette_color(palette, "--openbot-accent-soft", 0x1a304d);
    let warning = palette_color(palette, "--openbot-warning", 0xff9412);
    let warning_soft = palette_color(palette, "--openbot-warning-soft", 0x3a2b16);

    let mut body = div()
        .id("openbot-composer-picker-scroll")
        .flex()
        .flex_col()
        .gap(px(4.0))
        .max_h(px(260.0))
        .min_h(px(0.0))
        .overflow_y_scroll();

    match trigger {
        ComposerTrigger::At => {
            let resolved = state.resolve_active_at_picker();
            body = body.child(
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .px(px(8.0))
                    .py(px(4.0))
                    .child(
                        div()
                            .text_xs()
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(text_muted)
                            .child(format!(
                                "@ Mention · {} sections",
                                resolved.at_sections.len()
                            )),
                    )
                    .child(
                        div()
                            .text_xs()
                            .text_color(text_muted)
                            .child(format!("Messages ≥{} chars ({}ms)", THREAD_SEARCH_MIN_QUERY, THREAD_SEARCH_DELAY_MS)),
                    ),
            );

            for section in &resolved.at_sections {
                body = body.child(
                    div()
                        .px(px(8.0))
                        .pt(px(4.0))
                        .text_xs()
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(text_muted)
                        .child(section.to_uppercase()),
                );
                match section.as_str() {
                    "agents" => {
                        for (idx, ag) in resolved.agents.iter().enumerate() {
                            body = body.child(
                                div()
                                    .id(("openbot-at-agent", idx))
                                    .flex()
                                    .items_center()
                                    .justify_between()
                                    .px(px(8.0))
                                    .py(px(5.0))
                                    .rounded(px(8.0))
                                    .hover(move |s| s.bg(hover_bg))
                                    .child(
                                        div()
                                            .flex()
                                            .items_center()
                                            .gap(px(6.0))
                                            .child(tg_icon_rgba(IconName::BotRobot, 13.0, accent))
                                            .child(
                                                div()
                                                    .text_sm()
                                                    .text_color(text_primary)
                                                    .child(ag.name.clone()),
                                            ),
                                    )
                                    .when(ag.is_everyone, |el| {
                                        el.child(
                                            div()
                                                .px(px(6.0))
                                                .py(px(1.0))
                                                .rounded(px(999.0))
                                                .bg(accent_soft)
                                                .text_xs()
                                                .text_color(accent)
                                                .child("All members"),
                                        )
                                    }),
                            );
                        }
                    }
                    "files" => {
                        for (idx, file) in resolved.files.iter().enumerate() {
                            let tone = file_reference_tone(&file.name);
                            let (fg_hex, _) = tone.colors(palette.scheme);
                            let fg = parse_css_color_to_rgba(fg_hex).unwrap_or(accent);
                            body = body.child(
                                div()
                                    .id(("openbot-at-file", idx))
                                    .flex()
                                    .items_center()
                                    .gap(px(6.0))
                                    .px(px(8.0))
                                    .py(px(4.0))
                                    .rounded(px(8.0))
                                    .hover(move |s| s.bg(hover_bg))
                                    .child(tg_icon_rgba(IconName::FileDoc, 13.0, fg))
                                    .child(
                                        div()
                                            .text_sm()
                                            .text_color(text_primary)
                                            .truncate()
                                            .child(file.name.clone()),
                                    ),
                            );
                        }
                    }
                    "tabs" => {
                        for (idx, tab) in resolved.tabs.iter().enumerate() {
                            body = body.child(
                                div()
                                    .id(("openbot-at-tab", idx))
                                    .flex()
                                    .items_center()
                                    .gap(px(6.0))
                                    .px(px(8.0))
                                    .py(px(4.0))
                                    .rounded(px(8.0))
                                    .hover(move |s| s.bg(hover_bg))
                                    .child(tg_icon_rgba(IconName::ComputerDesktop, 13.0, text_muted))
                                    .child(
                                        div()
                                            .text_sm()
                                            .text_color(text_primary)
                                            .truncate()
                                            .child(tab.title.clone()),
                                    ),
                            );
                        }
                    }
                    "threads" => {
                        for (idx, th) in resolved.threads.iter().enumerate() {
                            body = body.child(
                                div()
                                    .id(("openbot-at-thread", idx))
                                    .flex()
                                    .items_center()
                                    .gap(px(6.0))
                                    .px(px(8.0))
                                    .py(px(4.0))
                                    .rounded(px(8.0))
                                    .hover(move |s| s.bg(hover_bg))
                                    .child(tg_icon_rgba(IconName::Reply, 13.0, text_muted))
                                    .child(
                                        div()
                                            .text_sm()
                                            .text_color(text_primary)
                                            .truncate()
                                            .child(th.title.clone()),
                                    ),
                            );
                        }
                    }
                    "messages" => {
                        for (idx, hit) in resolved.messages.iter().enumerate() {
                            body = body.child(
                                div()
                                    .id(("openbot-at-msg", idx))
                                    .flex()
                                    .flex_col()
                                    .px(px(8.0))
                                    .py(px(4.0))
                                    .rounded(px(8.0))
                                    .hover(move |s| s.bg(hover_bg))
                                    .child(
                                        div()
                                            .text_xs()
                                            .font_weight(FontWeight::SEMIBOLD)
                                            .text_color(text_muted)
                                            .child(hit.author.clone()),
                                    )
                                    .child(
                                        div()
                                            .text_sm()
                                            .text_color(text_primary)
                                            .truncate()
                                            .child(hit.preview.clone()),
                                    ),
                            );
                        }
                    }
                    _ => {}
                }
            }
        }
        ComposerTrigger::Dollar => {
            let resolved = state.resolve_active_dollar_picker();
            body = body.child(
                div()
                    .px(px(8.0))
                    .py(px(4.0))
                    .text_xs()
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(text_muted)
                    .child(format!(
                        "$ Skills & MCP Servers · {} available",
                        resolved.dollar_items.len()
                    )),
            );
            for (idx, item) in resolved.dollar_items.iter().enumerate() {
                let badge = item.kind.as_str().to_uppercase();
                body = body.child(
                    div()
                        .id(("openbot-dollar-item", idx))
                        .flex()
                        .items_center()
                        .justify_between()
                        .px(px(8.0))
                        .py(px(5.0))
                        .rounded(px(8.0))
                        .hover(move |s| s.bg(hover_bg))
                        .child(
                            div()
                                .flex()
                                .items_center()
                                .gap(px(6.0))
                                .child(tg_icon_rgba(IconName::TerminalMcp, 13.0, accent))
                                .child(
                                    div()
                                        .text_sm()
                                        .text_color(text_primary)
                                        .child(item.display_label.clone()),
                                ),
                        )
                        .child(
                            div()
                                .px(px(6.0))
                                .py(px(1.0))
                                .rounded(px(999.0))
                                .bg(accent_soft)
                                .text_xs()
                                .text_color(accent)
                                .child(badge),
                        ),
                );
            }
            if !resolved.skill_problems.is_empty() {
                body = body.child(
                    div()
                        .px(px(8.0))
                        .pt(px(6.0))
                        .text_xs()
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(warning)
                        .child("SKILLS NEEDING REPAIR"),
                );
                for (idx, problem) in resolved.skill_problems.iter().enumerate() {
                    body = body.child(
                        div()
                            .id(("openbot-skill-problem", idx))
                            .flex()
                            .items_center()
                            .justify_between()
                            .px(px(8.0))
                            .py(px(5.0))
                            .rounded(px(8.0))
                            .bg(warning_soft)
                            .child(
                                div()
                                    .flex()
                                    .flex_col()
                                    .child(
                                        div()
                                            .text_sm()
                                            .text_color(warning)
                                            .child(format!("{} ({})", problem.name, problem.slug)),
                                    )
                                    .child(
                                        div()
                                            .text_xs()
                                            .text_color(text_muted)
                                            .child(problem.repair_hint.clone()),
                                    ),
                            )
                            .child(
                                div()
                                    .text_xs()
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .text_color(warning)
                                    .child(problem.status.clone()),
                            ),
                    );
                }
            }
        }
    }

    let panel = div()
        .absolute()
        .bottom(px(56.0))
        .left(px(0.0))
        .w(px(360.0))
        .p(px(6.0))
        .rounded(px(14.0))
        .bg(surface)
        .border_1()
        .border_color(border)
        .occlude()
        .child(body);

    deferred(panel).with_priority(50).into_any_element()
}

fn render_add_menu_popover(palette: &OpenBotPalette) -> AnyElement {
    let surface = palette_color(palette, "--openbot-bg-surface-raised", 0x242424);
    let border = palette_color(palette, "--openbot-dialog-ring", 0x3a3a3c);
    let hover_bg = palette_color(palette, "--openbot-hover", 0x2e2e30);
    let text_primary = palette_color(palette, "--openbot-text-primary", 0xffffff);
    let text_muted = palette_color(palette, "--openbot-text-muted", 0x979797);
    let accent = palette_color(palette, "--openbot-accent", 0x007cf7);

    let mut menu = div()
        .absolute()
        .bottom(px(56.0))
        .left(px(0.0))
        .w(px(268.0))
        .p(px(6.0))
        .rounded(px(14.0))
        .bg(surface)
        .border_1()
        .border_color(border)
        .occlude()
        .flex()
        .flex_col()
        .gap(px(2.0));

    for (idx, item) in ComposerAddMenuItem::ALL.iter().copied().enumerate() {
        menu = menu.child(
            div()
                .id(("openbot-composer-add-item", idx))
                .flex()
                .items_center()
                .gap(px(8.0))
                .px(px(8.0))
                .py(px(6.0))
                .rounded(px(8.0))
                .hover(move |s| s.bg(hover_bg))
                .child(tg_icon_rgba(item.icon(), 14.0, accent))
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .min_w(px(0.0))
                        .child(
                            div()
                                .text_sm()
                                .font_weight(FontWeight::MEDIUM)
                                .text_color(text_primary)
                                .child(item.label()),
                        )
                        .child(
                            div()
                                .text_xs()
                                .text_color(text_muted)
                                .child(item.detail()),
                        ),
                ),
        );
    }

    deferred(menu).with_priority(52).into_any_element()
}

fn render_attachment_chip(
    att: &ComposerAttachmentDraft,
    idx: usize,
    palette: &OpenBotPalette,
) -> Stateful<Div> {
    let tone = file_reference_tone(&att.name);
    let (fg_hex, bg_css) = tone.colors(palette.scheme);
    let fg = parse_css_color_to_rgba(fg_hex)
        .unwrap_or_else(|| palette_color(palette, "--openbot-accent-text", 0x42a0ff));
    let bg = parse_css_color_to_rgba(bg_css)
        .unwrap_or_else(|| palette_color(palette, "--openbot-accent-soft", 0x1a304d));
    let text_primary = palette_color(palette, "--openbot-text-primary", 0xffffff);
    let text_muted = palette_color(palette, "--openbot-text-muted", 0x979797);
    let badge = file_reference_badge(&att.name).unwrap_or_else(|| {
        if att.is_image() {
            "IMG".to_string()
        } else {
            "FILE".to_string()
        }
    });

    div()
        .id(("openbot-composer-att", idx))
        .flex()
        .items_center()
        .gap(px(6.0))
        .px(px(8.0))
        .py(px(4.0))
        .rounded(px(10.0))
        .bg(bg)
        .border_1()
        .border_color(fg)
        .child(
            div()
                .px(px(5.0))
                .py(px(1.0))
                .rounded(px(5.0))
                .bg(fg)
                .text_xs()
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(palette_color(palette, "--openbot-text-on-accent", 0xffffff))
                .child(badge),
        )
        .child(
            div()
                .text_xs()
                .font_weight(FontWeight::MEDIUM)
                .text_color(text_primary)
                .max_w(px(150.0))
                .truncate()
                .child(att.name.clone()),
        )
        .child(
            div()
                .text_xs()
                .text_color(text_muted)
                .child(format_attachment_size(att.size_bytes)),
        )
        .child(tg_icon_rgba(IconName::Close, 11.0, text_muted))
}

/// Renders the complete OpenBot rich `@`/`$` composer with deferred popovers, extension-toned
/// attachment chips, text-only custom `pi` image warning banner, `ContextUsageMeter` (with 80%
/// auto-compaction notice), and tri-state `ComposerSendControl` (`Send` / `Stop` / `LiveVoice`).
pub fn render_composer(state: &ComposerState, palette: &OpenBotPalette) -> Div {
    let surface = palette_color(palette, "--openbot-bg-surface", 0x212121);
    let control_bg = palette_color(palette, "--openbot-bg-control", 0x212121);
    let control_hover = palette_color(palette, "--openbot-bg-control-hover", 0x242424);
    let border = palette_color(palette, "--openbot-border-strong", 0x333335);
    let text_primary = palette_color(palette, "--openbot-text-primary", 0xffffff);
    let text_muted = palette_color(palette, "--openbot-text-muted", 0x979797);
    let text_placeholder = palette_color(palette, "--openbot-text-placeholder", 0xbfbfbf);
    let accent = palette_color(palette, "--openbot-accent", 0x007cf7);
    let text_on_accent = palette_color(palette, "--openbot-text-on-accent", 0xffffff);
    let warning = palette_color(palette, "--openbot-warning", 0xff9412);
    let warning_soft = palette_color(palette, "--openbot-warning-soft", 0x3a2b16);
    let danger = palette_color(palette, "--openbot-danger", 0xfd2f3b);
    let danger_soft = palette_color(palette, "--openbot-danger-soft", 0x3d191c);
    let success = palette_color(palette, "--openbot-success", 0x31cf76);
    let success_soft = palette_color(palette, "--openbot-success-soft", 0x163222);

    let placeholder = if state.channel_archived_read_only {
        "Channel is archived (read-only)"
    } else if state.in_channel {
        "Message channel · @ to mention · $ for skills & MCP"
    } else {
        "Message agent · @ to reference · $ for skills & MCP"
    };

    let send_mode = state.send_mode();
    let send_bg = match send_mode {
        ComposerSendMode::Send => accent,
        ComposerSendMode::Stop => danger,
        ComposerSendMode::LiveVoice => control_hover,
    };
    let send_fg = match send_mode {
        ComposerSendMode::Send | ComposerSendMode::Stop => text_on_accent,
        ComposerSendMode::LiveVoice => text_primary,
    };
    let send_icon = match send_mode {
        ComposerSendMode::Send => IconName::ArrowUp,
        ComposerSendMode::Stop => IconName::Close,
        ComposerSendMode::LiveVoice => IconName::Mic,
    };
    let send_label = match send_mode {
        ComposerSendMode::Send => "Send",
        ComposerSendMode::Stop => "Stop",
        ComposerSendMode::LiveVoice => "Voice",
    };

    let mut root = div()
        .relative()
        .w_full()
        .flex()
        .flex_col()
        .gap(px(8.0))
        .p(px(10.0))
        .rounded(px(20.0))
        .bg(surface)
        .border_1()
        .border_color(border);

    if let Some(trigger) = state.active_trigger {
        root = root.child(render_picker_popover(state, trigger, palette));
    } else if state.add_menu_open && !state.channel_archived_read_only {
        root = root.child(render_add_menu_popover(palette));
    }

    if state.image_warning_banner_visible() {
        root = root.child(
            div()
                .flex()
                .items_center()
                .justify_between()
                .gap(px(8.0))
                .px(px(10.0))
                .py(px(7.0))
                .rounded(px(12.0))
                .bg(warning_soft)
                .border_1()
                .border_color(warning)
                .child(
                    div()
                        .text_xs()
                        .text_color(warning)
                        .child("Custom pi model is configured as text-only; image attachments require enabling image inputs."),
                )
                .child(
                    div()
                        .id("openbot-composer-allow-images")
                        .px(px(10.0))
                        .py(px(4.0))
                        .rounded(px(999.0))
                        .bg(warning)
                        .text_xs()
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(text_on_accent)
                        .child("Allow Images"),
                ),
        );
    } else if let Some(toast) = state.allow_images_toast.as_ref() {
        root = root.child(
            div()
                .px(px(10.0))
                .py(px(5.0))
                .rounded(px(10.0))
                .bg(success_soft)
                .text_xs()
                .text_color(success)
                .child(toast.clone()),
        );
    }

    if let Some(err) = state.validation_error.as_ref() {
        root = root.child(
            div()
                .px(px(10.0))
                .py(px(6.0))
                .rounded(px(10.0))
                .bg(danger_soft)
                .border_1()
                .border_color(danger)
                .text_xs()
                .text_color(danger)
                .child(err.clone()),
        );
    }

    if !state.attachments.is_empty() {
        let mut strip = div()
            .id("openbot-composer-attachments")
            .flex()
            .items_center()
            .gap(px(6.0))
            .min_w(px(0.0))
            .overflow_x_scroll();
        for (idx, att) in state.attachments.iter().enumerate() {
            strip = strip.child(render_attachment_chip(att, idx, palette));
        }
        root = root.child(strip);
    }

    let body_text = if state.text.is_empty() {
        div()
            .text_sm()
            .text_color(text_placeholder)
            .child(placeholder)
    } else {
        div()
            .text_sm()
            .text_color(text_primary)
            .child(state.text.clone())
    };

    root = root
        .child(div().min_h(px(36.0)).px(px(4.0)).py(px(2.0)).child(body_text))
        .child(
            div()
                .flex()
                .items_center()
                .justify_between()
                .gap(px(8.0))
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(6.0))
                        .child(
                            div()
                                .id("openbot-composer-add-btn")
                                .w(px(28.0))
                                .h(px(28.0))
                                .rounded(px(999.0))
                                .bg(control_bg)
                                .border_1()
                                .border_color(border)
                                .flex()
                                .items_center()
                                .justify_center()
                                .text_sm()
                                .font_weight(FontWeight::SEMIBOLD)
                                .text_color(text_primary)
                                .when(state.channel_archived_read_only, |el| el.opacity(0.4))
                                .when(!state.channel_archived_read_only, |el| {
                                    el.hover(move |s| s.bg(control_hover))
                                })
                                .child("+"),
                        )
                        .child(
                            div()
                                .id("openbot-composer-at-trigger")
                                .px(px(8.0))
                                .py(px(3.0))
                                .rounded(px(999.0))
                                .bg(control_bg)
                                .border_1()
                                .border_color(border)
                                .text_xs()
                                .font_weight(FontWeight::MEDIUM)
                                .text_color(text_muted)
                                .hover(move |s| s.bg(control_hover))
                                .child("@"),
                        )
                        .child(
                            div()
                                .id("openbot-composer-dollar-trigger")
                                .px(px(8.0))
                                .py(px(3.0))
                                .rounded(px(999.0))
                                .bg(control_bg)
                                .border_1()
                                .border_color(border)
                                .text_xs()
                                .font_weight(FontWeight::MEDIUM)
                                .text_color(text_muted)
                                .hover(move |s| s.bg(control_hover))
                                .child("$"),
                        )
                        .when(!state.attachments.is_empty(), |el| {
                            el.child(
                                div()
                                    .text_xs()
                                    .text_color(text_muted)
                                    .child(format!(
                                        "{}/{} files",
                                        state.attachments.len(),
                                        COMPOSER_ATTACHMENTS_LIMIT
                                    )),
                            )
                        }),
                )
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(8.0))
                        .child(
                            div()
                                .flex()
                                .items_center()
                                .gap(px(5.0))
                                .child(render_context_usage_ring(state.context_usage, palette))
                                .child(
                                    div()
                                        .text_xs()
                                        .text_color(if state.context_usage.shows_compaction_notice() {
                                            warning
                                        } else {
                                            text_muted
                                        })
                                        .child(if state.context_usage.shows_compaction_notice() {
                                            format!(
                                                "{}% · Auto-compacting",
                                                state.context_usage.usage_percent()
                                            )
                                        } else {
                                            format!("{}%", state.context_usage.usage_percent())
                                        }),
                                ),
                        )
                        .child(
                            div()
                                .text_xs()
                                .text_color(text_muted)
                                .child(if state.send_on_enter {
                                    "Enter"
                                } else {
                                    "Cmd+Enter"
                                }),
                        )
                        .child(
                            div()
                                .id("openbot-composer-send-control")
                                .flex()
                                .items_center()
                                .gap(px(5.0))
                                .px(px(10.0))
                                .h(px(30.0))
                                .rounded(px(999.0))
                                .bg(send_bg)
                                .text_xs()
                                .font_weight(FontWeight::SEMIBOLD)
                                .text_color(send_fg)
                                .child(tg_icon_rgba(send_icon, 13.0, send_fg))
                                .child(send_label),
                        ),
                ),
        );

    root
}

/// Interactive UI action emitted by [`render_composer_interactive`] when mounted in GPUI.
#[derive(Debug, Clone)]
pub enum ComposerUiAction {
    AllowImages,
    ToggleAddMenu,
    AcceptAtEveryone,
    InsertTag {
        kind: ChatTagKind,
        name: String,
        id: String,
    },
    AcceptDollar(DollarPickerItem),
    OpenTrigger(ComposerTrigger),
}

/// Live interactive GPUI composer surface wiring `click_allow_images`, `try_toggle_add_menu`,
/// `accept_at_everyone`, `insert_tag_token`, and `accept_dollar_item` to real click handlers.
pub fn render_composer_interactive<V: 'static>(
    state: &ComposerState,
    palette: &OpenBotPalette,
    cx: &mut Context<V>,
    on_action: impl Fn(&mut V, ComposerUiAction, &mut Context<V>) + Clone + 'static,
) -> Div {
    let surface = palette_color(palette, "--openbot-bg-surface", 0x1a1a1d);
    let border = palette_color(palette, "--openbot-border", 0x222222);
    let control_bg = palette_color(palette, "--openbot-bg-control", 0x222225);
    let control_hover = palette_color(palette, "--openbot-hover", 0x2a2a2e);
    let text_primary = palette_color(palette, "--openbot-text-primary", 0xffffff);
    let text_muted = palette_color(palette, "--openbot-text-muted", 0x979797);
    let warning = palette_color(palette, "--openbot-warning", 0xff9412);
    let warning_soft = palette_color(palette, "--openbot-warning-soft", 0x3a2810);
    let text_on_accent = palette_color(palette, "--openbot-text-on-accent", 0xffffff);

    let mut root = render_composer(state, palette);

    if state.image_warning_banner_visible() {
        let act = on_action.clone();
        root = root.child(
            div()
                .flex()
                .items_center()
                .justify_end()
                .child(
                    div()
                        .id("openbot-composer-allow-images-live")
                        .cursor_pointer()
                        .px(px(10.0))
                        .py(px(4.0))
                        .rounded(px(999.0))
                        .bg(warning_soft)
                        .border_1()
                        .border_color(warning)
                        .text_xs()
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(text_on_accent)
                        .on_click(cx.listener(move |this, _, _, cx| {
                            act(this, ComposerUiAction::AllowImages, cx);
                        }))
                        .child("Allow Images"),
                ),
        );
    }

    if let Some(trigger) = state.active_trigger {
        let mut quick_bar = div()
            .flex()
            .items_center()
            .flex_wrap()
            .gap(px(4.0))
            .px(px(4.0))
            .py(px(2.0))
            .bg(surface)
            .border_t_1()
            .border_color(border);
        match trigger {
            ComposerTrigger::At => {
                let resolved = state.resolve_active_at_picker();
                for (idx, ag) in resolved.agents.iter().take(4).enumerate() {
                    let act = on_action.clone();
                    let is_ev = ag.is_everyone;
                    let ag_id = ag.id.clone();
                    let ag_name = ag.name.clone();
                    quick_bar = quick_bar.child(
                        div()
                            .id(("openbot-composer-pick-at", idx))
                            .cursor_pointer()
                            .px(px(8.0))
                            .py(px(2.0))
                            .rounded(px(999.0))
                            .bg(control_bg)
                            .hover(move |s| s.bg(control_hover))
                            .text_xs()
                            .text_color(text_primary)
                            .on_click(cx.listener(move |this, _, _, cx| {
                                if is_ev {
                                    act(this, ComposerUiAction::AcceptAtEveryone, cx);
                                } else {
                                    act(
                                        this,
                                        ComposerUiAction::InsertTag {
                                            kind: ChatTagKind::Agent,
                                            name: ag_name.clone(),
                                            id: ag_id.clone(),
                                        },
                                        cx,
                                    );
                                }
                            }))
                            .child(ag.name.clone()),
                    );
                }
            }
            ComposerTrigger::Dollar => {
                let resolved = state.resolve_active_dollar_picker();
                for (idx, item) in resolved.dollar_items.iter().take(4).enumerate() {
                    let act = on_action.clone();
                    let cloned = item.clone();
                    quick_bar = quick_bar.child(
                        div()
                            .id(("openbot-composer-pick-dollar", idx))
                            .cursor_pointer()
                            .px(px(8.0))
                            .py(px(2.0))
                            .rounded(px(999.0))
                            .bg(control_bg)
                            .hover(move |s| s.bg(control_hover))
                            .text_xs()
                            .text_color(text_primary)
                            .on_click(cx.listener(move |this, _, _, cx| {
                                act(this, ComposerUiAction::AcceptDollar(cloned.clone()), cx);
                            }))
                            .child(item.display_label.clone()),
                    );
                }
            }
        }
        root = root.child(quick_bar);
    }

    let act_add = on_action.clone();
    let act_at = on_action.clone();
    let act_dollar = on_action;
    let disabled = state.channel_archived_read_only;
    root = root.child(
        div()
            .flex()
            .items_center()
            .gap(px(6.0))
            .px(px(4.0))
            .child(
                div()
                    .id("openbot-composer-live-add-toggle")
                    .cursor_pointer()
                    .px(px(7.0))
                    .py(px(2.0))
                    .rounded(px(999.0))
                    .bg(control_bg)
                    .border_1()
                    .border_color(border)
                    .text_xs()
                    .text_color(text_muted)
                    .when(disabled, |el| el.opacity(0.4))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        act_add(this, ComposerUiAction::ToggleAddMenu, cx);
                    }))
                    .child("+ Menu"),
            )
            .child(
                div()
                    .id("openbot-composer-live-at-btn")
                    .cursor_pointer()
                    .px(px(7.0))
                    .py(px(2.0))
                    .rounded(px(999.0))
                    .bg(control_bg)
                    .text_xs()
                    .text_color(text_muted)
                    .on_click(cx.listener(move |this, _, _, cx| {
                        act_at(this, ComposerUiAction::OpenTrigger(ComposerTrigger::At), cx);
                    }))
                    .child("@"),
            )
            .child(
                div()
                    .id("openbot-composer-live-dollar-btn")
                    .cursor_pointer()
                    .px(px(7.0))
                    .py(px(2.0))
                    .rounded(px(999.0))
                    .bg(control_bg)
                    .text_xs()
                    .text_color(text_muted)
                    .on_click(cx.listener(move |this, _, _, cx| {
                        act_dollar(
                            this,
                            ComposerUiAction::OpenTrigger(ComposerTrigger::Dollar),
                            cx,
                        );
                    }))
                    .child("$"),
            ),
    );

    root
}
