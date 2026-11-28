//! Virtualized 1-on-1 OpenBot conversation timeline, paced token streaming (`40ms`/`300ms`/`400ms`),
//! UTF-16 surrogate-safe `>20,000`-char message collapse (`<=3,000` preview), `ChatRowBoundary`
//! corrupt-row isolation, `VoicePlayer` (`40`-bar waveform canvas & speed cycle), all `8` rich
//! content renderers, extension-toned `@file` `ReferenceChip`, `ChatVisual` sandbox frame,
//! 1:1 `AgentExchangeSummary`, reactions, human-protected pins, `Cmd+F` + `ChatScrollRail` minimap,
//! and procedural `ChatBackdrop` (`OB-F15`).

use std::collections::BTreeSet;
use std::fmt;
use std::panic::{AssertUnwindSafe, catch_unwind};

use chrono::{DateTime, Utc};
use gpui::prelude::FluentBuilder;
use gpui::*;
use openbot_contracts::avatar::{AvatarHue, AvatarMood};
use openbot_contracts::conversation::{
    AgentExchangeDirection, AgentExchangeSummary, ConversationMessage, ConversationMessageAuthor,
    ConversationMessageStatus, ConversationReaction, ConversationReactionActor,
    MESSAGE_PINS_LIMIT, MESSAGE_REACTIONS, MORE_MESSAGE_REACTIONS, MessagePinner, PinnedMessage,
    is_message_reaction,
};
use openbot_contracts::providers::{AgentProviderId, AgentProviderRef};
use openbot_contracts::voice::{SPEECH_SPEEDS as CONTRACT_SPEECH_SPEEDS, next_speech_speed};
use serde::{Deserialize, Serialize};

use crate::components::bloub_avatar::{AgentAvatarProps, render_agent_avatar};
use crate::theme::openbot_palette::{
    ChatBackdropScene, OpenBotPalette, file_reference_badge, file_reference_tone,
    parse_css_color_to_rgba, readable_chat_background, render_chat_backdrop,
    solid_bubble_text_rgb,
};
use crate::theme::{IconName, tg_icon_rgba};

use super::ask_ui::{
    RuntimeTurnCard, UiBlockCardState, render_runtime_turn_card, render_ui_block_card,
};
use super::steps_and_trace::{
    AgentStepsState, SubagentDockState, TraceWaterfallState, render_agent_steps,
    render_subagent_dock, render_trace_waterfall_dialog,
};

/// Default streaming reveal step interval (`--stream-gap: 40ms`).
pub const STREAM_GAP_MS: u32 = 40;
/// Lower bound for streaming step interval (`>= 16ms`).
pub const STREAM_MIN_GAP_MS: u32 = 16;
/// Trailing token fade-in duration (`--stream-fade: 300ms`).
pub const STREAM_FADE_MS: u32 = 300;
/// Streaming catch-up window (`--stream-catch-up: 400ms`).
pub const STREAM_CATCH_UP_MS: u32 = 400;
/// Completed messages strictly longer than `20,000` UTF-16 code units auto-collapse.
pub const LONG_MESSAGE_CHARACTERS: usize = 20_000;
/// Collapsed preview cap (`<= 3,000` UTF-16 code units, surrogate-safe).
pub const MESSAGE_PREVIEW_CHARACTERS: usize = 3_000;
/// Minimum newline cut index (`> 1,500` UTF-16 units) when snapping collapsed previews.
pub const MESSAGE_PREVIEW_NEWLINE_FLOOR: usize = 1_500;
/// Consecutive-sender grouping window (`5 min = 300,000ms`).
pub const CHAT_GROUPING_WINDOW_MS: i64 = 300_000;
/// Time-gap separator threshold (`1 hr = 3,600,000ms`).
pub const CHAT_TIME_GAP_MS: i64 = 3_600_000;
/// Minimum conversation pane width (`CONVERSATION_MIN_WIDTH = 424px`).
pub const CONVERSATION_MIN_WIDTH_PX: f32 = 424.0;
/// Waveform bar count rendered inside [`VoicePlayerState`].
pub const VOICE_PLAYER_BAR_COUNT: usize = 40;
/// Keyboard/button seek increment for [`VoicePlayerState`] in seconds.
pub const VOICE_PLAYER_SEEK_STEP_SEC: f64 = 0.5;
/// Supported TTS / VoicePlayer playback speeds (`[0.75, 1.0, 1.25, 1.5, 2.0]`).
pub const SPEECH_SPEEDS: [f64; 5] = CONTRACT_SPEECH_SPEEDS;
/// Canonical 8 rich content renderer identifiers exposed by `OB-F15`.
pub const RICH_RENDERER_KINDS: [&str; 8] = [
    "markdown",
    "katex",
    "mermaid",
    "data_table",
    "comparison_table",
    "code_block",
    "lightbox",
    "voice_player",
];

fn pal_rgba(palette: &OpenBotPalette, token: &str, fallback_hex: u32) -> Rgba {
    palette.rgba(token).unwrap_or_else(|| rgb(fallback_hex))
}

/// Canonical paced streaming timing configuration (`40ms` step, `300ms` fade, `400ms` catch-up).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct StreamTimings {
    pub step_ms: u32,
    pub fade_ms: u32,
    pub catch_up_ms: u32,
}

impl Default for StreamTimings {
    fn default() -> Self {
        Self {
            step_ms: STREAM_GAP_MS,
            fade_ms: STREAM_FADE_MS,
            catch_up_ms: STREAM_CATCH_UP_MS,
        }
    }
}

/// Paced incremental token-reveal controller (`MessageRendering.tsx:L113-L255`).
///
/// Snaps immediately on `reduced_motion == true` or any non-prefix replacement edit.
#[derive(Debug, Clone, PartialEq)]
pub struct StreamingBodyPacer {
    pub target_text: String,
    pub committed_text: String,
    pub trailing_chunk: String,
    pub trailing_alpha: f32,
    pub step_ms: u32,
    pub fade_ms: u32,
    pub catch_up_ms: u32,
    pub reduced_motion: bool,
    pub elapsed_in_step_ms: u32,
    pub settle_remaining_ms: u32,
}

impl Default for StreamingBodyPacer {
    fn default() -> Self {
        Self::new(false)
    }
}

impl StreamingBodyPacer {
    pub fn new(reduced_motion: bool) -> Self {
        Self {
            target_text: String::new(),
            committed_text: String::new(),
            trailing_chunk: String::new(),
            trailing_alpha: 1.0,
            step_ms: STREAM_GAP_MS.max(STREAM_MIN_GAP_MS),
            fade_ms: STREAM_FADE_MS,
            catch_up_ms: STREAM_CATCH_UP_MS,
            reduced_motion,
            elapsed_in_step_ms: 0,
            settle_remaining_ms: 0,
        }
    }

    pub fn timings(&self) -> StreamTimings {
        StreamTimings {
            step_ms: self.step_ms.max(STREAM_MIN_GAP_MS),
            fade_ms: self.fade_ms,
            catch_up_ms: self.catch_up_ms,
        }
    }

    /// Updates target body text; snaps instantaneously on reduced motion or non-prefix edit.
    pub fn push_target(&mut self, next_text: &str) {
        let already_shown_prefix = format!("{}{}", self.committed_text, self.trailing_chunk);
        if self.reduced_motion || !next_text.starts_with(&already_shown_prefix) {
            self.target_text = next_text.to_string();
            self.committed_text = next_text.to_string();
            self.trailing_chunk.clear();
            self.trailing_alpha = 1.0;
            self.elapsed_in_step_ms = 0;
            self.settle_remaining_ms = 0;
            return;
        }
        self.target_text = next_text.to_string();
    }

    /// Advances incremental reveal by `delta_ms` while preserving UTF-8 character boundaries.
    pub fn tick(&mut self, delta_ms: u32) {
        if self.reduced_motion {
            self.finish_immediately();
            return;
        }
        let step_ms = self.step_ms.max(STREAM_MIN_GAP_MS);
        let fade_ms = self.fade_ms.max(1);
        if !self.trailing_chunk.is_empty() {
            self.trailing_alpha =
                (self.trailing_alpha + (delta_ms as f32 / fade_ms as f32)).min(1.0);
        }
        self.elapsed_in_step_ms = self.elapsed_in_step_ms.saturating_add(delta_ms);
        if self.elapsed_in_step_ms < step_ms {
            self.settle_remaining_ms = self.settle_remaining_ms.saturating_sub(delta_ms);
            return;
        }
        self.elapsed_in_step_ms %= step_ms;

        if !self.trailing_chunk.is_empty() {
            self.committed_text.push_str(&self.trailing_chunk);
            self.trailing_chunk.clear();
        }
        let shown_len = self.committed_text.len();
        if shown_len >= self.target_text.len() {
            self.trailing_alpha = 1.0;
            self.settle_remaining_ms = self.settle_remaining_ms.saturating_sub(delta_ms);
            return;
        }
        let remaining = &self.target_text[shown_len..];
        let steps_in_window = (self.catch_up_ms / step_ms).max(1) as usize;
        let remaining_chars = remaining.chars().count();
        let take_chars = remaining_chars.div_ceil(steps_in_window).max(1);
        let cut_byte = remaining
            .char_indices()
            .nth(take_chars)
            .map(|(b, _)| b)
            .unwrap_or(remaining.len());
        self.trailing_chunk = remaining[..cut_byte].to_string();
        self.trailing_alpha = 0.0;
        self.settle_remaining_ms = (step_ms.saturating_mul(2)).max(self.fade_ms);
    }

    pub fn finish_immediately(&mut self) {
        self.committed_text = self.target_text.clone();
        self.trailing_chunk.clear();
        self.trailing_alpha = 1.0;
        self.elapsed_in_step_ms = 0;
        self.settle_remaining_ms = 0;
    }

    pub fn visible_text(&self) -> String {
        format!("{}{}", self.committed_text, self.trailing_chunk)
    }

    pub fn is_settled(&self) -> bool {
        self.committed_text.len() + self.trailing_chunk.len() >= self.target_text.len()
            && self.settle_remaining_ms == 0
    }
}

/// Result of evaluating long-message auto-collapse (`>20,000` UTF-16 code units -> `<=3,000` preview).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MessagePreviewResult {
    pub collapsed: bool,
    pub preview_text: String,
    pub preview_utf16_len: usize,
    pub full_utf16_len: usize,
    pub surrogate_valid: bool,
    pub show_more_available: bool,
}

fn verify_utf16_surrogate_pairs(s: &str) -> bool {
    let mut iter = s.encode_utf16().peekable();
    while let Some(u) = iter.next() {
        if (0xD800..=0xDBFF).contains(&u) {
            match iter.next() {
                Some(lo) if (0xDC00..=0xDFFF).contains(&lo) => {}
                _ => return false,
            }
        } else if (0xDC00..=0xDFFF).contains(&u) {
            return false;
        }
    }
    true
}

/// Computes UTF-16 surrogate-safe message preview collapse (`MessageRendering.tsx:L86-L96`).
///
/// - Collapses iff `!streaming && !expanded && utf16_len(text) > 20_000`.
/// - Caps preview strictly `<= 3_000` UTF-16 code units on a valid Rust `char` boundary,
///   guaranteeing surrogate pairs (e.g. `😀` straddling index `2999..3000`) are NEVER split.
/// - Snaps back to the last `\n` when its UTF-16 offset exceeds `1_500`.
pub fn compute_message_preview(text: &str, streaming: bool, expanded: bool) -> MessagePreviewResult {
    let full_utf16_len: usize = text.chars().map(char::len_utf16).sum();
    let over_threshold = full_utf16_len > LONG_MESSAGE_CHARACTERS;
    if streaming || expanded || !over_threshold {
        return MessagePreviewResult {
            collapsed: false,
            preview_text: text.to_string(),
            preview_utf16_len: full_utf16_len,
            full_utf16_len,
            surrogate_valid: verify_utf16_surrogate_pairs(text),
            show_more_available: !streaming && over_threshold,
        };
    }

    let mut u16_acc = 0usize;
    let mut cut_byte = 0usize;
    let mut last_newline: Option<(usize, usize)> = None;
    for (byte_idx, ch) in text.char_indices() {
        let w = ch.len_utf16();
        if u16_acc + w > MESSAGE_PREVIEW_CHARACTERS {
            break;
        }
        if ch == '\n' {
            last_newline = Some((byte_idx, u16_acc));
        }
        u16_acc += w;
        cut_byte = byte_idx + ch.len_utf8();
    }

    let (final_byte, final_u16) = match last_newline {
        Some((nl_byte, nl_u16)) if nl_u16 > MESSAGE_PREVIEW_NEWLINE_FLOOR => (nl_byte, nl_u16),
        _ => (cut_byte, u16_acc),
    };

    let preview_text = text[..final_byte].to_string();
    let surrogate_valid = verify_utf16_surrogate_pairs(&preview_text);
    MessagePreviewResult {
        collapsed: true,
        preview_text,
        preview_utf16_len: final_u16,
        full_utf16_len,
        surrogate_valid,
        show_more_available: true,
    }
}

/// Alias of [`compute_message_preview`] matching `SCOPE.md` naming.
pub fn message_preview_surrogate_safe(
    text: &str,
    streaming: bool,
    expanded: bool,
) -> MessagePreviewResult {
    compute_message_preview(text, streaming, expanded)
}

/// Summary produced by [`ChatRowBoundary`] when isolating corrupt timeline rows from siblings.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChatRowRenderResult {
    pub rendered_rows: usize,
    pub isolated_corrupt_rows: usize,
    pub sibling_rows_rendered: usize,
    pub row_boundary_isolated: bool,
    pub isolated_row_ids: Vec<String>,
}

/// Per-row error boundary isolating corrupt message payloads so sibling timeline rows render cleanly.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChatRowBoundary {
    pub row_id: String,
    pub corrupt: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error_reason: Option<String>,
}

impl ChatRowBoundary {
    /// Inspects a single [`ChatTimelineRow`] inside `catch_unwind` so malformed payloads never crash siblings.
    pub fn check_row(row: &ChatTimelineRow) -> Self {
        let row_id = row.message.id.clone();
        if let Some(ref reason) = row.corrupt_override {
            return Self {
                row_id,
                corrupt: true,
                error_reason: Some(reason.clone()),
            };
        }
        let guarded = catch_unwind(AssertUnwindSafe(|| {
            if !row.message.is_valid() {
                return Err("message failed contract invariant check".to_string());
            }
            if row.message.text.contains('\0') {
                return Err("corrupt NUL byte in message payload".to_string());
            }
            Ok(())
        }));
        match guarded {
            Ok(Ok(())) => Self {
                row_id,
                corrupt: false,
                error_reason: None,
            },
            Ok(Err(reason)) => Self {
                row_id,
                corrupt: true,
                error_reason: Some(reason),
            },
            Err(_) => Self {
                row_id,
                corrupt: true,
                error_reason: Some("row renderer panicked and was isolated by ChatRowBoundary".to_string()),
            },
        }
    }

    /// Evaluates a slice of rows, optionally injecting a synthetic corrupt row for fault-isolation checks.
    pub fn evaluate_rows(
        rows: &[ChatTimelineRow],
        inject_corrupt_row: bool,
    ) -> ChatRowRenderResult {
        let mut healthy = 0usize;
        let mut corrupt = 0usize;
        let mut isolated_ids = Vec::new();
        for row in rows {
            let b = Self::check_row(row);
            if b.corrupt {
                corrupt += 1;
                isolated_ids.push(b.row_id);
            } else {
                healthy += 1;
            }
        }
        if inject_corrupt_row {
            let synth = ChatTimelineRow {
                message: ConversationMessage::new_simple(
                    "corrupt-injected-row",
                    ConversationMessageAuthor::Assistant,
                    "\0corrupt-payload",
                    "2026-11-26T00:00:00Z",
                    ConversationMessageStatus::Completed,
                ),
                timestamp_ms: 0,
                corrupt_override: Some("injected corrupt row payload".to_string()),
                steps: None,
                ui_block_card: None,
                runtime_cards: Vec::new(),
                voice_player: None,
            };
            let b = Self::check_row(&synth);
            if b.corrupt {
                corrupt += 1;
                isolated_ids.push(b.row_id);
            }
        }
        ChatRowRenderResult {
            rendered_rows: healthy + corrupt,
            isolated_corrupt_rows: corrupt,
            sibling_rows_rendered: healthy,
            row_boundary_isolated: corrupt > 0,
            isolated_row_ids: isolated_ids,
        }
    }
}

/// Renders the compact isolated fallback card when [`ChatRowBoundary`] catches a corrupt row.
pub fn render_isolated_corrupt_row(row_id: &str, reason: &str, palette: &OpenBotPalette) -> Div {
    let danger = pal_rgba(palette, "--openbot-danger", 0xFD2F3B);
    let danger_soft = pal_rgba(palette, "--openbot-danger-soft", 0x4A1B20);
    let text_primary = pal_rgba(palette, "--openbot-text-primary", 0xFFFFFF);
    let text_muted = pal_rgba(palette, "--openbot-text-muted", 0x979797);
    div()
        .w_full()
        .max_w(px(480.0))
        .px(px(12.0))
        .py(px(8.0))
        .rounded(px(12.0))
        .bg(danger_soft)
        .border_1()
        .border_color(danger)
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
                        .text_size(px(12.0))
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(text_primary)
                        .child(format!("Row isolated ({row_id})")),
                )
                .child(
                    div()
                        .text_size(px(11.0))
                        .text_color(text_muted)
                        .child(reason.to_string()),
                ),
        )
        .child(
            div()
                .px(px(8.0))
                .py(px(3.0))
                .rounded(px(6.0))
                .bg(danger)
                .text_size(px(10.0))
                .font_weight(FontWeight::MEDIUM)
                .text_color(rgb(0xFFFFFF))
                .child("Isolated"),
        )
}

fn paint_rect_path(window: &mut Window, x0: f32, y0: f32, x1: f32, y1: f32, color: Hsla) {
    if x1 <= x0 || y1 <= y0 {
        return;
    }
    let mut b = PathBuilder::fill();
    b.move_to(point(px(x0), px(y0)));
    b.line_to(point(px(x1), px(y0)));
    b.line_to(point(px(x1), px(y1)));
    b.line_to(point(px(x0), px(y1)));
    b.close();
    if let Ok(path) = b.build() {
        window.paint_path(path, color);
    }
}

/// Interactive audio/TTS waveform player (`40` bars, `0.5s` seek step, cyclic `SPEECH_SPEEDS`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct VoicePlayerState {
    pub track_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    pub waveform_bars: Vec<f32>,
    pub duration_sec: f64,
    pub position_sec: f64,
    pub seek_step_sec: f64,
    pub speed: f64,
    pub playing: bool,
}

impl VoicePlayerState {
    pub fn new(track_id: impl Into<String>, duration_sec: f64) -> Self {
        let id = track_id.into();
        let seed: u64 = id
            .bytes()
            .fold(0x9E37_79B9_7F4A_7C15u64, |acc, b| acc.wrapping_mul(31).wrapping_add(u64::from(b)));
        let mut bars = Vec::with_capacity(VOICE_PLAYER_BAR_COUNT);
        for i in 0..VOICE_PLAYER_BAR_COUNT {
            let phase = ((i as f64) * 0.42 + ((seed % 17) as f64) * 0.13).sin().abs();
            let env = 0.22 + 0.72 * phase * (0.65 + 0.35 * ((i as f64) * 0.19).cos().abs());
            bars.push((env as f32).clamp(0.12, 1.0));
        }
        let safe_duration = if duration_sec.is_finite() && duration_sec > 0.0 {
            duration_sec.max(0.1)
        } else {
            0.1
        };
        Self {
            track_id: id,
            title: None,
            waveform_bars: bars,
            duration_sec: safe_duration,
            position_sec: 0.0,
            seek_step_sec: VOICE_PLAYER_SEEK_STEP_SEC,
            speed: 1.0,
            playing: false,
        }
    }

    pub fn bar_count(&self) -> usize {
        if self.waveform_bars.is_empty() {
            VOICE_PLAYER_BAR_COUNT
        } else {
            self.waveform_bars.len()
        }
    }

    /// Peeks at the next speed in the cyclic `[1.0 -> 1.25 -> 1.5 -> 2.0 -> 0.75 -> 1.0]` ring.
    pub fn next_speed(&self) -> f64 {
        next_speech_speed(self.speed)
    }

    /// Advances playback speed to the next value in `SPEECH_SPEEDS` and returns it.
    pub fn cycle_speed(&mut self) -> f64 {
        self.speed = next_speech_speed(self.speed);
        self.speed
    }

    pub fn seek_forward(&mut self) {
        let dur = if self.duration_sec.is_finite() {
            self.duration_sec.max(0.1)
        } else {
            0.1
        };
        let pos = if self.position_sec.is_finite() {
            self.position_sec
        } else {
            0.0
        };
        self.position_sec = (pos + self.seek_step_sec).min(dur);
    }

    pub fn seek_backward(&mut self) {
        let pos = if self.position_sec.is_finite() {
            self.position_sec
        } else {
            0.0
        };
        self.position_sec = (pos - self.seek_step_sec).max(0.0);
    }

    pub fn seek_to_fraction(&mut self, frac: f64) {
        if !frac.is_finite() {
            return;
        }
        let dur = if self.duration_sec.is_finite() && self.duration_sec > 0.0 {
            self.duration_sec
        } else {
            0.1
        };
        self.position_sec = (frac.clamp(0.0, 1.0)) * dur;
    }

    pub fn progress_fraction(&self) -> f32 {
        if !self.position_sec.is_finite() || !self.duration_sec.is_finite() || self.duration_sec <= 0.0 {
            return 0.0;
        }
        ((self.position_sec / self.duration_sec.max(0.001)).clamp(0.0, 1.0)) as f32
    }

    fn format_clock(sec: f64) -> String {
        let total = if sec.is_finite() {
            sec.max(0.0).round() as u64
        } else {
            0
        };
        format!("{}:{:02}", total / 60, total % 60)
    }

    /// Renders the 40-bar GPUI `canvas(...)` waveform scrubber, play/pause toggle, clock, and speed pill.
    pub fn render(&self, palette: &OpenBotPalette) -> Div {
        let raised = pal_rgba(palette, "--openbot-bg-surface-raised", 0x242424);
        let border = pal_rgba(palette, "--openbot-border-strong", 0x333335);
        let text_primary = pal_rgba(palette, "--openbot-text-primary", 0xFFFFFF);
        let text_muted = pal_rgba(palette, "--openbot-text-muted", 0x979797);
        let accent = pal_rgba(palette, "--openbot-accent", 0x007CF7);
        let on_accent = pal_rgba(palette, "--openbot-text-on-accent", 0xFFFFFF);

        let bars = self.waveform_bars.clone();
        let progress = self.progress_fraction();
        let active_hsla: Hsla = accent.into();
        let idle_hsla: Hsla = text_muted.into();
        let speed_label = if (self.speed.fract()).abs() < 1e-6 {
            format!("{}x", self.speed as i64)
        } else {
            format!("{}x", self.speed)
        };

        div()
            .w_full()
            .max_w(px(420.0))
            .px(px(10.0))
            .py(px(8.0))
            .rounded(px(12.0))
            .bg(raised)
            .border_1()
            .border_color(border)
            .flex()
            .items_center()
            .gap(px(10.0))
            .child(
                div()
                    .w(px(30.0))
                    .h(px(30.0))
                    .rounded_full()
                    .bg(accent)
                    .flex_shrink_0()
                    .flex()
                    .items_center()
                    .justify_center()
                    .cursor_pointer()
                    .child(tg_icon_rgba(
                        if self.playing { IconName::Pause } else { IconName::Play },
                        13.0,
                        on_accent,
                    )),
            )
            .child(
                div().flex_1().h(px(28.0)).child(
                    canvas(
                        |_, _, _| {},
                        move |bounds, _, window, _cx| {
                            let bx = f32::from(bounds.origin.x);
                            let by = f32::from(bounds.origin.y);
                            let bw = f32::from(bounds.size.width).max(1.0);
                            let bh = f32::from(bounds.size.height).max(1.0);
                            let count = bars.len().max(1);
                            let step = bw / (count as f32);
                            let bar_w = (step * 0.58).max(1.5);
                            for (i, amp) in bars.iter().enumerate() {
                                let frac = (i as f32 + 0.5) / (count as f32);
                                let h = (amp.clamp(0.1, 1.0) * bh).max(3.0);
                                let x0 = bx + (i as f32) * step + (step - bar_w) * 0.5;
                                let y0 = by + (bh - h) * 0.5;
                                let col = if frac <= progress {
                                    active_hsla
                                } else {
                                    idle_hsla.opacity(0.45)
                                };
                                paint_rect_path(window, x0, y0, x0 + bar_w, y0 + h, col);
                            }
                        },
                    )
                    .size_full(),
                ),
            )
            .child(
                div()
                    .text_size(px(11.0))
                    .text_color(text_muted)
                    .flex_shrink_0()
                    .child(format!(
                        "{} / {}",
                        Self::format_clock(self.position_sec),
                        Self::format_clock(self.duration_sec)
                    )),
            )
            .child(
                div()
                    .h(px(24.0))
                    .px(px(7.0))
                    .rounded(px(6.0))
                    .bg(pal_rgba(palette, "--openbot-bg-control", 0x2A2A2A))
                    .flex()
                    .items_center()
                    .cursor_pointer()
                    .text_size(px(11.0))
                    .font_weight(FontWeight::MEDIUM)
                    .text_color(text_primary)
                    .child(speed_label),
            )
    }
}

/// All 8 canonical rich content renderer kinds supported inside conversation bubbles.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RichBlockKind {
    Markdown,
    Katex,
    Mermaid,
    DataTable,
    ComparisonTable,
    CodeBlock,
    Lightbox,
    VoicePlayer,
}

impl RichBlockKind {
    pub const ALL: [Self; 8] = [
        Self::Markdown,
        Self::Katex,
        Self::Mermaid,
        Self::DataTable,
        Self::ComparisonTable,
        Self::CodeBlock,
        Self::Lightbox,
        Self::VoicePlayer,
    ];

    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Markdown => "markdown",
            Self::Katex => "katex",
            Self::Mermaid => "mermaid",
            Self::DataTable => "data_table",
            Self::ComparisonTable => "comparison_table",
            Self::CodeBlock => "code_block",
            Self::Lightbox => "lightbox",
            Self::VoicePlayer => "voice_player",
        }
    }
}

/// Extension-toned inline `@file` reference chip powered by M2 `file_reference_tone` & `file_reference_badge`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FileReferenceChip {
    pub file_name: String,
    pub path: String,
}

impl FileReferenceChip {
    pub fn render(&self, palette: &OpenBotPalette) -> Div {
        let tone = file_reference_tone(&self.file_name);
        let (fg_css, bg_css) = tone.colors(palette.scheme);
        let fg = parse_css_color_to_rgba(fg_css).unwrap_or_else(|| rgb(0x74B9FF));
        let bg = parse_css_color_to_rgba(bg_css).unwrap_or_else(|| rgba(0x74B9FF24));
        let badge = file_reference_badge(&self.file_name);

        div()
            .px(px(7.0))
            .py(px(2.0))
            .rounded(px(6.0))
            .bg(bg)
            .flex()
            .items_center()
            .gap(px(5.0))
            .cursor_pointer()
            .when_some(badge, |el, ext| {
                el.child(
                    div()
                        .text_size(px(9.0))
                        .font_weight(FontWeight::BOLD)
                        .text_color(fg)
                        .child(ext),
                )
            })
            .child(
                div()
                    .text_size(px(12.0))
                    .font_weight(FontWeight::MEDIUM)
                    .text_color(fg)
                    .child(format!("@{}", self.file_name)),
            )
    }
}

/// Parsed rich block rendered inside an OpenBot message bubble.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum RichContentBlock {
    Markdown {
        text: String,
        reference_chips: Vec<FileReferenceChip>,
    },
    Katex {
        formula: String,
        display_mode: bool,
    },
    Mermaid {
        diagram_source: String,
        zoom: f32,
        pan_x: f32,
        pan_y: f32,
    },
    DataTable {
        headers: Vec<String>,
        rows: Vec<Vec<String>>,
        sort_column: Option<usize>,
        sort_asc: bool,
    },
    ComparisonTable {
        title: String,
        subjects: Vec<String>,
        rows: Vec<(String, Vec<String>)>,
    },
    CodeBlock {
        language: String,
        code: String,
        line_wrap: bool,
        html_live_preview: bool,
    },
    Lightbox {
        caption: Option<String>,
        src_url: String,
        width: u32,
        height: u32,
        open: bool,
    },
    VoicePlayer(VoicePlayerState),
    ChatVisual {
        title: String,
        html_snippet: String,
        height_px: u32,
        sandbox_csp: String,
    },
}

fn extract_inline_file_chips(text: &str) -> Vec<FileReferenceChip> {
    let mut chips = Vec::new();
    for tok in text.split_whitespace() {
        if let Some(rest) = tok.strip_prefix('@') {
            let cleaned = rest.trim_matches(|c: char| matches!(c, ',' | '.' | ';' | ':' | ')' | '(' | '`' | '"' | '\''));
            if cleaned.contains('.') && cleaned.len() >= 3 && cleaned.len() <= 128 {
                let file_name = cleaned.rsplit('/').next().unwrap_or(cleaned).to_string();
                chips.push(FileReferenceChip {
                    file_name,
                    path: cleaned.to_string(),
                });
            }
        }
    }
    chips
}

/// Parses fenced `mermaid` / code blocks, `$$...$$` KaTeX display formulas, pipe tables,
/// and markdown paragraphs with inline `@file` reference chips.
pub fn parse_rich_blocks(markdown_text: &str) -> Vec<RichContentBlock> {
    let mut blocks = Vec::new();
    let lines: Vec<&str> = markdown_text.lines().collect();
    let mut i = 0usize;
    let mut para_buf: Vec<&str> = Vec::new();

    let flush_para = |buf: &mut Vec<&str>, out: &mut Vec<RichContentBlock>| {
        if buf.is_empty() {
            return;
        }
        let joined = buf.join("\n");
        let chips = extract_inline_file_chips(&joined);
        out.push(RichContentBlock::Markdown {
            text: joined,
            reference_chips: chips,
        });
        buf.clear();
    };

    while i < lines.len() {
        let trimmed = lines[i].trim();
        if let Some(lang_raw) = trimmed.strip_prefix("```") {
            flush_para(&mut para_buf, &mut blocks);
            let lang = lang_raw.trim().to_ascii_lowercase();
            i += 1;
            let mut body = Vec::new();
            while i < lines.len() && !lines[i].trim().starts_with("```") {
                body.push(lines[i]);
                i += 1;
            }
            if i < lines.len() {
                i += 1;
            }
            let content = body.join("\n");
            if lang == "mermaid" {
                blocks.push(RichContentBlock::Mermaid {
                    diagram_source: content,
                    zoom: 1.0,
                    pan_x: 0.0,
                    pan_y: 0.0,
                });
            } else {
                let is_html = matches!(lang.as_str(), "html" | "svg");
                blocks.push(RichContentBlock::CodeBlock {
                    language: if lang.is_empty() { "text".to_string() } else { lang },
                    code: content,
                    line_wrap: false,
                    html_live_preview: is_html,
                });
            }
            continue;
        }
        if trimmed == "$$" || (trimmed.starts_with("$$") && trimmed.ends_with("$$") && trimmed.len() > 4) {
            flush_para(&mut para_buf, &mut blocks);
            let formula = if trimmed == "$$" {
                i += 1;
                let mut f_lines = Vec::new();
                while i < lines.len() && lines[i].trim() != "$$" {
                    f_lines.push(lines[i]);
                    i += 1;
                }
                if i < lines.len() {
                    i += 1;
                }
                f_lines.join("\n")
            } else {
                i += 1;
                trimmed[2..trimmed.len() - 2].trim().to_string()
            };
            blocks.push(RichContentBlock::Katex {
                formula,
                display_mode: true,
            });
            continue;
        }
        if trimmed.starts_with('|')
            && trimmed.ends_with('|')
            && i + 1 < lines.len()
            && lines[i + 1].trim().starts_with('|')
            && lines[i + 1].contains("---")
        {
            flush_para(&mut para_buf, &mut blocks);
            let headers: Vec<String> = trimmed
                .trim_matches('|')
                .split('|')
                .map(|c| c.trim().to_string())
                .collect();
            i += 2;
            let mut rows = Vec::new();
            while i < lines.len() {
                let r = lines[i].trim();
                if !r.starts_with('|') {
                    break;
                }
                rows.push(
                    r.trim_matches('|')
                        .split('|')
                        .map(|c| c.trim().to_string())
                        .collect(),
                );
                i += 1;
            }
            blocks.push(RichContentBlock::DataTable {
                headers,
                rows,
                sort_column: None,
                sort_asc: true,
            });
            continue;
        }
        para_buf.push(lines[i]);
        i += 1;
    }
    flush_para(&mut para_buf, &mut blocks);
    blocks
}

/// Renders any [`RichContentBlock`] (all 8 rich renderers + sandboxed `ChatVisual` preview frame).
pub fn render_rich_block(block: &RichContentBlock, palette: &OpenBotPalette) -> Div {
    let surface = pal_rgba(palette, "--openbot-bg-surface", 0x212121);
    let raised = pal_rgba(palette, "--openbot-bg-surface-raised", 0x242424);
    let border = pal_rgba(palette, "--openbot-border-strong", 0x333335);
    let text_primary = pal_rgba(palette, "--openbot-text-primary", 0xFFFFFF);
    let text_secondary = pal_rgba(palette, "--openbot-text-secondary", 0xDCDCDC);
    let text_muted = pal_rgba(palette, "--openbot-text-muted", 0x979797);
    let accent = pal_rgba(palette, "--openbot-accent", 0x007CF7);
    let accent_soft = pal_rgba(palette, "--openbot-accent-soft", 0x1A365D);

    match block {
        RichContentBlock::Markdown {
            text,
            reference_chips,
        } => {
            let mut wrap = div().flex().flex_col().gap(px(6.0)).child(
                div()
                    .text_size(px(14.0))
                    .line_height(relative(1.45))
                    .text_color(text_primary)
                    .child(text.clone()),
            );
            if !reference_chips.is_empty() {
                let mut chip_row = div().flex().flex_wrap().gap(px(5.0));
                for chip in reference_chips {
                    chip_row = chip_row.child(chip.render(palette));
                }
                wrap = wrap.child(chip_row);
            }
            wrap
        }
        RichContentBlock::Katex {
            formula,
            display_mode,
        } => div()
            .px(px(12.0))
            .py(px(if *display_mode { 10.0 } else { 4.0 }))
            .rounded(px(10.0))
            .bg(raised)
            .border_1()
            .border_color(border)
            .flex()
            .items_center()
            .justify_between()
            .gap(px(8.0))
            .child(
                div()
                    .text_size(px(if *display_mode { 15.0 } else { 13.0 }))
                    .italic()
                    .text_color(text_primary)
                    .child(formula.clone()),
            )
            .child(div().text_size(px(10.0)).text_color(text_muted).child("KaTeX")),
        RichContentBlock::Mermaid {
            diagram_source,
            zoom,
            ..
        } => {
            let node_count = diagram_source
                .lines()
                .filter(|l| !l.trim().is_empty())
                .count();
            div()
                .rounded(px(12.0))
                .bg(surface)
                .border_1()
                .border_color(border)
                .p(px(10.0))
                .flex()
                .flex_col()
                .gap(px(6.0))
                .child(
                    div()
                        .flex()
                        .items_center()
                        .justify_between()
                        .child(
                            div()
                                .text_size(px(11.0))
                                .font_weight(FontWeight::SEMIBOLD)
                                .text_color(accent)
                                .child(format!("Mermaid Diagram · {node_count} statements")),
                        )
                        .child(
                            div()
                                .text_size(px(11.0))
                                .text_color(text_muted)
                                .child(format!("{}%", (zoom * 100.0).round() as u32)),
                        ),
                )
                .child(
                    div()
                        .p(px(8.0))
                        .rounded(px(8.0))
                        .bg(raised)
                        .text_size(px(12.0))
                        .text_color(text_secondary)
                        .child(diagram_source.clone()),
                )
        }
        RichContentBlock::DataTable {
            headers,
            rows,
            sort_column,
            sort_asc,
        } => {
            let mut tbl = div()
                .rounded(px(10.0))
                .bg(surface)
                .border_1()
                .border_color(border)
                .overflow_hidden()
                .flex()
                .flex_col();
            let mut hdr = div()
                .bg(raised)
                .px(px(10.0))
                .py(px(6.0))
                .flex()
                .gap(px(8.0));
            for (col_idx, h) in headers.iter().enumerate() {
                let arrow = if *sort_column == Some(col_idx) {
                    if *sort_asc { " ↑" } else { " ↓" }
                } else {
                    ""
                };
                hdr = hdr.child(
                    div()
                        .flex_1()
                        .text_size(px(12.0))
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(text_primary)
                        .child(format!("{h}{arrow}")),
                );
            }
            tbl = tbl.child(hdr);
            for row in rows {
                let mut r_el = div()
                    .px(px(10.0))
                    .py(px(5.0))
                    .border_t_1()
                    .border_color(border)
                    .flex()
                    .gap(px(8.0));
                for cell in row {
                    r_el = r_el.child(
                        div()
                            .flex_1()
                            .text_size(px(12.0))
                            .text_color(text_secondary)
                            .child(cell.clone()),
                    );
                }
                tbl = tbl.child(r_el);
            }
            tbl
        }
        RichContentBlock::ComparisonTable {
            title,
            subjects,
            rows,
        } => {
            let mut card = div()
                .rounded(px(12.0))
                .bg(surface)
                .border_1()
                .border_color(border)
                .p(px(10.0))
                .flex()
                .flex_col()
                .gap(px(6.0))
                .child(
                    div()
                        .text_size(px(13.0))
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(text_primary)
                        .child(title.clone()),
                );
            let mut hdr = div().flex().gap(px(8.0)).pb(px(4.0)).border_b_1().border_color(border);
            hdr = hdr.child(div().w(px(110.0)).text_size(px(11.0)).text_color(text_muted).child("Metric"));
            for subj in subjects {
                hdr = hdr.child(
                    div()
                        .flex_1()
                        .text_size(px(11.0))
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(accent)
                        .child(subj.clone()),
                );
            }
            card = card.child(hdr);
            for (metric, vals) in rows {
                let mut r_el = div().flex().gap(px(8.0)).py(px(3.0));
                r_el = r_el.child(div().w(px(110.0)).text_size(px(12.0)).text_color(text_muted).child(metric.clone()));
                for v in vals {
                    r_el = r_el.child(div().flex_1().text_size(px(12.0)).text_color(text_primary).child(v.clone()));
                }
                card = card.child(r_el);
            }
            card
        }
        RichContentBlock::CodeBlock {
            language,
            code,
            line_wrap,
            html_live_preview,
        } => div()
            .rounded(px(10.0))
            .bg(surface)
            .border_1()
            .border_color(border)
            .overflow_hidden()
            .flex()
            .flex_col()
            .child(
                div()
                    .px(px(10.0))
                    .py(px(5.0))
                    .bg(raised)
                    .flex()
                    .items_center()
                    .justify_between()
                    .child(
                        div()
                            .text_size(px(11.0))
                            .font_weight(FontWeight::MEDIUM)
                            .text_color(text_muted)
                            .child(language.clone()),
                    )
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(8.0))
                            .when(*html_live_preview, |el| {
                                el.child(div().text_size(px(11.0)).text_color(accent).child("Live HTML"))
                            })
                            .child(
                                div()
                                    .text_size(px(11.0))
                                    .text_color(text_muted)
                                    .child(if *line_wrap { "Wrap: On" } else { "Wrap: Off" }),
                            )
                            .child(tg_icon_rgba(IconName::Copy, 12.0, text_muted)),
                    ),
            )
            .child(
                div()
                    .p(px(10.0))
                    .text_size(px(12.0))
                    .line_height(relative(1.4))
                    .text_color(text_secondary)
                    .child(code.clone()),
            ),
        RichContentBlock::Lightbox {
            caption,
            src_url,
            width,
            height,
            open,
        } => div()
            .rounded(px(12.0))
            .bg(raised)
            .border_1()
            .border_color(if *open { accent } else { border })
            .p(px(10.0))
            .flex()
            .flex_col()
            .gap(px(6.0))
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(6.0))
                            .child(tg_icon_rgba(IconName::Camera, 13.0, accent))
                            .child(
                                div()
                                    .text_size(px(12.0))
                                    .font_weight(FontWeight::MEDIUM)
                                    .text_color(text_primary)
                                    .child(caption.clone().unwrap_or_else(|| src_url.clone())),
                            ),
                    )
                    .child(
                        div()
                            .text_size(px(11.0))
                            .text_color(text_muted)
                            .child(format!("{width} × {height}")),
                    ),
            ),
        RichContentBlock::VoicePlayer(vp) => vp.render(palette),
        RichContentBlock::ChatVisual {
            title,
            html_snippet,
            height_px,
            sandbox_csp,
        } => div()
            .rounded(px(12.0))
            .bg(surface)
            .border_1()
            .border_color(accent_soft)
            .overflow_hidden()
            .flex()
            .flex_col()
            .child(
                div()
                    .px(px(10.0))
                    .py(px(6.0))
                    .bg(raised)
                    .flex()
                    .items_center()
                    .justify_between()
                    .child(
                        div()
                            .text_size(px(12.0))
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(text_primary)
                            .child(format!("ChatVisual · {title}")),
                    )
                    .child(
                        div()
                            .text_size(px(10.0))
                            .text_color(accent)
                            .child(format!("CSP: {sandbox_csp} · {}px", height_px)),
                    ),
            )
            .child(
                div()
                    .min_h(px((*height_px).clamp(80, 320) as f32))
                    .p(px(10.0))
                    .text_size(px(12.0))
                    .text_color(text_secondary)
                    .child(html_snippet.clone()),
            ),
    }
}

/// Renders a 1:1 incoming/outgoing peer [`AgentExchangeSummary`] card inside the timeline.
pub fn render_agent_exchange_summary(
    exchange: &AgentExchangeSummary,
    palette: &OpenBotPalette,
) -> Div {
    let surface = pal_rgba(palette, "--openbot-bg-surface", 0x212121);
    let raised = pal_rgba(palette, "--openbot-bg-surface-raised", 0x242424);
    let border = pal_rgba(palette, "--openbot-border-strong", 0x333335);
    let text_primary = pal_rgba(palette, "--openbot-text-primary", 0xFFFFFF);
    let text_muted = pal_rgba(palette, "--openbot-text-muted", 0x979797);
    let accent = pal_rgba(palette, "--openbot-accent", 0x007CF7);

    let dir_label = match exchange.direction {
        AgentExchangeDirection::Incoming => "Incoming from agent",
        AgentExchangeDirection::Outgoing => "Sent to peer agents",
    };
    let mut card = div()
        .w_full()
        .max_w(px(460.0))
        .rounded(px(12.0))
        .bg(surface)
        .border_1()
        .border_color(border)
        .p(px(10.0))
        .flex()
        .flex_col()
        .gap(px(6.0))
        .child(
            div()
                .flex()
                .items_center()
                .justify_between()
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(6.0))
                        .child(tg_icon_rgba(IconName::Forward, 13.0, accent))
                        .child(
                            div()
                                .text_size(px(12.0))
                                .font_weight(FontWeight::SEMIBOLD)
                                .text_color(text_primary)
                                .child(dir_label),
                        ),
                )
                .child(
                    div()
                        .text_size(px(11.0))
                        .text_color(text_muted)
                        .child(format!("{} recipients", exchange.recipient_agent_ids.len())),
                ),
        );

    for del in &exchange.deliveries {
        let pos_txt = del
            .position
            .map(|p| format!(" #{p}"))
            .unwrap_or_default();
        card = card.child(
            div()
                .px(px(8.0))
                .py(px(4.0))
                .rounded(px(6.0))
                .bg(raised)
                .flex()
                .items_center()
                .justify_between()
                .child(
                    div()
                        .text_size(px(12.0))
                        .text_color(text_primary)
                        .child(del.recipient_agent_id.clone()),
                )
                .child(
                    div()
                        .text_size(px(11.0))
                        .text_color(text_muted)
                        .child(format!("{:?}{pos_txt}", del.status)),
                ),
        );
    }
    card
}

/// Thread tab displayed inside the 1-on-1 conversation header (`ThreadTabs`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ThreadTabItem {
    pub id: String,
    pub title: String,
    pub active: bool,
    pub unread_count: u32,
}

/// Active provider/model/effort selection shown in the 1-on-1 header `ProviderModelPicker`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProviderPickerSummary {
    pub provider: AgentProviderRef,
    pub model: String,
    pub reasoning_effort: String,
    pub fast_mode: bool,
    pub ultrafast: bool,
    pub next_turn_pending: bool,
}

impl Default for ProviderPickerSummary {
    fn default() -> Self {
        Self {
            provider: AgentProviderRef::Known(AgentProviderId::Codex),
            model: "gpt-6-luna".to_string(),
            reasoning_effort: "low".to_string(),
            fast_mode: false,
            ultrafast: false,
            next_turn_pending: false,
        }
    }
}

/// Single virtualized timeline row inside [`ConversationViewState`].
#[derive(Debug, Clone, PartialEq)]
pub struct ChatTimelineRow {
    pub message: ConversationMessage,
    pub timestamp_ms: i64,
    pub corrupt_override: Option<String>,
    pub steps: Option<AgentStepsState>,
    pub ui_block_card: Option<UiBlockCardState>,
    pub runtime_cards: Vec<RuntimeTurnCard>,
    pub voice_player: Option<VoicePlayerState>,
}

/// Computed grouping metadata for a single row (`chat-grouping.ts:L1-L28`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RowGroupingFlags {
    pub grouped_with_prev: bool,
    pub show_time_gap: bool,
    pub day_marker_label: Option<String>,
}

fn day_key_from_ms(ts_ms: i64) -> String {
    DateTime::<Utc>::from_timestamp_millis(ts_ms)
        .map(|dt| dt.format("%Y-%m-%d").to_string())
        .unwrap_or_else(|| "1970-01-01".to_string())
}

/// Computes 5-minute sender grouping (`CHAT_GROUPING_WINDOW_MS`), 1-hour time gaps
/// (`CHAT_TIME_GAP_MS`), and date divider pills across `rows`.
pub fn compute_row_grouping(
    rows: &[ChatTimelineRow],
    compact_actions: bool,
) -> Vec<RowGroupingFlags> {
    let mut out = Vec::with_capacity(rows.len());
    for (idx, row) in rows.iter().enumerate() {
        let cur_day = day_key_from_ms(row.timestamp_ms);
        if idx == 0 {
            out.push(RowGroupingFlags {
                grouped_with_prev: false,
                show_time_gap: false,
                day_marker_label: Some(cur_day),
            });
            continue;
        }
        let prev = &rows[idx - 1];
        let prev_day = day_key_from_ms(prev.timestamp_ms);
        let day_changed = cur_day != prev_day;
        let delta_ms = row.timestamp_ms.saturating_sub(prev.timestamp_ms);
        let same_sender = prev.message.author == row.message.author
            && prev.message.sender_agent_id == row.message.sender_agent_id;
        let same_turn = prev.message.turn_id.is_some() && prev.message.turn_id == row.message.turn_id;
        let grouped_with_prev =
            !day_changed && same_sender && (same_turn || (0..=CHAT_GROUPING_WINDOW_MS).contains(&delta_ms));
        let show_time_gap = !day_changed && compact_actions && delta_ms >= CHAT_TIME_GAP_MS;
        out.push(RowGroupingFlags {
            grouped_with_prev,
            show_time_gap,
            day_marker_label: day_changed.then_some(cur_day),
        });
    }
    out
}

/// In-chat `Cmd+F` search state driving match navigation and `ChatScrollRail` minimap ticks.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct InChatSearchState {
    pub open: bool,
    pub query: String,
    pub match_row_indices: Vec<usize>,
    pub current_match: usize,
}

/// Error returned when a 1:1 pin operation violates the 50-pin cap or human-pin immutability.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConversationPinError {
    pub code: String,
    pub message: String,
}

impl fmt::Display for ConversationPinError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}: {}", self.code, self.message)
    }
}

impl std::error::Error for ConversationPinError {}

/// Complete state controller for the 1-on-1 OpenBot virtualized conversation surface (`OB-F15`).
#[derive(Debug, Clone, PartialEq)]
pub struct ConversationViewState {
    pub agent_id: String,
    pub agent_name: String,
    pub agent_title: Option<String>,
    pub agent_avatar_seed: String,
    pub agent_avatar_hue: Option<AvatarHue>,
    pub agent_avatar_color: Option<String>,
    pub agent_mood: AvatarMood,
    pub workspace_locked: bool,
    pub threads: Vec<ThreadTabItem>,
    pub provider_picker: ProviderPickerSummary,
    pub side_panel_open: bool,
    pub rows: Vec<ChatTimelineRow>,
    pub streaming_pacer: StreamingBodyPacer,
    pub expanded_long_messages: BTreeSet<String>,
    pub pins: Vec<PinnedMessage>,
    pub pinned_popover_open: bool,
    pub unread_count: u64,
    pub first_unread_message_id: Option<String>,
    pub search: InChatSearchState,
    pub backdrop_scene: Option<ChatBackdropScene>,
    pub chat_color_hex: Option<String>,
    pub user_bubble_solid_hex: Option<String>,
    pub compact_actions: bool,
    pub reduced_motion: bool,
    pub active_subagent_dock: Option<SubagentDockState>,
    pub active_trace_dialog: Option<TraceWaterfallState>,
    pub reply_return_message_id: Option<String>,
    pub is_at_bottom: bool,
    pub show_more_reactions_for: Option<String>,
}

impl ConversationViewState {
    pub fn new(
        agent_id: impl Into<String>,
        agent_name: impl Into<String>,
        avatar_seed: impl Into<String>,
    ) -> Self {
        Self {
            agent_id: agent_id.into(),
            agent_name: agent_name.into(),
            agent_title: None,
            agent_avatar_seed: avatar_seed.into(),
            agent_avatar_hue: None,
            agent_avatar_color: None,
            agent_mood: AvatarMood::Idle,
            workspace_locked: false,
            threads: vec![ThreadTabItem {
                id: "main".to_string(),
                title: "Main".to_string(),
                active: true,
                unread_count: 0,
            }],
            provider_picker: ProviderPickerSummary::default(),
            side_panel_open: false,
            rows: Vec::new(),
            streaming_pacer: StreamingBodyPacer::new(false),
            expanded_long_messages: BTreeSet::new(),
            pins: Vec::new(),
            pinned_popover_open: false,
            unread_count: 0,
            first_unread_message_id: None,
            search: InChatSearchState::default(),
            backdrop_scene: None,
            chat_color_hex: None,
            user_bubble_solid_hex: None,
            compact_actions: true,
            reduced_motion: false,
            active_subagent_dock: None,
            active_trace_dialog: None,
            reply_return_message_id: None,
            is_at_bottom: true,
            show_more_reactions_for: None,
        }
    }

    pub fn toggle_long_message_expanded(&mut self, message_id: &str) -> bool {
        if self.expanded_long_messages.contains(message_id) {
            self.expanded_long_messages.remove(message_id);
            false
        } else {
            self.expanded_long_messages.insert(message_id.to_string());
            true
        }
    }

    /// Toggles a validated RGI emoji reaction on a 1:1 conversation message.
    pub fn toggle_reaction(
        &mut self,
        message_id: &str,
        emoji: &str,
        actor: ConversationReactionActor,
    ) -> bool {
        if !is_message_reaction(emoji) {
            return false;
        }
        let Some(row) = self.rows.iter_mut().find(|r| r.message.id == message_id) else {
            return false;
        };
        let list = row.message.reactions.get_or_insert_with(Vec::new);
        if let Some(pos) = list
            .iter()
            .position(|r| r.emoji == emoji && r.actor == actor)
        {
            list.remove(pos);
            false
        } else {
            list.push(ConversationReaction {
                emoji: emoji.to_string(),
                actor,
            });
            true
        }
    }

    /// Pins a message while enforcing `MESSAGE_PINS_LIMIT = 50`.
    pub fn pin_message(&mut self, pin: PinnedMessage) -> Result<(), ConversationPinError> {
        if self.pins.iter().any(|p| p.message_id == pin.message_id) {
            return Ok(());
        }
        if self.pins.len() >= MESSAGE_PINS_LIMIT {
            return Err(ConversationPinError {
                code: "pin_limit_exceeded".to_string(),
                message: format!(
                    "cannot pin more than {MESSAGE_PINS_LIMIT} messages in a conversation"
                ),
            });
        }
        self.pins.push(pin);
        Ok(())
    }

    /// Unpins a message, strictly blocking agents from unpinning a human (`MessagePinner::Person`) pin.
    pub fn unpin_message(
        &mut self,
        message_id: &str,
        actor_is_agent: bool,
    ) -> Result<bool, ConversationPinError> {
        let Some(pos) = self.pins.iter().position(|p| p.message_id == message_id) else {
            return Ok(false);
        };
        if actor_is_agent && matches!(self.pins[pos].pinned_by, MessagePinner::Person { .. }) {
            return Err(ConversationPinError {
                code: "pinned-by-person".to_string(),
                message: "message was pinned by person and cannot be unpinned by an agent"
                    .to_string(),
            });
        }
        self.pins.remove(pos);
        Ok(true)
    }

    /// Runs case-insensitive in-chat `Cmd+F` search across rows and updates minimap tick positions.
    pub fn run_search(&mut self, query: &str) -> usize {
        self.search.open = true;
        self.search.query = query.to_string();
        let needle = query.trim().to_ascii_lowercase();
        self.search.match_row_indices.clear();
        self.search.current_match = 0;
        if needle.is_empty() {
            return 0;
        }
        for (idx, row) in self.rows.iter().enumerate() {
            if row.message.text.to_ascii_lowercase().contains(&needle) {
                self.search.match_row_indices.push(idx);
            }
        }
        self.search.match_row_indices.len()
    }

    pub fn next_search_match(&mut self) -> Option<usize> {
        if self.search.match_row_indices.is_empty() {
            return None;
        }
        self.search.current_match =
            (self.search.current_match + 1) % self.search.match_row_indices.len();
        self.search.match_row_indices.get(self.search.current_match).copied()
    }

    pub fn prev_search_match(&mut self) -> Option<usize> {
        if self.search.match_row_indices.is_empty() {
            return None;
        }
        let len = self.search.match_row_indices.len();
        self.search.current_match = (self.search.current_match + len - 1) % len;
        self.search.match_row_indices.get(self.search.current_match).copied()
    }

    /// Normalized vertical positions (`0.0..=1.0`) for `ChatScrollRail` search ticks.
    pub fn scroll_rail_tick_fractions(&self) -> Vec<f32> {
        let denom = self.rows.len().max(1) as f32;
        self.search
            .match_row_indices
            .iter()
            .map(|&idx| ((idx as f32 + 0.5) / denom).clamp(0.02, 0.98))
            .collect()
    }

    pub fn evaluate_row_boundaries(&self, inject_corrupt_row: bool) -> ChatRowRenderResult {
        ChatRowBoundary::evaluate_rows(&self.rows, inject_corrupt_row)
    }
}

fn render_conversation_header(
    state: &ConversationViewState,
    palette: &OpenBotPalette,
    now_sec: f64,
) -> Div {
    let surface = pal_rgba(palette, "--openbot-bg-surface", 0x212121);
    let raised = pal_rgba(palette, "--openbot-bg-surface-raised", 0x242424);
    let active_pill = pal_rgba(palette, "--openbot-tabs-pill-bg", 0x2A2A2A);
    let border = pal_rgba(palette, "--openbot-border-strong", 0x333335);
    let text_primary = pal_rgba(palette, "--openbot-text-primary", 0xFFFFFF);
    let text_secondary = pal_rgba(palette, "--openbot-text-secondary", 0xDCDCDC);
    let text_muted = pal_rgba(palette, "--openbot-text-muted", 0x979797);
    let accent = pal_rgba(palette, "--openbot-accent", 0x007CF7);
    let accent_soft = pal_rgba(palette, "--openbot-accent-soft", 0x1A365D);

    let mut avatar_props = AgentAvatarProps::new(&state.agent_avatar_seed)
        .with_size(18.0)
        .with_mood(state.agent_mood);
    avatar_props.hue = state.agent_avatar_hue;
    avatar_props.color = state.agent_avatar_color.clone();
    avatar_props.reduced_motion = state.reduced_motion;

    let mut thread_tabs = div()
        .id("openbot-thread-tabs")
        .flex()
        .items_center()
        .gap(px(4.0))
        .min_w(px(0.0))
        .overflow_x_scroll();
    for tab in &state.threads {
        thread_tabs = thread_tabs.child(
            div()
                .h(px(26.0))
                .px(px(9.0))
                .rounded(px(8.0))
                .bg(if tab.active { active_pill } else { rgba(0x00000000) })
                .flex()
                .items_center()
                .gap(px(5.0))
                .cursor_pointer()
                .child(
                    div()
                        .text_size(px(12.0))
                        .font_weight(if tab.active { FontWeight::MEDIUM } else { FontWeight::NORMAL })
                        .text_color(if tab.active { text_primary } else { text_muted })
                        .child(tab.title.clone()),
                )
                .when(tab.unread_count > 0, |el| {
                    el.child(
                        div()
                            .px(px(5.0))
                            .rounded_full()
                            .bg(accent)
                            .text_size(px(10.0))
                            .text_color(rgb(0xFFFFFF))
                            .child(tab.unread_count.to_string()),
                    )
                }),
        );
    }

    let provider_label = format!(
        "{} · {} ({})",
        state.provider_picker.provider.as_str(),
        state.provider_picker.model,
        state.provider_picker.reasoning_effort
    );

    div()
        .h(px(48.0))
        .px(px(14.0))
        .bg(surface)
        .border_b_1()
        .border_color(border)
        .flex()
        .items_center()
        .justify_between()
        .gap(px(10.0))
        .child(
            div()
                .flex()
                .items_center()
                .gap(px(8.0))
                .min_w(px(0.0))
                .child(render_agent_avatar(&avatar_props, None, now_sec))
                .child(
                    div()
                        .text_size(px(14.0))
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(text_primary)
                        .truncate()
                        .child(state.agent_name.clone()),
                )
                .when_some(state.agent_title.clone(), |el, title| {
                    el.child(
                        div()
                            .px(px(7.0))
                            .py(px(2.0))
                            .rounded_full()
                            .bg(raised)
                            .text_size(px(11.0))
                            .text_color(text_muted)
                            .child(title),
                    )
                })
                .when(state.workspace_locked, |el| {
                    el.child(tg_icon_rgba(IconName::Lock, 13.0, text_muted))
                })
                .child(thread_tabs),
        )
        .child(
            div()
                .flex()
                .items_center()
                .gap(px(6.0))
                .flex_shrink_0()
                .when(state.provider_picker.next_turn_pending, |el| {
                    el.child(
                        div()
                            .px(px(6.0))
                            .py(px(2.0))
                            .rounded_full()
                            .bg(accent_soft)
                            .text_size(px(10.0))
                            .text_color(accent)
                            .child("Next turn"),
                    )
                })
                .child(
                    div()
                        .h(px(28.0))
                        .px(px(9.0))
                        .rounded(px(8.0))
                        .bg(raised)
                        .border_1()
                        .border_color(border)
                        .flex()
                        .items_center()
                        .gap(px(5.0))
                        .cursor_pointer()
                        .child(
                            div()
                                .text_size(px(12.0))
                                .text_color(text_secondary)
                                .child(provider_label),
                        )
                        .child(tg_icon_rgba(IconName::ChevronDown, 11.0, text_muted)),
                )
                .child(
                    div()
                        .w(px(28.0))
                        .h(px(28.0))
                        .rounded(px(8.0))
                        .bg(raised)
                        .flex()
                        .items_center()
                        .justify_center()
                        .cursor_pointer()
                        .child(tg_icon_rgba(IconName::ComputerDesktop, 14.0, text_secondary)),
                )
                .child(
                    div()
                        .h(px(28.0))
                        .px(px(8.0))
                        .rounded(px(8.0))
                        .bg(if state.pinned_popover_open { accent_soft } else { raised })
                        .flex()
                        .items_center()
                        .gap(px(4.0))
                        .cursor_pointer()
                        .child(tg_icon_rgba(IconName::Pin, 13.0, text_secondary))
                        .when(!state.pins.is_empty(), |el| {
                            el.child(
                                div()
                                    .text_size(px(11.0))
                                    .text_color(text_primary)
                                    .child(state.pins.len().to_string()),
                            )
                        }),
                )
                .child(
                    div()
                        .w(px(28.0))
                        .h(px(28.0))
                        .rounded(px(8.0))
                        .bg(if state.side_panel_open { accent_soft } else { raised })
                        .flex()
                        .items_center()
                        .justify_center()
                        .cursor_pointer()
                        .child(tg_icon_rgba(IconName::SidebarPanel, 14.0, text_secondary)),
                ),
        )
}

fn render_chat_scroll_rail(ticks: Vec<f32>, palette: &OpenBotPalette) -> Div {
    let accent: Hsla = pal_rgba(palette, "--openbot-accent", 0x007CF7).into();
    let track: Hsla = pal_rgba(palette, "--openbot-border-strong", 0x333335).into();
    div()
        .w(px(10.0))
        .h_full()
        .flex_shrink_0()
        .py(px(8.0))
        .child(
            canvas(
                |_, _, _| {},
                move |bounds, _, window, _cx| {
                    let bx = f32::from(bounds.origin.x);
                    let by = f32::from(bounds.origin.y);
                    let bw = f32::from(bounds.size.width).max(2.0);
                    let bh = f32::from(bounds.size.height).max(1.0);
                    paint_rect_path(window, bx + bw * 0.4, by, bx + bw * 0.6, by + bh, track);
                    for frac in &ticks {
                        let y = by + frac.clamp(0.0, 1.0) * bh;
                        paint_rect_path(window, bx + 1.0, y - 1.5, bx + bw - 1.0, y + 1.5, accent);
                    }
                },
            )
            .size_full(),
        )
}

fn render_message_reactions_bar(
    reactions: &[ConversationReaction],
    show_more: bool,
    pinned: bool,
    palette: &OpenBotPalette,
) -> Div {
    let raised = pal_rgba(palette, "--openbot-bg-surface-raised", 0x242424);
    let border = pal_rgba(palette, "--openbot-border-strong", 0x333335);
    let text_primary = pal_rgba(palette, "--openbot-text-primary", 0xFFFFFF);
    let text_muted = pal_rgba(palette, "--openbot-text-muted", 0x979797);
    let accent = pal_rgba(palette, "--openbot-accent", 0x007CF7);

    let mut bar = div().flex().flex_wrap().items_center().gap(px(4.0)).pt(px(4.0));
    let mut counted: Vec<(&str, usize)> = Vec::new();
    for r in reactions {
        if let Some((_, c)) = counted.iter_mut().find(|(e, _)| *e == r.emoji.as_str()) {
            *c += 1;
        } else {
            counted.push((r.emoji.as_str(), 1));
        }
    }
    for (emoji, count) in counted {
        bar = bar.child(
            div()
                .px(px(7.0))
                .py(px(2.0))
                .rounded_full()
                .bg(raised)
                .border_1()
                .border_color(border)
                .flex()
                .items_center()
                .gap(px(4.0))
                .text_size(px(11.0))
                .text_color(text_primary)
                .child(format!("{emoji} {count}")),
        );
    }
    for preset in MESSAGE_REACTIONS {
        bar = bar.child(
            div()
                .px(px(5.0))
                .py(px(1.0))
                .rounded_full()
                .cursor_pointer()
                .text_size(px(11.0))
                .text_color(text_muted)
                .child(preset),
        );
    }
    if show_more {
        for extra in MORE_MESSAGE_REACTIONS {
            bar = bar.child(
                div()
                    .px(px(5.0))
                    .py(px(1.0))
                    .rounded_full()
                    .cursor_pointer()
                    .text_size(px(11.0))
                    .text_color(text_muted)
                    .child(extra),
            );
        }
    }
    if pinned {
        bar = bar.child(
            div()
                .px(px(6.0))
                .py(px(2.0))
                .rounded_full()
                .bg(raised)
                .flex()
                .items_center()
                .gap(px(3.0))
                .child(tg_icon_rgba(IconName::Pin, 10.0, accent))
                .child(div().text_size(px(10.0)).text_color(accent).child("Pinned")),
        );
    }
    bar
}

fn render_pinned_popover(pins: &[PinnedMessage], palette: &OpenBotPalette) -> Div {
    let surface = pal_rgba(palette, "--openbot-bg-surface", 0x212121);
    let raised = pal_rgba(palette, "--openbot-bg-surface-raised", 0x242424);
    let ring = pal_rgba(palette, "--openbot-dialog-ring", 0x3A3A3D);
    let text_primary = pal_rgba(palette, "--openbot-text-primary", 0xFFFFFF);
    let text_muted = pal_rgba(palette, "--openbot-text-muted", 0x979797);

    let mut list = div().flex().flex_col().gap(px(6.0));
    if pins.is_empty() {
        list = list.child(
            div()
                .text_size(px(12.0))
                .text_color(text_muted)
                .child("No pinned messages yet."),
        );
    } else {
        for pin in pins {
            let who = match &pin.pinned_by {
                MessagePinner::Person { .. } => "Pinned by person (protected)",
                MessagePinner::Agent { .. } => "Pinned by agent",
            };
            list = list.child(
                div()
                    .p(px(8.0))
                    .rounded(px(8.0))
                    .bg(raised)
                    .flex()
                    .flex_col()
                    .gap(px(2.0))
                    .child(
                        div()
                            .text_size(px(12.0))
                            .text_color(text_primary)
                            .truncate()
                            .child(pin.text.clone()),
                    )
                    .child(div().text_size(px(10.0)).text_color(text_muted).child(who)),
            );
        }
    }

    div()
        .absolute()
        .top(px(52.0))
        .right(px(14.0))
        .w(px(300.0))
        .max_h(px(340.0))
        .rounded(px(14.0))
        .bg(surface)
        .border_1()
        .border_color(ring)
        .p(px(12.0))
        .occlude()
        .flex()
        .flex_col()
        .gap(px(8.0))
        .child(
            div()
                .text_size(px(13.0))
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(text_primary)
                .child(format!("Pinned Messages ({}/{MESSAGE_PINS_LIMIT})", pins.len())),
        )
        .child(
            div()
                .id("openbot-pinned-popover-scroll")
                .flex_1()
                .min_h(px(0.0))
                .overflow_y_scroll()
                .child(list),
        )
}

/// Renders the complete 1-on-1 OpenBot virtualized conversation surface (`OB-F15`).
pub fn render_conversation_view(
    state: &ConversationViewState,
    palette: &OpenBotPalette,
    now_sec: f64,
) -> Div {
    let canvas_bg = state
        .chat_color_hex
        .as_deref()
        .and_then(|hex| readable_chat_background(hex, palette.scheme).ok())
        .and_then(|css| parse_css_color_to_rgba(&css))
        .unwrap_or_else(|| pal_rgba(palette, "--openbot-bg-canvas", 0x1A1A1A));
    let surface = pal_rgba(palette, "--openbot-bg-surface", 0x212121);
    let raised = pal_rgba(palette, "--openbot-bg-surface-raised", 0x242424);
    let border = pal_rgba(palette, "--openbot-border-strong", 0x333335);
    let text_primary = pal_rgba(palette, "--openbot-text-primary", 0xFFFFFF);
    let text_muted = pal_rgba(palette, "--openbot-text-muted", 0x979797);
    let accent = pal_rgba(palette, "--openbot-accent", 0x007CF7);
    let accent_soft = pal_rgba(palette, "--openbot-accent-soft", 0x1A365D);

    let grouping = compute_row_grouping(&state.rows, state.compact_actions);
    let mut timeline_col = div().flex().flex_col().gap(px(8.0)).p(px(16.0));

    for (idx, row) in state.rows.iter().enumerate() {
        let g = &grouping[idx];
        if let Some(ref day_label) = g.day_marker_label {
            timeline_col = timeline_col.child(
                div()
                    .w_full()
                    .flex()
                    .justify_center()
                    .py(px(4.0))
                    .child(
                        div()
                            .px(px(10.0))
                            .py(px(3.0))
                            .rounded_full()
                            .bg(surface)
                            .text_size(px(11.0))
                            .text_color(text_muted)
                            .child(day_label.clone()),
                    ),
            );
        }
        if g.show_time_gap {
            timeline_col = timeline_col.child(
                div()
                    .w_full()
                    .flex()
                    .justify_center()
                    .py(px(2.0))
                    .child(
                        div()
                            .text_size(px(10.0))
                            .text_color(text_muted)
                            .child("1+ hour later"),
                    ),
            );
        }
        if state.first_unread_message_id.as_deref() == Some(row.message.id.as_str()) {
            timeline_col = timeline_col.child(
                div()
                    .w_full()
                    .flex()
                    .items_center()
                    .gap(px(8.0))
                    .py(px(4.0))
                    .child(div().flex_1().h(px(1.0)).bg(accent))
                    .child(
                        div()
                            .text_size(px(11.0))
                            .font_weight(FontWeight::MEDIUM)
                            .text_color(accent)
                            .child("Unread messages"),
                    )
                    .child(div().flex_1().h(px(1.0)).bg(accent)),
            );
        }

        let boundary = ChatRowBoundary::check_row(row);
        if boundary.corrupt {
            timeline_col = timeline_col.child(render_isolated_corrupt_row(
                &boundary.row_id,
                boundary.error_reason.as_deref().unwrap_or("Corrupt row"),
                palette,
            ));
            continue;
        }

        let is_user = row.message.author == ConversationMessageAuthor::User;
        let streaming = row.message.status == ConversationMessageStatus::Streaming;
        let expanded = state.expanded_long_messages.contains(&row.message.id);
        let body_source = if streaming && !state.streaming_pacer.target_text.is_empty() {
            state.streaming_pacer.visible_text()
        } else {
            row.message.text.clone()
        };
        let preview = compute_message_preview(&body_source, streaming, expanded);
        let parsed_blocks = parse_rich_blocks(&preview.preview_text);

        let (bubble_bg, bubble_fg) = if is_user {
            if let Some(ref solid_hex) = state.user_bubble_solid_hex {
                let bg = parse_css_color_to_rgba(solid_hex).unwrap_or(accent);
                let fg = rgb(solid_bubble_text_rgb(solid_hex));
                (bg, fg)
            } else {
                (raised, text_primary)
            }
        } else {
            (surface, text_primary)
        };

        let is_search_hit = state.search.match_row_indices.contains(&idx);
        let mut bubble = div()
            .max_w(px(620.0))
            .rounded(px(22.0))
            .bg(bubble_bg)
            .border_1()
            .border_color(if is_search_hit { accent } else { border })
            .px(px(14.0))
            .py(px(10.0))
            .text_color(bubble_fg)
            .flex()
            .flex_col()
            .gap(px(8.0));

        for blk in &parsed_blocks {
            bubble = bubble.child(render_rich_block(blk, palette));
        }
        if preview.show_more_available {
            bubble = bubble.child(
                div()
                    .px(px(10.0))
                    .py(px(4.0))
                    .rounded(px(8.0))
                    .bg(accent_soft)
                    .cursor_pointer()
                    .text_size(px(12.0))
                    .font_weight(FontWeight::MEDIUM)
                    .text_color(accent)
                    .child(if preview.collapsed {
                        format!(
                            "Show more ({} / {} chars)",
                            preview.preview_utf16_len, preview.full_utf16_len
                        )
                    } else {
                        "Show less".to_string()
                    }),
            );
        }
        if let Some(ref vp) = row.voice_player {
            bubble = bubble.child(vp.render(palette));
        }
        if let Some(ref ex) = row.message.exchange {
            bubble = bubble.child(render_agent_exchange_summary(ex, palette));
        }
        if let Some(ref steps) = row.steps {
            bubble = bubble.child(render_agent_steps(steps, palette));
        }
        if let Some(ref card) = row.ui_block_card {
            bubble = bubble.child(render_ui_block_card(card, palette));
        }
        for rt_card in &row.runtime_cards {
            bubble = bubble.child(render_runtime_turn_card(rt_card, palette));
        }

        let is_pinned = state.pins.iter().any(|p| p.message_id == row.message.id);
        let show_more_rx =
            state.show_more_reactions_for.as_deref() == Some(row.message.id.as_str());
        let rx_slice = row.message.reactions.as_deref().unwrap_or(&[]);
        bubble = bubble.child(render_message_reactions_bar(
            rx_slice,
            show_more_rx,
            is_pinned,
            palette,
        ));

        let mut row_wrap = div()
            .w_full()
            .flex()
            .items_start()
            .gap(px(8.0))
            .when(is_user, |el| el.justify_end())
            .when(!is_user, |el| el.justify_start());
        if !is_user && !g.grouped_with_prev {
            let mut av = AgentAvatarProps::new(&state.agent_avatar_seed)
                .with_size(28.0)
                .with_mood(state.agent_mood);
            av.hue = state.agent_avatar_hue;
            av.color = state.agent_avatar_color.clone();
            av.reduced_motion = state.reduced_motion;
            row_wrap = row_wrap.child(render_agent_avatar(&av, None, now_sec));
        } else if !is_user {
            row_wrap = row_wrap.child(div().w(px(28.0)).flex_shrink_0());
        }
        timeline_col = timeline_col.child(row_wrap.child(bubble));
    }

    let mut root = div()
        .relative()
        .flex_1()
        .min_w(px(CONVERSATION_MIN_WIDTH_PX))
        .min_h(px(0.0))
        .h_full()
        .bg(canvas_bg)
        .flex()
        .flex_col()
        .overflow_hidden();

    if let Some(scene) = state.backdrop_scene {
        root = root.child(render_chat_backdrop(scene, palette.scheme, state.reduced_motion));
    }

    root = root.child(
        div()
            .relative()
            .flex_1()
            .min_h(px(0.0))
            .flex()
            .flex_col()
            .child(render_conversation_header(state, palette, now_sec))
            .when(state.search.open, |el| {
                let total_hits = state.search.match_row_indices.len();
                let cur_hit = if total_hits == 0 { 0 } else { state.search.current_match + 1 };
                el.child(
                    div()
                        .h(px(36.0))
                        .px(px(14.0))
                        .bg(surface)
                        .border_b_1()
                        .border_color(border)
                        .flex()
                        .items_center()
                        .justify_between()
                        .child(
                            div()
                                .flex()
                                .items_center()
                                .gap(px(8.0))
                                .child(tg_icon_rgba(IconName::Search, 13.0, text_muted))
                                .child(
                                    div()
                                        .text_size(px(12.0))
                                        .text_color(if state.search.query.is_empty() {
                                            text_muted
                                        } else {
                                            text_primary
                                        })
                                        .child(if state.search.query.is_empty() {
                                            "Search in conversation (Cmd+F)…".to_string()
                                        } else {
                                            state.search.query.clone()
                                        }),
                                ),
                        )
                        .child(
                            div()
                                .text_size(px(11.0))
                                .text_color(text_muted)
                                .child(format!("{cur_hit} / {total_hits}")),
                        ),
                )
            })
            .when(state.unread_count > 0, |el| {
                el.child(
                    div()
                        .px(px(14.0))
                        .py(px(5.0))
                        .bg(accent_soft)
                        .flex()
                        .items_center()
                        .justify_between()
                        .child(
                            div()
                                .text_size(px(12.0))
                                .font_weight(FontWeight::MEDIUM)
                                .text_color(accent)
                                .child(format!("{} unread messages", state.unread_count)),
                        )
                        .child(div().text_size(px(11.0)).text_color(accent).child("Jump to unread")),
                )
            })
            .child(
                div()
                    .flex_1()
                    .min_h(px(0.0))
                    .flex()
                    .flex_row()
                    .child(
                        div()
                            .id("openbot-conversation-timeline")
                            .flex_1()
                            .min_h(px(0.0))
                            .min_w(px(0.0))
                            .overflow_y_scroll()
                            .child(timeline_col),
                    )
                    .child(render_chat_scroll_rail(
                        state.scroll_rail_tick_fractions(),
                        palette,
                    )),
            )
            .when_some(state.active_subagent_dock.as_ref(), |el, dock| {
                el.child(
                    div()
                        .px(px(16.0))
                        .pb(px(10.0))
                        .flex()
                        .justify_center()
                        .child(render_subagent_dock(dock, palette)),
                )
            }),
    );

    if state.reply_return_message_id.is_some() || !state.is_at_bottom {
        root = root.child(
            div()
                .absolute()
                .bottom(px(16.0))
                .right(px(22.0))
                .flex()
                .items_center()
                .gap(px(8.0))
                .when(state.reply_return_message_id.is_some(), |el| {
                    el.child(
                        div()
                            .h(px(30.0))
                            .px(px(10.0))
                            .rounded_full()
                            .bg(surface)
                            .border_1()
                            .border_color(border)
                            .flex()
                            .items_center()
                            .cursor_pointer()
                            .text_size(px(11.0))
                            .text_color(text_primary)
                            .child("Return to reply"),
                    )
                })
                .when(!state.is_at_bottom, |el| {
                    el.child(
                        div()
                            .h(px(30.0))
                            .px(px(10.0))
                            .rounded_full()
                            .bg(accent)
                            .flex()
                            .items_center()
                            .gap(px(5.0))
                            .cursor_pointer()
                            .text_size(px(11.0))
                            .font_weight(FontWeight::MEDIUM)
                            .text_color(rgb(0xFFFFFF))
                            .child(if state.unread_count > 0 {
                                format!("Latest ({})", state.unread_count)
                            } else {
                                "Latest ↓".to_string()
                            }),
                    )
                }),
        );
    }

    if state.pinned_popover_open {
        root = root.child(deferred(render_pinned_popover(&state.pins, palette)).with_priority(45));
    }

    if let Some(ref dialog) = state.active_trace_dialog
        && dialog.open
    {
        root = root.child(
            deferred(
                div()
                    .absolute()
                    .inset_0()
                    .bg(rgba(0x00000088))
                    .flex()
                    .items_center()
                    .justify_center()
                    .occlude()
                    .child(render_trace_waterfall_dialog(dialog, palette)),
            )
            .with_priority(60),
        );
    }

    root
}
