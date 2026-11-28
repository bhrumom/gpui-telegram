//! Secondary OpenBot workspace pages (`OB-F18`):
//! - `RoutineCalendarState` & `render_routine_calendar` (`44`-day DST-safe window, `week/month/day`, `UPCOMING_LIMIT = 6`, inline create card)
//! - `TasksPanelState` & `render_tasks_panel` (`MAX_TASKS_PER_AGENT = 50`, resizable split)
//! - `LibraryPanelState` & `render_library_panel` (`Mosaic`/`List`, `marquee_selection = true`, `StorageUsageBar`, `MachineResources`)
//! - `MarketplacePageState` & `render_marketplace_page` (3 tabs `apps/agents/skills`, breadcrumbs, `fishing` alpha fallback via M2 `MarketplaceNoMatchPlushState`)
//! - `CrmPageState` & `render_crm_page` (13 SQLite tables, `customers`/`payments` with overdue `data-late` badge, Stripe Connectors, `CrmCustomerPanel` redaction, `ReviewPanel` `crm_propose`)
//! - `UsageAndCreditsState` & `render_usage_and_credits` (provider token/cost charts, quota bars, rolling metallic price display, credits wallet)

use std::collections::{BTreeMap, BTreeSet, HashMap};

use chrono::{Datelike, Days, Duration as ChronoDuration, NaiveDate, Utc, Weekday};
use gpui::prelude::FluentBuilder;
use gpui::*;
use gpui_telegram_client::net::HermesUsageSummaryDto;
use openbot_contracts::agents::{AgentTaskOutcome, AgentTaskState};
use openbot_contracts::avatar::AvatarMood;
use openbot_contracts::crm::{
    CrmActivity, CrmCustomer, CrmOverview, CrmPayment, CrmPrivateField, CrmProposal,
};
use openbot_contracts::routines::{
    ROUTINE_MINIMUM_INTERVAL_MINUTES, is_valid_timezone, routine_once_instant,
};
use openbot_storage::HostAnalytics;
use serde::{Deserialize, Serialize};

use crate::components::bloub_avatar::{
    AgentAvatarProps, MarketplaceNoMatchPlushState, render_agent_avatar,
};
use crate::i18n::tr_str;
use crate::theme::openbot_palette::{
    OpenBotColorScheme, OpenBotPalette, file_reference_badge, file_reference_tone,
    parse_css_color_to_rgba,
};
use crate::theme::{IconName, tg_icon_rgba};

fn pal_rgba(palette: &OpenBotPalette, key: &str, fallback_hex: u32) -> Rgba {
    palette.rgba(key).unwrap_or_else(|| rgb(fallback_hex))
}

// ============================================================================
// 1. RoutineCalendar (`RoutineCalendar.tsx`)
// ============================================================================

/// Exact calendar lookahead/window span (`INPUT_LIMITS.routine_calendar_days`).
pub const WINDOW_DAYS: usize = 44;
/// Maximum upcoming runs shown in the right-hand rail.
pub const UPCOMING_LIMIT: usize = 6;
/// Minimum routine interval in minutes (`>= 3m` / `180s`).
pub const ROUTINE_MIN_INTERVAL_MINUTES: u32 = ROUTINE_MINIMUM_INTERVAL_MINUTES;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum CalendarViewMode {
    Month,
    #[default]
    Week,
    Day,
}

impl CalendarViewMode {
    pub const ALL: [Self; 3] = [Self::Month, Self::Week, Self::Day];

    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Month => "month",
            Self::Week => "week",
            Self::Day => "day",
        }
    }

    pub const fn label(self) -> &'static str {
        match self {
            Self::Month => "Month",
            Self::Week => "Week",
            Self::Day => "Day",
        }
    }

    pub fn parse(raw: &str) -> Result<Self, String> {
        match raw.trim().to_ascii_lowercase().as_str() {
            "month" => Ok(Self::Month),
            "week" => Ok(Self::Week),
            "day" => Ok(Self::Day),
            other => Err(format!(
                "invalid routine calendar view '{other}': expected 'month', 'week', or 'day'"
            )),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum RoutineStatusPill {
    Active,
    Paused,
    Failed,
    Completed,
}

impl RoutineStatusPill {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Active => "active",
            Self::Paused => "paused",
            Self::Failed => "failed",
            Self::Completed => "completed",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RoutineCalendarItem {
    pub id: String,
    pub name: String,
    pub agent_id: String,
    pub date_ymd: String,
    pub time_hhmm: String,
    pub timezone: String,
    pub status: RoutineStatusPill,
    pub schedule_summary: String,
    pub is_webhook: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RoutineCalendarDayCell {
    pub date_ymd: String,
    pub day_of_month: u32,
    pub weekday_short: &'static str,
    pub in_current_month: bool,
    pub is_anchor_day: bool,
    pub is_dst_clock_change: bool,
    pub items: Vec<RoutineCalendarItem>,
    pub overflow_count: usize,
}

/// Inline `RoutineCalendarCreateCard` draft enforcing `>= 3m` interval or HMAC webhook.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RoutineCalendarCreateCard {
    pub open: bool,
    pub name: String,
    pub agent_id: String,
    pub instruction: String,
    pub interval_minutes: u32,
    pub is_webhook: bool,
    pub webhook_secret: String,
    pub timezone: String,
}

impl Default for RoutineCalendarCreateCard {
    fn default() -> Self {
        Self {
            open: false,
            name: String::new(),
            agent_id: String::new(),
            instruction: String::new(),
            interval_minutes: 15,
            is_webhook: false,
            webhook_secret: String::new(),
            timezone: "UTC".to_string(),
        }
    }
}

impl RoutineCalendarCreateCard {
    pub fn validate(&self) -> Result<(), String> {
        let trimmed_name = self.name.trim();
        if trimmed_name.is_empty() || trimmed_name.chars().count() > 80 {
            return Err("routine name must be between 1 and 80 characters".to_string());
        }
        if !is_valid_timezone(&self.timezone) {
            return Err(format!("invalid routine timezone '{}'", self.timezone));
        }
        if !self.is_webhook && self.interval_minutes < ROUTINE_MIN_INTERVAL_MINUTES {
            return Err(format!(
                "routine interval {}m is below the minimum {} minutes (180s)",
                self.interval_minutes, ROUTINE_MIN_INTERVAL_MINUTES
            ));
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RoutineCalendarState {
    pub view: CalendarViewMode,
    pub anchor_date: String,
    pub timezone: String,
    pub items: Vec<RoutineCalendarItem>,
    pub create_card: RoutineCalendarCreateCard,
    pub selected_day: Option<String>,
}

impl Default for RoutineCalendarState {
    fn default() -> Self {
        Self::new(CalendarViewMode::Week, "2026-03-29", "UTC")
    }
}

impl RoutineCalendarState {
    pub fn new(view: CalendarViewMode, anchor_date: &str, timezone: &str) -> Self {
        Self {
            view,
            anchor_date: anchor_date.to_string(),
            timezone: timezone.to_string(),
            items: Vec::new(),
            create_card: RoutineCalendarCreateCard {
                timezone: timezone.to_string(),
                ..RoutineCalendarCreateCard::default()
            },
            selected_day: Some(anchor_date.to_string()),
        }
    }

    pub const fn window_days(&self) -> usize {
        WINDOW_DAYS
    }

    pub const fn upcoming_limit(&self) -> usize {
        UPCOMING_LIMIT
    }

    pub const fn visible_days(&self) -> usize {
        match self.view {
            CalendarViewMode::Day => 1,
            CalendarViewMode::Week => 7,
            CalendarViewMode::Month => 42,
        }
    }

    pub const fn day_cell_limit(&self) -> usize {
        match self.view {
            CalendarViewMode::Week => 4,
            CalendarViewMode::Month => 3,
            CalendarViewMode::Day => 10,
        }
    }

    pub fn parsed_anchor(&self) -> NaiveDate {
        NaiveDate::parse_from_str(self.anchor_date.trim(), "%Y-%m-%d")
            .unwrap_or_else(|_| NaiveDate::from_ymd_opt(2026, 3, 29).expect("valid fallback date"))
    }

    /// Detects whether `date` undergoes a DST clock change (23h or 25h local day) in `timezone`.
    pub fn is_dst_clock_change_day(date: NaiveDate, timezone: &str) -> bool {
        let Some(next) = date.checked_add_days(Days::new(1)) else {
            return false;
        };
        let d0 = date.format("%Y-%m-%d").to_string();
        let d1 = next.format("%Y-%m-%d").to_string();
        match (
            routine_once_instant(&d0, "00:00", timezone),
            routine_once_instant(&d1, "00:00", timezone),
        ) {
            (Ok(t0), Ok(t1)) => (t1 - t0) != 86_400_000,
            _ => false,
        }
    }

    /// Generates the exact DST-safe 44-day calendar prefetch window using `NaiveDate` arithmetic.
    pub fn build_window_dates(&self) -> Vec<String> {
        let anchor = self.parsed_anchor();
        let start = anchor.checked_sub_days(Days::new(7)).unwrap_or(anchor);
        (0..WINDOW_DAYS as u64)
            .filter_map(|offset| start.checked_add_days(Days::new(offset)))
            .map(|d| d.format("%Y-%m-%d").to_string())
            .collect()
    }

    /// Generates the DST-safe visible grid (`Day => 1`, `Week => 7`, `Month => 42` cells).
    pub fn build_grid(&self) -> Vec<RoutineCalendarDayCell> {
        let anchor = self.parsed_anchor();
        let anchor_ymd = anchor.format("%Y-%m-%d").to_string();
        let grid_start = match self.view {
            CalendarViewMode::Day => anchor,
            CalendarViewMode::Week => {
                let back = u64::from(anchor.weekday().num_days_from_monday());
                anchor.checked_sub_days(Days::new(back)).unwrap_or(anchor)
            }
            CalendarViewMode::Month => {
                let first = NaiveDate::from_ymd_opt(anchor.year(), anchor.month(), 1)
                    .unwrap_or(anchor);
                let back = u64::from(first.weekday().num_days_from_monday());
                first.checked_sub_days(Days::new(back)).unwrap_or(first)
            }
        };
        let cell_cap = self.day_cell_limit();
        let count = self.visible_days();
        let mut cells = Vec::with_capacity(count);
        for offset in 0..count as u64 {
            let date = grid_start.checked_add_days(Days::new(offset)).unwrap_or(anchor);
            let date_ymd = date.format("%Y-%m-%d").to_string();
            let matching: Vec<RoutineCalendarItem> = self
                .items
                .iter()
                .filter(|it| it.date_ymd == date_ymd)
                .cloned()
                .collect();
            let overflow_count = matching.len().saturating_sub(cell_cap);
            let visible_items = matching.into_iter().take(cell_cap).collect();
            cells.push(RoutineCalendarDayCell {
                is_anchor_day: date_ymd == anchor_ymd,
                date_ymd,
                day_of_month: date.day(),
                weekday_short: weekday_short_name(date.weekday()),
                in_current_month: date.month() == anchor.month(),
                is_dst_clock_change: Self::is_dst_clock_change_day(date, &self.timezone),
                items: visible_items,
                overflow_count,
            });
        }
        cells
    }

    pub fn upcoming_items(&self) -> Vec<&RoutineCalendarItem> {
        let mut sorted: Vec<&RoutineCalendarItem> = self
            .items
            .iter()
            .filter(|it| it.date_ymd.as_str() >= self.anchor_date.as_str())
            .collect();
        sorted.sort_by(|a, b| a.date_ymd.cmp(&b.date_ymd).then_with(|| a.time_hhmm.cmp(&b.time_hhmm)));
        sorted.truncate(UPCOMING_LIMIT);
        sorted
    }

    pub fn probe_summary(&self) -> serde_json::Value {
        let window = self.build_window_dates();
        let grid = self.build_grid();
        let dst_days: Vec<String> = grid
            .iter()
            .filter(|c| c.is_dst_clock_change)
            .map(|c| c.date_ymd.clone())
            .collect();
        serde_json::json!({
            "window_days": window.len(),
            "view": self.view.as_str(),
            "visible_days": grid.len(),
            "day_cell_limit": self.day_cell_limit(),
            "upcoming_limit": UPCOMING_LIMIT,
            "anchor_date": self.anchor_date,
            "timezone": self.timezone,
            "dst_clock_change_days": dst_days,
        })
    }
}

fn weekday_short_name(w: Weekday) -> &'static str {
    match w {
        Weekday::Mon => "Mon",
        Weekday::Tue => "Tue",
        Weekday::Wed => "Wed",
        Weekday::Thu => "Thu",
        Weekday::Fri => "Fri",
        Weekday::Sat => "Sat",
        Weekday::Sun => "Sun",
    }
}

fn routine_pill_color(status: RoutineStatusPill, palette: &OpenBotPalette) -> Rgba {
    match status {
        RoutineStatusPill::Active => pal_rgba(palette, "--openbot-accent", 0x007cf7),
        RoutineStatusPill::Paused => pal_rgba(palette, "--openbot-warning", 0xff9412),
        RoutineStatusPill::Failed => pal_rgba(palette, "--openbot-danger", 0xfd2f3b),
        RoutineStatusPill::Completed => pal_rgba(palette, "--openbot-success", 0x31cf76),
    }
}

/// Renders the `RoutineCalendar` surface with view switcher, DST clock-change markers,
/// capped day cells, upcoming rail, and inline `RoutineCalendarCreateCard`.
pub fn render_routine_calendar(
    state: &RoutineCalendarState,
    palette: &OpenBotPalette,
    _now_sec: f64,
) -> Div {
    let bg_canvas = pal_rgba(palette, "--openbot-bg-canvas", 0x1a1a1a);
    let bg_surface = pal_rgba(palette, "--openbot-bg-surface", 0x212121);
    let bg_control = pal_rgba(palette, "--openbot-bg-control", 0x212121);
    let border = pal_rgba(palette, "--openbot-border-grouped", 0x333335);
    let text_primary = pal_rgba(palette, "--openbot-text-primary", 0xffffff);
    let text_muted = pal_rgba(palette, "--openbot-text-muted", 0x979797);
    let accent = pal_rgba(palette, "--openbot-accent", 0x007cf7);
    let warning = pal_rgba(palette, "--openbot-warning", 0xff9412);

    let cells = state.build_grid();
    let upcoming = state.upcoming_items();

    let mut mode_tabs = div().flex().items_center().gap(px(4.0)).p(px(3.0)).rounded(px(10.0)).bg(bg_control);
    for mode in CalendarViewMode::ALL {
        let active = mode == state.view;
        mode_tabs = mode_tabs.child(
            div()
                .px(px(10.0))
                .py(px(4.0))
                .rounded(px(8.0))
                .bg(if active { accent } else { bg_control })
                .text_size(px(12.0))
                .text_color(if active {
                    pal_rgba(palette, "--openbot-text-on-accent", 0xffffff)
                } else {
                    text_muted
                })
                .child(mode.label()),
        );
    }

    let mut grid_el = div().flex_1().flex().flex_wrap().gap(px(8.0)).p(px(12.0));
    for cell in &cells {
        let mut cell_box = div()
            .w(px(if state.view == CalendarViewMode::Day { 480.0 } else { 142.0 }))
            .min_h(px(if state.view == CalendarViewMode::Month { 86.0 } else { 126.0 }))
            .p(px(8.0))
            .rounded(px(12.0))
            .bg(bg_surface)
            .border_1()
            .border_color(if cell.is_anchor_day { accent } else { border })
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
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(if cell.in_current_month { text_primary } else { text_muted })
                            .child(format!("{} {}", cell.weekday_short, cell.day_of_month)),
                    )
                    .when(cell.is_dst_clock_change, |el| {
                        el.child(
                            div()
                                .px(px(4.0))
                                .py(px(1.0))
                                .rounded(px(4.0))
                                .bg(pal_rgba(palette, "--openbot-warning-soft", 0x3a280a))
                                .text_size(px(9.0))
                                .text_color(warning)
                                .child("DST"),
                        )
                    }),
            );
        for item in &cell.items {
            let pill_c = routine_pill_color(item.status, palette);
            cell_box = cell_box.child(
                div()
                    .px(px(6.0))
                    .py(px(2.0))
                    .rounded(px(6.0))
                    .border_1()
                    .border_color(pill_c)
                    .text_size(px(10.0))
                    .text_color(text_primary)
                    .child(format!("{} · {}", item.time_hhmm, item.name)),
            );
        }
        if cell.overflow_count > 0 {
            cell_box = cell_box.child(
                div()
                    .text_size(px(10.0))
                    .text_color(text_muted)
                    .child(format!("+{} more", cell.overflow_count)),
            );
        }
        grid_el = grid_el.child(cell_box);
    }

    let mut upcoming_rail = div()
        .w(px(260.0))
        .p(px(12.0))
        .border_l_1()
        .border_color(border)
        .bg(bg_surface)
        .flex()
        .flex_col()
        .gap(px(8.0))
        .child(
            div()
                .text_size(px(13.0))
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(text_primary)
                .child(format!("Upcoming (max {UPCOMING_LIMIT})")),
        );
    for it in upcoming {
        let c = routine_pill_color(it.status, palette);
        upcoming_rail = upcoming_rail.child(
            div()
                .p(px(8.0))
                .rounded(px(10.0))
                .bg(bg_control)
                .border_1()
                .border_color(border)
                .flex()
                .flex_col()
                .gap(px(2.0))
                .child(div().text_size(px(12.0)).text_color(text_primary).child(it.name.clone()))
                .child(
                    div()
                        .text_size(px(10.0))
                        .text_color(c)
                        .child(format!("{} {} · {}", it.date_ymd, it.time_hhmm, it.status.as_str())),
                ),
        );
    }
    if state.create_card.open {
        upcoming_rail = upcoming_rail.child(
            div()
                .p(px(10.0))
                .rounded(px(12.0))
                .bg(bg_canvas)
                .border_1()
                .border_color(accent)
                .flex()
                .flex_col()
                .gap(px(4.0))
                .child(div().text_size(px(12.0)).font_weight(FontWeight::SEMIBOLD).text_color(text_primary).child("New Routine"))
                .child(div().text_size(px(11.0)).text_color(text_muted).child(if state.create_card.is_webhook {
                    "Trigger: Signed HMAC Webhook".to_string()
                } else {
                    format!("Cadence: every {}m (min {}m)", state.create_card.interval_minutes, ROUTINE_MIN_INTERVAL_MINUTES)
                })),
        );
    }

    div()
        .size_full()
        .flex()
        .flex_col()
        .bg(bg_canvas)
        .child(
            div()
                .h(px(48.0))
                .px(px(16.0))
                .border_b_1()
                .border_color(border)
                .bg(bg_surface)
                .flex()
                .items_center()
                .justify_between()
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(8.0))
                        .child(tg_icon_rgba(IconName::ClockSending, 16.0, accent))
                        .child(div().text_size(px(15.0)).font_weight(FontWeight::SEMIBOLD).text_color(text_primary).child("Schedule"))
                        .child(div().text_size(px(11.0)).text_color(text_muted).child(format!("{} · {}d window", state.timezone, WINDOW_DAYS))),
                )
                .child(mode_tabs),
        )
        .child(div().flex_1().min_h(px(0.0)).flex().child(grid_el).child(upcoming_rail))
}

// ============================================================================
// 2. TasksPanel (`TasksView` / `MAX_TASKS_PER_AGENT = 50`)
// ============================================================================

/// Hard per-agent concurrent/open side-task limit (`INPUT_LIMITS.agent_tasks`).
pub const MAX_TASKS_PER_AGENT: usize = 50;
pub const TASK_PANEL_DEFAULT_WIDTH_PX: f32 = 340.0;
pub const TASK_PANEL_MIN_WIDTH_PX: f32 = 240.0;
pub const TASK_PANEL_MAX_WIDTH_PX: f32 = 640.0;

/// Validates an agent's side-task count against the hard `50`-task per-agent maximum.
pub fn validate_task_count(count: usize) -> Result<usize, String> {
    if count > MAX_TASKS_PER_AGENT {
        return Err(format!(
            "agent task count {count} exceeds maximum limit of {MAX_TASKS_PER_AGENT} tasks per agent"
        ));
    }
    Ok(count)
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TaskPanelRow {
    pub id: String,
    pub agent_id: String,
    pub agent_name: String,
    pub title: String,
    pub state: AgentTaskState,
    pub outcome: Option<AgentTaskOutcome>,
    pub progress: Option<String>,
    pub error: Option<String>,
    pub updated_at: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TasksPanelState {
    pub rows: Vec<TaskPanelRow>,
    pub selected_task_id: Option<String>,
    pub filter_agent_id: Option<String>,
    pub split_width_px: f32,
    pub reduced_motion: bool,
}

impl Default for TasksPanelState {
    fn default() -> Self {
        Self {
            rows: Vec::new(),
            selected_task_id: None,
            filter_agent_id: None,
            split_width_px: TASK_PANEL_DEFAULT_WIDTH_PX,
            reduced_motion: false,
        }
    }
}

impl TasksPanelState {
    pub fn set_split_width(&mut self, width_px: f32) {
        if width_px.is_finite() {
            self.split_width_px = width_px.clamp(TASK_PANEL_MIN_WIDTH_PX, TASK_PANEL_MAX_WIDTH_PX);
        }
    }

    pub fn count_for_agent(&self, agent_id: &str) -> usize {
        self.rows.iter().filter(|r| r.agent_id == agent_id).count()
    }

    pub fn add_task_row(&mut self, row: TaskPanelRow) -> Result<(), String> {
        let next = self.count_for_agent(&row.agent_id) + 1;
        validate_task_count(next)?;
        self.rows.push(row);
        Ok(())
    }

    pub fn visible_rows(&self) -> Vec<&TaskPanelRow> {
        self.rows
            .iter()
            .filter(|r| {
                self.filter_agent_id
                    .as_deref()
                    .is_none_or(|aid| r.agent_id == aid)
            })
            .collect()
    }

    pub fn probe_summary(&self, count_override: Option<usize>) -> Result<serde_json::Value, String> {
        let count = count_override.unwrap_or(self.rows.len());
        validate_task_count(count)?;
        Ok(serde_json::json!({
            "max_per_agent": MAX_TASKS_PER_AGENT,
            "count": count,
            "split_width_px": self.split_width_px,
        }))
    }
}

/// Renders the cross-agent `TasksPanel` with resizable split inspection drawer.
pub fn render_tasks_panel(
    state: &TasksPanelState,
    palette: &OpenBotPalette,
    now_sec: f64,
) -> Div {
    let bg_canvas = pal_rgba(palette, "--openbot-bg-canvas", 0x1a1a1a);
    let bg_surface = pal_rgba(palette, "--openbot-bg-surface", 0x212121);
    let border = pal_rgba(palette, "--openbot-border-grouped", 0x333335);
    let text_primary = pal_rgba(palette, "--openbot-text-primary", 0xffffff);
    let text_muted = pal_rgba(palette, "--openbot-text-muted", 0x979797);
    let accent = pal_rgba(palette, "--openbot-accent", 0x007cf7);
    let success = pal_rgba(palette, "--openbot-success", 0x31cf76);
    let danger = pal_rgba(palette, "--openbot-danger", 0xfd2f3b);
    let is_dark = palette.scheme == OpenBotColorScheme::Dark;

    let visible = state.visible_rows();
    let mut list_col = div().flex_1().p(px(12.0)).flex().flex_col().gap(px(8.0));
    for row in &visible {
        let selected = state.selected_task_id.as_deref() == Some(row.id.as_str());
        let mut av = AgentAvatarProps::new(row.agent_id.as_str()).with_size(24.0);
        av.dark_scheme = is_dark;
        av.reduced_motion = state.reduced_motion;
        av.mood = match (row.state, row.outcome) {
            (AgentTaskState::Running, _) => AvatarMood::Working,
            (_, Some(AgentTaskOutcome::Failed)) => AvatarMood::Failed,
            (_, Some(AgentTaskOutcome::Completed)) => AvatarMood::Responded,
            _ => AvatarMood::Idle,
        };
        let badge_c = match (row.state, row.outcome) {
            (AgentTaskState::Running, _) => accent,
            (_, Some(AgentTaskOutcome::Failed)) => danger,
            (_, Some(AgentTaskOutcome::Completed)) => success,
            _ => text_muted,
        };
        list_col = list_col.child(
            div()
                .p(px(10.0))
                .rounded(px(12.0))
                .bg(bg_surface)
                .border_1()
                .border_color(if selected { accent } else { border })
                .flex()
                .items_center()
                .justify_between()
                .gap(px(10.0))
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(8.0))
                        .child(render_agent_avatar(&av, None, now_sec))
                        .child(
                            div()
                                .flex()
                                .flex_col()
                                .child(div().text_size(px(13.0)).font_weight(FontWeight::MEDIUM).text_color(text_primary).child(row.title.clone()))
                                .child(div().text_size(px(11.0)).text_color(text_muted).child(format!("{} · {}", row.agent_name, row.updated_at))),
                        ),
                )
                .child(
                    div()
                        .px(px(7.0))
                        .py(px(2.0))
                        .rounded(px(999.0))
                        .border_1()
                        .border_color(badge_c)
                        .text_size(px(10.0))
                        .text_color(badge_c)
                        .child(format!("{:?}", row.state)),
                ),
        );
    }

    let selected_row = state
        .selected_task_id
        .as_deref()
        .and_then(|id| state.rows.iter().find(|r| r.id == id));

    let detail_col = div()
        .w(px(state.split_width_px))
        .p(px(14.0))
        .border_l_1()
        .border_color(border)
        .bg(bg_surface)
        .flex()
        .flex_col()
        .gap(px(8.0))
        .child(div().text_size(px(13.0)).font_weight(FontWeight::SEMIBOLD).text_color(text_primary).child("Task Details"))
        .when_some(selected_row, |el, row| {
            el.child(div().text_size(px(12.0)).text_color(text_primary).child(row.title.clone()))
                .when_some(row.progress.as_ref(), |el2, p| {
                    el2.child(div().text_size(px(11.0)).text_color(text_muted).child(p.clone()))
                })
                .when_some(row.error.as_ref(), |el2, err| {
                    el2.child(div().text_size(px(11.0)).text_color(danger).child(err.clone()))
                })
        });

    div()
        .size_full()
        .flex()
        .flex_col()
        .bg(bg_canvas)
        .child(
            div()
                .h(px(48.0))
                .px(px(16.0))
                .border_b_1()
                .border_color(border)
                .bg(bg_surface)
                .flex()
                .items_center()
                .justify_between()
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(8.0))
                        .child(tg_icon_rgba(IconName::BotRobot, 16.0, accent))
                        .child(div().text_size(px(15.0)).font_weight(FontWeight::SEMIBOLD).text_color(text_primary).child("Tasks")),
                )
                .child(
                    div()
                        .text_size(px(11.0))
                        .text_color(text_muted)
                        .child(format!("{} tasks · cap {}/agent", visible.len(), MAX_TASKS_PER_AGENT)),
                ),
        )
        .child(div().flex_1().min_h(px(0.0)).flex().child(list_col).child(detail_col))
}

// ============================================================================
// 3. LibraryPanel (`ServerLibrary.tsx`)
// ============================================================================

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum LibraryViewMode {
    #[default]
    Mosaic,
    List,
}

impl LibraryViewMode {
    pub const ALL: [Self; 2] = [Self::Mosaic, Self::List];
    pub const ALL_STRS: [&'static str; 2] = ["mosaic", "list"];

    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Mosaic => "mosaic",
            Self::List => "list",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LibraryFolderNode {
    pub id: String,
    pub name: String,
    pub parent_id: Option<String>,
    pub item_count: usize,
    pub expanded: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LibraryFileItem {
    pub id: String,
    pub name: String,
    pub folder_id: Option<String>,
    pub mime_type: String,
    pub size_bytes: u64,
    pub modified_at: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StorageUsageMetrics {
    pub used_bytes: u64,
    pub quota_bytes: u64,
    pub agents_bytes: u64,
    pub media_bytes: u64,
    pub database_bytes: u64,
}

impl Default for StorageUsageMetrics {
    fn default() -> Self {
        Self {
            used_bytes: 1_280_000_000,
            quota_bytes: 10_000_000_000,
            agents_bytes: 640_000_000,
            media_bytes: 520_000_000,
            database_bytes: 120_000_000,
        }
    }
}

impl StorageUsageMetrics {
    pub fn used_ratio(&self) -> f32 {
        if self.quota_bytes == 0 {
            0.0
        } else {
            ((self.used_bytes as f64) / (self.quota_bytes as f64)).clamp(0.0, 1.0) as f32
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MachineResourcesMetrics {
    pub cpu_percent: f32,
    pub ram_used_gib: f32,
    pub ram_total_gib: f32,
    pub disk_used_gib: f32,
    pub disk_total_gib: f32,
}

impl Default for MachineResourcesMetrics {
    fn default() -> Self {
        Self {
            cpu_percent: 18.5,
            ram_used_gib: 6.4,
            ram_total_gib: 32.0,
            disk_used_gib: 142.0,
            disk_total_gib: 512.0,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LibraryMarqueeRect {
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LibraryPanelState {
    pub view_mode: LibraryViewMode,
    pub marquee_selection: bool,
    pub active_marquee: Option<LibraryMarqueeRect>,
    pub folders: Vec<LibraryFolderNode>,
    pub active_folder_id: Option<String>,
    pub files: Vec<LibraryFileItem>,
    pub selected_file_ids: Vec<String>,
    pub search_query: String,
    pub storage: StorageUsageMetrics,
    pub resources: MachineResourcesMetrics,
}

impl Default for LibraryPanelState {
    fn default() -> Self {
        Self {
            view_mode: LibraryViewMode::Mosaic,
            marquee_selection: true,
            active_marquee: None,
            folders: vec![
                LibraryFolderNode { id: "shared".into(), name: "Shared".into(), parent_id: None, item_count: 4, expanded: true },
                LibraryFolderNode { id: "exports".into(), name: "Exports".into(), parent_id: Some("shared".into()), item_count: 2, expanded: false },
            ],
            active_folder_id: None,
            files: Vec::new(),
            selected_file_ids: Vec::new(),
            search_query: String::new(),
            storage: StorageUsageMetrics::default(),
            resources: MachineResourcesMetrics::default(),
        }
    }
}

impl LibraryPanelState {
    pub fn filtered_files(&self) -> Vec<&LibraryFileItem> {
        let q = self.search_query.trim().to_ascii_lowercase();
        self.files
            .iter()
            .filter(|f| {
                self.active_folder_id
                    .as_deref()
                    .is_none_or(|fid| f.folder_id.as_deref() == Some(fid))
                    && (q.is_empty() || f.name.to_ascii_lowercase().contains(&q))
            })
            .collect()
    }

    pub fn probe_summary(&self) -> serde_json::Value {
        serde_json::json!({
            "views": LibraryViewMode::ALL_STRS,
            "active_view": self.view_mode.as_str(),
            "marquee_selection": self.marquee_selection,
            "storage_used_ratio": self.storage.used_ratio(),
            "folder_count": self.folders.len(),
            "file_count": self.files.len(),
        })
    }
}

/// Renders the `LibraryPanel` with folder tree, `StorageUsageBar`, `MachineResources`, and `Mosaic`/`List` view.
pub fn render_library_panel(state: &LibraryPanelState, palette: &OpenBotPalette) -> Div {
    let bg_canvas = pal_rgba(palette, "--openbot-bg-canvas", 0x1a1a1a);
    let bg_surface = pal_rgba(palette, "--openbot-bg-surface", 0x212121);
    let bg_control = pal_rgba(palette, "--openbot-bg-control", 0x212121);
    let border = pal_rgba(palette, "--openbot-border-grouped", 0x333335);
    let text_primary = pal_rgba(palette, "--openbot-text-primary", 0xffffff);
    let text_muted = pal_rgba(palette, "--openbot-text-muted", 0x979797);
    let accent = pal_rgba(palette, "--openbot-accent", 0x007cf7);
    let success = pal_rgba(palette, "--openbot-success", 0x31cf76);

    let mut folder_rail = div()
        .w(px(236.0))
        .p(px(12.0))
        .border_r_1()
        .border_color(border)
        .bg(bg_surface)
        .flex()
        .flex_col()
        .justify_between()
        .gap(px(12.0));

    let mut folder_list = div().flex().flex_col().gap(px(4.0)).child(
        div()
            .text_size(px(11.0))
            .font_weight(FontWeight::SEMIBOLD)
            .text_color(text_muted)
            .child("FOLDERS"),
    );
    for folder in &state.folders {
        let active = state.active_folder_id.as_deref() == Some(folder.id.as_str());
        let indent = if folder.parent_id.is_some() { 14.0 } else { 0.0 };
        folder_list = folder_list.child(
            div()
                .pl(px(8.0 + indent))
                .pr(px(8.0))
                .py(px(6.0))
                .rounded(px(8.0))
                .bg(if active { accent } else { bg_control })
                .flex()
                .items_center()
                .justify_between()
                .child(div().text_size(px(12.0)).text_color(text_primary).child(folder.name.clone()))
                .child(div().text_size(px(10.0)).text_color(text_muted).child(format!("{}", folder.item_count))),
        );
    }

    let storage_pct = (state.storage.used_ratio() * 100.0).round() as u32;
    let ram_pct = if state.resources.ram_total_gib > 0.0 {
        ((state.resources.ram_used_gib / state.resources.ram_total_gib) * 100.0).clamp(0.0, 100.0)
    } else {
        0.0
    };
    let disk_pct = if state.resources.disk_total_gib > 0.0 {
        ((state.resources.disk_used_gib / state.resources.disk_total_gib) * 100.0).clamp(0.0, 100.0)
    } else {
        0.0
    };

    let meters = div()
        .flex()
        .flex_col()
        .gap(px(8.0))
        .child(
            div()
                .p(px(10.0))
                .rounded(px(10.0))
                .bg(bg_canvas)
                .border_1()
                .border_color(border)
                .flex()
                .flex_col()
                .gap(px(4.0))
                .child(div().text_size(px(11.0)).font_weight(FontWeight::SEMIBOLD).text_color(text_primary).child(format!("Storage · {storage_pct}%")))
                .child(
                    div()
                        .h(px(6.0))
                        .rounded(px(999.0))
                        .bg(bg_control)
                        .child(div().h_full().w(px(1.8 * (storage_pct as f32))).rounded(px(999.0)).bg(accent)),
                ),
        )
        .child(
            div()
                .p(px(10.0))
                .rounded(px(10.0))
                .bg(bg_canvas)
                .border_1()
                .border_color(border)
                .flex()
                .flex_col()
                .gap(px(3.0))
                .child(div().text_size(px(11.0)).font_weight(FontWeight::SEMIBOLD).text_color(text_primary).child("Machine Resources"))
                .child(div().text_size(px(10.0)).text_color(text_muted).child(format!("CPU {:.0}% · RAM {:.0}% · Disk {:.0}%", state.resources.cpu_percent, ram_pct, disk_pct)))
                .child(div().h(px(4.0)).rounded(px(999.0)).bg(bg_control).child(div().h_full().w(px(1.8 * state.resources.cpu_percent.clamp(0.0, 100.0))).rounded(px(999.0)).bg(success))),
        );
    folder_rail = folder_rail.child(folder_list).child(meters);

    let files = state.filtered_files();
    let mut files_area = div().relative().flex_1().p(px(14.0)).flex().gap(px(10.0));
    files_area = if state.view_mode == LibraryViewMode::Mosaic {
        files_area.flex_wrap()
    } else {
        files_area.flex_col()
    };
    for file in files {
        let selected = state.selected_file_ids.iter().any(|id| id == &file.id);
        let tone = file_reference_tone(&file.name);
        let (fg_hex, _) = tone.colors(palette.scheme);
        let tone_rgba = parse_css_color_to_rgba(fg_hex).unwrap_or(accent);
        let badge = file_reference_badge(&file.name).unwrap_or_else(|| "FILE".to_string());
        files_area = files_area.child(
            div()
                .when(state.view_mode == LibraryViewMode::Mosaic, |el| el.w(px(164.0)).h(px(110.0)).flex_col().justify_between())
                .when(state.view_mode == LibraryViewMode::List, |el| el.w_full().items_center().justify_between())
                .p(px(10.0))
                .rounded(px(12.0))
                .bg(bg_surface)
                .border_1()
                .border_color(if selected { accent } else { border })
                .flex()
                .gap(px(6.0))
                .child(div().px(px(6.0)).py(px(2.0)).rounded(px(6.0)).border_1().border_color(tone_rgba).text_size(px(10.0)).text_color(tone_rgba).child(badge))
                .child(div().text_size(px(12.0)).text_color(text_primary).child(file.name.clone()))
                .child(div().text_size(px(10.0)).text_color(text_muted).child(format!("{} B", file.size_bytes))),
        );
    }
    if let Some(m) = state.active_marquee {
        files_area = files_area.child(
            div()
                .absolute()
                .left(px(m.x))
                .top(px(m.y))
                .w(px(m.width))
                .h(px(m.height))
                .rounded(px(6.0))
                .border_1()
                .border_color(accent)
                .bg(pal_rgba(palette, "--openbot-accent-soft", 0x102a46)),
        );
    }

    div()
        .size_full()
        .flex()
        .flex_col()
        .bg(bg_canvas)
        .child(
            div()
                .h(px(48.0))
                .px(px(16.0))
                .border_b_1()
                .border_color(border)
                .bg(bg_surface)
                .flex()
                .items_center()
                .justify_between()
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(8.0))
                        .child(tg_icon_rgba(IconName::FolderAll, 16.0, accent))
                        .child(div().text_size(px(15.0)).font_weight(FontWeight::SEMIBOLD).text_color(text_primary).child("Library")),
                )
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(6.0))
                        .child(div().text_size(px(11.0)).text_color(text_muted).child(format!("View: {}", state.view_mode.as_str()))),
                ),
        )
        .child(div().flex_1().min_h(px(0.0)).flex().child(folder_rail).child(files_area))
}

// ============================================================================
// 4. MarketplacePage & `fishing` Alpha Fallback (`Marketplace.tsx`)
// ============================================================================

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum MarketplaceTab {
    #[default]
    Apps,
    Agents,
    Skills,
}

impl MarketplaceTab {
    pub const ALL: [Self; 3] = [Self::Apps, Self::Agents, Self::Skills];
    pub const ALL_STRS: [&'static str; 3] = ["apps", "agents", "skills"];

    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Apps => "apps",
            Self::Agents => "agents",
            Self::Skills => "skills",
        }
    }

    pub const fn label(self) -> &'static str {
        match self {
            Self::Apps => "Apps",
            Self::Agents => "Agents",
            Self::Skills => "Skills",
        }
    }

    pub fn parse(raw: &str) -> Result<Self, String> {
        match raw.trim().to_ascii_lowercase().as_str() {
            "apps" => Ok(Self::Apps),
            "agents" => Ok(Self::Agents),
            "skills" => Ok(Self::Skills),
            other => Err(format!(
                "invalid marketplace tab '{other}': expected 'apps', 'agents', or 'skills'"
            )),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum MarketplaceSourceFilter {
    #[default]
    All,
    Verified,
    Public,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MarketplaceBreadcrumb {
    pub kind: String,
    pub id: String,
    pub label: String,
    pub scroll_offset_px: f32,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MarketplaceCatalogItem {
    pub id: String,
    pub tab: MarketplaceTab,
    pub name: String,
    pub author: String,
    pub summary: String,
    pub category: String,
    pub verified: bool,
    pub installs: u64,
    pub gradient_hex: u32,
}

/// Resolves the `fishing` empty-state plush asset via M2 `MarketplaceNoMatchPlushState`:
/// returns `"fishing-still.webp"` when `reduced_motion || video_alpha_pixel >= 128`,
/// otherwise `"fishing.webm"`.
pub fn resolve_no_match_asset(video_alpha_pixel: u8, reduced_motion: bool) -> &'static str {
    let mut plush = MarketplaceNoMatchPlushState::new(reduced_motion);
    plush.on_loaded_top_left_alpha(video_alpha_pixel);
    plush.active_asset_name()
}

#[derive(Debug, Clone, PartialEq)]
pub struct MarketplacePageState {
    pub active_tab: MarketplaceTab,
    pub breadcrumbs: Vec<MarketplaceBreadcrumb>,
    pub search_query: String,
    pub source_filter: MarketplaceSourceFilter,
    pub items: Vec<MarketplaceCatalogItem>,
    pub no_match_plush: MarketplaceNoMatchPlushState,
}

impl Default for MarketplacePageState {
    fn default() -> Self {
        Self::new(false)
    }
}

impl MarketplacePageState {
    pub fn new(reduced_motion: bool) -> Self {
        Self {
            active_tab: MarketplaceTab::Apps,
            breadcrumbs: vec![MarketplaceBreadcrumb {
                kind: "browse".to_string(),
                id: "root".to_string(),
                label: "Marketplace".to_string(),
                scroll_offset_px: 0.0,
            }],
            search_query: String::new(),
            source_filter: MarketplaceSourceFilter::All,
            items: vec![
                MarketplaceCatalogItem {
                    id: "app-github".into(),
                    tab: MarketplaceTab::Apps,
                    name: "GitHub MCP".into(),
                    author: "OpenBot".into(),
                    summary: "PR reviews, issues, and workflow triggers".into(),
                    category: "Developer".into(),
                    verified: true,
                    installs: 14_820,
                    gradient_hex: 0x007cf7,
                },
                MarketplaceCatalogItem {
                    id: "agent-triage".into(),
                    tab: MarketplaceTab::Agents,
                    name: "Release Triage Bot".into(),
                    author: "OpenBot".into(),
                    summary: "Automated changelog & regression auditor".into(),
                    category: "Engineering".into(),
                    verified: true,
                    installs: 8_410,
                    gradient_hex: 0xd6adf2,
                },
                MarketplaceCatalogItem {
                    id: "skill-git".into(),
                    tab: MarketplaceTab::Skills,
                    name: "Git Hygiene".into(),
                    author: "Community".into(),
                    summary: "Safe rebase, bisect, and diff verification".into(),
                    category: "Git".into(),
                    verified: true,
                    installs: 21_300,
                    gradient_hex: 0x31cf76,
                },
            ],
            no_match_plush: MarketplaceNoMatchPlushState::new(reduced_motion),
        }
    }

    pub fn push_breadcrumb(&mut self, kind: &str, id: &str, label: &str, scroll_offset_px: f32) {
        self.breadcrumbs.push(MarketplaceBreadcrumb {
            kind: kind.to_string(),
            id: id.to_string(),
            label: label.to_string(),
            scroll_offset_px,
        });
    }

    pub fn pop_breadcrumb(&mut self) -> Option<MarketplaceBreadcrumb> {
        if self.breadcrumbs.len() > 1 {
            self.breadcrumbs.pop()
        } else {
            None
        }
    }

    pub fn apply_alpha_probe(&mut self, video_alpha_pixel: u8, reduced_motion: bool) -> &'static str {
        self.no_match_plush.set_reduced_motion(reduced_motion);
        self.no_match_plush.on_loaded_top_left_alpha(video_alpha_pixel);
        self.no_match_plush.active_asset_name()
    }

    pub fn filtered_items(&self) -> Vec<&MarketplaceCatalogItem> {
        let q = self.search_query.trim().to_ascii_lowercase();
        self.items
            .iter()
            .filter(|item| {
                item.tab == self.active_tab
                    && match self.source_filter {
                        MarketplaceSourceFilter::All => true,
                        MarketplaceSourceFilter::Verified => item.verified,
                        MarketplaceSourceFilter::Public => !item.verified,
                    }
                    && (q.is_empty()
                        || item.name.to_ascii_lowercase().contains(&q)
                        || item.summary.to_ascii_lowercase().contains(&q)
                        || item.category.to_ascii_lowercase().contains(&q))
            })
            .collect()
    }

    pub fn probe_summary(&self, video_alpha_pixel: u8, reduced_motion: bool) -> serde_json::Value {
        let no_match_asset = resolve_no_match_asset(video_alpha_pixel, reduced_motion);
        serde_json::json!({
            "tabs": MarketplaceTab::ALL_STRS,
            "active_tab": self.active_tab.as_str(),
            "query": self.search_query,
            "match_count": self.filtered_items().len(),
            "no_match_asset": no_match_asset,
        })
    }
}

/// Renders the 3-tab `MarketplacePage` with breadcrumbs, verified/public filters, and M2 `fishing` no-match plush.
pub fn render_marketplace_page(state: &MarketplacePageState, palette: &OpenBotPalette) -> Div {
    let bg_canvas = pal_rgba(palette, "--openbot-bg-canvas", 0x1a1a1a);
    let bg_surface = pal_rgba(palette, "--openbot-bg-surface", 0x212121);
    let bg_control = pal_rgba(palette, "--openbot-bg-control", 0x212121);
    let border = pal_rgba(palette, "--openbot-border-grouped", 0x333335);
    let text_primary = pal_rgba(palette, "--openbot-text-primary", 0xffffff);
    let text_muted = pal_rgba(palette, "--openbot-text-muted", 0x979797);
    let accent = pal_rgba(palette, "--openbot-accent", 0x007cf7);

    let mut tabs_row = div().flex().items_center().gap(px(4.0)).p(px(3.0)).rounded(px(10.0)).bg(bg_control);
    for tab in MarketplaceTab::ALL {
        let active = tab == state.active_tab;
        tabs_row = tabs_row.child(
            div()
                .px(px(12.0))
                .py(px(4.0))
                .rounded(px(8.0))
                .bg(if active { accent } else { bg_control })
                .text_size(px(12.0))
                .text_color(if active {
                    pal_rgba(palette, "--openbot-text-on-accent", 0xffffff)
                } else {
                    text_muted
                })
                .child(tab.label()),
        );
    }

    let crumb_text = state
        .breadcrumbs
        .iter()
        .map(|b| b.label.as_str())
        .collect::<Vec<_>>()
        .join(" / ");

    let matches = state.filtered_items();
    let body = if matches.is_empty() {
        div()
            .flex_1()
            .flex()
            .flex_col()
            .items_center()
            .justify_center()
            .gap(px(10.0))
            .child(state.no_match_plush.render_element(160.0))
            .child(div().text_size(px(14.0)).font_weight(FontWeight::SEMIBOLD).text_color(text_primary).child("No matches found"))
            .child(div().text_size(px(11.0)).text_color(text_muted).child(format!("Asset: {}", state.no_match_plush.active_asset_name())))
    } else {
        let mut grid = div().flex_1().p(px(16.0)).flex().flex_wrap().gap(px(12.0));
        for item in matches {
            grid = grid.child(
                div()
                    .w(px(240.0))
                    .rounded(px(14.0))
                    .bg(bg_surface)
                    .border_1()
                    .border_color(border)
                    .overflow_hidden()
                    .flex()
                    .flex_col()
                    .child(div().h(px(6.0)).w_full().bg(rgb(item.gradient_hex)))
                    .child(
                        div()
                            .p(px(12.0))
                            .flex()
                            .flex_col()
                            .gap(px(4.0))
                            .child(
                                div()
                                    .flex()
                                    .items_center()
                                    .justify_between()
                                    .child(div().text_size(px(13.0)).font_weight(FontWeight::SEMIBOLD).text_color(text_primary).child(item.name.clone()))
                                    .when(item.verified, |el| el.child(tg_icon_rgba(IconName::Verified, 14.0, accent))),
                            )
                            .child(div().text_size(px(11.0)).text_color(text_muted).child(item.summary.clone()))
                            .child(div().text_size(px(10.0)).text_color(text_muted).child(format!("{} · {} installs", item.author, item.installs))),
                    ),
            );
        }
        grid
    };

    div()
        .size_full()
        .flex()
        .flex_col()
        .bg(bg_canvas)
        .child(
            div()
                .h(px(48.0))
                .px(px(16.0))
                .border_b_1()
                .border_color(border)
                .bg(bg_surface)
                .flex()
                .items_center()
                .justify_between()
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(8.0))
                        .child(tg_icon_rgba(IconName::PremiumStar, 16.0, accent))
                        .child(div().text_size(px(14.0)).font_weight(FontWeight::SEMIBOLD).text_color(text_primary).child(crumb_text)),
                )
                .child(tabs_row),
        )
        .child(body)
}

// ============================================================================
// 5. CrmPage (13 SQLite Tables + `crm_propose` ReviewPanel) (`CrmView.tsx`)
// ============================================================================

/// Exact number of backing SQLite CRM tables.
pub const CRM_TABLES_COUNT: usize = 13;

/// All 13 canonical SQLite CRM tables (`features_and_data_spec.md §6.2`).
pub const CRM_SQLITE_TABLES: [&str; CRM_TABLES_COUNT] = [
    "crm_customers",
    "crm_statuses",
    "crm_tags",
    "crm_customer_tags",
    "crm_plans",
    "crm_payments",
    "crm_activity",
    "crm_proposals",
    "crm_sources",
    "crm_source_ignored",
    "crm_private_fields",
    "crm_agent_access",
    "crm_counters",
];

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum CrmPageTab {
    #[default]
    Customers,
    Payments,
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum CrmDrawerMode {
    #[default]
    Closed,
    CustomerDetail(String),
    ReviewQueue,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CrmPageState {
    pub active_tab: CrmPageTab,
    pub drawer: CrmDrawerMode,
    pub search_query: String,
    pub connectors_modal_open: bool,
    pub today_ymd: String,
    pub overview: CrmOverview,
    pub customers: Vec<CrmCustomer>,
    pub payments: Vec<CrmPayment>,
    pub activities: Vec<CrmActivity>,
}

impl Default for CrmPageState {
    fn default() -> Self {
        Self {
            active_tab: CrmPageTab::Customers,
            drawer: CrmDrawerMode::Closed,
            search_query: String::new(),
            connectors_modal_open: false,
            today_ymd: "2026-03-29".to_string(),
            overview: CrmOverview {
                statuses: Vec::new(),
                tags: Vec::new(),
                private_fields: Vec::new(),
                agent_ids: Vec::new(),
                proposals: Vec::new(),
                sources: Vec::new(),
            },
            customers: Vec::new(),
            payments: Vec::new(),
            activities: Vec::new(),
        }
    }
}

impl CrmPageState {
    pub fn overdue_payments_count(&self) -> usize {
        self.payments
            .iter()
            .filter(|p| p.paid_at.is_none() && p.due_on.as_str() < self.today_ymd.as_str())
            .count()
    }

    pub fn pending_proposals_count(&self) -> usize {
        self.overview.proposals.len()
    }

    pub fn is_field_private(&self, field: CrmPrivateField) -> bool {
        self.overview.private_fields.contains(&field)
    }

    pub fn redacted_field_display(&self, field: CrmPrivateField, raw: &str) -> String {
        if self.is_field_private(field) && !raw.is_empty() {
            "•••••• (private)".to_string()
        } else {
            raw.to_string()
        }
    }

    pub fn approve_proposal(&mut self, proposal_id: &str) -> Option<CrmProposal> {
        let idx = self.overview.proposals.iter().position(|p| p.id == proposal_id)?;
        Some(self.overview.proposals.remove(idx))
    }

    pub fn reject_proposal(&mut self, proposal_id: &str) -> Option<CrmProposal> {
        let idx = self.overview.proposals.iter().position(|p| p.id == proposal_id)?;
        Some(self.overview.proposals.remove(idx))
    }

    pub fn open_customer_drawer(&mut self, customer_id: impl Into<String>) {
        self.drawer = CrmDrawerMode::CustomerDetail(customer_id.into());
    }

    pub fn open_review_drawer(&mut self) {
        self.drawer = CrmDrawerMode::ReviewQueue;
    }

    pub fn close_drawer(&mut self) {
        self.drawer = CrmDrawerMode::Closed;
    }

    pub fn probe_summary(&self) -> serde_json::Value {
        serde_json::json!({
            "tables_count": CRM_TABLES_COUNT,
            "tables": CRM_SQLITE_TABLES,
            "overdue_payments": self.overdue_payments_count(),
            "pending_proposals": self.pending_proposals_count(),
            "connectors_modal_open": self.connectors_modal_open,
        })
    }
}

/// Renders `CrmPage` with `Customers`/`Payments` SlidingTabs (`data-late` overdue badge),
/// Stripe Connectors dialog, `CrmCustomerPanel` (with private-field redaction), and `ReviewPanel`.
pub fn render_crm_page(state: &CrmPageState, palette: &OpenBotPalette) -> Div {
    let bg_canvas = pal_rgba(palette, "--openbot-bg-canvas", 0x1a1a1a);
    let bg_surface = pal_rgba(palette, "--openbot-bg-surface", 0x212121);
    let bg_control = pal_rgba(palette, "--openbot-bg-control", 0x212121);
    let border = pal_rgba(palette, "--openbot-border-grouped", 0x333335);
    let text_primary = pal_rgba(palette, "--openbot-text-primary", 0xffffff);
    let text_muted = pal_rgba(palette, "--openbot-text-muted", 0x979797);
    let accent = pal_rgba(palette, "--openbot-accent", 0x007cf7);
    let danger = pal_rgba(palette, "--openbot-danger", 0xfd2f3b);
    let success = pal_rgba(palette, "--openbot-success", 0x31cf76);

    let overdue = state.overdue_payments_count();
    let pending = state.pending_proposals_count();

    let tabs = div()
        .flex()
        .items_center()
        .gap(px(4.0))
        .p(px(3.0))
        .rounded(px(10.0))
        .bg(bg_control)
        .child(
            div()
                .px(px(10.0))
                .py(px(4.0))
                .rounded(px(8.0))
                .bg(if state.active_tab == CrmPageTab::Customers { accent } else { bg_control })
                .text_size(px(12.0))
                .text_color(text_primary)
                .child(format!("Customers ({})", state.customers.len())),
        )
        .child(
            div()
                .px(px(10.0))
                .py(px(4.0))
                .rounded(px(8.0))
                .bg(if state.active_tab == CrmPageTab::Payments { accent } else { bg_control })
                .flex()
                .items_center()
                .gap(px(5.0))
                .child(div().text_size(px(12.0)).text_color(text_primary).child("Payments"))
                .when(overdue > 0, |el| {
                    el.child(
                        div()
                            .px(px(5.0))
                            .py(px(1.0))
                            .rounded(px(999.0))
                            .bg(danger)
                            .text_size(px(10.0))
                            .text_color(rgb(0xffffff))
                            .child(format!("{overdue} late")),
                    )
                }),
        );

    let mut table_body = div().flex_1().p(px(14.0)).flex().flex_col().gap(px(8.0));
    match state.active_tab {
        CrmPageTab::Customers => {
            for cust in &state.customers {
                let email = state.redacted_field_display(CrmPrivateField::ContactEmail, &cust.contact_email);
                table_body = table_body.child(
                    div()
                        .p(px(10.0))
                        .rounded(px(12.0))
                        .bg(bg_surface)
                        .border_1()
                        .border_color(border)
                        .flex()
                        .items_center()
                        .justify_between()
                        .child(
                            div()
                                .flex()
                                .flex_col()
                                .child(div().text_size(px(13.0)).font_weight(FontWeight::MEDIUM).text_color(text_primary).child(cust.name.clone()))
                                .child(div().text_size(px(11.0)).text_color(text_muted).child(format!("{email} · {}", cust.status_id))),
                        )
                        .child(
                            div()
                                .text_size(px(11.0))
                                .text_color(if cust.late_minor > 0 { danger } else { success })
                                .child(format!("{} {:.2} paid", cust.currency, (cust.paid_minor as f64) / 100.0)),
                        ),
                );
            }
        }
        CrmPageTab::Payments => {
            for pay in &state.payments {
                let is_late = pay.paid_at.is_none() && pay.due_on.as_str() < state.today_ymd.as_str();
                table_body = table_body.child(
                    div()
                        .p(px(10.0))
                        .rounded(px(12.0))
                        .bg(bg_surface)
                        .border_1()
                        .border_color(if is_late { danger } else { border })
                        .flex()
                        .items_center()
                        .justify_between()
                        .child(
                            div()
                                .flex()
                                .flex_col()
                                .child(div().text_size(px(13.0)).text_color(text_primary).child(pay.label.clone()))
                                .child(div().text_size(px(11.0)).text_color(text_muted).child(format!("Due {}", pay.due_on))),
                        )
                        .child(
                            div()
                                .text_size(px(12.0))
                                .font_weight(FontWeight::SEMIBOLD)
                                .text_color(if is_late { danger } else { text_primary })
                                .child(format!("{} {:.2}", pay.currency, (pay.amount_minor as f64) / 100.0)),
                        ),
                );
            }
        }
    }

    let drawer_el = match &state.drawer {
        CrmDrawerMode::Closed => None,
        CrmDrawerMode::CustomerDetail(cid) => {
            let customer = state.customers.iter().find(|c| &c.id == cid);
            let mut panel = div()
                .w(px(320.0))
                .p(px(14.0))
                .border_l_1()
                .border_color(border)
                .bg(bg_surface)
                .flex()
                .flex_col()
                .gap(px(8.0))
                .child(div().text_size(px(14.0)).font_weight(FontWeight::SEMIBOLD).text_color(text_primary).child("Customer Profile"));
            if let Some(c) = customer {
                panel = panel
                    .child(div().text_size(px(13.0)).text_color(text_primary).child(c.name.clone()))
                    .child(div().text_size(px(11.0)).text_color(text_muted).child(format!("Email: {}", state.redacted_field_display(CrmPrivateField::ContactEmail, &c.contact_email))))
                    .child(div().text_size(px(11.0)).text_color(text_muted).child(format!("Phone: {}", state.redacted_field_display(CrmPrivateField::Phone, &c.phone))))
                    .child(div().text_size(px(11.0)).text_color(text_muted).child(format!("Private fields locked: {}", state.overview.private_fields.len())));
            }
            Some(panel)
        }
        CrmDrawerMode::ReviewQueue => {
            let mut panel = div()
                .w(px(340.0))
                .p(px(14.0))
                .border_l_1()
                .border_color(border)
                .bg(bg_surface)
                .flex()
                .flex_col()
                .gap(px(8.0))
                .child(div().text_size(px(14.0)).font_weight(FontWeight::SEMIBOLD).text_color(text_primary).child(format!("Review Queue (crm_propose · {pending})")));
            for prop in &state.overview.proposals {
                panel = panel.child(
                    div()
                        .p(px(10.0))
                        .rounded(px(10.0))
                        .bg(bg_control)
                        .border_1()
                        .border_color(border)
                        .flex()
                        .flex_col()
                        .gap(px(6.0))
                        .child(div().text_size(px(12.0)).text_color(text_primary).child(prop.reason.clone()))
                        .child(div().text_size(px(10.0)).text_color(text_muted).child(format!("Proposed by {} · {}", prop.agent_id, prop.created_at)))
                        .child(
                            div()
                                .flex()
                                .gap(px(6.0))
                                .child(div().px(px(8.0)).py(px(3.0)).rounded(px(6.0)).bg(success).text_size(px(11.0)).text_color(rgb(0x141414)).child("Approve"))
                                .child(div().px(px(8.0)).py(px(3.0)).rounded(px(6.0)).bg(danger).text_size(px(11.0)).text_color(rgb(0xffffff)).child("Reject")),
                        ),
                );
            }
            Some(panel)
        }
    };

    div()
        .relative()
        .size_full()
        .flex()
        .flex_col()
        .bg(bg_canvas)
        .child(
            div()
                .h(px(48.0))
                .px(px(16.0))
                .border_b_1()
                .border_color(border)
                .bg(bg_surface)
                .flex()
                .items_center()
                .justify_between()
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(10.0))
                        .child(tg_icon_rgba(IconName::ContactsUsers, 16.0, accent))
                        .child(div().text_size(px(15.0)).font_weight(FontWeight::SEMIBOLD).text_color(text_primary).child(format!("CRM · {CRM_TABLES_COUNT} tables")))
                        .child(tabs),
                )
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(8.0))
                        .child(div().px(px(8.0)).py(px(4.0)).rounded(px(8.0)).bg(bg_control).border_1().border_color(border).text_size(px(11.0)).text_color(text_primary).child(format!("Connectors ({})", state.overview.sources.len())))
                        .child(div().px(px(8.0)).py(px(4.0)).rounded(px(8.0)).bg(if pending > 0 { accent } else { bg_control }).text_size(px(11.0)).text_color(text_primary).child(format!("Review ({pending})"))),
                ),
        )
        .child(div().flex_1().min_h(px(0.0)).flex().child(table_body).when_some(drawer_el, |el, d| el.child(d)))
        .when(state.connectors_modal_open, |el| {
            el.child(
                div()
                    .absolute()
                    .right(px(20.0))
                    .top(px(58.0))
                    .w(px(300.0))
                    .p(px(14.0))
                    .rounded(px(16.0))
                    .bg(bg_surface)
                    .border_1()
                    .border_color(accent)
                    .flex()
                    .flex_col()
                    .gap(px(6.0))
                    .child(div().text_size(px(13.0)).font_weight(FontWeight::SEMIBOLD).text_color(text_primary).child("Stripe Connectors"))
                    .child(div().text_size(px(11.0)).text_color(text_muted).child("Restricted read-only rk_* API keys only (sk_* rejected).")),
            )
        })
}

// ============================================================================
// 6. Usage & Credits (`Usage` / `Credits` / `rolling-price.tsx`)
// ============================================================================

/// Provider chart accent colors (`packages/brand/src/tokens*.css`).
pub const PROVIDER_COLOR_CODEX: u32 = 0xd6adf2;
pub const PROVIDER_COLOR_CLAUDE: u32 = 0xd97757;
pub const PROVIDER_COLOR_GROK: u32 = 0x6bc7d9;

/// Resolves a deterministic visual accent color for any real AI provider ID.
pub fn provider_usage_accent_hex(provider_id: &str) -> u32 {
    match provider_id.trim().to_ascii_lowercase().as_str() {
        "codex" | "openai" | "openai_responses" => PROVIDER_COLOR_CODEX,
        "claude" | "anthropic" => PROVIDER_COLOR_CLAUDE,
        "grok" | "xai" => PROVIDER_COLOR_GROK,
        "gemini" | "google" | "antigravity" => 0x42a0ff,
        "deepseek" => 0x4f7bff,
        "openrouter" => 0x7c6ff7,
        "qwen" | "dashscope" => 0x7259f0,
        "kimi" | "moonshot" => 0x31cf76,
        "glm" | "zhipu" => 0x3ba6ff,
        "minimax" => 0xf06b8e,
        "mistral" => 0xf08a24,
        "local" | "local_hermes" | "ollama" | "lmstudio" => 0x8c96a4,
        other => {
            const FALLBACKS: [u32; 6] = [
                0x42a0ff, 0x31cf76, 0xf08a24, 0xd6adf2, 0x6bc7d9, 0xd97757,
            ];
            let hash = other
                .bytes()
                .fold(0usize, |acc, b| acc.wrapping_mul(31).wrapping_add(b as usize));
            FALLBACKS[hash % FALLBACKS.len()]
        }
    }
}

fn canonicalize_openbot_provider(raw: &str) -> (String, String) {
    let p = raw.trim().to_ascii_lowercase();
    match p.as_str() {
        "codex" | "openai" | "openai_responses" => ("codex".into(), "OpenAI / Codex".into()),
        "claude" | "anthropic" => ("claude".into(), "Claude".into()),
        "grok" | "xai" => ("grok".into(), "Grok".into()),
        "gemini" | "google" | "antigravity" => ("gemini".into(), "Gemini".into()),
        "deepseek" => ("deepseek".into(), "DeepSeek".into()),
        "openrouter" => ("openrouter".into(), "OpenRouter".into()),
        "qwen" => ("qwen".into(), "Qwen".into()),
        "kimi" => ("kimi".into(), "Kimi".into()),
        "glm" => ("glm".into(), "GLM".into()),
        "minimax" => ("minimax".into(), "MiniMax".into()),
        "mistral" => ("mistral".into(), "Mistral".into()),
        "local" | "local_hermes" | "ollama" | "lmstudio" => ("local".into(), "Local".into()),
        "" => ("other".into(), "Other".into()),
        other => {
            let mut chars = other.chars();
            let display = match chars.next() {
                Some(first) => format!("{}{}", first.to_ascii_uppercase(), chars.as_str()),
                None => "Other".into(),
            };
            (other.to_string(), display)
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProviderUsageSeries {
    pub provider_id: String,
    pub display_name: String,
    pub accent_hex: u32,
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cached_tokens: u64,
    pub cost_usd: f64,
    pub quota_used_ratio: f32,
    pub daily_bars: Vec<f32>,
    #[serde(default)]
    pub turns: u64,
    #[serde(default)]
    pub models: Vec<String>,
    #[serde(default)]
    pub has_explicit_quota: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CreditLedgerEntry {
    pub id: String,
    pub label: String,
    pub delta_credits: i64,
    pub created_at: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UsageAndCreditsUiAction {
    SelectRangeDays(u32),
    Refresh,
    BuyCredits,
    MoveCredits,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UsageAndCreditsState {
    pub series: Vec<ProviderUsageSeries>,
    pub wallet_balance_credits: i64,
    pub pending_hold_credits: i64,
    pub selected_range_days: u32,
    pub ledger: Vec<CreditLedgerEntry>,
    #[serde(default)]
    pub date_labels: Vec<String>,
    #[serde(default)]
    pub wallet_ton_balance: f64,
    #[serde(default)]
    pub total_turns: u64,
    #[serde(default)]
    pub is_loading: bool,
    #[serde(default)]
    pub has_synced: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sync_error: Option<String>,
}

impl Default for UsageAndCreditsState {
    fn default() -> Self {
        Self {
            series: Vec::new(),
            wallet_balance_credits: 0,
            pending_hold_credits: 0,
            selected_range_days: 7,
            ledger: Vec::new(),
            date_labels: Self::utc_date_window(7),
            wallet_ton_balance: 0.0,
            total_turns: 0,
            is_loading: false,
            has_synced: false,
            sync_error: None,
        }
    }
}

impl UsageAndCreditsState {
    /// Returns the last `range_days` UTC calendar dates (`YYYY-MM-DD`) ending today inclusive.
    pub fn utc_date_window(range_days: u32) -> Vec<String> {
        let days = range_days.clamp(1, 90);
        let today = Utc::now().date_naive();
        (0..days)
            .rev()
            .map(|offset| (today - ChronoDuration::days(i64::from(offset))).format("%Y-%m-%d").to_string())
            .collect()
    }

    pub fn total_tokens(&self) -> u64 {
        self.series
            .iter()
            .map(|s| s.input_tokens.saturating_add(s.output_tokens))
            .fold(0u64, u64::saturating_add)
    }

    pub fn total_cached_tokens(&self) -> u64 {
        self.series
            .iter()
            .map(|s| s.cached_tokens)
            .fold(0u64, u64::saturating_add)
    }

    pub fn total_cost_usd(&self) -> f64 {
        self.series
            .iter()
            .map(|s| if s.cost_usd.is_finite() { s.cost_usd.max(0.0) } else { 0.0 })
            .sum()
    }

    /// Formats a USD figure into individual rolling-drum characters for metallic price rendering.
    pub fn format_rolling_price_digits(usd: f64) -> Vec<char> {
        let safe = if usd.is_finite() { usd.max(0.0) } else { 0.0 };
        format!("${:.2}", safe).chars().collect()
    }

    /// Formats a real USD cost without hiding small positive sub-cent API spend.
    pub fn format_cost_usd(usd: f64) -> String {
        let safe = if usd.is_finite() { usd.max(0.0) } else { 0.0 };
        if safe > 0.0 && safe < 0.01 {
            format!("${:.4}", safe)
        } else {
            format!("${:.2}", safe)
        }
    }

    /// Formats a token count with thousands separators for crisp readability.
    pub fn format_token_count(tokens: u64) -> String {
        let s = tokens.to_string();
        let bytes = s.as_bytes();
        let mut out = String::with_capacity(s.len() + s.len() / 3);
        for (idx, &b) in bytes.iter().enumerate() {
            if idx > 0 && (bytes.len() - idx) % 3 == 0 {
                out.push(',');
            }
            out.push(b as char);
        }
        out
    }

    /// Resets usage and credits state back to clean empty zero values (used on logout / account switch).
    pub fn reset(&mut self) {
        let range = self.selected_range_days.clamp(1, 90);
        *self = Self {
            selected_range_days: range,
            date_labels: Self::utc_date_window(range),
            ..Self::default()
        };
    }

    /// Applies real server wallet balance and transaction ledger entries.
    pub fn apply_wallet(
        &mut self,
        stars_balance: i64,
        ton_balance: f64,
        pending_hold: i64,
        ledger: Vec<CreditLedgerEntry>,
    ) {
        self.wallet_balance_credits = stars_balance.max(0);
        self.wallet_ton_balance = if ton_balance.is_finite() {
            ton_balance.max(0.0)
        } else {
            0.0
        };
        self.pending_hold_credits = pending_hold.max(0);
        self.ledger = ledger;
    }

    /// Recomputes `series`, `date_labels`, and `total_turns` strictly from real Hermes/Magpie
    /// server telemetry and/or real local OpenBot SQLite `AgentUsageStore` host analytics.
    pub fn apply_live_sources(
        &mut self,
        hermes: Option<&HermesUsageSummaryDto>,
        openbot: Option<&HostAnalytics>,
        range_days: u32,
    ) {
        let clamped_days = range_days.clamp(1, 90);
        self.selected_range_days = clamped_days;

        let dates: Vec<String> = hermes
            .filter(|h| !h.dates.is_empty())
            .map(|h| h.dates.clone())
            .or_else(|| {
                openbot
                    .filter(|o| !o.daily.is_empty())
                    .map(|o| o.daily.iter().map(|d| d.date.clone()).collect())
            })
            .unwrap_or_else(|| Self::utc_date_window(clamped_days));

        let date_index: HashMap<&str, usize> = dates
            .iter()
            .enumerate()
            .map(|(i, d)| (d.as_str(), i))
            .collect();

        struct Acc {
            provider_id: String,
            display_name: String,
            input_tokens: u64,
            output_tokens: u64,
            cached_tokens: u64,
            cost_usd: f64,
            turns: u64,
            daily_cost: Vec<f64>,
            daily_tokens: Vec<u64>,
            models: BTreeSet<String>,
        }

        let mut merged: BTreeMap<String, Acc> = BTreeMap::new();
        let mut total_turns: u64 = 0;

        if let Some(h) = hermes {
            total_turns = total_turns.saturating_add(h.total_turns);
            if !h.by_provider.is_empty() {
                for p in &h.by_provider {
                    let (pid, default_name) = canonicalize_openbot_provider(&p.provider_id);
                    let display_name = if p.display_name.trim().is_empty() {
                        default_name
                    } else {
                        p.display_name.clone()
                    };
                    let entry = merged.entry(pid.clone()).or_insert_with(|| Acc {
                        provider_id: pid,
                        display_name,
                        input_tokens: 0,
                        output_tokens: 0,
                        cached_tokens: 0,
                        cost_usd: 0.0,
                        turns: 0,
                        daily_cost: vec![0.0; dates.len()],
                        daily_tokens: vec![0; dates.len()],
                        models: BTreeSet::new(),
                    });
                    entry.input_tokens = entry.input_tokens.saturating_add(p.input_tokens);
                    entry.output_tokens = entry.output_tokens.saturating_add(p.output_tokens);
                    entry.cached_tokens = entry.cached_tokens.saturating_add(p.cached_tokens);
                    if p.cost_usd.is_finite() && p.cost_usd > 0.0 {
                        entry.cost_usd += p.cost_usd;
                    }
                    entry.turns = entry.turns.saturating_add(p.turns);
                    for m in &p.models {
                        if !m.trim().is_empty() {
                            entry.models.insert(m.trim().to_string());
                        }
                    }
                    for (idx, &c) in p.daily_cost_usd.iter().enumerate() {
                        if idx < entry.daily_cost.len() && c.is_finite() && c > 0.0 {
                            entry.daily_cost[idx] += c;
                        }
                    }
                    for (idx, &t) in p.daily_tokens.iter().enumerate() {
                        if idx < entry.daily_tokens.len() {
                            entry.daily_tokens[idx] = entry.daily_tokens[idx].saturating_add(t);
                        }
                    }
                }
            } else if h.prompt_tokens > 0 || h.completion_tokens > 0 || h.estimated_cost_usd > 0.0 {
                // Compatibility with older server responses that only returned flat totals / by_model.
                if !h.by_model.is_empty() {
                    for row in &h.by_model {
                        let model = row
                            .get("model")
                            .and_then(|v| v.as_str())
                            .unwrap_or("unknown");
                        let in_t = row
                            .get("input_tokens")
                            .and_then(|v| v.as_u64())
                            .unwrap_or(0);
                        let out_t = row
                            .get("output_tokens")
                            .and_then(|v| v.as_u64())
                            .unwrap_or(0);
                        let cache_t = row
                            .get("cached_tokens")
                            .and_then(|v| v.as_u64())
                            .unwrap_or(0);
                        let cost = row
                            .get("total_cost_usd")
                            .and_then(|v| v.as_f64())
                            .unwrap_or(0.0);
                        let turns = row
                            .get("message_count")
                            .and_then(|v| v.as_u64())
                            .unwrap_or(0);
                        let inferred = infer_provider_from_model(model);
                        let (pid, display_name) = canonicalize_openbot_provider(inferred);
                        let entry = merged.entry(pid.clone()).or_insert_with(|| Acc {
                            provider_id: pid,
                            display_name,
                            input_tokens: 0,
                            output_tokens: 0,
                            cached_tokens: 0,
                            cost_usd: 0.0,
                            turns: 0,
                            daily_cost: vec![0.0; dates.len()],
                            daily_tokens: vec![0; dates.len()],
                            models: BTreeSet::new(),
                        });
                        entry.input_tokens = entry.input_tokens.saturating_add(in_t);
                        entry.output_tokens = entry.output_tokens.saturating_add(out_t);
                        entry.cached_tokens = entry.cached_tokens.saturating_add(cache_t);
                        if cost.is_finite() && cost > 0.0 {
                            entry.cost_usd += cost;
                        }
                        entry.turns = entry.turns.saturating_add(turns);
                        entry.models.insert(model.to_string());
                        if let Some(last) = entry.daily_cost.last_mut() {
                            if cost.is_finite() && cost > 0.0 {
                                *last += cost;
                            }
                        }
                        if let Some(last) = entry.daily_tokens.last_mut() {
                            *last = last.saturating_add(in_t.saturating_add(out_t));
                        }
                    }
                } else {
                    let entry = merged.entry("other".to_string()).or_insert_with(|| Acc {
                        provider_id: "other".to_string(),
                        display_name: "AI Usage".to_string(),
                        input_tokens: 0,
                        output_tokens: 0,
                        cached_tokens: 0,
                        cost_usd: 0.0,
                        turns: 0,
                        daily_cost: vec![0.0; dates.len()],
                        daily_tokens: vec![0; dates.len()],
                        models: BTreeSet::new(),
                    });
                    entry.input_tokens = entry.input_tokens.saturating_add(h.prompt_tokens);
                    entry.output_tokens = entry.output_tokens.saturating_add(h.completion_tokens);
                    entry.cached_tokens = entry.cached_tokens.saturating_add(h.cached_tokens);
                    if h.estimated_cost_usd.is_finite() && h.estimated_cost_usd > 0.0 {
                        entry.cost_usd += h.estimated_cost_usd;
                    }
                    entry.turns = entry.turns.saturating_add(h.total_turns);
                    if let Some(last) = entry.daily_cost.last_mut() {
                        if h.estimated_cost_usd.is_finite() && h.estimated_cost_usd > 0.0 {
                            *last += h.estimated_cost_usd;
                        }
                    }
                    if let Some(last) = entry.daily_tokens.last_mut() {
                        *last = last.saturating_add(h.prompt_tokens.saturating_add(h.completion_tokens));
                    }
                }
            }
        }

        if let Some(ob) = openbot {
            total_turns = total_turns.saturating_add(ob.totals.turns as u64);
            for m in &ob.models {
                let (pid, display_name) = canonicalize_openbot_provider(&m.provider);
                let entry = merged.entry(pid.clone()).or_insert_with(|| Acc {
                    provider_id: pid,
                    display_name,
                    input_tokens: 0,
                    output_tokens: 0,
                    cached_tokens: 0,
                    cost_usd: 0.0,
                    turns: 0,
                    daily_cost: vec![0.0; dates.len()],
                    daily_tokens: vec![0; dates.len()],
                    models: BTreeSet::new(),
                });
                let in_t = m
                    .totals
                    .uncached_input
                    .unwrap_or(0)
                    .saturating_add(m.totals.cached_input.unwrap_or(0))
                    .saturating_add(m.totals.cache_creation.unwrap_or(0));
                let out_t = m.totals.output.unwrap_or(0);
                entry.input_tokens = entry.input_tokens.saturating_add(in_t);
                entry.output_tokens = entry.output_tokens.saturating_add(out_t);
                entry.cached_tokens = entry
                    .cached_tokens
                    .saturating_add(m.totals.cached_input.unwrap_or(0));
                if let Some(c) = m.totals.estimated_cost_usd {
                    if c.is_finite() && c > 0.0 {
                        entry.cost_usd += c;
                    }
                }
                entry.turns = entry.turns.saturating_add(m.totals.turns as u64);
                if !m.model.trim().is_empty() {
                    entry.models.insert(m.model.trim().to_string());
                }
            }
            for pd in &ob.provider_daily {
                let (pid, display_name) = canonicalize_openbot_provider(&pd.provider);
                let entry = merged.entry(pid.clone()).or_insert_with(|| Acc {
                    provider_id: pid,
                    display_name,
                    input_tokens: 0,
                    output_tokens: 0,
                    cached_tokens: 0,
                    cost_usd: 0.0,
                    turns: 0,
                    daily_cost: vec![0.0; dates.len()],
                    daily_tokens: vec![0; dates.len()],
                    models: BTreeSet::new(),
                });
                if let Some(&idx) = date_index.get(pd.date.as_str()) {
                    if let Some(c) = pd.estimated_cost_usd {
                        if c.is_finite() && c > 0.0 {
                            entry.daily_cost[idx] += c;
                        }
                    }
                    entry.daily_tokens[idx] =
                        entry.daily_tokens[idx].saturating_add(pd.processed_tokens);
                }
            }
        }

        let mut accs: Vec<Acc> = merged
            .into_values()
            .filter(|a| a.input_tokens > 0 || a.output_tokens > 0 || a.cost_usd > 0.0)
            .collect();
        accs.sort_by(|a, b| {
            b.cost_usd
                .partial_cmp(&a.cost_usd)
                .unwrap_or(std::cmp::Ordering::Equal)
                .then_with(|| {
                    (b.input_tokens + b.output_tokens).cmp(&(a.input_tokens + a.output_tokens))
                })
                .then_with(|| a.provider_id.cmp(&b.provider_id))
        });

        let total_cost: f64 = accs.iter().map(|a| a.cost_usd).sum();
        let total_tok: u64 = accs
            .iter()
            .map(|a| a.input_tokens.saturating_add(a.output_tokens))
            .sum();

        let max_daily_cost = accs
            .iter()
            .flat_map(|a| a.daily_cost.iter().copied())
            .fold(0.0_f64, f64::max);
        let max_daily_tokens = accs
            .iter()
            .flat_map(|a| a.daily_tokens.iter().copied())
            .max()
            .unwrap_or(0);

        self.series = accs
            .into_iter()
            .map(|a| {
                let prov_tokens = a.input_tokens.saturating_add(a.output_tokens);
                let share = if total_cost > 0.0 {
                    (a.cost_usd / total_cost) as f32
                } else if total_tok > 0 {
                    (prov_tokens as f64 / total_tok as f64) as f32
                } else {
                    0.0
                };
                let daily_bars: Vec<f32> = if max_daily_cost > 0.0 {
                    a.daily_cost
                        .iter()
                        .map(|&c| {
                            if c > 0.0 {
                                (c / max_daily_cost).clamp(0.0, 1.0) as f32
                            } else {
                                0.0
                            }
                        })
                        .collect()
                } else if max_daily_tokens > 0 {
                    a.daily_tokens
                        .iter()
                        .map(|&t| {
                            if t > 0 {
                                (t as f64 / max_daily_tokens as f64).clamp(0.0, 1.0) as f32
                            } else {
                                0.0
                            }
                        })
                        .collect()
                } else {
                    vec![0.0; dates.len()]
                };
                ProviderUsageSeries {
                    accent_hex: provider_usage_accent_hex(&a.provider_id),
                    provider_id: a.provider_id,
                    display_name: a.display_name,
                    input_tokens: a.input_tokens,
                    output_tokens: a.output_tokens,
                    cached_tokens: a.cached_tokens,
                    cost_usd: a.cost_usd,
                    quota_used_ratio: share.clamp(0.0, 1.0),
                    daily_bars,
                    turns: a.turns,
                    models: a.models.into_iter().collect(),
                    has_explicit_quota: false,
                }
            })
            .collect();

        self.date_labels = dates;
        self.total_turns = total_turns;
        self.is_loading = false;
        self.has_synced = true;
        self.sync_error = None;
    }
}

fn infer_provider_from_model(model: &str) -> &'static str {
    let m = model.trim().to_ascii_lowercase();
    if m == "hermes-local-v1" {
        "local"
    } else if m.contains("claude") {
        "claude"
    } else if m.contains("gpt-") || m.contains("codex") || m.starts_with("o1") || m.starts_with("o3") || m.starts_with("o4") {
        "codex"
    } else if m.contains("grok") {
        "grok"
    } else if m.contains("gemini") {
        "gemini"
    } else if m.contains("deepseek") {
        "deepseek"
    } else if m.contains("qwen") {
        "qwen"
    } else if m.contains("kimi") || m.contains("moonshot") {
        "kimi"
    } else if m.contains("glm") {
        "glm"
    } else if m.contains("minimax") {
        "minimax"
    } else if m.contains("mistral") || m.contains("codestral") {
        "mistral"
    } else {
        "other"
    }
}

fn paint_provider_usage_chart(
    bounds: Bounds<Pixels>,
    series: &[ProviderUsageSeries],
    window: &mut Window,
) {
    let ox = f32::from(bounds.origin.x);
    let oy = f32::from(bounds.origin.y);
    let bw = f32::from(bounds.size.width).max(1.0);
    let bh = f32::from(bounds.size.height).max(1.0);
    let day_count = series.iter().map(|s| s.daily_bars.len()).max().unwrap_or(0);
    if day_count == 0 || series.is_empty() {
        return;
    }
    let group_w = bw / (day_count as f32);
    let bar_w = ((group_w - 10.0) / (series.len() as f32)).clamp(3.0, 22.0);
    for (s_idx, s) in series.iter().enumerate() {
        let color: Hsla = rgb(s.accent_hex).into();
        let mut pb = PathBuilder::fill();
        let mut drew_any = false;
        for (d_idx, &norm) in s.daily_bars.iter().enumerate() {
            if !norm.is_finite() || norm <= 0.0 {
                continue;
            }
            drew_any = true;
            let h = (norm.clamp(0.04, 1.0) * (bh - 12.0)).max(3.0);
            let x0 = ox + (d_idx as f32) * group_w + 5.0 + (s_idx as f32) * (bar_w + 2.0);
            let y0 = oy + bh - h;
            pb.move_to(point(px(x0), px(y0)));
            pb.line_to(point(px(x0 + bar_w), px(y0)));
            pb.line_to(point(px(x0 + bar_w), px(oy + bh)));
            pb.line_to(point(px(x0), px(oy + bh)));
            pb.close();
        }
        if drew_any {
            if let Ok(path) = pb.build() {
                window.paint_path(path, color.opacity(0.9));
            }
        }
    }
}

fn short_month_day(iso_date: &str) -> String {
    if iso_date.len() >= 10 {
        iso_date[5..10].to_string()
    } else {
        iso_date.to_string()
    }
}

/// Renders the `Usage` and `Credits` dashboard (non-interactive snapshot variant).
pub fn render_usage_and_credits(state: &UsageAndCreditsState, palette: &OpenBotPalette) -> Div {
    build_usage_and_credits_view(state, palette, "en", None, None, None, None)
}

/// Renders the interactive `Usage` and `Credits` dashboard wired to live range switching,
/// manual refresh, and real Credits wallet navigation.
pub fn render_usage_and_credits_interactive<V: 'static>(
    state: &UsageAndCreditsState,
    palette: &OpenBotPalette,
    lang_id: &str,
    cx: &mut Context<V>,
    on_action: fn(&mut V, UsageAndCreditsUiAction, &mut Context<V>),
) -> Div {
    let range_pills = build_range_pills(state, palette, lang_id, cx, on_action);
    let refresh_btn = {
        let bg_control = pal_rgba(palette, "--openbot-bg-control", 0x212121);
        let bg_hover = pal_rgba(palette, "--openbot-bg-control-hover", 0x2a2a2a);
        let border = pal_rgba(palette, "--openbot-border-grouped", 0x333335);
        let text_primary = pal_rgba(palette, "--openbot-text-primary", 0xffffff);
        let label = if state.is_loading {
            tr_str(lang_id, "同步中…", "Syncing…")
        } else {
            tr_str(lang_id, "刷新", "Refresh")
        };
        div()
            .id("openbot-usage-refresh-btn")
            .px(px(10.0))
            .py(px(5.0))
            .rounded(px(8.0))
            .bg(bg_control)
            .border_1()
            .border_color(border)
            .cursor_pointer()
            .hover(move |s| s.bg(bg_hover))
            .text_size(px(11.0))
            .text_color(text_primary)
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |this, _, _, cx| {
                    on_action(this, UsageAndCreditsUiAction::Refresh, cx);
                }),
            )
            .child(label)
            .into_any_element()
    };
    let buy_btn = {
        let accent = pal_rgba(palette, "--openbot-accent", 0x007cf7);
        div()
            .id("openbot-usage-buy-credits-btn")
            .px(px(10.0))
            .py(px(6.0))
            .rounded(px(10.0))
            .bg(accent)
            .cursor_pointer()
            .text_size(px(12.0))
            .text_color(rgb(0xffffff))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |this, _, _, cx| {
                    on_action(this, UsageAndCreditsUiAction::BuyCredits, cx);
                }),
            )
            .child(tr_str(lang_id, "购买积分", "Buy Credits"))
            .into_any_element()
    };
    let move_btn = {
        let bg_control = pal_rgba(palette, "--openbot-bg-control", 0x212121);
        let bg_hover = pal_rgba(palette, "--openbot-bg-control-hover", 0x2a2a2a);
        let border = pal_rgba(palette, "--openbot-border-grouped", 0x333335);
        let text_primary = pal_rgba(palette, "--openbot-text-primary", 0xffffff);
        div()
            .id("openbot-usage-move-credits-btn")
            .px(px(10.0))
            .py(px(6.0))
            .rounded(px(10.0))
            .bg(bg_control)
            .border_1()
            .border_color(border)
            .cursor_pointer()
            .hover(move |s| s.bg(bg_hover))
            .text_size(px(12.0))
            .text_color(text_primary)
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |this, _, _, cx| {
                    on_action(this, UsageAndCreditsUiAction::MoveCredits, cx);
                }),
            )
            .child(tr_str(lang_id, "管理钱包", "Manage Wallet"))
            .into_any_element()
    };

    build_usage_and_credits_view(
        state,
        palette,
        lang_id,
        Some(range_pills),
        Some(refresh_btn),
        Some(buy_btn),
        Some(move_btn),
    )
}

fn build_range_pills<V: 'static>(
    state: &UsageAndCreditsState,
    palette: &OpenBotPalette,
    lang_id: &str,
    cx: &mut Context<V>,
    on_action: fn(&mut V, UsageAndCreditsUiAction, &mut Context<V>),
) -> AnyElement {
    let bg_control = pal_rgba(palette, "--openbot-bg-control", 0x212121);
    let border = pal_rgba(palette, "--openbot-border-grouped", 0x333335);
    let text_primary = pal_rgba(palette, "--openbot-text-primary", 0xffffff);
    let text_muted = pal_rgba(palette, "--openbot-text-muted", 0x979797);
    let accent = pal_rgba(palette, "--openbot-accent", 0x007cf7);

    let mut row = div().flex().items_center().gap(px(4.0));
    for days in [7u32, 14, 30] {
        let active = state.selected_range_days == days;
        let label = if lang_id.starts_with("zh") {
            format!("{days}天")
        } else {
            format!("{days}d")
        };
        row = row.child(
            div()
                .id(SharedString::from(format!("openbot-usage-range-{days}d")))
                .px(px(8.0))
                .py(px(4.0))
                .rounded(px(8.0))
                .cursor_pointer()
                .bg(if active { accent } else { bg_control })
                .border_1()
                .border_color(if active { accent } else { border })
                .text_size(px(11.0))
                .font_weight(if active {
                    FontWeight::SEMIBOLD
                } else {
                    FontWeight::NORMAL
                })
                .text_color(if active {
                    rgb(0xffffff)
                } else {
                    text_muted
                })
                .hover(move |s| {
                    if active {
                        s
                    } else {
                        s.text_color(text_primary)
                    }
                })
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(move |this, _, _, cx| {
                        on_action(this, UsageAndCreditsUiAction::SelectRangeDays(days), cx);
                    }),
                )
                .child(label),
        );
    }
    row.into_any_element()
}

#[allow(clippy::too_many_arguments)]
fn build_usage_and_credits_view(
    state: &UsageAndCreditsState,
    palette: &OpenBotPalette,
    lang_id: &str,
    range_pills: Option<AnyElement>,
    refresh_btn: Option<AnyElement>,
    buy_btn: Option<AnyElement>,
    move_btn: Option<AnyElement>,
) -> Div {
    let bg_canvas = pal_rgba(palette, "--openbot-bg-canvas", 0x1a1a1a);
    let bg_surface = pal_rgba(palette, "--openbot-bg-surface", 0x212121);
    let bg_control = pal_rgba(palette, "--openbot-bg-control", 0x212121);
    let border = pal_rgba(palette, "--openbot-border-grouped", 0x333335);
    let text_primary = pal_rgba(palette, "--openbot-text-primary", 0xffffff);
    let text_muted = pal_rgba(palette, "--openbot-text-muted", 0x979797);
    let accent = pal_rgba(palette, "--openbot-accent", 0x007cf7);
    let danger = pal_rgba(palette, "--openbot-danger", 0xfd2f3b);

    let total_cost = state.total_cost_usd();
    let total_tokens = state.total_tokens();
    let mut price_drums = div().flex().items_center().gap(px(2.0));
    for ch in UsageAndCreditsState::format_rolling_price_digits(total_cost) {
        price_drums = price_drums.child(
            div()
                .px(px(6.0))
                .py(px(4.0))
                .rounded(px(8.0))
                .bg(bg_control)
                .border_1()
                .border_color(border)
                .text_size(px(22.0))
                .font_weight(FontWeight::BOLD)
                .text_color(text_primary)
                .child(ch.to_string()),
        );
    }

    let chart_title = if lang_id.starts_with("zh") {
        format!("模型服务商消耗（近 {} 天）", state.selected_range_days)
    } else {
        format!("Provider Spend ({}d)", state.selected_range_days)
    };
    let tokens_summary = {
        let tok_str = UsageAndCreditsState::format_token_count(total_tokens);
        let exact_sub = if total_cost > 0.0 && total_cost < 0.01 {
            format!(" · {}", UsageAndCreditsState::format_cost_usd(total_cost))
        } else {
            String::new()
        };
        if state.total_turns > 0 {
            if lang_id.starts_with("zh") {
                format!("{tok_str} tokens · {} 次调用{exact_sub}", state.total_turns)
            } else {
                format!("{tok_str} tokens · {} calls{exact_sub}", state.total_turns)
            }
        } else {
            format!("{tok_str} tokens{exact_sub}")
        }
    };

    let chart_series = state.series.clone();
    let has_chart_bars = chart_series
        .iter()
        .any(|s| s.daily_bars.iter().any(|&v| v.is_finite() && v > 0.0));

    let chart_body: AnyElement = if chart_series.is_empty() || !has_chart_bars {
        div()
            .h(px(136.0))
            .w_full()
            .flex()
            .flex_col()
            .items_center()
            .justify_center()
            .gap(px(6.0))
            .child(
                div()
                    .text_size(px(13.0))
                    .font_weight(FontWeight::MEDIUM)
                    .text_color(text_primary)
                    .child(if state.is_loading {
                        tr_str(lang_id, "正在同步真实 AI 计费数据…", "Syncing live AI usage data…")
                            .to_string()
                    } else if lang_id.starts_with("zh") {
                        format!("近 {} 天暂无 AI 用量记录", state.selected_range_days)
                    } else {
                        format!(
                            "No AI usage recorded in the last {} days",
                            state.selected_range_days
                        )
                    }),
            )
            .child(
                div()
                    .text_size(px(11.0))
                    .text_color(text_muted)
                    .child(tr_str(
                        lang_id,
                        "与 AI 机器人对话或运行智能体后，将在此显示真实的 Token 消耗与估算费用。",
                        "Real token counts and estimated API costs appear here after you chat with an AI bot or agent.",
                    )),
            )
            .into_any_element()
    } else {
        let dates = state.date_labels.clone();
        let label_step = if dates.len() > 14 {
            5
        } else if dates.len() > 7 {
            2
        } else {
            1
        };
        let mut axis_row = div()
            .w_full()
            .flex()
            .items_center()
            .justify_between()
            .pt(px(4.0));
        for (idx, d) in dates.iter().enumerate() {
            let show = idx % label_step == 0 || idx + 1 == dates.len();
            axis_row = axis_row.child(
                div()
                    .flex_1()
                    .text_size(px(9.5))
                    .text_color(text_muted)
                    .child(if show {
                        short_month_day(d)
                    } else {
                        String::new()
                    }),
            );
        }
        div()
            .w_full()
            .flex()
            .flex_col()
            .child(
                div().h(px(136.0)).w_full().child(
                    canvas(
                        |_, _, _| {},
                        move |bounds, _, window, _cx| {
                            paint_provider_usage_chart(bounds, &chart_series, window);
                        },
                    )
                    .size_full(),
                ),
            )
            .when(!dates.is_empty(), |el| el.child(axis_row))
            .into_any_element()
    };

    let chart_card = div()
        .p(px(14.0))
        .rounded(px(14.0))
        .bg(bg_surface)
        .border_1()
        .border_color(border)
        .flex()
        .flex_col()
        .gap(px(10.0))
        .child(
            div()
                .flex()
                .items_center()
                .justify_between()
                .child(
                    div()
                        .text_size(px(13.0))
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(text_primary)
                        .child(chart_title),
                )
                .child(
                    div()
                        .text_size(px(11.0))
                        .text_color(text_muted)
                        .child(tokens_summary),
                ),
        )
        .child(chart_body);

    let mut quotas = div().flex().flex_wrap().gap(px(10.0));
    for s in &state.series {
        let pct = (s.quota_used_ratio.clamp(0.0, 1.0) * 100.0).round() as u32;
        let bar_px = (2.16 * (pct as f32)).clamp(0.0, 216.0);
        let ratio_label = if s.has_explicit_quota {
            if lang_id.starts_with("zh") {
                format!("配额 {pct}%")
            } else {
                format!("Quota {pct}%")
            }
        } else if lang_id.starts_with("zh") {
            format!("占比 {pct}%")
        } else {
            format!("Share {pct}%")
        };
        let detail_line = format!(
            "{ratio_label} · {} in / {} out · {} cached",
            UsageAndCreditsState::format_token_count(s.input_tokens),
            UsageAndCreditsState::format_token_count(s.output_tokens),
            UsageAndCreditsState::format_token_count(s.cached_tokens),
        );
        let models_line = if s.models.is_empty() {
            None
        } else {
            let joined = s.models.iter().take(3).cloned().collect::<Vec<_>>().join(", ");
            Some(if s.models.len() > 3 {
                format!("{joined} +{}", s.models.len() - 3)
            } else {
                joined
            })
        };
        quotas = quotas.child(
            div()
                .w(px(244.0))
                .p(px(12.0))
                .rounded(px(12.0))
                .bg(bg_surface)
                .border_1()
                .border_color(border)
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
                                .text_size(px(12.0))
                                .font_weight(FontWeight::SEMIBOLD)
                                .text_color(rgb(s.accent_hex))
                                .child(s.display_name.clone()),
                        )
                        .child(
                            div()
                                .text_size(px(11.0))
                                .font_weight(FontWeight::MEDIUM)
                                .text_color(text_primary)
                                .child(UsageAndCreditsState::format_cost_usd(s.cost_usd)),
                        ),
                )
                .child(
                    div()
                        .h(px(6.0))
                        .rounded(px(999.0))
                        .bg(bg_control)
                        .child(
                            div()
                                .h_full()
                                .w(px(bar_px))
                                .rounded(px(999.0))
                                .bg(rgb(s.accent_hex)),
                        ),
                )
                .child(
                    div()
                        .text_size(px(10.0))
                        .text_color(text_muted)
                        .child(detail_line),
                )
                .when_some(models_line, |el, mline| {
                    el.child(
                        div()
                            .text_size(px(9.5))
                            .text_color(text_muted)
                            .truncate()
                            .child(mline),
                    )
                }),
        );
    }

    let wallet_subline = if state.pending_hold_credits > 0 {
        if lang_id.starts_with("zh") {
            format!("待结算冻结: {} 积分", state.pending_hold_credits)
        } else {
            format!("Pending hold: {} credits", state.pending_hold_credits)
        }
    } else if state.wallet_ton_balance > 0.0 {
        format!("{:.2} TON", state.wallet_ton_balance)
    } else {
        tr_str(
            lang_id,
            "余额与交易实时同步自账户钱包",
            "Live balance synced from account wallet",
        )
        .to_string()
    };

    let default_buy = div()
        .px(px(10.0))
        .py(px(6.0))
        .rounded(px(10.0))
        .bg(accent)
        .text_size(px(12.0))
        .text_color(rgb(0xffffff))
        .child(tr_str(lang_id, "购买积分", "Buy Credits"))
        .into_any_element();
    let default_move = div()
        .px(px(10.0))
        .py(px(6.0))
        .rounded(px(10.0))
        .bg(bg_control)
        .border_1()
        .border_color(border)
        .text_size(px(12.0))
        .text_color(text_primary)
        .child(tr_str(lang_id, "管理钱包", "Manage Wallet"))
        .into_any_element();

    let wallet_card = div()
        .p(px(14.0))
        .rounded(px(14.0))
        .bg(bg_surface)
        .border_1()
        .border_color(border)
        .flex()
        .items_center()
        .justify_between()
        .child(
            div()
                .flex()
                .flex_col()
                .gap(px(4.0))
                .child(
                    div()
                        .text_size(px(12.0))
                        .text_color(text_muted)
                        .child(tr_str(lang_id, "积分钱包", "Credits Wallet")),
                )
                .child(
                    div()
                        .text_size(px(18.0))
                        .font_weight(FontWeight::BOLD)
                        .text_color(text_primary)
                        .child(if lang_id.starts_with("zh") {
                            format!("{} 积分", state.wallet_balance_credits)
                        } else {
                            format!("{} credits", state.wallet_balance_credits)
                        }),
                )
                .child(
                    div()
                        .text_size(px(10.0))
                        .text_color(text_muted)
                        .child(wallet_subline),
                ),
        )
        .child(
            div()
                .flex()
                .items_center()
                .gap(px(8.0))
                .child(buy_btn.unwrap_or(default_buy))
                .child(move_btn.unwrap_or(default_move)),
        );

    let ledger_card = if state.ledger.is_empty() {
        None
    } else {
        let mut card = div()
            .p(px(12.0))
            .rounded(px(12.0))
            .bg(bg_surface)
            .border_1()
            .border_color(border)
            .flex()
            .flex_col()
            .gap(px(6.0))
            .child(
                div()
                    .text_size(px(12.0))
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(text_primary)
                    .child(tr_str(lang_id, "最近积分变动", "Recent Credit Activity")),
            );
        for entry in state.ledger.iter().take(20) {
            let delta_color = if entry.delta_credits >= 0 {
                pal_rgba(palette, "--openbot-success", 0x31cf76)
            } else {
                danger
            };
            card = card.child(
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .text_size(px(11.0))
                    .child(div().text_color(text_primary).child(entry.label.clone()))
                    .child(
                        div()
                            .text_color(delta_color)
                            .child(format!("{:+} cr · {}", entry.delta_credits, entry.created_at)),
                    ),
            );
        }
        Some(card)
    };

    div()
        .size_full()
        .flex()
        .flex_col()
        .bg(bg_canvas)
        .child(
            div()
                .id("openbot-usage-credits-scroll")
                .flex_1()
                .min_h(px(0.0))
                .overflow_y_scroll()
                .flex()
                .flex_col()
                .gap(px(14.0))
                .p(px(16.0))
                .child(
                    div()
                        .flex()
                        .items_center()
                        .justify_between()
                        .gap(px(12.0))
                        .child(
                            div()
                                .flex()
                                .items_center()
                                .gap(px(10.0))
                                .child(tg_icon_rgba(IconName::WalletStars, 18.0, accent))
                                .child(
                                    div()
                                        .text_size(px(16.0))
                                        .font_weight(FontWeight::SEMIBOLD)
                                        .text_color(text_primary)
                                        .child(tr_str(lang_id, "AI 用量与计费", "Usage & Credits")),
                                )
                                .when_some(range_pills, |el, pills| el.child(pills))
                                .when_some(refresh_btn, |el, btn| el.child(btn)),
                        )
                        .child(price_drums),
                )
                .when_some(state.sync_error.clone(), |el, err| {
                    el.child(
                        div()
                            .px(px(12.0))
                            .py(px(8.0))
                            .rounded(px(10.0))
                            .bg(bg_surface)
                            .border_1()
                            .border_color(danger)
                            .text_size(px(11.0))
                            .text_color(danger)
                            .child(err),
                    )
                })
                .child(wallet_card)
                .child(chart_card)
                .when(!state.series.is_empty(), |el| el.child(quotas))
                .when_some(ledger_card, |el, card| el.child(card)),
        )
}

