//! ConversationStage view component (`SPEC-02-GPUI-COMPONENT-HIERARCHY` § 2.4).
//!
//! Sub-components:
//! 1. Chat Header (Avatar, Title, Subtitle, Search, Call, Video Call, Drawer Toggle).
//! 2. Pinned Message Bar (Pinned snippet, jump to message).
//! 3. Virtualized Message Transcript with 11 canonical message cards:
//!    - TextMessageView
//!    - MediaMessageView
//!    - VoiceMessageView (5-bit waveform renderer)
//!    - AlbumMessageView
//!    - LocationMessageView
//!    - PollMessageView
//!    - ServiceMessageView
//!    - CallMessageView
//!    - StoryMessageView
//!    - InvoiceMessageView
//!    - InlineBotKeyboardView

use std::cell::{Cell, RefCell};
use std::collections::{HashMap, HashSet, VecDeque};
use std::path::PathBuf;
use std::rc::Rc;
use std::time::{SystemTime, UNIX_EPOCH};

use gpui::*;
use gpui::prelude::FluentBuilder;

use crate::theme::{tg_icon, IconName, TdesktopThemePalette};
use crate::views::stage::history_drag_area::HistoryDragArea;
use crate::views::stage::history_view_highlight_manager::{
    ChatPaintHighlight, HistoryViewHighlightManager,
};
use crate::views::stage::pinned_bar::PinnedTracker;
use crate::views::stage::rich_meta::MessageRichMeta;
use crate::views::stage::stage_bubble::{
    render_message_row_apple, BotAssistRenderContext, BubbleGroupLayout,
};
use crate::views::stage::stage_context_menu::ReactionOverlayMode;
use crate::views::stage::stage_bars::{
    render_pinned_banner_46px, render_scroll_to_bottom_fab, render_stage_more_menu_overlay,
    render_stage_search_bar, render_stage_top_bar, TopBarUiAction,
};
use crate::views::stage::wallpaper::{
    render_empty_history_placeholder, render_empty_no_chat_pill, render_themed_wallpaper,
};
pub use crate::views::stage::cards::*;

pub use crate::views::stage::cards::text_card::{
    render_bubble_tail, BubbleRounding, BUBBLE_RADIUS_LARGE, BUBBLE_RADIUS_SMALL,
    K_ATTACH_MESSAGE_TO_PREVIOUS_SECONDS_DELTA, MSG_MAX_WIDTH, MSG_MIN_WIDTH,
    RichSpanKind, RichTextSpan, parse_markdown_spans,
};

pub fn parse_timestamp_seconds(ts: &str) -> Option<i64> {
    let ts = ts.trim();
    if let Ok(secs) = ts.parse::<i64>() {
        return Some(secs);
    }
    let parts: Vec<&str> = ts.split(':').collect();
    if parts.len() == 2 {
        if let (Ok(h), Ok(m)) = (parts[0].parse::<i64>(), parts[1].parse::<i64>()) {
            return Some(h * 3600 + m * 60);
        }
    } else if parts.len() == 3 {
        if let (Ok(h), Ok(m), Ok(s)) = (parts[0].parse::<i64>(), parts[1].parse::<i64>(), parts[2].parse::<i64>()) {
            return Some(h * 3600 + m * 60 + s);
        }
    }
    None
}

/// Compute consecutive message attachment flags (attached_prev, attached_next)
/// based on identical sender, message direction, and 900-second threshold.
pub fn compute_attachment(messages: &[MessageItemModel], index: usize) -> (bool, bool) {
    if index >= messages.len() {
        return (false, false);
    }
    let current = &messages[index];
    if matches!(current.content, MessageContent::Service { .. }) {
        return (false, false);
    }

    let attached_prev = if index > 0 {
        let prev = &messages[index - 1];
        if matches!(prev.content, MessageContent::Service { .. }) {
            false
        } else if prev.sender_id == current.sender_id && prev.direction == current.direction {
            let prev_sec = parse_timestamp_seconds(&prev.timestamp);
            let cur_sec = parse_timestamp_seconds(&current.timestamp);
            match (prev_sec, cur_sec) {
                (Some(p), Some(c)) => (c - p).abs() <= K_ATTACH_MESSAGE_TO_PREVIOUS_SECONDS_DELTA,
                _ => true,
            }
        } else {
            false
        }
    } else {
        false
    };

    let attached_next = if index + 1 < messages.len() {
        let next = &messages[index + 1];
        if matches!(next.content, MessageContent::Service { .. }) {
            false
        } else if next.sender_id == current.sender_id && next.direction == current.direction {
            let next_sec = parse_timestamp_seconds(&next.timestamp);
            let cur_sec = parse_timestamp_seconds(&current.timestamp);
            match (next_sec, cur_sec) {
                (Some(n), Some(c)) => (n - c).abs() <= K_ATTACH_MESSAGE_TO_PREVIOUS_SECONDS_DELTA,
                _ => true,
            }
        } else {
            false
        }
    } else {
        false
    };

    (attached_prev, attached_next)
}

#[derive(Debug, Clone)]
pub struct RowHeightCache {
    pub measured_heights: std::collections::HashMap<u64, f32>,
    pub prefix_sums: Vec<f32>,
    pub total_height: f32,
    pub dirty: bool,
}

impl RowHeightCache {
    pub fn new() -> Self {
        Self {
            measured_heights: std::collections::HashMap::new(),
            prefix_sums: Vec::new(),
            total_height: 0.0,
            dirty: true,
        }
    }

    pub fn mark_dirty(&mut self) {
        self.dirty = true;
    }

    pub fn is_dirty(&self) -> bool {
        self.dirty
    }

    pub fn len(&self) -> usize {
        self.prefix_sums.len()
    }

    pub fn is_empty(&self) -> bool {
        self.prefix_sums.is_empty()
    }

    pub fn estimate_row_height(msg: &MessageItemModel, attached_prev: bool) -> f32 {
        let margin = if attached_prev { 2.0 } else { 8.0 };
        let estimated = match &msg.content {
            MessageContent::Service { .. } => 32.0,
            MessageContent::Text { text, .. } => {
                let lines: usize = text
                    .split('\n')
                    .map(|line| {
                        let cols: usize = line
                            .chars()
                            .map(|c| if (c as u32) > 0x2E7F { 2 } else { 1 })
                            .sum();
                        cols.max(1).div_ceil(42)
                    })
                    .sum();
                let lines = lines.max(1) as f32;
                let reply_h = if msg.reply_to.is_some() { 28.0 } else { 0.0 };
                let reaction_h = if !msg.reactions.is_empty() { 24.0 } else { 0.0 };
                let bot_kb_h = msg.bot_keyboard.as_ref().map_or(0.0, |rows| {
                    if rows.is_empty() {
                        0.0
                    } else {
                        (rows.len() as f32) * 38.0 + 4.0
                    }
                });
                (26.0 + reply_h + (lines * 20.0) + reaction_h + bot_kb_h + margin).clamp(42.0, 600.0)
            }
            MessageContent::Media { caption, .. } => {
                let caption_lines = caption.as_ref().map_or(0, |c| c.split('\n').count()) as f32;
                (240.0 + caption_lines * 18.0 + margin).min(600.0)
            }
            MessageContent::Voice { .. } => 56.0 + margin,
            MessageContent::Album { .. } => 260.0 + margin,
            MessageContent::Location { .. } => 180.0 + margin,
            MessageContent::Poll { options, .. } => {
                (52.0 + (options.len() as f32 * 34.0) + 36.0 + margin).min(600.0)
            }
            MessageContent::Call { .. } => 60.0 + margin,
            MessageContent::Story { .. } => 220.0 + margin,
            MessageContent::Invoice { .. } => 240.0 + margin,
            MessageContent::BotKeyboard { .. } => 60.0 + margin,
            MessageContent::GenerativeCard(card) => {
                (64.0 + (card.components.len() as f32 * 28.0) + margin).clamp(64.0, 600.0)
            }
            MessageContent::LocalBotAssistCard(card) => {
                use bot_browser::{AssistCardPayload, AssistCardStatus};
                let h = match &card.payload {
                    AssistCardPayload::VaultLogin { status, .. } => match status {
                        AssistCardStatus::AwaitingUser | AssistCardStatus::TimedOutResumable => {
                            340.0
                        }
                        _ => 112.0,
                    },
                    AssistCardPayload::ReuseConfirm { status, .. } => match status {
                        AssistCardStatus::AwaitingUser | AssistCardStatus::TimedOutResumable => {
                            210.0
                        }
                        _ => 112.0,
                    },
                    AssistCardPayload::QrLogin { status, .. } => match status {
                        AssistCardStatus::AwaitingUser | AssistCardStatus::TimedOutResumable => {
                            290.0
                        }
                        _ => 112.0,
                    },
                    AssistCardPayload::ClickCaptcha {
                        status,
                        element_rect,
                        ..
                    } => match status {
                        AssistCardStatus::AwaitingUser | AssistCardStatus::TimedOutResumable => {
                            let (_, disp_h) = compute_captcha_display_size(element_rect);
                            disp_h + 150.0
                        }
                        _ => 112.0,
                    },
                };
                h + margin
            }
        };
        estimated.clamp(24.0, 600.0)
    }

    pub fn rebuild_prefix_sums(&mut self, messages: &[MessageItemModel]) {
        self.prefix_sums.clear();
        self.prefix_sums.reserve(messages.len());
        let mut accum = 0.0;
        for (idx, msg) in messages.iter().enumerate() {
            let h = match self.measured_heights.get(&msg.id) {
                Some(&measured) => measured,
                None => {
                    let attached_prev = compute_attachment(messages, idx).0;
                    Self::estimate_row_height(msg, attached_prev)
                }
            };
            accum += h;
            self.prefix_sums.push(accum);
        }
        self.total_height = accum;
        self.dirty = false;
    }

    pub fn visible_range(
        &self,
        scroll_top: f32,
        viewport_height: f32,
        overscan_px: f32,
        total_items: usize,
    ) -> (usize, usize) {
        if total_items == 0 || self.prefix_sums.is_empty() {
            return (0, 0);
        }
        let top_bound = (scroll_top - overscan_px).max(0.0);
        let bottom_bound = scroll_top + viewport_height + overscan_px;

        let start_idx = match self.prefix_sums.binary_search_by(|&val| {
            val.partial_cmp(&top_bound).unwrap_or(std::cmp::Ordering::Equal)
        }) {
            Ok(ix) => ix,
            Err(ix) => ix.min(total_items.saturating_sub(1)),
        };

        let end_idx = match self.prefix_sums.binary_search_by(|&val| {
            val.partial_cmp(&bottom_bound).unwrap_or(std::cmp::Ordering::Equal)
        }) {
            Ok(ix) => ix,
            Err(ix) => ix.min(total_items.saturating_sub(1)),
        };

        (start_idx, end_idx.max(start_idx).min(total_items.saturating_sub(1)))
    }
}

impl Default for RowHeightCache {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct MessageContextMenuState {
    pub message_id: u64,
    pub position: Point<Pixels>,
    pub is_outgoing: bool,
    pub is_pinned: bool,
    pub text_snippet: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PinnedBannerEntry {
    pub msg_id: u64,
    pub preview_text: String,
    pub sender_name: String,
    pub thumb_url: Option<String>,
}

pub struct ConversationStage {
    pub active_chat_id: Option<u64>,
    pub chat_title: String,
    pub chat_subtitle: String,
    pub chat_avatar_initials: String,
    pub chat_avatar_color: u32,
    pub chat_avatar_url: Option<String>,
    pub pinned_message: Option<String>,
    pub messages: Vec<MessageItemModel>,
    pub is_inspector_open: bool,
    pub pending_call_request: bool,
    pub pending_video_call: bool,
    pub pending_media_url: Option<(String, bool)>,
    pub pending_media_viewer: Option<(Vec<(String, bool, Option<String>)>, usize)>,
    pub pending_iv_url: Option<(String, String)>,
    pub pending_bot_action: Option<String>,
    pub pending_bot_button_action: Option<(u64, InlineButtonType)>,
    pub pending_edit_message: Option<(u64, String)>,
    pub pending_forward_message_ids: Option<Vec<u64>>,
    pub pending_delete_message_ids: Option<Vec<u64>>,
    pub pending_pin_toggle: Option<(u64, bool)>,
    pub pending_translate_message_id: Option<u64>,
    pub pending_poll_vote: Option<(u64, u32)>,
    pub pending_reaction_toggle: Option<(u64, String)>,
    pub pending_reaction_actions: VecDeque<(u64, String, bool)>,
    pub pending_voice_playback: Option<(u64, bool, f32, f32)>,
    pub pending_download_url: Option<(String, String)>,
    pub pending_dropped_paths: Option<Vec<PathBuf>>,
    pub pending_open_profile_peer_id: Option<u64>,
    pub pending_clear_history: bool,
    pub pending_export_chat: bool,
    pub pending_toggle_mute: bool,
    pub pending_change_wallpaper: bool,
    pub pending_toggle_pin_chat: bool,
    pub pending_set_ttl_seconds: Option<u32>,
    pub pending_toggle_block_peer: bool,
    pub pending_delete_or_leave_chat: bool,
    pub is_muted_chat: bool,
    pub is_pinned_chat: bool,
    pub is_peer_blocked: bool,
    pub is_group_chat: bool,
    pub is_saved_messages: bool,
    pub message_ttl_seconds: u32,
    pub ttl_submenu_open: bool,
    pub chat_histories: std::collections::HashMap<u64, Vec<MessageItemModel>>,
    pub chat_pinned_messages: std::collections::HashMap<u64, Option<String>>,
    pub chat_pinned_entries: std::collections::HashMap<u64, Vec<PinnedBannerEntry>>,
    pub message_expirations: std::collections::HashMap<u64, i64>,
    pub scroll_handle: ScrollHandle,
    pub row_height_cache: RowHeightCache,
    pub is_anchored_to_bottom: bool,
    pub chat_scroll_positions: std::collections::HashMap<u64, (Point<Pixels>, bool)>,
    pub unread_incoming_count: usize,
    pub unread_anchor_msg_id: Option<u64>,
    pub active_context_menu: Option<MessageContextMenuState>,
    pub context_menu_reactions_expanded: bool,
    pub pending_reply: Option<ReplyReference>,
    pub simulated_max_offset: Option<Size<Pixels>>,
    pub simulated_viewport_height: Option<f32>,
    pub stage_bounds_probe: Rc<Cell<Option<Bounds<Pixels>>>>,
    pub transcript_bounds_probe: Rc<Cell<Option<Bounds<Pixels>>>>,
    pub bubble_widths_probe: Rc<RefCell<HashMap<u64, f32>>>,
    pub bubble_bounds_probe: Rc<RefCell<HashMap<u64, Bounds<Pixels>>>>,
    pub row_heights_probe: Rc<RefCell<HashMap<u64, f32>>>,
    pub selected_message_ids: Vec<u64>,
    pub select_mode_active: bool,
    pub blur_wallpaper: bool,
    pub loop_animated_stickers: bool,
    pub palette: TdesktopThemePalette,
    pub lang_id: String,
    pub message_meta: HashMap<u64, MessageRichMeta>,
    pub pinned_tracker: PinnedTracker,
    pub highlight_manager: HistoryViewHighlightManager,
    pub drag_area: HistoryDragArea,
    pub focus_handle: Option<FocusHandle>,
    pub search_open: bool,
    pub search_query: String,
    pub search_match_ids: Vec<u64>,
    pub search_match_cursor: usize,
    pub hovered_message_id: Option<u64>,
    pub hovered_gutter_id: Option<u64>,
    pub hovered_reaction_bar_id: Option<u64>,
    pub hovered_reaction_bridge_id: Option<u64>,
    pub quick_strip_msg_id: Option<u64>,
    pub hover_grace: Option<(u64, ReactionOverlayMode)>,
    pub hover_grace_generation: u64,
    pub reaction_picker_msg_id: Option<u64>,
    pub revealed_spoilers: HashSet<(u64, usize)>,
    pub expanded_quotes: HashSet<(u64, usize)>,
    pub copied_code_msg: Option<u64>,
    pub more_menu_open: bool,
    pub toast_banner: Option<String>,
    pub active_fabushi_bot: Option<gpui_telegram_proto::miniapp_contracts::FabushiBotSummaryDto>,
    pub is_bot_chat: bool,
    pub bot_activity_state: String,
    pub bot_activity_detail: Option<String>,
    pub bot_description_card: Option<String>,
    pub show_bot_start_bar: bool,
    pub is_viewer_owner_fabushi_bot: bool,
    pub pending_bot_start: bool,
    pub bot_assist_login_drafts: HashMap<String, BotAssistLoginDraft>,
    pub bot_assist_click_drafts: HashMap<String, BotAssistClickDraft>,
    pub active_bot_card_field: Option<(String, BotAssistLoginField)>,
    pub bot_assist_ime_preedit: Option<String>,
    pub pending_bot_assist_action: Option<BotAssistCardAction>,
}

impl ConversationStage {
    pub fn new() -> Self {
        let palette = TdesktopThemePalette::default();
        let mut stage = Self {
            active_chat_id: None,
            chat_title: "Select a chat".to_string(),
            chat_subtitle: String::new(),
            chat_avatar_initials: String::new(),
            chat_avatar_color: palette.input_bg,
            chat_avatar_url: None,
            pinned_message: None,
            messages: Vec::new(),
            is_inspector_open: false,
            pending_call_request: false,
            pending_video_call: false,
            pending_media_url: None,
            pending_media_viewer: None,
            pending_iv_url: None,
            pending_bot_action: None,
            pending_bot_button_action: None,
            pending_edit_message: None,
            pending_forward_message_ids: None,
            pending_delete_message_ids: None,
            pending_pin_toggle: None,
            pending_translate_message_id: None,
            pending_poll_vote: None,
            pending_reaction_toggle: None,
            pending_reaction_actions: VecDeque::new(),
            pending_voice_playback: None,
            pending_download_url: None,
            pending_dropped_paths: None,
            pending_open_profile_peer_id: None,
            pending_clear_history: false,
            pending_export_chat: false,
            pending_toggle_mute: false,
            pending_change_wallpaper: false,
            pending_toggle_pin_chat: false,
            pending_set_ttl_seconds: None,
            pending_toggle_block_peer: false,
            pending_delete_or_leave_chat: false,
            is_muted_chat: false,
            is_pinned_chat: false,
            is_peer_blocked: false,
            is_group_chat: false,
            is_saved_messages: false,
            message_ttl_seconds: 0,
            ttl_submenu_open: false,
            chat_histories: std::collections::HashMap::new(),
            chat_pinned_messages: std::collections::HashMap::new(),
            chat_pinned_entries: std::collections::HashMap::new(),
            message_expirations: std::collections::HashMap::new(),
            scroll_handle: ScrollHandle::new(),
            row_height_cache: RowHeightCache::new(),
            is_anchored_to_bottom: true,
            chat_scroll_positions: std::collections::HashMap::new(),
            unread_incoming_count: 0,
            unread_anchor_msg_id: None,
            active_context_menu: None,
            context_menu_reactions_expanded: false,
            pending_reply: None,
            simulated_max_offset: None,
            simulated_viewport_height: None,
            stage_bounds_probe: Rc::new(Cell::new(None)),
            transcript_bounds_probe: Rc::new(Cell::new(None)),
            bubble_widths_probe: Rc::new(RefCell::new(HashMap::new())),
            bubble_bounds_probe: Rc::new(RefCell::new(HashMap::new())),
            row_heights_probe: Rc::new(RefCell::new(HashMap::new())),
            selected_message_ids: Vec::new(),
            select_mode_active: false,
            blur_wallpaper: false,
            loop_animated_stickers: true,
            palette,
            lang_id: "en".to_string(),
            message_meta: HashMap::new(),
            pinned_tracker: PinnedTracker::new(false),
            highlight_manager: HistoryViewHighlightManager::new(),
            drag_area: HistoryDragArea::new(),
            focus_handle: None,
            search_open: false,
            search_query: String::new(),
            search_match_ids: Vec::new(),
            search_match_cursor: 0,
            hovered_message_id: None,
            hovered_gutter_id: None,
            hovered_reaction_bar_id: None,
            hovered_reaction_bridge_id: None,
            quick_strip_msg_id: None,
            hover_grace: None,
            hover_grace_generation: 0,
            reaction_picker_msg_id: None,
            revealed_spoilers: HashSet::new(),
            expanded_quotes: HashSet::new(),
            copied_code_msg: None,
            more_menu_open: bool::default(),
            toast_banner: None,
            active_fabushi_bot: None,
            is_bot_chat: false,
            bot_activity_state: "idle".to_string(),
            bot_activity_detail: None,
            bot_description_card: None,
            show_bot_start_bar: false,
            is_viewer_owner_fabushi_bot: false,
            pending_bot_start: false,
            bot_assist_login_drafts: HashMap::new(),
            bot_assist_click_drafts: HashMap::new(),
            active_bot_card_field: None,
            bot_assist_ime_preedit: None,
            pending_bot_assist_action: None,
        };
        stage.row_height_cache.rebuild_prefix_sums(&stage.messages);
        stage
    }

    pub fn reset(&mut self) {
        self.clear_selection();
        self.chat_histories.clear();
        self.chat_pinned_messages.clear();
        self.chat_pinned_entries.clear();
        self.message_expirations.clear();
        self.chat_scroll_positions.clear();
        self.message_meta.clear();
        self.revealed_spoilers.clear();
        self.expanded_quotes.clear();
        self.bot_description_card = None;
        self.show_bot_start_bar = false;
        self.is_bot_chat = false;
        self.bot_activity_state = "idle".to_string();
        self.bot_activity_detail = None;
        self.is_viewer_owner_fabushi_bot = false;
        self.pending_bot_start = false;
        self.clear_all_bot_card_drafts();
    }

    pub fn sync_measured_row_heights(&mut self) -> bool {
        let mut changed = false;
        let probe = self.row_heights_probe.borrow();
        for (&msg_id, &measured_h) in probe.iter() {
            if measured_h > 1.0 {
                let needs_update = self
                    .row_height_cache
                    .measured_heights
                    .get(&msg_id)
                    .map_or(true, |&prev| (prev - measured_h).abs() > 0.5);
                if needs_update {
                    self.row_height_cache
                        .measured_heights
                        .insert(msg_id, measured_h);
                    changed = true;
                }
            }
        }
        drop(probe);
        if changed {
            self.row_height_cache.mark_dirty();
        }
        changed
    }

    pub fn configure_for_cloud_workspace(&mut self, _server_url: &str, _username: Option<&str>) {
        self.reset();
    }

    pub fn set_active_fabushi_bot(
        &mut self,
        bot: Option<gpui_telegram_proto::miniapp_contracts::FabushiBotSummaryDto>,
    ) {
        if bot.is_some() {
            self.is_bot_chat = true;
        }
        self.active_fabushi_bot = bot;
    }

    pub fn set_bot_activity(&mut self, state: &str, detail: Option<String>) {
        let trimmed = state.trim();
        self.bot_activity_state = if trimmed.is_empty() {
            "idle".to_string()
        } else {
            trimmed.to_string()
        };
        self.bot_activity_detail = detail.filter(|d| !d.trim().is_empty());
    }

    pub fn set_bot_prestart_state(
        &mut self,
        description_card: Option<String>,
        show_start_bar: bool,
        is_viewer_owner_fabushi: bool,
    ) {
        self.bot_description_card = description_card;
        self.show_bot_start_bar = show_start_bar;
        self.is_viewer_owner_fabushi_bot = is_viewer_owner_fabushi;
        if !show_start_bar {
            self.pending_bot_start = false;
        }
    }

    pub fn set_chat(&mut self, chat_id: u64, title: &str, subtitle: &str, initials: &str, color: u32) {
        if let Some(prev_id) = self.active_chat_id {
            self.chat_histories.insert(prev_id, self.messages.clone());
            self.chat_pinned_messages.insert(prev_id, self.pinned_message.clone());
            self.chat_scroll_positions.insert(prev_id, (self.scroll_handle.offset(), self.is_at_bottom()));
        }
        if self.active_chat_id != Some(chat_id) {
            self.active_context_menu = None;
            self.context_menu_reactions_expanded = false;
            self.pending_reply = None;
            self.is_muted_chat = false;
            self.is_pinned_chat = false;
            self.is_peer_blocked = false;
            self.is_group_chat = false;
            self.is_saved_messages = false;
            self.message_ttl_seconds = 0;
            self.bot_description_card = None;
            self.show_bot_start_bar = false;
            self.is_bot_chat = false;
            self.bot_activity_state = "idle".to_string();
            self.bot_activity_detail = None;
            self.is_viewer_owner_fabushi_bot = false;
            self.pending_bot_start = false;
            self.active_bot_card_field = None;
            self.bot_assist_ime_preedit = None;
            self.pending_bot_assist_action = None;
        }
        self.active_chat_id = Some(chat_id);
        self.chat_title = title.to_string();
        self.chat_subtitle = subtitle.to_string();
        self.chat_avatar_initials = initials.to_string();
        self.chat_avatar_color = color;
        self.active_context_menu = None;
        self.context_menu_reactions_expanded = false;
        self.hovered_message_id = None;
        self.hovered_gutter_id = None;
        self.hovered_reaction_bar_id = None;
        self.hovered_reaction_bridge_id = None;
        self.quick_strip_msg_id = None;
        self.hover_grace = None;
        self.hover_grace_generation = self.hover_grace_generation.wrapping_add(1);
        self.reaction_picker_msg_id = None;
        self.bubble_widths_probe.borrow_mut().clear();
        self.bubble_bounds_probe.borrow_mut().clear();
        self.row_heights_probe.borrow_mut().clear();
        self.row_height_cache.measured_heights.clear();
        self.selected_message_ids.clear();
        self.select_mode_active = false;
        self.more_menu_open = false;
        self.ttl_submenu_open = false;
        self.search_open = false;
        self.search_query.clear();
        self.search_match_ids.clear();
        self.search_match_cursor = 0;

        // 1. Cleanly load history without cross-chat bleeding
        self.messages = self.chat_histories.get(&chat_id).cloned().unwrap_or_default();
        self.row_height_cache.mark_dirty();

        // 2. Cleanly load pinned banner without cross-chat bleeding + sync multi-pin tracker
        let mut pinned_ids: Vec<i64> = self
            .messages
            .iter()
            .filter(|m| m.is_pinned)
            .map(|m| m.id as i64)
            .collect();
        if let Some(entries) = self.chat_pinned_entries.get(&chat_id) {
            for e in entries {
                let id = e.msg_id as i64;
                if !pinned_ids.contains(&id) {
                    pinned_ids.push(id);
                }
            }
        }
        self.pinned_tracker.set_pinned_ids(pinned_ids);
        self.pinned_tracker.reset_to_latest();
        self.pinned_message = self
            .chat_pinned_entries
            .get(&chat_id)
            .and_then(|v| {
                let active_id = self.pinned_tracker.current_msg_id().map(|id| id as u64);
                active_id
                    .and_then(|aid| v.iter().find(|e| e.msg_id == aid))
                    .or_else(|| v.last())
                    .map(|e| e.preview_text.clone())
            })
            .or_else(|| self.chat_pinned_messages.get(&chat_id).cloned().flatten())
            .or_else(|| {
                self.messages.iter().rev().find(|m| m.is_pinned).and_then(|m| match &m.content {
                    MessageContent::Text { text, .. } => Some(text.lines().next().unwrap_or(text).to_string()),
                    MessageContent::Media { caption, .. } | MessageContent::Album { caption, .. } => caption.clone(),
                    _ => None,
                })
            });

        // 3. Restore scroll position or scroll to bottom
        if let Some(&(saved_offset, was_at_bottom)) = self.chat_scroll_positions.get(&chat_id) {
            if !was_at_bottom {
                self.scroll_handle.set_offset(saved_offset);
                self.is_anchored_to_bottom = false;
            } else {
                self.scroll_to_bottom();
            }
        } else {
            self.scroll_to_bottom();
        }
    }
    fn build_visible_message_rows_apple(
        &self,
        start_idx: usize,
        end_idx: usize,
        is_group_chat: bool,
        stage_w: f32,
        highlight_paint: Option<(u64, ChatPaintHighlight)>,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> Vec<AnyElement> {
        let total_msgs = self.messages.len();
        let selected_set: HashSet<u64> = self.selected_message_ids.iter().copied().collect();
        let has_any_selection = self.select_mode_active || !selected_set.is_empty();
        let mut message_rows =
            Vec::with_capacity(if total_msgs == 0 { 0 } else { end_idx - start_idx + 1 });
        if total_msgs == 0 {
            return message_rows;
        }
        let last_outgoing_id = self
            .messages
            .iter()
            .rev()
            .find(|m| {
                m.direction == MessageDirection::Outgoing
                    && !matches!(m.content, MessageContent::Service { .. })
            })
            .map(|m| m.id);

        for idx in start_idx..=end_idx.min(total_msgs.saturating_sub(1)) {
            let msg = &self.messages[idx];
            let (attached_prev, attached_next) = compute_attachment(&self.messages, idx);
            let is_outgoing = msg.direction == MessageDirection::Outgoing;
            let group = BubbleGroupLayout {
                attached_to_prev: attached_prev,
                attached_to_next: attached_next,
                show_sender_name: !is_outgoing && !attached_prev && is_group_chat,
                show_bottom_avatar: !is_outgoing && !attached_next,
                show_tail: !attached_next,
                reserve_avatar_column: !is_outgoing && is_group_chat,
            };
            let meta = self.message_meta.get(&msg.id);
            let prev_date = if idx > 0 {
                self.message_meta
                    .get(&self.messages[idx - 1].id)
                    .and_then(|m| m.date_group_label.as_deref())
            } else {
                None
            };
            let cur_date = meta.and_then(|m| m.date_group_label.as_deref());
            let date_divider = match (prev_date, cur_date) {
                (Some(p), Some(c)) if p != c => Some(c),
                (None, Some(c)) if idx == 0 => Some(c),
                _ => None,
            };
            let hl_alpha = highlight_paint
                .as_ref()
                .and_then(|(hid, p)| if *hid == msg.id { Some(p.opacity) } else { None })
                .unwrap_or(0.0);

            let (is_within_viewport, reaction_overlay_layout) =
                self.compute_reaction_overlay_layout(idx, msg, group.reserve_avatar_column);
            let overlay_mode = if is_within_viewport {
                self.reaction_overlay_mode(msg.id)
            } else {
                None
            };
            let is_last_outgoing = last_outgoing_id == Some(msg.id);
            let is_row_hovered =
                self.hovered_message_id == Some(msg.id) || self.hovered_gutter_id == Some(msg.id);

            message_rows.push(render_message_row_apple(
                msg,
                meta,
                self.active_fabushi_bot.as_ref(),
                group,
                stage_w,
                selected_set.contains(&msg.id),
                has_any_selection,
                is_last_outgoing,
                is_row_hovered,
                overlay_mode,
                reaction_overlay_layout,
                Some(self.bubble_widths_probe.clone()),
                Some(self.bubble_bounds_probe.clone()),
                Some(self.row_heights_probe.clone()),
                hl_alpha,
                self.unread_anchor_msg_id == Some(msg.id),
                date_divider,
                &self.revealed_spoilers,
                &self.expanded_quotes,
                self.copied_code_msg,
                Some(BotAssistRenderContext {
                    login_drafts: &self.bot_assist_login_drafts,
                    click_drafts: &self.bot_assist_click_drafts,
                    active_field: self.active_bot_card_field.as_ref(),
                    ime_preedit: self.bot_assist_ime_preedit.as_deref(),
                }),
                &self.palette,
                &self.lang_id,
                window,
                cx,
                |this, action, cx| this.handle_bubble_action(action, cx),
            ));
        }
        message_rows
    }

    fn render_apple_sticky_date(&self, date_lbl: String, top_px: f32) -> AnyElement {
        div()
            .absolute()
            .top(px(top_px))
            .left_0()
            .right_0()
            .flex()
            .justify_center()
            .child(
                div()
                    .px(px(10.0))
                    .py(px(2.0))
                    .rounded_full()
                    .bg(TdesktopThemePalette::rgba_with_alpha(self.palette.stage_bg, 0.86))
                    .text_size(px(11.0))
                    .font_weight(FontWeight::MEDIUM)
                    .text_color(rgb(self.palette.text_secondary))
                    .child(date_lbl),
            )
            .into_any_element()
    }
}

impl Default for ConversationStage {
    fn default() -> Self {
        Self::new()
    }
}

impl Render for ConversationStage {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if self.focus_handle.is_none() {
            self.focus_handle = Some(cx.focus_handle());
        }
        let focus = self.focus_handle.clone().unwrap();

        let has_active_chat = self.active_chat_id.is_some();
        let total_msgs = self.messages.len();
        self.sync_measured_row_heights();
        if self.row_height_cache.is_dirty() || self.row_height_cache.len() != total_msgs {
            self.row_height_cache.rebuild_prefix_sums(&self.messages);
        }
        if self.is_at_bottom() {
            self.unread_incoming_count = 0;
        }

        let viewport_h = self.effective_viewport_height();
        let raw_scroll_top = f32::from(-self.scroll_handle.offset().y).max(0.0);
        let overscan_px = 600.0;
        let (start_idx, end_idx) = if total_msgs <= 64 {
            (0, total_msgs.saturating_sub(1))
        } else {
            let max_scroll_top = (self.row_height_cache.total_height + 60.0 - viewport_h).max(0.0);
            let effective_scroll_top = raw_scroll_top.min(max_scroll_top);
            self.row_height_cache
                .visible_range(effective_scroll_top, viewport_h, overscan_px, total_msgs)
        };
        let top_spacer_height = if start_idx > 0 && !self.row_height_cache.prefix_sums.is_empty() {
            self.row_height_cache.prefix_sums[(start_idx - 1).min(self.row_height_cache.prefix_sums.len() - 1)]
        } else {
            0.0
        };
        let bottom_spacer_height = if end_idx < total_msgs.saturating_sub(1) && !self.row_height_cache.prefix_sums.is_empty() {
            (self.row_height_cache.total_height - self.row_height_cache.prefix_sums[end_idx.min(self.row_height_cache.prefix_sums.len() - 1)]).max(0.0)
        } else {
            0.0
        };

        let highlight_paint = self.highlight_manager.paint_state(stage_now_ms());
        let is_group_chat = self.is_group_chat
            || self.chat_subtitle.contains("member")
            || self.chat_subtitle.contains("subscriber")
            || self.chat_subtitle == "group"
            || self.messages.iter().filter(|m| m.direction == MessageDirection::Incoming).map(|m| m.sender_id).collect::<HashSet<_>>().len() > 1;
        let can_block_peer = !is_group_chat && !self.is_saved_messages;
        let is_apple = self.palette.is_apple_style();
        let stage_w = self.effective_stage_width();
        let message_rows = if is_apple {
            self.build_visible_message_rows_apple(
                start_idx,
                end_idx,
                is_group_chat,
                stage_w,
                highlight_paint,
                window,
                cx,
            )
        } else {
            self.build_visible_message_rows(
                start_idx,
                end_idx,
                is_group_chat,
                stage_w,
                highlight_paint,
                window,
                cx,
            )
        };
        let pinned_info = self.pinned_banner_info();
        let show_top_chrome = has_active_chat
            || total_msgs > 0
            || self.show_bot_start_bar
            || self.bot_description_card.is_some();
        let show_fabushi_bar =
            show_top_chrome && self.is_viewer_owner_fabushi_bot && self.active_fabushi_bot.is_some();
        let apple_top_inset = if !show_top_chrome {
            14.0
        } else {
            self.palette.scaled_px(52.0)
                + if show_fabushi_bar { 32.0 } else { 0.0 }
                + if self.search_open { 44.0 } else { 0.0 }
                + if pinned_info.is_some() { 46.0 } else { 0.0 }
                + 12.0
        };
        let sticky_date_pill = if is_apple {
            if raw_scroll_top > 24.0 {
                self.sticky_date_label(start_idx)
                    .map(|lbl| self.render_apple_sticky_date(lbl, apple_top_inset - 4.0))
            } else {
                None
            }
        } else {
            self.sticky_date_label(start_idx)
                .map(|lbl| self.render_sticky_date_pill(lbl))
        };
        let toast_pill = self.toast_banner.clone().map(|toast| self.render_toast_banner_pill(toast, cx));
        let drag_overlay = self.drag_area.is_active().then(|| self.render_drag_area_overlay());
        let show_scroll_fab = has_active_chat && total_msgs > 0 && (!self.is_at_bottom() || self.unread_incoming_count > 0);
        let accent_hex = self.palette.accent;

        let bounds_probe_cell = self.stage_bounds_probe.clone();
        let transcript_probe_cell = self.transcript_bounds_probe.clone();
        let mut root = div()
            .id("conversation-stage-root")
            .track_focus(&focus)
            .key_context("ConversationStage")
            .on_key_down(cx.listener(|this, event: &KeyDownEvent, _window, cx| {
                this.handle_stage_key_down(event, cx);
            }))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, _, _, cx| {
                    if this.reaction_picker_msg_id.is_some() {
                        this.reaction_picker_msg_id = None;
                        cx.notify();
                    }
                }),
            )
            .drag_over::<gpui::ExternalPaths>(move |style, _paths: &gpui::ExternalPaths, _, _| {
                style.border_2().border_color(rgb(accent_hex))
            })
            .on_drop::<gpui::ExternalPaths>(cx.listener(|this, paths: &gpui::ExternalPaths, _, cx| {
                let dropped: Vec<PathBuf> = paths.paths().to_vec();
                if !dropped.is_empty() {
                    let strings: Vec<String> = dropped.iter().map(|p| p.display().to_string()).collect();
                    let (_zone, _) = this.drag_area.drop_files(strings, false);
                    this.pending_dropped_paths = Some(dropped);
                    this.toast_banner = Some(
                        crate::i18n::tr_str(&this.lang_id, "已添加拖入文件为附件", "Dropped files ready to attach").to_string(),
                    );
                    cx.notify();
                }
            }))
            .relative()
            .flex_1()
            .min_h(px(0.0))
            .w_full()
            .overflow_hidden()
            .bg(rgb(if is_apple { self.palette.stage_bg } else { self.palette.history_bg }))
            .flex()
            .flex_col()
            .child(
                canvas(
                    move |bounds, _window, _cx| {
                        bounds_probe_cell.set(Some(bounds));
                    },
                    |_, _, _, _| {},
                )
                .absolute()
                .inset_0()
                .size_full(),
            );

        let mut top_chrome_children: Vec<AnyElement> = Vec::new();
        if show_top_chrome {
            top_chrome_children.push(render_stage_top_bar(
                self.active_chat_id,
                &self.chat_title,
                &self.chat_subtitle,
                &self.chat_avatar_initials,
                self.chat_avatar_url.as_deref(),
                self.active_fabushi_bot.as_ref(),
                self.is_bot_chat,
                &self.bot_activity_state,
                self.select_mode_active,
                self.selected_message_ids.len(),
                self.message_ttl_seconds,
                self.search_open,
                &self.search_query,
                self.search_match_cursor,
                self.search_match_ids.len(),
                self.is_inspector_open,
                self.more_menu_open,
                &self.palette,
                &self.lang_id,
                cx,
                |this, action, cx| this.handle_topbar_action(action, cx),
            ));
            if self.is_viewer_owner_fabushi_bot {
                if let Some(fab_bot) = self.active_fabushi_bot.clone() {
                    let fab_color = crate::components::avatar::parse_fabushi_hex_color(&fab_bot.avatar_color)
                        .unwrap_or(self.palette.accent);
                    let fab_meta = format!(
                        "isolated · {} · {}",
                        fab_bot.thinking_level, fab_bot.approval_mode
                    );
                    let fab_model = fab_bot.preferred_model.trim().to_string();
                    let mut left = div()
                        .flex()
                        .items_center()
                        .gap(px(8.0))
                        .min_w(px(0.0))
                        .overflow_hidden()
                        .child(crate::components::grok_bot_avatar::render_grok_persona_avatar(
                            &fab_bot.agent_id,
                            &fab_bot.avatar_shape,
                            &fab_bot.avatar_color,
                            &fab_bot.display_name,
                            18.0,
                            crate::components::grok_bot_avatar::GrokPersonaState::from_activity_str(
                                &fab_bot.execution_state,
                                false,
                            ),
                            self.palette.card_bg,
                            false,
                        ))
                        .child(
                            div()
                                .px(px(6.0))
                                .py(px(1.0))
                                .rounded(px(999.0))
                                .bg(TdesktopThemePalette::rgba_with_alpha(fab_color, 0.14))
                                .text_size(px(10.5))
                                .font_weight(FontWeight::SEMIBOLD)
                                .text_color(rgb(fab_color))
                                .child(format!("@{}", fab_bot.agent_id)),
                        )
                        .child(
                            div()
                                .text_size(px(11.0))
                                .text_color(rgb(self.palette.text_secondary))
                                .child(fab_meta),
                        );
                    if !fab_model.is_empty() {
                        left = left.child(
                            div()
                                .px(px(6.0))
                                .py(px(1.0))
                                .rounded(px(6.0))
                                .bg(rgb(self.palette.input_bg))
                                .text_size(px(10.5))
                                .text_color(rgb(self.palette.text_primary))
                                .child(fab_model),
                        );
                    }
                    let insp_label = if self.is_inspector_open {
                        crate::i18n::tr_str(&self.lang_id, "收起面板", "Hide Inspector")
                    } else {
                        crate::i18n::tr_str(&self.lang_id, "智能体面板", "Inspector")
                    };
                    let bar_bg: Rgba = if is_apple {
                        TdesktopThemePalette::rgba_with_alpha(self.palette.topbar_bg, 0.90)
                    } else {
                        rgb(self.palette.top_bar_bg).into()
                    };
                    let sep_col: Rgba = if is_apple {
                        self.palette.separator_rgba()
                    } else {
                        rgb(self.palette.border_fg).into()
                    };
                    top_chrome_children.push(
                        div()
                            .id("stage-fabushi-session-bar")
                            .w_full()
                            .h(px(32.0))
                            .px(px(if is_apple { 16.0 } else { 14.0 }))
                            .flex()
                            .items_center()
                            .justify_between()
                            .gap(px(8.0))
                            .bg(bar_bg)
                            .border_b_1()
                            .border_color(sep_col)
                            .child(left)
                            .child(
                                div()
                                    .flex()
                                    .items_center()
                                    .gap(px(6.0))
                                    .flex_shrink_0()
                                    .child(
                                        div()
                                            .id("stage-fabushi-reset-session")
                                            .cursor_pointer()
                                            .px(px(8.0))
                                            .py(px(2.0))
                                            .rounded(px(if is_apple { 999.0 } else { 6.0 }))
                                            .bg(rgb(self.palette.input_bg))
                                            .border_1()
                                            .border_color(sep_col)
                                            .text_size(px(11.0))
                                            .font_weight(FontWeight::MEDIUM)
                                            .text_color(rgb(self.palette.text_primary))
                                            .on_mouse_down(
                                                MouseButton::Left,
                                                cx.listener(|this, _, _, cx| {
                                                    this.pending_bot_action = Some("hermes:/new".to_string());
                                                    cx.notify();
                                                }),
                                            )
                                            .child("/new"),
                                    )
                                    .child(
                                        div()
                                            .id("stage-fabushi-open-inspector")
                                            .cursor_pointer()
                                            .px(px(8.0))
                                            .py(px(2.0))
                                            .rounded(px(if is_apple { 999.0 } else { 6.0 }))
                                            .bg(TdesktopThemePalette::rgba_with_alpha(self.palette.accent, 0.14))
                                            .text_size(px(11.0))
                                            .font_weight(FontWeight::SEMIBOLD)
                                            .text_color(rgb(self.palette.accent))
                                            .on_mouse_down(
                                                MouseButton::Left,
                                                cx.listener(|this, _, _, cx| {
                                                    this.handle_topbar_action(TopBarUiAction::ToggleInspector, cx);
                                                }),
                                            )
                                            .child(insp_label),
                                    ),
                            )
                            .into_any_element(),
                    );
                }
            }
            if self.search_open {
                top_chrome_children.push(render_stage_search_bar(
                    &self.search_query,
                    self.search_match_cursor,
                    self.search_match_ids.len(),
                    &self.palette,
                    &self.lang_id,
                    cx,
                    |this, action, cx| this.handle_topbar_action(action, cx),
                ));
            }
        }

        if let Some((snippet, sender, thumb, count, active_idx)) = pinned_info {
            top_chrome_children.push(render_pinned_banner_46px(
                &snippet,
                &sender,
                thumb.as_deref(),
                count,
                active_idx,
                &self.palette,
                &self.lang_id,
                cx,
                |this, action, cx| this.handle_topbar_action(action, cx),
            ));
        }

        if !is_apple {
            for child in top_chrome_children.drain(..) {
                root = root.child(child);
            }
        }

        let bot_desc_card = self.bot_description_card.as_ref().map(|desc| {
            let card_bg = if is_apple {
                self.palette.card_surface_rgba()
            } else {
                TdesktopThemePalette::rgba_with_alpha(self.palette.card_bg, 0.96)
            };
            let body_text = if desc.trim().is_empty() {
                crate::i18n::tr_str(
                    &self.lang_id,
                    "点击下方开始使用此机器人。",
                    "Tap Start below to begin.",
                )
                .to_string()
            } else {
                desc.clone()
            };
            let mut card = div()
                .w(px(340.0))
                .max_w(px(380.0))
                .px(px(18.0))
                .py(px(14.0))
                .rounded(px(14.0))
                .bg(card_bg)
                .border_1()
                .border_color(if is_apple {
                    self.palette.separator_rgba()
                } else {
                    rgb(self.palette.border_fg).into()
                })
                .flex()
                .flex_col()
                .gap(px(6.0))
                .child(
                    div()
                        .text_size(px(13.5))
                        .font_weight(FontWeight::BOLD)
                        .text_color(rgb(self.palette.text_primary))
                        .child(crate::i18n::tr_str(
                            &self.lang_id,
                            "它能做什么？",
                            "What can this bot do?",
                        )),
                )
                .child(
                    div()
                        .text_size(px(13.0))
                        .line_height(relative(1.45))
                        .text_color(rgb(self.palette.text_primary))
                        .child(body_text),
                );
            card = if is_apple {
                card.shadow(self.palette.popover_shadows())
            } else {
                card.shadow_sm()
            };
            div()
                .id("stage-bot-description-card")
                .w_full()
                .my(px(16.0))
                .flex()
                .justify_center()
                .child(card)
                .into_any_element()
        });

        let transcript = div()
            .relative()
            .flex_1()
            .min_h(px(0.0))
            .w_full()
            .overflow_hidden()
            .child(
                canvas(
                    move |bounds, _window, _cx| {
                        transcript_probe_cell.set(Some(bounds));
                    },
                    |_, _, _, _| {},
                )
                .absolute()
                .inset_0()
                .size_full(),
            )
            .child(render_themed_wallpaper(&self.palette))
            .child(
                div()
                    .id("stage-messages-scroll")
                    .relative()
                    .size_full()
                    .px(px(12.0))
                    .pt(px(if is_apple { apple_top_inset } else { 14.0 }))
                    .pb(px(if is_apple { 28.0 } else { 46.0 }))
                    .overflow_y_scroll()
                    .track_scroll(&self.scroll_handle)
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(|this, _, _, cx| {
                            if this.reaction_picker_msg_id.is_some() {
                                this.reaction_picker_msg_id = None;
                                cx.notify();
                            }
                        }),
                    )
                    .flex()
                    .flex_col()
                    .when(!has_active_chat && total_msgs == 0 && bot_desc_card.is_none(), |el| {
                        el.child(render_empty_no_chat_pill(&self.palette, &self.lang_id))
                    })
                    .when(has_active_chat && total_msgs == 0 && bot_desc_card.is_none(), |el| {
                        el.child(render_empty_history_placeholder(&self.palette, &self.lang_id))
                    })
                    .when_some(bot_desc_card, |el, card| el.child(card))
                    .when(top_spacer_height > 0.0, |el| {
                        el.child(div().id("stage-top-spacer").w_full().h(px(top_spacer_height)).flex_shrink_0())
                    })
                    .children(message_rows)
                    .when(
                        self.is_bot_chat
                            && !self.bot_activity_state.is_empty()
                            && !self.bot_activity_state.eq_ignore_ascii_case("idle"),
                        |el| {
                            let status_label = match self.bot_activity_state.as_str() {
                                "tool" => self
                                    .bot_activity_detail
                                    .as_deref()
                                    .map(|t| format!("正在调用工具: {t}"))
                                    .unwrap_or_else(|| "正在执行工具...".to_string()),
                                "writing" => self
                                    .bot_activity_detail
                                    .clone()
                                    .unwrap_or_else(|| "正在组织回复...".to_string()),
                                _ => self
                                    .bot_activity_detail
                                    .clone()
                                    .unwrap_or_else(|| "正在深度思考...".to_string()),
                            };
                            if is_apple {
                                // Authentic Apple Messages incoming gray bubble with tail + 3 breathing dots
                                let phase = ((stage_now_ms() / 320) % 3) as usize;
                                let dot_alpha = |idx: usize| -> f32 {
                                    match (idx + 3 - phase) % 3 {
                                        0 => 0.92,
                                        1 => 0.58,
                                        _ => 0.34,
                                    }
                                };
                                let dot_hex = self.palette.text_secondary;
                                let show_detail = self.bot_activity_detail.is_some()
                                    || self.bot_activity_state == "tool";
                                el.child(
                                    div()
                                        .id("stage-bot-thinking-indicator")
                                        .w_full()
                                        .px(px(16.0))
                                        .pt(px(10.0))
                                        .pb(px(4.0))
                                        .flex_shrink_0()
                                        .flex()
                                        .flex_col()
                                        .items_start()
                                        .gap(px(4.0))
                                        .child(
                                            div()
                                                .relative()
                                                .flex()
                                                .flex_shrink_0()
                                                .child(
                                                    div()
                                                        .absolute()
                                                        .bottom(px(0.0))
                                                        .left(px(-6.0))
                                                        .child(tg_icon(
                                                            IconName::BubbleTailIn,
                                                            16.0,
                                                            self.palette.msg_in_bg,
                                                        )),
                                                )
                                                .child(
                                                    div()
                                                        .relative()
                                                        .px(px(14.0))
                                                        .py(px(11.0))
                                                        .rounded(px(18.0))
                                                        .bg(rgb(self.palette.msg_in_bg))
                                                        .flex()
                                                        .items_center()
                                                        .gap(px(5.0))
                                                        .child(
                                                            div()
                                                                .w(px(7.5))
                                                                .h(px(7.5))
                                                                .rounded_full()
                                                                .bg(TdesktopThemePalette::rgba_with_alpha(
                                                                    dot_hex,
                                                                    dot_alpha(0),
                                                                )),
                                                        )
                                                        .child(
                                                            div()
                                                                .w(px(7.5))
                                                                .h(px(7.5))
                                                                .rounded_full()
                                                                .bg(TdesktopThemePalette::rgba_with_alpha(
                                                                    dot_hex,
                                                                    dot_alpha(1),
                                                                )),
                                                        )
                                                        .child(
                                                            div()
                                                                .w(px(7.5))
                                                                .h(px(7.5))
                                                                .rounded_full()
                                                                .bg(TdesktopThemePalette::rgba_with_alpha(
                                                                    dot_hex,
                                                                    dot_alpha(2),
                                                                )),
                                                        ),
                                                ),
                                        )
                                        .when(show_detail, |col| {
                                            col.child(
                                                div()
                                                    .pl(px(6.0))
                                                    .text_size(px(11.5))
                                                    .font_weight(FontWeight::MEDIUM)
                                                    .text_color(rgb(self.palette.text_secondary))
                                                    .child(status_label),
                                            )
                                        }),
                                )
                            } else {
                                let (agent_seed, shape, color) = self
                                    .active_fabushi_bot
                                    .as_ref()
                                    .map(|fab| {
                                        (
                                            fab.agent_id.clone(),
                                            fab.avatar_shape.clone(),
                                            fab.avatar_color.clone(),
                                        )
                                    })
                                    .unwrap_or_else(|| {
                                        let seed = format!(
                                            "bot_{}_{}",
                                            self.active_chat_id.unwrap_or(0),
                                            self.chat_title
                                        );
                                        (
                                            seed.clone(),
                                            gpui_telegram_proto::miniapp_contracts::resolve_grok_persona_shape(&seed, None),
                                            gpui_telegram_proto::miniapp_contracts::resolve_grok_persona_color(&seed, None),
                                        )
                                    });
                                let persona_state =
                                    crate::components::grok_bot_avatar::GrokPersonaState::from_activity_str(
                                        &self.bot_activity_state,
                                        true,
                                    );
                                el.child(
                                    div()
                                        .id("stage-bot-thinking-indicator")
                                        .px(px(14.0))
                                        .py(px(8.0))
                                        .mx(px(10.0))
                                        .my(px(4.0))
                                        .rounded(px(14.0))
                                        .bg(rgb(self.palette.card_bg))
                                        .border_1()
                                        .border_color(rgb(self.palette.border_fg))
                                        .flex()
                                        .items_center()
                                        .gap(px(10.0))
                                        .child(
                                            crate::components::grok_bot_avatar::render_grok_bot_avatar(
                                                crate::components::grok_bot_avatar::GrokBotAvatarProps::new(
                                                    &agent_seed,
                                                    &shape,
                                                    &color,
                                                    30.0,
                                                )
                                                .with_state(persona_state)
                                                .with_surface_bg(self.palette.card_bg),
                                            ),
                                        )
                                        .child(
                                            div()
                                                .text_size(px(12.5))
                                                .font_weight(FontWeight::MEDIUM)
                                                .text_color(rgb(self.palette.text_secondary))
                                                .child(status_label),
                                        ),
                                )
                            }
                        },
                    )
                    .when(bottom_spacer_height > 0.0, |el| {
                        el.child(div().id("stage-bottom-spacer").w_full().h(px(bottom_spacer_height)).flex_shrink_0())
                    }),
            )
            .when(is_apple, |el| {
                // Soft bottom feather gradient blending transcript seamlessly into the floating capsule composer
                let bottom_feather = linear_gradient(
                    180.0,
                    linear_color_stop(TdesktopThemePalette::rgba_with_alpha(self.palette.stage_bg, 0.0), 0.0),
                    linear_color_stop(TdesktopThemePalette::rgba_with_alpha(self.palette.stage_bg, 0.90), 1.0),
                );
                el.child(
                    div()
                        .absolute()
                        .bottom_0()
                        .left_0()
                        .right_0()
                        .h(px(14.0))
                        .bg(bottom_feather),
                )
            })
            .when_some(sticky_date_pill, |el, pill| el.child(pill))
            .when_some(toast_pill, |el, pill| el.child(pill))
            .when(show_scroll_fab, |el| {
                let unread = self.unread_incoming_count as u32;
                el.child(render_scroll_to_bottom_fab(unread, &self.palette, cx, |this, cx| {
                    this.handle_topbar_action(TopBarUiAction::ScrollToBottom, cx);
                }))
            })
            .when_some(drag_overlay, |el, overlay| el.child(overlay));

        root = root.child(transcript);

        if is_apple {
            if !top_chrome_children.is_empty() {
                let top_feather = linear_gradient(
                    180.0,
                    linear_color_stop(TdesktopThemePalette::rgba_with_alpha(self.palette.stage_bg, 0.94), 0.0),
                    linear_color_stop(TdesktopThemePalette::rgba_with_alpha(self.palette.stage_bg, 0.0), 1.0),
                );
                root = root.child(
                    div()
                        .id("stage-floating-top-chrome")
                        .absolute()
                        .top_0()
                        .left_0()
                        .right_0()
                        .flex()
                        .flex_col()
                        .children(top_chrome_children)
                        .child(div().w_full().h(px(18.0)).bg(top_feather)),
                );
            } else {
                // Allow dragging the macOS window from the empty right-pane titlebar zone
                root = root.child(
                    div()
                        .absolute()
                        .top_0()
                        .left_0()
                        .right_0()
                        .h(px(52.0))
                        .window_control_area(WindowControlArea::Drag),
                );
            }
        }

        if self.show_bot_start_bar {
            let pal = self.palette;
            let lang = self.lang_id.as_str();
            root = root.child(
                div()
                    .id("stage-bot-start-bar")
                    .w_full()
                    .h(px(52.0))
                    .flex_shrink_0()
                    .px(px(16.0))
                    .bg(rgb(pal.composer_bg))
                    .when(!is_apple, |el| el.border_t_1().border_color(rgb(pal.border_fg)))
                    .flex()
                    .items_center()
                    .justify_center()
                    .child(
                        div()
                            .id("stage-bot-start-btn")
                            .cursor_pointer()
                            .w_full()
                            .h(px(36.0))
                            .rounded(px(if is_apple { 18.0 } else { 8.0 }))
                            .bg(rgb(pal.accent))
                            .hover(move |s| s.bg(rgb(pal.accent_hover)))
                            .flex()
                            .items_center()
                            .justify_center()
                            .text_size(px(13.5))
                            .font_weight(FontWeight::BOLD)
                            .text_color(rgb(0xFFFFFF))
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(|this, _, _, cx| {
                                    this.pending_bot_start = true;
                                    cx.notify();
                                }),
                            )
                            .child(crate::i18n::tr_str(lang, "开始", "START")),
                    ),
            );
        }

        if let Some(menu) = self.active_context_menu.clone() {
            root = root.child(self.render_active_context_menu_overlay(menu, cx));
        }

        if self.more_menu_open && has_active_chat {
            root = root.child(render_stage_more_menu_overlay(
                is_group_chat,
                can_block_peer,
                self.is_muted_chat,
                self.is_pinned_chat,
                self.is_peer_blocked,
                self.message_ttl_seconds,
                self.ttl_submenu_open,
                &self.palette,
                &self.lang_id,
                cx,
                |this, action, cx| this.handle_topbar_action(action, cx),
            ));
        }

        root
    }
}

pub(crate) fn stage_now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[std::prelude::v1::test]
    fn test_add_message_deduplicates_by_id_and_preserves_highest_delivery_status() {
        let mut stage = ConversationStage::new();
        stage.set_chat(7, "Alice", "online", "AL", 0x2AABEE);

        stage.add_message(MessageItemModel {
            id: 42,
            sender_id: 1,
            sender_name: "Me".to_string(),
            avatar_initials: "ME".to_string(),
            avatar_color: 0x2AABEE,
            direction: MessageDirection::Outgoing,
            timestamp: "12:00".to_string(),
            delivery_status: DeliveryStatus::Sent,
            reply_to: None,
            reactions: vec![],
            content: MessageContent::Text {
                text: "Initial".to_string(),
                is_edited: false,
            },
            bot_keyboard: None,
            is_pinned: false,
        });
        assert_eq!(stage.messages.len(), 1);

        // Stale Sending re-delivery refreshes content/reply/reactions without downgrading Sent or duplicating
        stage.add_message(MessageItemModel {
            id: 42,
            sender_id: 1,
            sender_name: "Me".to_string(),
            avatar_initials: "ME".to_string(),
            avatar_color: 0x2AABEE,
            direction: MessageDirection::Outgoing,
            timestamp: "12:00".to_string(),
            delivery_status: DeliveryStatus::Sending,
            reply_to: Some(ReplyReference {
                message_id: 41,
                sender_name: "Alice".to_string(),
                snippet: "Hi".to_string(),
            }),
            reactions: vec![ReactionItem {
                emoji: "👍".to_string(),
                count: 1,
                user_reacted: true,
            }],
            content: MessageContent::Text {
                text: "Updated".to_string(),
                is_edited: true,
            },
            bot_keyboard: None,
            is_pinned: false,
        });

        assert_eq!(stage.messages.len(), 1);
        assert_eq!(stage.chat_histories.get(&7).map(|h| h.len()), Some(1));
        assert_eq!(stage.messages[0].delivery_status, DeliveryStatus::Sent);
        assert_eq!(stage.messages[0].reactions.len(), 1);
        assert_eq!(stage.messages[0].reply_to.as_ref().map(|r| r.message_id), Some(41));
        assert!(matches!(
            stage.messages[0].content,
            MessageContent::Text { ref text, is_edited: true } if text == "Updated"
        ));

        // Higher Read status upgrades existing bubble cleanly
        stage.add_message(MessageItemModel {
            id: 42,
            sender_id: 1,
            sender_name: "Me".to_string(),
            avatar_initials: "ME".to_string(),
            avatar_color: 0x2AABEE,
            direction: MessageDirection::Outgoing,
            timestamp: "12:00".to_string(),
            delivery_status: DeliveryStatus::Read,
            reply_to: None,
            reactions: vec![],
            content: MessageContent::Text {
                text: "Updated".to_string(),
                is_edited: true,
            },
            bot_keyboard: None,
            is_pinned: false,
        });

        assert_eq!(stage.messages.len(), 1);
        assert_eq!(stage.messages[0].delivery_status, DeliveryStatus::Read);
    }
}
