//! 1:1 OpenBot Multi-Bot Group-Chat `WorkspaceChannelConversation`, `AwaitingReplies`,
//! `ChannelActivityIndicator`, `ChannelStoppedTasks` Loop Recovery, and Voice Attribution (`OB-F17`).
//! Mirrors `ChannelConversation.tsx`, `channel-conversation-rows.ts`, `ChannelActivityIndicator.tsx`,
//! `AwaitingReplies.tsx`, and `ChannelStoppedTasks.tsx`.

use std::collections::{BTreeMap, HashSet};

use gpui::prelude::FluentBuilder;
use gpui::*;
use openbot_contracts::avatar::{AvatarMood, avatar_head_color};
use openbot_contracts::channels::{
    CHANNEL_ASSIGNMENT_LIMIT_ERROR, Channel, ChannelIcon, ChannelMember, ChannelMessage,
    ChannelMessageAuthor, ChannelMessageAuthorKind, ChannelTask, ChannelTaskState, ChannelTaskWait,
};
use openbot_contracts::conversation::ConversationMessageStatus;
use openbot_contracts::mentions::without_own_name_label;
use serde::{Deserialize, Serialize};

use crate::components::bloub_avatar::{
    AgentAvatarProps, CHANNEL_ACTIVITY_OVERLAP_PX as BLOUB_ACTIVITY_OVERLAP_PX, CHANNEL_ROW_LIMIT,
    CHANNEL_ROW_OVERLAP_PX, ChannelActivityIndicatorState, ChannelAgentBrief, ChannelAvatarLayout,
    ChannelClusterEval, ChannelWorkerBrief, SidebarPlushRingKind, channel_activity_sentence,
    render_agent_avatar, render_channel_avatar, resolve_channel_cluster_sized,
};
use crate::theme::openbot_palette::{
    OpenBotColorScheme, OpenBotPalette, parse_css_color_to_rgba,
};
use crate::theme::{IconName, tg_icon_rgba};

/// Maximum overlapping avatars rendered in the channel conversation header row (`3`).
pub const CHANNEL_HEADER_ROW_LIMIT: usize = CHANNEL_ROW_LIMIT;
/// Horizontal negative overlap between adjacent header row member avatars (`-16.0px`).
pub const CHANNEL_HEADER_OVERLAP_PX: f32 = CHANNEL_ROW_OVERLAP_PX;
/// Horizontal negative overlap between adjacent active/departing worker avatars (`-8.0px`).
pub const CHANNEL_ACTIVITY_OVERLAP_PX: f32 = BLOUB_ACTIVITY_OVERLAP_PX;
/// Automatic transfer/reassignment loop threshold (`8`).
pub const CHANNEL_ASSIGNMENT_LIMIT: u64 = openbot_contracts::channels::CHANNEL_ASSIGNMENT_LIMIT;
/// Maximum concurrent active channel tasks before queuing with `ChannelFull` (`2`).
pub const CHANNEL_PARALLEL_LIMIT: usize = openbot_contracts::channels::CHANNEL_PARALLEL_LIMIT;
/// Maximum bot-to-bot `channel_reply` replies per parent task (`2`).
pub const CHANNEL_REPLY_LIMIT: usize = openbot_contracts::channels::CHANNEL_REPLY_LIMIT;
/// Maximum characters for a single bot-to-bot `channel_reply` message (`2_000`).
pub const CHANNEL_REPLY_CHARACTERS: usize = openbot_contracts::channels::CHANNEL_REPLY_CHARACTERS;
/// Minimum conversation viewport width (`424px`).
pub const CHANNEL_CONVERSATION_MIN_WIDTH_PX: f32 = 424.0;

/// Single coordinator routing event before consecutive-action merging.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChannelRoutingEvent {
    pub action: String,
    pub agent_id: String,
}

/// Merged compact `channel-routing` divider row (`channel-conversation-rows.ts:joinRoutingRows`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MergedRoutingRow {
    pub action: String,
    pub agent_ids: Vec<String>,
}

/// Merges adjacent `ChannelRoutingEvent`s that share an identical `action`, deduplicating
/// repeated `agent_id`s in encounter order while never merging across distinct actions.
pub fn merge_consecutive_routing_events(events: &[ChannelRoutingEvent]) -> Vec<MergedRoutingRow> {
    let mut merged: Vec<MergedRoutingRow> = Vec::new();
    for event in events {
        if let Some(last) = merged.last_mut() {
            if last.action == event.action {
                if !last.agent_ids.iter().any(|id| id == &event.agent_id) {
                    last.agent_ids.push(event.agent_id.clone());
                }
                continue;
            }
        }
        merged.push(MergedRoutingRow {
            action: event.action.clone(),
            agent_ids: vec![event.agent_id.clone()],
        });
    }
    merged
}

/// Formats a slice of display names into a natural English list (`"A"`, `"A and B"`, `"A, B, and C"`).
pub fn format_human_list(names: &[String]) -> String {
    match names {
        [] => String::new(),
        [only] => only.clone(),
        [first, second] => format!("{first} and {second}"),
        [head @ .., last] => format!("{}, and {last}", head.join(", ")),
    }
}

/// Exact 5 states tracked per awaited agent in `AwaitingReplies` (`AwaitingReplies.tsx:L10-L259`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum AwaitingReplyState {
    Asked,
    Working,
    Replied,
    Failed,
    Stopped,
}

impl AwaitingReplyState {
    pub const ALL: [Self; 5] = [
        Self::Asked,
        Self::Working,
        Self::Replied,
        Self::Failed,
        Self::Stopped,
    ];

    pub const fn is_settled(self) -> bool {
        matches!(self, Self::Replied | Self::Failed | Self::Stopped)
    }

    pub const fn is_active(self) -> bool {
        matches!(self, Self::Asked | Self::Working)
    }

    pub const fn is_failure(self) -> bool {
        matches!(self, Self::Failed | Self::Stopped)
    }

    pub const fn label(self) -> &'static str {
        match self {
            Self::Asked => "Asked",
            Self::Working => "Working",
            Self::Replied => "Replied",
            Self::Failed => "Failed",
            Self::Stopped => "Stopped",
        }
    }
}

/// Per-agent entry inside the collapsible `AwaitingReplies` progress bar.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AwaitingReplyEntry {
    pub agent_id: String,
    pub agent_name: String,
    pub state: AwaitingReplyState,
    pub preview: Option<String>,
    pub delivery_id: Option<String>,
    pub waiting_reason: Option<String>,
}

impl AwaitingReplyEntry {
    pub fn can_answer(&self) -> bool {
        self.state.is_active()
            && self
                .waiting_reason
                .as_deref()
                .is_some_and(|r| !r.trim().is_empty())
    }

    pub fn can_steer(&self) -> bool {
        self.state == AwaitingReplyState::Replied
            && self
                .delivery_id
                .as_deref()
                .is_some_and(|id| !id.trim().is_empty())
    }

    pub fn can_stop(&self) -> bool {
        self.state.is_active()
    }
}

/// Interactive row and header actions emitted by `AwaitingRepliesTracker`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum AwaitingReplyAction {
    Answer { agent_id: String },
    Steer { agent_id: String, delivery_id: String },
    Stop { agent_id: String },
    Dismiss,
}

/// Collapsible multi-bot `AwaitingReplies` tracker (automatically hidden when `archived == true` or empty).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct AwaitingRepliesTracker {
    pub entries: Vec<AwaitingReplyEntry>,
    pub collapsed: bool,
    pub dismissed: bool,
}

impl AwaitingRepliesTracker {
    pub fn new(entries: Vec<AwaitingReplyEntry>) -> Self {
        Self {
            entries,
            collapsed: false,
            dismissed: false,
        }
    }

    pub fn upsert_entry(&mut self, entry: AwaitingReplyEntry) {
        if entry.state.is_active() {
            self.dismissed = false;
        }
        if let Some(existing) = self
            .entries
            .iter_mut()
            .find(|item| item.agent_id == entry.agent_id)
        {
            *existing = entry;
        } else {
            self.entries.push(entry);
        }
    }

    /// Returns `(done, total)` settled reply counts across all entries.
    pub fn progress(&self) -> (usize, usize) {
        let done = self.entries.iter().filter(|e| e.state.is_settled()).count();
        (done, self.entries.len())
    }

    pub fn replied_count(&self) -> usize {
        self.entries
            .iter()
            .filter(|e| e.state == AwaitingReplyState::Replied)
            .count()
    }

    pub fn failed_or_stopped_count(&self) -> usize {
        self.entries.iter().filter(|e| e.state.is_failure()).count()
    }

    pub fn all_settled(&self) -> bool {
        !self.entries.is_empty() && self.entries.iter().all(|e| e.state.is_settled())
    }

    /// Hidden whenever `archived == true`, explicitly dismissed, or empty.
    pub fn visible(&self, archived: bool) -> bool {
        !archived && !self.dismissed && !self.entries.is_empty()
    }

    pub fn header_status_text(&self) -> String {
        if let Some(waiting) = self
            .entries
            .iter()
            .find_map(|e| e.waiting_reason.as_deref().filter(|r| !r.trim().is_empty()))
        {
            return waiting.to_string();
        }
        if self.all_settled() {
            let failed = self.failed_or_stopped_count();
            if failed > 0 {
                format!("{failed} stopped or failed")
            } else {
                "All replied".to_string()
            }
        } else {
            "Working".to_string()
        }
    }

    pub fn toggle_collapsed(&mut self) {
        self.collapsed = !self.collapsed;
    }

    /// Dismisses the tracker bar once every awaited reply is settled (`Replied | Failed | Stopped`).
    pub fn dismiss(&mut self) -> bool {
        if self.all_settled() {
            self.dismissed = true;
            true
        } else {
            false
        }
    }

    pub fn mark_stopped(&mut self, agent_id: &str) -> bool {
        if let Some(entry) = self
            .entries
            .iter_mut()
            .find(|e| e.agent_id == agent_id && e.state.is_active())
        {
            entry.state = AwaitingReplyState::Stopped;
            entry.waiting_reason = None;
            true
        } else {
            false
        }
    }

    pub fn available_actions_for(&self, agent_id: &str) -> Vec<AwaitingReplyAction> {
        let mut actions = Vec::new();
        if let Some(entry) = self.entries.iter().find(|e| e.agent_id == agent_id) {
            if entry.can_answer() {
                actions.push(AwaitingReplyAction::Answer {
                    agent_id: entry.agent_id.clone(),
                });
            }
            if entry.can_steer() {
                if let Some(delivery_id) = &entry.delivery_id {
                    actions.push(AwaitingReplyAction::Steer {
                        agent_id: entry.agent_id.clone(),
                        delivery_id: delivery_id.clone(),
                    });
                }
            }
            if entry.can_stop() {
                actions.push(AwaitingReplyAction::Stop {
                    agent_id: entry.agent_id.clone(),
                });
            }
        }
        if self.all_settled() {
            actions.push(AwaitingReplyAction::Dismiss);
        }
        actions
    }
}

/// Exact 3 recovery actions rendered on frosted-glass `ChannelStoppedTasks` cards.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum StoppedTaskRecoveryAction {
    Continue,
    Reassign,
    Cancel,
}

/// Canonical UI button labels for stopped channel task recovery.
pub const STOPPED_TASK_RECOVERY_ACTION_LABELS: [&str; 3] = ["Continue", "Reassign", "Cancel"];

/// Stopped or failed root channel task rendered above the composer (`ChannelStoppedTasks.tsx`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StoppedChannelTaskBannerItem {
    pub task_id: String,
    pub owner_agent_id: Option<String>,
    pub error: Option<String>,
    pub run_agent_ids: Vec<String>,
    pub is_assignment_loop: bool,
}

impl StoppedChannelTaskBannerItem {
    pub fn from_channel_task(task: &ChannelTask, run_agent_ids: Vec<String>) -> Self {
        let is_assignment_loop = task.assignment_count >= CHANNEL_ASSIGNMENT_LIMIT
            || task.error.as_deref() == Some(CHANNEL_ASSIGNMENT_LIMIT_ERROR);
        Self {
            task_id: task.id.clone(),
            owner_agent_id: task.owner_agent_id.clone(),
            error: task.error.clone(),
            run_agent_ids,
            is_assignment_loop,
        }
    }

    /// `Continue` is hidden if `owner_agent_id` is absent or has been removed from channel `members`.
    pub fn can_continue(&self, members: &[String]) -> bool {
        self.owner_agent_id
            .as_deref()
            .is_some_and(|owner| members.iter().any(|m| m == owner))
    }

    /// Returns eligible reassign targets from `members`, strictly excluding `owner_agent_id`.
    pub fn reassign_candidates(&self, members: &[String]) -> Vec<String> {
        let excluded = self.owner_agent_id.as_deref().unwrap_or("");
        ChannelStoppedTasksState::reassign_candidates(members, excluded)
    }

    /// Formats the stop/loop reason; on `CHANNEL_ASSIGNMENT_LIMIT` loops, explicitly names all `run_agent_ids`.
    pub fn formatted_reason(&self, agent_names: &BTreeMap<String, String>) -> String {
        if self.is_assignment_loop {
            let mut seen = HashSet::new();
            let mut names = Vec::new();
            for id in &self.run_agent_ids {
                if seen.insert(id.as_str()) {
                    let display = agent_names
                        .get(id)
                        .cloned()
                        .unwrap_or_else(|| id.clone());
                    names.push(display);
                }
            }
            if names.is_empty() {
                if let Some(owner) = &self.owner_agent_id {
                    names.push(
                        agent_names
                            .get(owner)
                            .cloned()
                            .unwrap_or_else(|| owner.clone()),
                    );
                }
            }
            let participants = if names.is_empty() {
                "channel members".to_string()
            } else {
                format_human_list(&names)
            };
            format!(
                "Automatic assignment limit ({CHANNEL_ASSIGNMENT_LIMIT}) reached across {participants}. Continue or reassign this task."
            )
        } else {
            self.error
                .as_deref()
                .filter(|s| !s.trim().is_empty())
                .unwrap_or("This channel task stopped and needs attention.")
                .to_string()
        }
    }
}

/// Controller for frosted-glass `ChannelStoppedTasks` loop recovery cards above the composer.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChannelStoppedTasksState {
    pub items: Vec<StoppedChannelTaskBannerItem>,
    pub open_reassign_dropdown_task_id: Option<String>,
    pub can_cancel: bool,
}

impl Default for ChannelStoppedTasksState {
    fn default() -> Self {
        Self {
            items: Vec::new(),
            open_reassign_dropdown_task_id: None,
            can_cancel: true,
        }
    }
}

impl ChannelStoppedTasksState {
    pub fn new(items: Vec<StoppedChannelTaskBannerItem>) -> Self {
        Self {
            items,
            open_reassign_dropdown_task_id: None,
            can_cancel: true,
        }
    }

    /// Hydrates stopped/failed root task banners from contract [`ChannelTask`] rows.
    pub fn from_channel_tasks(tasks: &[ChannelTask]) -> Self {
        let mut by_root: BTreeMap<&str, Vec<String>> = BTreeMap::new();
        for task in tasks {
            if let Some(owner) = &task.owner_agent_id {
                let entry = by_root.entry(task.root_task_id.as_str()).or_default();
                if !entry.iter().any(|id| id == owner) {
                    entry.push(owner.clone());
                }
            }
        }
        let items = tasks
            .iter()
            .filter(|t| {
                t.parent_task_id.is_none()
                    && matches!(t.state, ChannelTaskState::Paused | ChannelTaskState::Failed)
            })
            .map(|t| {
                let run_agents = by_root.get(t.root_task_id.as_str()).cloned().unwrap_or_default();
                StoppedChannelTaskBannerItem::from_channel_task(t, run_agents)
            })
            .collect();
        Self::new(items)
    }

    pub fn is_visible(&self, archived: bool) -> bool {
        !archived && !self.items.is_empty()
    }

    /// Returns eligible reassign targets from `members`, strictly excluding `owner_agent_id`.
    pub fn reassign_candidates(members: &[String], owner_agent_id: &str) -> Vec<String> {
        let mut seen = HashSet::new();
        let mut candidates = Vec::new();
        for member in members {
            if member != owner_agent_id && seen.insert(member.as_str()) {
                candidates.push(member.clone());
            }
        }
        candidates
    }

    pub const fn action_labels() -> [&'static str; 3] {
        STOPPED_TASK_RECOVERY_ACTION_LABELS
    }

    pub fn toggle_reassign_dropdown(&mut self, task_id: &str) {
        if self.open_reassign_dropdown_task_id.as_deref() == Some(task_id) {
            self.open_reassign_dropdown_task_id = None;
        } else {
            self.open_reassign_dropdown_task_id = Some(task_id.to_string());
        }
    }

    pub fn close_reassign_dropdown(&mut self) {
        self.open_reassign_dropdown_task_id = None;
    }

    pub fn remove_task(&mut self, task_id: &str) {
        self.items.retain(|item| item.task_id != task_id);
        if self.open_reassign_dropdown_task_id.as_deref() == Some(task_id) {
            self.open_reassign_dropdown_task_id = None;
        }
    }

    /// Validates a `Reassign` action against channel membership and owner exclusion.
    pub fn validate_reassign(
        &self,
        task_id: &str,
        members: &[String],
        target_agent_id: &str,
    ) -> Result<(), String> {
        let Some(item) = self.items.iter().find(|i| i.task_id == task_id) else {
            return Err(format!("Stopped task '{task_id}' not found."));
        };
        if !members.iter().any(|m| m == target_agent_id) {
            return Err(format!(
                "Agent '{target_agent_id}' is not a member of this channel."
            ));
        }
        if item.owner_agent_id.as_deref() == Some(target_agent_id) {
            return Err(format!(
                "Agent '{target_agent_id}' already owns task '{task_id}'."
            ));
        }
        Ok(())
    }
}

/// Queue / parallel-limit wait notices rendered above the channel composer (`ChannelTaskWait`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum ChannelTaskWaitNotice {
    ChannelFull {
        task_id: String,
        agent_id: String,
        parallel_limit: usize,
    },
    AgentBusy {
        task_id: String,
        agent_id: String,
        channel_id: String,
        channel_name: String,
    },
}

impl ChannelTaskWaitNotice {
    pub fn from_contract(wait: &ChannelTaskWait) -> Self {
        match wait {
            ChannelTaskWait::ChannelFull { task_id, agent_id } => Self::ChannelFull {
                task_id: task_id.clone(),
                agent_id: agent_id.clone(),
                parallel_limit: CHANNEL_PARALLEL_LIMIT,
            },
            ChannelTaskWait::AgentBusy {
                task_id,
                agent_id,
                channel_id,
                channel_name,
            } => Self::AgentBusy {
                task_id: task_id.clone(),
                agent_id: agent_id.clone(),
                channel_id: channel_id.clone(),
                channel_name: channel_name.clone(),
            },
        }
    }

    pub fn notice_text(&self, agent_names: &BTreeMap<String, String>) -> String {
        match self {
            Self::ChannelFull {
                agent_id,
                parallel_limit,
                ..
            } => {
                let name = agent_names
                    .get(agent_id)
                    .map(String::as_str)
                    .unwrap_or(agent_id.as_str());
                format!(
                    "{name} is waiting — channel is at its parallel limit ({parallel_limit} active tasks)."
                )
            }
            Self::AgentBusy {
                agent_id,
                channel_name,
                ..
            } => {
                let name = agent_names
                    .get(agent_id)
                    .map(String::as_str)
                    .unwrap_or(agent_id.as_str());
                format!("{name} is busy in #{channel_name} and will start once free.")
            }
        }
    }
}

/// Resolves which bot provides TTS / voice attribution in a multi-bot channel.
/// Priority: currently reading message's bot -> active running bot -> `lead_agent_id` -> `members[0]`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChannelVoiceAttribution {
    pub reading_agent_id: Option<String>,
    pub active_running_agent_ids: Vec<String>,
    pub lead_agent_id: Option<String>,
    pub members: Vec<String>,
}

impl ChannelVoiceAttribution {
    pub fn resolve_voice_agent(&self) -> Option<&str> {
        resolve_channel_voice_agent(
            self.reading_agent_id.as_deref(),
            &self.active_running_agent_ids,
            self.lead_agent_id.as_deref(),
            &self.members,
        )
    }
}

/// Pure function resolving channel `voiceAgent` priority:
/// reading bot -> active running bot -> `lead_agent_id` -> `members[0]`.
pub fn resolve_channel_voice_agent<'a>(
    reading_agent_id: Option<&'a str>,
    active_running_agent_ids: &'a [String],
    lead_agent_id: Option<&'a str>,
    members: &'a [String],
) -> Option<&'a str> {
    if let Some(reading) = reading_agent_id.filter(|s| !s.trim().is_empty()) {
        return Some(reading);
    }
    if let Some(active) = active_running_agent_ids
        .iter()
        .find(|s| !s.trim().is_empty())
    {
        return Some(active.as_str());
    }
    if let Some(lead) = lead_agent_id.filter(|s| !s.trim().is_empty()) {
        return Some(lead);
    }
    members.first().map(String::as_str)
}

/// Per-bot thinking/commentary group folded by `(sender_agent_id, turn_id)`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FoldedBotCommentary {
    pub sender_agent_id: String,
    pub turn_id: String,
    pub steps: Vec<String>,
}

/// Flat shared timeline message rendered inside `WorkspaceChannelConversation`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ChannelTimelineMessage {
    pub id: String,
    pub sequence: u64,
    pub author: ChannelMessageAuthor,
    pub task_id: Option<String>,
    pub turn_id: Option<String>,
    pub item_type: Option<String>,
    pub reply_to_message_id: Option<String>,
    pub reply_preview: Option<String>,
    pub superseded: bool,
    pub streaming: bool,
    pub text: String,
    pub created_at: String,
}

impl ChannelTimelineMessage {
    pub fn from_channel_message(msg: &ChannelMessage, reply_preview: Option<String>) -> Self {
        let reply_to_message_id = msg
            .message
            .reply_to_message_id
            .as_ref()
            .and_then(|opt| opt.as_ref())
            .cloned();
        Self {
            id: msg.id.clone(),
            sequence: msg.sequence,
            author: msg.author.clone(),
            task_id: msg.task_id.clone(),
            turn_id: msg.message.turn_id.clone(),
            item_type: msg.message.item_type.clone(),
            reply_to_message_id,
            reply_preview,
            superseded: msg.superseded,
            streaming: msg.message.status == ConversationMessageStatus::Streaming,
            text: msg.message.text.clone(),
            created_at: msg.message.created_at.clone(),
        }
    }

    pub fn is_commentary(&self) -> bool {
        self.item_type.as_deref() == Some("commentary")
    }

    /// Strips redundant `<OwnName>:` / `**<OwnName> report**:` prefixes on bot turns via
    /// `openbot_contracts::mentions::without_own_name_label`.
    pub fn display_text(&self) -> String {
        match self.author.kind {
            ChannelMessageAuthorKind::Agent => {
                without_own_name_label(&self.text, &self.author.name, self.streaming)
            }
            ChannelMessageAuthorKind::Member | ChannelMessageAuthorKind::Coordinator => {
                self.text.clone()
            }
        }
    }
}

/// Folds `item_type == "commentary"` rows into per-bot thinking steps keyed by
/// `(sender_agent_id, turn_id)` with own-name labels stripped.
pub fn fold_commentary_steps(
    messages: &[ChannelTimelineMessage],
) -> BTreeMap<(String, String), FoldedBotCommentary> {
    let mut grouped: BTreeMap<(String, String), FoldedBotCommentary> = BTreeMap::new();
    for msg in messages {
        if !msg.is_commentary() || msg.author.kind != ChannelMessageAuthorKind::Agent {
            continue;
        }
        let turn_id = msg
            .turn_id
            .clone()
            .or_else(|| msg.task_id.clone())
            .unwrap_or_else(|| msg.id.clone());
        let cleaned = msg.display_text();
        if cleaned.trim().is_empty() {
            continue;
        }
        let key = (msg.author.id.clone(), turn_id.clone());
        grouped
            .entry(key)
            .or_insert_with(|| FoldedBotCommentary {
                sender_agent_id: msg.author.id.clone(),
                turn_id,
                steps: Vec::new(),
            })
            .steps
            .push(cleaned);
    }
    grouped
}

/// Full view-model state for `WorkspaceChannelConversation`.
#[derive(Debug, Clone)]
pub struct ChannelConversationState {
    pub channel_id: String,
    pub name: String,
    pub title: String,
    pub instructions: String,
    pub members: Vec<String>,
    pub lead_agent_id: Option<String>,
    pub icon: Option<ChannelIcon>,
    pub image_url: Option<String>,
    pub image_failed: bool,
    pub isolated: bool,
    pub archived: bool,
    pub reactions_enabled: bool,
    pub pins_enabled: bool,
    pub read_only: bool,
    pub messages: Vec<ChannelTimelineMessage>,
    pub routing_events: Vec<ChannelRoutingEvent>,
    pub merged_routing_rows: Vec<MergedRoutingRow>,
    pub activity_indicator: ChannelActivityIndicatorState,
    pub awaiting_replies: AwaitingRepliesTracker,
    pub stopped_tasks: ChannelStoppedTasksState,
    pub wait_notices: Vec<ChannelTaskWaitNotice>,
    pub reading_agent_id: Option<String>,
    pub active_running_agent_ids: Vec<String>,
    pub composer_draft: String,
    pub reply_to_message_id: Option<String>,
    pub agent_names: BTreeMap<String, String>,
}

impl ChannelConversationState {
    pub fn new(
        channel_id: impl Into<String>,
        name: impl Into<String>,
        members: Vec<String>,
        lead_agent_id: Option<String>,
    ) -> Self {
        Self {
            channel_id: channel_id.into(),
            name: name.into(),
            title: String::new(),
            instructions: String::new(),
            members,
            lead_agent_id,
            icon: None,
            image_url: None,
            image_failed: false,
            isolated: false,
            archived: false,
            reactions_enabled: false,
            pins_enabled: false,
            read_only: false,
            messages: Vec::new(),
            routing_events: Vec::new(),
            merged_routing_rows: Vec::new(),
            activity_indicator: ChannelActivityIndicatorState::default(),
            awaiting_replies: AwaitingRepliesTracker::default(),
            stopped_tasks: ChannelStoppedTasksState::default(),
            wait_notices: Vec::new(),
            reading_agent_id: None,
            active_running_agent_ids: Vec::new(),
            composer_draft: String::new(),
            reply_to_message_id: None,
            agent_names: BTreeMap::new(),
        }
    }

    pub fn from_channel(channel: &Channel) -> Self {
        let members: Vec<String> = channel
            .draft
            .members
            .iter()
            .map(|m| m.agent_id.clone())
            .collect();
        let mut state = Self::new(
            channel.id.clone(),
            channel.draft.name.clone(),
            members,
            channel.draft.lead_agent_id.clone(),
        );
        state.title = channel.draft.title.clone();
        state.instructions = channel.draft.instructions.clone();
        state.icon = channel
            .draft
            .icon
            .as_ref()
            .and_then(|opt| opt.as_ref())
            .cloned();
        state.image_url = channel
            .image_url
            .as_ref()
            .and_then(|opt| opt.as_ref())
            .cloned();
        state.isolated = channel.draft.isolated.unwrap_or(false);
        state.set_archived(channel.archived);
        state
    }

    pub fn set_archived(&mut self, archived: bool) {
        self.archived = archived;
        self.read_only = archived;
    }

    pub fn mark_image_failed(&mut self) {
        self.image_failed = true;
    }

    pub fn clear_image_failed(&mut self) {
        self.image_failed = false;
    }

    pub fn set_routing_events(&mut self, events: Vec<ChannelRoutingEvent>) {
        self.merged_routing_rows = merge_consecutive_routing_events(&events);
        self.routing_events = events;
    }

    pub fn push_routing_event(&mut self, event: ChannelRoutingEvent) {
        self.routing_events.push(event);
        self.merged_routing_rows = merge_consecutive_routing_events(&self.routing_events);
    }

    pub fn sync_activity_workers(
        &mut self,
        workers: &[ChannelWorkerBrief],
        now_ms: u64,
        reduced_motion: bool,
    ) {
        self.activity_indicator
            .sync_workers(workers, now_ms, reduced_motion);
        self.active_running_agent_ids = workers.iter().map(|w| w.id.clone()).collect();
        for worker in workers {
            self.agent_names
                .entry(worker.id.clone())
                .or_insert_with(|| worker.name.clone());
        }
    }

    pub fn set_wait_notices_from_contract(&mut self, waits: &[ChannelTaskWait]) {
        self.wait_notices = waits.iter().map(ChannelTaskWaitNotice::from_contract).collect();
    }

    pub fn voice_attribution(&self) -> ChannelVoiceAttribution {
        ChannelVoiceAttribution {
            reading_agent_id: self.reading_agent_id.clone(),
            active_running_agent_ids: self.active_running_agent_ids.clone(),
            lead_agent_id: self.lead_agent_id.clone(),
            members: self.members.clone(),
        }
    }

    pub fn resolve_voice_agent(&self) -> Option<&str> {
        resolve_channel_voice_agent(
            self.reading_agent_id.as_deref(),
            &self.active_running_agent_ids,
            self.lead_agent_id.as_deref(),
            &self.members,
        )
    }

    pub fn channel_members(&self) -> Vec<ChannelMember> {
        self.members
            .iter()
            .map(|agent_id| ChannelMember {
                agent_id: agent_id.clone(),
            })
            .collect()
    }

    /// Evaluates the header `row` avatar layout (`<= 3` faces, `-16px` overlap, `failedUrl` fallback).
    pub fn header_avatar_eval(&self, size_px: f32) -> ChannelClusterEval {
        let members = self.channel_members();
        resolve_channel_cluster_sized(
            &members,
            ChannelAvatarLayout::Row,
            self.icon.as_ref(),
            self.image_url.as_deref(),
            self.image_failed,
            size_px,
        )
    }

    /// Evaluates the sidebar `cluster` `2x2` avatar layout (`5..12 -> 3 + +N`, `>=13 -> 2 + +N`, `failedUrl` fallback).
    pub fn sidebar_cluster_eval(&self, size_px: f32) -> ChannelClusterEval {
        let members = self.channel_members();
        resolve_channel_cluster_sized(
            &members,
            ChannelAvatarLayout::Cluster,
            self.icon.as_ref(),
            self.image_url.as_deref(),
            self.image_failed,
            size_px,
        )
    }

    pub fn folded_commentary(&self) -> BTreeMap<(String, String), FoldedBotCommentary> {
        fold_commentary_steps(&self.messages)
    }

    pub fn visible_timeline_messages(&self) -> Vec<&ChannelTimelineMessage> {
        self.messages
            .iter()
            .filter(|msg| !msg.is_commentary())
            .collect()
    }

    pub fn can_post(&self) -> bool {
        !self.read_only && !self.archived
    }

    /// Validates message deletion parity: streaming/busy messages refuse deletion with `"busy"`.
    pub fn can_delete_message(
        &self,
        message_id: &str,
        is_admin_or_owner: bool,
        requester_id: &str,
    ) -> Result<(), &'static str> {
        if self.archived {
            return Err("archived");
        }
        let Some(msg) = self.messages.iter().find(|m| m.id == message_id) else {
            return Err("not_found");
        };
        if msg.streaming {
            return Err("busy");
        }
        if !is_admin_or_owner && msg.author.id != requester_id {
            return Err("forbidden");
        }
        Ok(())
    }
}

fn view_rgba(palette: &OpenBotPalette, token: &str, fallback_rgb: u32) -> Rgba {
    palette.rgba(token).unwrap_or_else(|| rgb(fallback_rgb))
}

fn view_is_dark(palette: &OpenBotPalette) -> bool {
    palette.scheme == OpenBotColorScheme::Dark
}

/// Renders the sidebar `2x2` cluster channel avatar (`5..12 -> 3 + +N`, `>=13 -> 2 + +N`,
/// `failedUrl` fallback) via M2 [`render_channel_avatar`].
pub fn render_channel_sidebar_avatar(
    state: &ChannelConversationState,
    agents: &[ChannelAgentBrief],
    size_px: f32,
    ring_kind: Option<SidebarPlushRingKind>,
    palette: &OpenBotPalette,
    now_sec: f64,
) -> Div {
    let members = state.channel_members();
    render_channel_avatar(
        &members,
        agents,
        ChannelAvatarLayout::Cluster,
        state.icon.as_ref(),
        state.image_url.as_deref(),
        state.image_failed,
        size_px,
        ring_kind,
        view_is_dark(palette),
        now_sec,
    )
}

/// Renders the `WorkspaceChannelConversation` top header bar using M2 `render_channel_avatar`
/// in `Row` layout (`<= 3` faces, `-16px` overlap, `failedUrl` fallback).
pub fn render_channel_header(
    state: &ChannelConversationState,
    agents: &[ChannelAgentBrief],
    palette: &OpenBotPalette,
    now_sec: f64,
) -> Div {
    let dark = view_is_dark(palette);
    let bg_canvas = view_rgba(palette, "--openbot-bg-canvas", 0x1a1a1a);
    let bg_control = view_rgba(palette, "--openbot-bg-control", 0x212121);
    let bg_hover = view_rgba(palette, "--openbot-bg-control-hover", 0x242424);
    let border_col = view_rgba(palette, "--openbot-border-strong", 0x2c2c2e);
    let text_pri = view_rgba(palette, "--openbot-text-primary", 0xffffff);
    let text_muted = view_rgba(palette, "--openbot-text-muted", 0x979797);
    let accent = view_rgba(palette, "--openbot-accent", 0x007cf7);
    let warning = view_rgba(palette, "--openbot-warning-text", 0xff9412);
    let warning_soft = view_rgba(palette, "--openbot-warning-soft", 0x3b260b);
    let members = state.channel_members();
    let voice_name = state.resolve_voice_agent().map(|id| {
        state
            .agent_names
            .get(id)
            .cloned()
            .unwrap_or_else(|| id.to_string())
    });

    div()
        .w_full()
        .h(px(54.0))
        .px(px(16.0))
        .bg(bg_canvas)
        .border_b_1()
        .border_color(border_col)
        .flex()
        .items_center()
        .justify_between()
        .gap(px(12.0))
        .child(
            div()
                .flex()
                .items_center()
                .gap(px(10.0))
                .min_w(px(0.0))
                .flex_1()
                .child(render_channel_avatar(
                    &members,
                    agents,
                    ChannelAvatarLayout::Row,
                    state.icon.as_ref(),
                    state.image_url.as_deref(),
                    state.image_failed,
                    28.0,
                    None,
                    dark,
                    now_sec,
                ))
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .min_w(px(0.0))
                        .child(
                            div()
                                .flex()
                                .items_center()
                                .gap(px(6.0))
                                .child(
                                    div()
                                        .text_size(px(14.5))
                                        .font_weight(FontWeight::SEMIBOLD)
                                        .text_color(text_pri)
                                        .truncate()
                                        .child(state.name.clone()),
                                )
                                .when(state.isolated, |el| {
                                    el.child(tg_icon_rgba(IconName::Lock, 12.0, text_muted))
                                })
                                .when(state.archived, |el| {
                                    el.child(
                                        div()
                                            .px(px(6.0))
                                            .py(px(1.5))
                                            .rounded_full()
                                            .bg(warning_soft)
                                            .text_size(px(10.5))
                                            .font_weight(FontWeight::SEMIBOLD)
                                            .text_color(warning)
                                            .child("Archived"),
                                    )
                                }),
                        )
                        .child(
                            div()
                                .text_size(px(11.5))
                                .text_color(text_muted)
                                .truncate()
                                .child(if !state.title.trim().is_empty() {
                                    format!("{} · {} bots", state.title.trim(), state.members.len())
                                } else {
                                    format!("{} bots", state.members.len())
                                }),
                        ),
                ),
        )
        .child(
            div()
                .flex()
                .items_center()
                .gap(px(8.0))
                .flex_shrink_0()
                .when_some(voice_name, |el, vname| {
                    el.child(
                        div()
                            .px(px(8.0))
                            .py(px(4.0))
                            .rounded_full()
                            .bg(bg_control)
                            .border_1()
                            .border_color(border_col)
                            .flex()
                            .items_center()
                            .gap(px(4.0))
                            .child(tg_icon_rgba(IconName::Speaker, 12.0, accent))
                            .child(
                                div()
                                    .text_size(px(11.5))
                                    .text_color(text_muted)
                                    .child(vname),
                            ),
                    )
                })
                .child(
                    div()
                        .w(px(30.0))
                        .h(px(30.0))
                        .rounded(px(8.0))
                        .bg(bg_control)
                        .hover(move |s| s.bg(bg_hover))
                        .cursor_pointer()
                        .flex()
                        .items_center()
                        .justify_center()
                        .child(tg_icon_rgba(IconName::SettingsGear, 14.0, text_muted)),
                ),
        )
}

/// Renders a compact centered `channel-routing` action marker row (`joinRoutingRows`).
pub fn render_channel_routing_row(
    row: &MergedRoutingRow,
    agent_names: &BTreeMap<String, String>,
    palette: &OpenBotPalette,
) -> Div {
    let bg_surface = view_rgba(palette, "--openbot-bg-surface", 0x212121);
    let border_col = view_rgba(palette, "--openbot-border-strong", 0x2c2c2e);
    let text_muted = view_rgba(palette, "--openbot-text-muted", 0x979797);
    let names: Vec<String> = row
        .agent_ids
        .iter()
        .map(|id| agent_names.get(id).cloned().unwrap_or_else(|| id.clone()))
        .collect();
    let label = format!("{} {}", row.action, format_human_list(&names));

    div()
        .w_full()
        .py(px(4.0))
        .flex()
        .items_center()
        .justify_center()
        .gap(px(10.0))
        .child(div().flex_1().max_w(px(80.0)).h(px(1.0)).bg(border_col))
        .child(
            div()
                .px(px(10.0))
                .py(px(3.0))
                .rounded_full()
                .bg(bg_surface)
                .border_1()
                .border_color(border_col)
                .text_size(px(11.5))
                .text_color(text_muted)
                .child(label),
        )
        .child(div().flex_1().max_w(px(80.0)).h(px(1.0)).bg(border_col))
}

/// Renders the collapsible `AwaitingReplies` progress bar (`(done/total)` + per-row Answer/Steer/Stop
/// + Dismiss when all settled). Returns `None` when `archived == true` or empty/dismissed.
pub fn render_awaiting_replies_bar(
    tracker: &AwaitingRepliesTracker,
    archived: bool,
    agents: &[ChannelAgentBrief],
    palette: &OpenBotPalette,
    now_sec: f64,
) -> Option<Div> {
    if !tracker.visible(archived) {
        return None;
    }
    let dark = view_is_dark(palette);
    let bg_glass = view_rgba(palette, "--openbot-bg-glass", 0x212121);
    let bg_control = view_rgba(palette, "--openbot-bg-control", 0x212121);
    let border_grouped = view_rgba(palette, "--openbot-border-grouped", 0x333335);
    let text_pri = view_rgba(palette, "--openbot-text-primary", 0xffffff);
    let text_sec = view_rgba(palette, "--openbot-text-secondary", 0xdcdcdc);
    let text_muted = view_rgba(palette, "--openbot-text-muted", 0x979797);
    let accent = view_rgba(palette, "--openbot-accent", 0x007cf7);
    let accent_soft = view_rgba(palette, "--openbot-accent-soft", 0x0d2848);
    let success = view_rgba(palette, "--openbot-success", 0x31cf76);
    let warning = view_rgba(palette, "--openbot-warning-text", 0xff9412);
    let warning_soft = view_rgba(palette, "--openbot-warning-soft", 0x3b260b);
    let danger = view_rgba(palette, "--openbot-danger-text", 0xff6069);
    let danger_soft = view_rgba(palette, "--openbot-danger-soft", 0x451518);

    let (done, total) = tracker.progress();
    let ratio = if total == 0 {
        0.0
    } else {
        (done as f32 / total as f32).clamp(0.0, 1.0)
    };
    let chevron = if tracker.collapsed {
        IconName::ChevronRight
    } else {
        IconName::ChevronDown
    };

    let mut rows = div().flex().flex_col().gap(px(6.0)).pt(px(8.0));
    if !tracker.collapsed {
        for entry in &tracker.entries {
            let brief = agents.iter().find(|a| a.id == entry.agent_id);
            let mood = match entry.state {
                AwaitingReplyState::Asked => AvatarMood::Waiting,
                AwaitingReplyState::Working => AvatarMood::Working,
                AwaitingReplyState::Replied => AvatarMood::Responded,
                AwaitingReplyState::Failed | AwaitingReplyState::Stopped => AvatarMood::Failed,
            };
            let mut props = AgentAvatarProps::new(
                brief
                    .and_then(|b| b.avatar_seed.clone())
                    .unwrap_or_else(|| entry.agent_id.clone()),
            )
            .with_size(22.0)
            .with_mood(mood)
            .with_hue_and_color(
                brief.and_then(|b| b.avatar_hue),
                brief.and_then(|b| b.avatar_color.clone()),
            )
            .with_url(brief.and_then(|b| b.avatar_url.clone()));
            props.dark_scheme = dark;

            let (pill_bg, pill_fg) = match entry.state {
                AwaitingReplyState::Asked => (warning_soft, warning),
                AwaitingReplyState::Working => (accent_soft, accent),
                AwaitingReplyState::Replied => (bg_control, success),
                AwaitingReplyState::Failed | AwaitingReplyState::Stopped => (danger_soft, danger),
            };
            let detail = entry
                .waiting_reason
                .as_deref()
                .or(entry.preview.as_deref())
                .unwrap_or("");

            rows = rows.child(
                div()
                    .px(px(8.0))
                    .py(px(6.0))
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
                            .child(render_agent_avatar(&props, None, now_sec))
                            .child(
                                div()
                                    .flex()
                                    .flex_col()
                                    .min_w(px(0.0))
                                    .flex_1()
                                    .child(
                                        div()
                                            .text_size(px(12.5))
                                            .font_weight(FontWeight::MEDIUM)
                                            .text_color(text_pri)
                                            .truncate()
                                            .child(entry.agent_name.clone()),
                                    )
                                    .when(!detail.is_empty(), |el| {
                                        el.child(
                                            div()
                                                .text_size(px(11.0))
                                                .text_color(text_muted)
                                                .truncate()
                                                .child(detail.to_string()),
                                        )
                                    }),
                            ),
                    )
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(5.0))
                            .flex_shrink_0()
                            .child(
                                div()
                                    .px(px(7.0))
                                    .py(px(2.0))
                                    .rounded_full()
                                    .bg(pill_bg)
                                    .text_size(px(11.0))
                                    .font_weight(FontWeight::MEDIUM)
                                    .text_color(pill_fg)
                                    .child(entry.state.label()),
                            )
                            .when(entry.can_answer(), |el| {
                                el.child(
                                    div()
                                        .px(px(8.0))
                                        .py(px(3.0))
                                        .rounded(px(6.0))
                                        .bg(warning_soft)
                                        .text_size(px(11.0))
                                        .font_weight(FontWeight::SEMIBOLD)
                                        .text_color(warning)
                                        .cursor_pointer()
                                        .child("Answer"),
                                )
                            })
                            .when(entry.can_steer(), |el| {
                                el.child(
                                    div()
                                        .px(px(8.0))
                                        .py(px(3.0))
                                        .rounded(px(6.0))
                                        .bg(accent_soft)
                                        .text_size(px(11.0))
                                        .font_weight(FontWeight::SEMIBOLD)
                                        .text_color(accent)
                                        .cursor_pointer()
                                        .child("Steer"),
                                )
                            })
                            .when(entry.can_stop(), |el| {
                                el.child(
                                    div()
                                        .px(px(8.0))
                                        .py(px(3.0))
                                        .rounded(px(6.0))
                                        .bg(danger_soft)
                                        .text_size(px(11.0))
                                        .font_weight(FontWeight::SEMIBOLD)
                                        .text_color(danger)
                                        .cursor_pointer()
                                        .child("Stop"),
                                )
                            }),
                    ),
            );
        }
    }

    Some(
        div()
            .w_full()
            .p(px(10.0))
            .rounded(px(12.0))
            .bg(bg_glass)
            .border_1()
            .border_color(border_grouped)
            .flex()
            .flex_col()
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
                            .cursor_pointer()
                            .child(tg_icon_rgba(chevron, 12.0, text_muted))
                            .child(
                                div()
                                    .text_size(px(12.5))
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .text_color(text_pri)
                                    .child(format!("Awaiting replies ({done}/{total})")),
                            )
                            .child(
                                div()
                                    .text_size(px(11.5))
                                    .text_color(text_sec)
                                    .child(format!("· {}", tracker.header_status_text())),
                            ),
                    )
                    .when(tracker.all_settled(), |el| {
                        el.child(
                            div()
                                .w(px(22.0))
                                .h(px(22.0))
                                .rounded(px(6.0))
                                .flex()
                                .items_center()
                                .justify_center()
                                .cursor_pointer()
                                .child(tg_icon_rgba(IconName::Close, 12.0, text_muted)),
                        )
                    }),
            )
            .child(
                div()
                    .mt(px(6.0))
                    .w_full()
                    .h(px(3.0))
                    .rounded_full()
                    .bg(border_grouped)
                    .overflow_hidden()
                    .child(
                        div()
                            .h_full()
                            .w(relative(ratio))
                            .bg(if tracker.failed_or_stopped_count() > 0 {
                                warning
                            } else {
                                accent
                            }),
                    ),
            )
            .when(!tracker.collapsed, |el| el.child(rows)),
    )
}

/// Renders compact wait notices (`ChannelFull` parallel limit `2` or `AgentBusy` in another channel).
pub fn render_wait_notices_bar(
    notices: &[ChannelTaskWaitNotice],
    archived: bool,
    agent_names: &BTreeMap<String, String>,
    palette: &OpenBotPalette,
) -> Option<Div> {
    if archived || notices.is_empty() {
        return None;
    }
    let warning = view_rgba(palette, "--openbot-warning-text", 0xff9412);
    let warning_soft = view_rgba(palette, "--openbot-warning-soft", 0x3b260b);
    let border_grouped = view_rgba(palette, "--openbot-border-grouped", 0x333335);

    let mut stack = div().w_full().flex().flex_col().gap(px(4.0));
    for notice in notices {
        stack = stack.child(
            div()
                .w_full()
                .px(px(10.0))
                .py(px(6.0))
                .rounded(px(9.0))
                .bg(warning_soft)
                .border_1()
                .border_color(border_grouped)
                .flex()
                .items_center()
                .gap(px(6.0))
                .child(tg_icon_rgba(IconName::ClockSending, 12.0, warning))
                .child(
                    div()
                        .text_size(px(12.0))
                        .text_color(warning)
                        .child(notice.notice_text(agent_names)),
                ),
        );
    }
    Some(stack)
}

/// Renders frosted-glass `ChannelStoppedTasks` loop-recovery cards above the composer with
/// `Continue` (hidden if owner removed), `Reassign` dropdown (excluding `owner_agent_id`), and `Cancel`.
pub fn render_stopped_tasks_banner(
    stopped: &ChannelStoppedTasksState,
    members: &[String],
    archived: bool,
    agent_names: &BTreeMap<String, String>,
    palette: &OpenBotPalette,
) -> Option<Div> {
    if !stopped.is_visible(archived) {
        return None;
    }
    let bg_glass = view_rgba(palette, "--openbot-bg-glass", 0x212121);
    let bg_sheet = view_rgba(palette, "--openbot-bg-sheet", 0x141414);
    let bg_control = view_rgba(palette, "--openbot-bg-control", 0x212121);
    let bg_hover = view_rgba(palette, "--openbot-bg-control-hover", 0x242424);
    let border_grouped = view_rgba(palette, "--openbot-border-grouped", 0x333335);
    let text_pri = view_rgba(palette, "--openbot-text-primary", 0xffffff);
    let text_sec = view_rgba(palette, "--openbot-text-secondary", 0xdcdcdc);
    let text_muted = view_rgba(palette, "--openbot-text-muted", 0x979797);
    let accent = view_rgba(palette, "--openbot-accent", 0x007cf7);
    let text_on_accent = view_rgba(palette, "--openbot-text-on-accent", 0xffffff);
    let warning = view_rgba(palette, "--openbot-warning-text", 0xff9412);
    let danger_text = view_rgba(palette, "--openbot-danger-text", 0xff6069);
    let danger_soft = view_rgba(palette, "--openbot-danger-soft", 0x451518);
    let danger_border = view_rgba(palette, "--openbot-danger-border", 0x802028);

    let mut list = div().w_full().flex().flex_col().gap(px(8.0));
    for item in &stopped.items {
        let can_continue = item.can_continue(members);
        let candidates = item.reassign_candidates(members);
        let dropdown_open =
            stopped.open_reassign_dropdown_task_id.as_deref() == Some(item.task_id.as_str());
        let reason = item.formatted_reason(agent_names);

        let mut dropdown_rows = div().flex().flex_col().gap(px(2.0));
        for candidate_id in &candidates {
            let label = agent_names
                .get(candidate_id)
                .cloned()
                .unwrap_or_else(|| candidate_id.clone());
            dropdown_rows = dropdown_rows.child(
                div()
                    .px(px(10.0))
                    .py(px(6.0))
                    .rounded(px(6.0))
                    .hover(move |s| s.bg(bg_hover))
                    .cursor_pointer()
                    .text_size(px(12.5))
                    .text_color(text_pri)
                    .child(label),
            );
        }

        list = list.child(
            div()
                .w_full()
                .p(px(12.0))
                .rounded(px(14.0))
                .bg(bg_glass)
                .border_1()
                .border_color(if item.is_assignment_loop {
                    warning
                } else {
                    danger_border
                })
                .flex()
                .flex_col()
                .gap(px(8.0))
                .child(
                    div()
                        .flex()
                        .items_start()
                        .justify_between()
                        .gap(px(8.0))
                        .child(
                            div()
                                .flex()
                                .flex_col()
                                .gap(px(3.0))
                                .flex_1()
                                .child(
                                    div()
                                        .text_size(px(12.5))
                                        .font_weight(FontWeight::SEMIBOLD)
                                        .text_color(if item.is_assignment_loop {
                                            warning
                                        } else {
                                            danger_text
                                        })
                                        .child(if item.is_assignment_loop {
                                            "Assignment Loop Protection Paused Task"
                                        } else {
                                            "Channel Task Stopped"
                                        }),
                                )
                                .child(
                                    div()
                                        .text_size(px(12.0))
                                        .text_color(text_sec)
                                        .child(reason),
                                ),
                        ),
                )
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(6.0))
                        .when(can_continue, |el| {
                            el.child(
                                div()
                                    .px(px(11.0))
                                    .py(px(5.0))
                                    .rounded(px(8.0))
                                    .bg(accent)
                                    .cursor_pointer()
                                    .text_size(px(12.0))
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .text_color(text_on_accent)
                                    .child(STOPPED_TASK_RECOVERY_ACTION_LABELS[0]),
                            )
                        })
                        .when(!candidates.is_empty(), |el| {
                            el.child(
                                div()
                                    .relative()
                                    .child(
                                        div()
                                            .px(px(11.0))
                                            .py(px(5.0))
                                            .rounded(px(8.0))
                                            .bg(bg_control)
                                            .border_1()
                                            .border_color(border_grouped)
                                            .hover(move |s| s.bg(bg_hover))
                                            .cursor_pointer()
                                            .flex()
                                            .items_center()
                                            .gap(px(4.0))
                                            .text_size(px(12.0))
                                            .font_weight(FontWeight::MEDIUM)
                                            .text_color(text_pri)
                                            .child(STOPPED_TASK_RECOVERY_ACTION_LABELS[1])
                                            .child(tg_icon_rgba(
                                                IconName::ChevronDown,
                                                11.0,
                                                text_muted,
                                            )),
                                    )
                                    .when(dropdown_open, |d| {
                                        d.child(
                                            deferred(
                                                div()
                                                    .absolute()
                                                    .bottom(px(34.0))
                                                    .left_0()
                                                    .min_w(px(180.0))
                                                    .p(px(4.0))
                                                    .rounded(px(10.0))
                                                    .bg(bg_sheet)
                                                    .border_1()
                                                    .border_color(border_grouped)
                                                    .occlude()
                                                    .child(dropdown_rows),
                                            )
                                            .with_priority(45),
                                        )
                                    }),
                            )
                        })
                        .when(stopped.can_cancel, |el| {
                            el.child(
                                div()
                                    .px(px(11.0))
                                    .py(px(5.0))
                                    .rounded(px(8.0))
                                    .bg(danger_soft)
                                    .border_1()
                                    .border_color(danger_border)
                                    .cursor_pointer()
                                    .text_size(px(12.0))
                                    .font_weight(FontWeight::MEDIUM)
                                    .text_color(danger_text)
                                    .child(STOPPED_TASK_RECOVERY_ACTION_LABELS[2]),
                            )
                        }),
                ),
        );
    }

    Some(list)
}

fn render_channel_message_row(
    msg: &ChannelTimelineMessage,
    is_lead: bool,
    folded: Option<&FoldedBotCommentary>,
    agents: &[ChannelAgentBrief],
    palette: &OpenBotPalette,
    now_sec: f64,
) -> Div {
    let dark = view_is_dark(palette);
    let bg_surface = view_rgba(palette, "--openbot-bg-surface", 0x212121);
    let bg_control = view_rgba(palette, "--openbot-bg-control", 0x212121);
    let border_col = view_rgba(palette, "--openbot-border-strong", 0x2c2c2e);
    let text_pri = view_rgba(palette, "--openbot-text-primary", 0xffffff);
    let text_sec = view_rgba(palette, "--openbot-text-secondary", 0xdcdcdc);
    let text_muted = view_rgba(palette, "--openbot-text-muted", 0x979797);
    let accent = view_rgba(palette, "--openbot-accent", 0x007cf7);
    let accent_soft = view_rgba(palette, "--openbot-accent-soft", 0x0d2848);
    let is_human = msg.author.kind == ChannelMessageAuthorKind::Member;
    let is_coordinator = msg.author.kind == ChannelMessageAuthorKind::Coordinator;

    if is_coordinator {
        return div()
            .w_full()
            .py(px(4.0))
            .flex()
            .justify_center()
            .child(
                div()
                    .px(px(10.0))
                    .py(px(4.0))
                    .rounded_full()
                    .bg(bg_surface)
                    .border_1()
                    .border_color(border_col)
                    .text_size(px(11.5))
                    .text_color(text_muted)
                    .child(msg.display_text()),
            );
    }

    let brief = agents.iter().find(|a| a.id == msg.author.id);
    let author_name_color = if is_human {
        text_pri
    } else {
        let hex = avatar_head_color(
            brief
                .and_then(|b| b.avatar_seed.as_deref())
                .unwrap_or(&msg.author.id),
            brief.and_then(|b| b.avatar_hue),
            brief.and_then(|b| b.avatar_color.as_deref()),
        );
        parse_css_color_to_rgba(&hex).unwrap_or(text_pri)
    };

    let avatar_el = if !is_human {
        let mut p = AgentAvatarProps::new(
            brief
                .and_then(|b| b.avatar_seed.clone())
                .unwrap_or_else(|| msg.author.id.clone()),
        )
        .with_size(28.0)
        .with_mood(if msg.streaming {
            AvatarMood::Working
        } else {
            AvatarMood::Idle
        })
        .with_hue_and_color(
            brief.and_then(|b| b.avatar_hue),
            brief.and_then(|b| b.avatar_color.clone()),
        )
        .with_url(brief.and_then(|b| b.avatar_url.clone()));
        p.dark_scheme = dark;
        Some(render_agent_avatar(&p, None, now_sec))
    } else {
        None
    };

    let bubble = div()
        .max_w(px(560.0))
        .px(px(14.0))
        .py(px(10.0))
        .rounded(px(18.0))
        .bg(if is_human { accent_soft } else { bg_surface })
        .border_1()
        .border_color(if is_human { accent } else { border_col })
        .when(msg.superseded, |el| el.opacity(0.48))
        .flex()
        .flex_col()
        .gap(px(5.0))
        .when(msg.reply_to_message_id.is_some(), |el| {
            let preview = msg
                .reply_preview
                .as_deref()
                .filter(|s| !s.trim().is_empty())
                .unwrap_or("Replying to earlier channel message");
            el.child(
                div()
                    .px(px(8.0))
                    .py(px(4.0))
                    .rounded(px(8.0))
                    .bg(bg_control)
                    .border_l_2()
                    .border_color(accent)
                    .flex()
                    .items_center()
                    .gap(px(5.0))
                    .child(tg_icon_rgba(IconName::Reply, 11.0, accent))
                    .child(
                        div()
                            .text_size(px(11.5))
                            .text_color(text_muted)
                            .truncate()
                            .child(preview.to_string()),
                    ),
            )
        })
        .when_some(folded, |el, commentary| {
            el.child(
                div()
                    .px(px(8.0))
                    .py(px(3.5))
                    .rounded(px(8.0))
                    .bg(bg_control)
                    .text_size(px(11.0))
                    .text_color(text_muted)
                    .child(format!(
                        "Folded commentary · {} thinking step(s)",
                        commentary.steps.len()
                    )),
            )
        })
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
                        .gap(px(5.0))
                        .child(
                            div()
                                .text_size(px(12.5))
                                .font_weight(FontWeight::SEMIBOLD)
                                .text_color(author_name_color)
                                .child(msg.author.name.clone()),
                        )
                        .when(is_lead && !is_human, |b| {
                            b.child(div().text_size(px(11.0)).text_color(rgb(0xf0b429)).child("👑"))
                        })
                        .when(msg.superseded, |b| {
                            b.child(
                                div()
                                    .px(px(6.0))
                                    .py(px(1.0))
                                    .rounded_full()
                                    .bg(bg_control)
                                    .text_size(px(10.0))
                                    .text_color(text_muted)
                                    .child("Superseded"),
                            )
                        }),
                )
                .child(
                    div()
                        .text_size(px(10.5))
                        .text_color(text_muted)
                        .child(msg.created_at.clone()),
                ),
        )
        .child(
            div()
                .text_size(px(13.5))
                .text_color(text_sec)
                .child(msg.display_text()),
        );

    div()
        .w_full()
        .flex()
        .items_start()
        .gap(px(8.0))
        .when(is_human, |el| el.justify_end())
        .when_some(avatar_el, |el, av| el.child(av))
        .child(bubble)
}

/// Renders the complete multi-bot `WorkspaceChannelConversation` view (`OB-F17`):
/// - Header `Row` avatar (`<= 3`, `-16px` overlap, `failedUrl` fallback)
/// - Empty-state `Cluster` `2x2` avatar preview (`5..12 -> 3 + +N`, `>=13 -> 2 + +N`)
/// - Flat shared timeline with merged `channel-routing` rows, folded bot commentary, and ZERO reactions/pins
/// - Live `ChannelActivityIndicatorState::render` (`-8px` overlap, active & departing `leaving_ids`)
/// - Collapsible `AwaitingReplies` (hidden when archived)
/// - `ChannelTaskWaitNotice` & frosted-glass `ChannelStoppedTasks` loop-recovery cards above composer
pub fn render_channel_conversation(
    state: &ChannelConversationState,
    agents: &[ChannelAgentBrief],
    palette: &OpenBotPalette,
    now_sec: f64,
) -> Div {
    let dark = view_is_dark(palette);
    let bg_canvas = view_rgba(palette, "--openbot-bg-canvas", 0x1a1a1a);
    let bg_control = view_rgba(palette, "--openbot-bg-control", 0x212121);
    let border_col = view_rgba(palette, "--openbot-border-strong", 0x2c2c2e);
    let text_pri = view_rgba(palette, "--openbot-text-primary", 0xffffff);
    let text_muted = view_rgba(palette, "--openbot-text-muted", 0x979797);
    let accent = view_rgba(palette, "--openbot-accent", 0x007cf7);
    let text_on_accent = view_rgba(palette, "--openbot-text-on-accent", 0xffffff);
    let folded_map = state.folded_commentary();
    let visible_messages = state.visible_timeline_messages();

    let mut timeline = div().p(px(16.0)).flex().flex_col().gap(px(10.0));
    if visible_messages.is_empty() && state.merged_routing_rows.is_empty() {
        timeline = timeline.child(
            div()
                .py(px(36.0))
                .flex()
                .flex_col()
                .items_center()
                .gap(px(10.0))
                .child(render_channel_sidebar_avatar(
                    state, agents, 56.0, None, palette, now_sec,
                ))
                .child(
                    div()
                        .text_size(px(15.0))
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(text_pri)
                        .child(state.name.clone()),
                )
                .child(
                    div()
                        .text_size(px(12.5))
                        .text_color(text_muted)
                        .child("Mention @agent or @everyone to coordinate bots in this channel."),
                ),
        );
    }

    for routing_row in &state.merged_routing_rows {
        timeline = timeline.child(render_channel_routing_row(
            routing_row,
            &state.agent_names,
            palette,
        ));
    }

    for msg in visible_messages {
        let is_lead = state.lead_agent_id.as_deref() == Some(msg.author.id.as_str());
        let turn_key = msg
            .turn_id
            .clone()
            .or_else(|| msg.task_id.clone())
            .unwrap_or_else(|| msg.id.clone());
        let folded = folded_map.get(&(msg.author.id.clone(), turn_key));
        timeline = timeline.child(render_channel_message_row(
            msg, is_lead, folded, agents, palette, now_sec,
        ));
    }

    if !state.activity_indicator.face_ids.is_empty() && !state.archived {
        let worker_names: Vec<&str> = state
            .active_running_agent_ids
            .iter()
            .map(|id| {
                state
                    .agent_names
                    .get(id)
                    .map(String::as_str)
                    .unwrap_or(id.as_str())
            })
            .collect();
        let sentence = channel_activity_sentence(&worker_names);
        let label = if sentence.is_empty() {
            "working..."
        } else {
            &sentence
        };
        timeline = timeline.child(
            div()
                .pt(px(4.0))
                .child(state.activity_indicator.render(label, dark, now_sec)),
        );
    }

    let awaiting_bar =
        render_awaiting_replies_bar(&state.awaiting_replies, state.archived, agents, palette, now_sec);
    let wait_bar =
        render_wait_notices_bar(&state.wait_notices, state.archived, &state.agent_names, palette);
    let stopped_bar = render_stopped_tasks_banner(
        &state.stopped_tasks,
        &state.members,
        state.archived,
        &state.agent_names,
        palette,
    );

    let footer = if state.read_only || state.archived {
        div()
            .w_full()
            .px(px(14.0))
            .py(px(11.0))
            .rounded(px(12.0))
            .bg(bg_control)
            .border_1()
            .border_color(border_col)
            .text_size(px(12.5))
            .text_color(text_muted)
            .child("This channel is archived (read-only). Restore it in Channel Settings to post.")
    } else {
        div()
            .w_full()
            .px(px(12.0))
            .py(px(9.0))
            .rounded(px(14.0))
            .bg(bg_control)
            .border_1()
            .border_color(border_col)
            .flex()
            .items_center()
            .justify_between()
            .gap(px(10.0))
            .child(
                div()
                    .flex_1()
                    .min_w(px(0.0))
                    .text_size(px(13.0))
                    .text_color(if state.composer_draft.is_empty() {
                        text_muted
                    } else {
                        text_pri
                    })
                    .child(if state.composer_draft.is_empty() {
                        format!("Message #{} · @agent or @everyone...", state.name)
                    } else {
                        state.composer_draft.clone()
                    }),
            )
            .child(
                div()
                    .w(px(30.0))
                    .h(px(30.0))
                    .rounded_full()
                    .bg(accent)
                    .cursor_pointer()
                    .flex()
                    .items_center()
                    .justify_center()
                    .child(tg_icon_rgba(IconName::ArrowUp, 14.0, text_on_accent)),
            )
    };

    div()
        .min_w(px(CHANNEL_CONVERSATION_MIN_WIDTH_PX))
        .size_full()
        .bg(bg_canvas)
        .flex()
        .flex_col()
        .child(render_channel_header(state, agents, palette, now_sec))
        .child(
            div()
                .flex_1()
                .min_h(px(0.0))
                .child(
                    div()
                        .id("openbot-channel-timeline-scroll")
                        .size_full()
                        .overflow_y_scroll()
                        .child(timeline),
                ),
        )
        .child(
            div()
                .px(px(16.0))
                .pb(px(14.0))
                .pt(px(8.0))
                .flex()
                .flex_col()
                .gap(px(8.0))
                .when_some(awaiting_bar, |el, bar| el.child(bar))
                .when_some(wait_bar, |el, bar| el.child(bar))
                .when_some(stopped_bar, |el, bar| el.child(bar))
                .child(footer),
        )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[std::prelude::v1::test]
    fn merge_consecutive_routing_events_dedups_and_respects_action_boundaries() {
        let events = vec![
            ChannelRoutingEvent {
                action: "assigned".to_string(),
                agent_id: "a1".to_string(),
            },
            ChannelRoutingEvent {
                action: "assigned".to_string(),
                agent_id: "a1".to_string(),
            },
            ChannelRoutingEvent {
                action: "assigned".to_string(),
                agent_id: "a2".to_string(),
            },
            ChannelRoutingEvent {
                action: "transferred".to_string(),
                agent_id: "a3".to_string(),
            },
            ChannelRoutingEvent {
                action: "assigned".to_string(),
                agent_id: "a1".to_string(),
            },
        ];
        let merged = merge_consecutive_routing_events(&events);
        assert_eq!(
            merged,
            vec![
                MergedRoutingRow {
                    action: "assigned".to_string(),
                    agent_ids: vec!["a1".to_string(), "a2".to_string()],
                },
                MergedRoutingRow {
                    action: "transferred".to_string(),
                    agent_ids: vec!["a3".to_string()],
                },
                MergedRoutingRow {
                    action: "assigned".to_string(),
                    agent_ids: vec!["a1".to_string()],
                },
            ]
        );
    }

    #[std::prelude::v1::test]
    fn awaiting_replies_tracks_five_states_and_hides_when_archived() {
        let mut tracker = AwaitingRepliesTracker::new(vec![
            AwaitingReplyEntry {
                agent_id: "a1".to_string(),
                agent_name: "Alice".to_string(),
                state: AwaitingReplyState::Working,
                preview: None,
                delivery_id: None,
                waiting_reason: Some("Needs your answer".to_string()),
            },
            AwaitingReplyEntry {
                agent_id: "a2".to_string(),
                agent_name: "Bob".to_string(),
                state: AwaitingReplyState::Replied,
                preview: Some("Done".to_string()),
                delivery_id: Some("del-2".to_string()),
                waiting_reason: None,
            },
        ]);
        assert_eq!(tracker.progress(), (1, 2));
        assert!(!tracker.all_settled());
        assert!(tracker.visible(false));
        assert!(!tracker.visible(true));
        assert!(!tracker.dismiss());

        assert!(tracker.mark_stopped("a1"));
        assert_eq!(tracker.progress(), (2, 2));
        assert!(tracker.all_settled());
        assert!(tracker.dismiss());
        assert!(!tracker.visible(false));
    }

    #[std::prelude::v1::test]
    fn stopped_tasks_loop_reason_names_all_agents_and_excludes_owner_from_reassign() {
        let members = vec!["a1".to_string(), "a2".to_string(), "a3".to_string()];
        let candidates = ChannelStoppedTasksState::reassign_candidates(&members, "a1");
        assert_eq!(candidates, vec!["a2".to_string(), "a3".to_string()]);

        let item = StoppedChannelTaskBannerItem {
            task_id: "task-loop".to_string(),
            owner_agent_id: Some("a1".to_string()),
            error: Some(CHANNEL_ASSIGNMENT_LIMIT_ERROR.to_string()),
            run_agent_ids: vec!["a1".to_string(), "a2".to_string()],
            is_assignment_loop: true,
        };
        let mut names = BTreeMap::new();
        names.insert("a1".to_string(), "OwnerBot".to_string());
        names.insert("a2".to_string(), "RescueBot".to_string());
        let reason = item.formatted_reason(&names);
        assert!(reason.contains("OwnerBot"));
        assert!(reason.contains("RescueBot"));
        assert!(reason.contains('8'));
        assert!(item.can_continue(&members));
        assert!(!item.can_continue(&["a2".to_string(), "a3".to_string()]));
    }

    #[std::prelude::v1::test]
    fn voice_attribution_priority_and_channel_flags() {
        let members = vec!["m1".to_string(), "m2".to_string()];
        let active = vec!["m2".to_string()];
        assert_eq!(
            resolve_channel_voice_agent(Some("reading-bot"), &active, Some("m1"), &members),
            Some("reading-bot")
        );
        assert_eq!(
            resolve_channel_voice_agent(None, &active, Some("m1"), &members),
            Some("m2")
        );
        assert_eq!(
            resolve_channel_voice_agent(None, &[], Some("m1"), &members),
            Some("m1")
        );
        assert_eq!(
            resolve_channel_voice_agent(None, &[], None, &members),
            Some("m1")
        );

        let mut state = ChannelConversationState::new("ch-1", "Room", members, Some("m1".into()));
        assert!(!state.reactions_enabled);
        assert!(!state.pins_enabled);
        assert!(!state.read_only);
        state.set_archived(true);
        assert!(state.read_only);
        assert!(!state.can_post());
    }
}
