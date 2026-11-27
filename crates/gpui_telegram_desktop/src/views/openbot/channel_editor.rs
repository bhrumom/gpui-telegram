//! 1:1 OpenBot `ChannelEditor`, `ChannelCreateDialog`, `ChannelPictureEditor`, and `ChannelSettingsPane` (`OB-F17`).
//! Mirrors `ChannelCreateDialog.tsx`, `ChannelEditor.tsx`, `ChannelPictureEditor.tsx`,
//! `ChannelSettingsPane.tsx`, and `channel-icons.ts`.

use std::collections::HashSet;
use std::fmt;

use gpui::prelude::FluentBuilder;
use gpui::*;
use openbot_contracts::avatar::{AVATAR_HUES, AvatarHue, AvatarMood, avatar_head_color};
use openbot_contracts::channels::{
    CHANNEL_FOLDER_LIMIT, CHANNEL_ICON_NAMES, Channel, ChannelBrowser, ChannelDraft, ChannelIcon,
    ChannelIconName, ChannelMember, with_browser_member,
};
use openbot_contracts::limits::{INPUT_LIMITS, is_identifier, utf16_len};
use openbot_contracts::mentions::{MentionableAgent, rank_agent_mentions};
use serde::{Deserialize, Serialize};

use crate::components::bloub_avatar::{
    AgentAvatarProps, AvatarNormalizeError, CHANNEL_PICTURE_TRIGGER_SIZE_PX, ChannelAgentBrief,
    ChannelAvatarLayout, ChannelPictureEditorState, NormalizedAvatarOutput, render_agent_avatar,
    render_channel_avatar,
};
use crate::theme::openbot_palette::{
    OpenBotColorScheme, OpenBotPalette, parse_css_color_to_rgba,
};
use crate::theme::{IconName, tg_icon_rgba};

/// Exact canonical channel icon catalog cardinality (`channel-icons.ts:L26-L60`).
pub const ICON_CATALOG_COUNT: usize = 20;
/// Canonical 20 channel icon slugs in OpenBot catalog order.
pub const CHANNEL_ICON_CATALOG: [&str; ICON_CATALOG_COUNT] = CHANNEL_ICON_NAMES;
/// Canonical 10 avatar/channel icon hues (`0, 30, 55, 100, 150, 185, 215, 245, 280, 320`).
pub const CHANNEL_ICON_HUES: [AvatarHue; 10] = AVATAR_HUES;
/// Default normalized channel icon hue in degrees (`AvatarHue::Blue = 215`).
pub const DEFAULT_CHANNEL_ICON_HUE: u16 = 215;
/// Default normalized channel icon hue enum value (`215`).
pub const DEFAULT_CHANNEL_ICON_AVATAR_HUE: AvatarHue = AvatarHue::Blue;

/// Minimum trimmed UTF-16 length for channel name.
pub const CHANNEL_NAME_MIN_LEN: usize = 1;
/// Maximum UTF-16 length for channel name (`80`).
pub const CHANNEL_NAME_MAX_LEN: usize = INPUT_LIMITS.agent_name;
/// Maximum UTF-16 length for channel title (`120`).
pub const CHANNEL_TITLE_MAX_LEN: usize = INPUT_LIMITS.agent_title;
/// Maximum UTF-16 length for modified/new channel instructions (`2_000`).
pub const CHANNEL_INSTRUCTIONS_MAX_LEN: usize = INPUT_LIMITS.agent_description;
/// Maximum UTF-16 length for unchanged stored channel instructions (`32_000`).
pub const CHANNEL_STORED_INSTRUCTIONS_MAX_LEN: usize = INPUT_LIMITS.stored_channel_instructions;
/// Minimum channel members required to create/save a channel (`1`).
pub const CHANNEL_MEMBERS_MIN: usize = 1;
/// Maximum channel members permitted in a single channel (`100`).
pub const CHANNEL_MEMBERS_MAX: usize = INPUT_LIMITS.agents;
/// Maximum channel-scoped memories (`32`).
pub const CHANNEL_MEMORIES_MAX: usize = INPUT_LIMITS.channel_memories;
/// Maximum UTF-16 length of a single channel memory entry (`500`).
pub const CHANNEL_MEMORY_TEXT_MAX: usize = INPUT_LIMITS.agent_memory_text;
/// Maximum linked folders on a channel (`8`).
pub const CHANNEL_FOLDERS_MAX: usize = CHANNEL_FOLDER_LIMIT;
/// Members inside `ChannelEditor` stay strictly in insertion order (drag-reorder is unsupported).
pub const MEMBER_DRAG_REORDER_SUPPORTED: bool = false;

/// Resolves a channel icon `(name, hue)` pair against the 20-icon catalog and 10-hue palette.
///
/// - Unknown icon names (e.g. `"dragon"`) return `None`.
/// - Known icon names with `None` or an out-of-palette hue (e.g. `Some(999)`) normalize hue to
///   `AvatarHue::Blue` (`215`).
pub fn resolve_channel_icon(name: &str, hue: Option<u16>) -> Option<(ChannelIconName, AvatarHue)> {
    let normalized_name = name.trim().to_ascii_lowercase();
    let icon_name = ChannelIconName::parse(&normalized_name)?;
    let resolved_hue = hue
        .and_then(AvatarHue::from_degrees)
        .unwrap_or(DEFAULT_CHANNEL_ICON_AVATAR_HUE);
    Some((icon_name, resolved_hue))
}

/// Resolves an optional contract [`ChannelIcon`] using [`resolve_channel_icon`].
pub fn resolve_channel_icon_struct(icon: Option<&ChannelIcon>) -> Option<(ChannelIconName, AvatarHue)> {
    let icon = icon?;
    resolve_channel_icon(&icon.name, Some(icon.hue))
}

/// Converts a resolved `(ChannelIconName, AvatarHue)` into a contract [`ChannelIcon`].
pub fn channel_icon_from_resolved(name: ChannelIconName, hue: AvatarHue) -> ChannelIcon {
    ChannelIcon {
        name: name.as_str().to_string(),
        hue: hue.degrees(),
    }
}

/// Candidate agent row shown in the live-searchable `ChannelEditor` member checklist.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChannelEditorAgentOption {
    pub id: String,
    pub name: String,
    pub title: String,
    pub description: String,
    pub avatar_seed: Option<String>,
    pub avatar_hue: Option<AvatarHue>,
    pub avatar_color: Option<String>,
    pub avatar_url: Option<String>,
}

impl ChannelEditorAgentOption {
    /// Builds a `ChannelAgentBrief` for M2 `render_channel_avatar`.
    pub fn to_agent_brief(&self) -> ChannelAgentBrief {
        ChannelAgentBrief {
            id: self.id.clone(),
            avatar_seed: self.avatar_seed.clone(),
            avatar_hue: self.avatar_hue,
            avatar_color: self.avatar_color.clone(),
            avatar_url: self.avatar_url.clone(),
        }
    }
}

impl MentionableAgent for ChannelEditorAgentOption {
    fn mention_name(&self) -> &str {
        &self.name
    }

    fn mention_title(&self) -> &str {
        &self.title
    }

    fn mention_description(&self) -> &str {
        &self.description
    }
}

/// Validation and state-guard errors surfaced by `ChannelEditorState` & `ChannelSettingsPaneState`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum ChannelEditorValidationError {
    EmptyName,
    NameTooLong { max: usize },
    TitleTooLong { max: usize },
    InstructionsTooLong { max: usize },
    EmptyMembers,
    TooManyMembers { max: usize },
    DuplicateMember(String),
    InvalidMemberId(String),
    LeadNotMember(String),
    BrowserMemberNotMember(String),
    InvalidIcon(String),
    MemberDragReorderUnsupported,
    ArchiveBlockedByRunningTasks,
    TooManyMemories { max: usize },
    MemoryTextInvalid { max: usize },
    TooManyFolders { max: usize },
    InvalidFolderPath(String),
    ChannelArchivedReadOnly,
    Contract(String),
}

impl fmt::Display for ChannelEditorValidationError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::EmptyName => write!(f, "Channel name is required."),
            Self::NameTooLong { max } => {
                write!(f, "Channel name exceeds maximum length of {max} characters.")
            }
            Self::TitleTooLong { max } => {
                write!(f, "Channel title exceeds maximum length of {max} characters.")
            }
            Self::InstructionsTooLong { max } => {
                write!(
                    f,
                    "Channel instructions exceed maximum length of {max} characters."
                )
            }
            Self::EmptyMembers => {
                write!(f, "At least one channel member is required.")
            }
            Self::TooManyMembers { max } => {
                write!(f, "Channel member roster exceeds maximum limit of {max}.")
            }
            Self::DuplicateMember(id) => write!(f, "Duplicate channel member '{id}'."),
            Self::InvalidMemberId(id) => write!(f, "Invalid channel member identifier '{id}'."),
            Self::LeadNotMember(id) => {
                write!(f, "Lead agent '{id}' must be a member of the channel.")
            }
            Self::BrowserMemberNotMember(id) => {
                write!(
                    f,
                    "Browser policy member '{id}' must be a member of the channel."
                )
            }
            Self::InvalidIcon(name) => {
                write!(f, "Unknown channel icon '{name}' (expected one of 20 icons).")
            }
            Self::MemberDragReorderUnsupported => {
                write!(
                    f,
                    "Member drag-reorder is not supported in ChannelEditor; members remain in insertion order."
                )
            }
            Self::ArchiveBlockedByRunningTasks => {
                write!(
                    f,
                    "Cannot archive channel while tasks are still running."
                )
            }
            Self::TooManyMemories { max } => {
                write!(f, "Channel memory limit of {max} entries reached.")
            }
            Self::MemoryTextInvalid { max } => {
                write!(f, "Channel memory text must be 1..={max} characters.")
            }
            Self::TooManyFolders { max } => {
                write!(f, "Channel folder limit of {max} entries reached.")
            }
            Self::InvalidFolderPath(path) => write!(f, "Invalid channel folder path '{path}'."),
            Self::ChannelArchivedReadOnly => {
                write!(f, "Archived channels are read-only until restored.")
            }
            Self::Contract(msg) => write!(f, "{msg}"),
        }
    }
}

impl std::error::Error for ChannelEditorValidationError {}

/// Compact summary of a scheduled or webhook routine bound to a channel (`ChannelSettingsPane`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChannelRoutineSummary {
    pub id: String,
    pub name: String,
    pub schedule_label: String,
    pub enabled: bool,
    pub last_run_status: Option<String>,
    pub next_run_at: Option<String>,
}

/// Action triggers exposed in `ChannelSettingsPane` conversation & lifecycle section.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum ChannelSettingsDangerAction {
    StartNewConversation,
    CompactContext,
    DeleteHistory,
    ArchiveChannel,
    RestoreChannel,
}

/// Editable channel draft & member/lead/picture state shared between `ChannelCreateDialog` and `ChannelSettingsPane`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChannelEditorState {
    pub channel_id: Option<String>,
    pub name: String,
    pub title: String,
    pub instructions: String,
    pub existing_instructions: Option<String>,
    pub members: Vec<String>,
    pub lead_agent_id: Option<String>,
    pub member_search_query: String,
    pub picture_editor: ChannelPictureEditorState,
    pub image_failed: bool,
    pub submitting: bool,
    pub validation_error: Option<String>,
}

impl Default for ChannelEditorState {
    fn default() -> Self {
        Self::new_create()
    }
}

impl ChannelEditorState {
    /// Creates an empty create-channel draft (`members` empty until selected; first added becomes lead).
    pub fn new_create() -> Self {
        Self {
            channel_id: None,
            name: String::new(),
            title: String::new(),
            instructions: String::new(),
            existing_instructions: None,
            members: Vec::new(),
            lead_agent_id: None,
            member_search_query: String::new(),
            picture_editor: ChannelPictureEditorState::new(None, None),
            image_failed: false,
            submitting: false,
            validation_error: None,
        }
    }

    /// Hydrates editor state from an existing persisted [`Channel`].
    pub fn from_channel(channel: &Channel) -> Self {
        let mut unique_members = Vec::with_capacity(channel.draft.members.len());
        let mut seen = HashSet::new();
        for member in &channel.draft.members {
            if seen.insert(member.agent_id.clone()) {
                unique_members.push(member.agent_id.clone());
            }
        }
        let lead_agent_id = channel
            .draft
            .lead_agent_id
            .as_ref()
            .filter(|id| unique_members.iter().any(|m| m == *id))
            .cloned()
            .or_else(|| unique_members.first().cloned());

        let icon = channel
            .draft
            .icon
            .as_ref()
            .and_then(|opt| opt.as_ref())
            .and_then(|ic| resolve_channel_icon_struct(Some(ic)))
            .map(|(n, h)| channel_icon_from_resolved(n, h));
        let image_url = channel
            .image_url
            .as_ref()
            .and_then(|opt| opt.as_ref())
            .filter(|u| !u.trim().is_empty())
            .cloned();
        let picture_editor = if image_url.is_some() {
            ChannelPictureEditorState::new(None, image_url)
        } else {
            ChannelPictureEditorState::new(icon, None)
        };

        Self {
            channel_id: Some(channel.id.clone()),
            name: channel.draft.name.clone(),
            title: channel.draft.title.clone(),
            instructions: channel.draft.instructions.clone(),
            existing_instructions: Some(channel.draft.instructions.clone()),
            members: unique_members,
            lead_agent_id,
            member_search_query: String::new(),
            picture_editor,
            image_failed: false,
            submitting: false,
            validation_error: None,
        }
    }

    pub fn set_name(&mut self, name: impl Into<String>) {
        self.name = name.into();
        self.validation_error = None;
    }

    pub fn set_title(&mut self, title: impl Into<String>) {
        self.title = title.into();
        self.validation_error = None;
    }

    pub fn set_instructions(&mut self, instructions: impl Into<String>) {
        self.instructions = instructions.into();
        self.validation_error = None;
    }

    pub fn set_member_search_query(&mut self, query: impl Into<String>) {
        self.member_search_query = query.into();
    }

    /// Filters and ranks candidate agents via NFD diacritic-folded `rank_agent_mentions`.
    pub fn filtered_agents(
        &self,
        available_agents: &[ChannelEditorAgentOption],
    ) -> Vec<ChannelEditorAgentOption> {
        if self.member_search_query.trim().is_empty() {
            available_agents.to_vec()
        } else {
            rank_agent_mentions(available_agents, &self.member_search_query)
        }
    }

    pub fn is_member(&self, agent_id: &str) -> bool {
        self.members.iter().any(|m| m == agent_id)
    }

    /// Adds a member in strict insertion order; the first added member auto-becomes `lead_agent_id`.
    pub fn add_member(
        &mut self,
        agent_id: impl Into<String>,
    ) -> Result<bool, ChannelEditorValidationError> {
        let agent_id = agent_id.into();
        if !is_identifier(&agent_id) {
            return Err(ChannelEditorValidationError::InvalidMemberId(agent_id));
        }
        if self.is_member(&agent_id) {
            return Ok(false);
        }
        if self.members.len() >= CHANNEL_MEMBERS_MAX {
            return Err(ChannelEditorValidationError::TooManyMembers {
                max: CHANNEL_MEMBERS_MAX,
            });
        }
        self.members.push(agent_id.clone());
        if self.lead_agent_id.is_none() {
            self.lead_agent_id = Some(agent_id);
        }
        self.validation_error = None;
        Ok(true)
    }

    /// Removes a member preserving insertion order; removing the current lead falls back to `members[0]`.
    pub fn remove_member(&mut self, agent_id: &str) -> bool {
        let before = self.members.len();
        self.members.retain(|m| m != agent_id);
        let removed = self.members.len() != before;
        if removed && self.lead_agent_id.as_deref() == Some(agent_id) {
            self.lead_agent_id = self.members.first().cloned();
        }
        if removed {
            self.validation_error = None;
        }
        removed
    }

    /// Toggles membership for `agent_id` in the searchable member checklist.
    pub fn toggle_member(&mut self, agent_id: &str) -> Result<bool, ChannelEditorValidationError> {
        if self.is_member(agent_id) {
            self.remove_member(agent_id);
            Ok(false)
        } else {
            self.add_member(agent_id.to_string())?;
            Ok(true)
        }
    }

    /// Promotes an existing member to `lead_agent_id` (gold `Crown` button).
    pub fn promote_lead(&mut self, agent_id: &str) -> Result<(), ChannelEditorValidationError> {
        if !self.is_member(agent_id) {
            return Err(ChannelEditorValidationError::LeadNotMember(
                agent_id.to_string(),
            ));
        }
        self.lead_agent_id = Some(agent_id.to_string());
        self.validation_error = None;
        Ok(())
    }

    /// Explicitly rejects member drag-reorder (`MEMBER_DRAG_REORDER_SUPPORTED == false`).
    pub fn attempt_member_drag_reorder(
        &mut self,
        _from_index: usize,
        _to_index: usize,
    ) -> Result<(), ChannelEditorValidationError> {
        Err(ChannelEditorValidationError::MemberDragReorderUnsupported)
    }

    /// Selects a channel icon preset (clears custom uploaded image and failed-image state).
    pub fn select_icon(&mut self, name: ChannelIconName) -> ChannelIcon {
        self.image_failed = false;
        self.picture_editor.select_icon(name)
    }

    /// Selects a channel icon hue (defaults icon to `"star"` if none chosen; clears uploaded image).
    pub fn select_hue(&mut self, hue: AvatarHue) -> ChannelIcon {
        self.image_failed = false;
        self.picture_editor.select_hue(hue)
    }

    /// Applies an uploaded image URL and clears any selected icon tile (mutually exclusive).
    pub fn set_uploaded_image_url(&mut self, url: impl Into<String>) {
        let trimmed = url.into().trim().to_string();
        if trimmed.is_empty() {
            self.picture_editor.image_url = None;
        } else {
            self.picture_editor.icon = None;
            self.picture_editor.image_url = Some(trimmed);
            self.image_failed = false;
        }
    }

    /// Normalizes and attaches an uploaded image (`<= 10 MiB` -> `512px <= 512 KiB` WebP), clearing icon.
    pub fn upload_image_bytes(
        &mut self,
        bytes: &[u8],
        mime_hint: Option<&str>,
    ) -> Result<NormalizedAvatarOutput, AvatarNormalizeError> {
        let output = self.picture_editor.upload_image_bytes(bytes, mime_hint)?;
        self.picture_editor.icon = None;
        self.image_failed = false;
        Ok(output)
    }

    /// Marks the active custom image URL as failed so preview falls back to icon or member face cluster.
    pub fn mark_image_failed(&mut self) {
        self.image_failed = true;
    }

    /// Resets both uploaded image and icon tile so the channel reverts to its member face cluster.
    pub fn reset_picture(&mut self) {
        self.picture_editor.reset();
        self.image_failed = false;
    }

    pub fn toggle_picture_popover(&mut self) {
        self.picture_editor.open = !self.picture_editor.open;
    }

    pub fn resolved_icon(&self) -> Option<(ChannelIconName, AvatarHue)> {
        if self.picture_editor.has_image() && !self.image_failed {
            return None;
        }
        resolve_channel_icon_struct(self.picture_editor.icon.as_ref())
    }

    pub fn active_image_url(&self) -> Option<&str> {
        self.picture_editor.image_url.as_deref()
    }

    pub fn channel_members(&self) -> Vec<ChannelMember> {
        self.members
            .iter()
            .map(|agent_id| ChannelMember {
                agent_id: agent_id.clone(),
            })
            .collect()
    }

    /// Builds a wire [`ChannelDraft`] with optional browser policy & isolation override.
    pub fn to_channel_draft(
        &self,
        browser: Option<ChannelBrowser>,
        isolated: Option<bool>,
    ) -> ChannelDraft {
        let icon = if self.picture_editor.has_image() {
            Some(None)
        } else {
            Some(
                resolve_channel_icon_struct(self.picture_editor.icon.as_ref())
                    .map(|(n, h)| channel_icon_from_resolved(n, h)),
            )
        };
        ChannelDraft {
            name: self.name.trim().to_string(),
            title: self.title.trim().to_string(),
            instructions: self.instructions.clone(),
            members: self.channel_members(),
            lead_agent_id: self.lead_agent_id.clone(),
            isolated,
            browser,
            icon,
        }
    }

    /// Validates the channel editor draft against all OpenBot length, member, lead, and icon bounds.
    pub fn validate_draft(&self) -> Result<ChannelDraft, ChannelEditorValidationError> {
        let trimmed_name = self.name.trim();
        let name_len = utf16_len(trimmed_name);
        if name_len < CHANNEL_NAME_MIN_LEN {
            return Err(ChannelEditorValidationError::EmptyName);
        }
        if name_len > CHANNEL_NAME_MAX_LEN {
            return Err(ChannelEditorValidationError::NameTooLong {
                max: CHANNEL_NAME_MAX_LEN,
            });
        }
        if utf16_len(self.title.trim()) > CHANNEL_TITLE_MAX_LEN {
            return Err(ChannelEditorValidationError::TitleTooLong {
                max: CHANNEL_TITLE_MAX_LEN,
            });
        }
        let instructions_limit = if self.existing_instructions.as_deref()
            == Some(self.instructions.as_str())
        {
            CHANNEL_STORED_INSTRUCTIONS_MAX_LEN
        } else {
            CHANNEL_INSTRUCTIONS_MAX_LEN
        };
        if utf16_len(&self.instructions) > instructions_limit {
            return Err(ChannelEditorValidationError::InstructionsTooLong {
                max: instructions_limit,
            });
        }
        if self.members.len() < CHANNEL_MEMBERS_MIN {
            return Err(ChannelEditorValidationError::EmptyMembers);
        }
        if self.members.len() > CHANNEL_MEMBERS_MAX {
            return Err(ChannelEditorValidationError::TooManyMembers {
                max: CHANNEL_MEMBERS_MAX,
            });
        }
        let mut seen = HashSet::with_capacity(self.members.len());
        for member_id in &self.members {
            if !is_identifier(member_id) {
                return Err(ChannelEditorValidationError::InvalidMemberId(
                    member_id.clone(),
                ));
            }
            if !seen.insert(member_id.as_str()) {
                return Err(ChannelEditorValidationError::DuplicateMember(
                    member_id.clone(),
                ));
            }
        }
        if let Some(lead) = &self.lead_agent_id {
            if !seen.contains(lead.as_str()) {
                return Err(ChannelEditorValidationError::LeadNotMember(lead.clone()));
            }
        }
        if !self.picture_editor.has_image() {
            if let Some(raw_icon) = &self.picture_editor.icon {
                if resolve_channel_icon(&raw_icon.name, Some(raw_icon.hue)).is_none() {
                    return Err(ChannelEditorValidationError::InvalidIcon(
                        raw_icon.name.clone(),
                    ));
                }
            }
        }

        let draft = self.to_channel_draft(None, None);
        draft
            .validate_strict(self.existing_instructions.as_deref())
            .map_err(|e| ChannelEditorValidationError::Contract(e.to_string()))?;
        Ok(draft)
    }

    pub fn can_submit(&self) -> bool {
        !self.submitting && self.validate_draft().is_ok()
    }
}

/// Complete right-pane `ChannelSettingsPane` controller embedding `ChannelEditorState` plus
/// shared browser policy, isolated context, memories, routines, folders, and archive/restore lifecycle.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChannelSettingsPaneState {
    pub editor: ChannelEditorState,
    pub browser: ChannelBrowser,
    pub isolated: bool,
    pub memories: Vec<String>,
    pub routines: Vec<ChannelRoutineSummary>,
    pub folders: Vec<String>,
    pub archived: bool,
    pub has_running_tasks: bool,
    pub paused_task_count: usize,
    pub resumed_paused_task_ids: Vec<String>,
    pub status_notice: Option<String>,
}

impl Default for ChannelSettingsPaneState {
    fn default() -> Self {
        Self {
            editor: ChannelEditorState::new_create(),
            browser: ChannelBrowser::Members,
            isolated: false,
            memories: Vec::new(),
            routines: Vec::new(),
            folders: Vec::new(),
            archived: false,
            has_running_tasks: false,
            paused_task_count: 0,
            resumed_paused_task_ids: Vec::new(),
            status_notice: None,
        }
    }
}

impl ChannelSettingsPaneState {
    /// Hydrates settings pane state from a persisted [`Channel`].
    pub fn from_channel(channel: &Channel) -> Self {
        let editor = ChannelEditorState::from_channel(channel);
        let mut browser = channel.draft.browser.clone().unwrap_or(ChannelBrowser::Members);
        if let ChannelBrowser::Member { agent_id } = &browser {
            if !editor.is_member(agent_id) {
                browser = ChannelBrowser::None;
            }
        }
        Self {
            editor,
            browser,
            isolated: channel.draft.isolated.unwrap_or(false),
            memories: Vec::new(),
            routines: Vec::new(),
            folders: channel.folders.clone().unwrap_or_default(),
            archived: channel.archived,
            has_running_tasks: false,
            paused_task_count: 0,
            resumed_paused_task_ids: Vec::new(),
            status_notice: None,
        }
    }

    pub fn read_only(&self) -> bool {
        self.archived
    }

    /// Updates the shared browser policy (`Members`, `Member { agent_id }`, or `None`).
    pub fn set_browser_policy(
        &mut self,
        policy: ChannelBrowser,
    ) -> Result<(), ChannelEditorValidationError> {
        if self.archived {
            return Err(ChannelEditorValidationError::ChannelArchivedReadOnly);
        }
        if let ChannelBrowser::Member { agent_id } = &policy {
            if !self.editor.is_member(agent_id) {
                return Err(ChannelEditorValidationError::BrowserMemberNotMember(
                    agent_id.clone(),
                ));
            }
        }
        self.browser = policy;
        Ok(())
    }

    /// Removes a member from the channel roster and automatically resets `ChannelBrowser::Member`
    /// to `ChannelBrowser::None` if the removed member was the designated browser owner.
    pub fn remove_member_and_sync_browser(&mut self, agent_id: &str) -> bool {
        if self.archived {
            return false;
        }
        let removed = self.editor.remove_member(agent_id);
        if let ChannelBrowser::Member {
            agent_id: designated,
        } = &self.browser
        {
            if designated == agent_id || !self.editor.is_member(designated) {
                self.browser = ChannelBrowser::None;
            }
        }
        removed
    }

    pub fn set_isolated(&mut self, isolated: bool) -> Result<(), ChannelEditorValidationError> {
        if self.archived {
            return Err(ChannelEditorValidationError::ChannelArchivedReadOnly);
        }
        self.isolated = isolated;
        Ok(())
    }

    /// Appends a channel-scoped memory (`<= 32` entries, trimmed `1..=500` UTF-16 chars).
    pub fn add_memory(&mut self, text: impl Into<String>) -> Result<(), ChannelEditorValidationError> {
        if self.archived {
            return Err(ChannelEditorValidationError::ChannelArchivedReadOnly);
        }
        if self.memories.len() >= CHANNEL_MEMORIES_MAX {
            return Err(ChannelEditorValidationError::TooManyMemories {
                max: CHANNEL_MEMORIES_MAX,
            });
        }
        let trimmed = text.into().trim().to_string();
        let len = utf16_len(&trimmed);
        if len == 0 || len > CHANNEL_MEMORY_TEXT_MAX {
            return Err(ChannelEditorValidationError::MemoryTextInvalid {
                max: CHANNEL_MEMORY_TEXT_MAX,
            });
        }
        self.memories.push(trimmed);
        Ok(())
    }

    pub fn remove_memory_at(
        &mut self,
        index: usize,
    ) -> Result<Option<String>, ChannelEditorValidationError> {
        if self.archived {
            return Err(ChannelEditorValidationError::ChannelArchivedReadOnly);
        }
        if index >= self.memories.len() {
            return Ok(None);
        }
        Ok(Some(self.memories.remove(index)))
    }

    /// Links a workspace folder to the channel (`<= 8` unique paths).
    pub fn add_folder(&mut self, path: impl Into<String>) -> Result<(), ChannelEditorValidationError> {
        if self.archived {
            return Err(ChannelEditorValidationError::ChannelArchivedReadOnly);
        }
        let trimmed = path.into().trim().to_string();
        if trimmed.is_empty() || utf16_len(&trimmed) > INPUT_LIMITS.path {
            return Err(ChannelEditorValidationError::InvalidFolderPath(trimmed));
        }
        if self.folders.iter().any(|p| p == &trimmed) {
            return Ok(());
        }
        if self.folders.len() >= CHANNEL_FOLDERS_MAX {
            return Err(ChannelEditorValidationError::TooManyFolders {
                max: CHANNEL_FOLDERS_MAX,
            });
        }
        self.folders.push(trimmed);
        Ok(())
    }

    pub fn remove_folder(&mut self, path: &str) -> Result<bool, ChannelEditorValidationError> {
        if self.archived {
            return Err(ChannelEditorValidationError::ChannelArchivedReadOnly);
        }
        let before = self.folders.len();
        self.folders.retain(|p| p != path);
        Ok(self.folders.len() != before)
    }

    pub fn can_archive(&self) -> bool {
        !self.archived && !self.has_running_tasks
    }

    /// Archives the channel (`read_only = true`); refuses while any task is still running.
    pub fn archive(&mut self) -> Result<(), ChannelEditorValidationError> {
        if self.has_running_tasks {
            return Err(ChannelEditorValidationError::ArchiveBlockedByRunningTasks);
        }
        self.archived = true;
        self.status_notice = Some("Channel archived. Restoring re-enables messaging.".to_string());
        Ok(())
    }

    /// Restores an archived channel and returns the paused task IDs scheduled to resume.
    pub fn restore(&mut self, paused_task_ids: &[String]) -> Vec<String> {
        self.archived = false;
        self.resumed_paused_task_ids = paused_task_ids.to_vec();
        let count = self.resumed_paused_task_ids.len();
        self.paused_task_count = 0;
        self.status_notice = Some(if count > 0 {
            format!("Channel restored and resumed {count} paused task(s).")
        } else {
            "Channel restored.".to_string()
        });
        self.resumed_paused_task_ids.clone()
    }

    /// Validates the full settings payload including synchronized `ChannelBrowser` policy.
    pub fn validate_settings_draft(&self) -> Result<ChannelDraft, ChannelEditorValidationError> {
        let mut draft = self
            .editor
            .to_channel_draft(Some(self.browser.clone()), Some(self.isolated));
        with_browser_member(&mut draft);
        self.editor.validate_draft()?;
        draft
            .validate_strict(self.editor.existing_instructions.as_deref())
            .map_err(|e| ChannelEditorValidationError::Contract(e.to_string()))?;
        Ok(draft)
    }

    pub fn available_danger_actions(&self) -> Vec<ChannelSettingsDangerAction> {
        if self.archived {
            vec![ChannelSettingsDangerAction::RestoreChannel]
        } else {
            vec![
                ChannelSettingsDangerAction::StartNewConversation,
                ChannelSettingsDangerAction::CompactContext,
                ChannelSettingsDangerAction::DeleteHistory,
                ChannelSettingsDangerAction::ArchiveChannel,
            ]
        }
    }
}

fn editor_rgba(palette: &OpenBotPalette, token: &str, fallback_rgb: u32) -> Rgba {
    palette.rgba(token).unwrap_or_else(|| rgb(fallback_rgb))
}

fn editor_is_dark(palette: &OpenBotPalette) -> bool {
    palette.scheme == OpenBotColorScheme::Dark
}

/// Renders a single searchable `ChannelMemberRow` with checkbox, live `AgentAvatar`,
/// role title, and gold `Crown` lead-promoter badge.
pub fn render_channel_member_row(
    agent: &ChannelEditorAgentOption,
    is_member: bool,
    is_lead: bool,
    read_only: bool,
    palette: &OpenBotPalette,
    now_sec: f64,
) -> Div {
    let dark = editor_is_dark(palette);
    let bg_control = editor_rgba(palette, "--openbot-bg-control", 0x212121);
    let bg_hover = editor_rgba(palette, "--openbot-bg-control-hover", 0x242424);
    let border_col = editor_rgba(palette, "--openbot-border-strong", 0x2c2c2e);
    let text_pri = editor_rgba(palette, "--openbot-text-primary", 0xffffff);
    let text_muted = editor_rgba(palette, "--openbot-text-muted", 0x979797);
    let accent = editor_rgba(palette, "--openbot-accent", 0x007cf7);
    let accent_soft = editor_rgba(palette, "--openbot-accent-soft", 0x0d2848);
    let text_on_accent = editor_rgba(palette, "--openbot-text-on-accent", 0xffffff);
    let crown_gold = rgb(0xf0b429);
    let crown_gold_soft = rgba(0xf0b42926);

    let head_hex = avatar_head_color(
        agent.avatar_seed.as_deref().unwrap_or(&agent.id),
        agent.avatar_hue,
        agent.avatar_color.as_deref(),
    );
    let name_color = if is_member {
        parse_css_color_to_rgba(&head_hex).unwrap_or(text_pri)
    } else {
        text_pri
    };

    let mut avatar_props = AgentAvatarProps::new(
        agent
            .avatar_seed
            .clone()
            .unwrap_or_else(|| agent.id.clone()),
    )
    .with_size(28.0)
    .with_mood(AvatarMood::Idle)
    .with_hue_and_color(agent.avatar_hue, agent.avatar_color.clone())
    .with_url(agent.avatar_url.clone());
    avatar_props.dark_scheme = dark;

    div()
        .w_full()
        .px(px(10.0))
        .py(px(8.0))
        .rounded(px(10.0))
        .bg(if is_member { accent_soft } else { bg_control })
        .border_1()
        .border_color(if is_member { accent } else { border_col })
        .when(!read_only, |el| el.hover(move |s| s.bg(bg_hover)).cursor_pointer())
        .when(read_only, |el| el.opacity(0.65))
        .flex()
        .items_center()
        .justify_between()
        .gap(px(8.0))
        .child(
            div()
                .flex()
                .items_center()
                .gap(px(8.0))
                .min_w(px(0.0))
                .flex_1()
                .child(
                    div()
                        .w(px(18.0))
                        .h(px(18.0))
                        .rounded(px(5.0))
                        .border_1()
                        .border_color(if is_member { accent } else { text_muted })
                        .bg(if is_member { accent } else { rgba(0x00000000) })
                        .flex()
                        .items_center()
                        .justify_center()
                        .flex_shrink_0()
                        .when(is_member, |el| {
                            el.child(tg_icon_rgba(IconName::SelectCheck, 11.0, text_on_accent))
                        }),
                )
                .child(render_agent_avatar(&avatar_props, None, now_sec))
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .min_w(px(0.0))
                        .flex_1()
                        .child(
                            div()
                                .text_size(px(13.0))
                                .font_weight(FontWeight::SEMIBOLD)
                                .text_color(name_color)
                                .truncate()
                                .child(agent.name.clone()),
                        )
                        .when(!agent.title.trim().is_empty(), |el| {
                            el.child(
                                div()
                                    .text_size(px(11.5))
                                    .text_color(text_muted)
                                    .truncate()
                                    .child(agent.title.clone()),
                            )
                        }),
                ),
        )
        .when(is_member, |el| {
            el.child(
                div()
                    .px(px(8.0))
                    .py(px(4.0))
                    .rounded_full()
                    .bg(if is_lead { crown_gold_soft } else { rgba(0x00000000) })
                    .border_1()
                    .border_color(if is_lead { crown_gold } else { border_col })
                    .flex()
                    .items_center()
                    .gap(px(4.0))
                    .flex_shrink_0()
                    .text_size(px(11.5))
                    .font_weight(if is_lead {
                        FontWeight::SEMIBOLD
                    } else {
                        FontWeight::NORMAL
                    })
                    .text_color(if is_lead { crown_gold } else { text_muted })
                    .child("👑")
                    .child(if is_lead { "Lead" } else { "Make lead" }),
            )
        })
}

/// Renders the core `ChannelEditor` form: 62px `ChannelPictureEditor` trigger & popover,
/// name/title/instructions inputs, and live-searchable insertion-ordered member checklist.
pub fn render_channel_editor(
    state: &ChannelEditorState,
    available_agents: &[ChannelEditorAgentOption],
    palette: &OpenBotPalette,
    read_only: bool,
    now_sec: f64,
) -> Div {
    let dark = editor_is_dark(palette);
    let bg_surface = editor_rgba(palette, "--openbot-bg-surface", 0x212121);
    let bg_control = editor_rgba(palette, "--openbot-bg-control", 0x212121);
    let border_grouped = editor_rgba(palette, "--openbot-border-grouped", 0x333335);
    let text_pri = editor_rgba(palette, "--openbot-text-primary", 0xffffff);
    let text_sec = editor_rgba(palette, "--openbot-text-secondary", 0xdcdcdc);
    let text_muted = editor_rgba(palette, "--openbot-text-muted", 0x979797);
    let text_dim = editor_rgba(palette, "--openbot-text-dim", 0x6a6a6a);
    let danger = editor_rgba(palette, "--openbot-danger-text", 0xff6069);
    let danger_soft = editor_rgba(palette, "--openbot-danger-soft", 0x451518);
    let danger_border = editor_rgba(palette, "--openbot-danger-border", 0x802028);

    let briefs: Vec<ChannelAgentBrief> = available_agents
        .iter()
        .map(ChannelEditorAgentOption::to_agent_brief)
        .collect();
    let contract_members = state.channel_members();
    let preview_avatar = render_channel_avatar(
        &contract_members,
        &briefs,
        ChannelAvatarLayout::Cluster,
        state.picture_editor.icon.as_ref(),
        state.picture_editor.image_url.as_deref(),
        state.image_failed,
        CHANNEL_PICTURE_TRIGGER_SIZE_PX,
        None,
        dark,
        now_sec,
    );

    let filtered = state.filtered_agents(available_agents);
    let name_len = utf16_len(state.name.trim());
    let title_len = utf16_len(state.title.trim());
    let instr_len = utf16_len(&state.instructions);
    let mut member_rows = div().flex().flex_col().gap(px(6.0));
    for agent in &filtered {
        let is_member = state.is_member(&agent.id);
        let is_lead = state.lead_agent_id.as_deref() == Some(agent.id.as_str());
        member_rows = member_rows.child(render_channel_member_row(
            agent, is_member, is_lead, read_only, palette, now_sec,
        ));
    }
    if filtered.is_empty() {
        member_rows = member_rows.child(
            div()
                .py(px(14.0))
                .text_size(px(12.5))
                .text_color(text_muted)
                .child("No matching agents found."),
        );
    }

    div()
        .w_full()
        .flex()
        .flex_col()
        .gap(px(14.0))
        .child(
            div()
                .relative()
                .flex()
                .items_center()
                .gap(px(12.0))
                .child(
                    div()
                        .w(px(CHANNEL_PICTURE_TRIGGER_SIZE_PX))
                        .h(px(CHANNEL_PICTURE_TRIGGER_SIZE_PX))
                        .rounded(px(16.0))
                        .bg(bg_control)
                        .border_1()
                        .border_color(border_grouped)
                        .flex()
                        .items_center()
                        .justify_center()
                        .flex_shrink_0()
                        .when(!read_only, |el| el.cursor_pointer())
                        .child(preview_avatar),
                )
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .gap(px(4.0))
                        .flex_1()
                        .min_w(px(0.0))
                        .child(
                            div()
                                .text_size(px(13.0))
                                .font_weight(FontWeight::SEMIBOLD)
                                .text_color(text_pri)
                                .child("Channel Picture"),
                        )
                        .child(
                            div()
                                .text_size(px(12.0))
                                .text_color(text_muted)
                                .child(format!(
                                    "Upload image or pick from {ICON_CATALOG_COUNT} icons & {} hues (default {}°).",
                                    CHANNEL_ICON_HUES.len(),
                                    DEFAULT_CHANNEL_ICON_HUE
                                )),
                        )
                        .when(state.picture_editor.can_reset(), |el| {
                            el.child(
                                div()
                                    .text_size(px(11.5))
                                    .text_color(text_sec)
                                    .child("Reset reverts to member face cluster"),
                            )
                        }),
                )
                .when(state.picture_editor.open && !read_only, |el| {
                    el.child(
                        deferred(
                            div()
                                .absolute()
                                .top(px(CHANNEL_PICTURE_TRIGGER_SIZE_PX + 8.0))
                                .left_0()
                                .occlude()
                                .child(state.picture_editor.render_popover(360.0, dark)),
                        )
                        .with_priority(50),
                    )
                }),
        )
        .child(render_labeled_field(
            "Name",
            &format!("{name_len}/{CHANNEL_NAME_MAX_LEN}"),
            if state.name.is_empty() {
                "e.g. Design Sync"
            } else {
                &state.name
            },
            state.name.is_empty(),
            false,
            palette,
        ))
        .child(render_labeled_field(
            "Title",
            &format!("{title_len}/{CHANNEL_TITLE_MAX_LEN}"),
            if state.title.is_empty() {
                "Optional subtitle shown in header"
            } else {
                &state.title
            },
            state.title.is_empty(),
            false,
            palette,
        ))
        .child(render_labeled_field(
            "Instructions",
            &format!("{instr_len}/{CHANNEL_INSTRUCTIONS_MAX_LEN}"),
            if state.instructions.is_empty() {
                "Shared channel guidance and collaboration rules..."
            } else {
                &state.instructions
            },
            state.instructions.is_empty(),
            true,
            palette,
        ))
        .child(
            div()
                .flex()
                .flex_col()
                .gap(px(8.0))
                .p(px(10.0))
                .rounded(px(12.0))
                .bg(bg_surface)
                .border_1()
                .border_color(border_grouped)
                .child(
                    div()
                        .flex()
                        .items_center()
                        .justify_between()
                        .child(
                            div()
                                .text_size(px(12.5))
                                .font_weight(FontWeight::SEMIBOLD)
                                .text_color(text_pri)
                                .child(format!(
                                    "Members ({}/{CHANNEL_MEMBERS_MAX})",
                                    state.members.len()
                                )),
                        )
                        .child(
                            div()
                                .text_size(px(11.0))
                                .text_color(text_dim)
                                .child("Insertion order · click 👑 to set lead"),
                        ),
                )
                .child(
                    div()
                        .h(px(32.0))
                        .px(px(10.0))
                        .rounded(px(8.0))
                        .bg(bg_control)
                        .border_1()
                        .border_color(border_grouped)
                        .flex()
                        .items_center()
                        .gap(px(6.0))
                        .child(tg_icon_rgba(IconName::Search, 13.0, text_muted))
                        .child(
                            div()
                                .text_size(px(12.5))
                                .text_color(if state.member_search_query.is_empty() {
                                    text_dim
                                } else {
                                    text_pri
                                })
                                .child(if state.member_search_query.is_empty() {
                                    "Filter bots by name or role...".to_string()
                                } else {
                                    state.member_search_query.clone()
                                }),
                        ),
                )
                .child(member_rows),
        )
        .when_some(state.validation_error.as_ref(), |el, err| {
            el.child(
                div()
                    .px(px(10.0))
                    .py(px(8.0))
                    .rounded(px(10.0))
                    .bg(danger_soft)
                    .border_1()
                    .border_color(danger_border)
                    .text_size(px(12.5))
                    .text_color(danger)
                    .child(err.clone()),
            )
        })
}

fn render_labeled_field(
    label: &str,
    counter: &str,
    display_value: &str,
    is_placeholder: bool,
    multiline: bool,
    palette: &OpenBotPalette,
) -> Div {
    let bg_control = editor_rgba(palette, "--openbot-bg-control", 0x212121);
    let border_grouped = editor_rgba(palette, "--openbot-border-grouped", 0x333335);
    let text_pri = editor_rgba(palette, "--openbot-text-primary", 0xffffff);
    let text_sec = editor_rgba(palette, "--openbot-text-secondary", 0xdcdcdc);
    let text_dim = editor_rgba(palette, "--openbot-text-dim", 0x6a6a6a);

    div()
        .flex()
        .flex_col()
        .gap(px(4.0))
        .child(
            div()
                .flex()
                .items_center()
                .justify_between()
                .child(
                    div()
                        .text_size(px(12.0))
                        .font_weight(FontWeight::MEDIUM)
                        .text_color(text_sec)
                        .child(label.to_string()),
                )
                .child(
                    div()
                        .text_size(px(11.0))
                        .text_color(text_dim)
                        .child(counter.to_string()),
                ),
        )
        .child(
            div()
                .w_full()
                .min_h(px(if multiline { 68.0 } else { 34.0 }))
                .px(px(10.0))
                .py(px(7.0))
                .rounded(px(8.0))
                .bg(bg_control)
                .border_1()
                .border_color(border_grouped)
                .text_size(px(13.0))
                .text_color(if is_placeholder { text_dim } else { text_pri })
                .child(display_value.to_string()),
        )
}

/// Renders the modal `ChannelCreateDialog` wrapping `render_channel_editor` with Cancel / Create CTA.
pub fn render_channel_create_dialog(
    state: &ChannelEditorState,
    available_agents: &[ChannelEditorAgentOption],
    palette: &OpenBotPalette,
    now_sec: f64,
) -> Div {
    let bg_sheet = editor_rgba(palette, "--openbot-bg-sheet", 0x141414);
    let bg_control = editor_rgba(palette, "--openbot-bg-control", 0x212121);
    let bg_hover = editor_rgba(palette, "--openbot-bg-control-hover", 0x242424);
    let dialog_ring = editor_rgba(palette, "--openbot-dialog-ring", 0x333335);
    let border_col = editor_rgba(palette, "--openbot-border-grouped", 0x333335);
    let text_pri = editor_rgba(palette, "--openbot-text-primary", 0xffffff);
    let text_muted = editor_rgba(palette, "--openbot-text-muted", 0x979797);
    let accent = editor_rgba(palette, "--openbot-accent", 0x007cf7);
    let accent_hover = editor_rgba(palette, "--openbot-accent-hover", 0x1f8fff);
    let text_on_accent = editor_rgba(palette, "--openbot-text-on-accent", 0xffffff);
    let can_submit = state.can_submit();

    div()
        .w(px(520.0))
        .max_h(px(680.0))
        .rounded(px(20.0))
        .bg(bg_sheet)
        .border_1()
        .border_color(dialog_ring)
        .flex()
        .flex_col()
        .overflow_hidden()
        .child(
            div()
                .h(px(52.0))
                .px(px(18.0))
                .border_b_1()
                .border_color(border_col)
                .flex()
                .items_center()
                .justify_between()
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(8.0))
                        .child(tg_icon_rgba(IconName::ChannelMegaphone, 16.0, accent))
                        .child(
                            div()
                                .text_size(px(15.0))
                                .font_weight(FontWeight::SEMIBOLD)
                                .text_color(text_pri)
                                .child("New Channel"),
                        ),
                )
                .child(
                    div()
                        .w(px(28.0))
                        .h(px(28.0))
                        .rounded(px(8.0))
                        .flex()
                        .items_center()
                        .justify_center()
                        .hover(move |s| s.bg(bg_hover))
                        .cursor_pointer()
                        .child(tg_icon_rgba(IconName::Close, 14.0, text_muted)),
                ),
        )
        .child(
            div()
                .flex_1()
                .min_h(px(0.0))
                .child(
                    div()
                        .id("openbot-channel-create-scroll")
                        .size_full()
                        .overflow_y_scroll()
                        .p(px(18.0))
                        .child(render_channel_editor(
                            state,
                            available_agents,
                            palette,
                            false,
                            now_sec,
                        )),
                ),
        )
        .child(
            div()
                .h(px(56.0))
                .px(px(18.0))
                .border_t_1()
                .border_color(border_col)
                .flex()
                .items_center()
                .justify_end()
                .gap(px(8.0))
                .child(
                    div()
                        .h(px(34.0))
                        .px(px(14.0))
                        .rounded(px(9.0))
                        .bg(bg_control)
                        .border_1()
                        .border_color(border_col)
                        .hover(move |s| s.bg(bg_hover))
                        .cursor_pointer()
                        .flex()
                        .items_center()
                        .text_size(px(13.0))
                        .text_color(text_pri)
                        .child("Cancel"),
                )
                .child(
                    div()
                        .h(px(34.0))
                        .px(px(16.0))
                        .rounded(px(9.0))
                        .bg(accent)
                        .when(can_submit, |el| {
                            el.hover(move |s| s.bg(accent_hover)).cursor_pointer()
                        })
                        .when(!can_submit, |el| el.opacity(0.45))
                        .flex()
                        .items_center()
                        .text_size(px(13.0))
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(text_on_accent)
                        .child(if state.submitting {
                            "Creating..."
                        } else {
                            "Create Channel"
                        }),
                ),
        )
}

fn render_browser_policy_card(
    state: &ChannelSettingsPaneState,
    available_agents: &[ChannelEditorAgentOption],
    palette: &OpenBotPalette,
) -> Div {
    let bg_surface = editor_rgba(palette, "--openbot-bg-surface", 0x212121);
    let bg_pill = editor_rgba(palette, "--openbot-tabs-bar-bg", 0x212121);
    let bg_pill_active = editor_rgba(palette, "--openbot-tabs-pill-bg", 0x2a2a2a);
    let border_grouped = editor_rgba(palette, "--openbot-border-grouped", 0x333335);
    let text_pri = editor_rgba(palette, "--openbot-text-primary", 0xffffff);
    let text_muted = editor_rgba(palette, "--openbot-text-muted", 0x979797);
    let accent = editor_rgba(palette, "--openbot-accent", 0x007cf7);
    let accent_soft = editor_rgba(palette, "--openbot-accent-soft", 0x0d2848);

    let active_kind = match &state.browser {
        ChannelBrowser::Members => 0usize,
        ChannelBrowser::Member { .. } => 1usize,
        ChannelBrowser::None => 2usize,
    };
    let tabs = ["Everyone", "Specific member", "No browser"];
    let mut seg = div()
        .p(px(3.0))
        .rounded(px(10.0))
        .bg(bg_pill)
        .border_1()
        .border_color(border_grouped)
        .flex()
        .items_center()
        .gap(px(3.0));
    for (idx, label) in tabs.iter().enumerate() {
        let selected = idx == active_kind;
        seg = seg.child(
            div()
                .flex_1()
                .h(px(28.0))
                .rounded(px(7.0))
                .bg(if selected {
                    bg_pill_active
                } else {
                    rgba(0x00000000)
                })
                .flex()
                .items_center()
                .justify_center()
                .text_size(px(12.0))
                .font_weight(if selected {
                    FontWeight::SEMIBOLD
                } else {
                    FontWeight::NORMAL
                })
                .text_color(if selected { text_pri } else { text_muted })
                .when(!state.archived, |el| el.cursor_pointer())
                .child(*label),
        );
    }

    let designated_id = match &state.browser {
        ChannelBrowser::Member { agent_id } => Some(agent_id.as_str()),
        _ => None,
    };
    let mut member_chips = div().flex().flex_wrap().gap(px(6.0));
    if active_kind == 1 {
        for member_id in &state.editor.members {
            let label = available_agents
                .iter()
                .find(|a| a.id == *member_id)
                .map(|a| a.name.clone())
                .unwrap_or_else(|| member_id.clone());
            let selected = designated_id == Some(member_id.as_str());
            member_chips = member_chips.child(
                div()
                    .px(px(9.0))
                    .py(px(4.0))
                    .rounded_full()
                    .bg(if selected { accent_soft } else { bg_pill })
                    .border_1()
                    .border_color(if selected { accent } else { border_grouped })
                    .text_size(px(12.0))
                    .text_color(if selected { text_pri } else { text_muted })
                    .child(label),
            );
        }
    }

    div()
        .p(px(12.0))
        .rounded(px(12.0))
        .bg(bg_surface)
        .border_1()
        .border_color(border_grouped)
        .flex()
        .flex_col()
        .gap(px(8.0))
        .child(
            div()
                .text_size(px(13.0))
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(text_pri)
                .child("Shared Browser Policy"),
        )
        .child(seg)
        .when(active_kind == 1, |el| el.child(member_chips))
}

/// Renders the full right-pane `ChannelSettingsPane` (`min_w >= 360px`), including archived read-only
/// banner, embedded `ChannelEditor`, shared browser policy, isolated context toggle, memories (`<= 32`),
/// channel routines, linked folders (`<= 8`), and conversation/archive/restore controls.
pub fn render_channel_settings_pane(
    state: &ChannelSettingsPaneState,
    available_agents: &[ChannelEditorAgentOption],
    palette: &OpenBotPalette,
    now_sec: f64,
) -> Div {
    let bg_sheet = editor_rgba(palette, "--openbot-bg-sheet", 0x141414);
    let bg_surface = editor_rgba(palette, "--openbot-bg-surface", 0x212121);
    let bg_control = editor_rgba(palette, "--openbot-bg-control", 0x212121);
    let bg_hover = editor_rgba(palette, "--openbot-bg-control-hover", 0x242424);
    let border_grouped = editor_rgba(palette, "--openbot-border-grouped", 0x333335);
    let text_pri = editor_rgba(palette, "--openbot-text-primary", 0xffffff);
    let text_sec = editor_rgba(palette, "--openbot-text-secondary", 0xdcdcdc);
    let text_muted = editor_rgba(palette, "--openbot-text-muted", 0x979797);
    let accent = editor_rgba(palette, "--openbot-accent", 0x007cf7);
    let text_on_accent = editor_rgba(palette, "--openbot-text-on-accent", 0xffffff);
    let success = editor_rgba(palette, "--openbot-success", 0x31cf76);
    let warning = editor_rgba(palette, "--openbot-warning-text", 0xff9412);
    let warning_soft = editor_rgba(palette, "--openbot-warning-soft", 0x3b260b);
    let danger_text = editor_rgba(palette, "--openbot-danger-text", 0xff6069);
    let danger_soft = editor_rgba(palette, "--openbot-danger-soft", 0x451518);
    let danger_border = editor_rgba(palette, "--openbot-danger-border", 0x802028);

    let mut memories_list = div().flex().flex_col().gap(px(6.0));
    if state.memories.is_empty() {
        memories_list = memories_list.child(
            div()
                .text_size(px(12.0))
                .text_color(text_muted)
                .child("No channel-scoped memories yet."),
        );
    } else {
        for memory in &state.memories {
            memories_list = memories_list.child(
                div()
                    .px(px(10.0))
                    .py(px(7.0))
                    .rounded(px(8.0))
                    .bg(bg_control)
                    .border_1()
                    .border_color(border_grouped)
                    .flex()
                    .items_center()
                    .justify_between()
                    .gap(px(8.0))
                    .child(
                        div()
                            .flex_1()
                            .min_w(px(0.0))
                            .text_size(px(12.5))
                            .text_color(text_sec)
                            .child(memory.clone()),
                    )
                    .when(!state.archived, |el| {
                        el.child(tg_icon_rgba(IconName::DeleteTrash, 13.0, text_muted))
                    }),
            );
        }
    }

    let mut routines_list = div().flex().flex_col().gap(px(6.0));
    if state.routines.is_empty() {
        routines_list = routines_list.child(
            div()
                .text_size(px(12.0))
                .text_color(text_muted)
                .child("No channel routines configured."),
        );
    } else {
        for routine in &state.routines {
            routines_list = routines_list.child(
                div()
                    .px(px(10.0))
                    .py(px(8.0))
                    .rounded(px(8.0))
                    .bg(bg_control)
                    .border_1()
                    .border_color(border_grouped)
                    .flex()
                    .items_center()
                    .justify_between()
                    .gap(px(8.0))
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(8.0))
                            .min_w(px(0.0))
                            .flex_1()
                            .child(
                                div()
                                    .w(px(8.0))
                                    .h(px(8.0))
                                    .rounded_full()
                                    .bg(if routine.enabled { success } else { text_muted }),
                            )
                            .child(
                                div()
                                    .flex()
                                    .flex_col()
                                    .min_w(px(0.0))
                                    .child(
                                        div()
                                            .text_size(px(12.5))
                                            .font_weight(FontWeight::MEDIUM)
                                            .text_color(text_pri)
                                            .truncate()
                                            .child(routine.name.clone()),
                                    )
                                    .child(
                                        div()
                                            .text_size(px(11.0))
                                            .text_color(text_muted)
                                            .child(routine.schedule_label.clone()),
                                    ),
                            ),
                    )
                    .when_some(routine.last_run_status.as_ref(), |el, status| {
                        el.child(
                            div()
                                .text_size(px(11.0))
                                .text_color(text_muted)
                                .child(status.clone()),
                        )
                    }),
            );
        }
    }

    let mut folders_list = div().flex().flex_col().gap(px(6.0));
    if state.folders.is_empty() {
        folders_list = folders_list.child(
            div()
                .text_size(px(12.0))
                .text_color(text_muted)
                .child("No shared folders attached."),
        );
    } else {
        for folder in &state.folders {
            folders_list = folders_list.child(
                div()
                    .px(px(10.0))
                    .py(px(6.0))
                    .rounded(px(8.0))
                    .bg(bg_control)
                    .border_1()
                    .border_color(border_grouped)
                    .flex()
                    .items_center()
                    .gap(px(8.0))
                    .child(tg_icon_rgba(IconName::FolderWork, 14.0, accent))
                    .child(
                        div()
                            .flex_1()
                            .min_w(px(0.0))
                            .text_size(px(12.0))
                            .text_color(text_sec)
                            .truncate()
                            .child(folder.clone()),
                    ),
            );
        }
    }

    let can_archive = state.can_archive();
    let content = div()
        .p(px(16.0))
        .flex()
        .flex_col()
        .gap(px(14.0))
        .when(state.archived, |el| {
            el.child(
                div()
                    .p(px(12.0))
                    .rounded(px(12.0))
                    .bg(warning_soft)
                    .border_1()
                    .border_color(warning)
                    .flex()
                    .items_center()
                    .justify_between()
                    .gap(px(8.0))
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .gap(px(2.0))
                            .child(
                                div()
                                    .text_size(px(13.0))
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .text_color(warning)
                                    .child("Archived (Read-only)"),
                            )
                            .child(
                                div()
                                    .text_size(px(11.5))
                                    .text_color(text_sec)
                                    .child(if state.paused_task_count > 0 {
                                        format!(
                                            "Restore resumes {} paused task(s).",
                                            state.paused_task_count
                                        )
                                    } else {
                                        "Restore to resume messaging in this channel.".to_string()
                                    }),
                            ),
                    )
                    .child(
                        div()
                            .px(px(12.0))
                            .py(px(6.0))
                            .rounded(px(8.0))
                            .bg(accent)
                            .text_size(px(12.0))
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(text_on_accent)
                            .cursor_pointer()
                            .child("Restore"),
                    ),
            )
        })
        .when_some(state.status_notice.as_ref(), |el, notice| {
            el.child(
                div()
                    .px(px(10.0))
                    .py(px(7.0))
                    .rounded(px(8.0))
                    .bg(bg_surface)
                    .border_1()
                    .border_color(border_grouped)
                    .text_size(px(12.0))
                    .text_color(text_sec)
                    .child(notice.clone()),
            )
        })
        .child(render_channel_editor(
            &state.editor,
            available_agents,
            palette,
            state.archived,
            now_sec,
        ))
        .child(render_browser_policy_card(state, available_agents, palette))
        .child(
            div()
                .p(px(12.0))
                .rounded(px(12.0))
                .bg(bg_surface)
                .border_1()
                .border_color(border_grouped)
                .flex()
                .items_center()
                .justify_between()
                .gap(px(12.0))
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .gap(px(2.0))
                        .flex_1()
                        .child(
                            div()
                                .text_size(px(13.0))
                                .font_weight(FontWeight::SEMIBOLD)
                                .text_color(text_pri)
                                .child("Isolated Context"),
                        )
                        .child(
                            div()
                                .text_size(px(11.5))
                                .text_color(text_muted)
                                .child("Keep channel memory and context separate from 1:1 chats."),
                        ),
                )
                .child(
                    div()
                        .w(px(36.0))
                        .h(px(20.0))
                        .p(px(2.0))
                        .rounded_full()
                        .bg(if state.isolated { accent } else { border_grouped })
                        .flex()
                        .items_center()
                        .when(state.isolated, |el| el.justify_end())
                        .when(!state.archived, |el| el.cursor_pointer())
                        .child(
                            div()
                                .w(px(16.0))
                                .h(px(16.0))
                                .rounded_full()
                                .bg(text_on_accent),
                        ),
                ),
        )
        .child(
            div()
                .p(px(12.0))
                .rounded(px(12.0))
                .bg(bg_surface)
                .border_1()
                .border_color(border_grouped)
                .flex()
                .flex_col()
                .gap(px(8.0))
                .child(
                    div()
                        .text_size(px(13.0))
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(text_pri)
                        .child(format!(
                            "Channel Memories ({}/{CHANNEL_MEMORIES_MAX})",
                            state.memories.len()
                        )),
                )
                .child(memories_list),
        )
        .child(
            div()
                .p(px(12.0))
                .rounded(px(12.0))
                .bg(bg_surface)
                .border_1()
                .border_color(border_grouped)
                .flex()
                .flex_col()
                .gap(px(8.0))
                .child(
                    div()
                        .text_size(px(13.0))
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(text_pri)
                        .child(format!("Channel Routines ({})", state.routines.len())),
                )
                .child(routines_list),
        )
        .child(
            div()
                .p(px(12.0))
                .rounded(px(12.0))
                .bg(bg_surface)
                .border_1()
                .border_color(border_grouped)
                .flex()
                .flex_col()
                .gap(px(8.0))
                .child(
                    div()
                        .text_size(px(13.0))
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(text_pri)
                        .child(format!(
                            "Channel Folders ({}/{CHANNEL_FOLDERS_MAX})",
                            state.folders.len()
                        )),
                )
                .child(folders_list),
        )
        .child(
            div()
                .p(px(12.0))
                .rounded(px(12.0))
                .bg(bg_surface)
                .border_1()
                .border_color(border_grouped)
                .flex()
                .flex_col()
                .gap(px(8.0))
                .child(
                    div()
                        .text_size(px(13.0))
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(text_pri)
                        .child("Lifecycle & History"),
                )
                .when(!state.archived, |el| {
                    el.child(
                        div()
                            .flex()
                            .flex_wrap()
                            .gap(px(6.0))
                            .child(
                                div()
                                    .px(px(10.0))
                                    .py(px(6.0))
                                    .rounded(px(8.0))
                                    .bg(bg_control)
                                    .border_1()
                                    .border_color(border_grouped)
                                    .hover(move |s| s.bg(bg_hover))
                                    .cursor_pointer()
                                    .text_size(px(12.0))
                                    .text_color(text_pri)
                                    .child("Start new conversation"),
                            )
                            .child(
                                div()
                                    .px(px(10.0))
                                    .py(px(6.0))
                                    .rounded(px(8.0))
                                    .bg(bg_control)
                                    .border_1()
                                    .border_color(border_grouped)
                                    .hover(move |s| s.bg(bg_hover))
                                    .cursor_pointer()
                                    .text_size(px(12.0))
                                    .text_color(text_pri)
                                    .child("Compact context"),
                            )
                            .child(
                                div()
                                    .px(px(10.0))
                                    .py(px(6.0))
                                    .rounded(px(8.0))
                                    .bg(danger_soft)
                                    .border_1()
                                    .border_color(danger_border)
                                    .cursor_pointer()
                                    .text_size(px(12.0))
                                    .text_color(danger_text)
                                    .child("Delete history"),
                            ),
                    )
                    .child(
                        div()
                            .px(px(12.0))
                            .py(px(8.0))
                            .rounded(px(9.0))
                            .bg(danger_soft)
                            .border_1()
                            .border_color(danger_border)
                            .when(!can_archive, |b| b.opacity(0.45))
                            .when(can_archive, |b| b.cursor_pointer())
                            .flex()
                            .items_center()
                            .justify_between()
                            .child(
                                div()
                                    .text_size(px(12.5))
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .text_color(danger_text)
                                    .child("Archive Channel"),
                            )
                            .when(state.has_running_tasks, |b| {
                                b.child(
                                    div()
                                        .text_size(px(11.0))
                                        .text_color(warning)
                                        .child("Stop running tasks first"),
                                )
                            }),
                    )
                })
                .when(state.archived, |el| {
                    el.child(
                        div()
                            .px(px(12.0))
                            .py(px(8.0))
                            .rounded(px(9.0))
                            .bg(accent)
                            .cursor_pointer()
                            .flex()
                            .items_center()
                            .justify_center()
                            .text_size(px(12.5))
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(text_on_accent)
                            .child("Restore Channel"),
                    )
                }),
        );

    div()
        .min_w(px(360.0))
        .size_full()
        .bg(bg_sheet)
        .flex()
        .flex_col()
        .child(
            div()
                .h(px(48.0))
                .px(px(16.0))
                .border_b_1()
                .border_color(border_grouped)
                .flex()
                .items_center()
                .justify_between()
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(8.0))
                        .child(tg_icon_rgba(IconName::SettingsGear, 15.0, text_sec))
                        .child(
                            div()
                                .text_size(px(14.0))
                                .font_weight(FontWeight::SEMIBOLD)
                                .text_color(text_pri)
                                .child("Channel Settings"),
                        ),
                )
                .child(
                    div()
                        .text_size(px(11.5))
                        .text_color(text_muted)
                        .child(format!("{} member(s)", state.editor.members.len())),
                ),
        )
        .child(
            div()
                .flex_1()
                .min_h(px(0.0))
                .child(
                    div()
                        .id("openbot-channel-settings-scroll")
                        .size_full()
                        .overflow_y_scroll()
                        .child(content),
                ),
        )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[std::prelude::v1::test]
    fn resolve_channel_icon_handles_unknown_and_invalid_hue() {
        assert_eq!(ICON_CATALOG_COUNT, 20);
        assert_eq!(CHANNEL_ICON_HUES.len(), 10);
        assert_eq!(DEFAULT_CHANNEL_ICON_HUE, 215);
        assert!(resolve_channel_icon("dragon", Some(999)).is_none());
        assert_eq!(
            resolve_channel_icon("shield", Some(999)),
            Some((ChannelIconName::Shield, AvatarHue::Blue))
        );
        assert_eq!(
            resolve_channel_icon("workflow", Some(280)),
            Some((ChannelIconName::Workflow, AvatarHue::Violet))
        );
        assert_eq!(
            resolve_channel_icon("code", None),
            Some((ChannelIconName::Code, AvatarHue::Blue))
        );
    }

    #[std::prelude::v1::test]
    fn channel_editor_auto_lead_crown_insertion_order_and_no_drag_reorder() {
        assert!(!MEMBER_DRAG_REORDER_SUPPORTED);
        let mut editor = ChannelEditorState::new_create();
        editor.set_name("Design Sync");
        assert!(editor.add_member("agent-a").unwrap());
        assert!(editor.add_member("agent-b").unwrap());
        assert!(editor.add_member("agent-c").unwrap());
        assert_eq!(editor.members, vec!["agent-a", "agent-b", "agent-c"]);
        assert_eq!(editor.lead_agent_id.as_deref(), Some("agent-a"));

        editor.promote_lead("agent-c").unwrap();
        assert_eq!(editor.lead_agent_id.as_deref(), Some("agent-c"));

        assert!(editor.remove_member("agent-c"));
        assert_eq!(editor.lead_agent_id.as_deref(), Some("agent-a"));
        assert_eq!(editor.members, vec!["agent-a", "agent-b"]);

        assert_eq!(
            editor.attempt_member_drag_reorder(0, 1),
            Err(ChannelEditorValidationError::MemberDragReorderUnsupported)
        );
        assert_eq!(editor.members, vec!["agent-a", "agent-b"]);
        assert!(editor.validate_draft().is_ok());
    }

    #[std::prelude::v1::test]
    fn channel_editor_picture_exclusivity_and_bounds() {
        let mut editor = ChannelEditorState::new_create();
        assert_eq!(
            editor.validate_draft(),
            Err(ChannelEditorValidationError::EmptyName)
        );
        editor.set_name("Ops Room");
        assert_eq!(
            editor.validate_draft(),
            Err(ChannelEditorValidationError::EmptyMembers)
        );
        editor.add_member("bot-1").unwrap();

        editor.select_icon(ChannelIconName::Shield);
        assert_eq!(
            editor.resolved_icon(),
            Some((ChannelIconName::Shield, AvatarHue::Blue))
        );
        editor.set_uploaded_image_url("https://example.invalid/custom.webp");
        assert!(editor.picture_editor.icon.is_none());
        assert!(editor.active_image_url().is_some());

        editor.select_hue(AvatarHue::Violet);
        assert!(editor.active_image_url().is_none());
        assert_eq!(
            editor.resolved_icon(),
            Some((ChannelIconName::Star, AvatarHue::Violet))
        );

        editor.reset_picture();
        assert!(editor.resolved_icon().is_none());
        assert!(editor.active_image_url().is_none());
    }

    #[std::prelude::v1::test]
    fn settings_pane_browser_auto_reset_archive_guard_and_restore() {
        let mut pane = ChannelSettingsPaneState::default();
        pane.editor.set_name("Recovery");
        pane.editor.add_member("m1").unwrap();
        pane.editor.add_member("m2").unwrap();
        pane.set_browser_policy(ChannelBrowser::Member {
            agent_id: "m2".to_string(),
        })
        .unwrap();
        assert!(pane.remove_member_and_sync_browser("m2"));
        assert_eq!(pane.browser, ChannelBrowser::None);

        pane.has_running_tasks = true;
        assert_eq!(
            pane.archive(),
            Err(ChannelEditorValidationError::ArchiveBlockedByRunningTasks)
        );
        assert!(!pane.archived);

        pane.has_running_tasks = false;
        pane.archive().unwrap();
        assert!(pane.read_only());

        let resumed = pane.restore(&["task-1".to_string(), "task-2".to_string()]);
        assert!(!pane.read_only());
        assert_eq!(resumed, vec!["task-1".to_string(), "task-2".to_string()]);
    }
}

/// Live candidate member row (Fabushi bot or Telegram contact) shown in `ChannelCreateDialogModal`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChannelCreateMemberCandidate {
    pub id: String,
    pub user_id: Option<u64>,
    pub username: Option<String>,
    pub name: String,
    pub description: String,
    pub is_bot: bool,
    pub avatar_seed: Option<String>,
    pub avatar_hue: Option<AvatarHue>,
    pub avatar_color: Option<String>,
    pub avatar_url: Option<String>,
}

/// Submitted payload when the user clicks `Create` in `ChannelCreateDialogModal`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChannelCreateSubmission {
    pub name: String,
    pub member_user_ids: Vec<u64>,
    pub member_usernames: Vec<String>,
    pub member_agent_ids: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChannelCreateInputField {
    Name,
    MemberSearch,
}

/// Interactive OpenBot `ChannelCreateDialog` modal entity (`UI-F11`, `ChannelCreateDialog.tsx`).
/// Owns its own `FocusHandle` and `EntityInputHandler` `canvas` for full macOS/Linux CJK IME.
pub struct ChannelCreateDialogModal {
    pub name: String,
    pub search_query: String,
    pub active_field: ChannelCreateInputField,
    pub candidates: Vec<ChannelCreateMemberCandidate>,
    pub selected_ids: Vec<String>,
    pub validation_error: Option<String>,
    pub palette: crate::theme::TdesktopThemePalette,
    pub language_id: String,
    pub focus_handle: FocusHandle,
    pub ime: crate::components::ui::TextInputController,
    pub pending_submit: Option<ChannelCreateSubmission>,
    pub pending_close: bool,
}

impl ChannelCreateDialogModal {
    pub fn new(
        candidates: Vec<ChannelCreateMemberCandidate>,
        palette: crate::theme::TdesktopThemePalette,
        language_id: impl Into<String>,
        cx: &mut Context<Self>,
    ) -> Self {
        Self {
            name: String::new(),
            search_query: String::new(),
            active_field: ChannelCreateInputField::Name,
            candidates,
            selected_ids: Vec::new(),
            validation_error: None,
            palette,
            language_id: language_id.into(),
            focus_handle: cx.focus_handle(),
            ime: crate::components::ui::TextInputController::new(),
            pending_submit: None,
            pending_close: false,
        }
    }

    pub fn set_theme_and_locale(
        &mut self,
        palette: crate::theme::TdesktopThemePalette,
        language_id: impl Into<String>,
    ) {
        self.palette = palette;
        self.language_id = language_id.into();
    }

    pub fn take_pending_submit(&mut self) -> Option<ChannelCreateSubmission> {
        self.pending_submit.take()
    }

    pub fn take_pending_close(&mut self) -> bool {
        std::mem::take(&mut self.pending_close)
    }

    pub fn can_submit(&self) -> bool {
        !self.name.trim().is_empty() && !self.selected_ids.is_empty()
    }

    pub fn filtered_candidates(&self) -> Vec<&ChannelCreateMemberCandidate> {
        let q = self.search_query.trim().to_ascii_lowercase();
        if q.is_empty() {
            return self.candidates.iter().collect();
        }
        self.candidates
            .iter()
            .filter(|c| {
                c.name.to_ascii_lowercase().contains(&q)
                    || c.description.to_ascii_lowercase().contains(&q)
                    || c.username
                        .as_deref()
                        .is_some_and(|u| u.to_ascii_lowercase().contains(&q))
            })
            .collect()
    }

    pub fn toggle_member(&mut self, id: &str) {
        if let Some(pos) = self.selected_ids.iter().position(|s| s == id) {
            self.selected_ids.remove(pos);
        } else if self.selected_ids.len() < CHANNEL_MEMBERS_MAX {
            self.selected_ids.push(id.to_string());
        }
        self.validation_error = None;
    }

    pub fn request_close(&mut self, cx: &mut Context<Self>) {
        self.ime.clear_ime_state();
        self.pending_close = true;
        cx.notify();
    }

    pub fn try_submit(&mut self, cx: &mut Context<Self>) {
        let trimmed = self.name.trim();
        if trimmed.is_empty() {
            self.validation_error = Some(
                crate::i18n::tr_str(&self.language_id, "请输入群聊名称", "Channel name is required")
                    .to_string(),
            );
            cx.notify();
            return;
        }
        if self.selected_ids.is_empty() {
            self.validation_error = Some(
                crate::i18n::tr_str(
                    &self.language_id,
                    "请至少选择 1 位助手或成员",
                    "Select at least one agent or member",
                )
                .to_string(),
            );
            cx.notify();
            return;
        }
        let mut member_user_ids = Vec::new();
        let mut member_usernames = Vec::new();
        let mut member_agent_ids = Vec::new();
        for sid in &self.selected_ids {
            if let Some(c) = self.candidates.iter().find(|cand| &cand.id == sid) {
                if let Some(uid) = c.user_id {
                    if !member_user_ids.contains(&uid) {
                        member_user_ids.push(uid);
                    }
                }
                if let Some(ref uname) = c.username {
                    let clean = uname.trim().trim_start_matches('@').to_string();
                    if !clean.is_empty() && !member_usernames.contains(&clean) {
                        member_usernames.push(clean);
                    }
                }
                if c.is_bot {
                    member_agent_ids.push(c.id.clone());
                }
            }
        }
        self.ime.clear_ime_state();
        self.pending_submit = Some(ChannelCreateSubmission {
            name: trimmed.to_string(),
            member_user_ids,
            member_usernames,
            member_agent_ids,
        });
        self.pending_close = true;
        cx.notify();
    }

    fn active_text(&self) -> &str {
        match self.active_field {
            ChannelCreateInputField::Name => &self.name,
            ChannelCreateInputField::MemberSearch => &self.search_query,
        }
    }

    fn active_text_mut(&mut self) -> &mut String {
        match self.active_field {
            ChannelCreateInputField::Name => &mut self.name,
            ChannelCreateInputField::MemberSearch => &mut self.search_query,
        }
    }

    fn insert_committed_text(&mut self, raw: &str, cx: &mut Context<Self>) {
        let clean = crate::components::ui::sanitize_paste_text(raw, false);
        if clean.is_empty() {
            return;
        }
        let limit = match self.active_field {
            ChannelCreateInputField::Name => CHANNEL_NAME_MAX_LEN,
            ChannelCreateInputField::MemberSearch => 120,
        };
        let buf = self.active_text_mut();
        for ch in clean.chars() {
            if utf16_len(buf) + ch.len_utf16() <= limit {
                buf.push(ch);
            }
        }
        self.validation_error = None;
        self.ime.clear_preedit();
        cx.notify();
    }
}

impl ChannelCreateDialogModal {
    pub fn handle_key_down(&mut self, event: &KeyDownEvent, cx: &mut Context<Self>) {
        let key = event.keystroke.key.as_str();
        let mods = event.keystroke.modifiers;
        let cmd = mods.platform || mods.control;

        if key.eq_ignore_ascii_case("escape") {
            cx.stop_propagation();
            self.request_close(cx);
            return;
        }
        if key.eq_ignore_ascii_case("tab") {
            cx.stop_propagation();
            self.ime.clear_ime_state();
            self.active_field = match self.active_field {
                ChannelCreateInputField::Name => ChannelCreateInputField::MemberSearch,
                ChannelCreateInputField::MemberSearch => ChannelCreateInputField::Name,
            };
            cx.notify();
            return;
        }
        if key.eq_ignore_ascii_case("enter") {
            cx.stop_propagation();
            if self.ime.has_preedit() {
                if let Some(p) = self.ime.ime_preedit.take().filter(|s| !s.is_empty()) {
                    self.insert_committed_text(&p, cx);
                }
                return;
            }
            if self.can_submit() {
                self.try_submit(cx);
            }
            return;
        }
        if cmd && !mods.shift && !mods.alt && key.eq_ignore_ascii_case("v") {
            cx.stop_propagation();
            if let Some(clip) = cx.read_from_clipboard() {
                if let Some(text) = clip.text() {
                    self.insert_committed_text(&text, cx);
                }
            }
            return;
        }
        if key.eq_ignore_ascii_case("backspace") {
            cx.stop_propagation();
            if self.ime.has_preedit() {
                self.ime.clear_preedit();
                cx.notify();
                return;
            }
            self.active_text_mut().pop();
            self.validation_error = None;
            cx.notify();
            return;
        }
        if !cmd && !mods.alt {
            if self.ime.has_preedit() || self.ime.take_just_committed() {
                return;
            }
            if key == "space" {
                cx.stop_propagation();
                self.insert_committed_text(" ", cx);
            } else if let Some(ref kc) = event.keystroke.key_char {
                if !kc.chars().all(|c| c.is_control()) {
                    cx.stop_propagation();
                    self.insert_committed_text(kc, cx);
                }
            }
        }
    }
}

impl EntityInputHandler for ChannelCreateDialogModal {
    fn text_for_range(
        &mut self,
        range_utf16: std::ops::Range<usize>,
        _adjusted_range: &mut Option<std::ops::Range<usize>>,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<String> {
        let base = self.active_text();
        let full = self
            .ime
            .ime_preedit
            .as_deref()
            .map_or_else(|| base.to_string(), |p| format!("{base}{p}"));
        let start = crate::components::ui::utf16_offset_to_utf8(&full, range_utf16.start);
        let end = crate::components::ui::utf16_offset_to_utf8(&full, range_utf16.end).max(start);
        Some(full.get(start..end).unwrap_or("").to_string())
    }

    fn selected_text_range(
        &mut self,
        _ignore_disabled_input: bool,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<UTF16Selection> {
        let base = self.active_text();
        let range = if self.ime.has_preedit() {
            self.ime.selected_utf16_range(base)?
        } else {
            let total = crate::components::ui::utf16_len(base);
            total..total
        };
        Some(UTF16Selection {
            range,
            reversed: false,
        })
    }

    fn marked_text_range(
        &self,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<std::ops::Range<usize>> {
        if self.ime.has_preedit() {
            self.ime.marked_utf16_range(self.active_text())
        } else {
            None
        }
    }

    fn unmark_text(&mut self, _window: &mut Window, cx: &mut Context<Self>) {
        if let Some(p) = self.ime.ime_preedit.take().filter(|s| !s.is_empty()) {
            self.insert_committed_text(&p, cx);
            self.ime.mark_ime_committed();
            return;
        }
        self.ime.clear_ime_state();
        cx.notify();
    }

    fn replace_text_in_range(
        &mut self,
        _range_utf16: Option<std::ops::Range<usize>>,
        text: &str,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let was_composing = self.ime.has_preedit();
        self.insert_committed_text(text, cx);
        if was_composing {
            self.ime.mark_ime_committed();
        }
    }

    fn replace_and_mark_text_in_range(
        &mut self,
        _range_utf16: Option<std::ops::Range<usize>>,
        new_text: &str,
        _new_selected_range_utf16: Option<std::ops::Range<usize>>,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(committed) = self.ime.handle_replace_and_mark(new_text) {
            self.insert_committed_text(&committed, cx);
            self.ime.mark_ime_committed();
        } else {
            cx.notify();
        }
    }

    fn bounds_for_range(
        &mut self,
        _range_utf16: std::ops::Range<usize>,
        element_bounds: Bounds<Pixels>,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<Bounds<Pixels>> {
        let chars = self.active_text().chars().count()
            + self
                .ime
                .ime_preedit
                .as_deref()
                .map_or(0, |p| p.chars().count());
        let caret_x = element_bounds.origin.x + px(24.0 + (chars as f32 * 8.0).min(240.0));
        let y_off = match self.active_field {
            ChannelCreateInputField::Name => 92.0,
            ChannelCreateInputField::MemberSearch => 176.0,
        };
        Some(Bounds::new(
            point(caret_x, element_bounds.origin.y + px(y_off)),
            size(px(2.0), px(20.0)),
        ))
    }

    fn character_index_for_point(
        &mut self,
        _point: Point<Pixels>,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<usize> {
        Some(crate::components::ui::utf16_len(self.active_text()))
    }
}

impl Render for ChannelCreateDialogModal {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let ob = self.palette.openbot_palette();
        let is_dark = self.palette.mode.is_dark();
        let lang = self.language_id.as_str();
        let is_zh = lang.starts_with("zh");
        let vp_h = f32::from(window.viewport_size().height).max(480.0);
        let scrim = if is_dark {
            hsla(0.0, 0.0, 0.0, 0.52)
        } else {
            hsla(0.0, 0.0, 0.0, 0.28)
        };
        let input_bg: Rgba = if is_dark {
            rgb(0x1E1E1E).into()
        } else {
            ob.bg_canvas()
        };
        let hover_bg = ob.hover();
        let focus_border = ob.text_dim();
        let idle_border = ob.border_strong_hairline();

        let ime_entity = cx.entity().clone();
        let ime_focus = self.focus_handle.clone();
        let ime_canvas = canvas(
            move |_, _, _| {},
            move |bounds, _, window, cx| {
                window.handle_input(
                    &ime_focus,
                    ElementInputHandler::new(bounds, ime_entity.clone()),
                    cx,
                );
            },
        )
        .absolute()
        .inset_0()
        .size_full();

        let preedit = self.ime.ime_preedit.as_deref().unwrap_or("");
        let name_focused = self.active_field == ChannelCreateInputField::Name;
        let search_focused = self.active_field == ChannelCreateInputField::MemberSearch;
        let name_display = if name_focused && !preedit.is_empty() {
            format!("{}{}", self.name, preedit)
        } else {
            self.name.clone()
        };
        let search_display = if search_focused && !preedit.is_empty() {
            format!("{}{}", self.search_query, preedit)
        } else {
            self.search_query.clone()
        };

        let filtered: Vec<ChannelCreateMemberCandidate> =
            self.filtered_candidates().into_iter().cloned().collect();
        let selected_count = self.selected_ids.len();
        let can_submit = self.can_submit();

        let mut list = div()
            .id("channel-create-member-scroll")
            .w_full()
            .max_h(px(320.0))
            .overflow_y_scroll()
            .flex()
            .flex_col()
            .gap(px(2.0));

        if filtered.is_empty() {
            list = list.child(
                div()
                    .py(px(24.0))
                    .flex()
                    .items_center()
                    .justify_center()
                    .text_size(px(13.0))
                    .text_color(ob.text_muted())
                    .child(if self.candidates.is_empty() {
                        if is_zh { "暂无可添加的助手或联系人" } else { "No agents or contacts available" }
                    } else if is_zh {
                        "未找到匹配的成员"
                    } else {
                        "No matching agents or contacts"
                    }),
            );
        } else {
            for (idx, cand) in filtered.into_iter().enumerate() {
                let is_checked = self.selected_ids.iter().any(|s| s == &cand.id);
                let cid = cand.id.clone();
                let avatar_el: AnyElement = if cand.is_bot {
                    crate::components::avatar::render_plush_bot_avatar(
                        cand.avatar_seed.as_deref().unwrap_or(&cand.name),
                        None,
                        cand.avatar_hue,
                        cand.avatar_color.as_deref(),
                        24.0,
                        AvatarMood::Idle,
                        None,
                    )
                    .into_any_element()
                } else {
                    let initial: String = cand
                        .name
                        .chars()
                        .filter(|c| !c.is_whitespace())
                        .take(1)
                        .collect::<String>()
                        .to_uppercase();
                    div()
                        .w(px(24.0))
                        .h(px(24.0))
                        .rounded_full()
                        .flex_shrink_0()
                        .bg(rgb(self.palette.peer_avatar_color(cand.user_id.unwrap_or(idx as u64) as i64)))
                        .flex()
                        .items_center()
                        .justify_center()
                        .text_size(px(10.5))
                        .font_weight(FontWeight::BOLD)
                        .text_color(rgb(0xFFFFFF))
                        .child(if initial.is_empty() { "?".to_string() } else { initial })
                        .into_any_element()
                };
                let check_bg = if is_checked { ob.surface_light() } else { input_bg };
                let row_bg = if is_checked { ob.accent_soft() } else { hsla(0.0, 0.0, 0.0, 0.0).into() };
                list = list.child(
                    div()
                        .id(("channel-create-cand-row", idx))
                        .w_full()
                        .h(px(40.0))
                        .px(px(8.0))
                        .rounded(px(8.0))
                        .bg(row_bg)
                        .hover(move |s| s.bg(hover_bg))
                        .cursor_pointer()
                        .flex()
                        .items_center()
                        .gap(px(9.0))
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(move |this, _, _, cx| {
                                cx.stop_propagation();
                                this.toggle_member(&cid);
                                cx.notify();
                            }),
                        )
                        .child(
                            div()
                                .w(px(16.0))
                                .h(px(16.0))
                                .flex_shrink_0()
                                .rounded(px(4.0))
                                .bg(check_bg)
                                .border_1()
                                .border_color(if is_checked { ob.surface_light() } else { idle_border })
                                .flex()
                                .items_center()
                                .justify_center()
                                .when(is_checked, |b| {
                                    b.child(crate::views::stage::openbot_stage_chrome::openbot_stage_icon(
                                        crate::views::stage::openbot_stage_chrome::OpenBotStageIcon::Check,
                                        11.0,
                                        ob.text_on_light(),
                                    ))
                                }),
                        )
                        .child(avatar_el)
                        .child(
                            div()
                                .flex_1()
                                .min_w(px(0.0))
                                .flex()
                                .flex_col()
                                .child(
                                    div()
                                        .overflow_hidden()
                                        .whitespace_nowrap()
                                        .text_ellipsis()
                                        .text_size(px(13.0))
                                        .font_weight(FontWeight::MEDIUM)
                                        .text_color(ob.text_primary())
                                        .child(cand.name),
                                )
                                .when(!cand.description.trim().is_empty(), |col| {
                                    col.child(
                                        div()
                                            .overflow_hidden()
                                            .whitespace_nowrap()
                                            .text_ellipsis()
                                            .text_size(px(11.5))
                                            .text_color(ob.text_muted())
                                            .child(cand.description),
                                    )
                                }),
                        )
                        .when(cand.is_bot, |row| {
                            row.child(
                                div()
                                    .flex_shrink_0()
                                    .px(px(6.0))
                                    .py(px(1.5))
                                    .rounded(px(5.0))
                                    .bg(ob.bg_control_active())
                                    .text_size(px(10.5))
                                    .text_color(ob.text_muted())
                                    .child(if is_zh { "助手" } else { "Agent" }),
                            )
                        }),
                );
            }
        }

        let card = div()
            .id("openbot-channel-create-dialog-card")
            .relative()
            .w(px(440.0))
            .max_w_full()
            .max_h(px((vp_h - 48.0).max(360.0)))
            .rounded(px(14.0))
            .bg(ob.bg_surface())
            .border_1()
            .border_color(ob.border_strong_hairline())
            .shadow(self.palette.popover_shadows())
            .occlude()
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .flex()
            .flex_col()
            .child(ime_canvas)
            .child(
                div()
                    .px(px(16.0))
                    .py(px(14.0))
                    .border_b_1()
                    .border_color(ob.border_hairline())
                    .flex()
                    .items_center()
                    .justify_between()
                    .child(
                        div()
                            .text_size(px(16.0))
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(ob.text_primary())
                            .child(if is_zh { "新建群聊" } else { "New channel" }),
                    )
                    .child(
                        div()
                            .id("channel-create-close-btn")
                            .w(px(28.0))
                            .h(px(28.0))
                            .rounded(px(6.0))
                            .hover(move |s| s.bg(hover_bg))
                            .cursor_pointer()
                            .flex()
                            .items_center()
                            .justify_center()
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(|this, _, _, cx| {
                                    cx.stop_propagation();
                                    this.request_close(cx);
                                }),
                            )
                            .child(crate::views::stage::openbot_stage_chrome::openbot_stage_icon(
                                crate::views::stage::openbot_stage_chrome::OpenBotStageIcon::Close,
                                15.0,
                                ob.text_muted(),
                            )),
                    ),
            )
            .child(
                div()
                    .p(px(16.0))
                    .flex()
                    .flex_col()
                    .gap(px(14.0))
                    .when_some(self.validation_error.clone(), |col, err| {
                        col.child(
                            div()
                                .px(px(10.0))
                                .py(px(7.0))
                                .rounded(px(8.0))
                                .bg(ob.rgba("--openbot-danger-soft").unwrap_or_else(|| ob.danger()))
                                .text_size(px(12.5))
                                .text_color(ob.danger())
                                .child(err),
                        )
                    })
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .gap(px(6.0))
                            .child(
                                div()
                                    .text_size(px(12.5))
                                    .font_weight(FontWeight::MEDIUM)
                                    .text_color(ob.text_secondary())
                                    .child(if is_zh { "名称 *" } else { "Name *" }),
                            )
                            .child(
                                div()
                                    .id("channel-create-name-input")
                                    .h(px(38.0))
                                    .px(px(12.0))
                                    .rounded(px(8.0))
                                    .bg(input_bg)
                                    .border_1()
                                    .border_color(if name_focused { focus_border } else { idle_border })
                                    .cursor_text()
                                    .flex()
                                    .items_center()
                                    .on_mouse_down(
                                        MouseButton::Left,
                                        cx.listener(|this, _, window, cx| {
                                            cx.stop_propagation();
                                            this.ime.clear_ime_state();
                                            this.active_field = ChannelCreateInputField::Name;
                                            window.focus(&this.focus_handle);
                                            cx.notify();
                                        }),
                                    )
                                    .child(
                                        div()
                                            .flex_1()
                                            .min_w(px(0.0))
                                            .overflow_hidden()
                                            .whitespace_nowrap()
                                            .text_ellipsis()
                                            .text_size(px(14.0))
                                            .text_color(if name_display.is_empty() {
                                                ob.text_dim()
                                            } else {
                                                ob.text_primary()
                                            })
                                            .child(if name_display.is_empty() {
                                                if is_zh {
                                                    "例如：产品设计讨论组".to_string()
                                                } else {
                                                    "e.g. Design Sync".to_string()
                                                }
                                            } else if name_focused {
                                                format!("{}|", name_display)
                                            } else {
                                                name_display
                                            }),
                                    ),
                            ),
                    )
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .gap(px(8.0))
                            .child(
                                div()
                                    .flex()
                                    .items_center()
                                    .justify_between()
                                    .child(
                                        div()
                                            .text_size(px(12.5))
                                            .font_weight(FontWeight::MEDIUM)
                                            .text_color(ob.text_secondary())
                                            .child(if is_zh { "添加助手或成员 *" } else { "Add agents *" }),
                                    )
                                    .when(selected_count > 0, |r| {
                                        r.child(
                                            div()
                                                .text_size(px(11.5))
                                                .text_color(ob.text_muted())
                                                .child(if is_zh {
                                                    format!("已选 {selected_count}")
                                                } else {
                                                    format!("{selected_count} selected")
                                                }),
                                        )
                                    }),
                            )
                            .child(
                                div()
                                    .id("channel-create-member-search")
                                    .h(px(34.0))
                                    .px(px(10.0))
                                    .rounded(px(8.0))
                                    .bg(input_bg)
                                    .border_1()
                                    .border_color(if search_focused { focus_border } else { idle_border })
                                    .cursor_text()
                                    .flex()
                                    .items_center()
                                    .gap(px(7.0))
                                    .on_mouse_down(
                                        MouseButton::Left,
                                        cx.listener(|this, _, window, cx| {
                                            cx.stop_propagation();
                                            this.ime.clear_ime_state();
                                            this.active_field = ChannelCreateInputField::MemberSearch;
                                            window.focus(&this.focus_handle);
                                            cx.notify();
                                        }),
                                    )
                                    .child(crate::views::stage::openbot_stage_chrome::openbot_stage_icon(
                                        crate::views::stage::openbot_stage_chrome::OpenBotStageIcon::Search,
                                        14.0,
                                        ob.text_muted(),
                                    ))
                                    .child(
                                        div()
                                            .flex_1()
                                            .min_w(px(0.0))
                                            .overflow_hidden()
                                            .whitespace_nowrap()
                                            .text_ellipsis()
                                            .text_size(px(13.0))
                                            .text_color(if search_display.is_empty() {
                                                ob.text_dim()
                                            } else {
                                                ob.text_primary()
                                            })
                                            .child(if search_display.is_empty() {
                                                if is_zh {
                                                    "搜索助手或联系人…".to_string()
                                                } else {
                                                    "Search agents or contacts…".to_string()
                                                }
                                            } else if search_focused {
                                                format!("{}|", search_display)
                                            } else {
                                                search_display
                                            }),
                                    ),
                            )
                            .child(list),
                    ),
            )
            .child(
                div()
                    .px(px(16.0))
                    .py(px(12.0))
                    .border_t_1()
                    .border_color(ob.border_hairline())
                    .flex()
                    .items_center()
                    .justify_end()
                    .gap(px(8.0))
                    .child(
                        div()
                            .id("channel-create-cancel-btn")
                            .h(px(32.0))
                            .px(px(14.0))
                            .rounded(px(8.0))
                            .hover(move |s| s.bg(hover_bg))
                            .cursor_pointer()
                            .flex()
                            .items_center()
                            .text_size(px(13.0))
                            .text_color(ob.text_muted())
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(|this, _, _, cx| {
                                    cx.stop_propagation();
                                    this.request_close(cx);
                                }),
                            )
                            .child(if is_zh { "取消" } else { "Cancel" }),
                    )
                    .child(
                        div()
                            .id("channel-create-submit-btn")
                            .h(px(32.0))
                            .px(px(16.0))
                            .rounded(px(8.0))
                            .bg(ob.surface_light())
                            .when(!can_submit, |el| el.opacity(0.45))
                            .when(can_submit, |el| {
                                el.cursor_pointer().hover(|s| s.opacity(0.88)).on_mouse_down(
                                    MouseButton::Left,
                                    cx.listener(|this, _, _, cx| {
                                        cx.stop_propagation();
                                        this.try_submit(cx);
                                    }),
                                )
                            })
                            .flex()
                            .items_center()
                            .text_size(px(13.0))
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(ob.text_on_light())
                            .child(if is_zh { "创建" } else { "Create" }),
                    ),
            );

        div()
            .id("openbot-channel-create-backdrop")
            .track_focus(&self.focus_handle)
            .key_context("ChannelCreateDialogModal")
            .on_key_down(cx.listener(|this, event: &KeyDownEvent, _, cx| {
                this.handle_key_down(event, cx);
            }))
            .absolute()
            .inset_0()
            .bg(scrim)
            .occlude()
            .flex()
            .items_center()
            .justify_center()
            .p(px(24.0))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, _, _, cx| {
                    cx.stop_propagation();
                    this.request_close(cx);
                }),
            )
            .child(card)
    }
}
