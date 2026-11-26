//! Routine schedules (9 variants), pure-Rust 5-field cron & `>= 180s` (`3m`) cadence validator,
//! `Routine` / `ChannelRoutine` / `EventRoutine` (`RoutineDefinition`), RFC-6901 `EventFilter`,
//! native HMAC-SHA256 & Standard Webhooks (`whsec_`) verifier, `QuietRunPolicy`, flows, and result deliveries.
//! Mirrors `packages/contracts/src/{ipc-routines,ipc-channel-routines,ipc-events,ipc-routine-flows,ipc-routine-results}.ts`,
//! `signal-protocol/webhook-route.ts`, `src/main/webhook-security.ts`, and `src/backend/agent/routine-quiet-runs.ts`.

use std::collections::{BTreeSet, HashSet};
use std::sync::LazyLock;

use base64::Engine;
use base64::engine::general_purpose::{STANDARD as BASE64_STD, URL_SAFE_NO_PAD, URL_SAFE as BASE64_URL};
use chrono::{DateTime, Datelike, Days, LocalResult, NaiveDate, TimeZone, Timelike, Utc};
use chrono_tz::{TZ_VARIANTS, Tz};
use hmac::{Hmac, Mac};
use regex::Regex;
use serde::{Deserialize, Serialize};
use sha2::Sha256;
use thiserror::Error;

use crate::channels::{ChannelTask, ChannelTaskState};
use crate::limits::{
    ContractError, INPUT_LIMITS, is_bounded_string, is_calendar_date, is_identifier,
    is_iso_timestamp, non_null_option, utf16_len,
};

pub const ROUTINE_MINIMUM_INTERVAL_MINUTES: u32 = 3;
pub const ROUTINE_MINIMUM_INTERVAL_SECONDS: u64 = 180;
pub const ROUTINE_MISSED_COUNT_LIMIT: u32 = 999;
pub const EVENT_DELIVERY_ID_MAX_LENGTH: usize = 512;
pub const WEBHOOK_DELIVERY_BODY_BYTES_LIMIT: usize = 64 * 1024;
pub const WEBHOOK_MAX_CLOCK_SKEW_SECONDS: i64 = 5 * 60;
pub const ROUTINE_FLOW_MAX_COORDINATE: f64 = 1_000_000.0;

pub const ROUTINE_NO_UPDATE_TOOL: &str = "routine_no_update";
pub const ROUTINE_REPORT_FAILURE_TOOL: &str = "routine_report_failure";
pub const ROUTINE_NO_UPDATE_MARKER: &str = "[[no-update]]";
pub const HERMES_SILENT_MARKER: &str = "[silent]";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum RoutineIntervalUnit {
    Minutes,
    Hours,
    Days,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum RoutineSubDayUnit {
    Minutes,
    Hours,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case")]
pub enum RoutineDaySelection {
    EveryDay,
    DaysOfWeek { days: Vec<u8> },
    DaysOfMonth { days: Vec<u8> },
}

impl RoutineDaySelection {
    pub fn is_valid(&self) -> bool {
        match self {
            Self::EveryDay => true,
            Self::DaysOfWeek { days } => !days.is_empty() && days.iter().all(|d| *d <= 6),
            Self::DaysOfMonth { days } => !days.is_empty() && days.iter().all(|d| (1..=31).contains(d)),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case")]
pub enum RoutineTimeSelection {
    AtTime { time: String },
    Every { amount: u32, unit: RoutineSubDayUnit },
}

pub fn is_routine_time(value: &str) -> bool {
    let bytes = value.as_bytes();
    if bytes.len() != 5 || bytes[2] != b':' {
        return false;
    }
    let Some(hh) = value[..2].parse::<u8>().ok() else {
        return false;
    };
    let Some(mm) = value[3..].parse::<u8>().ok() else {
        return false;
    };
    value.bytes().enumerate().all(|(i, b)| i == 2 || b.is_ascii_digit()) && hh <= 23 && mm <= 59
}

fn parse_timezone(tz: &str) -> Result<Tz, ContractError> {
    if tz.is_empty() || !is_bounded_string(tz, INPUT_LIMITS.identifier) {
        return Err(ContractError::InvalidValue("Invalid routine timezone.".to_string()));
    }
    tz.parse::<Tz>()
        .ok()
        .or_else(|| {
            TZ_VARIANTS
                .iter()
                .copied()
                .find(|v| v.name().eq_ignore_ascii_case(tz))
        })
        .ok_or_else(|| ContractError::InvalidValue(format!("Invalid routine timezone: {tz}")))
}

pub fn is_valid_timezone(tz: &str) -> bool {
    parse_timezone(tz).is_ok()
}

impl RoutineTimeSelection {
    pub fn is_valid(&self) -> bool {
        match self {
            Self::AtTime { time } => is_routine_time(time),
            Self::Every { amount, .. } => (1..=100_000).contains(amount),
        }
    }
}

/// All 9 `RoutineSchedule` variants (`ipc-routines.ts:L16-L30`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "lowercase")]
pub enum RoutineSchedule {
    Hourly {
        minute: u8,
    },
    Daily {
        time: String,
    },
    Weekdays {
        time: String,
    },
    Weekly {
        weekday: u8,
        time: String,
    },
    Monthly {
        day: u8,
        time: String,
    },
    Interval {
        amount: u32,
        unit: RoutineIntervalUnit,
        #[serde(rename = "anchorAt")]
        anchor_at: String,
    },
    Advanced {
        months: Vec<u8>,
        days: RoutineDaySelection,
        time: RoutineTimeSelection,
    },
    Custom {
        expression: String,
    },
    Once {
        date: String,
        time: String,
    },
}

impl RoutineSchedule {
    pub fn is_valid(&self) -> bool {
        match self {
            Self::Hourly { minute } => *minute <= 59,
            Self::Daily { time } | Self::Weekdays { time } => is_routine_time(time),
            Self::Weekly { weekday, time } => *weekday <= 6 && is_routine_time(time),
            Self::Monthly { day, time } => (1..=31).contains(day) && is_routine_time(time),
            Self::Interval {
                amount, anchor_at, ..
            } => (1..=100_000).contains(amount) && is_iso_timestamp(anchor_at),
            Self::Advanced { months, days, time } => {
                !months.is_empty()
                    && months.iter().all(|m| (1..=12).contains(m))
                    && days.is_valid()
                    && time.is_valid()
            }
            Self::Custom { expression } => {
                !expression.is_empty() && is_bounded_string(expression, INPUT_LIMITS.routine_cron)
            }
            Self::Once { date, time } => is_calendar_date(date) && is_routine_time(time),
        }
    }

    pub fn is_released_schedule(&self) -> bool {
        self.is_valid() && !matches!(self, Self::Once { .. })
    }
}

const MINIMUM_INTERVAL_MS: i64 = (ROUTINE_MINIMUM_INTERVAL_MINUTES as i64) * 60_000;
const MAX_SEARCH_DAYS: u64 = 366 * 5;
const MAX_GAP_MINUTES: i64 = 3 * 60;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CronField {
    pub values: Vec<u8>,
    pub wildcard: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CronSpec {
    pub minute: CronField,
    pub hour: CronField,
    pub day: CronField,
    pub month: CronField,
    pub weekday: CronField,
    pub exact_times: Option<HashSet<(u8, u8)>>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CronSchedule {
    pub minutes: BTreeSet<u8>,
    pub hours: BTreeSet<u8>,
    pub days_of_month: BTreeSet<u8>,
    pub months: BTreeSet<u8>,
    pub days_of_week: BTreeSet<u8>,
    pub day_wildcard: bool,
    pub weekday_wildcard: bool,
}

#[derive(Debug, Clone, Copy)]
struct CalendarParts {
    year: i32,
    month: u8,
    day: u8,
    hour: u8,
    minute: u8,
}

fn is_cron_decimal(value: &str) -> bool {
    !value.is_empty() && value.bytes().all(|b| b.is_ascii_digit())
}

/// Strict 5-field decimal cron save syntax check (`routine-schedule.ts:L338-L360`).
pub fn validate_cron_save_syntax(expression: &str) -> Result<(), ContractError> {
    let values: Vec<&str> = expression.trim().split_whitespace().collect();
    if values.len() != 5 {
        return Err(ContractError::InvalidValue(
            "cron expression must have exactly 5 fields".to_string(),
        ));
    }
    let fields = [
        (values[0], "minute"),
        (values[1], "hour"),
        (values[2], "day"),
        (values[3], "month"),
        (values[4], "weekday"),
    ];
    for (source, label) in fields {
        for segment in source.split(',') {
            let parts: Vec<&str> = segment.split('/').collect();
            if parts.len() > 2 {
                return Err(ContractError::InvalidValue(format!(
                    "invalid cron {label} value: {segment}"
                )));
            }
            let range_source = parts[0];
            let bounds: Vec<&str> = if range_source == "*" {
                Vec::new()
            } else {
                range_source.split('-').collect()
            };
            let step: Vec<&str> = if parts.len() == 2 {
                vec![parts[1]]
            } else {
                Vec::new()
            };
            if !bounds.into_iter().chain(step).all(is_cron_decimal) {
                return Err(ContractError::InvalidValue(format!(
                    "invalid cron {label} value: {segment}"
                )));
            }
        }
    }
    Ok(())
}

/// Exact port of `parseCronField` (`routine-schedule.ts:L366-L405`; `"0/1"` -> `start = end = 0` -> `[0]`).
pub fn parse_cron_field(
    source: &str,
    minimum: u8,
    maximum: u8,
    label: &str,
    sunday_alias: bool,
) -> Result<CronField, ContractError> {
    let wildcard = source == "*";
    let mut result = BTreeSet::new();
    for segment in source.split(',') {
        let mut slash_iter = segment.split('/');
        let Some(range_source) = slash_iter.next() else {
            return Err(ContractError::InvalidValue(format!("invalid cron {label} value")));
        };
        let step_source = slash_iter.next();
        let step: u32 = match step_source {
            None => 1,
            Some(s) => {
                let parsed = s.trim().parse::<i64>().map_err(|_| {
                    ContractError::InvalidValue(format!("The cron {label} step is invalid."))
                })?;
                if parsed < 1 || parsed > 1_000_000 {
                    return Err(ContractError::InvalidValue(format!(
                        "The cron {label} step is invalid."
                    )));
                }
                parsed as u32
            }
        };
        let (start, end): (i64, i64) = if range_source == "*" {
            (i64::from(minimum), i64::from(maximum))
        } else if range_source.contains('-') {
            let parts: Vec<&str> = range_source.split('-').collect();
            if parts.len() != 2 {
                return Err(ContractError::InvalidValue(format!(
                    "The cron {label} range is invalid."
                )));
            }
            let r_start = parts[0].trim().parse::<i64>().map_err(|_| {
                ContractError::InvalidValue(format!("invalid cron {label} value"))
            })?;
            let r_end = parts[1].trim().parse::<i64>().map_err(|_| {
                ContractError::InvalidValue(format!("invalid cron {label} value"))
            })?;
            (r_start, r_end)
        } else {
            let single = range_source.trim().parse::<i64>().map_err(|_| {
                ContractError::InvalidValue(format!("invalid cron {label} value"))
            })?;
            (single, single)
        };
        let allowed_maximum = i64::from(if sunday_alias { 7 } else { maximum });
        if start < i64::from(minimum) || end > allowed_maximum || end < start {
            return Err(ContractError::InvalidValue(format!(
                "invalid cron {label} value: {segment}"
            )));
        }
        let mut value = start;
        while value <= end {
            let mapped = if sunday_alias && value == 7 {
                0u8
            } else {
                value as u8
            };
            result.insert(mapped);
            value += i64::from(step);
        }
    }
    if result.is_empty() {
        return Err(ContractError::InvalidValue(format!("empty cron {label} field")));
    }
    Ok(CronField {
        values: result.into_iter().collect(),
        wildcard,
    })
}

fn parse_cron_spec(expression: &str) -> Result<CronSpec, ContractError> {
    let values: Vec<&str> = expression.trim().split_whitespace().collect();
    if values.len() != 5 {
        return Err(ContractError::InvalidValue(
            "cron expression must have exactly 5 fields".to_string(),
        ));
    }
    Ok(CronSpec {
        minute: parse_cron_field(values[0], 0, 59, "minute", false)?,
        hour: parse_cron_field(values[1], 0, 23, "hour", false)?,
        day: parse_cron_field(values[2], 1, 31, "day", false)?,
        month: parse_cron_field(values[3], 1, 12, "month", false)?,
        weekday: parse_cron_field(values[4], 0, 6, "weekday", true)?,
        exact_times: None,
    })
}

pub fn parse_cron_expression(expression: &str) -> Result<CronSchedule, ContractError> {
    validate_cron_save_syntax(expression)?;
    let spec = parse_cron_spec(expression)?;
    Ok(CronSchedule {
        minutes: spec.minute.values.into_iter().collect(),
        hours: spec.hour.values.into_iter().collect(),
        days_of_month: spec.day.values.into_iter().collect(),
        months: spec.month.values.into_iter().collect(),
        days_of_week: spec.weekday.values.into_iter().collect(),
        day_wildcard: spec.day.wildcard,
        weekday_wildcard: spec.weekday.wildcard,
    })
}

fn field_values(values: &[u8]) -> CronField {
    let mut set = BTreeSet::new();
    for &v in values {
        set.insert(v);
    }
    CronField {
        values: set.into_iter().collect(),
        wildcard: false,
    }
}

fn wildcard_field(minimum: u8, maximum: u8) -> CronField {
    CronField {
        values: (minimum..=maximum).collect(),
        wildcard: true,
    }
}

fn stepped_values(minimum: u8, maximum: u8, step: u32) -> Vec<u8> {
    let mut out = Vec::new();
    let mut val = u32::from(minimum);
    while val <= u32::from(maximum) {
        out.push(val as u8);
        val = val.saturating_add(step.max(1));
    }
    out
}

fn parse_hh_mm(value: &str) -> Result<(u8, u8), ContractError> {
    if !is_routine_time(value) {
        return Err(ContractError::InvalidValue(format!("Invalid routine time: {value}")));
    }
    Ok((value[..2].parse().unwrap(), value[3..].parse().unwrap()))
}

fn interval_milliseconds(amount: u32, unit: RoutineIntervalUnit) -> i64 {
    let unit_ms = match unit {
        RoutineIntervalUnit::Minutes => 60_000i64,
        RoutineIntervalUnit::Hours => 3_600_000i64,
        RoutineIntervalUnit::Days => 86_400_000i64,
    };
    i64::from(amount).saturating_mul(unit_ms)
}

fn sub_day_interval_milliseconds(amount: u32, unit: RoutineSubDayUnit) -> i64 {
    let unit_ms = match unit {
        RoutineSubDayUnit::Minutes => 60_000i64,
        RoutineSubDayUnit::Hours => 3_600_000i64,
    };
    i64::from(amount).saturating_mul(unit_ms)
}

fn schedule_cron_spec(schedule: &RoutineSchedule) -> Result<CronSpec, ContractError> {
    match schedule {
        RoutineSchedule::Hourly { minute } => Ok(CronSpec {
            minute: field_values(&[*minute]),
            hour: wildcard_field(0, 23),
            day: wildcard_field(1, 31),
            month: wildcard_field(1, 12),
            weekday: wildcard_field(0, 6),
            exact_times: None,
        }),
        RoutineSchedule::Daily { time } => {
            let (hour, minute) = parse_hh_mm(time)?;
            Ok(CronSpec {
                minute: field_values(&[minute]),
                hour: field_values(&[hour]),
                day: wildcard_field(1, 31),
                month: wildcard_field(1, 12),
                weekday: wildcard_field(0, 6),
                exact_times: None,
            })
        }
        RoutineSchedule::Weekdays { time } => {
            let (hour, minute) = parse_hh_mm(time)?;
            Ok(CronSpec {
                minute: field_values(&[minute]),
                hour: field_values(&[hour]),
                day: wildcard_field(1, 31),
                month: wildcard_field(1, 12),
                weekday: field_values(&[1, 2, 3, 4, 5]),
                exact_times: None,
            })
        }
        RoutineSchedule::Weekly { weekday, time } => {
            let (hour, minute) = parse_hh_mm(time)?;
            Ok(CronSpec {
                minute: field_values(&[minute]),
                hour: field_values(&[hour]),
                day: wildcard_field(1, 31),
                month: wildcard_field(1, 12),
                weekday: field_values(&[*weekday]),
                exact_times: None,
            })
        }
        RoutineSchedule::Monthly { day, time } => {
            let (hour, minute) = parse_hh_mm(time)?;
            Ok(CronSpec {
                minute: field_values(&[minute]),
                hour: field_values(&[hour]),
                day: field_values(&[*day]),
                month: wildcard_field(1, 12),
                weekday: wildcard_field(0, 6),
                exact_times: None,
            })
        }
        RoutineSchedule::Advanced { months, days, time } => {
            let day = match days {
                RoutineDaySelection::DaysOfMonth { days } => field_values(days),
                _ => wildcard_field(1, 31),
            };
            let weekday = match days {
                RoutineDaySelection::DaysOfWeek { days } => field_values(days),
                _ => wildcard_field(0, 6),
            };
            match time {
                RoutineTimeSelection::Every { amount, unit } => {
                    let minutes = match unit {
                        RoutineSubDayUnit::Minutes => stepped_values(0, 59, *amount),
                        RoutineSubDayUnit::Hours => vec![0],
                    };
                    let hours = match unit {
                        RoutineSubDayUnit::Hours => stepped_values(0, 23, *amount),
                        RoutineSubDayUnit::Minutes => stepped_values(0, 23, 1),
                    };
                    Ok(CronSpec {
                        minute: field_values(&minutes),
                        hour: field_values(&hours),
                        day,
                        month: field_values(months),
                        weekday,
                        exact_times: None,
                    })
                }
                RoutineTimeSelection::AtTime { time } => {
                    let (hour, minute) = parse_hh_mm(time)?;
                    let mut exact = HashSet::new();
                    exact.insert((hour, minute));
                    Ok(CronSpec {
                        minute: field_values(&[minute]),
                        hour: field_values(&[hour]),
                        day,
                        month: field_values(months),
                        weekday,
                        exact_times: Some(exact),
                    })
                }
            }
        }
        RoutineSchedule::Custom { expression } => parse_cron_spec(expression),
        RoutineSchedule::Interval { .. } | RoutineSchedule::Once { .. } => Err(
            ContractError::InvalidValue("Non-cron routine schedule.".to_string()),
        ),
    }
}

fn zoned_parts(epoch_ms: i64, tz: Tz) -> Result<CalendarParts, ContractError> {
    let dt = tz
        .timestamp_millis_opt(epoch_ms)
        .single()
        .ok_or_else(|| ContractError::InvalidValue("Timestamp out of range.".to_string()))?;
    Ok(CalendarParts {
        year: dt.year(),
        month: dt.month() as u8,
        day: dt.day() as u8,
        hour: dt.hour() as u8,
        minute: dt.minute() as u8,
    })
}

fn zoned_date_time_candidates(
    year: i32,
    month: u8,
    day: u8,
    hour: u8,
    minute: u8,
    tz: Tz,
) -> Vec<i64> {
    let Some(naive_date) = NaiveDate::from_ymd_opt(year, u32::from(month), u32::from(day)) else {
        return Vec::new();
    };
    let Some(naive_dt) = naive_date.and_hms_opt(u32::from(hour), u32::from(minute), 0) else {
        return Vec::new();
    };
    match tz.from_local_datetime(&naive_dt) {
        LocalResult::None => Vec::new(),
        LocalResult::Single(c) => vec![c.timestamp_millis()],
        LocalResult::Ambiguous(a, b) => {
            let mut v = vec![a.timestamp_millis(), b.timestamp_millis()];
            v.sort_unstable();
            v.dedup();
            v
        }
    }
}

fn next_cron_occurrence(spec: &CronSpec, tz: Tz, after_ms: i64) -> Result<i64, ContractError> {
    let start = zoned_parts(after_ms, tz)?;
    let start_date = NaiveDate::from_ymd_opt(start.year, u32::from(start.month), u32::from(start.day))
        .ok_or_else(|| ContractError::InvalidValue("Invalid start date.".to_string()))?;
    let repeated_start = zoned_date_time_candidates(
        start.year,
        start.month,
        start.day,
        start.hour,
        start.minute,
        tz,
    )
    .len()
        > 1;

    for offset in 0..=MAX_SEARCH_DAYS {
        let Some(current) = start_date.checked_add_days(Days::new(offset)) else {
            break;
        };
        let year = current.year();
        let month = current.month() as u8;
        let day = current.day() as u8;
        let weekday = current.weekday().num_days_from_sunday() as u8;
        if !spec.month.values.contains(&month) {
            continue;
        }
        let day_matches = spec.day.values.contains(&day);
        let weekday_matches = spec.weekday.values.contains(&weekday);
        let calendar_matches = if spec.day.wildcard && spec.weekday.wildcard {
            true
        } else if spec.day.wildcard {
            weekday_matches
        } else if spec.weekday.wildcard {
            day_matches
        } else {
            day_matches || weekday_matches
        };
        if !calendar_matches {
            continue;
        }

        let mut repeated_candidate: Option<i64> = None;
        for &hour in &spec.hour.values {
            for &minute in &spec.minute.values {
                if let Some(exact) = &spec.exact_times
                    && !exact.contains(&(hour, minute))
                {
                    continue;
                }
                if offset == 0
                    && (hour < start.hour || (hour == start.hour && minute < start.minute))
                    && !repeated_start
                {
                    continue;
                }
                let candidates = zoned_date_time_candidates(year, month, day, hour, minute, tz);
                for (index, candidate) in candidates.into_iter().enumerate() {
                    if candidate <= after_ms {
                        continue;
                    }
                    if index == 0 {
                        return Ok(match repeated_candidate {
                            Some(rc) if rc < candidate => rc,
                            _ => candidate,
                        });
                    }
                    if repeated_candidate.is_none_or(|rc| candidate < rc) {
                        repeated_candidate = Some(candidate);
                    }
                }
            }
        }
        if let Some(rc) = repeated_candidate {
            return Ok(rc);
        }
    }
    Err(ContractError::InvalidValue(
        "Routine schedule has no occurrence within 5-year window.".to_string(),
    ))
}

fn has_runs_too_close(spec: &CronSpec, tz: Tz) -> Result<bool, ContractError> {
    let reference_ms = NaiveDate::from_ymd_opt(2026, 1, 1)
        .expect("2026-01-01")
        .and_hms_opt(0, 0, 0)
        .expect("00:00:00")
        .and_utc()
        .timestamp_millis();
    let mut previous = next_cron_occurrence(spec, tz, reference_ms)?;
    for _ in 0..200 {
        let next = next_cron_occurrence(spec, tz, previous)?;
        if next - previous < MINIMUM_INTERVAL_MS {
            return Ok(true);
        }
        previous = next;
    }
    Ok(false)
}

fn parse_anchor_ms(anchor_at: &str) -> Result<i64, ContractError> {
    if let Ok(dt) = DateTime::parse_from_rfc3339(anchor_at) {
        return Ok(dt.timestamp_millis());
    }
    if let Ok(dt) = DateTime::parse_from_str(anchor_at, "%Y-%m-%dT%H:%M:%S%.f%z") {
        return Ok(dt.timestamp_millis());
    }
    if is_calendar_date(anchor_at)
        && let Ok(d) = NaiveDate::parse_from_str(anchor_at, "%Y-%m-%d")
    {
        return Ok(d.and_hms_opt(0, 0, 0).unwrap().and_utc().timestamp_millis());
    }
    Err(ContractError::InvalidValue("Invalid routine anchorAt timestamp.".to_string()))
}

/// Exact port of `routineOnceInstant` (`routine-schedule.ts:L111-L131`).
pub fn routine_once_instant(date: &str, time: &str, timezone: &str) -> Result<i64, ContractError> {
    let tz = parse_timezone(timezone)?;
    if !is_calendar_date(date) {
        return Err(ContractError::InvalidValue("Invalid once date.".to_string()));
    }
    let d = NaiveDate::parse_from_str(date, "%Y-%m-%d")
        .map_err(|_| ContractError::InvalidValue("Invalid once date.".to_string()))?;
    let (hour, minute) = parse_hh_mm(time)?;
    let base_utc_ms = d
        .and_hms_opt(u32::from(hour), u32::from(minute), 0)
        .expect("valid time")
        .and_utc()
        .timestamp_millis();
    for step in 0..=MAX_GAP_MINUTES {
        let local_utc = Utc
            .timestamp_millis_opt(base_utc_ms + step * 60_000)
            .single()
            .ok_or_else(|| ContractError::InvalidValue("Once instant out of range.".to_string()))?;
        let candidates = zoned_date_time_candidates(
            local_utc.year(),
            local_utc.month() as u8,
            local_utc.day() as u8,
            local_utc.hour() as u8,
            local_utc.minute() as u8,
            tz,
        );
        if let Some(&first) = candidates.first() {
            return Ok(first);
        }
    }
    Err(ContractError::InvalidValue(
        "Routine once schedule has no valid occurrence.".to_string(),
    ))
}

/// Exact port of `validateStoredRoutineSchedule` (`routine-schedule.ts:L38-L62`).
pub fn validate_stored_routine_schedule(
    schedule: &RoutineSchedule,
    timezone: &str,
) -> Result<(), ContractError> {
    if !schedule.is_valid() {
        return Err(ContractError::InvalidValue("Invalid routine schedule.".to_string()));
    }
    let tz = parse_timezone(timezone)?;
    if let RoutineSchedule::Interval { amount, unit, .. } = schedule {
        if interval_milliseconds(*amount, *unit) < MINIMUM_INTERVAL_MS {
            return Err(ContractError::InvalidValue(
                "Routine interval must be at least 3 minutes (180s).".to_string(),
            ));
        }
        return Ok(());
    }
    if let RoutineSchedule::Advanced {
        time: RoutineTimeSelection::Every { amount, unit },
        ..
    } = schedule
        && sub_day_interval_milliseconds(*amount, *unit) < MINIMUM_INTERVAL_MS
    {
        return Err(ContractError::InvalidValue(
            "Advanced routine interval must be at least 3 minutes (180s).".to_string(),
        ));
    }
    if let RoutineSchedule::Custom { expression } = schedule {
        let spec = parse_cron_spec(expression)?;
        if has_runs_too_close(&spec, tz)? {
            return Err(ContractError::InvalidValue(
                "Custom cron schedule must not run more often than every 3 minutes (180s)."
                    .to_string(),
            ));
        }
    }
    Ok(())
}

/// Exact port of `validateRoutineSchedule` (`routine-schedule.ts:L75-L87`).
pub fn validate_routine_schedule(
    schedule: &RoutineSchedule,
    timezone: &str,
) -> Result<(), ContractError> {
    validate_stored_routine_schedule(schedule, timezone)?;
    let tz = parse_timezone(timezone)?;
    if let RoutineSchedule::Custom { expression } = schedule {
        validate_cron_save_syntax(expression)?;
    }
    if let RoutineSchedule::Advanced {
        time: RoutineTimeSelection::Every { .. },
        ..
    } = schedule
    {
        let spec = schedule_cron_spec(schedule)?;
        if has_runs_too_close(&spec, tz)? {
            return Err(ContractError::InvalidValue(
                "Advanced routine runs occur closer than 3 minutes (180s).".to_string(),
            ));
        }
    }
    Ok(())
}

/// Exact port of `nextValidRoutineOccurrence` (`routine-schedule.ts:L94-L109`).
pub fn next_valid_routine_occurrence(
    schedule: &RoutineSchedule,
    timezone: &str,
    after_ms: i64,
) -> Result<i64, ContractError> {
    let tz = parse_timezone(timezone)?;
    match schedule {
        RoutineSchedule::Once { date, time } => {
            let due = routine_once_instant(date, time, timezone)?;
            if after_ms < due {
                Ok(due)
            } else {
                Err(ContractError::InvalidValue("Routine once time has passed.".to_string()))
            }
        }
        RoutineSchedule::Interval {
            amount,
            unit,
            anchor_at,
        } => {
            let duration = interval_milliseconds(*amount, *unit);
            let anchor = parse_anchor_ms(anchor_at)?;
            if after_ms < anchor {
                return Ok(anchor);
            }
            let elapsed = after_ms - anchor;
            let steps = elapsed.div_euclid(duration) + 1;
            Ok(anchor + steps * duration)
        }
        _ => {
            let spec = schedule_cron_spec(schedule)?;
            next_cron_occurrence(&spec, tz, after_ms)
        }
    }
}

/// Exact port of `nextRoutineOccurrence` (`routine-schedule.ts:L89-L92`).
pub fn next_routine_occurrence(
    schedule: &RoutineSchedule,
    timezone: &str,
    after_ms: i64,
) -> Result<i64, ContractError> {
    validate_stored_routine_schedule(schedule, timezone)?;
    next_valid_routine_occurrence(schedule, timezone, after_ms)
}

/// Exact port of `collapseMissedOccurrences` (`routine-schedule.ts:L433-L446`).
pub fn collapse_missed_occurrences(
    schedule: &RoutineSchedule,
    timezone: &str,
    from_ms: i64,
    now_ms: i64,
) -> Result<(i64, i64), ContractError> {
    let mut scheduled_for = from_ms;
    let mut next_run_at = next_routine_occurrence(schedule, timezone, scheduled_for)?;
    while next_run_at <= now_ms {
        scheduled_for = next_run_at;
        next_run_at = next_routine_occurrence(schedule, timezone, scheduled_for)?;
    }
    Ok((scheduled_for, next_run_at))
}

impl RoutineSchedule {
    /// Enforces structural validity, save syntax, AND `>= 3m` (`180s`) minimum cadence (`routine-schedule.ts:L75-L87`).
    pub fn validate_minimum_interval(&self) -> Result<(), ContractError> {
        validate_routine_schedule(self, "UTC")
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RoutineTrigger {
    pub id: String,
    pub routine_id: String,
    pub schedule: RoutineSchedule,
    pub next_run_at: String,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum RoutineLimitPolicy {
    Wait,
    Skip,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum RoutineMissedPolicy {
    Skip,
    RunOnce,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum RoutineRunPlace {
    Chat,
    Thread,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RoutinePausedByRun {
    pub scheduled_for: String,
}

impl RoutinePausedByRun {
    pub fn is_valid(&self) -> bool {
        is_iso_timestamp(&self.scheduled_for)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Routine {
    pub id: String,
    pub agent_id: String,
    pub name: String,
    pub instruction: String,
    pub active: bool,
    pub timezone: String,
    pub trigger: RoutineTrigger,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "non_null_option"
    )]
    pub limit_policy: Option<RoutineLimitPolicy>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "non_null_option"
    )]
    pub missed_policy: Option<RoutineMissedPolicy>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "non_null_option"
    )]
    pub run_in: Option<RoutineRunPlace>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "non_null_option"
    )]
    pub paused_by_run: Option<RoutinePausedByRun>,
    pub created_at: String,
    pub updated_at: String,
}

/// Alias matching `PROJECT.md § Interface Contracts § 1` (`RoutineDefinition`).
pub type RoutineDefinition = Routine;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ChannelRoutine {
    pub id: String,
    pub channel_id: String,
    pub name: String,
    pub instruction: String,
    pub active: bool,
    pub timezone: String,
    pub trigger: RoutineTrigger,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "non_null_option"
    )]
    pub limit_policy: Option<RoutineLimitPolicy>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "non_null_option"
    )]
    pub missed_policy: Option<RoutineMissedPolicy>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "non_null_option"
    )]
    pub run_in: Option<RoutineRunPlace>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "non_null_option"
    )]
    pub paused_by_run: Option<RoutinePausedByRun>,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum RoutineRunStatus {
    Queued,
    Running,
    NeedsAttention,
    Succeeded,
    Failed,
    Interrupted,
    Cancelled,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ChannelRoutineRunStatus {
    Queued,
    Running,
    NeedsAttention,
    Succeeded,
    Failed,
    Cancelled,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum RoutineRunKind {
    Scheduled,
    Manual,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum RoutineRunSource {
    Test,
    Script,
    Webhook,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RoutineMissedRuns {
    pub count: u32,
    #[serde(deserialize_with = "Option::deserialize")]
    pub until: Option<String>,
}

impl RoutineMissedRuns {
    pub fn is_valid(&self) -> bool {
        self.count >= 1
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RoutineRun {
    pub id: String,
    pub routine_id: String,
    pub agent_id: String,
    #[serde(deserialize_with = "Option::deserialize")]
    pub delivery_id: Option<String>,
    #[serde(deserialize_with = "Option::deserialize")]
    pub trigger_id: Option<String>,
    pub kind: RoutineRunKind,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "non_null_option"
    )]
    pub source: Option<RoutineRunSource>,
    pub scheduled_for: String,
    pub routine_name: String,
    pub instruction: String,
    pub status: RoutineRunStatus,
    #[serde(deserialize_with = "Option::deserialize")]
    pub error: Option<String>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "non_null_option"
    )]
    pub missed: Option<RoutineMissedRuns>,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ChannelRoutineRun {
    pub id: String,
    pub routine_id: String,
    pub channel_id: String,
    #[serde(deserialize_with = "Option::deserialize")]
    pub request_message_id: Option<String>,
    #[serde(deserialize_with = "Option::deserialize")]
    pub trigger_id: Option<String>,
    pub kind: RoutineRunKind,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "non_null_option"
    )]
    pub source: Option<RoutineRunSource>,
    pub scheduled_for: String,
    pub routine_name: String,
    pub instruction: String,
    pub status: ChannelRoutineRunStatus,
    #[serde(deserialize_with = "Option::deserialize")]
    pub error: Option<String>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "non_null_option"
    )]
    pub missed: Option<RoutineMissedRuns>,
    pub created_at: String,
    pub updated_at: String,
}

fn stopped_blocker<'a>(
    tasks: &'a [ChannelTask],
    task: &'a ChannelTask,
    seen: &mut HashSet<&'a str>,
) -> Option<&'a ChannelTask> {
    for dep_id in &task.dependencies {
        if !seen.insert(dep_id.as_str()) {
            continue;
        }
        let Some(dep) = tasks.iter().find(|t| t.id == *dep_id) else {
            continue;
        };
        if matches!(dep.state, ChannelTaskState::Failed | ChannelTaskState::Paused) {
            return Some(dep);
        }
        if let Some(deeper) = stopped_blocker(tasks, dep, seen) {
            return Some(deeper);
        }
    }
    None
}

pub fn channel_run_status_for_tasks(
    tasks: &[ChannelTask],
    has_assignment: bool,
) -> (ChannelRoutineRunStatus, Option<String>) {
    if tasks.is_empty() {
        return (ChannelRoutineRunStatus::Cancelled, None);
    }
    if tasks.iter().any(|t| t.state == ChannelTaskState::Running) {
        return (ChannelRoutineRunStatus::Running, None);
    }
    let open: Vec<&ChannelTask> = tasks
        .iter()
        .filter(|t| matches!(t.state, ChannelTaskState::Queued | ChannelTaskState::Waiting))
        .collect();
    if !open.is_empty() {
        let blockers: Vec<Option<&ChannelTask>> = open
            .iter()
            .map(|t| stopped_blocker(tasks, t, &mut HashSet::new()))
            .collect();
        if blockers.iter().any(Option::is_none) {
            let st = if has_assignment {
                ChannelRoutineRunStatus::Running
            } else {
                ChannelRoutineRunStatus::Queued
            };
            return (st, None);
        }
        if let Some(Some(b)) = blockers
            .iter()
            .find(|b| b.is_some_and(|t| t.state == ChannelTaskState::Failed))
            .or_else(|| blockers.first())
        {
            let st = if b.state == ChannelTaskState::Failed {
                ChannelRoutineRunStatus::Failed
            } else {
                ChannelRoutineRunStatus::NeedsAttention
            };
            return (st, b.error.clone());
        }
    }
    if let Some(failed) = tasks.iter().find(|t| t.state == ChannelTaskState::Failed) {
        return (ChannelRoutineRunStatus::Failed, failed.error.clone());
    }
    if let Some(paused) = tasks.iter().find(|t| t.state == ChannelTaskState::Paused) {
        return (ChannelRoutineRunStatus::NeedsAttention, paused.error.clone());
    }
    if tasks
        .iter()
        .any(|t| t.parent_task_id.is_none() && t.state == ChannelTaskState::Cancelled)
    {
        return (ChannelRoutineRunStatus::Cancelled, None);
    }
    if let Some(rep) = tasks
        .iter()
        .find(|t| t.state == ChannelTaskState::Completed && t.error.is_some())
    {
        return (ChannelRoutineRunStatus::Failed, rep.error.clone());
    }
    (ChannelRoutineRunStatus::Succeeded, None)
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum EventScalar {
    Null,
    Bool(bool),
    Number(f64),
    String(String),
}

/// RFC-6901 JSON Pointer validator (`ipc-events.ts:isEventFilterPointer`, `L200-L204`).
pub fn is_event_filter_pointer(pointer: &str) -> bool {
    if utf16_len(pointer) > 2_048 {
        return false;
    }
    if pointer.is_empty() {
        return true;
    }
    if !pointer.starts_with('/') {
        return false;
    }
    let bytes = pointer.as_bytes();
    for (idx, &b) in bytes.iter().enumerate() {
        if b == b'~' && !matches!(bytes.get(idx + 1), Some(b'0' | b'1')) {
            return false;
        }
    }
    true
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct EventFilter {
    pub pointer: String,
    pub value: EventScalar,
}

impl EventFilter {
    pub fn is_valid(&self) -> bool {
        is_event_filter_pointer(&self.pointer)
            && match self.value {
                EventScalar::Number(n) => n.is_finite(),
                _ => true,
            }
    }
}

pub fn validate_event_json_budget(value: &serde_json::Value) -> bool {
    fn walk(val: &serde_json::Value, depth: usize, remaining: &mut isize) -> bool {
        if depth > 32 || *remaining <= 0 {
            return false;
        }
        *remaining -= 1;
        match val {
            serde_json::Value::Null | serde_json::Value::Bool(_) | serde_json::Value::String(_) => true,
            serde_json::Value::Number(n) => n.as_f64().is_some_and(f64::is_finite),
            serde_json::Value::Array(arr) => {
                arr.len() <= 10_000 && arr.iter().all(|v| walk(v, depth + 1, remaining))
            }
            serde_json::Value::Object(map) => {
                map.len() <= 10_000 && map.values().all(|v| walk(v, depth + 1, remaining))
            }
        }
    }
    let mut budget: isize = 10_000;
    walk(value, 0, &mut budget)
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "lowercase")]
pub enum EventRoutineTrigger {
    Schedule {
        schedule: RoutineSchedule,
    },
    Webhook {
        #[serde(deserialize_with = "Option::deserialize")]
        url: Option<String>,
        #[serde(rename = "eventType", deserialize_with = "Option::deserialize")]
        event_type: Option<String>,
        filters: Vec<EventFilter>,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "lowercase")]
pub enum EventRoutineOwner {
    Agent { id: String },
    Channel { id: String },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EventRoutine {
    pub id: String,
    pub owner: EventRoutineOwner,
    pub name: String,
    pub instruction: String,
    pub active: bool,
    pub timezone: String,
    pub trigger: EventRoutineTrigger,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "non_null_option"
    )]
    pub limit_policy: Option<RoutineLimitPolicy>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "non_null_option"
    )]
    pub missed_policy: Option<RoutineMissedPolicy>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "non_null_option"
    )]
    pub run_in: Option<RoutineRunPlace>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "non_null_option"
    )]
    pub paused_by_run: Option<RoutinePausedByRun>,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum QuietRunPolicy {
    #[default]
    AllowNoUpdate,
    AlwaysDeliver,
}

pub fn quiet_routine_run(kind: RoutineRunKind, source: Option<RoutineRunSource>) -> bool {
    kind == RoutineRunKind::Scheduled
        || matches!(
            source,
            Some(RoutineRunSource::Script | RoutineRunSource::Webhook)
        )
}

pub fn is_no_update_answer(text: &str) -> bool {
    let trimmed = text.trim();
    trimmed == ROUTINE_NO_UPDATE_MARKER || trimmed.eq_ignore_ascii_case(HERMES_SILENT_MARKER)
}

#[derive(Debug, Error, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WebhookSecurityError {
    #[error("webhook body exceeds 64 KiB")]
    BodyTooLarge,
    #[error("invalid webhook timestamp")]
    InvalidTimestamp,
    #[error("stale webhook timestamp (> 5m skew)")]
    StaleTimestamp,
    #[error("invalid webhook delivery id")]
    InvalidDeliveryId,
    #[error("invalid webhook signature")]
    InvalidSignature,
}

#[derive(Debug, Clone)]
pub struct WebhookSignatureInput<'a> {
    pub timestamp: &'a str,
    pub delivery_id: &'a str,
    pub body: &'a [u8],
    pub now_ms: i64,
}

pub const WEBHOOK_TIMESTAMP_HEADER: &str = "x-openbot-timestamp";
pub const WEBHOOK_DELIVERY_ID_HEADER: &str = "x-openbot-delivery-id";
pub const WEBHOOK_SIGNATURE_HEADER: &str = "x-openbot-signature";
pub const STANDARD_WEBHOOK_SECRET_PREFIX: &str = "whsec_";

static WEBHOOK_DELIVERY_ID_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^[A-Za-z0-9][A-Za-z0-9._:-]{0,255}$").expect("valid webhook delivery id regex")
});
static STANDARD_WEBHOOK_SIG_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^[a-z0-9]{1,8},[A-Za-z0-9+/=]{1,128}(?: [a-z0-9]{1,8},[A-Za-z0-9+/=]{1,128}){0,7}$")
        .expect("valid standard webhook signature regex")
});

fn constant_time_eq_32(a: &[u8], b: &[u8]) -> bool {
    if a.len() != 32 || b.len() != 32 {
        return false;
    }
    let mut diff = 0u8;
    for (&x, &y) in a.iter().zip(b.iter()) {
        diff |= x ^ y;
    }
    diff == 0
}

fn hmac_sha256(key: &[u8], prefix: &str, body: &[u8]) -> [u8; 32] {
    let mut mac = Hmac::<Sha256>::new_from_slice(key).expect("HMAC-SHA256 accepts any key length");
    mac.update(prefix.as_bytes());
    mac.update(body);
    let bytes = mac.finalize().into_bytes();
    let mut arr = [0u8; 32];
    arr.copy_from_slice(&bytes);
    arr
}

fn verify_webhook_input(input: &WebhookSignatureInput<'_>) -> Result<(), WebhookSecurityError> {
    if input.body.len() > WEBHOOK_DELIVERY_BODY_BYTES_LIMIT {
        return Err(WebhookSecurityError::BodyTooLarge);
    }
    if input.timestamp.is_empty()
        || input.timestamp.len() > 12
        || !input.timestamp.bytes().all(|b| b.is_ascii_digit())
    {
        return Err(WebhookSecurityError::InvalidTimestamp);
    }
    let ts_sec: i64 = input
        .timestamp
        .parse()
        .map_err(|_| WebhookSecurityError::InvalidTimestamp)?;
    let skew_ms = (ts_sec.saturating_mul(1_000) - input.now_ms).abs();
    if skew_ms > WEBHOOK_MAX_CLOCK_SKEW_SECONDS * 1_000 {
        return Err(WebhookSecurityError::StaleTimestamp);
    }
    if !WEBHOOK_DELIVERY_ID_RE.is_match(input.delivery_id) {
        return Err(WebhookSecurityError::InvalidDeliveryId);
    }
    Ok(())
}

pub fn sign_webhook_body(secret: &str, timestamp: &str, delivery_id: &str, body: &[u8]) -> String {
    let digest = hmac_sha256(secret.as_bytes(), &format!("{timestamp}.{delivery_id}."), body);
    format!("sha256={}", hex::encode(digest))
}

pub fn verify_webhook_signature(
    secret: &str,
    input: &WebhookSignatureInput<'_>,
    signature: &str,
) -> Result<(), WebhookSecurityError> {
    verify_webhook_input(input)?;
    let hex_part = signature
        .strip_prefix("sha256=")
        .or_else(|| signature.strip_prefix("SHA256="))
        .ok_or(WebhookSecurityError::InvalidSignature)?;
    if hex_part.len() != 64 {
        return Err(WebhookSecurityError::InvalidSignature);
    }
    let supplied = hex::decode(hex_part).map_err(|_| WebhookSecurityError::InvalidSignature)?;
    let expected = hmac_sha256(
        secret.as_bytes(),
        &format!("{}.{}.", input.timestamp, input.delivery_id),
        input.body,
    );
    if !constant_time_eq_32(&expected, &supplied) {
        return Err(WebhookSecurityError::InvalidSignature);
    }
    Ok(())
}

fn standard_webhook_keys(secret: &str) -> Vec<Vec<u8>> {
    let mut keys = vec![secret.as_bytes().to_vec()];
    if let Some(encoded) = secret.strip_prefix(STANDARD_WEBHOOK_SECRET_PREFIX) {
        if let Ok(decoded) = BASE64_STD.decode(encoded)
            && decoded.len() >= 16
            && BASE64_STD.encode(&decoded) == encoded
        {
            keys.push(decoded);
        }
        if let Ok(decoded) = BASE64_URL.decode(encoded).or_else(|_| URL_SAFE_NO_PAD.decode(encoded))
            && decoded.len() >= 16
            && (BASE64_URL.encode(&decoded) == encoded
                || URL_SAFE_NO_PAD.encode(&decoded) == encoded)
        {
            keys.push(decoded);
        }
    }
    keys
}

/// Standard Webhooks / Svix verifier (`webhook-security.ts:L77-L89`, note `{deliveryId}.{timestamp}.` order!).
pub fn verify_standard_webhook_signature(
    secret: &str,
    input: &WebhookSignatureInput<'_>,
    signature: &str,
) -> Result<(), WebhookSecurityError> {
    verify_webhook_input(input)?;
    if !STANDARD_WEBHOOK_SIG_RE.is_match(signature) {
        return Err(WebhookSecurityError::InvalidSignature);
    }
    let supplied: Vec<Vec<u8>> = signature
        .split(' ')
        .filter_map(|entry| entry.strip_prefix("v1,"))
        .filter_map(|b64| BASE64_STD.decode(b64).ok())
        .filter(|bytes| bytes.len() == 32)
        .collect();
    if supplied.is_empty() {
        return Err(WebhookSecurityError::InvalidSignature);
    }
    let prefix = format!("{}.{}.", input.delivery_id, input.timestamp);
    let expected: Vec<[u8; 32]> = standard_webhook_keys(secret)
        .into_iter()
        .map(|k| hmac_sha256(&k, &prefix, input.body))
        .collect();
    let matched = supplied
        .iter()
        .any(|sup| expected.iter().any(|exp| constant_time_eq_32(exp, sup)));
    if !matched {
        return Err(WebhookSecurityError::InvalidSignature);
    }
    Ok(())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum RoutineFlowStepStatus {
    Running,
    Succeeded,
    Failed,
    Skipped,
    Cancelled,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RoutineFlowLink {
    pub id: String,
    pub routine_id: String,
    pub from_agent_id: String,
    pub to_agent_id: String,
    pub instruction: String,
    pub created_at: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RoutineFlowStep {
    pub id: String,
    pub run_id: String,
    pub agent_id: String,
    #[serde(deserialize_with = "Option::deserialize")]
    pub delivery_id: Option<String>,
    pub input: String,
    #[serde(deserialize_with = "Option::deserialize")]
    pub output: Option<String>,
    pub status: RoutineFlowStepStatus,
    #[serde(deserialize_with = "Option::deserialize")]
    pub error: Option<String>,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RoutineFlowPosition {
    pub node_key: String,
    pub x: f64,
    pub y: f64,
}

/// `Omit<EventRoutine, "owner"> & { agentId: string }` (`ipc-routine-flows.ts:L38`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RoutineFlowRoutineInfo {
    pub id: String,
    pub agent_id: String,
    pub name: String,
    pub instruction: String,
    pub active: bool,
    pub timezone: String,
    pub trigger: EventRoutineTrigger,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "non_null_option"
    )]
    pub limit_policy: Option<RoutineLimitPolicy>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "non_null_option"
    )]
    pub missed_policy: Option<RoutineMissedPolicy>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "non_null_option"
    )]
    pub run_in: Option<RoutineRunPlace>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "non_null_option"
    )]
    pub paused_by_run: Option<RoutinePausedByRun>,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RoutineFlowRoutine {
    pub routine: RoutineFlowRoutineInfo,
    pub recent_runs: Vec<RoutineRun>,
    pub upcoming_runs: Vec<String>,
    pub steps: Vec<RoutineFlowStep>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RoutineFlowCanvas {
    pub agent_id: String,
    pub routines: Vec<RoutineFlowRoutine>,
    pub links: Vec<RoutineFlowLink>,
    pub positions: Vec<RoutineFlowPosition>,
    pub placed_agent_ids: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum MessagingPlatform {
    Slack,
    Discord,
    Telegram,
    #[serde(rename = "whatsapp")]
    WhatsApp,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum RoutineResultDeliveryStatus {
    Pending,
    Sending,
    Sent,
    Failed,
    Uncertain,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum RoutineResultDeliveryReason {
    Expired,
    ConnectionGone,
    Refused,
    SendFailed,
    Restart,
    Stopped,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RoutineResultDestination {
    pub connection_id: String,
    pub platform: MessagingPlatform,
    pub connection_name: String,
    pub place_id: String,
    pub place_name: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RoutineResultDelivery {
    pub run_id: String,
    pub status: RoutineResultDeliveryStatus,
    #[serde(deserialize_with = "Option::deserialize")]
    pub reason: Option<RoutineResultDeliveryReason>,
    pub attempts: u32,
    pub destination: RoutineResultDestination,
    pub updated_at: String,
    #[serde(deserialize_with = "Option::deserialize")]
    pub sent_at: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RoutineFeed {
    #[serde(deserialize_with = "Option::deserialize")]
    pub url: Option<String>,
}

pub fn is_routine_flow_node_key(key: &str) -> bool {
    let Some((kind, id)) = key.split_once(':') else {
        return false;
    };
    !id.contains(':') && matches!(kind, "agent" | "routine") && is_identifier(id)
}
