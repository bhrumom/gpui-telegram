//! `AgentSteps` (`220px` bottom-aligned live cap, strikethrough memory diffs, expandable `FailedStep` stderr,
//! `ActivityShine`), Gantt `TraceWaterfallDialog` (`4` span kinds, `5` hue bands, `5` ticks, `1/2/5/10x` replay),
//! and FLIP `SubagentDock` (`340ms` in / `260ms` out, `RadialProgress` canvas) (`OB-F15`).

use std::collections::{BTreeSet, HashSet};
use std::f32::consts::{FRAC_PI_2, TAU};

use chrono::DateTime;
use gpui::prelude::FluentBuilder;
use gpui::*;
use openbot_contracts::conversation::{
    ConversationMemoryChange, ConversationMemoryChangeAction, ConversationSubagent,
    ConversationSubagentStatus, ConversationSubagentStepKind, ConversationTraceStepKind,
    ConversationTurnTrace,
};
use serde::{Deserialize, Serialize};

use crate::theme::openbot_palette::OpenBotPalette;
use crate::theme::{IconName, tg_icon_rgba};

/// Viewport height cap for the live/finished `AgentSteps` list (`AgentSteps.tsx`).
pub const STEPS_MAX_HEIGHT: f32 = 220.0;
/// FLIP dock-in animation duration in milliseconds (`Subagents.tsx`).
pub const DOCK_IN_MS: u32 = 340;
/// FLIP dock-out animation duration in milliseconds (`Subagents.tsx`).
pub const DOCK_OUT_MS: u32 = 260;
/// Canonical span kinds rendered in [`TraceWaterfallState`].
pub const TRACE_SPAN_KINDS: [&str; 4] = ["model", "tool", "waiting", "handoff"];
/// Supported replay speed levels in [`TraceWaterfallState`].
pub const TRACE_REPLAY_SPEEDS: [&str; 4] = ["1", "2", "5", "10"];
/// Five semantic hue bands for Gantt waterfall bars (`TraceWaterfall.tsx`).
pub const TRACE_HUE_BANDS: [u32; 5] = [0x007CF7, 0x31CF76, 0xFF9412, 0xD6ADF2, 0x6BC7D9];
/// Number of time-axis tick divisions rendered in [`TraceWaterfallState`].
pub const TRACE_TICK_COUNT: usize = 5;

const STEP_ROW_ESTIMATED_HEIGHT_PX: f32 = 28.0;
const FAILED_STDERR_ESTIMATED_HEIGHT_PX: f32 = 54.0;

fn pal_rgba(palette: &OpenBotPalette, token: &str, fallback_hex: u32) -> Rgba {
    palette.rgba(token).unwrap_or_else(|| rgb(fallback_hex))
}

/// The 4 canonical `AgentStep` entry kinds (`AgentSteps.tsx`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum AgentStepKind {
    Thinking,
    Progress,
    Memory,
    MemorySummary,
}

impl AgentStepKind {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Thinking => "thinking",
            Self::Progress => "progress",
            Self::Memory => "memory",
            Self::MemorySummary => "memory-summary",
        }
    }

    pub fn parse(raw: &str) -> Self {
        match raw.trim().to_ascii_lowercase().as_str() {
            "thinking" | "reasoning" => Self::Thinking,
            "memory" => Self::Memory,
            "memory-summary" | "memory_summary" => Self::MemorySummary,
            _ => Self::Progress,
        }
    }
}

/// Single step entry inside [`AgentStepsState`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentStepEntry {
    pub id: String,
    pub kind: AgentStepKind,
    pub label: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
    #[serde(default)]
    pub failed: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stderr: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub memory_op: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub previous_text: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub duration_ms: Option<u64>,
}

impl AgentStepEntry {
    /// Returns `true` when this memory step renders overwritten or forgotten text with strikethrough (`<s>`).
    pub fn is_strikethrough_memory(&self) -> bool {
        self.kind == AgentStepKind::Memory
            && (matches!(self.memory_op.as_deref(), Some("updated" | "forgotten"))
                || self.previous_text.is_some())
    }

    /// Converts an authoritative [`ConversationMemoryChange`] into an [`AgentStepEntry`].
    pub fn from_memory_change(change: &ConversationMemoryChange) -> Self {
        let op = match change.action {
            ConversationMemoryChangeAction::Saved => "saved",
            ConversationMemoryChangeAction::Updated => "updated",
            ConversationMemoryChangeAction::Forgotten => "forgotten",
            ConversationMemoryChangeAction::NotChanged => "notChanged",
        };
        Self {
            id: format!("mem:{}", change.memory_id),
            kind: AgentStepKind::Memory,
            label: change.text.clone(),
            detail: None,
            failed: false,
            stderr: None,
            memory_op: Some(op.to_string()),
            previous_text: change.previous_text.clone(),
            duration_ms: None,
        }
    }

    /// Parses a JSON step entry from headless probes or runtime payloads.
    pub fn from_probe_value(val: &serde_json::Value, idx: usize) -> Self {
        let kind_str = val
            .get("kind")
            .and_then(serde_json::Value::as_str)
            .unwrap_or("progress");
        let kind = AgentStepKind::parse(kind_str);
        let id = val
            .get("id")
            .and_then(serde_json::Value::as_str)
            .map(ToString::to_string)
            .unwrap_or_else(|| format!("step-{idx}"));
        let label = val
            .get("label")
            .or_else(|| val.get("text"))
            .and_then(serde_json::Value::as_str)
            .unwrap_or("Working")
            .to_string();
        let detail = val
            .get("detail")
            .and_then(serde_json::Value::as_str)
            .map(ToString::to_string);
        let stderr = val
            .get("stderr")
            .or_else(|| val.get("error"))
            .and_then(serde_json::Value::as_str)
            .map(ToString::to_string);
        let status_failed = val
            .get("status")
            .and_then(serde_json::Value::as_str)
            .is_some_and(|s| s.eq_ignore_ascii_case("failed") || s.eq_ignore_ascii_case("error"));
        let failed = val
            .get("failed")
            .and_then(serde_json::Value::as_bool)
            .unwrap_or_else(|| {
                status_failed || stderr.as_deref().is_some_and(|s| !s.trim().is_empty())
            });
        let memory_op = val
            .get("op")
            .or_else(|| val.get("memory_op"))
            .or_else(|| val.get("memoryOp"))
            .or_else(|| val.get("action"))
            .and_then(serde_json::Value::as_str)
            .map(ToString::to_string);
        let previous_text = val
            .get("previousText")
            .or_else(|| val.get("previous_text"))
            .or_else(|| val.get("oldText"))
            .or_else(|| val.get("old_text"))
            .and_then(serde_json::Value::as_str)
            .map(ToString::to_string);
        let duration_ms = val
            .get("durationMs")
            .or_else(|| val.get("duration_ms"))
            .and_then(serde_json::Value::as_u64);
        Self {
            id,
            kind,
            label,
            detail,
            failed,
            stderr,
            memory_op,
            previous_text,
            duration_ms,
        }
    }
}

/// State controller for the collapsible `AgentSteps` disclosure and live bottom-aligned list.
#[derive(Debug, Clone, PartialEq)]
pub struct AgentStepsState {
    pub steps: Vec<AgentStepEntry>,
    pub live: bool,
    pub expanded: bool,
    pub expanded_failed_ids: BTreeSet<String>,
    pub total_duration_ms: u64,
    pub reduced_motion: bool,
    pub shimmer_phase: f32,
}

impl Default for AgentStepsState {
    fn default() -> Self {
        Self {
            steps: Vec::new(),
            live: false,
            expanded: true,
            expanded_failed_ids: BTreeSet::new(),
            total_duration_ms: 0,
            reduced_motion: false,
            shimmer_phase: 0.0,
        }
    }
}

impl AgentStepsState {
    pub fn from_probe_values(
        values: &[serde_json::Value],
        live: bool,
        reduced_motion: bool,
    ) -> Self {
        let steps: Vec<AgentStepEntry> = values
            .iter()
            .enumerate()
            .map(|(idx, v)| AgentStepEntry::from_probe_value(v, idx))
            .collect();
        let total_duration_ms = steps.iter().filter_map(|s| s.duration_ms).sum();
        Self {
            steps,
            live,
            expanded: true,
            expanded_failed_ids: BTreeSet::new(),
            total_duration_ms,
            reduced_motion,
            shimmer_phase: 0.35,
        }
    }

    /// Formats the collapsible header summary (`"N steps · Memory saved · 12s"`).
    pub fn summary(&self) -> String {
        let count = self.steps.len();
        let mut parts = vec![if count == 1 {
            "1 step".to_string()
        } else {
            format!("{count} steps")
        }];
        let mem_count = self
            .steps
            .iter()
            .filter(|s| matches!(s.kind, AgentStepKind::Memory | AgentStepKind::MemorySummary))
            .count();
        if mem_count == 1 {
            parts.push("Memory updated".to_string());
        } else if mem_count > 1 {
            parts.push(format!("{mem_count} memory changes"));
        }
        let fail_count = self.steps.iter().filter(|s| s.failed).count();
        if fail_count > 0 {
            parts.push(format!("{fail_count} failed"));
        }
        if self.total_duration_ms >= 1_000 {
            parts.push(format!("{}s", self.total_duration_ms / 1_000));
        } else if self.total_duration_ms > 0 {
            parts.push(format!("{}ms", self.total_duration_ms));
        }
        parts.join(" · ")
    }

    /// Counts memory entries rendered with `<s/>` strikethrough (`updated` or `forgotten`).
    pub fn strikethrough_count(&self) -> usize {
        self.steps
            .iter()
            .filter(|s| s.is_strikethrough_memory())
            .count()
    }

    /// Returns `true` if any step failed and exposes an expandable stderr disclosure.
    pub fn has_expandable_failure(&self) -> bool {
        self.steps.iter().any(|s| s.failed)
    }

    /// Estimates unclipped content height in pixels for the `220px` live bottom-align calculation.
    pub fn estimated_content_height_px(&self) -> f32 {
        self.steps
            .iter()
            .map(|s| {
                let base = STEP_ROW_ESTIMATED_HEIGHT_PX;
                if s.failed && self.expanded_failed_ids.contains(&s.id) {
                    base + FAILED_STDERR_ESTIMATED_HEIGHT_PX
                } else {
                    base
                }
            })
            .sum()
    }

    /// Computes vertical bottom-align shift (`220 - contentHeight`) when live and overflowing `STEPS_MAX_HEIGHT`.
    pub fn live_viewport_translate_y_px(&self) -> f32 {
        let content_h = self.estimated_content_height_px();
        if self.live && content_h > STEPS_MAX_HEIGHT {
            STEPS_MAX_HEIGHT - content_h
        } else {
            0.0
        }
    }

    /// Becomes scrollable once finished (`!live`) and content exceeds `STEPS_MAX_HEIGHT`.
    pub fn is_scrollable(&self) -> bool {
        !self.live && self.estimated_content_height_px() > STEPS_MAX_HEIGHT
    }

    /// Toggles the expandable `FailedStep` stderr block for `step_id`.
    pub fn toggle_failed_stderr(&mut self, step_id: &str) -> bool {
        if self.expanded_failed_ids.contains(step_id) {
            self.expanded_failed_ids.remove(step_id);
            false
        } else {
            self.expanded_failed_ids.insert(step_id.to_string());
            true
        }
    }
}

/// Renders the collapsible `AgentSteps` disclosure, bottom-aligned `220px` live viewport (or scrollable
/// finished list), `<s/>` strikethrough memory diffs, expandable `FailedStep` stderr, and `ActivityShine`.
pub fn render_agent_steps(state: &AgentStepsState, palette: &OpenBotPalette) -> Div {
    let surface = pal_rgba(palette, "--openbot-bg-surface", 0x212121);
    let raised = pal_rgba(palette, "--openbot-bg-surface-raised", 0x242424);
    let border = pal_rgba(palette, "--openbot-border-strong", 0x333335);
    let text_primary = pal_rgba(palette, "--openbot-text-primary", 0xFFFFFF);
    let text_secondary = pal_rgba(palette, "--openbot-text-secondary", 0xDCDCDC);
    let text_muted = pal_rgba(palette, "--openbot-text-muted", 0x979797);
    let accent = pal_rgba(palette, "--openbot-accent", 0x007CF7);
    let danger = pal_rgba(palette, "--openbot-danger", 0xFD2F3B);
    let danger_soft = pal_rgba(palette, "--openbot-danger-soft", 0x4A1B20);

    let mut rows = div().flex().flex_col().gap(px(5.0));
    let last_idx = state.steps.len().saturating_sub(1);
    for (idx, step) in state.steps.iter().enumerate() {
        let is_newest_live = state.live && idx == last_idx;
        let icon = match step.kind {
            AgentStepKind::Thinking => IconName::PremiumStar,
            AgentStepKind::Progress => IconName::TerminalMcp,
            AgentStepKind::Memory => IconName::BotRobot,
            AgentStepKind::MemorySummary => IconName::FileDoc,
        };
        let icon_color = if step.failed {
            danger
        } else if is_newest_live {
            accent
        } else {
            text_muted
        };

        let mut step_body = div().flex_1().min_w(px(0.0)).flex().flex_col().gap(px(3.0));
        if step.is_strikethrough_memory() {
            let struck = step
                .previous_text
                .as_deref()
                .unwrap_or(step.label.as_str())
                .to_string();
            let mut mem_row = div().flex().flex_wrap().items_center().gap(px(6.0)).child(
                div()
                    .text_size(px(12.0))
                    .text_color(text_muted)
                    .line_through()
                    .child(struck),
            );
            if step.memory_op.as_deref() == Some("updated") && step.previous_text.is_some() {
                mem_row = mem_row
                    .child(div().text_size(px(11.0)).text_color(text_muted).child("→"))
                    .child(
                        div()
                            .text_size(px(12.0))
                            .text_color(text_primary)
                            .child(step.label.clone()),
                    );
            }
            step_body = step_body.child(mem_row);
        } else {
            step_body = step_body.child(
                div()
                    .text_size(px(12.0))
                    .font_weight(if is_newest_live {
                        FontWeight::MEDIUM
                    } else {
                        FontWeight::NORMAL
                    })
                    .text_color(if step.failed {
                        danger
                    } else if is_newest_live {
                        text_primary
                    } else {
                        text_secondary
                    })
                    .child(step.label.clone()),
            );
        }

        if let Some(ref detail) = step.detail {
            step_body = step_body.child(
                div()
                    .text_size(px(11.0))
                    .text_color(text_muted)
                    .child(detail.clone()),
            );
        }

        if step.failed {
            let expanded = state.expanded_failed_ids.contains(&step.id);
            let err_text = step
                .stderr
                .clone()
                .unwrap_or_else(|| "Command exited with error".to_string());
            step_body = step_body.child(
                div()
                    .px(px(8.0))
                    .py(px(5.0))
                    .rounded(px(6.0))
                    .bg(danger_soft)
                    .flex()
                    .flex_col()
                    .gap(px(3.0))
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .justify_between()
                            .text_size(px(11.0))
                            .font_weight(FontWeight::MEDIUM)
                            .text_color(danger)
                            .child(if expanded {
                                "FailedStep stderr (hide)"
                            } else {
                                "FailedStep stderr (expand)"
                            }),
                    )
                    .when(expanded, |el| {
                        el.child(
                            div()
                                .text_size(px(11.0))
                                .text_color(text_primary)
                                .line_height(relative(1.35))
                                .child(err_text),
                        )
                    }),
            );
        }

        let shine_alpha = if is_newest_live && !state.reduced_motion {
            0.14 + 0.10 * (state.shimmer_phase * TAU).sin().abs()
        } else {
            0.0
        };

        rows = rows.child(
            div()
                .relative()
                .overflow_hidden()
                .px(px(8.0))
                .py(px(5.0))
                .rounded(px(8.0))
                .when(is_newest_live && shine_alpha > 0.0, |el| {
                    el.bg(Rgba {
                        r: accent.r,
                        g: accent.g,
                        b: accent.b,
                        a: shine_alpha,
                    })
                })
                .flex()
                .items_start()
                .gap(px(7.0))
                .child(div().pt(px(2.0)).child(tg_icon_rgba(icon, 13.0, icon_color)))
                .child(step_body),
        );
    }

    let shift_y = state.live_viewport_translate_y_px();
    let list_viewport: AnyElement = if state.live {
        div()
            .relative()
            .max_h(px(STEPS_MAX_HEIGHT))
            .overflow_hidden()
            .child(div().mt(px(shift_y)).child(rows))
            .into_any_element()
    } else {
        div()
            .id("openbot-agent-steps-scroll")
            .max_h(px(STEPS_MAX_HEIGHT))
            .min_h(px(0.0))
            .overflow_y_scroll()
            .child(rows)
            .into_any_element()
    };

    div()
        .w_full()
        .max_w(px(540.0))
        .rounded(px(12.0))
        .bg(surface)
        .border_1()
        .border_color(border)
        .p(px(10.0))
        .flex()
        .flex_col()
        .gap(px(8.0))
        .child(
            div()
                .flex()
                .items_center()
                .justify_between()
                .cursor_pointer()
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(6.0))
                        .child(tg_icon_rgba(
                            if state.expanded {
                                IconName::ChevronDown
                            } else {
                                IconName::ChevronRight
                            },
                            12.0,
                            text_muted,
                        ))
                        .child(
                            div()
                                .text_size(px(12.0))
                                .font_weight(FontWeight::MEDIUM)
                                .text_color(text_secondary)
                                .child(state.summary()),
                        ),
                )
                .when(state.live, |el| {
                    el.child(
                        div()
                            .px(px(6.0))
                            .py(px(1.0))
                            .rounded_full()
                            .bg(raised)
                            .text_size(px(10.0))
                            .text_color(accent)
                            .child("Live"),
                    )
                }),
        )
        .when(state.expanded && !state.steps.is_empty(), |el| el.child(list_viewport))
}

/// The 4 canonical Gantt span kinds in `TraceWaterfallDialog`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum TraceSpanKind {
    Model,
    Tool,
    Waiting,
    Handoff,
}

impl TraceSpanKind {
    pub const ALL: [Self; 4] = [Self::Model, Self::Tool, Self::Waiting, Self::Handoff];

    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Model => "model",
            Self::Tool => "tool",
            Self::Waiting => "waiting",
            Self::Handoff => "handoff",
        }
    }

    pub fn parse(raw: &str) -> Self {
        match raw.trim().to_ascii_lowercase().as_str() {
            "model" | "reasoning" | "message" => Self::Model,
            "waiting" | "wait" => Self::Waiting,
            "handoff" | "delegate" => Self::Handoff,
            _ => Self::Tool,
        }
    }

    pub const fn default_hue_band(self) -> usize {
        match self {
            Self::Model => 0,
            Self::Tool => 1,
            Self::Waiting => 2,
            Self::Handoff => 3,
        }
    }
}

/// Semantic icon tag for each span in `TraceWaterfallDialog`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum TraceSpanIcon {
    Agent,
    Thinking,
    Text,
    Search,
    Read,
    Edit,
    Command,
    Web,
    Tool,
}

impl TraceSpanIcon {
    pub const fn icon_name(self) -> IconName {
        match self {
            Self::Agent => IconName::BotRobot,
            Self::Thinking => IconName::PremiumStar,
            Self::Text => IconName::ComposePen,
            Self::Search => IconName::Search,
            Self::Read => IconName::FileDoc,
            Self::Edit => IconName::Edit,
            Self::Command => IconName::TerminalMcp,
            Self::Web => IconName::ComputerDesktop,
            Self::Tool => IconName::SettingsGear,
        }
    }
}

/// Single timed span in the Gantt `TraceWaterfallDialog`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TraceSpanItem {
    pub id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent_id: Option<String>,
    #[serde(default)]
    pub depth: usize,
    pub kind: TraceSpanKind,
    pub icon: TraceSpanIcon,
    pub label: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
    pub start_ms: u64,
    pub end_ms: u64,
    pub hue_band: usize,
    #[serde(default)]
    pub failed: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tokens: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cached_input: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cost_usd: Option<f64>,
    #[serde(default)]
    pub collapsed: bool,
}

impl TraceSpanItem {
    pub fn duration_ms(&self) -> u64 {
        self.end_ms.saturating_sub(self.start_ms)
    }

    pub fn from_probe_value(val: &serde_json::Value, idx: usize) -> Self {
        let kind = TraceSpanKind::parse(
            val.get("kind")
                .and_then(serde_json::Value::as_str)
                .unwrap_or("tool"),
        );
        let id = val
            .get("id")
            .and_then(serde_json::Value::as_str)
            .map(ToString::to_string)
            .unwrap_or_else(|| format!("span-{idx}"));
        let parent_id = val
            .get("parentId")
            .or_else(|| val.get("parent_id"))
            .and_then(serde_json::Value::as_str)
            .map(ToString::to_string);
        let start_ms = val
            .get("startMs")
            .or_else(|| val.get("start_ms"))
            .and_then(serde_json::Value::as_u64)
            .unwrap_or(0);
        let end_ms = val
            .get("endMs")
            .or_else(|| val.get("end_ms"))
            .and_then(serde_json::Value::as_u64)
            .unwrap_or(start_ms + 50);
        let label = val
            .get("label")
            .or_else(|| val.get("name"))
            .and_then(serde_json::Value::as_str)
            .map(ToString::to_string)
            .unwrap_or_else(|| format!("{} #{idx}", kind.as_str()));
        let icon = match kind {
            TraceSpanKind::Model => TraceSpanIcon::Thinking,
            TraceSpanKind::Tool => TraceSpanIcon::Tool,
            TraceSpanKind::Waiting => TraceSpanIcon::Web,
            TraceSpanKind::Handoff => TraceSpanIcon::Agent,
        };
        let hue_band = val
            .get("hueBand")
            .and_then(serde_json::Value::as_u64)
            .map(|h| (h as usize) % TRACE_HUE_BANDS.len())
            .unwrap_or_else(|| kind.default_hue_band() % TRACE_HUE_BANDS.len());
        Self {
            id,
            parent_id,
            depth: val
                .get("depth")
                .and_then(serde_json::Value::as_u64)
                .unwrap_or(0) as usize,
            kind,
            icon,
            label,
            detail: val
                .get("detail")
                .and_then(serde_json::Value::as_str)
                .map(ToString::to_string),
            start_ms,
            end_ms: end_ms.max(start_ms),
            hue_band,
            failed: val
                .get("failed")
                .and_then(serde_json::Value::as_bool)
                .unwrap_or(false),
            tokens: val.get("tokens").and_then(serde_json::Value::as_u64),
            cached_input: val
                .get("cachedInput")
                .or_else(|| val.get("cached_input"))
                .and_then(serde_json::Value::as_u64),
            cost_usd: val
                .get("costUsd")
                .or_else(|| val.get("cost_usd"))
                .and_then(serde_json::Value::as_f64),
            collapsed: false,
        }
    }
}

fn parse_rfc3339_ms(ts: &str) -> Option<i64> {
    DateTime::parse_from_rfc3339(ts)
        .ok()
        .map(|dt| dt.timestamp_millis())
}

/// Complete state controller for the Gantt `TraceWaterfallDialog`.
#[derive(Debug, Clone, PartialEq)]
pub struct TraceWaterfallState {
    pub open: bool,
    pub title: String,
    pub spans: Vec<TraceSpanItem>,
    pub search_query: String,
    pub replay_speed: String,
    pub replay_cursor_ms: u64,
    pub playing: bool,
    pub total_tokens: u64,
    pub cached_input_tokens: u64,
    pub total_cost_usd: f64,
    pub selected_span_id: Option<String>,
}

impl Default for TraceWaterfallState {
    fn default() -> Self {
        Self {
            open: false,
            title: "Turn Trace".to_string(),
            spans: Vec::new(),
            search_query: String::new(),
            replay_speed: "1".to_string(),
            replay_cursor_ms: 0,
            playing: false,
            total_tokens: 0,
            cached_input_tokens: 0,
            total_cost_usd: 0.0,
            selected_span_id: None,
        }
    }
}

impl TraceWaterfallState {
    /// Builds a waterfall from an authoritative [`ConversationTurnTrace`].
    pub fn from_turn_trace(title: impl Into<String>, trace: &ConversationTurnTrace) -> Self {
        let origin_ms = parse_rfc3339_ms(&trace.started_at).unwrap_or(0);
        let mut spans = Vec::with_capacity(trace.steps.len());
        for (idx, step) in trace.steps.iter().enumerate() {
            let start_abs = parse_rfc3339_ms(&step.started_at).unwrap_or(origin_ms);
            let end_abs = step
                .ended_at
                .as_deref()
                .and_then(parse_rfc3339_ms)
                .unwrap_or(start_abs + 50);
            let start_ms = start_abs.saturating_sub(origin_ms).max(0) as u64;
            let end_ms = end_abs.saturating_sub(origin_ms).max(start_ms as i64) as u64;
            let (kind, icon) = match step.kind {
                ConversationTraceStepKind::Reasoning => {
                    (TraceSpanKind::Model, TraceSpanIcon::Thinking)
                }
                ConversationTraceStepKind::Message => (
                    if step.recipient_agent_ids.as_ref().is_some_and(|r| !r.is_empty()) {
                        TraceSpanKind::Handoff
                    } else {
                        TraceSpanKind::Model
                    },
                    TraceSpanIcon::Text,
                ),
                ConversationTraceStepKind::Tool => (
                    TraceSpanKind::Tool,
                    if step.command.is_some() {
                        TraceSpanIcon::Command
                    } else {
                        TraceSpanIcon::Tool
                    },
                ),
            };
            spans.push(TraceSpanItem {
                id: step.id.clone(),
                parent_id: None,
                depth: 0,
                kind,
                icon,
                label: step
                    .command
                    .clone()
                    .or_else(|| step.text.clone())
                    .unwrap_or_else(|| format!("{} #{idx}", kind.as_str())),
                detail: step.error.clone(),
                start_ms,
                end_ms,
                hue_band: (kind.default_hue_band() + idx) % TRACE_HUE_BANDS.len(),
                failed: step.failed == Some(true),
                tokens: None,
                cached_input: None,
                cost_usd: None,
                collapsed: false,
            });
        }
        Self {
            open: true,
            title: title.into(),
            spans,
            search_query: String::new(),
            replay_speed: "1".to_string(),
            replay_cursor_ms: 0,
            playing: false,
            total_tokens: trace.tokens.unwrap_or(0),
            cached_input_tokens: trace.cached_input.unwrap_or(0),
            total_cost_usd: trace.cost_usd.flatten().unwrap_or(0.0),
            selected_span_id: None,
        }
    }

    /// Builds a waterfall from JSON probe spans.
    pub fn from_probe_values(values: &[serde_json::Value]) -> Self {
        let spans: Vec<TraceSpanItem> = values
            .iter()
            .enumerate()
            .map(|(idx, v)| TraceSpanItem::from_probe_value(v, idx))
            .collect();
        let total_tokens = spans.iter().filter_map(|s| s.tokens).sum();
        let cached_input_tokens = spans.iter().filter_map(|s| s.cached_input).sum();
        let total_cost_usd = spans.iter().filter_map(|s| s.cost_usd).sum();
        Self {
            open: true,
            title: "Turn Trace".to_string(),
            spans,
            search_query: String::new(),
            replay_speed: "1".to_string(),
            replay_cursor_ms: 0,
            playing: false,
            total_tokens,
            cached_input_tokens,
            total_cost_usd,
            selected_span_id: None,
        }
    }

    pub fn total_duration_ms(&self) -> u64 {
        self.spans.iter().map(|s| s.end_ms).max().unwrap_or(100).max(1)
    }

    /// Generates `TRACE_TICK_COUNT` (`5`) clean `[1, 2, 5]`-step ticks covering `0..=total_duration_ms`.
    pub fn time_ticks_ms(&self) -> Vec<u64> {
        let total = self.total_duration_ms();
        let raw_step = (total as f64 / (TRACE_TICK_COUNT - 1) as f64).max(1.0);
        let mag = 10f64.powf(raw_step.log10().floor());
        let norm = raw_step / mag;
        let nice_factor = if norm <= 1.0 {
            1.0
        } else if norm <= 2.0 {
            2.0
        } else if norm <= 5.0 {
            5.0
        } else {
            10.0
        };
        let step = ((nice_factor * mag).round() as u64).max(1);
        (0..TRACE_TICK_COUNT).map(|i| step * (i as u64)).collect()
    }

    /// Filters spans by `search_query` and excludes children of collapsed parent spans.
    pub fn filtered_spans(&self) -> Vec<&TraceSpanItem> {
        let q = self.search_query.trim().to_ascii_lowercase();
        let collapsed_ids: HashSet<&str> = self
            .spans
            .iter()
            .filter(|s| s.collapsed)
            .map(|s| s.id.as_str())
            .collect();
        self.spans
            .iter()
            .filter(|s| {
                let hidden_by_parent = s
                    .parent_id
                    .as_deref()
                    .is_some_and(|pid| collapsed_ids.contains(pid));
                if hidden_by_parent {
                    return false;
                }
                if q.is_empty() {
                    return true;
                }
                s.label.to_ascii_lowercase().contains(&q)
                    || s.kind.as_str().contains(&q)
                    || s.detail
                        .as_deref()
                        .is_some_and(|d| d.to_ascii_lowercase().contains(&q))
            })
            .collect()
    }

    pub fn set_replay_speed(&mut self, speed: &str) -> bool {
        if TRACE_REPLAY_SPEEDS.contains(&speed) {
            self.replay_speed = speed.to_string();
            true
        } else {
            false
        }
    }

    pub fn cycle_replay_speed(&mut self) -> &str {
        let idx = TRACE_REPLAY_SPEEDS
            .iter()
            .position(|&s| s == self.replay_speed.as_str())
            .unwrap_or(0);
        self.replay_speed = TRACE_REPLAY_SPEEDS[(idx + 1) % TRACE_REPLAY_SPEEDS.len()].to_string();
        &self.replay_speed
    }
}

fn paint_filled_rect(
    window: &mut Window,
    x0: f32,
    y0: f32,
    x1: f32,
    y1: f32,
    color: Hsla,
) {
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

/// Renders the Gantt `TraceWaterfallDialog` with 5 hue bands, 5 time-axis ticks, collapsible span
/// list, search filter bar, token/cost metrics, `1/2/5/10x` replay controls, and GPUI `canvas(...)` bars.
pub fn render_trace_waterfall_dialog(
    state: &TraceWaterfallState,
    palette: &OpenBotPalette,
) -> Div {
    let surface = pal_rgba(palette, "--openbot-bg-surface", 0x212121);
    let raised = pal_rgba(palette, "--openbot-bg-surface-raised", 0x242424);
    let border = pal_rgba(palette, "--openbot-border-strong", 0x333335);
    let ring = pal_rgba(palette, "--openbot-dialog-ring", 0x3A3A3D);
    let text_primary = pal_rgba(palette, "--openbot-text-primary", 0xFFFFFF);
    let text_secondary = pal_rgba(palette, "--openbot-text-secondary", 0xDCDCDC);
    let text_muted = pal_rgba(palette, "--openbot-text-muted", 0x979797);
    let accent = pal_rgba(palette, "--openbot-accent", 0x007CF7);
    let on_accent = pal_rgba(palette, "--openbot-text-on-accent", 0xFFFFFF);
    let danger = pal_rgba(palette, "--openbot-danger", 0xFD2F3B);

    let visible = state.filtered_spans();
    let ticks = state.time_ticks_ms();
    let axis_max_ms = ticks
        .last()
        .copied()
        .unwrap_or_else(|| state.total_duration_ms())
        .max(state.total_duration_ms())
        .max(1);

    let mut speed_row = div().flex().items_center().gap(px(4.0));
    for speed in TRACE_REPLAY_SPEEDS {
        let active = state.replay_speed == speed;
        speed_row = speed_row.child(
            div()
                .h(px(24.0))
                .px(px(8.0))
                .rounded(px(6.0))
                .bg(if active { accent } else { raised })
                .flex()
                .items_center()
                .cursor_pointer()
                .text_size(px(11.0))
                .font_weight(FontWeight::MEDIUM)
                .text_color(if active { on_accent } else { text_secondary })
                .child(format!("{speed}x")),
        );
    }

    let mut tick_header = div()
        .flex_1()
        .flex()
        .items_center()
        .justify_between()
        .px(px(6.0));
    for t_ms in &ticks {
        tick_header = tick_header.child(
            div()
                .text_size(px(10.0))
                .text_color(text_muted)
                .child(format!("{t_ms}ms")),
        );
    }

    let mut span_rows = div().flex().flex_col().gap(px(4.0));
    for span in &visible {
        let hue_rgb = TRACE_HUE_BANDS[span.hue_band % TRACE_HUE_BANDS.len()];
        let bar_color: Hsla = if span.failed {
            danger.into()
        } else {
            rgb(hue_rgb).into()
        };
        let grid_color: Hsla = border.into();
        let cursor_color: Hsla = accent.into();
        let start_frac = (span.start_ms as f32 / axis_max_ms as f32).clamp(0.0, 1.0);
        let end_frac = (span.end_ms as f32 / axis_max_ms as f32).clamp(start_frac, 1.0);
        let cursor_frac = (state.replay_cursor_ms as f32 / axis_max_ms as f32).clamp(0.0, 1.0);
        let is_selected = state.selected_span_id.as_deref() == Some(span.id.as_str());
        let indent_px = (span.depth as f32) * 14.0;

        span_rows = span_rows.child(
            div()
                .h(px(28.0))
                .px(px(8.0))
                .rounded(px(8.0))
                .bg(if is_selected { raised } else { rgba(0x00000000) })
                .flex()
                .items_center()
                .gap(px(8.0))
                .child(
                    div()
                        .w(px(210.0))
                        .pl(px(indent_px))
                        .flex_shrink_0()
                        .flex()
                        .items_center()
                        .gap(px(6.0))
                        .min_w(px(0.0))
                        .child(tg_icon_rgba(
                            span.icon.icon_name(),
                            12.0,
                            if span.failed { danger } else { rgb(hue_rgb) },
                        ))
                        .child(
                            div()
                                .text_size(px(12.0))
                                .text_color(text_primary)
                                .truncate()
                                .child(span.label.clone()),
                        ),
                )
                .child(
                    div().flex_1().h(px(18.0)).child(
                        canvas(
                            |_, _, _| {},
                            move |bounds, _, window, _cx| {
                                let bx = f32::from(bounds.origin.x);
                                let by = f32::from(bounds.origin.y);
                                let bw = f32::from(bounds.size.width).max(1.0);
                                let bh = f32::from(bounds.size.height).max(1.0);
                                for tick_i in 0..TRACE_TICK_COUNT {
                                    let tx = bx + (tick_i as f32 / (TRACE_TICK_COUNT - 1) as f32) * bw;
                                    paint_filled_rect(window, tx, by, tx + 1.0, by + bh, grid_color);
                                }
                                let x0 = bx + start_frac * bw;
                                let x1 = (bx + end_frac * bw).max(x0 + 3.0).min(bx + bw);
                                paint_filled_rect(window, x0, by + 3.0, x1, by + bh - 3.0, bar_color);
                                let cx_px = bx + cursor_frac * bw;
                                paint_filled_rect(window, cx_px, by, cx_px + 1.5, by + bh, cursor_color);
                            },
                        )
                        .size_full(),
                    ),
                )
                .child(
                    div()
                        .w(px(54.0))
                        .flex_shrink_0()
                        .text_size(px(11.0))
                        .text_color(text_muted)
                        .child(format!("{}ms", span.duration_ms())),
                ),
        );
    }

    div()
        .w(px(720.0))
        .max_h(px(520.0))
        .rounded(px(20.0))
        .bg(surface)
        .border_1()
        .border_color(ring)
        .p(px(16.0))
        .occlude()
        .flex()
        .flex_col()
        .gap(px(12.0))
        .child(
            div()
                .flex()
                .items_center()
                .justify_between()
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(8.0))
                        .child(
                            div()
                                .text_size(px(15.0))
                                .font_weight(FontWeight::SEMIBOLD)
                                .text_color(text_primary)
                                .child(state.title.clone()),
                        )
                        .child(
                            div()
                                .px(px(8.0))
                                .py(px(2.0))
                                .rounded_full()
                                .bg(raised)
                                .text_size(px(11.0))
                                .text_color(text_muted)
                                .child(format!(
                                    "{} tok · {} cached · ${:.4}",
                                    state.total_tokens,
                                    state.cached_input_tokens,
                                    state.total_cost_usd
                                )),
                        ),
                )
                .child(speed_row),
        )
        .child(
            div()
                .h(px(30.0))
                .px(px(10.0))
                .rounded(px(8.0))
                .bg(raised)
                .border_1()
                .border_color(border)
                .flex()
                .items_center()
                .justify_between()
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(6.0))
                        .child(tg_icon_rgba(IconName::Search, 13.0, text_muted))
                        .child(
                            div()
                                .text_size(px(12.0))
                                .text_color(if state.search_query.is_empty() {
                                    text_muted
                                } else {
                                    text_primary
                                })
                                .child(if state.search_query.is_empty() {
                                    "Filter spans (Cmd+F)…".to_string()
                                } else {
                                    state.search_query.clone()
                                }),
                        ),
                )
                .child(
                    div()
                        .text_size(px(11.0))
                        .text_color(text_muted)
                        .child(format!("{}/{} spans", visible.len(), state.spans.len())),
                ),
        )
        .child(
            div()
                .flex()
                .items_center()
                .gap(px(8.0))
                .px(px(8.0))
                .child(div().w(px(210.0)).text_size(px(11.0)).text_color(text_muted).child("Span"))
                .child(tick_header)
                .child(div().w(px(54.0)).text_size(px(11.0)).text_color(text_muted).child("Time")),
        )
        .child(
            div()
                .id("openbot-trace-waterfall-rows")
                .flex_1()
                .min_h(px(0.0))
                .overflow_y_scroll()
                .child(span_rows),
        )
}

/// Maps a subagent step kind to its canonical GPUI `IconName` glyph.
pub const fn subagent_step_glyph_icon(kind: ConversationSubagentStepKind) -> IconName {
    match kind {
        ConversationSubagentStepKind::Thinking => IconName::BotRobot,
        ConversationSubagentStepKind::Search => IconName::Search,
        ConversationSubagentStepKind::Read => IconName::FileDoc,
        ConversationSubagentStepKind::Web => IconName::ComputerDesktop,
        ConversationSubagentStepKind::Edit => IconName::Edit,
        ConversationSubagentStepKind::Command => IconName::TerminalMcp,
        ConversationSubagentStepKind::Text => IconName::ComposePen,
        ConversationSubagentStepKind::Tool => IconName::SettingsGear,
    }
}

/// Live or completed subagent entry rendered inside [`SubagentDockState`].
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SubagentItem {
    pub id: String,
    pub title: String,
    pub status: String,
    pub step_glyph: ConversationSubagentStepKind,
    pub progress: f32,
    pub elapsed_ms: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub latest_step: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

impl SubagentItem {
    pub fn is_running(&self) -> bool {
        self.status.eq_ignore_ascii_case("running")
    }

    pub fn is_finished(&self) -> bool {
        matches!(
            self.status.to_ascii_lowercase().as_str(),
            "finished" | "completed" | "failed" | "stopped"
        )
    }

    pub fn from_contract(sub: &ConversationSubagent) -> Self {
        let status = match sub.status {
            ConversationSubagentStatus::Running => "running",
            ConversationSubagentStatus::Completed => "finished",
            ConversationSubagentStatus::Failed => "failed",
            ConversationSubagentStatus::Stopped => "stopped",
        };
        let step_glyph = sub
            .steps
            .last()
            .map(|s| s.kind)
            .unwrap_or(ConversationSubagentStepKind::Thinking);
        let latest_step = sub.steps.last().map(|s| s.text.clone());
        let progress = match sub.status {
            ConversationSubagentStatus::Completed => 1.0,
            ConversationSubagentStatus::Running => {
                ((sub.steps.len() as f32) / 8.0).clamp(0.15, 0.92)
            }
            ConversationSubagentStatus::Failed | ConversationSubagentStatus::Stopped => 1.0,
        };
        let elapsed_ms = match (
            parse_rfc3339_ms(&sub.started_at),
            sub.ended_at.as_deref().and_then(parse_rfc3339_ms),
        ) {
            (Some(s), Some(e)) => e.saturating_sub(s).max(0) as u64,
            _ => (sub.steps.len() as u64) * 850,
        };
        Self {
            id: sub.id.clone(),
            title: sub.title.clone(),
            status: status.to_string(),
            step_glyph,
            progress,
            elapsed_ms,
            latest_step,
            error: sub.error.clone(),
        }
    }

    pub fn from_probe_value(val: &serde_json::Value, idx: usize) -> Self {
        let id = val
            .get("id")
            .and_then(serde_json::Value::as_str)
            .map(ToString::to_string)
            .unwrap_or_else(|| format!("sub-{idx}"));
        let status = val
            .get("status")
            .and_then(serde_json::Value::as_str)
            .unwrap_or("running")
            .to_string();
        let title = val
            .get("title")
            .and_then(serde_json::Value::as_str)
            .map(ToString::to_string)
            .unwrap_or_else(|| format!("Subagent {id}"));
        let progress = val
            .get("progress")
            .and_then(serde_json::Value::as_f64)
            .map(|p| (p as f32).clamp(0.0, 1.0))
            .unwrap_or_else(|| if status.eq_ignore_ascii_case("running") { 0.45 } else { 1.0 });
        let elapsed_ms = val
            .get("elapsedMs")
            .or_else(|| val.get("elapsed_ms"))
            .and_then(serde_json::Value::as_u64)
            .unwrap_or(1_200);
        Self {
            id,
            title,
            status,
            step_glyph: ConversationSubagentStepKind::Thinking,
            progress,
            elapsed_ms,
            latest_step: val
                .get("latestStep")
                .and_then(serde_json::Value::as_str)
                .map(ToString::to_string),
            error: val
                .get("error")
                .and_then(serde_json::Value::as_str)
                .map(ToString::to_string),
        }
    }
}

/// Sticky FLIP-animated `SubagentDock` state (`340ms` dock-in, `260ms` dock-out).
#[derive(Debug, Clone, PartialEq)]
pub struct SubagentDockState {
    pub items: Vec<SubagentItem>,
    pub expanded: bool,
    pub docked: bool,
    pub reduced_motion: bool,
    pub selected_subagent_id: Option<String>,
}

impl Default for SubagentDockState {
    fn default() -> Self {
        Self {
            items: Vec::new(),
            expanded: false,
            docked: true,
            reduced_motion: false,
            selected_subagent_id: None,
        }
    }
}

impl SubagentDockState {
    pub fn from_contract_list(subs: &[ConversationSubagent], reduced_motion: bool) -> Self {
        let items: Vec<SubagentItem> = subs.iter().map(SubagentItem::from_contract).collect();
        let mut dock = Self {
            items,
            expanded: false,
            docked: true,
            reduced_motion,
            selected_subagent_id: None,
        };
        dock.sync_auto_expand();
        dock
    }

    pub fn from_probe_values(values: &[serde_json::Value], reduced_motion: bool) -> Self {
        let items: Vec<SubagentItem> = values
            .iter()
            .enumerate()
            .map(|(idx, v)| SubagentItem::from_probe_value(v, idx))
            .collect();
        let mut dock = Self {
            items,
            expanded: false,
            docked: true,
            reduced_motion,
            selected_subagent_id: None,
        };
        dock.sync_auto_expand();
        dock
    }

    pub fn running_count(&self) -> usize {
        self.items.iter().filter(|i| i.is_running()).count()
    }

    pub fn finished_count(&self) -> usize {
        self.items.iter().filter(|i| i.is_finished()).count()
    }

    /// Effective FLIP dock-in duration (`0ms` under reduced motion, otherwise `DOCK_IN_MS = 340`).
    pub fn effective_in_ms(&self) -> u32 {
        if self.reduced_motion { 0 } else { DOCK_IN_MS }
    }

    /// Effective FLIP dock-out duration (`0ms` under reduced motion, otherwise `DOCK_OUT_MS = 260`).
    pub fn effective_out_ms(&self) -> u32 {
        if self.reduced_motion { 0 } else { DOCK_OUT_MS }
    }

    /// Auto-expands whenever `running_count() > 0` (`Subagents.tsx`).
    pub fn sync_auto_expand(&mut self) {
        if self.running_count() > 0 {
            self.expanded = true;
        }
    }
}

/// Renders a crisp GPUI `canvas(...)` `RadialProgress` indicator ring (`0.0..=1.0`).
pub fn render_radial_progress(
    progress: f32,
    size_px: f32,
    track_color: Rgba,
    fill_color: Rgba,
) -> Div {
    let clamped = progress.clamp(0.0, 1.0);
    let track_hsla: Hsla = track_color.into();
    let fill_hsla: Hsla = fill_color.into();
    div()
        .w(px(size_px))
        .h(px(size_px))
        .flex_shrink_0()
        .child(
            canvas(
                |_, _, _| {},
                move |bounds, _, window, _cx| {
                    let bw = f32::from(bounds.size.width);
                    let bh = f32::from(bounds.size.height);
                    let cx = f32::from(bounds.origin.x) + bw * 0.5;
                    let cy = f32::from(bounds.origin.y) + bh * 0.5;
                    let radius = (bw.min(bh) * 0.5 - 2.0).max(2.0);
                    let segs = 32usize;

                    let mut track = PathBuilder::stroke(px(2.0));
                    for i in 0..=segs {
                        let ang = (i as f32 / segs as f32) * TAU;
                        let pt = point(px(cx + ang.cos() * radius), px(cy + ang.sin() * radius));
                        if i == 0 {
                            track.move_to(pt);
                        } else {
                            track.line_to(pt);
                        }
                    }
                    if let Ok(path) = track.build() {
                        window.paint_path(path, track_hsla);
                    }

                    if clamped > 0.001 {
                        let sweep_segs = ((segs as f32) * clamped).ceil().max(2.0) as usize;
                        let mut arc = PathBuilder::stroke(px(2.25));
                        for i in 0..=sweep_segs {
                            let frac = (i as f32 / sweep_segs as f32) * clamped;
                            let ang = -FRAC_PI_2 + frac * TAU;
                            let pt = point(px(cx + ang.cos() * radius), px(cy + ang.sin() * radius));
                            if i == 0 {
                                arc.move_to(pt);
                            } else {
                                arc.line_to(pt);
                            }
                        }
                        if let Ok(path) = arc.build() {
                            window.paint_path(path, fill_hsla);
                        }
                    }
                },
            )
            .size_full(),
        )
}

/// Renders the sticky FLIP `SubagentDock` with running/finished counts, live step glyphs,
/// elapsed timers, and GPUI `canvas(...)` `RadialProgress` rings.
pub fn render_subagent_dock(state: &SubagentDockState, palette: &OpenBotPalette) -> Div {
    let glass = pal_rgba(palette, "--openbot-bg-glass", 0x212121);
    let raised = pal_rgba(palette, "--openbot-bg-surface-raised", 0x242424);
    let border = pal_rgba(palette, "--openbot-border-strong", 0x333335);
    let text_primary = pal_rgba(palette, "--openbot-text-primary", 0xFFFFFF);
    let text_muted = pal_rgba(palette, "--openbot-text-muted", 0x979797);
    let accent = pal_rgba(palette, "--openbot-accent", 0x007CF7);
    let success = pal_rgba(palette, "--openbot-success", 0x31CF76);
    let danger = pal_rgba(palette, "--openbot-danger", 0xFD2F3B);

    let running = state.running_count();
    let finished = state.finished_count();
    let header_summary = format!("{running} running · {finished} finished");

    let mut list = div().flex().flex_col().gap(px(5.0));
    for item in &state.items {
        let ring_color = if item.status.eq_ignore_ascii_case("failed") {
            danger
        } else if item.is_finished() {
            success
        } else {
            accent
        };
        let elapsed_label = if item.elapsed_ms >= 1_000 {
            format!("{:.1}s", item.elapsed_ms as f64 / 1000.0)
        } else {
            format!("{}ms", item.elapsed_ms)
        };
        list = list.child(
            div()
                .px(px(8.0))
                .py(px(6.0))
                .rounded(px(10.0))
                .bg(raised)
                .flex()
                .items_center()
                .justify_between()
                .gap(px(8.0))
                .cursor_pointer()
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(8.0))
                        .min_w(px(0.0))
                        .child(render_radial_progress(item.progress, 18.0, border, ring_color))
                        .child(tg_icon_rgba(
                            subagent_step_glyph_icon(item.step_glyph),
                            13.0,
                            ring_color,
                        ))
                        .child(
                            div()
                                .flex()
                                .flex_col()
                                .min_w(px(0.0))
                                .child(
                                    div()
                                        .text_size(px(12.0))
                                        .font_weight(FontWeight::MEDIUM)
                                        .text_color(text_primary)
                                        .truncate()
                                        .child(item.title.clone()),
                                )
                                .when_some(item.latest_step.clone(), |el, st| {
                                    el.child(
                                        div()
                                            .text_size(px(11.0))
                                            .text_color(text_muted)
                                            .truncate()
                                            .child(st),
                                    )
                                }),
                        ),
                )
                .child(
                    div()
                        .text_size(px(11.0))
                        .text_color(text_muted)
                        .flex_shrink_0()
                        .child(elapsed_label),
                ),
        );
    }

    div()
        .w_full()
        .max_w(px(540.0))
        .rounded(px(14.0))
        .bg(glass)
        .border_1()
        .border_color(border)
        .p(px(10.0))
        .flex()
        .flex_col()
        .gap(px(8.0))
        .child(
            div()
                .flex()
                .items_center()
                .justify_between()
                .cursor_pointer()
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(6.0))
                        .child(tg_icon_rgba(IconName::BotRobot, 14.0, accent))
                        .child(
                            div()
                                .text_size(px(12.0))
                                .font_weight(FontWeight::SEMIBOLD)
                                .text_color(text_primary)
                                .child("Subagents"),
                        )
                        .child(
                            div()
                                .text_size(px(11.0))
                                .text_color(text_muted)
                                .child(header_summary),
                        ),
                )
                .child(tg_icon_rgba(
                    if state.expanded {
                        IconName::ChevronDown
                    } else {
                        IconName::ChevronRight
                    },
                    12.0,
                    text_muted,
                )),
        )
        .when(state.expanded && !state.items.is_empty(), |el| el.child(list))
}
