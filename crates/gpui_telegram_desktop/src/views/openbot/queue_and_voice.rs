//! OpenBot Steerable FIFO `QueuePanel` (`steerRefused` guard) and Duplex `VoicePill` (`OB-F16`).
//!
//! Mirrors `QueuePanel.tsx`, `ConversationComposer.tsx` (`steerRefused`), and `VoicePill.tsx`:
//! - FIFO queued deliveries (`QUEUE_REORDER_LIMIT = 100`) with drag/drop + `Alt+Up`/`Alt+Down` reorder
//! - Pause / Resume drain, 10-minute `QUEUE_EDIT_HOLD_MS = 600_000` edit hold + confirm-delete guard
//! - Routine-turn `steerRefused` guard disabling `Tab` steer & row Steer button with `"Not steered"`
//!   badge + `"Routine run"` tooltip (distinct from `provider-unsupported` and `steer-failed`)
//! - Duplex `VoicePill` with `PILL_LEVELS = 12` GPUI `canvas(...)` `VoiceWaveform`, phases
//!   (`Starting`, `Listening`, `Thinking`, `Speaking`, `Offline`), `Open` / `Push` talk modes,
//!   mute, interrupt, cyclic TTS speed (`SPEECH_SPEEDS`), and end-call control.

use std::collections::{HashMap, HashSet};

use chrono::Utc;
use gpui::prelude::FluentBuilder;
use gpui::*;
use openbot_contracts::avatar::AvatarMood;
use openbot_contracts::limits::INPUT_LIMITS;
use openbot_contracts::queue::{
    QUEUE_EDIT_HOLD_MS as CONTRACT_QUEUE_EDIT_HOLD_MS, QueueDelivery, QueueDeliveryStatus,
    QueueHold, QueueSender, QueueSteerFallback, routine_runs_turn,
};
use openbot_contracts::voice::{SPEECH_SPEEDS, next_speech_speed};
use serde::{Deserialize, Serialize};

use crate::components::bloub_avatar::{AgentAvatarProps, render_agent_avatar};
use crate::theme::openbot_palette::OpenBotPalette;
use crate::theme::{IconName, tg_icon_rgba};

/// 10-minute edit hold (`600_000 ms`) while a queued delivery is being edited in the composer.
pub const QUEUE_EDIT_HOLD_MS: u64 = CONTRACT_QUEUE_EDIT_HOLD_MS;
/// Maximum queue items allowed in a single reorder operation (`100`).
pub const QUEUE_REORDER_LIMIT: usize = INPUT_LIMITS.queue_reorder;
/// Exact live waveform bar count inside `VoicePill`.
pub const PILL_LEVELS: usize = 12;
/// Badge displayed on a queue item when steering was refused on a routine turn.
pub const ROUTINE_STEER_BADGE: &str = "Not steered";
/// Tooltip displayed alongside `ROUTINE_STEER_BADGE` when active turn is a routine run.
pub const ROUTINE_STEER_TOOLTIP: &str = "Routine run";

/// Visual row model representing a single FIFO entry inside [`QueuePanelState`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct QueuePanelItem {
    pub delivery_id: String,
    pub text: String,
    pub sender: QueueSender,
    pub status: QueueDeliveryStatus,
    pub position: u32,
    pub attachment_count: usize,
    pub editing: bool,
    pub steer_fallback: Option<QueueSteerFallback>,
    pub routine_steer_refused: bool,
}

impl QueuePanelItem {
    pub fn new(
        delivery_id: impl Into<String>,
        text: impl Into<String>,
        sender: QueueSender,
        position: u32,
    ) -> Self {
        Self {
            delivery_id: delivery_id.into(),
            text: text.into(),
            sender,
            status: QueueDeliveryStatus::Queued,
            position,
            attachment_count: 0,
            editing: false,
            steer_fallback: None,
            routine_steer_refused: false,
        }
    }

    pub fn from_delivery(delivery: &QueueDelivery, fallback_position: u32) -> Self {
        Self {
            delivery_id: delivery.id.clone(),
            text: delivery.text.clone(),
            sender: delivery.sender.clone(),
            status: delivery.status,
            position: delivery.position.unwrap_or(fallback_position),
            attachment_count: delivery.attachments.len(),
            editing: delivery.editing.unwrap_or(false),
            steer_fallback: delivery.steer_fallback,
            routine_steer_refused: false,
        }
    }
}

/// Active 10-minute edit hold on a queued message with confirm-delete protection.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct QueueEditLock {
    pub delivery_id: String,
    pub edit_id: String,
    pub started_at_ms: u64,
    pub expires_at_ms: u64,
    pub confirm_delete_pending: bool,
}

impl QueueEditLock {
    pub fn is_expired(&self, now_ms: u64) -> bool {
        now_ms >= self.expires_at_ms
    }

    pub fn remaining_ms(&self, now_ms: u64) -> u64 {
        self.expires_at_ms.saturating_sub(now_ms)
    }
}

/// Steerability evaluation for active turn + `Tab` steer shortcut (`steerRefused` guard).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SteerEvaluation {
    pub can_steer: bool,
    pub steer_refused: bool,
    pub steered_on_tab: bool,
    pub fallback_reason: Option<String>,
    pub badge_label: Option<String>,
    pub tooltip: Option<String>,
}

/// Evaluates whether steering into the active turn is permitted or refused (`Routine run`).
pub fn evaluate_steer(active_turn_routine: bool, press_tab: bool) -> SteerEvaluation {
    if active_turn_routine {
        SteerEvaluation {
            can_steer: false,
            steer_refused: press_tab,
            steered_on_tab: false,
            fallback_reason: Some("routine-run".to_string()),
            badge_label: Some(ROUTINE_STEER_BADGE.to_string()),
            tooltip: Some(ROUTINE_STEER_TOOLTIP.to_string()),
        }
    } else {
        SteerEvaluation {
            can_steer: true,
            steer_refused: false,
            steered_on_tab: press_tab,
            fallback_reason: None,
            badge_label: None,
            tooltip: None,
        }
    }
}

/// Returns `(badge, tooltip)` for non-routine steer fallbacks (`provider-unsupported` / `steer-failed`).
pub const fn steer_fallback_copy(fallback: QueueSteerFallback) -> (&'static str, &'static str) {
    match fallback {
        QueueSteerFallback::ProviderUnsupported => (
            "Not steered",
            "Current provider does not support steering active turns",
        ),
        QueueSteerFallback::SteerFailed => (
            "Not steered",
            "Steer attempt failed; kept queued for next turn",
        ),
    }
}

/// Steerable FIFO queue panel state positioned above the OpenBot composer.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct QueuePanelState {
    pub items: Vec<QueuePanelItem>,
    pub paused: bool,
    pub active_turn_id: Option<String>,
    pub active_turn_routine: bool,
    pub edit_lock: Option<QueueEditLock>,
    pub dragging_index: Option<usize>,
    pub drag_target_index: Option<usize>,
    pub hold: Option<QueueHold>,
}

impl QueuePanelState {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn push_item(
        &mut self,
        delivery_id: impl Into<String>,
        text: impl Into<String>,
        sender: QueueSender,
    ) -> Result<usize, String> {
        if self.items.len() >= QUEUE_REORDER_LIMIT {
            return Err(format!(
                "Queue item count exceeds maximum reorder limit of {QUEUE_REORDER_LIMIT}."
            ));
        }
        let pos = (self.items.len() + 1) as u32;
        self.items.push(QueuePanelItem::new(delivery_id, text, sender, pos));
        Ok(self.items.len())
    }

    /// Reorders item at `from` to `to` (supports pointer drag-drop and `Alt+Up`/`Alt+Down`).
    pub fn reorder(&mut self, from: usize, to: usize) -> Result<(), String> {
        if self.items.len() > QUEUE_REORDER_LIMIT {
            return Err(format!(
                "Cannot reorder more than {QUEUE_REORDER_LIMIT} queued items."
            ));
        }
        if from >= self.items.len() || to >= self.items.len() {
            return Err(format!(
                "Queue reorder indices ({from} -> {to}) out of bounds for {} items.",
                self.items.len()
            ));
        }
        if from != to {
            let item = self.items.remove(from);
            self.items.insert(to, item);
            self.reindex_positions();
        }
        Ok(())
    }

    /// Keyboard `Alt+ArrowUp` (`move_up = true`) / `Alt+ArrowDown` (`move_up = false`) reorder.
    pub fn move_with_alt_arrow(&mut self, index: usize, move_up: bool) -> Result<usize, String> {
        if index >= self.items.len() {
            return Err(format!("Queue row index {index} out of bounds."));
        }
        let target = if move_up {
            index.saturating_sub(1)
        } else {
            (index + 1).min(self.items.len().saturating_sub(1))
        };
        self.reorder(index, target)?;
        Ok(target)
    }

    /// Applies an explicit delivery-id permutation, rejecting stale or partial lists.
    pub fn apply_delivery_id_order(&mut self, ordered_ids: &[String]) -> Result<(), String> {
        if ordered_ids.len() > QUEUE_REORDER_LIMIT {
            return Err(format!(
                "Queue reorder limit exceeded: max {QUEUE_REORDER_LIMIT} delivery IDs."
            ));
        }
        if ordered_ids.len() != self.items.len() {
            return Err(
                "Stale queue order: delivery ID list does not match current queued items."
                    .to_string(),
            );
        }
        let seen: HashSet<&str> = ordered_ids.iter().map(String::as_str).collect();
        if seen.len() != ordered_ids.len() {
            return Err("Invalid queue order: duplicate delivery IDs detected.".to_string());
        }
        let mut by_id: HashMap<String, QueuePanelItem> = self
            .items
            .drain(..)
            .map(|item| (item.delivery_id.clone(), item))
            .collect();
        let mut reordered = Vec::with_capacity(ordered_ids.len());
        for id in ordered_ids {
            if let Some(item) = by_id.remove(id) {
                reordered.push(item);
            } else {
                // Restore original items before failing.
                self.items = reordered;
                self.items.extend(by_id.into_values());
                self.reindex_positions();
                return Err(format!(
                    "Stale queue order: delivery '{id}' is no longer in the queue."
                ));
            }
        }
        self.items = reordered;
        self.reindex_positions();
        Ok(())
    }

    fn reindex_positions(&mut self) {
        for (idx, item) in self.items.iter_mut().enumerate() {
            item.position = (idx + 1) as u32;
        }
    }
}

impl QueuePanelState {
    pub fn pause(&mut self) {
        self.paused = true;
    }

    pub fn resume(&mut self) {
        self.paused = false;
    }

    /// Synchronizes `active_turn_routine` from raw `openbot_contracts::queue::QueueDelivery` rows.
    pub fn sync_from_deliveries(&mut self, deliveries: &[QueueDelivery], active_turn_id: Option<&str>) {
        self.active_turn_id = active_turn_id.map(ToString::to_string);
        self.active_turn_routine = routine_runs_turn(deliveries, active_turn_id);
        self.items = deliveries
            .iter()
            .filter(|d| d.status == QueueDeliveryStatus::Queued)
            .enumerate()
            .map(|(i, d)| QueuePanelItem::from_delivery(d, (i + 1) as u32))
            .collect();
    }

    /// Begins editing `delivery_id`, establishing a 10-minute (`QUEUE_EDIT_HOLD_MS`) edit lock.
    pub fn begin_edit(&mut self, delivery_id: &str) -> Result<&QueueEditLock, String> {
        let now_ms = Utc::now().timestamp_millis().max(0) as u64;
        self.begin_edit_at(delivery_id, now_ms)
    }

    /// Deterministic timestamp variant of [`Self::begin_edit`] for headless testing.
    pub fn begin_edit_at(
        &mut self,
        delivery_id: &str,
        now_ms: u64,
    ) -> Result<&QueueEditLock, String> {
        let Some(pos) = self
            .items
            .iter()
            .position(|item| item.delivery_id == delivery_id)
        else {
            return Err(format!("Queued delivery '{delivery_id}' not found for editing."));
        };

        for item in &mut self.items {
            item.editing = false;
        }
        self.items[pos].editing = true;

        self.edit_lock = Some(QueueEditLock {
            delivery_id: delivery_id.to_string(),
            edit_id: format!("edit-{delivery_id}-{now_ms}"),
            started_at_ms: now_ms,
            expires_at_ms: now_ms.saturating_add(QUEUE_EDIT_HOLD_MS),
            confirm_delete_pending: false,
        });
        Ok(self.edit_lock.as_ref().expect("just inserted edit lock"))
    }

    /// Cancels an active edit hold and clears `editing` state on queued items.
    pub fn cancel_edit(&mut self) {
        self.edit_lock = None;
        for item in &mut self.items {
            item.editing = false;
        }
    }

    /// Saves updated text back into the currently held delivery and releases the edit lock.
    pub fn save_edit(&mut self, updated_text: impl Into<String>) -> Result<(), String> {
        let Some(lock) = self.edit_lock.take() else {
            return Err("No active queue edit lock to save.".to_string());
        };
        let text = updated_text.into();
        if text.trim().is_empty() {
            self.edit_lock = Some(lock);
            return Err("Edited queued message text cannot be empty.".to_string());
        }
        if let Some(item) = self
            .items
            .iter_mut()
            .find(|i| i.delivery_id == lock.delivery_id)
        {
            item.text = text;
            item.editing = false;
            Ok(())
        } else {
            Err(format!("Queued delivery '{}' no longer exists.", lock.delivery_id))
        }
    }

    /// Requests deletion of a queued delivery. If the item is protected by an active edit hold
    /// and `confirmed == false`, arms `confirm_delete_pending = true` and returns `Ok(false)`.
    pub fn request_delete(&mut self, delivery_id: &str, confirmed: bool) -> Result<bool, String> {
        if let Some(lock) = self.edit_lock.as_mut()
            && lock.delivery_id == delivery_id
            && !confirmed
        {
            lock.confirm_delete_pending = true;
            return Ok(false);
        }
        let before = self.items.len();
        self.items.retain(|item| item.delivery_id != delivery_id);
        if self.items.len() == before {
            return Err(format!("Queued delivery '{delivery_id}' not found."));
        }
        if self
            .edit_lock
            .as_ref()
            .is_some_and(|lock| lock.delivery_id == delivery_id)
        {
            self.edit_lock = None;
        }
        self.reindex_positions();
        Ok(true)
    }

    /// Releases the edit hold if its 10-minute (`QUEUE_EDIT_HOLD_MS`) window has elapsed.
    pub fn sweep_expired_edit_lock(&mut self, now_ms: u64) -> bool {
        if self
            .edit_lock
            .as_ref()
            .is_some_and(|lock| lock.is_expired(now_ms))
        {
            self.cancel_edit();
            true
        } else {
            false
        }
    }

    /// Evaluates steering into the active turn with the `steerRefused` routine-run guard.
    pub fn evaluate_steer(&self, active_turn_routine: bool, press_tab: bool) -> SteerEvaluation {
        evaluate_steer(active_turn_routine, press_tab)
    }

    /// Convenience wrapper using `self.active_turn_routine`.
    pub fn evaluate_steer_for_state(&self, press_tab: bool) -> SteerEvaluation {
        evaluate_steer(self.active_turn_routine, press_tab)
    }

    /// Enqueues a busy follow-up delivery (if not already present) and evaluates `Tab` steer
    /// against `active_turn_routine`, updating the queued item's running/refused state (`PAIR-19`).
    pub fn enqueue_and_steer_on_tab(
        &mut self,
        delivery_id: &str,
        text: &str,
        active_turn_routine: bool,
    ) -> (SteerEvaluation, bool) {
        self.active_turn_routine = active_turn_routine;
        if !self.items.iter().any(|i| i.delivery_id == delivery_id) {
            let _ = self.push_item(delivery_id, text, QueueSender::User);
        }
        let eval = self.evaluate_steer_for_state(true);
        let mut steered = false;
        let target_idx = self
            .items
            .iter()
            .position(|i| i.delivery_id == delivery_id)
            .or_else(|| (!self.items.is_empty()).then_some(0));
        if let Some(target) = target_idx.and_then(|idx| self.items.get_mut(idx)) {
            if eval.steered_on_tab {
                target.status = QueueDeliveryStatus::Running;
                target.routine_steer_refused = false;
                steered = true;
            } else if eval.steer_refused {
                target.routine_steer_refused = true;
            }
        }
        (eval, steered)
    }

    /// Drains all queued items and clears transient edit/drag locks when returning to idle (`PAIR-19`).
    pub fn drain_all(&mut self) -> usize {
        let drained = self.items.len();
        self.items.clear();
        self.edit_lock = None;
        self.dragging_index = None;
        self.drag_target_index = None;
        self.paused = false;
        drained
    }
}

fn palette_color(palette: &OpenBotPalette, token: &str, fallback: u32) -> Rgba {
    palette.rgba(token).unwrap_or_else(|| rgb(fallback))
}

fn sender_badge(sender: &QueueSender) -> String {
    match sender {
        QueueSender::User => "You".to_string(),
        QueueSender::Agent { agent_id } => format!("@{agent_id}"),
        QueueSender::Routine { routine_name, .. } => format!("Routine · {routine_name}"),
    }
}

/// Renders the steerable FIFO `QueuePanel` above the composer, including reorder controls,
/// pause/resume toggle, 10-min edit hold + confirm-delete banner, and `steerRefused` routine guard.
pub fn render_queue_panel(state: &QueuePanelState, palette: &OpenBotPalette) -> Div {
    let surface = palette_color(palette, "--openbot-bg-surface", 0x212121);
    let surface_raised = palette_color(palette, "--openbot-bg-surface-raised", 0x242424);
    let border = palette_color(palette, "--openbot-border-strong", 0x333335);
    let text_primary = palette_color(palette, "--openbot-text-primary", 0xffffff);
    let text_muted = palette_color(palette, "--openbot-text-muted", 0x979797);
    let accent = palette_color(palette, "--openbot-accent", 0x007cf7);
    let accent_soft = palette_color(palette, "--openbot-accent-soft", 0x1a304d);
    let warning = palette_color(palette, "--openbot-warning", 0xff9412);
    let warning_soft = palette_color(palette, "--openbot-warning-soft", 0x3a2b16);
    let danger = palette_color(palette, "--openbot-danger", 0xfd2f3b);
    let danger_soft = palette_color(palette, "--openbot-danger-soft", 0x3d191c);

    let steer_eval = state.evaluate_steer_for_state(false);

    let mut list = div()
        .id("openbot-queue-panel-list")
        .flex()
        .flex_col()
        .gap(px(4.0))
        .max_h(px(196.0))
        .min_h(px(0.0))
        .overflow_y_scroll();

    for (idx, item) in state.items.iter().enumerate() {
        let is_drag_target = state.drag_target_index == Some(idx);
        let row_border = if is_drag_target { accent } else { border };
        let fallback_badge: Option<(&str, &str)> = if state.active_turn_routine || item.routine_steer_refused {
            Some((ROUTINE_STEER_BADGE, ROUTINE_STEER_TOOLTIP))
        } else {
            item.steer_fallback.map(steer_fallback_copy)
        };

        list = list.child(
            div()
                .id(("openbot-queue-row", idx))
                .flex()
                .items_center()
                .justify_between()
                .gap(px(8.0))
                .px(px(8.0))
                .py(px(6.0))
                .rounded(px(10.0))
                .bg(surface_raised)
                .border_1()
                .border_color(row_border)
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(6.0))
                        .min_w(px(0.0))
                        .flex_1()
                        .child(
                            div()
                                .text_xs()
                                .font_weight(FontWeight::SEMIBOLD)
                                .text_color(text_muted)
                                .child(format!("#{}", item.position)),
                        )
                        .child(
                            div()
                                .px(px(5.0))
                                .py(px(1.0))
                                .rounded(px(6.0))
                                .bg(accent_soft)
                                .text_xs()
                                .text_color(accent)
                                .child(sender_badge(&item.sender)),
                        )
                        .child(
                            div()
                                .text_sm()
                                .text_color(text_primary)
                                .truncate()
                                .flex_1()
                                .child(item.text.clone()),
                        )
                        .when(item.editing, |el| {
                            el.child(
                                div()
                                    .px(px(6.0))
                                    .py(px(1.0))
                                    .rounded(px(999.0))
                                    .bg(warning_soft)
                                    .text_xs()
                                    .text_color(warning)
                                    .child("Editing (10m hold)"),
                            )
                        })
                        .when_some(fallback_badge, |el, (badge, tooltip)| {
                            el.child(
                                div()
                                    .flex()
                                    .items_center()
                                    .gap(px(4.0))
                                    .px(px(6.0))
                                    .py(px(1.0))
                                    .rounded(px(999.0))
                                    .bg(warning_soft)
                                    .text_xs()
                                    .text_color(warning)
                                    .child(format!("{badge} · {tooltip}")),
                            )
                        }),
                )
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(4.0))
                        .child(
                            div()
                                .px(px(6.0))
                                .py(px(2.0))
                                .rounded(px(6.0))
                                .bg(if steer_eval.can_steer { accent_soft } else { surface })
                                .text_xs()
                                .font_weight(FontWeight::MEDIUM)
                                .text_color(if steer_eval.can_steer { accent } else { text_muted })
                                .child(if steer_eval.can_steer {
                                    "Steer (Tab)"
                                } else {
                                    "Steer off"
                                }),
                        )
                        .child(tg_icon_rgba(IconName::Edit, 12.0, text_muted))
                        .child(tg_icon_rgba(IconName::DeleteTrash, 12.0, text_muted)),
                ),
        );
    }

    let mut panel = div()
        .w_full()
        .flex()
        .flex_col()
        .gap(px(6.0))
        .p(px(8.0))
        .rounded(px(14.0))
        .bg(surface)
        .border_1()
        .border_color(border)
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
                                .text_xs()
                                .font_weight(FontWeight::SEMIBOLD)
                                .text_color(text_primary)
                                .child(format!("Queue ({}/{})", state.items.len(), QUEUE_REORDER_LIMIT)),
                        )
                        .when(state.active_turn_routine, |el| {
                            el.child(
                                div()
                                    .px(px(6.0))
                                    .py(px(1.0))
                                    .rounded(px(999.0))
                                    .bg(warning_soft)
                                    .text_xs()
                                    .text_color(warning)
                                    .child(format!("{ROUTINE_STEER_BADGE} · {ROUTINE_STEER_TOOLTIP}")),
                            )
                        }),
                )
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(6.0))
                        .child(
                            div()
                                .text_xs()
                                .text_color(text_muted)
                                .child("Alt+↑/↓ reorder"),
                        )
                        .child(
                            div()
                                .id("openbot-queue-pause-toggle")
                                .flex()
                                .items_center()
                                .gap(px(4.0))
                                .px(px(8.0))
                                .py(px(2.0))
                                .rounded(px(999.0))
                                .bg(if state.paused { warning_soft } else { accent_soft })
                                .text_xs()
                                .font_weight(FontWeight::MEDIUM)
                                .text_color(if state.paused { warning } else { accent })
                                .child(tg_icon_rgba(
                                    if state.paused { IconName::Play } else { IconName::Pause },
                                    11.0,
                                    if state.paused { warning } else { accent },
                                ))
                                .child(if state.paused { "Resume" } else { "Pause" }),
                        ),
                ),
        )
        .child(list);

    if let Some(lock) = state.edit_lock.as_ref()
        && lock.confirm_delete_pending
    {
        panel = panel.child(
            div()
                .flex()
                .items_center()
                .justify_between()
                .px(px(8.0))
                .py(px(5.0))
                .rounded(px(8.0))
                .bg(danger_soft)
                .border_1()
                .border_color(danger)
                .child(
                    div()
                        .text_xs()
                        .text_color(danger)
                        .child(format!(
                            "Confirm delete of held delivery '{}'?",
                            lock.delivery_id
                        )),
                )
                .child(
                    div()
                        .text_xs()
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(danger)
                        .child("Confirm Delete"),
                ),
        );
    }

    panel
}

/// Duplex voice pill lifecycle phase (`VoicePill.tsx`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum VoicePillPhase {
    Starting,
    Listening,
    Thinking,
    Speaking,
    #[default]
    Offline,
}

impl VoicePillPhase {
    pub const ALL: [Self; 5] = [
        Self::Starting,
        Self::Listening,
        Self::Thinking,
        Self::Speaking,
        Self::Offline,
    ];

    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Starting => "starting",
            Self::Listening => "listening",
            Self::Thinking => "thinking",
            Self::Speaking => "speaking",
            Self::Offline => "offline",
        }
    }

    pub fn parse(raw: &str) -> Option<Self> {
        match raw.trim().to_ascii_lowercase().as_str() {
            "starting" | "connecting" => Some(Self::Starting),
            "listening" => Some(Self::Listening),
            "thinking" => Some(Self::Thinking),
            "speaking" => Some(Self::Speaking),
            "offline" => Some(Self::Offline),
            _ => None,
        }
    }

    pub const fn label(self) -> &'static str {
        match self {
            Self::Starting => "Starting",
            Self::Listening => "Listening",
            Self::Thinking => "Thinking",
            Self::Speaking => "Speaking",
            Self::Offline => "Offline",
        }
    }

    pub const fn is_live(self) -> bool {
        matches!(
            self,
            Self::Starting | Self::Listening | Self::Thinking | Self::Speaking
        )
    }

    pub const fn avatar_mood(self) -> AvatarMood {
        match self {
            Self::Starting => AvatarMood::Connecting,
            Self::Listening => AvatarMood::Waiting,
            Self::Thinking => AvatarMood::Working,
            Self::Speaking => AvatarMood::Responded,
            Self::Offline => AvatarMood::Idle,
        }
    }
}

/// Duplex voice microphone activation mode (`open` hands-free vs `push` push-to-talk).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum VoiceTalkMode {
    #[default]
    Open,
    Push,
}

impl VoiceTalkMode {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Open => "open",
            Self::Push => "push",
        }
    }

    pub fn parse(raw: &str) -> Option<Self> {
        match raw.trim().to_ascii_lowercase().as_str() {
            "open" => Some(Self::Open),
            "push" => Some(Self::Push),
            _ => None,
        }
    }

    /// Push-to-talk key bindings are suppressed while focus sits inside an editable text field.
    pub const fn ignores_talk_key(self, focus_in_editable: bool) -> bool {
        matches!(self, Self::Push) && focus_in_editable
    }
}

/// Full interactive state for the floating duplex `VoicePill` widget.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct VoicePillState {
    pub phase: VoicePillPhase,
    pub talk_mode: VoiceTalkMode,
    pub levels: [f32; PILL_LEVELS],
    pub muted: bool,
    pub speed_index: usize,
    pub download_progress: Option<f32>,
    pub talk_key_label: String,
    pub reduced_motion: bool,
}

impl Default for VoicePillState {
    fn default() -> Self {
        Self::new(VoicePillPhase::Offline, VoiceTalkMode::Open)
    }
}

impl VoicePillState {
    pub fn new(phase: VoicePillPhase, talk_mode: VoiceTalkMode) -> Self {
        Self {
            phase,
            talk_mode,
            levels: [0.18; PILL_LEVELS],
            muted: false,
            speed_index: 1, // SPEECH_SPEEDS[1] == 1.0x
            download_progress: None,
            talk_key_label: "Space".to_string(),
            reduced_motion: false,
        }
    }

    pub const fn is_live(&self) -> bool {
        self.phase.is_live()
    }

    pub const fn bar_count(&self) -> usize {
        PILL_LEVELS
    }

    pub fn current_speed(&self) -> f64 {
        SPEECH_SPEEDS[self.speed_index % SPEECH_SPEEDS.len()]
    }

    /// Cycles TTS speed through `openbot_contracts::voice::SPEECH_SPEEDS` (`1.0->1.25->1.5->2.0->0.75->1.0`).
    pub fn cycle_speed(&mut self) -> f64 {
        let next = next_speech_speed(self.current_speed());
        if let Some(idx) = SPEECH_SPEEDS
            .iter()
            .position(|&s| (s - next).abs() < f64::EPSILON)
        {
            self.speed_index = idx;
        }
        next
    }

    pub fn formatted_speed(&self) -> String {
        let speed = self.current_speed();
        if (speed - speed.round()).abs() < 1e-6 {
            format!("{}x", speed.round() as u32)
        } else {
            format!("{speed}x")
        }
    }

    pub fn toggle_mute(&mut self) {
        self.muted = !self.muted;
    }

    pub fn toggle_talk_mode(&mut self) {
        self.talk_mode = match self.talk_mode {
            VoiceTalkMode::Open => VoiceTalkMode::Push,
            VoiceTalkMode::Push => VoiceTalkMode::Open,
        };
    }

    pub fn interrupt(&mut self) {
        if matches!(self.phase, VoicePillPhase::Speaking | VoicePillPhase::Thinking) {
            self.phase = VoicePillPhase::Listening;
        }
    }

    pub fn end_call(&mut self) {
        self.phase = VoicePillPhase::Offline;
        self.levels = [0.14; PILL_LEVELS];
        self.download_progress = None;
    }

    /// Computes live 12-bar normalized waveform amplitudes (`0.12..=1.0`) for `now_sec`.
    pub fn sample_waveform_levels(&self, now_sec: f64) -> [f32; PILL_LEVELS] {
        let mut out = [0.14_f32; PILL_LEVELS];
        if self.muted || self.phase == VoicePillPhase::Offline {
            return out;
        }
        for (i, slot) in out.iter_mut().enumerate() {
            let base = self.levels[i].clamp(0.12, 1.0);
            if self.reduced_motion {
                let envelope = 1.0 - ((i as f32 - 5.5).abs() / 6.5) * 0.45;
                *slot = (base.max(0.42) * envelope).clamp(0.14, 0.95);
            } else {
                let freq = match self.phase {
                    VoicePillPhase::Speaking => 6.4,
                    VoicePillPhase::Listening => 4.2,
                    VoicePillPhase::Thinking => 2.6,
                    VoicePillPhase::Starting => 2.0,
                    VoicePillPhase::Offline => 0.0,
                };
                let phase_shift = (i as f64) * 0.58;
                let wave = ((now_sec * freq + phase_shift).sin() * 0.5 + 0.5) as f32;
                let center_weight = 1.0 - ((i as f32 - 5.5).abs() / 7.0) * 0.35;
                let target = (0.18 + 0.78 * wave * center_weight).max(base * 0.6);
                *slot = target.clamp(0.14, 1.0);
            }
        }
        out
    }
}

fn render_voice_waveform_canvas(
    levels: [f32; PILL_LEVELS],
    bar_color: Rgba,
) -> Div {
    div().w(px(64.0)).h(px(22.0)).flex_shrink_0().child(
        canvas(
            |_, _, _| {},
            move |bounds, _, window, _cx| {
                let x0 = f32::from(bounds.origin.x);
                let y0 = f32::from(bounds.origin.y);
                let w = f32::from(bounds.size.width).max(12.0);
                let h = f32::from(bounds.size.height).max(6.0);
                let slot_w = w / (PILL_LEVELS as f32);
                let bar_w = (slot_w * 0.54).clamp(2.0, 4.0);
                let mut builder = PathBuilder::fill();

                for (idx, level) in levels.iter().copied().enumerate() {
                    let bar_h = (h * level.clamp(0.14, 1.0)).max(3.0);
                    let bx = x0 + (idx as f32) * slot_w + (slot_w - bar_w) * 0.5;
                    let by = y0 + (h - bar_h) * 0.5;
                    builder.move_to(point(px(bx), px(by)));
                    builder.line_to(point(px(bx + bar_w), px(by)));
                    builder.line_to(point(px(bx + bar_w), px(by + bar_h)));
                    builder.line_to(point(px(bx), px(by + bar_h)));
                    builder.close();
                }

                if let Ok(path) = builder.build() {
                    window.paint_path(path, bar_color);
                }
            },
        )
        .size_full(),
    )
}

/// Renders the floating duplex `VoicePill` with agent avatar, 12-bar `VoiceWaveform` canvas,
/// phase & talk-mode badges, download progress (when `Starting`), mute, interrupt, speed cycle
/// chip, and end-call button.
pub fn render_voice_pill(
    state: &VoicePillState,
    agent_seed: &str,
    palette: &OpenBotPalette,
    now_sec: f64,
) -> Div {
    let glass = palette_color(palette, "--openbot-bg-glass", 0x212121);
    let control = palette_color(palette, "--openbot-bg-control", 0x212121);
    let control_hover = palette_color(palette, "--openbot-bg-control-hover", 0x2a2a2a);
    let border = palette_color(palette, "--openbot-dialog-ring", 0x3a3a3c);
    let text_primary = palette_color(palette, "--openbot-text-primary", 0xffffff);
    let text_muted = palette_color(palette, "--openbot-text-muted", 0x979797);
    let text_dim = palette_color(palette, "--openbot-text-dim", 0x6a6a6a);
    let accent = palette_color(palette, "--openbot-accent", 0x007cf7);
    let success = palette_color(palette, "--openbot-success", 0x31cf76);
    let warning = palette_color(palette, "--openbot-warning", 0xff9412);
    let danger = palette_color(palette, "--openbot-danger", 0xfd2f3b);
    let danger_soft = palette_color(palette, "--openbot-danger-soft", 0x3d191c);
    let text_on_accent = palette_color(palette, "--openbot-text-on-accent", 0xffffff);

    let wave_color = if state.muted || state.phase == VoicePillPhase::Offline {
        text_dim
    } else {
        match state.phase {
            VoicePillPhase::Speaking => success,
            VoicePillPhase::Listening => accent,
            VoicePillPhase::Thinking | VoicePillPhase::Starting => warning,
            VoicePillPhase::Offline => text_dim,
        }
    };

    let avatar_props = AgentAvatarProps::new(agent_seed)
        .with_size(26.0)
        .with_mood(state.phase.avatar_mood());
    let levels = state.sample_waveform_levels(now_sec);

    let mode_text = match state.talk_mode {
        VoiceTalkMode::Open => "Open mic".to_string(),
        VoiceTalkMode::Push => format!("Hold {}", state.talk_key_label),
    };

    div()
        .flex()
        .items_center()
        .gap(px(10.0))
        .px(px(12.0))
        .py(px(6.0))
        .rounded(px(999.0))
        .bg(glass)
        .border_1()
        .border_color(border)
        .child(render_agent_avatar(&avatar_props, None, now_sec))
        .child(
            div()
                .flex()
                .flex_col()
                .gap(px(1.0))
                .child(
                    div()
                        .text_xs()
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(text_primary)
                        .child(state.phase.label()),
                )
                .child(
                    div()
                        .text_xs()
                        .text_color(text_muted)
                        .child(if let Some(p) = state.download_progress
                            && state.phase == VoicePillPhase::Starting
                        {
                            format!("Downloading {}%", (p.clamp(0.0, 1.0) * 100.0).round() as u8)
                        } else {
                            mode_text
                        }),
                ),
        )
        .child(render_voice_waveform_canvas(levels, wave_color))
        .child(
            div()
                .flex()
                .items_center()
                .gap(px(6.0))
                .child(
                    div()
                        .id("openbot-voice-mute-btn")
                        .w(px(28.0))
                        .h(px(28.0))
                        .rounded(px(999.0))
                        .bg(if state.muted { danger_soft } else { control })
                        .flex()
                        .items_center()
                        .justify_center()
                        .hover(move |s| s.bg(control_hover))
                        .child(tg_icon_rgba(
                            if state.muted { IconName::Mute } else { IconName::Mic },
                            13.0,
                            if state.muted { danger } else { text_primary },
                        )),
                )
                .when(
                    matches!(state.phase, VoicePillPhase::Speaking | VoicePillPhase::Thinking),
                    |el| {
                        el.child(
                            div()
                                .id("openbot-voice-interrupt-btn")
                                .px(px(8.0))
                                .h(px(26.0))
                                .rounded(px(999.0))
                                .bg(control)
                                .flex()
                                .items_center()
                                .gap(px(4.0))
                                .text_xs()
                                .font_weight(FontWeight::MEDIUM)
                                .text_color(text_primary)
                                .hover(move |s| s.bg(control_hover))
                                .child(tg_icon_rgba(IconName::Pause, 11.0, text_primary))
                                .child("Stop"),
                        )
                    },
                )
                .child(
                    div()
                        .id("openbot-voice-speed-btn")
                        .px(px(8.0))
                        .h(px(26.0))
                        .rounded(px(999.0))
                        .bg(control)
                        .flex()
                        .items_center()
                        .text_xs()
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(text_primary)
                        .hover(move |s| s.bg(control_hover))
                        .child(state.formatted_speed()),
                )
                .child(
                    div()
                        .id("openbot-voice-end-btn")
                        .w(px(28.0))
                        .h(px(28.0))
                        .rounded(px(999.0))
                        .bg(danger)
                        .flex()
                        .items_center()
                        .justify_center()
                        .child(tg_icon_rgba(IconName::Hangup, 13.0, text_on_accent)),
                ),
        )
}

pub const COMPOSER_STACK_RADIUS_PX: f32 = 14.0;
pub const COMPOSER_STACK_OVERLAP_BOTTOM_PX: f32 = 24.0;
pub const COMPOSER_STACK_Z_INDEX: usize = 10;
pub const COMPOSER_PILL_Z_INDEX: usize = 11;
pub const COMPOSER_STACK_ROW_HEIGHT_PX: f32 = 29.0;
pub const COMPOSER_STACK_MAX_VISIBLE_ROWS: usize = 7;
pub const COMPOSER_REPLY_PREVIEW_MIN_HEIGHT_PX: f32 = 42.0;
pub const COMPOSER_OPEN_WAITS_MAX: usize = 20;
pub const COMPOSER_NOTICE_MAX_BODY_CHARS: usize = 240;

/// Dismissible tucked notice card (`ComposerNotice`) with optional copy-details reference (`UI-F14-06`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ComposerNoticeState {
    pub title: Option<String>,
    pub body: String,
    pub reference: Option<String>,
    pub dismissible: bool,
}

impl ComposerNoticeState {
    pub fn is_truncated(&self) -> bool {
        self.body.chars().count() > COMPOSER_NOTICE_MAX_BODY_CHARS
    }

    pub fn has_copy_details(&self) -> bool {
        self.reference
            .as_deref()
            .is_some_and(|r| !r.trim().is_empty())
            || self.is_truncated()
    }
}

pub struct ComposerStackInput<'a> {
    pub queue_items: &'a [String],
    pub queue_can_steer: bool,
    pub reply_sender: Option<&'a str>,
    pub awaiting_tasks: &'a [String],
    pub awaiting_all_settled: bool,
    pub stopped_tasks: &'a [String],
    pub open_waits_count: usize,
    pub notice: Option<&'a ComposerNoticeState>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ComposerStackLayoutSnapshot {
    pub visible: bool,
    pub height_px: f32,
    pub radius_px: f32,
    pub overlap_bottom_px: f32,
    pub z_index: usize,
    pub composer_z_index: usize,
    pub queue_row_height_px: f32,
    pub queue_items: Vec<String>,
    pub queue_visible_rows: usize,
    pub queue_can_steer: bool,
    pub reply_preview_visible: bool,
    pub reply_preview_min_height_px: f32,
    pub reply_preview_sender: Option<String>,
    pub awaiting_visible: bool,
    pub awaiting_title: &'static str,
    pub awaiting_tasks: Vec<String>,
    pub awaiting_dismiss_available: bool,
    pub stopped_visible: bool,
    pub stopped_tasks: Vec<String>,
    pub open_waits_count: usize,
    pub notice_visible: bool,
    pub notice_has_copy_details: bool,
    pub notice_truncated: bool,
}

/// Computes the single-source-of-truth tucked `.composer-stack` geometry consumed directly by
/// `render_bot_fusion_composer_strip` (`UI-F14`).
pub fn compute_composer_stack_layout(input: &ComposerStackInput<'_>) -> ComposerStackLayoutSnapshot {
    let queue_visible_rows = input.queue_items.len().min(COMPOSER_STACK_MAX_VISIBLE_ROWS);
    let reply_preview_visible = input
        .reply_sender
        .is_some_and(|s| !s.trim().is_empty());
    let awaiting_visible = !input.awaiting_tasks.is_empty();
    let stopped_visible = !input.stopped_tasks.is_empty();
    let open_waits_count = input.open_waits_count.min(COMPOSER_OPEN_WAITS_MAX);
    let notice_visible = input.notice.is_some();
    let visible = queue_visible_rows > 0
        || reply_preview_visible
        || awaiting_visible
        || stopped_visible
        || open_waits_count > 0
        || notice_visible;

    let mut height_px = 0.0_f32;
    if queue_visible_rows > 0 {
        height_px += 26.0 + (queue_visible_rows as f32) * COMPOSER_STACK_ROW_HEIGHT_PX;
    }
    if reply_preview_visible {
        height_px += COMPOSER_REPLY_PREVIEW_MIN_HEIGHT_PX;
    }
    if awaiting_visible {
        height_px += 28.0 + (input.awaiting_tasks.len().min(COMPOSER_STACK_MAX_VISIBLE_ROWS) as f32) * COMPOSER_STACK_ROW_HEIGHT_PX;
    }
    if stopped_visible {
        height_px += 28.0 + (input.stopped_tasks.len().min(COMPOSER_STACK_MAX_VISIBLE_ROWS) as f32) * COMPOSER_STACK_ROW_HEIGHT_PX;
    }
    if open_waits_count > 0 {
        height_px += COMPOSER_STACK_ROW_HEIGHT_PX;
    }
    if notice_visible {
        height_px += 40.0;
    }

    ComposerStackLayoutSnapshot {
        visible,
        height_px,
        radius_px: COMPOSER_STACK_RADIUS_PX,
        overlap_bottom_px: if visible {
            COMPOSER_STACK_OVERLAP_BOTTOM_PX
        } else {
            0.0
        },
        z_index: COMPOSER_STACK_Z_INDEX,
        composer_z_index: COMPOSER_PILL_Z_INDEX,
        queue_row_height_px: COMPOSER_STACK_ROW_HEIGHT_PX,
        queue_items: input.queue_items.to_vec(),
        queue_visible_rows,
        queue_can_steer: input.queue_can_steer,
        reply_preview_visible,
        reply_preview_min_height_px: COMPOSER_REPLY_PREVIEW_MIN_HEIGHT_PX,
        reply_preview_sender: input.reply_sender.map(str::to_string),
        awaiting_visible,
        awaiting_title: "Subtasks",
        awaiting_tasks: input.awaiting_tasks.to_vec(),
        awaiting_dismiss_available: awaiting_visible && input.awaiting_all_settled,
        stopped_visible,
        stopped_tasks: input.stopped_tasks.to_vec(),
        open_waits_count,
        notice_visible,
        notice_has_copy_details: input.notice.is_some_and(ComposerNoticeState::has_copy_details),
        notice_truncated: input.notice.is_some_and(ComposerNoticeState::is_truncated),
    }
}
