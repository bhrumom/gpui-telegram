//! Message CRUD, history merging, reaction, poll, scroll, and row-building operations for `ConversationStage`.

use std::collections::HashSet;

use gpui::*;

use crate::views::conversation_stage::{compute_attachment, ConversationStage, MSG_MAX_WIDTH, MSG_MIN_WIDTH};
use crate::views::stage::cards::*;
use crate::views::stage::history_view_highlight_manager::ChatPaintHighlight;
use crate::views::stage::rich_meta::MessageRichMeta;
use crate::views::stage::stage_bubble::{
    render_message_row_1to1, BotAssistRenderContext, BubbleGroupLayout,
};
use crate::views::stage::stage_context_menu::{
    should_open_selector_below, ReactionOverlayLayout, ReactionOverlayMode,
};

fn delivery_status_rank(status: DeliveryStatus) -> u8 {
    match status {
        DeliveryStatus::Failed => 0,
        DeliveryStatus::Sending => 0,
        DeliveryStatus::Sent => 1,
        DeliveryStatus::Read => 2,
    }
}

fn max_delivery_status(a: DeliveryStatus, b: DeliveryStatus) -> DeliveryStatus {
    if delivery_status_rank(a) >= delivery_status_rank(b) {
        a
    } else {
        b
    }
}

fn merge_message_in_place(existing: &mut MessageItemModel, incoming: &MessageItemModel) {
    existing.delivery_status = max_delivery_status(existing.delivery_status, incoming.delivery_status);
    if !incoming.sender_name.is_empty() {
        existing.sender_name = incoming.sender_name.clone();
    }
    if !incoming.avatar_initials.is_empty() {
        existing.avatar_initials = incoming.avatar_initials.clone();
    }
    if !incoming.timestamp.is_empty() {
        existing.timestamp = incoming.timestamp.clone();
    }
    if incoming.reply_to.is_some() {
        existing.reply_to = incoming.reply_to.clone();
    }
    if !incoming.reactions.is_empty() {
        existing.reactions = incoming.reactions.clone();
    }
    existing.content = incoming.content.clone();
    if incoming.bot_keyboard.is_some() {
        existing.bot_keyboard = incoming.bot_keyboard.clone();
    }
    existing.is_pinned = existing.is_pinned || incoming.is_pinned;
}

impl ConversationStage {
    pub fn clear_selection(&mut self) {
        self.active_chat_id = None;
        self.chat_title = "Select a chat".to_string();
        self.chat_subtitle.clear();
        self.chat_avatar_initials.clear();
        self.chat_avatar_color = self.palette.input_bg;
        self.chat_avatar_url = None;
        self.pinned_message = None;
        self.messages.clear();
        self.is_inspector_open = false;
        self.pending_call_request = false;
        self.pending_video_call = false;
        self.pending_media_url = None;
        self.pending_open_url = None;
        self.pending_media_viewer = None;
        self.pending_iv_url = None;
        self.pending_bot_action = None;
        self.pending_bot_button_action = None;
        self.pending_edit_message = None;
        self.pending_forward_message_ids = None;
        self.pending_delete_message_ids = None;
        self.pending_pin_toggle = None;
        self.pending_translate_message_id = None;
        self.pending_poll_vote = None;
        self.pending_reaction_toggle = None;
        self.pending_reaction_actions.clear();
        self.pending_voice_playback = None;
        self.pending_download_url = None;
        self.pending_dropped_paths = None;
        self.pending_open_profile_peer_id = None;
        self.pending_clear_history = false;
        self.pending_export_chat = false;
        self.pending_toggle_mute = false;
        self.pending_change_wallpaper = false;
        self.pending_toggle_pin_chat = false;
        self.pending_set_ttl_seconds = None;
        self.pending_toggle_block_peer = false;
        self.pending_delete_or_leave_chat = false;
        self.is_muted_chat = false;
        self.is_pinned_chat = false;
        self.is_peer_blocked = false;
        self.is_group_chat = false;
        self.is_saved_messages = false;
        self.message_ttl_seconds = 0;
        self.ttl_submenu_open = false;
        self.unread_incoming_count = 0;
        self.unread_anchor_msg_id = None;
        self.active_context_menu = None;
        self.context_menu_reactions_expanded = false;
        self.pending_reply = None;
        self.simulated_max_offset = None;
        self.simulated_viewport_height = None;
        self.selected_message_ids.clear();
        self.select_mode_active = false;
        self.pinned_tracker.set_pinned_ids(Vec::new());
        self.search_open = false;
        self.search_query.clear();
        self.search_match_ids.clear();
        self.search_match_cursor = 0;
        self.hovered_message_id = None;
        self.hovered_gutter_id = None;
        self.hovered_reaction_bar_id = None;
        self.hovered_reaction_bridge_id = None;
        self.quick_strip_msg_id = None;
        self.hover_grace = None;
        self.hover_grace_generation = self.hover_grace_generation.wrapping_add(1);
        self.reaction_picker_msg_id = None;
        self.transcript_bounds_probe.set(None);
        self.bubble_widths_probe.borrow_mut().clear();
        self.bubble_bounds_probe.borrow_mut().clear();
        self.row_heights_probe.borrow_mut().clear();
        self.more_menu_open = false;
        self.dismiss_toast_banner();
        self.copied_code_msg = None;
        self.active_fabushi_bot = None;
        self.bot_description_card = None;
        self.show_bot_start_bar = false;
        self.is_bot_chat = false;
        self.bot_activity_state = "idle".to_string();
        self.bot_activity_detail = None;
        self.is_viewer_owner_fabushi_bot = false;
        self.pending_bot_start = false;
        self.room_agent_states.clear();
        self.pending_stage_action = None;
        self.group_member_tiles.clear();
        self.permission_dock.sync_requests(Vec::new());
        self.active_bot_card_field = None;
        self.bot_assist_ime_preedit = None;
        self.pending_bot_assist_action = None;
        self.scroll_handle.set_offset(Point::default());
        self.is_anchored_to_bottom = true;
        self.is_loading_history = false;
        self.row_height_cache.measured_heights.clear();
        self.row_height_cache.rebuild_prefix_sums(&self.messages);
    }

    pub fn toggle_inspector(&mut self) {
        self.is_inspector_open = !self.is_inspector_open;
    }

    pub fn set_chat_avatar_url(&mut self, url: Option<String>) {
        self.chat_avatar_url = url;
    }

    pub fn set_message_meta(&mut self, msg_id: u64, meta: MessageRichMeta) {
        self.message_meta.insert(msg_id, meta);
        self.row_height_cache.mark_dirty();
    }

    pub fn sync_pinned_tracker(&mut self) {
        let mut pinned_ids: Vec<i64> = self.messages.iter().filter(|m| m.is_pinned).map(|m| m.id as i64).collect();
        if let Some(cid) = self.active_chat_id {
            if let Some(entries) = self.chat_pinned_entries.get(&cid) {
                for entry in entries {
                    let id = entry.msg_id as i64;
                    if !pinned_ids.contains(&id) {
                        pinned_ids.push(id);
                    }
                }
            }
        }
        self.pinned_tracker.set_pinned_ids(pinned_ids);
    }

    pub fn set_pinned_entries(&mut self, chat_id: u64, entries: Vec<super::super::conversation_stage::PinnedBannerEntry>) {
        let latest_preview = entries.last().map(|e| e.preview_text.clone());
        let entry_ids: Vec<i64> = entries.iter().map(|e| e.msg_id as i64).collect();
        if entries.is_empty() {
            self.chat_pinned_entries.remove(&chat_id);
            self.chat_pinned_messages.remove(&chat_id);
        } else {
            self.chat_pinned_entries.insert(chat_id, entries);
            self.chat_pinned_messages.insert(chat_id, latest_preview.clone());
        }
        if self.active_chat_id == Some(chat_id) {
            self.pinned_message = latest_preview;
            self.sync_pinned_tracker();
            if !entry_ids.is_empty() {
                self.pinned_tracker.reset_to_latest();
            }
        }
    }

    pub fn purge_expired_messages_before(&mut self, now_ms: i64) -> usize {
        let expired_ids: std::collections::HashSet<u64> = self
            .message_expirations
            .iter()
            .filter_map(|(&id, &exp)| (exp <= now_ms).then_some(id))
            .collect();
        if expired_ids.is_empty() {
            return 0;
        }
        for id in &expired_ids {
            self.message_expirations.remove(id);
        }
        let before_active = self.messages.len();
        self.messages.retain(|m| !expired_ids.contains(&m.id));
        let mut removed = before_active.saturating_sub(self.messages.len());
        for hist in self.chat_histories.values_mut() {
            let b = hist.len();
            hist.retain(|m| !expired_ids.contains(&m.id));
            removed = removed.max(b.saturating_sub(hist.len()));
        }
        for entries in self.chat_pinned_entries.values_mut() {
            entries.retain(|e| !expired_ids.contains(&e.msg_id));
        }
        if !expired_ids.is_empty() {
            self.sync_pinned_tracker();
            self.row_height_cache.mark_dirty();
        }
        removed.max(expired_ids.len())
    }

    pub fn add_message(&mut self, message: MessageItemModel) {
        if message.is_pinned {
            let mut ids: Vec<i64> = self
                .messages
                .iter()
                .filter(|m| m.is_pinned)
                .map(|m| m.id as i64)
                .collect();
            if !ids.contains(&(message.id as i64)) {
                ids.push(message.id as i64);
            }
            self.pinned_tracker.set_pinned_ids(ids);
            if let MessageContent::Text { ref text, .. } = message.content {
                let snippet = text.lines().next().unwrap_or(text).to_string();
                self.pinned_message = Some(snippet);
                if let Some(cid) = self.active_chat_id {
                    self.chat_pinned_messages.insert(cid, self.pinned_message.clone());
                }
            }
        }
        if let Some(pos) = self.messages.iter().position(|m| m.id == message.id) {
            merge_message_in_place(&mut self.messages[pos], &message);
            if let Some(cid) = self.active_chat_id {
                let hist = self.chat_histories.entry(cid).or_default();
                if let Some(hpos) = hist.iter().position(|m| m.id == message.id) {
                    merge_message_in_place(&mut hist[hpos], &message);
                } else {
                    hist.push(self.messages[pos].clone());
                }
            }
            self.row_height_cache.mark_dirty();
            return;
        }

        let is_outgoing = message.direction == MessageDirection::Outgoing;
        let was_at_bottom = self.is_at_bottom();
        if !is_outgoing && !was_at_bottom && self.unread_anchor_msg_id.is_none() {
            self.unread_anchor_msg_id = Some(message.id);
        }

        self.is_loading_history = false;
        self.messages.push(message.clone());
        if let Some(cid) = self.active_chat_id {
            self.chat_histories.entry(cid).or_default().push(message);
        }
        self.row_height_cache.mark_dirty();

        if is_outgoing || was_at_bottom {
            self.unread_anchor_msg_id = None;
            self.scroll_to_bottom();
        } else {
            self.unread_incoming_count += 1;
        }
    }

    pub fn add_outgoing_text_message(&mut self, text: &str) {
        let next_id = self.messages.iter().map(|m| m.id).max().unwrap_or(0) + 1;
        self.add_message(MessageItemModel {
            id: next_id,
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
                text: text.to_string(),
                is_edited: false,
            },
            bot_keyboard: None,
            is_pinned: false,
        });
    }

    pub fn add_incoming_message(&mut self, sender_id: u64, text: &str) {
        let next_id = self.messages.iter().map(|m| m.id).max().unwrap_or(0) + 1;
        let sender_name = if self.chat_title.is_empty() || self.chat_title == "Select a chat" {
            "Peer".to_string()
        } else {
            self.chat_title.clone()
        };
        let avatar_initials = if !self.chat_avatar_initials.is_empty() {
            self.chat_avatar_initials.clone()
        } else {
            sender_name
                .chars()
                .next()
                .map(|c| c.to_uppercase().to_string())
                .unwrap_or_else(|| "?".to_string())
        };
        self.add_message(MessageItemModel {
            id: next_id,
            sender_id,
            sender_name,
            avatar_initials,
            avatar_color: 0x9B59B6,
            direction: MessageDirection::Incoming,
            timestamp: "12:01".to_string(),
            delivery_status: DeliveryStatus::Read,
            reply_to: None,
            reactions: vec![],
            content: MessageContent::Text {
                text: text.to_string(),
                is_edited: false,
            },
            bot_keyboard: None,
            is_pinned: false,
        });
    }

    /// Merges server-fetched historical messages with existing local messages,
    /// deduplicating by message ID while preserving local pending/sending messages
    /// and reconciling server-side reaction/delivery updates on existing messages.
    pub fn merge_historical_messages(&mut self, server_messages: Vec<MessageItemModel>) {
        self.merge_historical_messages_preserving(server_messages, &HashSet::new());
    }

    /// Merges server-fetched historical messages while preserving optimistic reactions on
    /// messages whose IDs are in `protected_reaction_msg_ids` (e.g. in-flight reaction requests).
    pub fn merge_historical_messages_preserving(
        &mut self,
        server_messages: Vec<MessageItemModel>,
        protected_reaction_msg_ids: &HashSet<u64>,
    ) {
        if self.messages.is_empty() {
            self.messages = server_messages;
            self.row_height_cache.mark_dirty();
            return;
        }

        // Keep local sending messages (id not yet confirmed by server) and strictly local bot cards
        let sending_messages: Vec<MessageItemModel> = self
            .messages
            .iter()
            .filter(|m| !m.is_local_only() && m.delivery_status == DeliveryStatus::Sending)
            .cloned()
            .collect();
        let local_only_messages: Vec<MessageItemModel> = self
            .messages
            .iter()
            .filter(|m| m.is_local_only())
            .cloned()
            .collect();

        let existing_remote: Vec<MessageItemModel> = self
            .messages
            .iter()
            .filter(|m| !m.is_local_only())
            .cloned()
            .collect();
        let mut existing_ids: std::collections::HashSet<u64> =
            existing_remote.iter().map(|m| m.id).collect();
        let mut merged = existing_remote;

        for s_msg in server_messages {
            if !existing_ids.contains(&s_msg.id) {
                existing_ids.insert(s_msg.id);
                merged.push(s_msg);
            } else if let Some(existing) = merged.iter_mut().find(|m| m.id == s_msg.id) {
                if existing.delivery_status != DeliveryStatus::Sending {
                    if !protected_reaction_msg_ids.contains(&s_msg.id) {
                        existing.reactions = s_msg.reactions;
                    }
                    existing.delivery_status =
                        max_delivery_status(existing.delivery_status, s_msg.delivery_status);
                    existing.is_pinned = existing.is_pinned || s_msg.is_pinned;
                }
            }
        }

        // Re-append local sending items at the bottom
        for sending in sending_messages {
            if !existing_ids.contains(&sending.id) {
                merged.push(sending);
            }
        }
        // Re-append strictly local bot-assist cards in chronological order after remote messages
        for local_msg in local_only_messages {
            if !merged.iter().any(|m| m.id == local_msg.id) {
                merged.push(local_msg);
            }
        }

        self.messages = merged;
        self.row_height_cache.mark_dirty();
    }

    /// Remaps active hover, picker, context-menu, and selection IDs when an optimistic
    /// `local_id` (`client_nonce`) is promoted to an authoritative `server_id`.
    pub fn remap_message_id(&mut self, local_id: u64, server_id: u64) {
        if server_id == 0 || local_id == server_id {
            return;
        }
        if self.hovered_message_id == Some(local_id) {
            self.hovered_message_id = Some(server_id);
        }
        if self.hovered_gutter_id == Some(local_id) {
            self.hovered_gutter_id = Some(server_id);
        }
        if self.hovered_reaction_bar_id == Some(local_id) {
            self.hovered_reaction_bar_id = Some(server_id);
        }
        if self.hovered_reaction_bridge_id == Some(local_id) {
            self.hovered_reaction_bridge_id = Some(server_id);
        }
        if self.quick_strip_msg_id == Some(local_id) {
            self.quick_strip_msg_id = Some(server_id);
        }
        if let Some((ref mut gid, _)) = self.hover_grace {
            if *gid == local_id {
                *gid = server_id;
            }
        }
        if self.reaction_picker_msg_id == Some(local_id) {
            self.reaction_picker_msg_id = Some(server_id);
        }
        {
            let mut widths = self.bubble_widths_probe.borrow_mut();
            if let Some(w) = widths.remove(&local_id) {
                widths.insert(server_id, w);
            }
        }
        {
            let mut bounds = self.bubble_bounds_probe.borrow_mut();
            if let Some(b) = bounds.remove(&local_id) {
                bounds.insert(server_id, b);
            }
        }
        {
            let mut row_heights = self.row_heights_probe.borrow_mut();
            if let Some(h) = row_heights.remove(&local_id) {
                row_heights.insert(server_id, h);
            }
        }
        if let Some(h) = self.row_height_cache.measured_heights.remove(&local_id) {
            self.row_height_cache.measured_heights.insert(server_id, h);
        }
        if let Some(ref mut menu) = self.active_context_menu {
            if menu.message_id == local_id {
                menu.message_id = server_id;
            }
        }
        for sel in &mut self.selected_message_ids {
            if *sel == local_id {
                *sel = server_id;
            }
        }
    }

    pub fn enqueue_reaction_toggle(&mut self, msg_id: u64, emoji: String) {
        let is_added = self
            .messages
            .iter()
            .find(|m| m.id == msg_id)
            .map(|m| m.reactions.iter().any(|r| r.emoji == emoji && r.user_reacted))
            .unwrap_or(true);
        self.pending_reaction_toggle = Some((msg_id, emoji.clone()));
        self.pending_reaction_actions
            .push_back((msg_id, emoji, is_added));
    }

    pub fn drain_pending_reaction_actions(&mut self) -> Vec<(u64, String, bool)> {
        if !self.pending_reaction_actions.is_empty() {
            self.pending_reaction_toggle = None;
            return self.pending_reaction_actions.drain(..).collect();
        }
        if let Some((mid, emoji)) = self.pending_reaction_toggle.take() {
            let is_added = self
                .messages
                .iter()
                .find(|m| m.id == mid)
                .map(|m| m.reactions.iter().any(|r| r.emoji == emoji && r.user_reacted))
                .unwrap_or(true);
            return vec![(mid, emoji, is_added)];
        }
        Vec::new()
    }

    pub fn resolved_stage_bounds(&self) -> Option<Bounds<Pixels>> {
        self.stage_bounds_probe
            .get()
            .filter(|b| b.size.width > px(1.0) && b.size.height > px(1.0))
    }

    pub fn set_simulated_stage_bounds(&mut self, bounds: Option<Bounds<Pixels>>) {
        self.stage_bounds_probe.set(bounds);
    }

    pub fn effective_viewport_height(&self) -> f32 {
        self.simulated_viewport_height
            .filter(|&h| h > 10.0)
            .or_else(|| {
                self.transcript_bounds_probe
                    .get()
                    .map(|b| f32::from(b.size.height))
                    .filter(|&h| h > 10.0)
            })
            .or_else(|| {
                let h = f32::from(self.scroll_handle.bounds().size.height);
                (h > 10.0).then_some(h)
            })
            .unwrap_or(600.0)
    }

    pub fn effective_stage_width(&self) -> f32 {
        self.resolved_stage_bounds()
            .map(|b| f32::from(b.size.width))
            .filter(|&w| w > 10.0)
            .or_else(|| {
                self.transcript_bounds_probe
                    .get()
                    .map(|b| f32::from(b.size.width))
                    .filter(|&w| w > 10.0)
            })
            .or_else(|| {
                let w = f32::from(self.scroll_handle.bounds().size.width);
                (w > 10.0).then_some(w)
            })
            .unwrap_or(760.0)
    }

    pub fn resolved_or_estimated_bubble_width(
        &self,
        msg: &MessageItemModel,
        stage_w: f32,
        reserve_avatar_column: bool,
    ) -> f32 {
        let avatar_skip = if reserve_avatar_column { 40.0 } else { 0.0 };
        let max_avail = (stage_w - 56.0 - 56.0 - avatar_skip).clamp(MSG_MIN_WIDTH, MSG_MAX_WIDTH);
        if let Some(&measured) = self.bubble_widths_probe.borrow().get(&msg.id) {
            return measured.clamp(MSG_MIN_WIDTH, max_avail);
        }
        let estimated = match &msg.content {
            MessageContent::Text { text, .. } => {
                let max_line_px = text
                    .lines()
                    .map(|line| {
                        line.chars()
                            .map(|ch| if ch.is_ascii() { 7.6_f32 } else { 14.5_f32 })
                            .sum::<f32>()
                    })
                    .fold(0.0_f32, f32::max);
                max_line_px + 72.0
            }
            MessageContent::Voice { .. } => 260.0,
            MessageContent::Service { .. } => MSG_MIN_WIDTH,
            _ => 320.0,
        };
        estimated.clamp(MSG_MIN_WIDTH, max_avail)
    }

    fn row_viewport_vertical_span(&self, idx: usize) -> (f32, f32, f32) {
        let viewport_h = self.effective_viewport_height();
        if self.simulated_viewport_height.is_none() {
            if let (Some(vp), Some(msg)) = (self.transcript_bounds_probe.get(), self.messages.get(idx))
            {
                let vp_h = f32::from(vp.size.height);
                if vp_h > 10.0 {
                    if let Some(&b_bounds) = self.bubble_bounds_probe.borrow().get(&msg.id) {
                        let b_h = f32::from(b_bounds.size.height);
                        if b_h > 1.0 {
                            let rel_top = f32::from(b_bounds.origin.y - vp.origin.y);
                            return (rel_top, rel_top + b_h, vp_h);
                        }
                    }
                }
            }
        }
        let scroll_top = f32::from(-self.scroll_handle.offset().y).max(0.0);
        let pad_top = 14.0_f32;
        let row_top = if idx == 0 {
            0.0
        } else {
            self.row_height_cache
                .prefix_sums
                .get(idx.saturating_sub(1))
                .copied()
                .unwrap_or(0.0)
        } + pad_top;
        let row_bottom = self
            .row_height_cache
            .prefix_sums
            .get(idx)
            .copied()
            .map(|v| v + pad_top)
            .unwrap_or(row_top + 42.0);
        (row_top - scroll_top, row_bottom - scroll_top, viewport_h)
    }

    /// Computes `(is_within_viewport, anchor_below)` in scroll-relative pixel coordinates
    /// so scrolled chats never clip `ExpandedSelector` behind the top header or paint
    /// `deferred` overlays from overscanned rows over stage chrome.
    pub fn compute_reaction_overlay_placement(&self, idx: usize) -> (bool, bool) {
        let (rel_top, rel_bottom, viewport_h) = self.row_viewport_vertical_span(idx);
        let is_within_viewport = rel_bottom > 12.0 && rel_top < viewport_h - 12.0;
        let anchor_below = should_open_selector_below(rel_top, rel_bottom, viewport_h);
        (is_within_viewport, anchor_below)
    }

    pub fn compute_reaction_overlay_layout(
        &self,
        idx: usize,
        msg: &MessageItemModel,
        reserve_avatar_column: bool,
    ) -> (bool, ReactionOverlayLayout) {
        let (rel_top, rel_bottom, viewport_h) = self.row_viewport_vertical_span(idx);
        let is_within_viewport = rel_bottom > 12.0 && rel_top < viewport_h - 12.0;
        let selector_below = should_open_selector_below(rel_top, rel_bottom, viewport_h);
        let stage_w = self.effective_stage_width();
        let bubble_w = self.resolved_or_estimated_bubble_width(msg, stage_w, reserve_avatar_column);
        let is_outgoing = msg.direction == MessageDirection::Outgoing;
        let layout = ReactionOverlayLayout::compute(
            selector_below,
            rel_top,
            rel_bottom,
            viewport_h,
            bubble_w,
            stage_w,
            is_outgoing,
            reserve_avatar_column,
        );
        (is_within_viewport, layout)
    }

    pub fn reaction_overlay_mode(&self, msg_id: u64) -> Option<ReactionOverlayMode> {
        if !self.selected_message_ids.is_empty() || self.active_context_menu.is_some() {
            return None;
        }
        let directly_hovering = self.hovered_message_id == Some(msg_id)
            || self.hovered_gutter_id == Some(msg_id)
            || self.hovered_reaction_bar_id == Some(msg_id)
            || self.hovered_reaction_bridge_id == Some(msg_id);
        if self.reaction_picker_msg_id == Some(msg_id) {
            Some(ReactionOverlayMode::ExpandedSelector)
        } else if self.hovered_reaction_bar_id == Some(msg_id)
            || self.hovered_reaction_bridge_id == Some(msg_id)
            || (directly_hovering && self.quick_strip_msg_id == Some(msg_id))
        {
            Some(ReactionOverlayMode::QuickStrip)
        } else if self.hovered_message_id == Some(msg_id)
            || self.hovered_gutter_id == Some(msg_id)
        {
            Some(ReactionOverlayMode::CornerButton)
        } else if let Some((grace_id, grace_mode)) = self.hover_grace {
            (grace_id == msg_id).then_some(grace_mode)
        } else {
            None
        }
    }

    pub fn distance_from_bottom(&self) -> Pixels {
        let max_y = self.max_scroll_offset().height;
        let cur_y = -self.scroll_handle.offset().y;
        (max_y - cur_y).max(px(0.0))
    }

    pub fn is_at_bottom(&self) -> bool {
        let max_y = self.max_scroll_offset().height;
        if max_y <= px(0.0) {
            return true;
        }
        self.distance_from_bottom() <= px(48.0)
    }

    pub fn scroll_to_bottom(&mut self) {
        self.scroll_handle.scroll_to_bottom();
        if let Some(sim) = self.simulated_max_offset {
            self.scroll_handle.set_offset(Point::new(px(0.0), -sim.height));
        }
        self.is_anchored_to_bottom = true;
        self.unread_incoming_count = 0;
    }

    pub fn set_simulated_scroll_offset(&mut self, offset: Point<Pixels>, max_offset: Size<Pixels>) {
        self.scroll_handle.set_offset(offset);
        self.simulated_max_offset = Some(max_offset);
    }

    pub fn max_scroll_offset(&self) -> Size<Pixels> {
        if let Some(sim) = self.simulated_max_offset {
            sim
        } else {
            self.scroll_handle.max_offset()
        }
    }

    pub fn copy_message_text(&self, msg_id: u64) -> Option<String> {
        self.messages.iter().find(|m| m.id == msg_id).and_then(|m| match &m.content {
            MessageContent::Text { text, .. } => Some(text.clone()),
            MessageContent::Media { caption, .. } => caption.clone(),
            _ => None,
        })
    }

    pub fn toggle_pin_message(&mut self, msg_id: u64) {
        let mut new_pinned_state = false;
        let mut snippet_opt = None;
        if let Some(m) = self.messages.iter_mut().find(|m| m.id == msg_id) {
            m.is_pinned = !m.is_pinned;
            new_pinned_state = m.is_pinned;
            if new_pinned_state {
                snippet_opt = match &m.content {
                    MessageContent::Text { text, .. } => Some(text.lines().next().unwrap_or(text).to_string()),
                    MessageContent::LocalBotAssistCard(_) => Some("[安全存储卡片]".to_string()),
                    _ => Some("Pinned message".to_string()),
                };
            }
        }
        if new_pinned_state {
            self.pinned_message = snippet_opt;
        } else {
            self.pinned_message = self.messages.iter().find(|m| m.is_pinned).and_then(|m| match &m.content {
                MessageContent::Text { text, .. } => Some(text.lines().next().unwrap_or(text).to_string()),
                MessageContent::LocalBotAssistCard(_) => Some("[安全存储卡片]".to_string()),
                _ => Some("Pinned message".to_string()),
            });
        }
        if let Some(cid) = self.active_chat_id {
            self.chat_pinned_messages.insert(cid, self.pinned_message.clone());
            if let Some(hist) = self.chat_histories.get_mut(&cid) {
                if let Some(m) = hist.iter_mut().find(|m| m.id == msg_id) {
                    m.is_pinned = new_pinned_state;
                }
            }
        }
    }

    pub fn delete_message(&mut self, msg_id: u64) {
        self.messages.retain(|m| m.id != msg_id);
        if let Some(cid) = self.active_chat_id {
            if let Some(hist) = self.chat_histories.get_mut(&cid) {
                hist.retain(|m| m.id != msg_id);
            }
        }
        if self.hovered_message_id == Some(msg_id) {
            self.hovered_message_id = None;
        }
        if self.hovered_gutter_id == Some(msg_id) {
            self.hovered_gutter_id = None;
        }
        if self.hovered_reaction_bar_id == Some(msg_id) {
            self.hovered_reaction_bar_id = None;
        }
        if self.hovered_reaction_bridge_id == Some(msg_id) {
            self.hovered_reaction_bridge_id = None;
        }
        if self.quick_strip_msg_id == Some(msg_id) {
            self.quick_strip_msg_id = None;
        }
        if self.reaction_picker_msg_id == Some(msg_id) {
            self.reaction_picker_msg_id = None;
        }
        if self.hover_grace.is_some_and(|(gid, _)| gid == msg_id) {
            self.hover_grace = None;
            self.hover_grace_generation = self.hover_grace_generation.wrapping_add(1);
        }
        self.bubble_widths_probe.borrow_mut().remove(&msg_id);
        self.bubble_bounds_probe.borrow_mut().remove(&msg_id);
        self.row_heights_probe.borrow_mut().remove(&msg_id);
        self.row_height_cache.measured_heights.remove(&msg_id);
        self.row_height_cache.mark_dirty();
    }

    pub fn compute_attachment(&self, idx: usize) -> (bool, bool) {
        compute_attachment(&self.messages, idx)
    }

    pub fn vote_poll(&mut self, message_id: u64, option_id: u32) {
        if let Some(msg) = self.messages.iter_mut().find(|m| m.id == message_id) {
            if let MessageContent::Poll {
                ref mut options,
                ref mut total_votes,
                ref mut selected_option_id,
                is_closed,
                ..
            } = msg.content
            {
                if is_closed {
                    return;
                }
                *selected_option_id = Some(option_id);
                *total_votes += 1;
                for opt in options.iter_mut() {
                    if opt.id == option_id {
                        opt.votes += 1;
                        opt.is_selected = true;
                    } else {
                        opt.is_selected = false;
                    }
                }
                let total = (*total_votes as f32).max(1.0);
                for opt in options.iter_mut() {
                    opt.percentage = (opt.votes as f32 / total) * 100.0;
                }
            }
        }
    }

    pub fn toggle_reaction(&mut self, message_id: u64, emoji: &str) {
        let mut updated = false;
        if let Some(msg) = self.messages.iter_mut().find(|m| m.id == message_id) {
            msg.toggle_reaction(emoji);
            updated = true;
        }
        if let Some(cid) = self.active_chat_id {
            if let Some(hist) = self.chat_histories.get_mut(&cid) {
                if let Some(hmsg) = hist.iter_mut().find(|m| m.id == message_id) {
                    hmsg.toggle_reaction(emoji);
                    updated = true;
                }
            }
        }
        if updated {
            self.row_height_cache.mark_dirty();
        }
    }

    pub fn set_message_reactions(
        &mut self,
        chat_id: u64,
        message_id: u64,
        reactions: Vec<ReactionItem>,
    ) {
        let mut updated = false;
        if self.active_chat_id == Some(chat_id) || self.active_chat_id.is_none() {
            if let Some(msg) = self.messages.iter_mut().find(|m| m.id == message_id) {
                msg.reactions = reactions.clone();
                updated = true;
            }
        }
        if let Some(hist) = self.chat_histories.get_mut(&chat_id) {
            if let Some(hmsg) = hist.iter_mut().find(|m| m.id == message_id) {
                hmsg.reactions = reactions;
                updated = true;
            }
        }
        if updated {
            self.row_height_cache.mark_dirty();
        }
    }

    pub fn message_count(&self) -> usize {
        self.messages.len()
    }

    pub fn edit_message_text(&mut self, message_id: u64, new_text: String) {
        if let Some(msg) = self.messages.iter_mut().find(|m| m.id == message_id) {
            msg.content = MessageContent::Text {
                text: new_text,
                is_edited: true,
            };
        }
        if let Some(chat_id) = self.active_chat_id {
            if let Some(hist) = self.chat_histories.get_mut(&chat_id) {
                if let Some(msg) = hist.iter_mut().find(|m| m.id == message_id) {
                    msg.content = MessageContent::Text {
                        text: match &self.messages.iter().find(|m| m.id == message_id).map(|m| &m.content) {
                            Some(MessageContent::Text { text, .. }) => text.clone(),
                            _ => String::new(),
                        },
                        is_edited: true,
                    };
                }
            }
        }
        self.row_height_cache.dirty = true;
    }

    pub fn is_selecting_messages(&self) -> bool {
        self.select_mode_active || !self.selected_message_ids.is_empty()
    }

    pub fn clear_selected_messages(&mut self) {
        self.select_mode_active = false;
        self.selected_message_ids.clear();
    }

    pub fn toggle_select_message(&mut self, msg_id: u64) {
        if let Some(pos) = self.selected_message_ids.iter().position(|&x| x == msg_id) {
            self.selected_message_ids.remove(pos);
        } else {
            self.selected_message_ids.push(msg_id);
        }
    }

    pub fn delete_messages_by_ids(&mut self, ids: &[u64]) {
        self.messages.retain(|m| !ids.contains(&m.id));
        self.selected_message_ids.retain(|id| !ids.contains(id));
        if let Some(chat_id) = self.active_chat_id {
            if let Some(hist) = self.chat_histories.get_mut(&chat_id) {
                hist.retain(|m| !ids.contains(&m.id));
            }
        }
        {
            let mut row_heights = self.row_heights_probe.borrow_mut();
            for id in ids {
                row_heights.remove(id);
                self.row_height_cache.measured_heights.remove(id);
            }
        }
        self.row_height_cache.mark_dirty();
        self.row_height_cache.rebuild_prefix_sums(&self.messages);
    }

    pub(crate) fn build_visible_message_rows(
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

            message_rows.push(render_message_row_1to1(
                msg,
                meta,
                self.active_fabushi_bot.as_ref(),
                group,
                stage_w,
                selected_set.contains(&msg.id),
                has_any_selection,
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

    pub(crate) fn sticky_date_label(&self, start_idx: usize) -> Option<String> {
        if self.messages.is_empty() || self.active_chat_id.is_none() {
            return None;
        }
        self.messages.get(start_idx).and_then(|m| {
            self.message_meta
                .get(&m.id)
                .and_then(|rm| rm.date_group_label.clone())
                .or_else(|| {
                    if m.timestamp.contains(' ') {
                        m.timestamp.split(' ').next().map(str::to_string)
                    } else {
                        None
                    }
                })
        })
    }

    pub fn pinned_banner_info(&self) -> Option<(String, String, Option<String>, usize, usize)> {
        let active_cid = self.active_chat_id?;
        let entries = self.chat_pinned_entries.get(&active_cid);
        let fallback_snippet = self.pinned_message.as_ref().or_else(|| {
            entries.and_then(|v| v.last()).map(|e| &e.preview_text)
        })?;
        let target_id = self.pinned_tracker.current_msg_id().map(|id| id as u64);
        let target_entry = target_id
            .and_then(|id| entries.and_then(|v| v.iter().find(|e| e.msg_id == id)))
            .or_else(|| entries.and_then(|v| v.last()));
        let target_msg = target_id
            .and_then(|id| self.messages.iter().find(|m| m.id == id))
            .or_else(|| self.messages.iter().rev().find(|m| m.is_pinned));
        let snippet = target_entry
            .map(|e| e.preview_text.clone())
            .or_else(|| {
                target_msg.and_then(|m| match &m.content {
                    MessageContent::Text { text, .. } => Some(text.lines().next().unwrap_or(text).to_string()),
                    MessageContent::Media { caption, .. } | MessageContent::Album { caption, .. } => caption.clone(),
                    _ => None,
                })
            })
            .unwrap_or_else(|| fallback_snippet.clone());
        let sender = target_entry
            .map(|e| e.sender_name.clone())
            .filter(|s| !s.is_empty())
            .or_else(|| target_msg.map(|m| m.sender_name.clone()))
            .unwrap_or_default();
        let thumb = target_entry
            .and_then(|e| e.thumb_url.clone())
            .or_else(|| {
                target_msg.and_then(|m| match &m.content {
                    MessageContent::Media { url, .. } => Some(url.clone()),
                    MessageContent::Album { items, .. } => items.first().map(|i| i.url.clone()),
                    _ => None,
                })
            });
        let count = self.pinned_tracker.count().max(entries.map(|v| v.len()).unwrap_or(0)).max(1);
        let active_idx = self.pinned_tracker.current_index.min(count.saturating_sub(1));
        Some((snippet, sender, thumb, count, active_idx))
    }

    /// Finds a `BotAssistCardModel` by `card_id` in the active stage messages or `chat_histories`.
    pub fn find_bot_assist_card(&self, card_id: &str) -> Option<&BotAssistCardModel> {
        self.messages
            .iter()
            .chain(self.chat_histories.values().flat_map(|v| v.iter()))
            .find_map(|m| match &m.content {
                MessageContent::LocalBotAssistCard(card) if card.card_id() == card_id => {
                    Some(card)
                }
                _ => None,
            })
    }

    fn find_bot_assist_payload(&self, card_id: &str) -> Option<&bot_browser::AssistCardPayload> {
        self.find_bot_assist_card(card_id).map(|c| &c.payload)
    }

    /// Returns `true` if `card_id` refers to an editable login card (`AwaitingUser` or `TimedOutResumable`).
    pub fn is_bot_card_editable(&self, card_id: &str) -> bool {
        self.find_bot_assist_card(card_id)
            .is_some_and(BotAssistCardModel::is_login_editable)
    }

    /// Zeroizes and clears all ephemeral Bot Assist login and click drafts.
    pub fn clear_all_bot_card_drafts(&mut self) {
        for draft in self.bot_assist_login_drafts.values_mut() {
            draft.clear_and_zeroize();
        }
        self.bot_assist_login_drafts.clear();
        self.bot_assist_click_drafts.clear();
        self.active_bot_card_field = None;
        self.bot_assist_ime_preedit = None;
        self.pending_bot_assist_action = None;
    }

    /// Zeroizes and removes draft state for a specific `card_id`.
    pub fn clear_bot_card_draft(&mut self, card_id: &str) {
        if let Some(mut draft) = self.bot_assist_login_drafts.remove(card_id) {
            draft.clear_and_zeroize();
        }
        self.bot_assist_click_drafts.remove(card_id);
        if self
            .active_bot_card_field
            .as_ref()
            .is_some_and(|(cid, _)| cid == card_id)
        {
            self.active_bot_card_field = None;
            self.bot_assist_ime_preedit = None;
        }
    }

    /// Focuses a login field on an editable `VaultLogin` card.
    pub fn focus_bot_card_field(&mut self, card_id: &str, field: BotAssistLoginField) -> bool {
        let Some(payload) = self.find_bot_assist_payload(card_id).cloned() else {
            return false;
        };
        if !matches!(
            payload,
            bot_browser::AssistCardPayload::VaultLogin {
                status: bot_browser::AssistCardStatus::AwaitingUser
                    | bot_browser::AssistCardStatus::TimedOutResumable,
                ..
            }
        ) {
            return false;
        }
        for (cid, draft) in &mut self.bot_assist_login_drafts {
            if cid != card_id {
                draft.active_field = None;
            }
        }
        let draft = self
            .bot_assist_login_drafts
            .entry(card_id.to_string())
            .or_insert_with(|| BotAssistLoginDraft::for_payload(&payload));
        draft.active_field = Some(field);
        self.active_bot_card_field = Some((card_id.to_string(), field));
        self.bot_assist_ime_preedit = None;
        true
    }

    /// Blurs any focused Bot Assist card input without erasing draft contents.
    pub fn blur_bot_card_input(&mut self) {
        self.active_bot_card_field = None;
        self.bot_assist_ime_preedit = None;
        for draft in self.bot_assist_login_drafts.values_mut() {
            draft.active_field = None;
        }
    }

    /// Returns the committed text in the currently active Bot Assist card field, if any.
    pub fn active_bot_card_text(&self) -> &str {
        let Some((card_id, field)) = &self.active_bot_card_field else {
            return "";
        };
        self.bot_assist_login_drafts
            .get(card_id)
            .map(|d| d.field_text(*field))
            .unwrap_or("")
    }

    /// Inserts committed/pasted text into the active Bot Assist card field.
    pub fn insert_bot_card_text(&mut self, text: &str) -> bool {
        let Some((card_id, field)) = self.active_bot_card_field.clone() else {
            return false;
        };
        if !self.is_bot_card_editable(&card_id) {
            return false;
        }
        if let Some(draft) = self.bot_assist_login_drafts.get_mut(&card_id) {
            draft.active_field = Some(field);
            return draft.insert_text(text);
        }
        false
    }

    /// Backspaces a character or word from the active Bot Assist card field.
    pub fn backspace_bot_card_field(&mut self, delete_word: bool) -> bool {
        let Some((card_id, field)) = self.active_bot_card_field.clone() else {
            return false;
        };
        if !self.is_bot_card_editable(&card_id) {
            return false;
        }
        if let Some(draft) = self.bot_assist_login_drafts.get_mut(&card_id) {
            draft.active_field = Some(field);
            return draft.backspace(delete_word);
        }
        false
    }

    /// Zeroizes and clears the active Bot Assist card field.
    pub fn clear_active_bot_card_field(&mut self) -> bool {
        let Some((card_id, field)) = self.active_bot_card_field.clone() else {
            return false;
        };
        if !self.is_bot_card_editable(&card_id) {
            return false;
        }
        if let Some(draft) = self.bot_assist_login_drafts.get_mut(&card_id) {
            draft.active_field = Some(field);
            return draft.clear_active_field();
        }
        false
    }

    /// Cycles focus across `Username -> Password -> TotpSecret` on the active card.
    pub fn cycle_bot_card_field(&mut self, reverse: bool) -> Option<BotAssistLoginField> {
        let (card_id, current_field) = self.active_bot_card_field.clone()?;
        if !self.is_bot_card_editable(&card_id) {
            return None;
        }
        let draft = self.bot_assist_login_drafts.get_mut(&card_id)?;
        draft.active_field = Some(current_field);
        let next = draft.cycle_field(reverse);
        self.active_bot_card_field = Some((card_id, next));
        self.bot_assist_ime_preedit = None;
        Some(next)
    }

    /// Toggles `show_password` for the given card's login draft.
    pub fn toggle_bot_card_show_password(&mut self, card_id: &str) {
        let Some(payload) = self.find_bot_assist_payload(card_id).cloned() else {
            return;
        };
        let draft = self
            .bot_assist_login_drafts
            .entry(card_id.to_string())
            .or_insert_with(|| BotAssistLoginDraft::for_payload(&payload));
        draft.show_password = !draft.show_password;
    }

    /// Toggles `allow_auto_reuse` for the given card's login draft.
    pub fn toggle_bot_card_auto_reuse(&mut self, card_id: &str) {
        let Some(payload) = self.find_bot_assist_payload(card_id).cloned() else {
            return;
        };
        let draft = self
            .bot_assist_login_drafts
            .entry(card_id.to_string())
            .or_insert_with(|| BotAssistLoginDraft::for_payload(&payload));
        draft.allow_auto_reuse = !draft.allow_auto_reuse;
    }

    /// Takes a `LocalLoginSubmission` from `card_id`'s draft while zeroizing the in-memory draft.
    pub fn take_bot_card_login_submission(
        &mut self,
        card_id: &str,
    ) -> Result<bot_browser::LocalLoginSubmission, String> {
        if !self.is_bot_card_editable(card_id) {
            return Err("当前登录卡片已锁定".to_string());
        }
        let Some(draft) = self.bot_assist_login_drafts.get_mut(card_id) else {
            return Err("请输入账号和密码".to_string());
        };
        let res = draft.take_submission(card_id);
        if res.is_ok() {
            self.bot_assist_login_drafts.remove(card_id);
            if self
                .active_bot_card_field
                .as_ref()
                .is_some_and(|(cid, _)| cid == card_id)
            {
                self.active_bot_card_field = None;
                self.bot_assist_ime_preedit = None;
            }
        }
        res
    }

    /// Inserts or updates a strictly local `MessageContent::LocalBotAssistCard` message for `chat_id`.
    pub fn upsert_bot_assist_card(
        &mut self,
        chat_id: u64,
        local_msg_id: u64,
        payload: bot_browser::AssistCardPayload,
        timestamp: String,
    ) {
        let card_id = payload.card_id().to_string();
        if matches!(
            payload,
            bot_browser::AssistCardPayload::VaultLogin {
                status: bot_browser::AssistCardStatus::AwaitingUser
                    | bot_browser::AssistCardStatus::TimedOutResumable,
                ..
            }
        ) {
            self.bot_assist_login_drafts
                .entry(card_id.clone())
                .or_insert_with(|| BotAssistLoginDraft::for_payload(&payload));
        }
        if matches!(
            payload,
            bot_browser::AssistCardPayload::ClickCaptcha {
                status: bot_browser::AssistCardStatus::AwaitingUser
                    | bot_browser::AssistCardStatus::TimedOutResumable,
                ..
            }
        ) {
            self.bot_assist_click_drafts
                .entry(card_id.clone())
                .or_default();
        }

        let model = MessageItemModel {
            id: local_msg_id,
            sender_id: 0,
            sender_name: "安全助手".to_string(),
            avatar_initials: "🔒".to_string(),
            avatar_color: 0x1E293B,
            direction: MessageDirection::Incoming,
            timestamp,
            delivery_status: DeliveryStatus::Read,
            reply_to: None,
            reactions: Vec::new(),
            content: MessageContent::LocalBotAssistCard(BotAssistCardModel::new(payload)),
            bot_keyboard: None,
            is_pinned: false,
        };

        let upsert_in = |list: &mut Vec<MessageItemModel>| {
            if let Some(existing) = list.iter_mut().find(|m| {
                m.id == local_msg_id
                    || matches!(
                        &m.content,
                        MessageContent::LocalBotAssistCard(c) if c.card_id() == card_id
                    )
            }) {
                existing.content = model.content.clone();
            } else {
                list.push(model.clone());
            }
        };

        let hist = self.chat_histories.entry(chat_id).or_default();
        upsert_in(hist);

        if self.active_chat_id == Some(chat_id) {
            let was_at_bottom = self.is_at_bottom();
            upsert_in(&mut self.messages);
            self.row_heights_probe.borrow_mut().remove(&local_msg_id);
            self.row_height_cache.measured_heights.remove(&local_msg_id);
            self.row_height_cache.mark_dirty();
            if was_at_bottom {
                self.scroll_to_bottom();
            }
        }
    }

    /// Updates status/metadata on an existing `LocalBotAssistCard` and zeroizes drafts on terminal state.
    pub fn update_bot_assist_card_status(
        &mut self,
        chat_id: Option<u64>,
        card_id: &str,
        status: bot_browser::AssistCardStatus,
        status_note: Option<String>,
        saved_credential_id: Option<String>,
        saved_account_hint: Option<String>,
        redirected_url: Option<String>,
    ) -> bool {
        let mut updated = false;
        let mut matched_msg_id = None;
        let apply_list = |list: &mut Vec<MessageItemModel>,
                          updated_flag: &mut bool,
                          msg_id_out: &mut Option<u64>| {
            for m in list.iter_mut() {
                if let MessageContent::LocalBotAssistCard(ref mut card) = m.content {
                    if card.card_id() == card_id {
                        card.apply_status_update(
                            status,
                            status_note.clone(),
                            saved_credential_id.clone(),
                            saved_account_hint.clone(),
                            redirected_url.clone(),
                        );
                        *updated_flag = true;
                        *msg_id_out = Some(m.id);
                    }
                }
            }
        };

        apply_list(&mut self.messages, &mut updated, &mut matched_msg_id);
        if let Some(cid) = chat_id {
            if let Some(hist) = self.chat_histories.get_mut(&cid) {
                apply_list(hist, &mut updated, &mut matched_msg_id);
            }
        } else {
            for hist in self.chat_histories.values_mut() {
                apply_list(hist, &mut updated, &mut matched_msg_id);
            }
        }

        if matches!(
            status,
            bot_browser::AssistCardStatus::SavedAndLoggedIn
                | bot_browser::AssistCardStatus::Completed
                | bot_browser::AssistCardStatus::Denied
                | bot_browser::AssistCardStatus::Expired
                | bot_browser::AssistCardStatus::Cancelled
        ) {
            self.clear_bot_card_draft(card_id);
        }
        if updated {
            if let Some(mid) = matched_msg_id {
                self.row_heights_probe.borrow_mut().remove(&mid);
                self.row_height_cache.measured_heights.remove(&mid);
            }
            self.row_height_cache.mark_dirty();
        }
        updated
    }

    /// Updates QR PNG image, countdown, and poll attempt on a `QrLogin` card.
    pub fn refresh_bot_assist_qr(
        &mut self,
        chat_id: Option<u64>,
        card_id: &str,
        png_bytes: Vec<u8>,
        countdown_secs: u32,
        poll_attempt: u32,
    ) -> bool {
        let mut updated = false;
        let apply_list = |list: &mut Vec<MessageItemModel>, updated_flag: &mut bool| {
            for m in list.iter_mut() {
                if let MessageContent::LocalBotAssistCard(ref mut card) = m.content {
                    if card.card_id() == card_id
                        && card.refresh_qr_image(png_bytes.clone(), countdown_secs, poll_attempt)
                    {
                        *updated_flag = true;
                    }
                }
            }
        };
        apply_list(&mut self.messages, &mut updated);
        if let Some(cid) = chat_id {
            if let Some(hist) = self.chat_histories.get_mut(&cid) {
                apply_list(hist, &mut updated);
            }
        } else {
            for hist in self.chat_histories.values_mut() {
                apply_list(hist, &mut updated);
            }
        }
        updated
    }
}
