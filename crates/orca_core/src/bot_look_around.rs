//! Pure-Rust Bot Look Around schedule, deterministic sleep catch-up calculator,
//! read-only agent enforcement, Up Next suggestions, URL normalization, and
//! credential-URL security filtering (Deliverable #5).

use std::collections::HashSet;
use std::fmt;

use chrono::{DateTime, Datelike, Days, FixedOffset, NaiveDate, TimeDelta, Utc, Weekday};
use serde::{Deserialize, Serialize};

use crate::bot_memory::looks_secret_line;
use crate::bot_types::{BotAgentId, BotLookAroundConfig};
use crate::{BotError, BotErrorCode};

// ---------------------------------------------------------------------------
// Constants & Read-Only Agent Guard
// ---------------------------------------------------------------------------

pub const LOOK_AROUND_AGENTS: [BotAgentId; 2] = [BotAgentId::Claude, BotAgentId::Codex];
pub const LOOK_AROUND_FOCUS_MAX_LEN: usize = 500;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SuggestionLimits {
    pub max_per_run: usize,
    pub title_max: usize,
    pub why_max: usize,
    pub ask_max: usize,
    pub shown: usize,
}

pub const SUGGESTION_LIMITS: SuggestionLimits = SuggestionLimits {
    max_per_run: 5,
    title_max: 80,
    why_max: 400,
    ask_max: 2_000,
    shown: 3,
};

pub const fn is_look_around_agent_supported(agent: BotAgentId) -> bool {
    agent.supports_look_around()
}

pub fn is_look_around_agent_str_supported(raw: &str) -> bool {
    BotAgentId::parse_strict(raw).is_some_and(BotAgentId::supports_look_around)
}

pub fn ensure_look_around_agent(agent: BotAgentId) -> Result<(), BotError> {
    if is_look_around_agent_supported(agent) {
        return Ok(());
    }
    Err(BotError::new(
        BotErrorCode::InvalidArgument,
        format!(
            "Look Around is only supported for read-only Claude and Codex bots (got '{}').",
            agent.as_str()
        ),
    ))
}

// ---------------------------------------------------------------------------
// Schedule Presets, State & Patch Semantics
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum LookAroundEvery {
    #[default]
    #[serde(rename = "every-4h", alias = "every4h")]
    Every4h,
    #[serde(rename = "every-2h", alias = "every2h")]
    Every2h,
    TwiceDaily,
    Mornings,
}

impl LookAroundEvery {
    pub const ALL: [Self; 4] = [
        Self::Every4h,
        Self::Every2h,
        Self::TwiceDaily,
        Self::Mornings,
    ];

    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Every4h => "every-4h",
            Self::Every2h => "every-2h",
            Self::TwiceDaily => "twice-daily",
            Self::Mornings => "mornings",
        }
    }

    pub fn parse(raw: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|v| v.as_str() == raw)
    }

    pub const fn hours(self) -> &'static [u8] {
        match self {
            Self::Every4h => &[8, 12, 16, 20],
            Self::Every2h => &[8, 10, 12, 14, 16, 18, 20],
            Self::TwiceDaily => &[8, 13],
            Self::Mornings => &[8],
        }
    }

    pub const fn interval_ms(self) -> u64 {
        match self {
            Self::Every4h => 4 * 3_600_000,
            Self::Every2h => 2 * 3_600_000,
            Self::TwiceDaily => 5 * 3_600_000,
            Self::Mornings => 24 * 3_600_000,
        }
    }

    pub const fn cron_expression(self, weekdays_only: bool) -> &'static str {
        match (self, weekdays_only) {
            (Self::Every4h, true) => "0 8,12,16,20 * * 1-5",
            (Self::Every4h, false) => "0 8,12,16,20 * * *",
            (Self::Every2h, true) => "0 8,10,12,14,16,18,20 * * 1-5",
            (Self::Every2h, false) => "0 8,10,12,14,16,18,20 * * *",
            (Self::TwiceDaily, true) => "0 8,13 * * 1-5",
            (Self::TwiceDaily, false) => "0 8,13 * * *",
            (Self::Mornings, true) => "0 8 * * 1-5",
            (Self::Mornings, false) => "0 8 * * *",
        }
    }
}

impl fmt::Display for LookAroundEvery {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

fn default_weekdays_only() -> bool {
    true
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LookAroundSchedule {
    pub every: LookAroundEvery,
    #[serde(default = "default_weekdays_only", alias = "weekdaysOnly")]
    pub weekdays_only: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub focus: Option<String>,
    #[serde(default, alias = "sinceMs", alias = "since", skip_serializing_if = "Option::is_none")]
    pub since_ms: Option<i64>,
}

impl Default for LookAroundSchedule {
    fn default() -> Self {
        Self {
            every: LookAroundEvery::Every4h,
            weekdays_only: true,
            focus: None,
            since_ms: None,
        }
    }
}

impl LookAroundSchedule {
    pub fn validate(&self) -> Result<(), BotError> {
        if let Some(ref focus) = self.focus
            && focus.chars().count() > LOOK_AROUND_FOCUS_MAX_LEN
        {
            return Err(BotError::new(
                BotErrorCode::InvalidArgument,
                format!("Look Around focus exceeds {LOOK_AROUND_FOCUS_MAX_LEN} characters."),
            ));
        }
        Ok(())
    }

    pub const fn cron_expression(&self) -> &'static str {
        self.every.cron_expression(self.weekdays_only)
    }

    /// Explicit conversion from persisted M1.1 `BotLookAroundConfig`.
    pub fn from_bot_config(cfg: &BotLookAroundConfig) -> Result<Self, BotError> {
        let every = match cfg.schedule.as_deref() {
            Some(raw) => LookAroundEvery::parse(raw).ok_or_else(|| {
                BotError::new(
                    BotErrorCode::InvalidArgument,
                    format!("Unknown Look Around schedule preset '{raw}'."),
                )
            })?,
            None => LookAroundEvery::Every4h,
        };
        Ok(Self {
            every,
            weekdays_only: cfg.weekdays_only,
            focus: None,
            since_ms: None,
        })
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct LookAroundState {
    #[serde(default, alias = "enabled")]
    pub on: bool,
    #[serde(default)]
    pub schedule: LookAroundSchedule,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct LookAroundPatch {
    pub on: Option<bool>,
    pub every: Option<LookAroundEvery>,
    pub weekdays_only: Option<bool>,
    pub focus: Option<Option<String>>,
    pub since_ms: Option<i64>,
}

impl LookAroundState {
    pub fn from_bot_config(cfg: &BotLookAroundConfig) -> Result<Self, BotError> {
        Ok(Self {
            on: cfg.enabled,
            schedule: LookAroundSchedule::from_bot_config(cfg)?,
        })
    }

    pub fn due_now(
        &self,
        now_ms: i64,
        last_run_at_ms: Option<i64>,
        utc_offset_seconds: i32,
    ) -> Result<bool, BotError> {
        if !self.on {
            return Ok(false);
        }
        due_now(&self.schedule, now_ms, last_run_at_ms, utc_offset_seconds)
    }
}

/// Applies a validated patch to `LookAroundState`:
/// - `on: false -> true` stamps `since_ms = Some(now_ms)`.
/// - `on: true -> true` and `on: true -> false` preserve `since_ms` untouched.
pub fn patch_look_around(
    current: &LookAroundState,
    patch: LookAroundPatch,
    now_ms: i64,
) -> Result<LookAroundState, BotError> {
    let mut next = current.clone();
    if let Some(every) = patch.every {
        next.schedule.every = every;
    }
    if let Some(weekdays_only) = patch.weekdays_only {
        next.schedule.weekdays_only = weekdays_only;
    }
    if let Some(focus_opt) = patch.focus {
        next.schedule.focus = focus_opt;
    }
    next.schedule.validate()?;

    let next_on = patch.on.unwrap_or(current.on);
    if let Some(explicit_since) = patch.since_ms {
        next.schedule.since_ms = Some(explicit_since);
    } else if !current.on && next_on {
        next.schedule.since_ms = Some(now_ms);
    }
    next.on = next_on;
    Ok(next)
}

/// Parses an RFC-3339 timestamp string into UTC epoch milliseconds.
pub fn parse_rfc3339_ms(raw: &str) -> Result<i64, BotError> {
    DateTime::parse_from_rfc3339(raw.trim())
        .map(|dt| dt.timestamp_millis())
        .map_err(|e| BotError::new(BotErrorCode::InvalidArgument, format!("Invalid RFC-3339 timestamp: {e}")))
}

// ---------------------------------------------------------------------------
// Deterministic Sleep Catch-Up Evaluation (`evaluate_look_around` / `due_now`)
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct LookAroundEvaluation {
    #[serde(rename = "dueNow", alias = "due_now")]
    pub due_now: bool,
    #[serde(rename = "latestSlotMs", alias = "latest_slot_ms", skip_serializing_if = "Option::is_none")]
    pub latest_slot_ms: Option<i64>,
    #[serde(rename = "nextSlotMs", alias = "next_slot_ms", skip_serializing_if = "Option::is_none")]
    pub next_slot_ms: Option<i64>,
}

fn slot_timestamp_ms(
    offset_delta: TimeDelta,
    date: NaiveDate,
    hour: u8,
) -> Result<i64, BotError> {
    let dt = date
        .and_hms_opt(u32::from(hour), 0, 0)
        .ok_or_else(|| BotError::new(BotErrorCode::InvalidArgument, "Invalid schedule hour."))?;
    let utc_naive = dt.checked_sub_signed(offset_delta).ok_or_else(|| {
        BotError::new(
            BotErrorCode::InvalidArgument,
            "Schedule slot timestamp out of calendar range.",
        )
    })?;
    Ok(utc_naive.and_utc().timestamp_millis())
}

fn next_eligible_day(mut date: NaiveDate, weekdays_only: bool) -> Result<NaiveDate, BotError> {
    for _ in 0..7 {
        date = date.checked_add_days(Days::new(1)).ok_or_else(|| {
            BotError::new(BotErrorCode::InvalidArgument, "Schedule date overflow.")
        })?;
        if !weekdays_only || !matches!(date.weekday(), Weekday::Sat | Weekday::Sun) {
            return Ok(date);
        }
    }
    Err(BotError::new(
        BotErrorCode::InvalidArgument,
        "Unable to compute next eligible schedule day.",
    ))
}

/// Deterministically evaluates whether a Look Around schedule is due at `now_ms`
/// under fixed timezone offset `utc_offset_seconds` (`-86_399..=86_399`).
pub fn evaluate_look_around(
    schedule: &LookAroundSchedule,
    now_ms: i64,
    last_run_at_ms: Option<i64>,
    utc_offset_seconds: i32,
) -> Result<LookAroundEvaluation, BotError> {
    schedule.validate()?;
    if FixedOffset::east_opt(utc_offset_seconds).is_none() {
        return Err(BotError::new(
            BotErrorCode::InvalidArgument,
            format!("Invalid UTC offset seconds: {utc_offset_seconds}"),
        ));
    }
    let offset_delta = TimeDelta::seconds(i64::from(utc_offset_seconds));
    let now_utc = DateTime::<Utc>::from_timestamp_millis(now_ms).ok_or_else(|| {
        BotError::new(
            BotErrorCode::InvalidArgument,
            format!("Invalid epoch timestamp milliseconds: {now_ms}"),
        )
    })?;

    let local_naive = now_utc
        .naive_utc()
        .checked_add_signed(offset_delta)
        .ok_or_else(|| {
            BotError::new(
                BotErrorCode::InvalidArgument,
                "Local timestamp out of calendar range.",
            )
        })?;
    let local_date = local_naive.date();
    let hours = schedule.every.hours();
    let first_hour = hours.first().copied().ok_or_else(|| {
        BotError::new(BotErrorCode::InvalidArgument, "Schedule has no slot hours.")
    })?;

    if schedule.weekdays_only && matches!(local_date.weekday(), Weekday::Sat | Weekday::Sun) {
        let next_slot_ms = next_eligible_day(local_date, true)
            .ok()
            .and_then(|d| slot_timestamp_ms(offset_delta, d, first_hour).ok());
        return Ok(LookAroundEvaluation {
            due_now: false,
            latest_slot_ms: None,
            next_slot_ms,
        });
    }

    let mut latest_slot_ms: Option<i64> = None;
    let mut next_slot_ms: Option<i64> = None;
    for &h in hours {
        let slot_ms = slot_timestamp_ms(offset_delta, local_date, h)?;
        if slot_ms <= now_ms {
            latest_slot_ms = Some(slot_ms);
        } else if next_slot_ms.is_none() {
            next_slot_ms = Some(slot_ms);
        }
    }

    if next_slot_ms.is_none() {
        next_slot_ms = next_eligible_day(local_date, schedule.weekdays_only)
            .ok()
            .and_then(|d| slot_timestamp_ms(offset_delta, d, first_hour).ok());
    }

    let is_due = match latest_slot_ms {
        None => false,
        Some(slot_ms) => {
            let after_since = schedule.since_ms.is_none_or(|since| slot_ms >= since);
            let not_already_ran = last_run_at_ms.is_none_or(|last| last < slot_ms);
            after_since && not_already_ran
        }
    };

    Ok(LookAroundEvaluation {
        due_now: is_due,
        latest_slot_ms,
        next_slot_ms,
    })
}

pub fn due_now(
    schedule: &LookAroundSchedule,
    now_ms: i64,
    last_run_at_ms: Option<i64>,
    utc_offset_seconds: i32,
) -> Result<bool, BotError> {
    Ok(evaluate_look_around(schedule, now_ms, last_run_at_ms, utc_offset_seconds)?.due_now)
}

// ---------------------------------------------------------------------------
// Up Next Suggestions, Pure-Rust URL Normalization & Credential Filtering
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Suggestion {
    pub id: String,
    pub title: String,
    pub why: String,
    pub ask: String,
    #[serde(default, alias = "url", skip_serializing_if = "Option::is_none")]
    pub link: Option<String>,
    #[serde(
        default,
        alias = "createdAtMs",
        alias = "createdAt",
        skip_serializing_if = "Option::is_none"
    )]
    pub created_at_ms: Option<i64>,
}

impl Suggestion {
    pub fn validate(&self) -> Result<(), BotError> {
        if self.id.trim().is_empty() || self.id.chars().any(char::is_control) {
            return Err(BotError::new(
                BotErrorCode::InvalidArgument,
                "Suggestion id must be non-empty and cannot contain control characters.",
            ));
        }
        if self.title.chars().any(char::is_control) {
            return Err(BotError::new(
                BotErrorCode::InvalidArgument,
                "Suggestion title cannot contain control characters.",
            ));
        }
        if self.title.trim().is_empty() || self.title.chars().count() > SUGGESTION_LIMITS.title_max {
            return Err(BotError::new(
                BotErrorCode::InvalidArgument,
                format!(
                    "Suggestion title must be 1..={} characters.",
                    SUGGESTION_LIMITS.title_max
                ),
            ));
        }
        if self
            .why
            .chars()
            .any(|ch| ch.is_control() && !matches!(ch, '\n' | '\r' | '\t'))
        {
            return Err(BotError::new(
                BotErrorCode::InvalidArgument,
                "Suggestion why cannot contain non-whitespace control characters.",
            ));
        }
        if self.why.trim().is_empty() || self.why.chars().count() > SUGGESTION_LIMITS.why_max {
            return Err(BotError::new(
                BotErrorCode::InvalidArgument,
                format!(
                    "Suggestion why must be 1..={} characters.",
                    SUGGESTION_LIMITS.why_max
                ),
            ));
        }
        if self
            .ask
            .chars()
            .any(|ch| ch.is_control() && !matches!(ch, '\n' | '\r' | '\t'))
        {
            return Err(BotError::new(
                BotErrorCode::InvalidArgument,
                "Suggestion ask cannot contain non-whitespace control characters.",
            ));
        }
        if self.ask.trim().is_empty() || self.ask.chars().count() > SUGGESTION_LIMITS.ask_max {
            return Err(BotError::new(
                BotErrorCode::InvalidArgument,
                format!(
                    "Suggestion ask must be 1..={} characters.",
                    SUGGESTION_LIMITS.ask_max
                ),
            ));
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum UpNextState {
    #[default]
    Idle,
    Running,
    Ready,
    Failed,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct BotUpNext {
    pub state: UpNextState,
    #[serde(default)]
    pub suggestions: Vec<Suggestion>,
    pub shown: usize,
    pub overflow: usize,
}

impl BotUpNext {
    pub fn from_suggestions(state: UpNextState, suggestions: Vec<Suggestion>) -> Self {
        let shown = suggestions.len().min(SUGGESTION_LIMITS.shown);
        let overflow = suggestions.len().saturating_sub(SUGGESTION_LIMITS.shown);
        Self {
            state,
            suggestions,
            shown,
            overflow,
        }
    }
}

struct ParsedHttpUrl<'a> {
    scheme_lower: &'static str,
    host_lower: String,
    port: Option<u16>,
    path: &'a str,
}

fn is_valid_hostname_char(b: u8) -> bool {
    b.is_ascii_alphanumeric() || matches!(b, b'-' | b'.' | b'_')
}

fn parse_host_and_port(host_port: &str) -> Option<(String, Option<u16>)> {
    if host_port.is_empty() {
        return None;
    }
    if let Some(rest) = host_port.strip_prefix('[') {
        let close = rest.find(']')?;
        let ipv6 = rest.get(..close)?;
        if ipv6.is_empty()
            || !ipv6
                .bytes()
                .all(|b| b.is_ascii_hexdigit() || matches!(b, b':' | b'.'))
        {
            return None;
        }
        let after = rest.get(close + 1..)?;
        let port = if after.is_empty() {
            None
        } else {
            let p_str = after.strip_prefix(':')?;
            if p_str.is_empty() || !p_str.bytes().all(|b| b.is_ascii_digit()) {
                return None;
            }
            Some(p_str.parse::<u16>().ok()?)
        };
        return Some((format!("[{}]", ipv6.to_ascii_lowercase()), port));
    }

    if host_port.contains('[') || host_port.contains(']') {
        return None;
    }

    let (host, port) = match host_port.rsplit_once(':') {
        Some((h, p_str)) => {
            if h.contains(':') || p_str.is_empty() || !p_str.bytes().all(|b| b.is_ascii_digit()) {
                return None;
            }
            (h, Some(p_str.parse::<u16>().ok()?))
        }
        None => (host_port, None),
    };

    if host.is_empty()
        || !host.bytes().all(is_valid_hostname_char)
        || host.starts_with('.')
        || host.ends_with('.')
        || host.contains("..")
    {
        return None;
    }
    Some((host.to_ascii_lowercase(), port))
}

fn parse_http_url(raw: &str) -> Option<ParsedHttpUrl<'_>> {
    let trimmed = raw.trim();
    if trimmed.is_empty() || trimmed.chars().any(|c| c.is_control() || c.is_whitespace()) {
        return None;
    }
    let (scheme_raw, after_scheme) = trimmed.split_once("://")?;
    let scheme_lower = if scheme_raw.eq_ignore_ascii_case("https") {
        "https"
    } else if scheme_raw.eq_ignore_ascii_case("http") {
        "http"
    } else {
        return None;
    };

    let before_frag = after_scheme
        .split_once('#')
        .map_or(after_scheme, |(b, _)| b);
    let before_query = before_frag.split_once('?').map_or(before_frag, |(b, _)| b);
    let (authority, path) = match before_query.split_once('/') {
        Some((a, p)) => (a, p),
        None => (before_query, ""),
    };
    if authority.is_empty() {
        return None;
    }

    let host_port = authority.rsplit_once('@').map_or(authority, |(_, hp)| hp);
    let (host_lower, port) = parse_host_and_port(host_port)?;

    Some(ParsedHttpUrl {
        scheme_lower,
        host_lower,
        port,
        path,
    })
}

fn fold_title(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut in_ws = false;
    for ch in s.trim().chars() {
        if ch.is_whitespace() {
            if !in_ws {
                out.push(' ');
                in_ws = true;
            }
        } else {
            in_ws = false;
            for lc in ch.to_lowercase() {
                out.push(lc);
            }
        }
    }
    out
}

/// Normalizes a suggestion's link or falls back to case/whitespace-folded title.
/// - GitHub (`github.com` / `www.github.com`): normalizes to `github.com`, truncating
///   tails past `/pull/<digits>` and `/issues/<digits>` (preserving non-numeric `/pull/new/main`).
/// - Linear (`linear.app` / `www.linear.app`): normalizes to `linear.app`, truncating
///   tails past `/<ws>/issue/<key>` or `/issue/<key>`.
/// - Generic HTTP(S) URLs: drops scheme, userinfo, default port, query, fragment, trailing slash.
/// - Fallback (missing/malformed/non-http(s)): `title:<folded>`.
pub fn suggestion_key(title: &str, link: Option<&str>) -> String {
    if let Some(raw_link) = link
        && let Some(parsed) = parse_http_url(raw_link)
    {
        let mut host = parsed.host_lower;
        let is_github = host == "github.com" || host == "www.github.com";
        let is_linear = host == "linear.app" || host == "www.linear.app";
        if is_github {
            host = "github.com".to_string();
        } else if is_linear {
            host = "linear.app".to_string();
        }

        let default_port = match parsed.scheme_lower {
            "http" => Some(80),
            "https" => Some(443),
            _ => None,
        };
        let host_port = match parsed.port {
            Some(p) if Some(p) != default_port => format!("{host}:{p}"),
            _ => host,
        };

        let trimmed_path = parsed.path.trim_matches('/');
        let segs: Vec<&str> = trimmed_path
            .split('/')
            .filter(|s| !s.is_empty())
            .collect();

        let canonical_path = if is_github && segs.len() >= 4 {
            let s0 = segs[0];
            let s1 = segs[1];
            let s2 = segs[2];
            let s3 = segs[3];
            let is_numeric = !s3.is_empty() && s3.bytes().all(|b| b.is_ascii_digit());
            if is_numeric && (s2.eq_ignore_ascii_case("pull") || s2.eq_ignore_ascii_case("pulls")) {
                format!("/{}/{}/pull/{s3}", s0.to_lowercase(), s1.to_lowercase())
            } else if is_numeric && s2.eq_ignore_ascii_case("issues") {
                format!("/{}/{}/issues/{s3}", s0.to_lowercase(), s1.to_lowercase())
            } else if trimmed_path.is_empty() {
                String::new()
            } else {
                format!("/{trimmed_path}")
            }
        } else if is_linear && segs.len() >= 3 && segs[1].eq_ignore_ascii_case("issue") {
            format!(
                "/{}/issue/{}",
                segs[0].to_lowercase(),
                segs[2].to_lowercase()
            )
        } else if is_linear && segs.len() >= 2 && segs[0].eq_ignore_ascii_case("issue") {
            format!("/issue/{}", segs[1].to_lowercase())
        } else if trimmed_path.is_empty() {
            String::new()
        } else {
            format!("/{trimmed_path}")
        };

        return format!("link:{host_port}{canonical_path}");
    }

    format!("title:{}", fold_title(title))
}

fn hex_val(b: u8) -> Option<u8> {
    match b {
        b'0'..=b'9' => Some(b - b'0'),
        b'a'..=b'f' => Some(b - b'a' + 10),
        b'A'..=b'F' => Some(b - b'A' + 10),
        _ => None,
    }
}

fn percent_decode_once(input: &str, plus_as_space: bool) -> String {
    let bytes = input.as_bytes();
    let mut out: Vec<u8> = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        let Some(rest) = bytes.get(i..) else {
            break;
        };
        let next_special = if plus_as_space {
            rest.iter().position(|&b| b == b'%' || b == b'+')
        } else {
            rest.iter().position(|&b| b == b'%')
        };
        let Some(rel) = next_special else {
            out.extend_from_slice(rest);
            break;
        };
        if let Some(chunk) = rest.get(..rel) {
            out.extend_from_slice(chunk);
        }
        let pos = i.saturating_add(rel);
        let Some(&b) = bytes.get(pos) else {
            break;
        };
        if b == b'%'
            && let Some(dec) = decode_pct_at(bytes, pos)
        {
            out.push(dec);
            i = pos.saturating_add(3);
            continue;
        }
        if plus_as_space && b == b'+' {
            out.push(b' ');
        } else {
            out.push(b);
        }
        i = pos.saturating_add(1);
    }
    String::from_utf8_lossy(&out).into_owned()
}

const PERCENT_DECODE_MAX_PASSES: usize = 8;

fn percent_decode_once_preserve_pct20(input: &str) -> String {
    let bytes = input.as_bytes();
    let mut out: Vec<u8> = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        let Some(rest) = bytes.get(i..) else {
            break;
        };
        let Some(rel) = rest.iter().position(|&b| b == b'%') else {
            out.extend_from_slice(rest);
            break;
        };
        if let Some(chunk) = rest.get(..rel) {
            out.extend_from_slice(chunk);
        }
        let pos = i.saturating_add(rel);
        if let Some(dec) = decode_pct_at(bytes, pos) {
            if matches!(dec, b' ' | b'+' | b',') {
                if let Some(raw_pct) = bytes.get(pos..pos.saturating_add(3)) {
                    out.extend_from_slice(raw_pct);
                }
            } else {
                out.push(dec);
            }
            i = pos.saturating_add(3);
            continue;
        }
        if let Some(&b) = bytes.get(pos) {
            out.push(b);
        }
        i = pos.saturating_add(1);
    }
    String::from_utf8_lossy(&out).into_owned()
}

fn should_run_keep_pct20(s: &str) -> bool {
    let bytes = s.as_bytes();
    let mut has_preserved_sep = false;
    let mut has_encoded_auth = false;
    let mut i = 0;
    while i < bytes.len() {
        let Some(rel) = bytes.get(i..).and_then(|r| r.iter().position(|&b| b == b'%')) else {
            break;
        };
        let pos = i.saturating_add(rel);
        if let Some(dec) = decode_pct_at(bytes, pos) {
            if matches!(dec, b' ' | b'+' | b',') {
                has_preserved_sep = true;
            } else if dec == b'@'
                || (matches!(dec, b'/' | b'\\')
                    && decode_pct_at(bytes, pos.saturating_add(3))
                        .is_some_and(|d2| matches!(d2, b'/' | b'\\')))
            {
                has_encoded_auth = true;
            }
            if has_preserved_sep && has_encoded_auth {
                return true;
            }
            i = pos.saturating_add(3);
            continue;
        }
        i = pos.saturating_add(1);
    }
    false
}

fn has_pct_json_or_url_structural_escape(s: &str) -> bool {
    let bytes = s.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        let Some(rel) = bytes.get(i..).and_then(|r| r.iter().position(|&b| b == b'%')) else {
            break;
        };
        let pos = i.saturating_add(rel);
        if let Some(dec) = decode_pct_at(bytes, pos) {
            if matches!(
                dec,
                b'{' | b'}' | b'[' | b']' | b'"' | b'\\' | b'?' | b'#' | b'=' | b'&' | b';'
            ) {
                return true;
            }
            i = pos.saturating_add(3);
            continue;
        }
        i = pos.saturating_add(1);
    }
    false
}

fn has_non_pct25_escape(s: &str) -> bool {
    let bytes = s.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        let Some(rel) = bytes.get(i..).and_then(|r| r.iter().position(|&b| b == b'%')) else {
            break;
        };
        let pos = i.saturating_add(rel);
        if let Some(dec) = decode_pct_at(bytes, pos) {
            if dec != b'%' {
                return true;
            }
            i = pos.saturating_add(3);
            continue;
        }
        i = pos.saturating_add(1);
    }
    false
}

fn parse_hex4(bytes: &[u8], start: usize) -> Option<u32> {
    let h0 = u32::from(hex_val(bytes.get(start).copied()?)?);
    let h1 = u32::from(hex_val(bytes.get(start.checked_add(1)?).copied()?)?);
    let h2 = u32::from(hex_val(bytes.get(start.checked_add(2)?).copied()?)?);
    let h3 = u32::from(hex_val(bytes.get(start.checked_add(3)?).copied()?)?);
    Some((h0 << 12) | (h1 << 8) | (h2 << 4) | h3)
}

fn has_backslash_u_escape(s: &str) -> bool {
    let bytes = s.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        let Some(rel) = bytes.get(i..).and_then(|r| r.iter().position(|&b| b == b'\\')) else {
            break;
        };
        let pos = i.saturating_add(rel);
        if matches!(bytes.get(pos.saturating_add(1)), Some(b'u' | b'U')) {
            return true;
        }
        i = pos.saturating_add(1);
    }
    false
}

fn unescape_json_u_escapes_once(s: &str) -> Option<String> {
    let bytes = s.as_bytes();
    let mut out: Vec<u8> = Vec::with_capacity(bytes.len());
    let mut changed = false;
    let mut i = 0;
    while i < bytes.len() {
        let Some(rest) = bytes.get(i..) else {
            break;
        };
        let Some(rel) = rest.iter().position(|&b| b == b'\\') else {
            out.extend_from_slice(rest);
            break;
        };
        if let Some(chunk) = rest.get(..rel) {
            out.extend_from_slice(chunk);
        }
        let pos = i.saturating_add(rel);
        // Collapse `\\uXXXX` -> `\uXXXX` so double-escaped JSON strings unescape cleanly in pass 2.
        if bytes.get(pos.saturating_add(1)) == Some(&b'\\')
            && matches!(bytes.get(pos.saturating_add(2)), Some(b'u' | b'U'))
            && parse_hex4(bytes, pos.saturating_add(3)).is_some()
        {
            out.push(b'\\');
            changed = true;
            i = pos.saturating_add(2);
            continue;
        }
        if matches!(bytes.get(pos.saturating_add(1)), Some(b'u' | b'U'))
            && let Some(cp) = parse_hex4(bytes, pos.saturating_add(2))
            && let Some(ch) = char::from_u32(cp)
        {
            let mut buf = [0u8; 4];
            out.extend_from_slice(ch.encode_utf8(&mut buf).as_bytes());
            changed = true;
            i = pos.saturating_add(6);
            continue;
        }
        out.push(b'\\');
        i = pos.saturating_add(1);
    }
    changed.then(|| String::from_utf8_lossy(&out).into_owned())
}

fn unescape_json_u_escapes(s: &str) -> Option<String> {
    let first = unescape_json_u_escapes_once(s)?;
    if has_backslash_u_escape(&first)
        && let Some(second) = unescape_json_u_escapes_once(&first)
    {
        return Some(second);
    }
    Some(first)
}

fn percent_decode_fixed_point(input: &str, plus_as_space: bool, max_passes: usize) -> String {
    let mut cur = input.to_string();
    for _ in 0..max_passes {
        let has_pct = cur.as_bytes().contains(&b'%');
        let has_plus = plus_as_space && cur.as_bytes().contains(&b'+');
        if !has_pct && !has_plus {
            break;
        }
        let next = percent_decode_once(&cur, plus_as_space);
        if next == cur {
            break;
        }
        cur = next;
    }
    cur
}

fn is_secret_param_key(raw_key: &str) -> bool {
    let trimmed = raw_key.trim();
    if trimmed.len() < 3 || !trimmed.as_bytes().iter().any(u8::is_ascii_alphabetic) {
        return false;
    }
    let lower = trimmed.to_ascii_lowercase();
    let mut prev_tok: Option<&str> = None;
    let mut has_sig_or_pair = false;
    for tok in lower
        .split(|c: char| !c.is_ascii_alphanumeric())
        .filter(|t| !t.is_empty())
    {
        if tok == "sig" {
            has_sig_or_pair = true;
            break;
        }
        if tok == "key" && matches!(prev_tok, Some("api" | "access")) {
            has_sig_or_pair = true;
            break;
        }
        prev_tok = Some(tok);
    }
    has_sig_or_pair
        || lower.contains("token")
        || lower.contains("secret")
        || lower.contains("passw")
        || lower.contains("apikey")
        || lower.contains("accesskey")
        || lower.contains("signature")
        || lower.contains("auth")
        || lower.contains("credential")
        || lower.contains("session")
}

fn contains_secret_token_or_webhook_key(s: &str) -> bool {
    looks_secret_line(s)
        || crate::triggers_webhooks::redact_webhook_keys(s) != s
        || s.contains(crate::triggers_webhooks::REDACTED_WEBHOOK_KEY)
}

fn decode_pct_at(bytes: &[u8], idx: usize) -> Option<u8> {
    if bytes.get(idx).copied() != Some(b'%') {
        return None;
    }
    let hi = hex_val(bytes.get(idx.checked_add(1)?).copied()?)?;
    let lo = hex_val(bytes.get(idx.checked_add(2)?).copied()?)?;
    Some((hi << 4) | lo)
}

const MAX_URL_SCHEME_LEN: usize = 64;

fn has_url_scheme_prefix(s: &str) -> bool {
    let bytes = s.as_bytes();
    let Some(&first) = bytes.first() else {
        return false;
    };
    if !first.is_ascii_alphabetic() {
        return false;
    }
    let limit = bytes.len().min(MAX_URL_SCHEME_LEN);
    let mut i = 1;
    while i < limit {
        let Some(&b) = bytes.get(i) else {
            break;
        };
        if b == b':' {
            return true;
        }
        if !(b.is_ascii_alphanumeric() || matches!(b, b'+' | b'-' | b'.')) {
            return false;
        }
        i = i.saturating_add(1);
    }
    false
}

fn is_whatwg_special_scheme(scheme: &str) -> bool {
    scheme.eq_ignore_ascii_case("http")
        || scheme.eq_ignore_ascii_case("https")
        || scheme.eq_ignore_ascii_case("ws")
        || scheme.eq_ignore_ascii_case("wss")
        || scheme.eq_ignore_ascii_case("ftp")
}

fn is_known_url_scheme(scheme: &str) -> bool {
    is_whatwg_special_scheme(scheme)
        || scheme.eq_ignore_ascii_case("postgres")
        || scheme.eq_ignore_ascii_case("postgresql")
        || scheme.eq_ignore_ascii_case("redis")
        || scheme.eq_ignore_ascii_case("mysql")
        || scheme.eq_ignore_ascii_case("mongodb")
        || scheme.eq_ignore_ascii_case("mongodb+srv")
        || scheme.eq_ignore_ascii_case("amqp")
        || scheme.eq_ignore_ascii_case("amqps")
        || scheme.eq_ignore_ascii_case("ssh")
        || scheme.eq_ignore_ascii_case("sftp")
        || scheme.eq_ignore_ascii_case("ldap")
        || scheme.eq_ignore_ascii_case("ldaps")
}

fn is_plus_joined_known_scheme(scheme: &str) -> bool {
    let Some((_, tail)) = scheme.rsplit_once('+') else {
        return false;
    };
    is_known_url_scheme(tail)
}

fn is_valid_hostname_str(s: &str) -> bool {
    !s.is_empty()
        && s.bytes().all(is_valid_hostname_char)
        && !s.starts_with('.')
        && !s.ends_with('.')
        && !s.contains("..")
}

fn is_valid_rfc_host_port(hp: &str) -> bool {
    if hp.is_empty() || hp.len() > 260 {
        return false;
    }
    if let Some(rest) = hp.strip_prefix('[') {
        let Some(close) = rest.find(']') else {
            return false;
        };
        let Some(ipv6) = rest.get(..close) else {
            return false;
        };
        if ipv6.is_empty()
            || !ipv6
                .bytes()
                .all(|b| b.is_ascii_hexdigit() || matches!(b, b':' | b'.'))
        {
            return false;
        }
        let Some(after) = rest.get(close.saturating_add(1)..) else {
            return false;
        };
        if after.is_empty() {
            return true;
        }
        let Some(p_str) = after.strip_prefix(':') else {
            return false;
        };
        return !p_str.is_empty()
            && p_str.bytes().all(|b| b.is_ascii_digit())
            && p_str.parse::<u16>().is_ok_and(|p| p > 0);
    }
    if hp.contains('[') || hp.contains(']') {
        return false;
    }
    let host = match hp.rsplit_once(':') {
        Some((h, p_str)) => {
            if h.contains(':')
                || p_str.is_empty()
                || !p_str.bytes().all(|b| b.is_ascii_digit())
                || !p_str.parse::<u16>().is_ok_and(|p| p > 0)
            {
                return false;
            }
            h
        }
        None => hp,
    };
    if host.is_empty()
        || host.len() > 253
        || host.starts_with('.')
        || host.ends_with('.')
        || host.contains("..")
    {
        return false;
    }
    host.split('.')
        .all(|label| !label.is_empty() && label.len() <= 63 && label.bytes().all(is_valid_hostname_char))
}

fn is_valid_target_host(u: &url::Url) -> bool {
    match u.host() {
        Some(url::Host::Ipv4(_) | url::Host::Ipv6(_)) => true,
        Some(url::Host::Domain(d)) => {
            !d.is_empty()
                && !d.chars().any(|c| {
                    c.is_whitespace()
                        || matches!(
                            c,
                            '=' | ','
                                | ';'
                                | '&'
                                | '\''
                                | '"'
                                | '('
                                | ')'
                                | '+'
                                | '['
                                | ']'
                                | '{'
                                | '}'
                                | '|'
                                | '^'
                                | '`'
                                | '*'
                                | '~'
                                | '!'
                        )
                })
        }
        None => false,
    }
}

fn has_at_or_unicode_escape(s: &str) -> bool {
    let bytes = s.as_bytes();
    if bytes.contains(&b'@') {
        return true;
    }
    let mut i = 0;
    while i < bytes.len() {
        let Some(rel) = bytes
            .get(i..)
            .and_then(|r| r.iter().position(|&b| b == b'\\' || b == b'%'))
        else {
            break;
        };
        let pos = i.saturating_add(rel);
        let Some(&b) = bytes.get(pos) else {
            break;
        };
        if b == b'\\' {
            if matches!(bytes.get(pos.saturating_add(1)), Some(b'u' | b'U')) {
                return true;
            }
        } else if decode_pct_at(bytes, pos) == Some(b'\\')
            && matches!(bytes.get(pos.saturating_add(3)), Some(b'u' | b'U'))
        {
            return true;
        }
        i = pos.saturating_add(1);
    }
    false
}

fn strip_proto_rel_prefix(s: &str) -> Option<&str> {
    let bytes = s.as_bytes();
    let mut end = 0;
    while matches!(bytes.get(end), Some(b'/' | b'\\')) {
        end = end.saturating_add(1);
    }
    if end >= 2 { s.get(end..) } else { None }
}

fn starts_with_pct_ci(s: &str, pct3: &str) -> bool {
    s.get(..3).is_some_and(|p| p.eq_ignore_ascii_case(pct3))
}

fn ends_with_pct_ci(s: &str, pct3: &str) -> bool {
    let Some(start) = s.len().checked_sub(3) else {
        return false;
    };
    s.get(start..).is_some_and(|p| p.eq_ignore_ascii_case(pct3))
}

fn find_pct_ci(s: &str, pct3: &str) -> Option<usize> {
    let bytes = s.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        let rel = bytes
            .get(i..)
            .and_then(|r| r.iter().position(|&b| b == b'%'))?;
        let pos = i.saturating_add(rel);
        if s.get(pos..pos.saturating_add(3))
            .is_some_and(|p| p.eq_ignore_ascii_case(pct3))
        {
            return Some(pos);
        }
        i = pos.saturating_add(1);
    }
    None
}

fn strip_outer_wrappers(s: &str) -> &str {
    let mut cur = s;
    for _ in 0..4 {
        let starts_opener = matches!(
            cur.as_bytes().first(),
            Some(b'(' | b'[' | b'{' | b'\'' | b'"' | b'<')
        );
        let work = if starts_opener {
            cur.trim_end_matches(['.', ';', ','])
        } else {
            cur
        };
        let mut stripped = None;
        for (open, close) in [
            ('(', ')'),
            ('[', ']'),
            ('{', '}'),
            ('\'', '\''),
            ('"', '"'),
            ('<', '>'),
        ] {
            if let Some(inner) = work.strip_prefix(open).and_then(|r| r.strip_suffix(close)) {
                stripped = Some(inner);
                break;
            }
        }
        if let Some(inner) = stripped {
            cur = inner;
        } else {
            break;
        }
    }
    cur
}

fn is_proto_rel_prev_char(ch: char) -> bool {
    !ch.is_ascii_alphanumeric() && !matches!(ch, ':' | '/' | '\\')
}

/// Strictly O(1) (decodes at most 72 trailing `[0-9A-Fa-f%]` bytes) check for whether
/// `prefix` ends with a literal or multi-pass percent-encoded non-ASCII-alphanumeric
/// delimiter (rejecting `:`, `/`, `\` when `allow_colon_slash == false`).
fn prefix_ends_with_delim(prefix: &str, in_raw_pass: bool, allow_colon_slash: bool) -> bool {
    let char_ok = |ch: char| {
        if allow_colon_slash {
            !ch.is_ascii_alphanumeric()
        } else {
            is_proto_rel_prev_char(ch)
        }
    };
    match prefix.chars().next_back() {
        None => return false,
        Some(':' | '/' | '\\') if !allow_colon_slash => return false,
        Some(ch) if char_ok(ch) => return true,
        Some(_) => {}
    }
    if !in_raw_pass || !prefix.as_bytes().last().is_some_and(u8::is_ascii_hexdigit) {
        return false;
    }
    let bytes = prefix.as_bytes();
    let cap = bytes.len().saturating_sub(72);
    let mut start = bytes.len();
    while start > cap {
        let prev = start.saturating_sub(1);
        let Some(&b) = bytes.get(prev) else {
            break;
        };
        if b == b'%' || b.is_ascii_hexdigit() {
            start = prev;
        } else {
            break;
        }
    }
    while start < bytes.len() && !prefix.is_char_boundary(start) {
        start = start.saturating_add(1);
    }
    let Some(tail) = prefix.get(start..) else {
        return false;
    };
    if !tail.as_bytes().contains(&b'%') {
        return false;
    }
    let decoded = percent_decode_fixed_point(tail, false, PERCENT_DECODE_MAX_PASSES);
    match decoded.chars().next_back() {
        None => false,
        Some(':' | '/' | '\\') if !allow_colon_slash => false,
        Some(ch) => char_ok(ch),
    }
}

fn prefix_ends_with_proto_rel_delim(prefix: &str, in_raw_pass: bool) -> bool {
    prefix_ends_with_delim(prefix, in_raw_pass, false)
}

fn prefix_ends_with_scheme_delim(prefix: &str, in_raw_pass: bool) -> bool {
    prefix_ends_with_delim(prefix, in_raw_pass, true)
}

fn parse_url_or_proto_rel(s: &str, base: &url::Url) -> Option<(url::Url, bool)> {
    if let Some(after_slashes) = strip_proto_rel_prefix(s) {
        let norm = format!("//{after_slashes}");
        return base.join(&norm).ok().map(|u| (u, true));
    }
    if has_url_scheme_prefix(s) {
        return url::Url::parse(s).ok().map(|u| (u, false));
    }
    None
}

fn try_pct_host_fallback_at(
    s: &str,
    base: &url::Url,
    auth_start: usize,
    auth_end: usize,
    rel_at: usize,
) -> Option<(url::Url, bool)> {
    let at_pos = auth_start.saturating_add(rel_at);
    let host_start = at_pos.checked_add(1)?;
    let host_tail = s.get(host_start..auth_end)?;
    let bytes = host_tail.as_bytes();
    let starts_ipv6 = bytes.first() == Some(&b'[');
    let mut seen_ipv6_close = false;
    let mut seen_port_colon = false;
    let limit = bytes.len().min(264);
    let mut i = 0;
    while i < limit {
        let Some(&b) = bytes.get(i) else {
            break;
        };
        if starts_ipv6 && !seen_ipv6_close {
            if i == 0 {
                i = 1;
                continue;
            }
            if b == b']' {
                seen_ipv6_close = true;
                i = i.saturating_add(1);
                continue;
            }
            if b.is_ascii_hexdigit() || matches!(b, b':' | b'.') {
                i = i.saturating_add(1);
                continue;
            }
            return None;
        }
        let is_valid_host_byte = if b == b':' {
            if !seen_port_colon
                && bytes
                    .get(i.saturating_add(1))
                    .is_some_and(u8::is_ascii_digit)
            {
                seen_port_colon = true;
                true
            } else {
                false
            }
        } else if seen_port_colon {
            b.is_ascii_digit()
        } else {
            is_valid_hostname_char(b)
        };
        if is_valid_host_byte {
            i = i.saturating_add(1);
            continue;
        }
        let is_delim = if b == b'%' {
            decode_pct_at(bytes, i)
                .is_some_and(|dec| !dec.is_ascii_alphanumeric() && !matches!(dec, b'-' | b'.' | b'_'))
        } else {
            true
        };
        if is_delim {
            let prefix_hp = host_tail.get(..i)?;
            if is_valid_rfc_host_port(prefix_hp) {
                let trunc = s.get(..host_start.saturating_add(i))?;
                return parse_url_or_proto_rel(trunc, base);
            }
            return None;
        }
        i = i.saturating_add(1);
    }
    None
}

fn parse_candidate_with_pct_host_fallback(s: &str, base: &url::Url) -> Option<(url::Url, bool)> {
    let bytes_all = s.as_bytes();
    let auth_start = if let Some(after) = strip_proto_rel_prefix(s) {
        s.len().saturating_sub(after.len())
    } else if has_url_scheme_prefix(s)
        && let Some(colon) = s.find(':')
    {
        let mut p = colon.saturating_add(1);
        while matches!(bytes_all.get(p), Some(b'/' | b'\\')) {
            p = p.saturating_add(1);
        }
        p
    } else {
        return None;
    };
    let rest_after_scheme = s.get(auth_start..)?;
    let auth_rel_end = rest_after_scheme
        .find(['/', '\\', '?', '#'])
        .unwrap_or(rest_after_scheme.len());
    let auth_end = auth_start.saturating_add(auth_rel_end);
    let auth_slice = s.get(auth_start..auth_end)?;
    let rel_at_opt = auth_slice.rfind('@');

    let host_definitely_invalid = rel_at_opt.is_some_and(|rel_at| {
        rel_at > 0
            && auth_slice.get(rel_at.saturating_add(1)..).is_some_and(|ht| {
                ht.bytes().any(|b| {
                    b.is_ascii_whitespace()
                        || matches!(
                            b,
                            b'=' | b','
                                | b';'
                                | b'&'
                                | b'\''
                                | b'"'
                                | b'('
                                | b')'
                                | b'+'
                                | b'{'
                                | b'}'
                                | b'|'
                                | b'^'
                                | b'`'
                                | b'*'
                                | b'~'
                                | b'!'
                        )
                })
            })
    });

    if !host_definitely_invalid
        && let Some((u, is_proto_rel)) = parse_url_or_proto_rel(s, base)
        && ((u.username().is_empty() && u.password().is_none()) || is_valid_target_host(&u))
    {
        return Some((u, is_proto_rel));
    }
    let rel_at = rel_at_opt?;
    if let Some(res) = try_pct_host_fallback_at(s, base, auth_start, auth_end, rel_at) {
        return Some(res);
    }
    if let Some(first_at) = auth_slice.find('@')
        && first_at != rel_at
    {
        return try_pct_host_fallback_at(s, base, auth_start, auth_end, first_at);
    }
    None
}

fn earliest_query_prose_sep(s: &str) -> Option<(usize, usize, bool)> {
    let lit = s.find(['+', ',']).map(|p| (p, 1usize, false));
    let pct20 = find_pct_ci(s, "%20").map(|p| (p, 3usize, true));
    match (lit, pct20) {
        (Some(a), Some(b)) => Some(if a.0 <= b.0 { a } else { b }),
        (Some(a), None) => Some(a),
        (None, Some(b)) => Some(b),
        (None, None) => None,
    }
}

fn is_benign_search_qualifier(tok: &str) -> bool {
    tok.eq_ignore_ascii_case("author")
        || tok.eq_ignore_ascii_case("mailto")
        || tok.eq_ignore_ascii_case("contact")
        || tok.eq_ignore_ascii_case("email")
        || tok.eq_ignore_ascii_case("from")
        || tok.eq_ignore_ascii_case("to")
        || tok.eq_ignore_ascii_case("cc")
        || tok.eq_ignore_ascii_case("npm")
        || tok.eq_ignore_ascii_case("owner")
        || tok.eq_ignore_ascii_case("assignee")
        || tok.eq_ignore_ascii_case("mentions")
        || tok.eq_ignore_ascii_case("commenter")
        || tok.eq_ignore_ascii_case("involves")
}

fn is_standard_web_port(scheme: &str, port: u16) -> bool {
    if scheme.eq_ignore_ascii_case("https") {
        matches!(port, 443 | 8443)
    } else if scheme.eq_ignore_ascii_case("http") {
        matches!(port, 80 | 3000 | 4000 | 5000 | 5173 | 8000 | 8080 | 8888)
    } else {
        false
    }
}

fn hosts_same_or_subdomain(a: &str, b: &str) -> bool {
    if a.eq_ignore_ascii_case(b) {
        return true;
    }
    let check_sub = |longer: &str, shorter: &str| {
        longer.len() > shorter.len()
            && longer
                .get(longer.len().saturating_sub(shorter.len())..)
                .is_some_and(|tail| tail.eq_ignore_ascii_case(shorter))
            && longer
                .as_bytes()
                .get(longer.len().saturating_sub(shorter.len()).saturating_sub(1))
                == Some(&b'.')
    };
    check_sub(a, b) || check_sub(b, a)
}

fn raw_userinfo_prefix(cand: &str) -> Option<&str> {
    let before_at = cand.rsplit_once('@')?.0;
    if let Some(after) = strip_proto_rel_prefix(before_at) {
        return Some(after);
    }
    let colon = before_at.find(':')?;
    let mut start = colon.saturating_add(1);
    let bytes = before_at.as_bytes();
    while matches!(bytes.get(start), Some(b'/' | b'\\')) {
        start = start.saturating_add(1);
    }
    before_at.get(start..)
}

fn is_clean_email_local(s: &str) -> bool {
    s.bytes()
        .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'-'))
}

fn split_earliest_colon_or_pct3a(s: &str) -> Option<(&str, &str)> {
    let lit = s.find(':').map(|p| (p, 1usize));
    let pct = find_pct_ci(s, "%3A").map(|p| (p, 3usize));
    let (pos, len) = match (lit, pct) {
        (Some(a), Some(b)) => {
            if a.0 <= b.0 {
                a
            } else {
                b
            }
        }
        (Some(a), None) | (None, Some(a)) => a,
        (None, None) => return None,
    };
    Some((s.get(..pos)?, s.get(pos.saturating_add(len)..)?))
}

fn amp_segments_are_benign(rest: &str, has_path_or_port: bool) -> bool {
    let mut any_seg = false;
    for seg in rest.split('&') {
        let Some((k, v)) = seg.split_once('=').or_else(|| {
            let pos = find_pct_ci(seg, "%3D")?;
            Some((seg.get(..pos)?, seg.get(pos.saturating_add(3)..)?))
        }) else {
            return false;
        };
        if k.is_empty() {
            return false;
        }
        if let Some((before_c, after_c)) = split_earliest_colon_or_pct3a(v)
            && (has_path_or_port
                || split_earliest_colon_or_pct3a(after_c).is_some()
                || !is_benign_search_qualifier(before_c))
        {
            return false;
        }
        any_seg = true;
    }
    any_seg
}

fn match_trailing_search_qualifier(after_sep: &str) -> Option<(&str, bool)> {
    if is_benign_search_qualifier(after_sep) {
        return Some((after_sep, false));
    }
    let bytes = after_sep.as_bytes();
    let mut word_start = 0;
    let mut i = 0;
    let mut word_count: usize = 0;
    while i < bytes.len() {
        let sep_len = if matches!(bytes.get(i), Some(b'+' | b' ')) {
            1
        } else if bytes.get(i) == Some(&b'%') && starts_with_pct_ci(after_sep.get(i..).unwrap_or(""), "%20") {
            3
        } else {
            0
        };
        if sep_len > 0 {
            let word = after_sep.get(word_start..i)?;
            if word.is_empty()
                || !word
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_'))
            {
                return None;
            }
            word_count = word_count.saturating_add(1);
            i = i.saturating_add(sep_len);
            word_start = i;
        } else {
            i = i.saturating_add(1);
        }
    }
    let tail = after_sep.get(word_start..)?;
    if tail.is_empty() || word_count == 0 || !is_benign_search_qualifier(tail) {
        return None;
    }
    Some((tail, true))
}

fn is_clean_prose_after_sep(after_sep: &str) -> bool {
    if after_sep.is_empty() {
        return true;
    }
    if let Some((before_c, after_c)) = split_earliest_colon_or_pct3a(after_sep) {
        if match_trailing_search_qualifier(before_c).is_none() {
            return false;
        }
        let after_clean = if starts_with_pct_ci(after_c, "%20") {
            after_c.get(3..).unwrap_or("")
        } else {
            after_c.strip_prefix(['+', ' ']).unwrap_or(after_c)
        };
        return is_clean_email_local(after_clean);
    }
    let work = if ends_with_pct_ci(after_sep, "%20") {
        after_sep
            .len()
            .checked_sub(3)
            .and_then(|e| after_sep.get(..e))
            .unwrap_or("")
    } else {
        after_sep.strip_suffix(['+', ' ']).unwrap_or(after_sep)
    };
    if work.is_empty() {
        return true;
    }
    let bytes = work.as_bytes();
    let mut word_start = 0;
    let mut i = 0;
    while i < bytes.len() {
        let sep_len = if matches!(bytes.get(i), Some(b'+' | b' ')) {
            1
        } else if bytes.get(i) == Some(&b'%')
            && starts_with_pct_ci(work.get(i..).unwrap_or(""), "%20")
        {
            3
        } else {
            0
        };
        if sep_len > 0 {
            let Some(word) = work.get(word_start..i) else {
                return false;
            };
            if word.is_empty() || !is_clean_email_local(word) {
                return false;
            }
            i = i.saturating_add(sep_len);
            word_start = i;
        } else {
            i = i.saturating_add(1);
        }
    }
    work.get(word_start..)
        .is_some_and(|w| !w.is_empty() && is_clean_email_local(w))
}

fn last_amp_param_key_is_benign_contact(between: &str) -> bool {
    let last_seg = between.rsplit_once('&').map_or(between, |(_, seg)| seg);
    let Some((k, _)) = last_seg.split_once('=').or_else(|| {
        let pos = find_pct_ci(last_seg, "%3D")?;
        Some((last_seg.get(..pos)?, last_seg.get(pos.saturating_add(3)..)?))
    }) else {
        return false;
    };
    is_benign_search_qualifier(k)
}

fn is_benign_embedded_userinfo(
    cand: &str,
    u: &url::Url,
    base: &url::Url,
    is_proto_rel: bool,
    from_query_or_frag: bool,
    in_prose_after_prefix: bool,
) -> bool {
    if !from_query_or_frag || !matches!(u.scheme(), "http" | "https") || u.port().is_some() {
        return false;
    }
    let before_at = cand.rsplit_once('@').map_or(cand, |(b, _)| b);

    // 1. Partially percent-encoded path/query URL (%2F / %3F before '@', never %5C).
    let has_pct3f = find_pct_ci(before_at, "%3F").is_some();
    if find_pct_ci(before_at, "%5C").is_none()
        && (find_pct_ci(before_at, "%2F").is_some() || has_pct3f)
    {
        let dec1 = percent_decode_once(cand, false);
        if let Some((u_dec, _)) = parse_url_or_proto_rel(&dec1, base)
            && u_dec.username().is_empty()
            && u_dec.password().is_none()
            && let Some(dec_host) = u_dec.host_str()
        {
            let host_and_port_ok = if let Some(pw) = u.password() {
                let first_pct = match (find_pct_ci(pw, "%2F"), find_pct_ci(pw, "%3F")) {
                    (Some(a), Some(b)) => Some(a.min(b)),
                    (Some(a), None) | (None, Some(a)) => Some(a),
                    (None, None) => None,
                };
                let port_opt = first_pct
                    .and_then(|pos| pw.get(..pos))
                    .filter(|s| !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit()))
                    .and_then(|s| s.parse::<u16>().ok())
                    .filter(|&p| p > 0);
                if let Some(port) = port_opt {
                    let local_or_dev_ok = !has_pct3f
                        && ((dec_host.eq_ignore_ascii_case("localhost")
                            && (((u.scheme() == "http" || is_proto_rel)
                                && is_standard_web_port("http", port))
                                || is_standard_web_port(u.scheme(), port)))
                            || (dec_host.contains('_')
                                && u.scheme() == "http"
                                && is_standard_web_port("http", port))
                            || (matches!(u_dec.host(), Some(url::Host::Ipv4(_) | url::Host::Ipv6(_)))
                                && is_standard_web_port(u.scheme(), port)));
                    local_or_dev_ok
                        || (dec_host.contains('.') && is_standard_web_port(u.scheme(), port))
                } else {
                    false
                }
            } else {
                dec_host.contains('.')
            };

            if host_and_port_ok {
                if u_dec.query().is_none() && u_dec.fragment().is_none() {
                    if !cand.ends_with('/')
                        && !u_dec.path().ends_with('/')
                        && !matches!(u.host(), Some(url::Host::Ipv4(_) | url::Host::Ipv6(_)))
                        && let Some((path_before_at, path_after_at)) = u_dec.path().split_once('@')
                    {
                        if path_before_at.ends_with('/') {
                            let slash_pos = path_after_at.find('/');
                            let sep_pos = earliest_query_prose_sep(path_after_at);
                            let handle_end = match (slash_pos, sep_pos.map(|x| x.0)) {
                                (Some(a), Some(b)) => a.min(b),
                                (Some(a), None) | (None, Some(a)) => a,
                                (None, None) => path_after_at.len(),
                            };
                            let handle = path_after_at.get(..handle_end).unwrap_or(path_after_at);
                            if !handle.is_empty()
                                && !handle.contains('.')
                                && !handle.eq_ignore_ascii_case("localhost")
                            {
                                if let Some(sp) = slash_pos {
                                    let tail = path_after_at
                                        .get(sp.saturating_add(1)..)
                                        .unwrap_or("");
                                    let no_extra_at = !path_after_at
                                        .get(..sp)
                                        .is_some_and(|pre| pre.contains('@'))
                                        && !tail.contains('@');
                                    let slash_before_sep =
                                        sep_pos.is_none_or(|(sep, _, _)| sp < sep);
                                    if u.password().is_none()
                                        && no_extra_at
                                        && slash_before_sep
                                        && path_before_at.len() > 1
                                        && !tail.is_empty()
                                        && !tail.contains('/')
                                    {
                                        return true;
                                    }
                                } else if let Some(last_at) = path_after_at.rfind('@') {
                                    // Multi-@ prose/CSV pair after `%2F@handle` (e.g. `@alice+bob@example.com`).
                                    if let Some((sep, sep_len, _)) = sep_pos
                                        && sep < last_at
                                        && let Some(between_sep_at) =
                                            path_after_at.get(sep.saturating_add(sep_len)..last_at)
                                        && !between_sep_at.contains('@')
                                        && is_clean_email_local(between_sep_at)
                                    {
                                        return true;
                                    }
                                } else {
                                    return true;
                                }
                            }
                        } else if u.password().is_none()
                            && !path_after_at.contains('/')
                            && !path_after_at.contains('@')
                            && path_after_at.contains('.')
                            && hosts_same_or_subdomain(dec_host, path_after_at)
                        {
                            return true;
                        }
                    }
                } else if u_dec.query().is_some()
                    && u_dec.fragment().is_none()
                    && has_pct3f
                    && !matches!(u.host(), Some(url::Host::Ipv4(_) | url::Host::Ipv6(_)))
                    && u_dec.query().is_some_and(|q| {
                        let first_at = q.find('@');
                        let last_at = q.rfind('@');
                        first_at == last_at
                            && q.find('=')
                                .is_some_and(|eq| last_at.is_none_or(|at| eq < at))
                            && last_at.is_none_or(|at| {
                                !q.get(..at).is_some_and(|before| before.contains(':'))
                            })
                    })
                {
                    let clean_no_raw_path = u.path() == "/" && !cand.ends_with('/');
                    let clean_eq_handle = u.password().is_none()
                        && before_at.ends_with('=')
                        && u_dec.query().is_some_and(|q| {
                            q.rfind('@')
                                .and_then(|at| q.get(..at))
                                .is_some_and(|pre| {
                                    let last_param = pre
                                        .rsplit_once('&')
                                        .map_or(pre, |(_, p)| p);
                                    last_param.strip_suffix('=').is_some_and(|k| {
                                        !k.is_empty() && !k.contains('=')
                                    })
                                })
                        })
                        && u.path().get(1..).is_some_and(|p| !p.contains('/'))
                        && u.host_str()
                            .is_some_and(|h| !h.contains('.') && !h.eq_ignore_ascii_case("localhost"));
                    if clean_no_raw_path || clean_eq_handle {
                        return true;
                    }
                }
            }
        }
    }

    if is_proto_rel || !matches!(u.host(), Some(url::Host::Domain(_))) {
        return false;
    }

    // 2. Decoded sibling `&key=val` (or raw `%26key=val` without path) adjacent to clean host / host:port.
    let has_path = u.path() != "/" || cand.ends_with('/');
    let single_seg_path = !cand.ends_with('/') && u.path().get(1..).is_some_and(|p| !p.contains('/'));
    if u.password().is_none() {
        let amp_split = u.username().split_once('&').map(|(h, r)| (h, r, false)).or_else(|| {
            if has_path {
                return None;
            }
            let pos = find_pct_ci(u.username(), "%26")?;
            Some((
                u.username().get(..pos)?,
                u.username().get(pos.saturating_add(3)..)?,
                true,
            ))
        });
        if let Some((host_part, rest, is_pct_amp)) = amp_split
            && ((host_part.contains('.') && is_valid_hostname_str(host_part))
                || host_part.eq_ignore_ascii_case("localhost"))
            && (!is_pct_amp
                || (!rest.contains(':')
                    && before_at.contains('=')
                    && find_pct_ci(before_at, "%3D").is_none()))
            && amp_segments_are_benign(rest, has_path)
        {
            let scoped_handle_path = rest.ends_with('=')
                && single_seg_path
                && u.host_str()
                    .is_some_and(|h| !h.contains('.') && !h.eq_ignore_ascii_case("localhost"));
            if !has_path || scoped_handle_path {
                return true;
            }
        }
    } else if let Some(pw) = u.password()
        && let Some((port_s, rest)) = pw.split_once('&')
        && !port_s.is_empty()
        && port_s.bytes().all(|b| b.is_ascii_digit())
        && let Ok(port) = port_s.parse::<u16>()
        && port > 0
        && amp_segments_are_benign(rest, has_path)
    {
        let user = u.username();
        let benign_contact_key = last_amp_param_key_is_benign_contact(rest);
        let scoped_handle_path = rest.ends_with('=')
            && single_seg_path
            && u.host_str()
                .is_some_and(|h| !h.contains('.') && !h.eq_ignore_ascii_case("localhost"));
        let host_ok = (user.eq_ignore_ascii_case("localhost")
            && ((u.scheme() == "http" && is_standard_web_port("http", port))
                || is_standard_web_port(u.scheme(), port))
            && (!has_path
                || (u.scheme() == "http"
                    && is_standard_web_port("http", port)
                    && single_seg_path
                    && (benign_contact_key || scoped_handle_path))))
            || (user.contains('.')
                && is_valid_hostname_str(user)
                && is_standard_web_port(u.scheme(), port)
                && (!has_path || scoped_handle_path))
            || (u.scheme() == "http"
                && is_standard_web_port("http", port)
                && is_valid_hostname_str(user)
                && benign_contact_key
                && !has_path
                && u.query().is_none()
                && u.fragment().is_none());
        if host_ok {
            return true;
        }
    }

    // 3. Query search / prose / CSV separators ('+', ',', '%20' — never '%2B' or '%2C').
    if find_pct_ci(before_at, "%2B").is_some()
        || find_pct_ci(before_at, "%2C").is_some()
        || u.query().is_some()
        || u.fragment().is_some()
        || cand.ends_with('/')
    {
        return false;
    }

    // 3a. Bracketed IPv6 host:port + prose separator (e.g. `http://[::1]:3000+alice@example.com`).
    if u.path() == "/"
        && let Some(raw_ui) = raw_userinfo_prefix(cand)
        && raw_ui.starts_with('[')
        && let Some(close_br) = raw_ui.find(']')
        && let Some(ipv6_lit) = raw_ui.get(..close_br.saturating_add(1))
        && is_valid_rfc_host_port(ipv6_lit)
        && let Some(after_br) = raw_ui.get(close_br.saturating_add(1)..)
    {
        let after_sep_opt = if let Some(after_colon) = after_br.strip_prefix(':') {
            earliest_query_prose_sep(after_colon).and_then(|(sp, sp_len, _)| {
                let ps = after_colon.get(..sp)?;
                if ps.is_empty() || !ps.bytes().all(|b| b.is_ascii_digit()) {
                    return None;
                }
                let p = ps.parse::<u16>().ok()?;
                if p > 0 && is_standard_web_port(u.scheme(), p) {
                    after_colon.get(sp.saturating_add(sp_len)..)
                } else {
                    None
                }
            })
        } else {
            earliest_query_prose_sep(after_br)
                .filter(|&(sp, _, _)| sp == 0)
                .and_then(|(_, sp_len, _)| after_br.get(sp_len..))
        };
        if let Some(after_sep) = after_sep_opt
            && is_clean_prose_after_sep(after_sep)
        {
            return true;
        }
    }

    let user = u.username();
    if let Some((sep_pos, sep_len, sep_is_pct20)) = earliest_query_prose_sep(user) {
        let first_host = user.get(..sep_pos).unwrap_or("");
        let after_sep = user.get(sep_pos.saturating_add(sep_len)..).unwrap_or("");
        let is_dotted_host = first_host.contains('.') && is_valid_hostname_str(first_host);
        let is_named_host = is_dotted_host
            || first_host.eq_ignore_ascii_case("localhost")
            || (first_host.contains('_') && is_valid_hostname_str(first_host));

        if let Some(pw) = u.password() {
            if is_named_host
                && let Some((qual, has_prefix_words)) = match_trailing_search_qualifier(after_sep)
            {
                let pw_clean = if starts_with_pct_ci(pw, "%20") {
                    pw.get(3..).unwrap_or("")
                } else {
                    pw.strip_prefix('+').unwrap_or(pw)
                };
                let pw_ok = is_clean_email_local(pw_clean);
                let path_ok = u.path() == "/"
                    || (!has_prefix_words
                        && (qual.eq_ignore_ascii_case("author")
                            || (pw_clean.is_empty()
                                && u.host_str().is_some_and(|h| {
                                    !h.contains('.') && !h.eq_ignore_ascii_case("localhost")
                                }))));
                if pw_ok && path_ok {
                    return true;
                }
            }
        } else {
            let is_handle_target = is_named_host
                && (user.ends_with('+')
                    || user.ends_with(',')
                    || ends_with_pct_ci(user, "%20")
                    || (before_at.ends_with(':') && is_benign_search_qualifier(after_sep)))
                && u.host_str()
                    .is_some_and(|h| !h.contains('.') && !h.eq_ignore_ascii_case("localhost"));
            let is_dotted_prose_email = is_dotted_host
                && is_clean_prose_after_sep(after_sep)
                && (u.path() == "/"
                    || (in_prose_after_prefix
                        && sep_is_pct20
                        && u.host_str().is_some_and(|h| h.contains('.'))
                        && u.path().get(1..).is_some_and(|p| !p.contains('/'))
                        && is_clean_email_local(after_sep))
                    || (!before_at.as_bytes().contains(&b'%')
                        && before_at.as_bytes().contains(&b' ')
                        && u.host_str().is_some_and(|h| h.contains('.'))
                        && u.path().get(1..).is_some_and(|p| !p.contains('/'))
                        && is_clean_email_local(after_sep))
                    || u.host_str()
                        .is_some_and(|h| hosts_same_or_subdomain(first_host, h)));
            if is_handle_target || is_dotted_prose_email {
                return true;
            }
        }
    } else if let Some(pw) = u.password()
        && is_valid_hostname_str(user)
        && let Some((sep_pos, sep_len, sep_is_pct20)) = earliest_query_prose_sep(pw)
    {
        let port_s = pw.get(..sep_pos).unwrap_or("");
        let after_sep = pw.get(sep_pos.saturating_add(sep_len)..).unwrap_or("");
        if port_s.is_empty() {
            if sep_is_pct20
                && user.contains('.')
                && u.path() == "/"
                && !after_sep.is_empty()
                && is_clean_email_local(after_sep)
            {
                return true;
            }
        } else if port_s.bytes().all(|b| b.is_ascii_digit())
            && let Ok(port) = port_s.parse::<u16>()
            && port > 0
        {
            let is_dev_or_loopback = user.eq_ignore_ascii_case("localhost") || user.contains('_');
            let is_dotted_std_port = user.contains('.') && is_standard_web_port(u.scheme(), port);
            if (is_dev_or_loopback && u.scheme() == "http" && is_standard_web_port("http", port))
                || is_dotted_std_port
            {
                if u.path() == "/" && is_clean_prose_after_sep(after_sep) {
                    return true;
                }
                let at_handle_path = (after_sep.is_empty()
                    || after_sep.ends_with('+')
                    || after_sep.ends_with(',')
                    || ends_with_pct_ci(after_sep, "%20"))
                    && u.host_str()
                        .is_some_and(|h| !h.contains('.') && !h.eq_ignore_ascii_case("localhost"));
                let qualifier_path = after_sep
                    .split_once(':')
                    .or_else(|| {
                        let pos = find_pct_ci(after_sep, "%3A")?;
                        Some((after_sep.get(..pos)?, after_sep.get(pos.saturating_add(3)..)?))
                    })
                    .is_some_and(|(q, v)| is_benign_search_qualifier(q) && is_clean_email_local(v));
                let clean_profile_path = ((user.eq_ignore_ascii_case("localhost")
                    && u.scheme() == "http"
                    && is_standard_web_port("http", port))
                    || is_dotted_std_port)
                    && u.host_str().is_some_and(|h| h.contains('.'))
                    && u.path().get(1..).is_some_and(|p| !p.contains('/'))
                    && !after_sep.is_empty()
                    && is_clean_email_local(after_sep);
                if at_handle_path || qualifier_path || clean_profile_path {
                    return true;
                }
            }
        }
    }

    false
}

enum UrlUserinfoOutcome {
    NoUrl,
    Blocked,
    Clean(url::Url),
}

fn check_parsed_url_userinfo_only(
    raw_cand: &str,
    base: &url::Url,
    from_query_or_frag: bool,
    in_prose_after_prefix: bool,
) -> UrlUserinfoOutcome {
    if !raw_cand.as_bytes().contains(&b'@') {
        return UrlUserinfoOutcome::NoUrl;
    }
    let trimmed = raw_cand.trim();
    let cand = strip_outer_wrappers(trimmed.trim_start_matches('+'));
    let Some(first_at) = cand.find('@') else {
        return UrlUserinfoOutcome::NoUrl;
    };
    if first_at.saturating_add(1) >= cand.len() {
        return UrlUserinfoOutcome::NoUrl;
    }
    let Some((u, is_proto_rel)) = parse_candidate_with_pct_host_fallback(cand, base) else {
        return UrlUserinfoOutcome::NoUrl;
    };
    if from_query_or_frag && !is_proto_rel && is_plus_joined_known_scheme(u.scheme()) {
        return UrlUserinfoOutcome::NoUrl;
    }
    if (!u.username().is_empty() || u.password().is_some()) && is_valid_target_host(&u) {
        return if is_benign_embedded_userinfo(
            cand,
            &u,
            base,
            is_proto_rel,
            from_query_or_frag,
            in_prose_after_prefix,
        ) {
            UrlUserinfoOutcome::Clean(u)
        } else {
            UrlUserinfoOutcome::Blocked
        };
    }
    UrlUserinfoOutcome::Clean(u)
}

fn extract_raw_url_path(raw_cand: &str) -> Option<&str> {
    let cand = strip_outer_wrappers(raw_cand.trim().trim_start_matches('+'));
    let bytes = cand.as_bytes();
    let auth_start = if let Some(after) = strip_proto_rel_prefix(cand) {
        cand.len().saturating_sub(after.len())
    } else if has_url_scheme_prefix(cand)
        && let Some(colon) = cand.find(':')
    {
        let mut p = colon.saturating_add(1);
        while matches!(bytes.get(p), Some(b'/' | b'\\')) {
            p = p.saturating_add(1);
        }
        p
    } else {
        return None;
    };
    let rest = cand.get(auth_start..)?;
    let rel_slash = rest.find(['/', '\\', '?', '#'])?;
    if !matches!(rest.as_bytes().get(rel_slash), Some(b'/' | b'\\')) {
        return None;
    }
    let path_start = auth_start.saturating_add(rel_slash);
    let after_path_start = cand.get(path_start..)?;
    let path_len = after_path_start
        .find(['?', '#'])
        .unwrap_or(after_path_start.len());
    after_path_start.get(..path_len)
}

fn unescape_json_structural_pct(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes.get(i) == Some(&b'%')
            && let Some(dec) = decode_pct_at(bytes, i)
        {
            if matches!(dec, b'{' | b'}' | b'[' | b']' | b'"' | b':' | b',') {
                out.push(dec);
                i = i.saturating_add(3);
                continue;
            }
            // Unescape `%5C` before `uXXXX` (4 hex digits) so JSON `\u0040` escapes parse in pass 0.
            if dec == b'\\'
                && matches!(bytes.get(i.saturating_add(3)), Some(b'u' | b'U'))
                && bytes
                    .get(i.saturating_add(4)..i.saturating_add(8))
                    .is_some_and(|hex4| hex4.len() == 4 && hex4.iter().all(u8::is_ascii_hexdigit))
            {
                out.push(b'\\');
                i = i.saturating_add(3);
                continue;
            }
        }
        if let Some(&b) = bytes.get(i) {
            out.push(b);
        }
        i = i.saturating_add(1);
    }
    String::from_utf8_lossy(&out).into_owned()
}

fn matching_closer_byte(open: u8) -> Option<u8> {
    match open {
        b'(' => Some(b')'),
        b'[' => Some(b']'),
        b'{' => Some(b'}'),
        b'\'' => Some(b'\''),
        b'"' => Some(b'"'),
        b'<' => Some(b'>'),
        _ => None,
    }
}

fn is_clean_host_for_wrapper_close(hp: &str) -> bool {
    if hp.is_empty() || hp.len() > 260 {
        return false;
    }
    if hp.starts_with('[') {
        return is_valid_rfc_host_port(hp);
    }
    if let Some((h, _)) = hp.split_once(':') {
        return is_valid_rfc_host_port(hp)
            && (h.contains('.') || h.eq_ignore_ascii_case("localhost") || h.contains('_'));
    }
    is_valid_rfc_host_port(hp)
}

fn is_clean_host_for_space_bridge(hp: &str) -> bool {
    if hp.is_empty() || hp.len() > 260 {
        return false;
    }
    if hp.starts_with('[') {
        return is_valid_rfc_host_port(hp);
    }
    if let Some((h, p)) = hp.split_once(':') {
        if p.is_empty() {
            return h.contains('.') && is_valid_hostname_str(h);
        }
        return is_valid_rfc_host_port(hp)
            && (h.contains('.') || h.eq_ignore_ascii_case("localhost") || h.contains('_'));
    }
    is_valid_rfc_host_port(hp)
        && (hp.contains('.') || hp.eq_ignore_ascii_case("localhost") || hp.contains('_'))
}

struct AuthColonState {
    auth_start: usize,
    starts_bracket: bool,
    first_colon_pos: Option<usize>,
    host_dotted_valid: bool,
    host_port_base_valid: bool,
    port_digits: usize,
    port_all_digits: bool,
    port_val: u32,
}

impl AuthColonState {
    fn new(s: &str, auth_start: usize) -> Self {
        Self {
            auth_start,
            starts_bracket: s.as_bytes().get(auth_start) == Some(&b'['),
            first_colon_pos: None,
            host_dotted_valid: false,
            host_port_base_valid: false,
            port_digits: 0,
            port_all_digits: true,
            port_val: 0,
        }
    }

    fn push_byte(&mut self, s: &str, pos: usize, b: u8) {
        if self.starts_bracket {
            return;
        }
        if self.first_colon_pos.is_none() {
            if b == b':' {
                self.first_colon_pos = Some(pos);
                let host_len = pos.saturating_sub(self.auth_start);
                if host_len <= 260
                    && let Some(h) = s.get(self.auth_start..pos)
                {
                    self.host_dotted_valid = h.contains('.') && is_valid_hostname_str(h);
                    self.host_port_base_valid = is_valid_rfc_host_port(h)
                        && (h.contains('.')
                            || h.eq_ignore_ascii_case("localhost")
                            || h.contains('_'));
                }
            }
        } else if self.port_all_digits {
            if b.is_ascii_digit() {
                self.port_digits = self.port_digits.saturating_add(1);
                let d = u32::from(b.saturating_sub(b'0'));
                self.port_val = self.port_val.saturating_mul(10).saturating_add(d).min(70_000);
            } else {
                self.port_all_digits = false;
            }
        }
    }

    fn is_non_port_cred(&self, pos: usize) -> bool {
        if self.starts_bracket || self.first_colon_pos.is_none() {
            return false;
        }
        if pos.saturating_sub(self.auth_start) > 260 {
            return true;
        }
        let clean_space_bridge = if self.port_digits == 0 && self.port_all_digits {
            self.host_dotted_valid
        } else {
            self.host_port_base_valid
                && self.port_all_digits
                && self.port_digits > 0
                && self.port_val >= 1
                && self.port_val <= u32::from(u16::MAX)
        };
        !clean_space_bridge
    }
}

#[derive(Default)]
struct PeekCache {
    has_scan: bool,
    scan_start: usize,
    scan_end: usize,
    ended_at_at: bool,
    ended_at_hard_delim: bool,
    has_colon_scan: bool,
    colon_scan_start: usize,
    colon_scan_end: usize,
    found_colon: Option<usize>,
}

impl PeekCache {
    fn peek_at_before_auth_delim(
        &mut self,
        bytes: &[u8],
        start: usize,
    ) -> (Option<usize>, Option<usize>) {
        if self.has_scan && start >= self.scan_start && start <= self.scan_end {
            if self.ended_at_at {
                return (Some(self.scan_end), None);
            }
            return (
                None,
                self.ended_at_hard_delim.then_some(self.scan_end),
            );
        }
        let mut p = start;
        while let Some(&b) = bytes.get(p) {
            if b == b'@' {
                self.has_scan = true;
                self.scan_start = start;
                self.scan_end = p;
                self.ended_at_at = true;
                self.ended_at_hard_delim = false;
                return (Some(p), None);
            }
            if matches!(b, b'/' | b'\\' | b'?' | b'#' | b'\r' | b'\n') {
                self.has_scan = true;
                self.scan_start = start;
                self.scan_end = p;
                self.ended_at_at = false;
                self.ended_at_hard_delim = true;
                return (None, Some(p));
            }
            if matches!(b, b'"' | b'<' | b'>') {
                self.has_scan = true;
                self.scan_start = start;
                self.scan_end = p;
                self.ended_at_at = false;
                self.ended_at_hard_delim = false;
                return (None, None);
            }
            p = p.saturating_add(1);
        }
        self.has_scan = true;
        self.scan_start = start;
        self.scan_end = p;
        self.ended_at_at = false;
        self.ended_at_hard_delim = true;
        (None, Some(p))
    }

    fn find_colon(&mut self, bytes: &[u8], start: usize, end: usize) -> Option<usize> {
        if self.has_colon_scan && start >= self.colon_scan_start && end == self.colon_scan_end {
            match self.found_colon {
                None => return None,
                Some(c) if c >= start => return Some(c),
                Some(_) => {}
            }
        }
        let found = bytes
            .get(start..end)
            .and_then(|slice| slice.iter().position(|&b| b == b':'))
            .map(|rel| start.saturating_add(rel));
        self.has_colon_scan = true;
        self.colon_scan_start = start;
        self.colon_scan_end = end;
        self.found_colon = found;
        found
    }
}

fn qualifier_before_colon_is_benign(tok: &str, after_ws: usize, colon_pos: usize) -> bool {
    let bytes = tok.as_bytes();
    let cap = colon_pos.saturating_sub(10).max(after_ws);
    let mut run_start = colon_pos;
    while run_start > cap {
        let prev = run_start.saturating_sub(1);
        if bytes.get(prev).is_some_and(u8::is_ascii_alphabetic) {
            run_start = prev;
        } else {
            break;
        }
    }
    if run_start == colon_pos {
        return false;
    }
    if run_start > after_ws
        && !tok
            .get(..run_start)
            .and_then(|pre| pre.chars().next_back())
            .is_some_and(char::is_whitespace)
    {
        return false;
    }
    tok.get(run_start..colon_pos)
        .is_some_and(is_benign_search_qualifier)
}

fn authority_or_wrapped_span_end(
    tok: &str,
    start: usize,
    auth_start: usize,
    opener_b: Option<(u8, u8)>,
    is_http_like: bool,
    peek_cache: &mut PeekCache,
) -> (usize, bool, Option<usize>) {
    let Some(rest) = tok.get(auth_start..) else {
        return (tok.len(), false, None);
    };
    let bytes = tok.as_bytes();
    let mut depth: usize = 1;
    let mut seen_at = false;
    let mut bridge_space_until: usize = 0;
    let mut colon_state = AuthColonState::new(tok, auth_start);
    for (rel, ch) in rest.char_indices() {
        let pos = auth_start.saturating_add(rel);
        if ch == '@' {
            seen_at = true;
        }
        if let Some((open_b, close_b)) = opener_b {
            if ch != '@' && open_b != close_b && ch == char::from(open_b) {
                depth = depth.saturating_add(1);
            } else if ch != '@' && ch == char::from(close_b) {
                depth = depth.saturating_sub(1);
                if depth == 0 {
                    let next_b = bytes.get(pos.saturating_add(1)).copied();
                    let followed_by_boundary = next_b.is_none()
                        || matches!(
                            next_b,
                            Some(
                                b'+' | b','
                                    | b';'
                                    | b'&'
                                    | b'.'
                                    | b' '
                                    | b'\t'
                                    | b'\r'
                                    | b'\n'
                                    | b')'
                                    | b']'
                                    | b'}'
                                    | b'>'
                                    | b'"'
                                    | b'\''
                            )
                        )
                        || (next_b == Some(b'%')
                            && decode_pct_at(bytes, pos.saturating_add(1)).is_some_and(|d| {
                                d.is_ascii_whitespace()
                                    || matches!(d, b'+' | b',' | b';' | b'&' | b')' | b']' | b'}')
                            }));
                    let pre_is_clean_host = tok
                        .get(auth_start..pos)
                        .is_some_and(is_clean_host_for_wrapper_close);
                    if seen_at || (followed_by_boundary && pre_is_clean_host) {
                        return (pos.max(start), true, None);
                    }
                    depth = 1;
                }
            }
        }
        if matches!(ch, '/' | '\\' | '?' | '#') {
            return (
                if ch == '/' {
                    pos.saturating_add(1)
                } else {
                    pos
                },
                false,
                None,
            );
        }
        let push_b = if ch.is_ascii() { ch as u8 } else { 0x80 };
        if matches!(ch, '"' | '<' | '>') {
            if !seen_at && colon_state.is_non_port_cred(pos) {
                colon_state.push_byte(tok, pos, push_b);
                continue;
            }
            return (pos, false, None);
        }
        if ch.is_whitespace() && !matches!(ch, '\r' | '\n' | '\t') && !seen_at {
            if pos < bridge_space_until {
                colon_state.push_byte(tok, pos, push_b);
                continue;
            }
            let after_ws = pos.saturating_add(ch.len_utf8());
            let (at_opt, hard_stop) = peek_cache.peek_at_before_auth_delim(bytes, after_ws);
            if let Some(at_p) = at_opt {
                let before_sp = tok.get(auth_start..pos).unwrap_or("");
                let should_bridge = if !is_http_like
                    || colon_state.is_non_port_cred(pos)
                    || !is_clean_host_for_space_bridge(before_sp)
                {
                    true
                } else if let Some(c_pos) = peek_cache.find_colon(bytes, after_ws, at_p) {
                    !qualifier_before_colon_is_benign(tok, after_ws, c_pos)
                } else {
                    false
                };
                if should_bridge {
                    bridge_space_until = at_p;
                    colon_state.push_byte(tok, pos, push_b);
                    continue;
                }
            }
            return (pos, false, hard_stop);
        }
        if ch.is_whitespace() {
            return (pos, false, None);
        }
        colon_state.push_byte(tok, pos, push_b);
    }
    (tok.len(), false, None)
}

struct SchemeAnchor<'a> {
    start: usize,
    scheme: &'a str,
    opener: Option<(u8, u8)>,
}

/// Strictly O(1) (at most `MAX_URL_SCHEME_LEN` = 64 bytes backward) scheme anchor lookup
/// immediately before `colon_pos`. Accepts `scheme_start == 0`, ANY non-ASCII-alphanumeric
/// boundary, or percent-encoded non-alnum boundary (`%20`, `%28`, `%60`, `%7C`, `%C2%A0`,
/// `%25XX`), and strips known-scheme suffixes joined by `+`/`-`/`.`.
fn find_scheme_before_colon(
    tok: &str,
    colon_pos: usize,
    in_raw_pass: bool,
) -> Option<SchemeAnchor<'_>> {
    let bytes = tok.as_bytes();
    let cap = colon_pos.saturating_sub(MAX_URL_SCHEME_LEN);
    let mut raw_start = colon_pos;
    while raw_start > cap {
        let prev = raw_start.saturating_sub(1);
        let Some(&b) = bytes.get(prev) else {
            break;
        };
        if b.is_ascii_alphanumeric() || matches!(b, b'+' | b'-' | b'.') {
            raw_start = prev;
        } else {
            break;
        }
    }
    if raw_start == colon_pos {
        return None;
    }

    let mut scheme_start = raw_start;
    let mut opener_b: Option<(u8, u8)> = None;
    let mut matched_pct_delim = false;

    if in_raw_pass
        && raw_start > 0
        && bytes.get(raw_start.saturating_sub(1)) == Some(&b'%')
        && let Some(mut dec) = decode_pct_at(bytes, raw_start.saturating_sub(1))
    {
        let mut pct_end = raw_start.saturating_add(2);
        let mut layers = 0usize;
        while dec == b'%' && layers < PERCENT_DECODE_MAX_PASSES {
            let Some(hi) = bytes.get(pct_end).copied().and_then(hex_val) else {
                break;
            };
            let Some(lo) = bytes
                .get(pct_end.saturating_add(1))
                .copied()
                .and_then(hex_val)
            else {
                break;
            };
            dec = (hi << 4) | lo;
            pct_end = pct_end.saturating_add(2);
            layers = layers.saturating_add(1);
        }
        if pct_end < colon_pos && !dec.is_ascii_alphanumeric() {
            scheme_start = pct_end;
            opener_b = matching_closer_byte(dec).map(|c| (dec, c));
            matched_pct_delim = true;
        }
    }

    let before_alpha_trim = scheme_start;
    while scheme_start < colon_pos
        && !bytes.get(scheme_start).is_some_and(u8::is_ascii_alphabetic)
    {
        scheme_start = scheme_start.saturating_add(1);
    }
    if scheme_start >= colon_pos {
        return None;
    }
    if scheme_start > before_alpha_trim {
        matched_pct_delim = false;
        opener_b = None;
    }

    if let Some(cand) = tok.get(scheme_start..colon_pos) {
        let cb = cand.as_bytes();
        let mut j = cb.len();
        while j > 0 {
            j = j.saturating_sub(1);
            if matches!(cb.get(j), Some(b'+' | b'-' | b'.'))
                && let Some(tail) = cand.get(j.saturating_add(1)..)
                && is_known_url_scheme(tail)
            {
                scheme_start = scheme_start.saturating_add(j).saturating_add(1);
                matched_pct_delim = false;
                opener_b = None;
                break;
            }
        }
    }

    if scheme_start > 0 {
        let prev_prefix = tok.get(..scheme_start)?;
        if matched_pct_delim {
            if !prefix_ends_with_scheme_delim(prev_prefix, true) {
                return None;
            }
        } else {
            let prev_ch = prev_prefix.chars().next_back()?;
            if !prev_ch.is_ascii_alphanumeric() {
                if prev_ch.is_ascii() {
                    let b = prev_ch as u8;
                    opener_b = matching_closer_byte(b).map(|c| (b, c));
                }
            } else if in_raw_pass && prefix_ends_with_scheme_delim(prev_prefix, true) {
                if let Some(p_start) = scheme_start.checked_sub(3)
                    && let Some(dec) = decode_pct_at(bytes, p_start)
                {
                    opener_b = matching_closer_byte(dec).map(|c| (dec, c));
                }
            } else {
                return None;
            }
        }
    }

    let scheme = tok.get(scheme_start..colon_pos)?;
    Some(SchemeAnchor {
        start: scheme_start,
        scheme,
        opener: opener_b,
    })
}

/// Strictly O(N) `:`-anchored embedded scheme scanner (Level 1 — calls only Level 0).
fn scan_embedded_delim_schemes(
    tok: &str,
    base: &url::Url,
    from_query_or_frag: bool,
    in_raw_pass: bool,
) -> bool {
    if !tok.as_bytes().contains(&b'@') {
        return false;
    }
    let bytes = tok.as_bytes();
    let mut i = 0;
    let mut no_0slash_until: usize = 0;
    let mut peek_cache = PeekCache::default();
    while i < bytes.len() {
        let Some(rel) = bytes.get(i..).and_then(|r| r.iter().position(|&b| b == b':')) else {
            break;
        };
        let colon_pos = i.saturating_add(rel);
        i = colon_pos.saturating_add(1);

        let Some(SchemeAnchor {
            start: scheme_start,
            scheme,
            opener: opener_b,
        }) = find_scheme_before_colon(tok, colon_pos, in_raw_pass)
        else {
            continue;
        };

        let mut auth_start = colon_pos.saturating_add(1);
        while matches!(bytes.get(auth_start), Some(b'/' | b'\\')) {
            auth_start = auth_start.saturating_add(1);
        }
        let slash_count = auth_start.saturating_sub(colon_pos.saturating_add(1));
        let is_special = is_whatwg_special_scheme(scheme);
        if is_special {
            if slash_count == 0 {
                let Some(&first_after) = bytes.get(auth_start) else {
                    continue;
                };
                if first_after.is_ascii_whitespace()
                    || matches!(
                        first_after,
                        b'/' | b'\\'
                            | b'?'
                            | b'#'
                            | b'"'
                            | b'<'
                            | b'>'
                            | b')'
                            | b']'
                            | b'}'
                            | b','
                            | b';'
                            | b'&'
                    )
                {
                    continue;
                }
                if auth_start < no_0slash_until {
                    continue;
                }
            }
        } else if slash_count != 2 {
            continue;
        }

        let is_http_like =
            scheme.eq_ignore_ascii_case("http") || scheme.eq_ignore_ascii_case("https");
        let (span_end, closed_by_wrapper, hard_no_at_stop) = authority_or_wrapped_span_end(
            tok,
            scheme_start,
            auth_start,
            opener_b,
            is_http_like,
            &mut peek_cache,
        );

        let span_has_at = tok
            .get(auth_start..span_end)
            .is_some_and(|a| a.as_bytes().contains(&b'@'));

        if !span_has_at {
            if is_special && slash_count == 0 {
                no_0slash_until = no_0slash_until.max(span_end);
                if let Some(stop) = hard_no_at_stop {
                    no_0slash_until = no_0slash_until.max(stop);
                }
            }
            continue;
        }

        let mut check_end = span_end;
        if !closed_by_wrapper
            && span_end > auth_start
            && bytes.get(span_end.saturating_sub(1)) == Some(&b'/')
            && let Some(path_rest) = tok.get(span_end..)
        {
            let limit = path_rest.len().min(256);
            let mut seg_len = 0;
            for (p_rel, ch) in path_rest.char_indices() {
                if p_rel >= limit
                    || ch.is_whitespace()
                    || matches!(
                        ch,
                        '\\' | '?'
                            | '#'
                            | '"'
                            | '\''
                            | '<'
                            | '>'
                            | ')'
                            | ']'
                            | '}'
                            | ','
                            | ';'
                            | '&'
                            | '|'
                            | '`'
                    )
                    || (ch == '%'
                        && path_rest
                            .get(p_rel..p_rel.saturating_add(3))
                            .is_some_and(|p3| p3.eq_ignore_ascii_case("%20")))
                {
                    break;
                }
                seg_len = p_rel.saturating_add(ch.len_utf8());
            }
            check_end = span_end.saturating_add(seg_len);
        }

        let in_prose = from_query_or_frag
            && scheme_start > 0
            && tok.get(..scheme_start).is_some_and(|pre| {
                (ends_with_pct_ci(pre, "%20") && pre.len() > 3)
                    || (pre.ends_with([' ', '+']) && pre.len() > 1)
            });

        if let Some(check_span) = tok.get(scheme_start..check_end) {
            match check_parsed_url_userinfo_only(check_span, base, from_query_or_frag, in_prose) {
                UrlUserinfoOutcome::Blocked => return true,
                UrlUserinfoOutcome::NoUrl | UrlUserinfoOutcome::Clean(_) => {
                    if is_special && slash_count == 0 {
                        no_0slash_until = no_0slash_until.max(span_end);
                    }
                }
            }
        }
    }
    false
}

/// Strictly O(N) `//`-anchored protocol-relative scanner (Level 1 — calls only Level 0).
fn scan_proto_rel_userinfo(
    s: &str,
    base: &url::Url,
    in_raw_pass: bool,
    allow_start: bool,
    from_query_or_frag: bool,
) -> bool {
    if !s.as_bytes().contains(&b'@') {
        return false;
    }
    let bytes = s.as_bytes();
    let mut i = 0;
    let mut peek_cache = PeekCache::default();
    while i < bytes.len() {
        let Some(rel) = bytes
            .get(i..)
            .and_then(|r| r.iter().position(|&b| matches!(b, b'/' | b'\\')))
        else {
            break;
        };
        let slash_pos = i.saturating_add(rel);
        let mut run_end = slash_pos;
        while matches!(bytes.get(run_end), Some(b'/' | b'\\')) {
            run_end = run_end.saturating_add(1);
        }
        if run_end.saturating_sub(slash_pos) >= 2 {
            let prev_ok = if slash_pos == 0 {
                allow_start
            } else {
                s.get(..slash_pos)
                    .is_some_and(|p| prefix_ends_with_proto_rel_delim(p, in_raw_pass))
            };
            let path_alnum_seg = !prev_ok
                && !from_query_or_frag
                && slash_pos > 0
                && bytes
                    .get(slash_pos.saturating_sub(1))
                    .is_some_and(u8::is_ascii_alphanumeric)
                && !(in_raw_pass
                    && s.get(..slash_pos)
                        .is_some_and(|p| ends_with_pct_ci(p, "%3A")));
            if prev_ok || path_alnum_seg {
                let opener_b = if prev_ok {
                    slash_pos
                        .checked_sub(1)
                        .and_then(|p| bytes.get(p).copied())
                        .and_then(|b| matching_closer_byte(b).map(|c| (b, c)))
                } else {
                    None
                };
                let (span_end, _, _) = authority_or_wrapped_span_end(
                    s,
                    slash_pos,
                    run_end,
                    opener_b,
                    true,
                    &mut peek_cache,
                );
                if let Some(span) = s.get(run_end..span_end)
                    && span.as_bytes().contains(&b'@')
                {
                    let cand = format!("//{span}");
                    if prev_ok {
                        if matches!(
                            check_parsed_url_userinfo_only(&cand, base, from_query_or_frag, false),
                            UrlUserinfoOutcome::Blocked
                        ) {
                            return true;
                        }
                    } else if let Some((u, _)) = parse_candidate_with_pct_host_fallback(&cand, base)
                        && u.password().is_some()
                        && is_valid_target_host(&u)
                    {
                        return true;
                    }
                }
            }
        }
        i = run_end;
    }
    false
}

/// Checks single extra leading slash/backslash after the top-level `/` (e.g. `https://ex.com//user:pass@evil.com/`)
/// strictly requiring an explicit `:password` to avoid false positives on benign `//alice@example.com` paths.
fn check_leading_extra_slash_password(path: &str, base: &url::Url) -> bool {
    let bytes = path.as_bytes();
    if !matches!(bytes.first(), Some(b'/' | b'\\'))
        || matches!(bytes.get(1), Some(b'/' | b'\\'))
        || !bytes.contains(&b'@')
    {
        return false;
    }
    let mut peek_cache = PeekCache::default();
    let (span_end, _, _) = authority_or_wrapped_span_end(path, 0, 1, None, true, &mut peek_cache);
    let Some(span) = path.get(1..span_end) else {
        return false;
    };
    if !span.as_bytes().contains(&b'@') {
        return false;
    }
    let cand = format!("//{span}");
    parse_candidate_with_pct_host_fallback(&cand, base)
        .is_some_and(|(u, _)| u.password().is_some() && is_valid_target_host(&u))
}

fn scan_embedded_path_respecting_pct_q(
    path: &str,
    base: &url::Url,
    in_raw_pass: bool,
) -> bool {
    if !path.as_bytes().contains(&b'@') {
        return false;
    }
    let pct_q = match (find_pct_ci(path, "%3F"), find_pct_ci(path, "%23")) {
        (Some(a), Some(b)) => Some(a.min(b)),
        (Some(a), None) | (None, Some(a)) => Some(a),
        (None, None) => None,
    };
    if let Some(q_pos) = pct_q {
        let head = path.get(..q_pos).unwrap_or("");
        let tail = path.get(q_pos.saturating_add(3)..).unwrap_or("");
        return scan_embedded_delim_schemes(head, base, false, in_raw_pass)
            || scan_proto_rel_userinfo(head, base, in_raw_pass, true, false)
            || scan_embedded_delim_schemes(tail, base, true, in_raw_pass)
            || scan_proto_rel_userinfo(tail, base, in_raw_pass, true, true);
    }
    scan_embedded_delim_schemes(path, base, false, in_raw_pass)
        || scan_proto_rel_userinfo(path, base, in_raw_pass, true, false)
}

fn check_single_url_userinfo_opts(
    raw_cand: &str,
    base: &url::Url,
    from_query_or_frag: bool,
    in_prose_after_prefix: bool,
) -> bool {
    match check_parsed_url_userinfo_only(raw_cand, base, from_query_or_frag, in_prose_after_prefix)
    {
        UrlUserinfoOutcome::Blocked => true,
        UrlUserinfoOutcome::Clean(u) => {
            let upath_pct = u.path().as_bytes().contains(&b'%');
            if scan_embedded_path_respecting_pct_q(u.path(), base, upath_pct) {
                return true;
            }
            if let Some(raw_path) = extract_raw_url_path(raw_cand) {
                let raw_has_pct = raw_path.as_bytes().contains(&b'%');
                if scan_embedded_path_respecting_pct_q(raw_path, base, raw_has_pct) {
                    return true;
                }
            }
            false
        }
        UrlUserinfoOutcome::NoUrl => false,
    }
}

fn check_single_url_userinfo(raw_cand: &str, base: &url::Url, from_query_or_frag: bool) -> bool {
    check_single_url_userinfo_opts(raw_cand, base, from_query_or_frag, false)
}

fn check_token_for_embedded_urls(
    tok: &str,
    base: &url::Url,
    from_query_or_frag: bool,
    in_raw_pass: bool,
) -> bool {
    if !tok.as_bytes().contains(&b'@') {
        return false;
    }
    if starts_like_url_or_wrapper(tok) && check_single_url_userinfo(tok, base, from_query_or_frag) {
        return true;
    }
    scan_embedded_delim_schemes(tok, base, from_query_or_frag, in_raw_pass)
        || scan_proto_rel_userinfo(tok, base, in_raw_pass, true, from_query_or_frag)
}

fn check_json_leaf_text(leaf: &str, base: &url::Url) -> bool {
    if leaf.chars().any(char::is_control) || contains_secret_token_or_webhook_key(leaf) {
        return true;
    }
    let has_pct = leaf.as_bytes().contains(&b'%');
    if let Some((_, after_semi)) = leaf.split_once(';')
        && slice_has_secret_param_key(after_semi, !has_pct)
    {
        return true;
    }
    if has_backslash_u_escape(leaf)
        && let Some(unescaped) = unescape_json_u_escapes(leaf)
    {
        if unescaped.chars().any(char::is_control)
            || contains_secret_token_or_webhook_key(&unescaped)
        {
            return true;
        }
        if let Some((_, after_semi)) = unescaped.split_once(';')
            && slice_has_secret_param_key(after_semi, false)
        {
            return true;
        }
        if let Some(dpos) = unescaped.find(['?', '#'])
            && let Some(utail) = unescaped.get(dpos.saturating_add(1)..)
            && slice_has_secret_param_key(utail, false)
        {
            return true;
        }
        if unescaped.as_bytes().contains(&b'@')
            && (scan_embedded_delim_schemes(&unescaped, base, true, false)
                || scan_proto_rel_userinfo(&unescaped, base, false, true, true))
        {
            return true;
        }
    }

    let head = if let Some(delim_pos) = leaf.find(['?', '#']) {
        if let Some(tail) = leaf.get(delim_pos.saturating_add(1)..)
            && !tail.is_empty()
        {
            if slice_has_secret_param_key(tail, !has_pct) {
                return true;
            }
            if tail.as_bytes().contains(&b'@') {
                let is_frag = leaf.as_bytes().get(delim_pos) == Some(&b'#');
                for chunk in split_outer_query_chunks(tail, is_frag) {
                    if !chunk.as_bytes().contains(&b'@') {
                        continue;
                    }
                    let val = match chunk.split_once('=') {
                        Some((lhs, rhs)) if is_plain_param_key(lhs) => rhs,
                        _ => chunk,
                    };
                    if check_token_for_embedded_urls(val, base, true, has_pct) {
                        return true;
                    }
                }
            }
        }
        leaf.get(..delim_pos).unwrap_or("")
    } else {
        leaf
    };

    if !head.as_bytes().contains(&b'@') {
        return false;
    }
    let head_has_encoded_q =
        has_pct && (find_pct_ci(head, "%3F").is_some() || find_pct_ci(head, "%23").is_some());
    if check_single_url_userinfo(head, base, head_has_encoded_q)
        || scan_embedded_delim_schemes(head, base, head_has_encoded_q, has_pct)
        || scan_proto_rel_userinfo(head, base, has_pct, true, head_has_encoded_q)
    {
        return true;
    }
    if head.as_bytes().contains(&b',') {
        for piece in head.split(',') {
            if piece.as_bytes().contains(&b'@')
                && check_single_url_userinfo(piece, base, head_has_encoded_q)
            {
                return true;
            }
        }
    }
    false
}

struct NestedJsonBudget {
    collected: Vec<serde_json::Value>,
    seen: HashSet<String>,
    count: usize,
    max: usize,
}

impl NestedJsonBudget {
    fn new(max: usize) -> Self {
        Self {
            collected: Vec::new(),
            seen: HashSet::new(),
            count: 0,
            max,
        }
    }

    fn remaining(&self) -> bool {
        self.count < self.max
    }

    fn try_push_parsed(&mut self, raw: &str) {
        if !self.remaining() || self.seen.contains(raw) {
            return;
        }
        if let Ok(jv) = serde_json::from_str::<serde_json::Value>(raw) {
            self.seen.insert(raw.to_string());
            self.count = self.count.saturating_add(1);
            self.collected.push(jv);
        }
    }
}

fn try_extract_nested_json_from_leaf(leaf: &str, budget: &mut NestedJsonBudget) {
    if !budget.remaining() {
        return;
    }
    let trimmed = leaf.trim();
    if matches!(trimmed.as_bytes().first(), Some(b'{' | b'[' | b'"')) {
        budget.try_push_parsed(trimmed);
        if !budget.remaining() {
            return;
        }
    }
    if let Some(delim_pos) = leaf.find(['?', '#'])
        && let Some(tail) = leaf.get(delim_pos.saturating_add(1)..)
    {
        let is_frag = leaf.as_bytes().get(delim_pos) == Some(&b'#');
        for chunk in split_outer_query_chunks(tail, is_frag) {
            if !budget.remaining() {
                break;
            }
            let val = match chunk.split_once('=') {
                Some((lhs, rhs)) if is_plain_param_key(lhs) => rhs.trim(),
                _ => chunk.trim(),
            };
            if matches!(val.as_bytes().first(), Some(b'{' | b'[' | b'"')) {
                budget.try_push_parsed(val);
            } else if starts_with_pct_ci(val, "%7B")
                || starts_with_pct_ci(val, "%5B")
                || starts_with_pct_ci(val, "%22")
            {
                let unesc = unescape_json_structural_pct(val);
                budget.try_push_parsed(&unesc);
            }
        }
    }
}

fn check_json_string_with_decodes(
    leaf: &str,
    base: &url::Url,
    collect_nested: bool,
    budget: &mut NestedJsonBudget,
) -> bool {
    if check_json_leaf_text(leaf, base) {
        return true;
    }
    if collect_nested && budget.remaining() {
        try_extract_nested_json_from_leaf(leaf, budget);
    }
    if leaf.as_bytes().contains(&b'%') {
        let mut cur = leaf.to_string();
        for pass in 0..PERCENT_DECODE_MAX_PASSES {
            if !cur.as_bytes().contains(&b'%') {
                break;
            }
            if should_run_keep_pct20(&cur) {
                let keep20 = percent_decode_once_preserve_pct20(&cur);
                if check_json_leaf_text(&keep20, base) {
                    return true;
                }
            }
            let prev_had_non_pct25 = has_non_pct25_escape(&cur);
            let prev_had_struct = collect_nested
                && budget.remaining()
                && has_pct_json_or_url_structural_escape(&cur);
            let next = percent_decode_once(&cur, false);
            if next == cur {
                break;
            }
            cur = next;
            let cur_has_non_pct25 = has_non_pct25_escape(&cur);
            if (prev_had_non_pct25
                || cur_has_non_pct25
                || pass.saturating_add(1) == PERCENT_DECODE_MAX_PASSES)
                && check_json_leaf_text(&cur, base)
            {
                return true;
            }
            if prev_had_struct && budget.remaining() {
                try_extract_nested_json_from_leaf(&cur, budget);
            }
        }
    }
    false
}

fn check_json_value_for_userinfo(root: &serde_json::Value, base: &url::Url) -> bool {
    const MAX_NESTED_JSON_DEPTH: u8 = 4;
    const MAX_NESTED_JSON_EXTRACTIONS: usize = 32;
    let mut nested_queue: Vec<(serde_json::Value, u8)> = Vec::new();
    let mut stack: Vec<&serde_json::Value> = vec![root];
    let mut budget = NestedJsonBudget::new(MAX_NESTED_JSON_EXTRACTIONS);

    while let Some(node) = stack.pop() {
        match node {
            serde_json::Value::String(leaf) => {
                if check_json_string_with_decodes(leaf, base, true, &mut budget) {
                    return true;
                }
            }
            serde_json::Value::Array(items) => {
                for item in items {
                    stack.push(item);
                }
            }
            serde_json::Value::Object(map) => {
                for (k, val) in map {
                    if check_json_string_with_decodes(k, base, true, &mut budget) {
                        return true;
                    }
                    stack.push(val);
                }
            }
            _ => {}
        }
    }
    for nv in budget.collected.drain(..) {
        nested_queue.push((nv, 1));
    }

    while let Some((owned_root, depth)) = nested_queue.pop() {
        let allow_more = depth < MAX_NESTED_JSON_DEPTH && budget.remaining();
        let mut inner_stack: Vec<&serde_json::Value> = vec![&owned_root];
        while let Some(node) = inner_stack.pop() {
            match node {
                serde_json::Value::String(leaf) => {
                    if check_json_string_with_decodes(leaf, base, allow_more, &mut budget) {
                        return true;
                    }
                }
                serde_json::Value::Array(items) => {
                    for item in items {
                        inner_stack.push(item);
                    }
                }
                serde_json::Value::Object(map) => {
                    for (k, val) in map {
                        if check_json_string_with_decodes(k, base, allow_more, &mut budget) {
                            return true;
                        }
                        inner_stack.push(val);
                    }
                }
                _ => {}
            }
        }
        for nv in budget.collected.drain(..) {
            nested_queue.push((nv, depth.saturating_add(1)));
        }
    }
    false
}

fn match_html_lt(bytes: &[u8], i: usize, pct_tags: bool) -> Option<usize> {
    if bytes.get(i) == Some(&b'<') {
        Some(1)
    } else if pct_tags && decode_pct_at(bytes, i) == Some(b'<') {
        Some(3)
    } else {
        None
    }
}

fn match_html_gt(bytes: &[u8], i: usize, pct_tags: bool) -> Option<usize> {
    if bytes.get(i) == Some(&b'>') {
        Some(1)
    } else if pct_tags && decode_pct_at(bytes, i) == Some(b'>') {
        Some(3)
    } else {
        None
    }
}

fn match_html_eq(bytes: &[u8], i: usize, pct_tags: bool) -> Option<usize> {
    if bytes.get(i) == Some(&b'=') {
        Some(1)
    } else if pct_tags && decode_pct_at(bytes, i) == Some(b'=') {
        Some(3)
    } else {
        None
    }
}

fn match_html_attr_sep(bytes: &[u8], i: usize) -> Option<usize> {
    let &b = bytes.get(i)?;
    if b.is_ascii_whitespace() || b == b'+' {
        Some(1)
    } else if b == b'%' && decode_pct_at(bytes, i).is_some_and(|d| d.is_ascii_whitespace()) {
        Some(3)
    } else {
        None
    }
}

fn match_html_quote(bytes: &[u8], i: usize, pct_tags: bool) -> Option<(u8, usize)> {
    match bytes.get(i).copied() {
        Some(b'"') => Some((b'"', 1)),
        Some(b'\'') => Some((b'\'', 1)),
        Some(b'%') if pct_tags => match decode_pct_at(bytes, i) {
            Some(b'"') => Some((b'"', 3)),
            Some(b'\'') => Some((b'\'', 3)),
            _ => None,
        },
        _ => None,
    }
}

/// Strictly O(1)-amortized-per-byte tracker for unclosed non-port URL authorities inside
/// HTML attribute values (`href=https://u:p...` or `%22https://u:p...%22`).
struct HtmlAuthTracker {
    auth_start: Option<usize>,
    closed: bool,
    cursor: usize,
    colon_state: AuthColonState,
}

impl HtmlAuthTracker {
    fn new(trimmed: &str, val_start: usize) -> Self {
        let auth_start = trimmed.get(val_start..).and_then(|rest| {
            let bytes = rest.as_bytes();
            if let Some(after) = strip_proto_rel_prefix(rest) {
                return Some(val_start.saturating_add(rest.len().saturating_sub(after.len())));
            }
            if has_url_scheme_prefix(rest)
                && let Some(colon) = rest
                    .get(..rest.len().min(MAX_URL_SCHEME_LEN))
                    .and_then(|p| p.find(':'))
            {
                let mut p = colon.saturating_add(1);
                while matches!(bytes.get(p), Some(b'/' | b'\\')) {
                    p = p.saturating_add(1);
                }
                if p > colon.saturating_add(1) {
                    return Some(val_start.saturating_add(p));
                }
            }
            None
        });
        let start_pos = auth_start.unwrap_or(val_start);
        Self {
            auth_start,
            closed: auth_start.is_none(),
            cursor: start_pos,
            colon_state: AuthColonState::new(trimmed, start_pos),
        }
    }

    fn advance_to(&mut self, trimmed: &str, target: usize) {
        if self.closed || target <= self.cursor {
            return;
        }
        let bytes = trimmed.as_bytes();
        while self.cursor < target {
            let Some(&b) = bytes.get(self.cursor) else {
                break;
            };
            if matches!(b, b'/' | b'\\' | b'@') {
                self.closed = true;
                self.cursor = target;
                return;
            }
            self.colon_state.push_byte(trimmed, self.cursor, b);
            self.cursor = self.cursor.saturating_add(1);
        }
    }

    fn is_unclosed_non_port(&self, pos: usize) -> bool {
        !self.closed
            && self.auth_start.is_some_and(|a| pos > a)
            && self.colon_state.is_non_port_cred(pos)
    }
}

fn is_html_tag_close_after_quote(bytes: &[u8], pos: usize, pct_tags: bool) -> bool {
    if pos >= bytes.len() {
        return true;
    }
    if match_html_gt(bytes, pos, pct_tags).is_some() {
        return true;
    }
    if bytes.get(pos) == Some(&b'/') && match_html_gt(bytes, pos.saturating_add(1), pct_tags).is_some()
    {
        return true;
    }
    if pct_tags
        && decode_pct_at(bytes, pos) == Some(b'/')
        && match_html_gt(bytes, pos.saturating_add(3), pct_tags).is_some()
    {
        return true;
    }
    if match_html_attr_sep(bytes, pos).is_some() {
        let mut p = pos;
        let limit = bytes.len().min(pos.saturating_add(64));
        while p < limit {
            if let Some(sl) = match_html_attr_sep(bytes, p) {
                p = p.saturating_add(sl);
            } else {
                break;
            }
        }
        if p >= bytes.len()
            || match_html_gt(bytes, p, pct_tags).is_some()
            || bytes.get(p) == Some(&b'/')
            || (pct_tags && decode_pct_at(bytes, p) == Some(b'/'))
        {
            return true;
        }
        let name_start = p;
        while p < limit
            && bytes
                .get(p)
                .is_some_and(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_'))
        {
            p = p.saturating_add(1);
        }
        if p > name_start
            && bytes.get(name_start).is_some_and(u8::is_ascii_alphabetic)
            && match_html_eq(bytes, p, pct_tags).is_some()
        {
            return true;
        }
    }
    false
}

fn extract_html_segments(trimmed: &str, pct_tags: bool) -> Vec<&str> {
    let bytes = trimmed.as_bytes();
    let mut segs: Vec<&str> = Vec::new();
    let mut i = 0;
    while i < bytes.len() {
        if let Some(lt_len) = match_html_lt(bytes, i, pct_tags) {
            let tag_start = i.saturating_add(lt_len);
            let mut p = tag_start;
            if bytes.get(p) == Some(&b'/') {
                p = p.saturating_add(1);
            } else if pct_tags && decode_pct_at(bytes, p) == Some(b'/') {
                p = p.saturating_add(3);
            }
            let name_start = p;
            while let Some(&b) = bytes.get(p) {
                if b.is_ascii_alphanumeric() || b == b'-' {
                    p = p.saturating_add(1);
                } else {
                    break;
                }
            }
            let has_tag_name = p > name_start
                && bytes
                    .get(name_start)
                    .is_some_and(u8::is_ascii_alphabetic)
                && (p >= bytes.len()
                    || match_html_gt(bytes, p, pct_tags).is_some()
                    || match_html_attr_sep(bytes, p).is_some()
                    || bytes.get(p) == Some(&b'/')
                    || (pct_tags && decode_pct_at(bytes, p) == Some(b'/')));

            if !has_tag_name {
                // Angle-bracketed URL, HTML comment (`<!--...-->`), or unclosed `<...`.
                let mut end = tag_start;
                while end < bytes.len() && match_html_gt(bytes, end, pct_tags).is_none() {
                    end = end.saturating_add(1);
                }
                if let Some(inner) = trimmed.get(tag_start..end)
                    && !inner.is_empty()
                {
                    segs.push(inner);
                }
                if let Some(gt_len) = match_html_gt(bytes, end, pct_tags) {
                    i = end.saturating_add(gt_len);
                } else {
                    i = end;
                }
                continue;
            }

            // Inside a real HTML tag `<tag attr = "val" ...>`: parse attributes respecting quotes.
            let mut cur = p;
            while cur < bytes.len() {
                if let Some(gt_len) = match_html_gt(bytes, cur, pct_tags) {
                    cur = cur.saturating_add(gt_len);
                    break;
                }
                if let Some(sep_len) = match_html_attr_sep(bytes, cur) {
                    cur = cur.saturating_add(sep_len);
                    continue;
                }
                if bytes.get(cur) == Some(&b'/') {
                    cur = cur.saturating_add(1);
                    continue;
                }
                if pct_tags && decode_pct_at(bytes, cur) == Some(b'/') {
                    cur = cur.saturating_add(3);
                    continue;
                }
                let attr_start = cur;
                while cur < bytes.len()
                    && match_html_eq(bytes, cur, pct_tags).is_none()
                    && match_html_attr_sep(bytes, cur).is_none()
                    && match_html_gt(bytes, cur, pct_tags).is_none()
                {
                    cur = cur.saturating_add(1);
                }
                let mut eq_pos = cur;
                while let Some(sep_len) = match_html_attr_sep(bytes, eq_pos) {
                    eq_pos = eq_pos.saturating_add(sep_len);
                }
                if let Some(eq_len) = match_html_eq(bytes, eq_pos, pct_tags) {
                    cur = eq_pos.saturating_add(eq_len);
                    while let Some(sep_len) = match_html_attr_sep(bytes, cur) {
                        cur = cur.saturating_add(sep_len);
                    }
                    if let Some((q_byte, q_len)) = match_html_quote(bytes, cur, pct_tags) {
                        let val_start = cur.saturating_add(q_len);
                        let mut val_end = val_start;
                        let mut closed_at = None;
                        let mut tracker = (q_len > 1).then(|| HtmlAuthTracker::new(trimmed, val_start));
                        while val_end < bytes.len() {
                            if q_len == 1 {
                                if bytes.get(val_end) == Some(&q_byte) {
                                    closed_at = Some(val_end.saturating_add(1));
                                    break;
                                }
                            } else if decode_pct_at(bytes, val_end) == Some(q_byte) {
                                let unclosed_non_port = tracker.as_mut().is_some_and(|t| {
                                    t.advance_to(trimmed, val_end);
                                    t.is_unclosed_non_port(val_end)
                                });
                                if !unclosed_non_port
                                    && is_html_tag_close_after_quote(
                                        bytes,
                                        val_end.saturating_add(3),
                                        pct_tags,
                                    )
                                {
                                    closed_at = Some(val_end.saturating_add(3));
                                    break;
                                }
                            }
                            val_end = val_end.saturating_add(1);
                        }
                        if let Some(val) = trimmed.get(val_start..val_end)
                            && !val.is_empty()
                        {
                            segs.push(val);
                        }
                        cur = closed_at.unwrap_or(val_end);
                    } else {
                        let val_start = cur;
                        let mut tracker = HtmlAuthTracker::new(trimmed, val_start);
                        while cur < bytes.len() && match_html_gt(bytes, cur, pct_tags).is_none() {
                            if let Some(sep_len) = match_html_attr_sep(bytes, cur) {
                                tracker.advance_to(trimmed, cur);
                                if tracker.is_unclosed_non_port(cur) {
                                    cur = cur.saturating_add(sep_len);
                                    continue;
                                }
                                break;
                            }
                            cur = cur.saturating_add(1);
                        }
                        if let Some(val) = trimmed.get(val_start..cur)
                            && !val.is_empty()
                        {
                            segs.push(val);
                        }
                    }
                } else if let Some(bare) = trimmed.get(attr_start..cur)
                    && !bare.is_empty()
                {
                    segs.push(bare);
                }
            }
            i = cur;
        } else {
            let text_start = i;
            while i < bytes.len() && match_html_lt(bytes, i, pct_tags).is_none() {
                i = i.saturating_add(1);
            }
            if let Some(text) = trimmed.get(text_start..i)
                && !text.is_empty()
            {
                segs.push(text);
            }
        }
    }
    segs
}

fn starts_like_url_or_wrapper(s: &str) -> bool {
    let no_plus = s.trim_start_matches('+');
    has_url_scheme_prefix(no_plus)
        || strip_proto_rel_prefix(no_plus).is_some()
        || matches!(
            s.as_bytes().first(),
            Some(b'(' | b'[' | b'\'' | b'"' | b'<' | b'{')
        )
}

fn check_value_for_embedded_urls(
    val: &str,
    base: &url::Url,
    from_query_or_frag: bool,
    in_raw_pass: bool,
    allow_json: bool,
) -> bool {
    if !has_at_or_unicode_escape(val) {
        return false;
    }
    let trimmed = val.trim();
    if trimmed.is_empty() {
        return false;
    }

    // 1. JSON object/array/string container (raw or structurally percent-encoded `%7B`/`%5B`/`%22`).
    if trimmed.starts_with('{') || trimmed.starts_with('[') || trimmed.starts_with('"') {
        if let Ok(jv) = serde_json::from_str::<serde_json::Value>(trimmed) {
            return allow_json && check_json_value_for_userinfo(&jv, base);
        }
    } else if in_raw_pass
        && (starts_with_pct_ci(trimmed, "%7B")
            || starts_with_pct_ci(trimmed, "%5B")
            || starts_with_pct_ci(trimmed, "%22"))
    {
        let unescaped = unescape_json_structural_pct(trimmed);
        if let Ok(jv) = serde_json::from_str::<serde_json::Value>(&unescaped) {
            return allow_json && check_json_value_for_userinfo(&jv, base);
        }
    }

    if !trimmed.as_bytes().contains(&b'@') {
        return false;
    }

    // 2. HTML / angle-bracketed container (`<...>` or `%3C...`).
    let is_pct_html = in_raw_pass && starts_with_pct_ci(trimmed, "%3C");
    if trimmed.starts_with('<') || is_pct_html {
        let outer_stripped = if let Some(inner) =
            trimmed.strip_prefix('<').and_then(|r| r.strip_suffix('>'))
        {
            Some(inner)
        } else if is_pct_html && ends_with_pct_ci(trimmed, "%3E") {
            trimmed.len().checked_sub(3).and_then(|end| trimmed.get(3..end))
        } else {
            None
        };
        if let Some(inner) = outer_stripped
            && check_single_url_userinfo(inner, base, from_query_or_frag)
        {
            return true;
        }
        for seg in extract_html_segments(trimmed, in_raw_pass) {
            if !seg.as_bytes().contains(&b'@') {
                continue;
            }
            if check_single_url_userinfo(seg, base, from_query_or_frag)
                || scan_embedded_delim_schemes(seg, base, from_query_or_frag, in_raw_pass)
                || scan_proto_rel_userinfo(seg, base, in_raw_pass, true, from_query_or_frag)
            {
                return true;
            }
            if seg.chars().any(char::is_whitespace) {
                if seg
                    .split(char::is_whitespace)
                    .any(|t| check_token_for_embedded_urls(t, base, from_query_or_frag, in_raw_pass))
                {
                    return true;
                }
            } else if check_token_for_embedded_urls(seg, base, from_query_or_frag, in_raw_pass) {
                return true;
            }
        }
        return false;
    }

    // 3. Direct check of `trimmed` when starting like a URL/wrapper (preserves `,`, `&`, `;`, `=`, `)`, `'`).
    if starts_like_url_or_wrapper(trimmed)
        && check_single_url_userinfo(trimmed, base, from_query_or_frag)
    {
        return true;
    }

    // 4. Unified strictly O(N) `:`-anchored & `//`-anchored scanning across prose, Markdown,
    //    CSV, `+`-queries, nested wrappers, 0-slash special schemes, and Pass-1 space bridging.
    scan_embedded_delim_schemes(trimmed, base, from_query_or_frag, in_raw_pass)
        || scan_proto_rel_userinfo(trimmed, base, in_raw_pass, true, from_query_or_frag)
}

fn is_plain_param_key(lhs: &str) -> bool {
    let t = lhs.trim_start();
    !t.is_empty()
        && !lhs.contains(':')
        && !lhs.contains('/')
        && !lhs.contains('\\')
        && !lhs.contains('@')
        && !matches!(t.as_bytes().first(), Some(b'{' | b'[' | b'<'))
        && !starts_with_pct_ci(t, "%7B")
        && !starts_with_pct_ci(t, "%5B")
        && !starts_with_pct_ci(t, "%3C")
}

fn check_chunk_for_embedded_urls(
    chunk: &str,
    base: &url::Url,
    in_raw_pass: bool,
    allow_json: bool,
) -> bool {
    if !has_at_or_unicode_escape(chunk) {
        return false;
    }
    if let Some((lhs, rhs)) = chunk.split_once('=')
        && is_plain_param_key(lhs)
    {
        return check_value_for_embedded_urls(rhs, base, true, in_raw_pass, allow_json);
    }
    check_value_for_embedded_urls(chunk, base, true, in_raw_pass, allow_json)
}

fn split_host_port_for_amp(auth_prefix: &str) -> (&str, Option<&str>) {
    if let Some(rest) = auth_prefix.strip_prefix('[') {
        if let Some(close) = rest.find(']') {
            let after_br = rest.get(close.saturating_add(1)..).unwrap_or("");
            let host = auth_prefix.get(..close.saturating_add(2)).unwrap_or(auth_prefix);
            if let Some(port_s) = after_br.strip_prefix(':') {
                return (host, Some(port_s));
            }
            return (host, None);
        }
        return (auth_prefix, None);
    }
    match auth_prefix.split_once(':') {
        Some((h, p)) => (h, Some(p)),
        None => (auth_prefix, None),
    }
}

#[derive(Clone, Copy)]
struct AmpSplitCtx {
    amp_pos: usize,
    at_pos: usize,
    is_fragment: bool,
    is_proto_rel: bool,
    after_qmark: bool,
}

fn is_benign_amp_host_prefix(
    scheme: &str,
    auth_prefix: &str,
    slice: &str,
    ctx: AmpSplitCtx,
) -> bool {
    if auth_prefix.is_empty() {
        return false;
    }
    if !ctx.is_proto_rel
        && !scheme.eq_ignore_ascii_case("http")
        && !scheme.eq_ignore_ascii_case("https")
    {
        return false;
    }
    // Bounded O(1) peek (<= 260 bytes) after `@` for target host, IPv4, `/`/`\`, or `:port`.
    let bytes = slice.as_bytes();
    let host_start = ctx.at_pos.saturating_add(1);
    let mut after_at_has_slash = false;
    let mut after_at_has_colon = false;
    let mut scan = host_start;
    let scan_limit = bytes.len().min(scan.saturating_add(260));
    while scan < scan_limit {
        let Some(&b) = bytes.get(scan) else {
            break;
        };
        if matches!(b, b'&' | b'?' | b'#' | b'"' | b'<' | b'>' | b' ' | b'\t' | b'\r' | b'\n') {
            break;
        }
        if b == b':' {
            after_at_has_colon = true;
            break;
        }
        if matches!(b, b'/' | b'\\') {
            after_at_has_slash = true;
            break;
        }
        scan = scan.saturating_add(1);
    }
    // Explicit `:port` on the target host is never a benign email or handle.
    if after_at_has_colon {
        return false;
    }
    let after_at_host = slice.get(host_start..scan).unwrap_or("");
    if after_at_host.parse::<std::net::Ipv4Addr>().is_ok() || after_at_host.starts_with('[') {
        return false;
    }
    let mut after_at_single_seg_path = false;
    if after_at_has_slash {
        let path_start = scan.saturating_add(1);
        let mut p = path_start;
        let p_limit = bytes.len().min(p.saturating_add(260));
        let mut has_second_slash = false;
        while p < p_limit {
            let Some(&pb) = bytes.get(p) else {
                break;
            };
            if matches!(
                pb,
                b'&' | b'?' | b'#' | b'"' | b'<' | b'>' | b' ' | b'\t' | b'\r' | b'\n'
            ) {
                break;
            }
            if matches!(pb, b'/' | b'\\') {
                has_second_slash = true;
                break;
            }
            p = p.saturating_add(1);
        }
        after_at_single_seg_path = p > path_start && !has_second_slash;
    }
    // Check ALL `&`-segments between `amp_pos` and `at_pos` for unexpected `:` or malformed pairs.
    let Some(between) = slice.get(ctx.amp_pos.saturating_add(1)..ctx.at_pos) else {
        return false;
    };
    if !amp_segments_are_benign(between, after_at_has_slash) {
        return false;
    }
    let is_scoped_handle_slash = after_at_has_slash
        && between.ends_with('=')
        && after_at_single_seg_path
        && !after_at_host.contains('.')
        && !after_at_host.eq_ignore_ascii_case("localhost");
    let is_benign_contact_key = last_amp_param_key_is_benign_contact(between);

    if ctx.is_proto_rel {
        return !ctx.is_fragment
            && !after_at_has_slash
            && auth_prefix.contains('.')
            && is_valid_hostname_str(auth_prefix);
    }

    let (host_part, port_opt) = split_host_port_for_amp(auth_prefix);
    if host_part.starts_with('[') {
        return false;
    }
    match port_opt {
        None => {
            let is_clean_host = (host_part.contains('.') && is_valid_hostname_str(host_part))
                || host_part.eq_ignore_ascii_case("localhost");
            if !is_clean_host {
                return false;
            }
            !after_at_has_slash
                || (!ctx.is_fragment
                    && (is_scoped_handle_slash || (ctx.after_qmark && host_part.contains('.'))))
        }
        Some(port_s) => {
            if port_s.is_empty() || !port_s.bytes().all(|b| b.is_ascii_digit()) {
                return false;
            }
            let Ok(port) = port_s.parse::<u16>() else {
                return false;
            };
            if port == 0 {
                return false;
            }
            if host_part.eq_ignore_ascii_case("localhost") {
                let std_port_ok = (scheme.eq_ignore_ascii_case("http")
                    && is_standard_web_port("http", port))
                    || is_standard_web_port(scheme, port);
                if !after_at_has_slash {
                    return std_port_ok;
                }
                return !ctx.is_fragment
                    && scheme.eq_ignore_ascii_case("http")
                    && is_standard_web_port("http", port)
                    && after_at_single_seg_path
                    && (is_benign_contact_key || is_scoped_handle_slash);
            }
            if host_part.contains('.') && is_valid_hostname_str(host_part) {
                return !ctx.is_fragment
                    && is_standard_web_port(scheme, port)
                    && (!after_at_has_slash || is_scoped_handle_slash || ctx.after_qmark);
            }
            !ctx.is_fragment
                && scheme.eq_ignore_ascii_case("http")
                && is_standard_web_port("http", port)
                && is_valid_hostname_str(host_part)
                && !after_at_has_slash
                && is_benign_contact_key
        }
    }
}

/// Strictly bounded backward check in `[floor..amp_pos]` (disjoint across splits -> O(N) total)
/// deciding whether `&` at `amp_pos` (with known `@` at `at_pos`) sits inside an unclosed
/// credential authority (`scheme://user:pass&a=b@host`, `//user:p&a=b@host`, or `https:u:p&a=b@host`).
fn should_suppress_amp_split(
    slice: &str,
    floor: usize,
    amp_pos: usize,
    at_pos: usize,
    is_fragment: bool,
    after_qmark: bool,
) -> bool {
    let bytes = slice.as_bytes();
    let mut p = amp_pos;
    while p > floor {
        let prev = p.saturating_sub(1);
        let Some(&b) = bytes.get(prev) else {
            break;
        };
        if matches!(
            b,
            b'/' | b'\\' | b'?' | b'#' | b'@' | b'"' | b'<' | b'>' | b' ' | b'\t' | b'\r' | b'\n'
        ) {
            break;
        }
        p = prev;
    }

    if p > floor && matches!(bytes.get(p.saturating_sub(1)), Some(b'/' | b'\\')) {
        let slash_end = p;
        let mut slash_start = p;
        while slash_start > floor
            && matches!(bytes.get(slash_start.saturating_sub(1)), Some(b'/' | b'\\'))
        {
            slash_start = slash_start.saturating_sub(1);
        }
        let slash_count = slash_end.saturating_sub(slash_start);
        if slash_start > floor && bytes.get(slash_start.saturating_sub(1)) == Some(&b':') {
            let scheme_end = slash_start.saturating_sub(1);
            let mut scheme_start = scheme_end;
            while scheme_start > floor
                && bytes.get(scheme_start.saturating_sub(1)).is_some_and(|&sb| {
                    sb.is_ascii_alphanumeric() || matches!(sb, b'+' | b'-' | b'.')
                })
            {
                scheme_start = scheme_start.saturating_sub(1);
            }
            let raw_scheme = slice.get(scheme_start..scheme_end).unwrap_or("");
            let scheme = if is_plus_joined_known_scheme(raw_scheme) {
                raw_scheme
                    .rsplit_once('+')
                    .map_or(raw_scheme, |(_, tail)| tail)
            } else {
                raw_scheme
            };
            if scheme.as_bytes().first().is_some_and(u8::is_ascii_alphabetic)
                && (slash_count >= 2 || is_whatwg_special_scheme(scheme))
            {
                let auth_prefix = slice.get(slash_end..amp_pos).unwrap_or("");
                return !is_benign_amp_host_prefix(
                    scheme,
                    auth_prefix,
                    slice,
                    AmpSplitCtx {
                        amp_pos,
                        at_pos,
                        is_fragment,
                        is_proto_rel: false,
                        after_qmark,
                    },
                );
            }
            return false;
        }
        if slash_count >= 2 {
            let prev_ok = slash_start == floor
                || slice
                    .get(floor..slash_start)
                    .is_some_and(|pref| prefix_ends_with_proto_rel_delim(pref, true));
            if prev_ok {
                let auth_prefix = slice.get(slash_end..amp_pos).unwrap_or("");
                return !is_benign_amp_host_prefix(
                    "",
                    auth_prefix,
                    slice,
                    AmpSplitCtx {
                        amp_pos,
                        at_pos,
                        is_fragment,
                        is_proto_rel: true,
                        after_qmark,
                    },
                );
            }
        }
        return false;
    }

    // No `/` or `\` between `p` and `amp_pos`: scan `:` anchors in `[p..amp_pos]` for 0-slash
    // WHATWG special schemes (`https:u:p&a=b@host`, `foo,https:u:p&a=b@host`, `see+https:...`).
    let mut c_idx = p;
    while c_idx < amp_pos {
        let Some(rel) = bytes
            .get(c_idx..amp_pos)
            .and_then(|r| r.iter().position(|&b| b == b':'))
        else {
            break;
        };
        let colon_pos = c_idx.saturating_add(rel);
        c_idx = colon_pos.saturating_add(1);
        if let Some(SchemeAnchor {
            start: scheme_start,
            scheme,
            ..
        }) = find_scheme_before_colon(slice, colon_pos, true)
            && scheme_start >= p
            && is_whatwg_special_scheme(scheme)
        {
            let auth_prefix = slice.get(colon_pos.saturating_add(1)..amp_pos).unwrap_or("");
            if !is_benign_amp_host_prefix(
                scheme,
                auth_prefix,
                slice,
                AmpSplitCtx {
                    amp_pos,
                    at_pos,
                    is_fragment,
                    is_proto_rel: false,
                    after_qmark,
                },
            ) {
                return true;
            }
        }
    }
    false
}

/// Strictly O(N) outer query/fragment chunk splitter (<= 4N byte inspections worst-case):
/// - Always splits on `?` and `#` (switching `in_frag = true` once `#` is passed).
/// - Splits on `&` ONLY when immediately followed by `key=` before `@`/`/`/`\`/`?`/`#`/`&`/`:`/ws
///   AND NOT inside an unclosed credential authority (`scheme://user:pass&a=b@host`, etc.).
/// - Uses monotonic watermarks (`no_at_until`, `known_at_pos`, `protected_until`, `floor`)
///   so forward and backward authority scans visit disjoint byte ranges.
/// - Never splits on `;`.
fn split_outer_query_chunks(slice: &str, is_fragment: bool) -> Vec<&str> {
    let bytes = slice.as_bytes();
    let has_at = bytes.contains(&b'@');
    let mut chunks = Vec::new();
    let mut start: usize = 0;
    let mut i: usize = 0;
    let mut in_frag = is_fragment;
    let mut seen_qmark = false;
    let mut no_at_until: usize = if has_at { 0 } else { bytes.len() };
    let mut known_at_pos: usize = 0;
    let mut protected_until: usize = 0;

    while i < bytes.len() {
        let Some(rel_delim) = bytes
            .get(i..)
            .and_then(|rest| rest.iter().position(|&b| matches!(b, b'?' | b'#' | b'&')))
        else {
            break;
        };
        let Some(pos) = i.checked_add(rel_delim) else {
            break;
        };
        let Some(&b) = bytes.get(pos) else {
            break;
        };
        let is_split = if matches!(b, b'?' | b'#') {
            true
        } else if pos < protected_until {
            false
        } else {
            let mut scan = pos.saturating_add(1);
            let mut found_eq = false;
            while let Some(&nb) = bytes.get(scan) {
                if nb == b'=' {
                    found_eq = scan > pos.saturating_add(1);
                    break;
                }
                if matches!(
                    nb,
                    b'@' | b'/' | b'\\' | b'?' | b'#' | b'&' | b':' | b' ' | b'\t' | b'\r' | b'\n'
                ) {
                    break;
                }
                scan = scan.saturating_add(1);
            }
            if !found_eq {
                false
            } else if pos < no_at_until {
                true
            } else {
                let at_opt = if pos < known_at_pos {
                    Some(known_at_pos)
                } else {
                    let mut end = pos.saturating_add(1);
                    while let Some(&eb) = bytes.get(end) {
                        if matches!(
                            eb,
                            b'/' | b'\\'
                                | b'?'
                                | b'#'
                                | b'@'
                                | b'"'
                                | b'<'
                                | b'>'
                                | b' '
                                | b'\t'
                                | b'\r'
                                | b'\n'
                        ) {
                            break;
                        }
                        end = end.saturating_add(1);
                    }
                    if bytes.get(end) == Some(&b'@') {
                        known_at_pos = end;
                        Some(end)
                    } else {
                        no_at_until = no_at_until.max(end);
                        None
                    }
                };
                if let Some(at_pos) = at_opt {
                    let floor = start.max(protected_until);
                    if should_suppress_amp_split(slice, floor, pos, at_pos, in_frag, seen_qmark) {
                        protected_until = protected_until.max(at_pos);
                        false
                    } else {
                        true
                    }
                } else {
                    true
                }
            }
        };
        if is_split {
            if let Some(chunk) = slice.get(start..pos)
                && !chunk.is_empty()
            {
                chunks.push(chunk);
            }
            start = pos.saturating_add(1);
            if b == b'?' {
                seen_qmark = true;
            } else if b == b'#' {
                in_frag = true;
                seen_qmark = false;
            }
        }
        i = pos.saturating_add(1);
    }
    if let Some(tail) = slice.get(start..)
        && !tail.is_empty()
    {
        chunks.push(tail);
    }
    chunks
}

fn strip_scheme_authority_tail(part: &str) -> Option<&str> {
    let scheme_pos = part.rfind("://")?;
    let after_scheme = part.get(scheme_pos.saturating_add(3)..)?;
    let bytes = after_scheme.as_bytes();
    let mut p = 0;
    if bytes.first() == Some(&b'[') {
        if let Some(close) = after_scheme.find(']') {
            p = close.saturating_add(1);
        }
    } else {
        while let Some(&b) = bytes.get(p) {
            if is_valid_hostname_char(b) {
                p = p.saturating_add(1);
            } else {
                break;
            }
        }
    }
    if bytes.get(p) == Some(&b':') {
        let mut q = p.saturating_add(1);
        while bytes.get(q).is_some_and(u8::is_ascii_digit) {
            q = q.saturating_add(1);
        }
        if q > p.saturating_add(1) && !bytes.get(q).is_some_and(u8::is_ascii_alphabetic) {
            p = q;
        }
    }
    after_scheme.get(p..)
}

fn tail_tokens_are_secret_key(work: &str) -> bool {
    let mut prev_tok: Option<&str> = None;
    let mut last_tok: Option<&str> = None;
    for tok in work
        .split(|c: char| !c.is_ascii_alphanumeric())
        .filter(|t| !t.is_empty())
    {
        prev_tok = last_tok;
        last_tok = Some(tok);
    }
    let Some(last) = last_tok else {
        return false;
    };
    if is_secret_param_key(last) {
        return true;
    }
    last.eq_ignore_ascii_case("key")
        && prev_tok
            .is_some_and(|p| p.eq_ignore_ascii_case("api") || p.eq_ignore_ascii_case("access"))
}

fn part_matches_secret_key(part: &str, idx: usize) -> bool {
    if let Some(rem) = strip_scheme_authority_tail(part) {
        return tail_tokens_are_secret_key(rem);
    }
    if idx == 0 {
        return is_secret_param_key(part);
    }
    tail_tokens_are_secret_key(part)
}

fn check_secret_key_candidate(part: &str, idx: usize, already_decoded: bool) -> bool {
    if !part_matches_secret_key(part, idx) {
        return false;
    }
    if !already_decoded && part.as_bytes().contains(&b'%') {
        let dec = percent_decode_fixed_point(part, false, PERCENT_DECODE_MAX_PASSES);
        let (dec_cand, dec_idx) = match dec.rsplit_once(['?', '#', '&', ';', ',']) {
            Some((_, tail)) => (tail, 0),
            None => (dec.as_str(), idx),
        };
        return part_matches_secret_key(dec_cand, dec_idx);
    }
    true
}

fn slice_has_secret_param_key(slice: &str, already_decoded: bool) -> bool {
    if !slice.as_bytes().contains(&b'=') {
        return false;
    }
    let mut parts = slice.split('=').peekable();
    let mut first = true;
    while let Some(part) = parts.next() {
        if parts.peek().is_none() {
            break;
        }
        let (cand, idx) = match part.rsplit_once(['?', '#', '&', ';', ',']) {
            Some((_, after_sep)) => (after_sep, 0),
            None => (part, usize::from(!first)),
        };
        if !cand.is_empty() && check_secret_key_candidate(cand, idx, already_decoded) {
            return true;
        }
        first = false;
    }
    false
}

fn scan_balanced_json_end(slice: &str, val_start: usize, is_pct: bool) -> Option<usize> {
    let bytes = slice.as_bytes();
    let next_tok = |pos: usize| -> Option<(u8, usize)> {
        let &b = bytes.get(pos)?;
        if is_pct && b == b'%' && let Some(dec) = decode_pct_at(bytes, pos) {
            return Some((dec, 3));
        }
        Some((b, 1))
    };
    let (first_b, first_len) = next_tok(val_start)?;
    if first_b == b'"' {
        let mut p = val_start.saturating_add(first_len);
        while p < bytes.len() {
            let Some((tb, tlen)) = next_tok(p) else {
                break;
            };
            p = p.saturating_add(tlen);
            if tb == b'\\' {
                if let Some((_, esc_len)) = next_tok(p) {
                    p = p.saturating_add(esc_len);
                }
            } else if tb == b'"' {
                return Some(p);
            }
        }
        return None;
    }
    if !matches!(first_b, b'{' | b'[') {
        return None;
    }
    let mut p = val_start;
    let mut depth: usize = 0;
    let mut in_str = false;
    while p < bytes.len() {
        let Some((tb, tlen)) = next_tok(p) else {
            break;
        };
        p = p.saturating_add(tlen);
        if in_str {
            if tb == b'\\' {
                if let Some((_, esc_len)) = next_tok(p) {
                    p = p.saturating_add(esc_len);
                }
            } else if tb == b'"' {
                in_str = false;
            }
        } else if tb == b'"' {
            in_str = true;
        } else if matches!(tb, b'{' | b'[') {
            depth = depth.saturating_add(1);
        } else if matches!(tb, b'}' | b']') {
            depth = depth.saturating_sub(1);
            if depth == 0 {
                return Some(p);
            }
        }
    }
    None
}

enum IntactJsonOutcome {
    Blocked,
    Complete,
    BudgetExhausted,
}

/// Strictly O(N) intact JSON parameter scanner executed BEFORE `split_outer_query_chunks`
/// so internal `?`, `#`, and in-string `&k=v` never shatter JSON objects/arrays/strings.
fn check_intact_json_params(slice: &str, base: &url::Url, in_raw_pass: bool) -> IntactJsonOutcome {
    if !has_at_or_unicode_escape(slice) {
        return IntactJsonOutcome::Complete;
    }
    let bytes = slice.as_bytes();
    let has_lit_json = bytes.iter().any(|&b| matches!(b, b'{' | b'[' | b'"'));
    if !has_lit_json && (!in_raw_pass || !bytes.contains(&b'%')) {
        return IntactJsonOutcome::Complete;
    }
    let failed_scan_budget = bytes.len().saturating_mul(4);
    let mut failed_scan_bytes: usize = 0;
    let mut i = 0;
    while i < bytes.len() {
        let mut cand_start = i;
        while matches!(bytes.get(cand_start), Some(b' ' | b'\t')) {
            cand_start = cand_start.saturating_add(1);
        }
        if let Some(rest) = slice.get(cand_start..) {
            let is_lit = matches!(rest.as_bytes().first(), Some(b'{' | b'[' | b'"'));
            let is_pct = !is_lit
                && in_raw_pass
                && (starts_with_pct_ci(rest, "%7B")
                    || starts_with_pct_ci(rest, "%5B")
                    || starts_with_pct_ci(rest, "%22"));
            if is_lit || is_pct {
                if let Some(val_end) = scan_balanced_json_end(slice, cand_start, is_pct) {
                    if let Some(raw_json) = slice.get(cand_start..val_end) {
                        let parsed = if is_pct {
                            let unesc = unescape_json_structural_pct(raw_json);
                            serde_json::from_str::<serde_json::Value>(&unesc)
                        } else {
                            serde_json::from_str::<serde_json::Value>(raw_json)
                        };
                        if let Ok(jv) = parsed {
                            if check_json_value_for_userinfo(&jv, base) {
                                return IntactJsonOutcome::Blocked;
                            }
                            i = val_end;
                            continue;
                        }
                        if !raw_json
                            .as_bytes()
                            .iter()
                            .any(|&b| matches!(b, b'&' | b'?' | b'#'))
                        {
                            i = val_end;
                            continue;
                        }
                    }
                    failed_scan_bytes =
                        failed_scan_bytes.saturating_add(val_end.saturating_sub(cand_start));
                } else {
                    failed_scan_bytes =
                        failed_scan_bytes.saturating_add(bytes.len().saturating_sub(cand_start));
                }
                if failed_scan_bytes >= failed_scan_budget {
                    return IntactJsonOutcome::BudgetExhausted;
                }
                let next_start = cand_start.saturating_add(1);
                let Some(rel) = bytes
                    .get(next_start..)
                    .and_then(|r| r.iter().position(|&b| matches!(b, b'&' | b'?' | b'#')))
                else {
                    break;
                };
                i = next_start.saturating_add(rel).saturating_add(1);
                continue;
            }
        }
        let Some(rel) = bytes
            .get(i..)
            .and_then(|r| r.iter().position(|&b| matches!(b, b'=' | b'&' | b'?' | b'#')))
        else {
            break;
        };
        i = i.saturating_add(rel).saturating_add(1);
    }
    IntactJsonOutcome::Complete
}

fn chunks_have_embedded_userinfo(
    slice: &str,
    base: &url::Url,
    in_raw_pass: bool,
    is_fragment: bool,
    allow_json: bool,
) -> bool {
    if !has_at_or_unicode_escape(slice) {
        return false;
    }
    let chunk_allow_json = if allow_json {
        match check_intact_json_params(slice, base, in_raw_pass) {
            IntactJsonOutcome::Blocked => return true,
            IntactJsonOutcome::Complete => false,
            IntactJsonOutcome::BudgetExhausted => true,
        }
    } else {
        false
    };
    for chunk in split_outer_query_chunks(slice, is_fragment) {
        if !has_at_or_unicode_escape(chunk) {
            continue;
        }
        let chunk_raw = in_raw_pass && chunk.as_bytes().contains(&b'%');
        if check_chunk_for_embedded_urls(chunk, base, chunk_raw, chunk_allow_json) {
            return true;
        }
    }
    false
}

fn inspect_param_slice(slice: &str, base: &url::Url, is_fragment: bool) -> bool {
    if slice.is_empty() {
        return false;
    }
    if slice_has_secret_param_key(slice, false) {
        return true;
    }
    let pass0_chunk_allow_json =
        match check_intact_json_params(slice, base, slice.as_bytes().contains(&b'%')) {
            IntactJsonOutcome::Blocked => return true,
            IntactJsonOutcome::Complete => false,
            IntactJsonOutcome::BudgetExhausted => true,
        };

    for chunk in split_outer_query_chunks(slice, is_fragment) {
        let chunk_has_pct = chunk.as_bytes().contains(&b'%');
        let chunk_has_at_or_u = has_at_or_unicode_escape(chunk);
        if chunk_has_at_or_u
            && check_chunk_for_embedded_urls(chunk, base, chunk_has_pct, pass0_chunk_allow_json)
        {
            return true;
        }

        let mut cur_decoded: Option<String> = None;
        if chunk_has_pct {
            let mut prev_had_json_struct = has_pct_json_or_url_structural_escape(chunk);
            let mut prev_had_at_or_u = chunk_has_at_or_u;
            let mut prev_had_non_pct25 = has_non_pct25_escape(chunk);
            if should_run_keep_pct20(chunk) {
                let keep20 = percent_decode_once_preserve_pct20(chunk);
                let keep_run_json = prev_had_json_struct
                    || (!prev_had_at_or_u && has_at_or_unicode_escape(&keep20));
                if chunks_have_embedded_userinfo(&keep20, base, true, is_fragment, keep_run_json) {
                    return true;
                }
            }
            let mut cur = percent_decode_once(chunk, false);
            if cur != chunk {
                for pass in 0..PERCENT_DECODE_MAX_PASSES {
                    if pass > 0 {
                        if !cur.as_bytes().contains(&b'%') {
                            break;
                        }
                        prev_had_json_struct = has_pct_json_or_url_structural_escape(&cur);
                        prev_had_at_or_u = has_at_or_unicode_escape(&cur);
                        prev_had_non_pct25 = has_non_pct25_escape(&cur);
                        if should_run_keep_pct20(&cur) {
                            let keep20 = percent_decode_once_preserve_pct20(&cur);
                            let keep_run_json = prev_had_json_struct
                                || (!prev_had_at_or_u && has_at_or_unicode_escape(&keep20));
                            if chunks_have_embedded_userinfo(
                                &keep20,
                                base,
                                true,
                                is_fragment,
                                keep_run_json,
                            ) {
                                return true;
                            }
                        }
                        let next = percent_decode_once(&cur, false);
                        if next == cur {
                            break;
                        }
                        cur = next;
                    }
                    let cur_has_pct = cur.as_bytes().contains(&b'%');
                    let cur_has_non_pct25 = cur_has_pct && has_non_pct25_escape(&cur);
                    if !prev_had_non_pct25
                        && !cur_has_non_pct25
                        && pass.saturating_add(1) < PERCENT_DECODE_MAX_PASSES
                    {
                        continue;
                    }
                    let cur_has_at_or_u = has_at_or_unicode_escape(&cur);
                    let run_json = prev_had_json_struct || (!prev_had_at_or_u && cur_has_at_or_u);
                    if (cur_has_at_or_u
                        && chunks_have_embedded_userinfo(
                            &cur,
                            base,
                            cur_has_pct,
                            is_fragment,
                            run_json,
                        ))
                        || slice_has_secret_param_key(&cur, !cur_has_pct)
                    {
                        return true;
                    }
                }
                if cur.chars().any(char::is_control) {
                    return true;
                }
                cur_decoded = Some(cur);
            }
        }

        let final_str = cur_decoded.as_deref().unwrap_or(chunk);
        if final_str.as_bytes().contains(&b'+') {
            let plus_decoded =
                percent_decode_fixed_point(final_str, true, PERCENT_DECODE_MAX_PASSES);
            if plus_decoded.chars().any(char::is_control)
                || contains_secret_token_or_webhook_key(&plus_decoded)
            {
                return true;
            }
        }
    }

    false
}

fn inspect_path(path: &str, base: &url::Url) -> bool {
    if path.is_empty() {
        return false;
    }
    let path_has_pct = path.as_bytes().contains(&b'%');
    if scan_embedded_delim_schemes(path, base, false, path_has_pct)
        || scan_proto_rel_userinfo(path, base, path_has_pct, true, false)
        || check_leading_extra_slash_password(path, base)
    {
        return true;
    }

    if let Some((_, after_semi)) = path.split_once(';')
        && slice_has_secret_param_key(after_semi, false)
    {
        return true;
    }

    // Per-raw-segment matrix inspection: split RAW `path` on '/' FIRST so decoded `%2F`
    // inside a `%3B`-encoded matrix segment can never shatter the segment.
    for seg in path.split('/') {
        if let Some((_, after_semi)) = seg.split_once(';') {
            if inspect_param_slice(after_semi, base, false) {
                return true;
            }
        } else if seg.as_bytes().contains(&b'%') {
            let mut cur = seg.to_string();
            let mut saw_semi = false;
            for _ in 0..PERCENT_DECODE_MAX_PASSES {
                if !cur.as_bytes().contains(&b'%') {
                    break;
                }
                let prev_had_json_struct = has_pct_json_or_url_structural_escape(&cur);
                let prev_had_at_or_u = has_at_or_unicode_escape(&cur);
                if should_run_keep_pct20(&cur) {
                    let keep20 = percent_decode_once_preserve_pct20(&cur);
                    if let Some((_, after_semi)) = keep20.split_once(';') {
                        let keep_run_json = !saw_semi
                            || prev_had_json_struct
                            || (!prev_had_at_or_u && has_at_or_unicode_escape(after_semi));
                        if chunks_have_embedded_userinfo(after_semi, base, true, false, keep_run_json)
                        {
                            return true;
                        }
                    }
                }
                let next = percent_decode_once(&cur, false);
                if next == cur {
                    break;
                }
                cur = next;
                if let Some((_, after_semi)) = cur.split_once(';') {
                    let first_semi = !saw_semi;
                    saw_semi = true;
                    let raw_flag = after_semi.as_bytes().contains(&b'%');
                    let run_json = first_semi
                        || prev_had_json_struct
                        || (!prev_had_at_or_u && has_at_or_unicode_escape(after_semi));
                    if chunks_have_embedded_userinfo(after_semi, base, raw_flag, false, run_json)
                        || slice_has_secret_param_key(after_semi, !raw_flag)
                    {
                        return true;
                    }
                }
            }
            if saw_semi {
                if cur.chars().any(char::is_control) {
                    return true;
                }
                if let Some((_, after_semi)) = cur.split_once(';')
                    && (after_semi.as_bytes().contains(&b'+') || seg.as_bytes().contains(&b'+'))
                {
                    let plus_decoded =
                        percent_decode_fixed_point(after_semi, true, PERCENT_DECODE_MAX_PASSES);
                    if plus_decoded.chars().any(char::is_control)
                        || contains_secret_token_or_webhook_key(&plus_decoded)
                    {
                        return true;
                    }
                }
            }
        }
    }

    // Per-pass path decode loop for encoded scheme userinfo, encoded protocol-relative
    // userinfo, and path-encoded `?`/`#` parameter tails.
    if path_has_pct {
        let mut cur = path.to_string();
        let mut saw_tail_delim = false;
        for _ in 0..PERCENT_DECODE_MAX_PASSES {
            if !cur.as_bytes().contains(&b'%') {
                break;
            }
            let prev_had_json_struct = has_pct_json_or_url_structural_escape(&cur);
            let prev_had_at_or_u = has_at_or_unicode_escape(&cur);
            if should_run_keep_pct20(&cur) {
                let keep20 = percent_decode_once_preserve_pct20(&cur);
                if scan_embedded_delim_schemes(&keep20, base, false, true)
                    || scan_proto_rel_userinfo(&keep20, base, true, true, false)
                    || check_leading_extra_slash_password(&keep20, base)
                {
                    return true;
                }
                if let Some(pos) = keep20.find(['?', '#'])
                    && let Some(tail) = pos.checked_add(1).and_then(|start| keep20.get(start..))
                {
                    let tail_is_frag = keep20.as_bytes().get(pos) == Some(&b'#');
                    let keep_run_json = !saw_tail_delim
                        || prev_had_json_struct
                        || (!prev_had_at_or_u && has_at_or_unicode_escape(tail));
                    if chunks_have_embedded_userinfo(tail, base, true, tail_is_frag, keep_run_json)
                    {
                        return true;
                    }
                }
            }
            let next = percent_decode_once(&cur, false);
            if next == cur {
                break;
            }
            cur = next;
            let raw_flag = cur.as_bytes().contains(&b'%');
            if scan_embedded_delim_schemes(&cur, base, false, raw_flag)
                || scan_proto_rel_userinfo(&cur, base, raw_flag, true, false)
                || check_leading_extra_slash_password(&cur, base)
            {
                return true;
            }
            if let Some((_, after_semi)) = cur.split_once(';') {
                let semi_raw = after_semi.as_bytes().contains(&b'%');
                if slice_has_secret_param_key(after_semi, !semi_raw) {
                    return true;
                }
            }
            if let Some(pos) = cur.find(['?', '#'])
                && let Some(tail) = pos.checked_add(1).and_then(|start| cur.get(start..))
            {
                let first_tail = !saw_tail_delim;
                saw_tail_delim = true;
                let tail_raw = tail.as_bytes().contains(&b'%');
                let tail_is_frag = cur.as_bytes().get(pos) == Some(&b'#');
                let run_json = first_tail
                    || prev_had_json_struct
                    || (!prev_had_at_or_u && has_at_or_unicode_escape(tail));
                if slice_has_secret_param_key(tail, !tail_raw)
                    || chunks_have_embedded_userinfo(tail, base, tail_raw, tail_is_frag, run_json)
                {
                    return true;
                }
            }
        }
        if cur.chars().any(char::is_control) {
            return true;
        }
        if let Some(pos) = cur.find(['?', '#'])
            && let Some(tail) = pos.checked_add(1).and_then(|start| cur.get(start..))
            && (tail.as_bytes().contains(&b'+') || path.as_bytes().contains(&b'+'))
        {
            let plus_decoded = percent_decode_fixed_point(tail, true, PERCENT_DECODE_MAX_PASSES);
            if plus_decoded.chars().any(char::is_control)
                || contains_secret_token_or_webhook_key(&plus_decoded)
            {
                return true;
            }
        }
    }

    false
}

fn check_unicode_unescaped_url(src: &str, top: &url::Url) -> bool {
    if !has_backslash_u_escape(src) {
        return false;
    }
    let Some(unesc) = unescape_json_u_escapes(src) else {
        return false;
    };
    if unesc.chars().any(char::is_control) || contains_secret_token_or_webhook_key(&unesc) {
        return true;
    }
    let after_scheme = unesc.split_once("://").map_or(unesc.as_str(), |(_, r)| r);
    let (before_frag, u_frag) = match after_scheme.split_once('#') {
        Some((b, f)) => (b, Some(f)),
        None => (after_scheme, None),
    };
    let (before_query, u_query) = match before_frag.split_once('?') {
        Some((b, q)) => (b, Some(q)),
        None => (before_frag, None),
    };
    let u_path = before_query.split_once('/').map_or("", |(_, p)| p);
    if inspect_path(u_path, top) {
        return true;
    }
    if let Some(q) = u_query
        && (slice_has_secret_param_key(q, true) || inspect_param_slice(q, top, false))
    {
        return true;
    }
    if let Some(f) = u_frag
        && (slice_has_secret_param_key(f, true) || inspect_param_slice(f, top, true))
    {
        return true;
    }
    false
}

/// Fail-closed, zero-recursion, strictly linear O(N) credential-URL filter backed by
/// the WHATWG URL Standard parser (`url::Url`) and `serde_json`:
/// - Rejects empty, malformed, raw/percent-encoded control-char, whitespace, and non-`http(s)` URLs.
/// - Rejects top-level authority userinfo and embedded RFC 3986 / WHATWG `scheme:` or
///   protocol-relative (`//`, `///`, `/\`, `\\`) userinfo across query, fragment, path/Wayback,
///   JSON, HTML, Markdown, CSV, bracketed/quoted, `\uXXXX`-escaped, and multi-pass percent-encoded values.
/// - Rejects secret query, fragment, RFC 3986 path matrix (`;k=v` / `%3B`), decoded path `?`/`#`,
///   comma-separated, and multi-`=` parameter keys (`sig`, `api/key`, `access/key`, `access+key`,
///   etc.) while preserving benign URLs (`localhost:3000`, `my_host:8080`, `%2F@handle`, multi-segment
///   `%2F` paths, prose search queries, raw-scheme JSON, HTML, and `auth-service%3Fx=1`).
/// - Rejects embedded API tokens, JWTs, Bearer headers, PEM headers, and Orca webhook keys
///   (including already-redacted `[orca webhook key]` markers).
pub fn link_carries_credential(raw: &str) -> bool {
    let trimmed = raw.trim();
    if trimmed.is_empty() || trimmed.chars().any(|c| c.is_control() || c.is_whitespace()) {
        return true;
    }
    let Ok(top) = url::Url::parse(trimmed) else {
        return true;
    };
    if !matches!(top.scheme(), "http" | "https") || top.host().is_none() {
        return true;
    }
    if !top.username().is_empty() || top.password().is_some() {
        return true;
    }
    let Some((_, after_scheme)) = trimmed.split_once("://") else {
        return true;
    };
    let (before_frag, raw_fragment) = match after_scheme.split_once('#') {
        Some((b, f)) => (b, Some(f)),
        None => (after_scheme, None),
    };
    let (before_query, raw_query) = match before_frag.split_once('?') {
        Some((b, q)) => (b, Some(q)),
        None => (before_frag, None),
    };
    let (raw_authority, raw_path) = match before_query.split_once('/') {
        Some((a, p)) => (a, p),
        None => (before_query, ""),
    };
    if raw_authority.is_empty() || raw_authority.contains('@') || raw_authority.contains('\\') {
        return true;
    }
    if contains_secret_token_or_webhook_key(trimmed) {
        return true;
    }
    if trimmed.as_bytes().contains(&b'%') {
        let decoded = percent_decode_fixed_point(trimmed, false, PERCENT_DECODE_MAX_PASSES);
        if decoded.chars().any(char::is_control)
            || (decoded != trimmed && contains_secret_token_or_webhook_key(&decoded))
            || check_unicode_unescaped_url(&decoded, &top)
        {
            return true;
        }
    }
    if check_unicode_unescaped_url(trimmed, &top) {
        return true;
    }
    if inspect_path(raw_path, &top) {
        return true;
    }
    if let Some(q) = raw_query
        && inspect_param_slice(q, &top, false)
    {
        return true;
    }
    if let Some(frag) = raw_fragment
        && inspect_param_slice(frag, &top, true)
    {
        return true;
    }
    false
}

/// Returns `Some(trimmed_url)` iff `raw` is a clean `http(s)` URL carrying no credentials.
pub fn sanitize_suggestion_link(raw: Option<&str>) -> Option<String> {
    let trimmed = raw?.trim();
    if trimmed.is_empty() || link_carries_credential(trimmed) {
        return None;
    }
    Some(trimmed.to_string())
}

/// Validates suggestion bounds, strips credential-bearing links before key calculation,
/// deduplicates preserving first occurrence by `suggestion_key`, and caps to `5`.
pub fn dedup_and_cap_suggestions(incoming: Vec<Suggestion>) -> Result<Vec<Suggestion>, BotError> {
    let mut seen: HashSet<String> = HashSet::new();
    let mut out: Vec<Suggestion> = Vec::new();
    for mut s in incoming {
        s.validate()?;
        s.link = sanitize_suggestion_link(s.link.as_deref());
        let key = suggestion_key(&s.title, s.link.as_deref());
        if seen.insert(key) {
            out.push(s);
            if out.len() >= SUGGESTION_LIMITS.max_per_run {
                break;
            }
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn la_01_read_only_agent_enforcement() {
        for agent in BotAgentId::ALL {
            let expected = matches!(agent, BotAgentId::Claude | BotAgentId::Codex);
            assert_eq!(is_look_around_agent_supported(agent), expected);
            assert_eq!(is_look_around_agent_str_supported(agent.as_str()), expected);
            assert_eq!(ensure_look_around_agent(agent).is_ok(), expected);
        }
        assert!(!is_look_around_agent_str_supported("gemini"));
        assert!(!is_look_around_agent_str_supported("foo"));
        assert_eq!(
            ensure_look_around_agent(BotAgentId::Cursor).unwrap_err().code,
            BotErrorCode::InvalidArgument
        );
    }

    #[test]
    fn la_02_focus_unicode_boundary_and_since_patch_semantics() {
        let mut st = LookAroundState::default();
        assert!(!st.on);
        assert!(st.schedule.weekdays_only);
        assert_eq!(st.schedule.since_ms, None);

        // 500 CJK chars (1,500 bytes) ok; 501 fails
        let ok_focus = "字".repeat(500);
        assert_eq!(ok_focus.len(), 1_500);
        st = patch_look_around(
            &st,
            LookAroundPatch {
                on: Some(true),
                every: None,
                weekdays_only: None,
                focus: Some(Some(ok_focus)),
                since_ms: None,
            },
            1_000_000,
        )
        .expect("500 chars ok & false->true stamps since");
        assert!(st.on);
        assert_eq!(st.schedule.since_ms, Some(1_000_000));

        let bad = patch_look_around(
            &st,
            LookAroundPatch {
                focus: Some(Some("字".repeat(501))),
                ..Default::default()
            },
            2_000_000,
        )
        .unwrap_err();
        assert_eq!(bad.code, BotErrorCode::InvalidArgument);

        // true -> true preserves since
        let st2 = patch_look_around(
            &st,
            LookAroundPatch {
                on: Some(true),
                every: Some(LookAroundEvery::Every2h),
                ..Default::default()
            },
            3_000_000,
        )
        .expect("true->true");
        assert_eq!(st2.schedule.since_ms, Some(1_000_000));
        assert_eq!(st2.schedule.every, LookAroundEvery::Every2h);

        // true -> false preserves since
        let st3 = patch_look_around(
            &st2,
            LookAroundPatch {
                on: Some(false),
                ..Default::default()
            },
            4_000_000,
        )
        .expect("true->false");
        assert!(!st3.on);
        assert_eq!(st3.schedule.since_ms, Some(1_000_000));
        assert!(!st3.due_now(5_000_000, None, 0).expect("disabled not due"));

        // BotLookAroundConfig conversion
        let cfg = BotLookAroundConfig {
            enabled: true,
            schedule: Some("twice-daily".to_string()),
            weekdays_only: true,
            last_run_at: None,
        };
        let converted = LookAroundState::from_bot_config(&cfg).expect("convert");
        assert!(converted.on);
        assert_eq!(converted.schedule.every, LookAroundEvery::TwiceDaily);
        assert!(converted.schedule.weekdays_only);
    }

    #[test]
    fn la_03_presets_cron_and_due_now_sleep_catchup() {
        assert_eq!(LookAroundEvery::Every4h.hours(), &[8, 12, 16, 20]);
        assert_eq!(LookAroundEvery::Every4h.interval_ms(), 4 * 3_600_000);
        assert_eq!(LookAroundEvery::Every4h.cron_expression(true), "0 8,12,16,20 * * 1-5");
        assert_eq!(LookAroundEvery::Every4h.cron_expression(false), "0 8,12,16,20 * * *");

        assert_eq!(LookAroundEvery::Every2h.hours(), &[8, 10, 12, 14, 16, 18, 20]);
        assert_eq!(LookAroundEvery::Every2h.interval_ms(), 2 * 3_600_000);
        assert_eq!(LookAroundEvery::Every2h.cron_expression(true), "0 8,10,12,14,16,18,20 * * 1-5");

        assert_eq!(LookAroundEvery::TwiceDaily.hours(), &[8, 13]);
        assert_eq!(LookAroundEvery::TwiceDaily.interval_ms(), 5 * 3_600_000);
        assert_eq!(LookAroundEvery::TwiceDaily.cron_expression(true), "0 8,13 * * 1-5");

        assert_eq!(LookAroundEvery::Mornings.hours(), &[8]);
        assert_eq!(LookAroundEvery::Mornings.interval_ms(), 24 * 3_600_000);
        assert_eq!(LookAroundEvery::Mornings.cron_expression(true), "0 8 * * 1-5");
        assert_eq!(LookAroundEvery::Mornings.cron_expression(false), "0 8 * * *");

        assert_eq!(LookAroundEvery::parse("every-5m"), None);
        assert_eq!(LookAroundEvery::parse("hourly"), None);
        assert_eq!(LookAroundEvery::parse(""), None);

        // Enabled Friday 10:00Z -> 08:00 slot skipped; 13:00 slot fires on wake at 15:30Z
        let fri_10 = parse_rfc3339_ms("2026-11-20T10:00:00Z").expect("ts");
        let fri_11 = parse_rfc3339_ms("2026-11-20T11:00:00Z").expect("ts");
        let fri_13 = parse_rfc3339_ms("2026-11-20T13:00:00Z").expect("ts");
        let fri_15_30 = parse_rfc3339_ms("2026-11-20T15:30:00Z").expect("ts");
        let sched = LookAroundSchedule {
            every: LookAroundEvery::TwiceDaily,
            weekdays_only: true,
            focus: None,
            since_ms: Some(fri_10),
        };
        assert!(!due_now(&sched, fri_11, None, 0).expect("before 13:00 slot"));
        assert!(due_now(&sched, fri_15_30, None, 0).expect("catch up 13:00 slot"));
        assert!(!due_now(&sched, fri_15_30, Some(fri_13), 0).expect("already ran at 13:00"));

        // Weekend invariant: Friday since 07:00Z, Saturday 08:30Z MUST NOT fire when weekdays_only=true
        let fri_07 = parse_rfc3339_ms("2026-11-20T07:00:00Z").expect("ts");
        let sat_08_30 = parse_rfc3339_ms("2026-11-21T08:30:00Z").expect("ts");
        let mon_08_30 = parse_rfc3339_ms("2026-11-23T08:30:00Z").expect("ts");
        let sched_wk = LookAroundSchedule {
            every: LookAroundEvery::Mornings,
            weekdays_only: true,
            focus: None,
            since_ms: Some(fri_07),
        };
        assert!(!due_now(&sched_wk, sat_08_30, None, 0).expect("sat blocked"));
        assert!(due_now(&sched_wk, mon_08_30, None, 0).expect("mon catches up 08:00"));

        // Negative UTC offset (-5h = -18000s): 13:00Z is 08:00 local
        let mon_13_00z = parse_rfc3339_ms("2026-11-23T13:00:00Z").expect("ts");
        let mon_12_59z = parse_rfc3339_ms("2026-11-23T12:59:59Z").expect("ts");
        assert!(!due_now(&sched_wk, mon_12_59z, None, -18_000).expect("07:59 local"));
        assert!(due_now(&sched_wk, mon_13_00z, None, -18_000).expect("08:00 local"));
    }

    #[test]
    fn la_04_suggestion_key_normalization_and_fallback() {
        assert_eq!(
            suggestion_key("t", Some("https://www.github.com/Org/Repo/pull/12/files#diff-1?w=1")),
            "link:github.com/org/repo/pull/12"
        );
        assert_eq!(
            suggestion_key("t", Some("https://github.com/org/repo/pull/12")),
            "link:github.com/org/repo/pull/12"
        );
        assert_eq!(
            suggestion_key("t", Some("https://github.com/org/repo/issues/99#issuecomment-5")),
            "link:github.com/org/repo/issues/99"
        );
        assert_eq!(
            suggestion_key("t", Some("https://linear.app/acme/issue/ENG-42/fix-bug?utm=1")),
            "link:linear.app/acme/issue/eng-42"
        );
        assert_eq!(
            suggestion_key("t", Some("https://www.linear.app/acme/issue/ENG-42")),
            "link:linear.app/acme/issue/eng-42"
        );
        // Non-numeric /pull/new/main is NOT truncated
        assert_eq!(
            suggestion_key("t", Some("https://github.com/org/repo/pull/new/main")),
            "link:github.com/org/repo/pull/new/main"
        );
        // Malformed URL falls back cleanly to title
        assert_eq!(
            suggestion_key("  Fix   Bug\t", Some("not://[bad")),
            "title:fix bug"
        );
        assert_eq!(suggestion_key("fix bug", None), "title:fix bug");
    }

    #[test]
    fn la_05_06_07_link_carries_credential_and_false_positive_guards() {
        // LA-05: percent-encoded & fragment bypasses
        let bad_params = [
            "https://ex.com/?%61pi_key=xyz",
            "https://ex.com/?%2561pi_key=xyz",
            "https://ex.com/?api%5Fkey=xyz",
            "https://ex.com/#access_token=abc",
            "https://ex.com/#%74oken=abc",
            "https://ex.com/#/cb?token=abc",
            "https://ex.com/?API_KEY=1",
            "https://ex.com/?x=1&sig=deadbeef",
            "https://ex.com/?signature=deadbeef",
            "https://ex.com/?auth=1",
            "https://ex.com/?session=xyz",
            "https://ex.com/?credential=1",
            "https://ex.com/?passwd=1",
            "https://ex.com/?secret=1",
            "https://ex.com/?accessKey=1",
        ];
        for url in bad_params {
            assert!(link_carries_credential(url), "expected credential rejection for {url}");
        }

        // LA-06: userinfo, path secrets, webhook keys & fail-closed
        let bad_misc = [
            "https://user:pass@ex.com/a",
            "https://user:@ex.com/a",
            "https://:pass@ex.com/a",
            "https://user@ex.com/a",
            "https://ex.com/repos/ghp_12345678901234567890/issues",
            "https://ex.com/hooks/wh_AbCdEfGh12345678/orcawh_0123456789abcdef0123456789abcdef0123456789a",
            "https://ex.com/?q=sk-ant-1234567890",
            "https://ex.com/?q=eyJ01234567.eyJ01234567.12345678",
            "not-a-url",
            "https://[",
            "",
            "javascript:alert(1)",
        ];
        for url in bad_misc {
            assert!(link_carries_credential(url), "expected credential/fail-closed for {url}");
        }

        // LA-07: benign false-positive guards
        let clean = [
            "https://github.com/org/repo/pull/12?design=1",
            "https://github.com/acme/repo/pull/42/files?w=1#diff-1",
            "https://github.com/acme/token-service/pull/55",
            "https://ex.com/?signal=ok",
            "https://ex.com/?assign=bob",
            "https://ex.com/?unsigned=true",
            "https://ex.com/?page=2&tab=checks#discussion_r123",
            "https://linear.app/team/issue/ENG-10",
            "https://linear.app/acme/issue/ENG-108/fix-auth?utm_source=digest",
        ];
        for url in clean {
            assert!(!link_carries_credential(url), "false positive on clean URL {url}");
            assert_eq!(sanitize_suggestion_link(Some(url)).as_deref(), Some(url));
        }
    }

    #[test]
    fn la_08_suggestion_limits_dedup_and_up_next() {
        let mk = |i: usize, title: String, link: Option<&str>| Suggestion {
            id: format!("s-{i}"),
            title,
            why: "w".repeat(400),
            ask: "a".repeat(2_000),
            link: link.map(str::to_string),
            created_at_ms: Some(1_000),
        };
        assert!(mk(0, "🔥".repeat(80), None).validate().is_ok());
        assert_eq!(mk(0, "🔥".repeat(81), None).validate().unwrap_err().code, BotErrorCode::InvalidArgument);

        let mut bad_why = mk(0, "ok".to_string(), None);
        bad_why.why = "w".repeat(401);
        assert_eq!(bad_why.validate().unwrap_err().code, BotErrorCode::InvalidArgument);

        let mut bad_ask = mk(0, "ok".to_string(), None);
        bad_ask.ask = "a".repeat(2_001);
        assert_eq!(bad_ask.validate().unwrap_err().code, BotErrorCode::InvalidArgument);

        // `url` alias on Suggestion
        let from_url: Suggestion = serde_json::from_str(
            r#"{"id":"1","title":"Check PR","why":"Ready","ask":"Review","url":"https://github.com/a/b/pull/1"}"#,
        )
        .expect("url alias");
        assert_eq!(from_url.link.as_deref(), Some("https://github.com/a/b/pull/1"));

        // Dedup + credential stripping + cap 5 + UpNext shown/overflow
        let incoming = vec![
            mk(1, "PR 1".to_string(), Some("https://github.com/a/b/pull/1/files")),
            mk(2, "PR 1 dup".to_string(), Some("https://www.github.com/A/B/pull/1")),
            mk(3, "Secret link stripped".to_string(), Some("https://ex.com/?token=abc")),
            mk(4, "Task 4".to_string(), None),
            mk(5, "Task 5".to_string(), None),
            mk(6, "Task 6".to_string(), None),
            mk(7, "Task 7 capped".to_string(), None),
        ];
        let deduped = dedup_and_cap_suggestions(incoming).expect("dedup");
        assert_eq!(deduped.len(), 5);
        assert_eq!(deduped[1].id, "s-3");
        assert_eq!(deduped[1].link, None);

        let up_next = BotUpNext::from_suggestions(UpNextState::Ready, deduped);
        assert_eq!(up_next.shown, 3);
        assert_eq!(up_next.overflow, 2);
        let small = BotUpNext::from_suggestions(UpNextState::Ready, up_next.suggestions[..3].to_vec());
        assert_eq!(small.shown, 3);
        assert_eq!(small.overflow, 0);
    }

    #[test]
    fn la_09_remediation_regressions() {
        // Finding #5: Extreme UTC boundary timestamps + offsets NEVER panic
        let sched = LookAroundSchedule::default();
        let max_utc_ms = chrono::DateTime::<Utc>::MAX_UTC.timestamp_millis();
        let min_utc_ms = chrono::DateTime::<Utc>::MIN_UTC.timestamp_millis();
        let out_of_range_cases = [
            (max_utc_ms, 86_399),
            (max_utc_ms, 3_600),
            (max_utc_ms - 3_600_000, 86_399),
            (min_utc_ms, -86_399),
            (min_utc_ms, -3_600),
            (min_utc_ms + 3_600_000, -86_399),
            (min_utc_ms, 86_399),
            (i64::MAX, 0),
            (i64::MIN, 0),
        ];
        for (ts, off) in out_of_range_cases {
            let err = evaluate_look_around(&sched, ts, None, off).unwrap_err();
            assert_eq!(err.code, BotErrorCode::InvalidArgument);
            assert!(due_now(&sched, ts, None, off).is_err());
        }
        // MAX_UTC with offset=0 returns Ok safely with next_slot_ms = None
        let max_eval = evaluate_look_around(&sched, max_utc_ms, None, 0).expect("max utc ok");
        assert_eq!(max_eval.next_slot_ms, None);

        // Finding #6: All 21 credential-URL filter bypasses closed
        let all_21_bypasses = [
            "https://ex.com/?%252561pi_key=1",
            "https://ex.com/?%252574oken=1",
            "https://ex.com/?%252573ig=deadbeef",
            "https://ex.com/?sig%3Ddeadbeef",
            "https://ex.com/?auth%3D1",
            "https://ex.com/?session%3Dxyz",
            "https://ex.com/?access_key%3Dxyz",
            "https://ex.com/?signature%3Dxyz",
            "https://ex.com/?apiKey%3Dxyz",
            "https://ex.com/?accessToken%3Dxyz",
            "https://ex.com/?a=1%26sig=deadbeef",
            "https://ex.com/?a=1%26auth=secret",
            "https://ex.com/#/r?a=1?sig=deadbeef",
            "https://ex.com/#/sig=deadbeef",
            "https://ex.com/?sig%00=deadbeef",
            "https://ex.com/?%00sig=deadbeef",
            "https://ex.com/?api+key=xyz",
            "https://ex.com/?access+key=xyz",
            "https://ex.com/?access%20key=xyz",
            "https://ex.com/?next=https://u:p@e.com/",
            "https://ex.com/?next=https://e.com/?session=xyz",
        ];
        for url in all_21_bypasses {
            assert!(
                link_carries_credential(url),
                "expected credential rejection for bypass URL: {url}"
            );
        }

        // Benign false-positive guards MUST remain strictly false
        let benign = [
            "https://github.com/org/repo/pull/12?design=1",
            "https://ex.com/?signal=ok",
            "https://ex.com/?assign=bob",
            "https://ex.com/?unsigned=4",
            "https://ex.com/auth/login",
            "https://github.com/acme/token-service/pull/55",
            "https://ex.com/#diff-1?w=1",
            "https://ex.com/?next=https://github.com/acme/repo/pull/42",
            "https://ex.com/?api_key",
        ];
        for url in benign {
            assert!(
                !link_carries_credential(url),
                "unexpected false positive on benign URL: {url}"
            );
        }

        // 80KB benign URL remains fast and false
        let mut big_url = String::from("https://ex.com/?p0=v");
        for i in 1..20_000 {
            big_url.push_str(&format!("&p{i}=v"));
        }
        assert!(!link_carries_credential(&big_url));

        // Finding #7: Suggestion::validate empty id & control chars + suggestion_key leading slashes
        let base_s = Suggestion {
            id: "s-1".to_string(),
            title: "Clean title".to_string(),
            why: "Line 1\nLine 2\tindented".to_string(),
            ask: "Step 1\r\nStep 2".to_string(),
            link: None,
            created_at_ms: None,
        };
        assert!(base_s.validate().is_ok());

        let mut empty_id = base_s.clone();
        empty_id.id = "   ".to_string();
        assert_eq!(empty_id.validate().unwrap_err().code, BotErrorCode::InvalidArgument);

        let mut ctrl_id = base_s.clone();
        ctrl_id.id = "s\u{0000}1".to_string();
        assert_eq!(ctrl_id.validate().unwrap_err().code, BotErrorCode::InvalidArgument);

        let mut ctrl_title = base_s.clone();
        ctrl_title.title = "bad\u{0000}title".to_string();
        assert_eq!(ctrl_title.validate().unwrap_err().code, BotErrorCode::InvalidArgument);

        let mut nl_title = base_s.clone();
        nl_title.title = "bad\ntitle".to_string();
        assert_eq!(nl_title.validate().unwrap_err().code, BotErrorCode::InvalidArgument);

        let mut ctrl_why = base_s.clone();
        ctrl_why.why = "bad\u{0007}why".to_string();
        assert_eq!(ctrl_why.validate().unwrap_err().code, BotErrorCode::InvalidArgument);

        let mut ctrl_ask = base_s;
        ctrl_ask.ask = "bad\u{001f}ask".to_string();
        assert_eq!(ctrl_ask.validate().unwrap_err().code, BotErrorCode::InvalidArgument);

        assert_eq!(
            suggestion_key("t", Some("https://example.com///foo///")),
            "link:example.com/foo"
        );
        assert_eq!(
            suggestion_key("t", Some("https://example.com///")),
            "link:example.com"
        );
    }

    #[test]
    fn la_10_iter2_remediation_regressions() {
        // BUG-I2-02 Part A: All 6 benign nested URLs with %20 / + MUST return false
        let benign_nested = [
            "https://ex.com/?next=https://example.com/docs/hello%20world",
            "https://ex.com/?next=https%3A%2F%2Fexample.com%2Fdocs%2Fhello%2520world",
            "https://ex.com/#next=https://example.com/a%20b",
            "https://ex.com/?next=https://github.com/acme/repo/issues?q=is:open+is:issue",
            "https://ex.com/?next=https://github.com/acme/repo/issues?q=is%3Aopen+is%3Aissue",
            "https://ex.com/?next=https://en.wikipedia.org/wiki/C++_guide",
        ];
        for url in benign_nested {
            assert!(
                !link_carries_credential(url),
                "false positive on benign nested URL: {url}"
            );
        }

        // BUG-I2-02 Part B (1): All 10 percent-encoded ? (%3F) / # (%23) in path MUST return true
        let encoded_path_delims = [
            "https://ex.com/a%3Fsig=deadbeef",
            "https://ex.com/a%3Fsession=xyz",
            "https://ex.com/a%3Fauth=secret",
            "https://ex.com/a%3Faccess_key=xyz",
            "https://ex.com/a%3Fsignature=deadbeef",
            "https://ex.com/a%3FapiKey=xyz",
            "https://ex.com/a%3FaccessToken=xyz",
            "https://ex.com/a%23session=xyz",
            "https://ex.com/a%23sig=deadbeef",
            "https://ex.com/redirect%3Fnext=https://u:p@e.com/",
        ];
        for url in encoded_path_delims {
            assert!(
                link_carries_credential(url),
                "expected credential rejection for encoded path delimiter URL: {url}"
            );
        }

        // BUG-I2-02 Part B (2): All 5 double-nested credential URLs via # / %23 MUST return true
        let double_nested = [
            "https://ex.com/?a=https://e.com/%23next=https://u:p@f.com/",
            "https://ex.com/#a=https://e.com/#next=https://u:p@f.com/",
            "https://ex.com/?a=https%3A%2F%2Fe.com%2F%23next%3Dhttps%3A%2F%2Fu%3Ap%40f.com%2F",
            "https://ex.com/?a=https://e.com/%23next=https://f.com/%23session=xyz",
            "https://ex.com/?a=https://e.com/%23https://u:p@f.com/",
        ];
        for url in double_nested {
            assert!(
                link_carries_credential(url),
                "expected credential rejection for double-nested URL: {url}"
            );
        }

        // BUG-I2-02 Part B (3): All 6 RFC 3986 path matrix parameters MUST return true
        let matrix_params = [
            "https://ex.com/api;jsessionid=ABCDEF123456",
            "https://ex.com/api;session=xyz",
            "https://ex.com/api;sig=deadbeef",
            "https://ex.com/api;auth=secret",
            "https://ex.com/api;access_key=xyz",
            "https://ex.com/api;apiKey=xyz",
        ];
        for url in matrix_params {
            assert!(
                link_carries_credential(url),
                "expected credential rejection for matrix param URL: {url}"
            );
        }

        // BUG-I2-02 Part B (4): All 8 sig boundary evasions MUST return true
        let sig_evasions = [
            "https://ex.com/#!sig=deadbeef",
            "https://ex.com/#/sig/=deadbeef",
            "https://ex.com/?sig[]=deadbeef",
            "https://ex.com/?q[sig]=deadbeef",
            "https://ex.com/?x-sig=deadbeef",
            "https://ex.com/?x_sig=deadbeef",
            "https://ex.com/?%EF%BB%BFsig=deadbeef",
            "https://ex.com/?%FFsig=deadbeef",
        ];
        for url in sig_evasions {
            assert!(
                link_carries_credential(url),
                "expected credential rejection for sig evasion URL: {url}"
            );
        }

        // Additional hardening checks: percent-encoded matrix ';', unencoded nested matrix URL,
        // Bearer+secret in nested URL, control chars in nested URL
        let extra_malicious = [
            "https://ex.com/api%3Bsession=xyz",
            "https://ex.com/api;next=https://u:p@e.com/",
            "https://ex.com/?next=https://example.com/?q=Bearer+eyJ01234567.eyJ01234567.12345678",
            "https://ex.com/?next=https://example.com/a%00b",
            "https://ex.com/?next=https://example.com/?api%20key=xyz",
        ];
        for url in extra_malicious {
            assert!(
                link_carries_credential(url),
                "expected credential rejection for extra malicious URL: {url}"
            );
        }

        // Benign guards remain strictly false (including benign matrix param & clean paths)
        let benign_guards = [
            "https://github.com/org/repo/pull/12?design=1",
            "https://ex.com/?signal=ok",
            "https://ex.com/?assign=bob",
            "https://ex.com/?unsigned=4",
            "https://ex.com/auth/login",
            "https://github.com/acme/token-service/pull/55",
            "https://ex.com/#diff-1?w=1",
            "https://ex.com/?next=https://github.com/acme/repo/pull/42",
            "https://ex.com/?api_key",
            "https://ex.com/v1;version=2/docs",
            "https://ex.com/a%3Fpage=2",
        ];
        for url in benign_guards {
            assert!(
                !link_carries_credential(url),
                "unexpected false positive on benign guard URL: {url}"
            );
        }
    }

    #[test]
    fn la_11_iter3_remediation_regressions() {
        use std::time::{Duration, Instant};

        // 1. DoS bombs: MUST return false and finish in strictly linear time (< 250ms in debug)
        let hash_bomb_16kb = format!("https://a.com/#{}", "https://b.com/a%20b#".repeat(800));
        let t0 = Instant::now();
        assert!(!link_carries_credential(&hash_bomb_16kb));
        let d_hash = t0.elapsed();
        assert!(
            d_hash < Duration::from_millis(250),
            "16KB # bomb took too long: {d_hash:?}"
        );

        let query_bomb_16kb = format!("https://a.com/?{}", "https://b.com/a%20b?".repeat(800));
        let t1 = Instant::now();
        assert!(!link_carries_credential(&query_bomb_16kb));
        let d_query = t1.elapsed();
        assert!(
            d_query < Duration::from_millis(250),
            "16KB ? bomb took too long: {d_query:?}"
        );

        let mut hash_kv_bomb_31kb = String::from("https://ex.com/");
        for i in 0..1_200 {
            hash_kv_bomb_31kb.push_str(&format!("#a{i}=https://ex.com/x%20y"));
        }
        let t2 = Instant::now();
        assert!(!link_carries_credential(&hash_kv_bomb_31kb));
        let d_kv = t2.elapsed();
        assert!(
            d_kv < Duration::from_millis(250),
            "31KB #a{{i}} bomb took too long: {d_kv:?}"
        );

        // 2. + / %2B prefixed nested credential URLs, path-embedded & Wayback userinfo,
        //    comma-list / goto: / JSON userinfo, protocol-relative //u:p@host, non-HTTP DSNs,
        //    deep nesting, and double-encoded password delimiters MUST return true
        let embedded_userinfo_bypasses = [
            // + / %2B form-urlencoded space prefix
            "https://ex.com/?next=+https://u:p@e.com/",
            "https://ex.com/?next=%2Bhttps://u:p@e.com/",
            "https://ex.com/?next=+https%3A%2F%2Fu%3Ap%40e.com%2F",
            "https://ex.com/#next=+https://u:p@e.com/",
            "https://ex.com/?+https://u:p@e.com/",
            "https://ex.com/#+https://u:p@e.com/",
            // Path-embedded & Wayback userinfo
            "https://ex.com/proxy/https://u:p@e.com/",
            "https://ex.com/proxy/https%3A%2F%2Fu%3Ap%40e.com%2F",
            "https://ex.com/fetch/http://admin:s3cr3t@10.0.0.1/metrics",
            "https://ex.com/v1;version=2/proxy/https://u:p@e.com/",
            "https://ex.com/redirect/https://user:pass@evil.com/path",
            "https://ex.com/redirect/https%3A%2F%2Fuser%3Apass%40evil.com%2Fpath",
            "https://ex.com/a;b=1/https://user:pass@evil.com/path",
            "https://web.archive.org/web/20260101/https://user:pass@example.com/",
            // Multi-URL comma list, goto:, JSON
            "https://ex.com/?urls=https://ok.com/a,https://u:p@e.com/",
            "https://ex.com/?r=goto:https://u:p@e.com/",
            "https://ex.com/?data=%7B%22url%22%3A%22https%3A%2F%2Fu%3Ap%40e.com%2F%22%7D",
            "https://ex.com/?data=%7B%22url%22%3A%22https%3A%2F%2Fe.com%2F%3Fsession%3Dxyz%22%7D",
            // Protocol-relative //userinfo@host
            "https://ex.com/?next=//u:p@e.com/",
            "https://ex.com/?next=%2F%2Fu%3Ap%40e.com%2F",
            "https://ex.com/#next=//u:p@e.com/",
            "https://ex.com/?next=+//u:p@e.com/",
            "https://ex.com/a;next=//u:p@e.com/",
            "https://ex.com/a%3Fnext=//u:p@e.com/",
            // Non-HTTP DSNs
            "https://ex.com/?db=postgres://admin:hunter2@db.internal:5432/prod",
            "https://ex.com/?src=ftp://user:pass@files.internal/dump.sql",
            "https://ex.com/?dsn=redis://:mysecret@cache.internal:6379/0",
            // Deep nesting (> depth 4) & double-encoded delimiters inside password
            "https://a.com/?n=https://b.com/?n=https://c.com/?n=https://d.com/?n=https://e.com/?n=https://u:p@f.com/",
            "https://a.com/?n=https://b.com/?n=https://c.com/?n=https://d.com/?n=https://e.com/?n=https://f.com/?sig=1",
            "https://ex.com/?next=https%3A%2F%2Fuser%3Ap%252Fw%40e.com%2F",
            "https://ex.com/?next=%2F%2Fuser%3Ap%252Fw%40e.com%2F",
        ];
        for url in embedded_userinfo_bypasses {
            assert!(
                link_carries_credential(url),
                "expected credential rejection for embedded/protocol-relative/DSN URL: {url}"
            );
        }

        // 3. All 7 api/access non-alnum key separator variants + Bearer+ / encoded key=
        let key_separator_bypasses = [
            "https://ex.com/?api[key]=xyz",
            "https://ex.com/?access[key]=xyz",
            "https://ex.com/?api.key=xyz",
            "https://ex.com/?access.key=xyz",
            "https://ex.com/?api/key=xyz",
            "https://ex.com/?access/key=xyz",
            "https://ex.com/?api__key=xyz",
            "https://ex.com/?api%2Bkey=xyz",
            "https://ex.com/?access%2Bkey=xyz",
            "https://ex.com/?foo%3Dsig=1",
            "https://ex.com/?q=Bearer+mysecrettoken12345",
            "https://ex.com/?q=Bearer%2Bmysecrettoken12345",
        ];
        for url in key_separator_bypasses {
            assert!(
                link_carries_credential(url),
                "expected credential rejection for key separator/Bearer URL: {url}"
            );
        }

        // 4. All webhook secret & already-redacted [orca webhook key] variants MUST return true
        let webhook_variants = [
            "https://ex.com/hooks/wh_AbCdEfGh12345678/orcawh_0123456789abcdef0123456789abcdef0123456789a",
            "https://ex.com/hooks/wh_AbCdEfGh12345678/[orca webhook key]",
            "https://ex.com/hooks/wh_AbCdEfGh12345678/[orca%20webhook%20key]",
            "https://ex.com/hooks/wh_AbCdEfGh12345678/%5Borca%20webhook%20key%5D",
            "https://ex.com/hooks/wh_AbCdEfGh12345678/[orca+webhook+key]",
            "https://ex.com/hooks/wh_AbCdEfGh12345678/[orca%20webhook%20key]EXTRA",
            "https://ex.com/?next=https%3A%2F%2Fex.com%2Fhooks%2Fwh_AbCdEfGh12345678%2F%5Borca%20webhook%20key%5D",
            "https://ex.com/?next=https%3A%2F%2Fex.com%2Fhooks%2Fwh_AbCdEfGh12345678%2F%5Borca%2Bwebhook%2Bkey%5D",
            "https://ex.com/?k=%25252525252525256frcawh_0123456789abcdef0123456789abcdef0123456789a",
            "https://ex.com/?k=\\u006frcawh_0123456789abcdef0123456789abcdef0123456789a",
            "https://ex.com/?k=%5Cu006frcawh_0123456789abcdef0123456789abcdef0123456789a",
            "https://ex.com/?k=orcawh\\_0123456789abcdef0123456789abcdef0123456789a",
            "https://ex.com/?k=orcawh%5C_0123456789abcdef0123456789abcdef0123456789a",
        ];
        for url in webhook_variants {
            assert!(
                link_carries_credential(url),
                "expected credential rejection for webhook URL variant: {url}"
            );
        }

        // 5. Encoded Unicode whitespace nested URLs, benign Wayback, benign email/@alice,
        //    and benign protocol-relative URLs MUST remain strictly false
        let benign_iter3 = [
            "https://example.com/docs/hello%C2%A0world",
            "https://ex.com/?next=https://example.com/docs/hello%C2%A0world",
            "https://ja.example.com/docs/a%E3%80%80b",
            "https://ex.com/?next=https://ja.example.com/docs/a%E3%80%80b",
            "https://example.com/docs/a%E2%80%89b",
            "https://ex.com/?next=https://example.com/docs/a%E2%80%89b",
            "https://ex.com/?next=https%3A%2F%2Fexample.com%2Fdocs%2Fa%25C2%25A0b",
            "https://web.archive.org/web/20260101/https://github.com/acme/repo/pull/42",
            "https://ex.com/?email=alice@example.com",
            "https://ex.com/?q=author:@alice",
            "https://ex.com/?next=https://mastodon.social/@alice",
            "https://ex.com/?next=//github.com/acme/repo",
            "https://ex.com/?next=//github.com/@alice",
            "https://ex.com/?data=%7B%22url%22%3A%22https%3A%2F%2Fok.com%22%2C%22email%22%3A%22alice%40example.com%22%7D",
            "https://example.com///@alice",
        ];
        for url in benign_iter3 {
            assert!(
                !link_carries_credential(url),
                "unexpected false positive on benign URL: {url}"
            );
            assert_eq!(sanitize_suggestion_link(Some(url)).as_deref(), Some(url));
        }
    }

    #[test]
    fn la_12_iter4_remediation_regressions() {
        use std::time::{Duration, Instant};

        // 1. REV4-01: All 10 protocol-relative %2F password URLs MUST return true
        let rev4_01 = [
            "https://ex.com/?next=%20//user:p%2Fw@e.com/",
            "https://ex.com/?next=%2B//user:p%2Fw@e.com/",
            "https://ex.com/?next%3D//user:p%2Fw@e.com/",
            "https://ex.com/a%3Fnext=//user:p%2Fw@e.com/",
            "https://ex.com/a%3Fnext=%2F%2Fuser%3Ap%252Fw%40e.com%2F",
            "https://ex.com/a%3Bnext=//user:p%2Fw@e.com/",
            "https://ex.com/a%3Bnext=%2F%2Fuser%3Ap%252Fw%40e.com%2F",
            "https://ex.com/a%23next=//user:p%2Fw@e.com/",
            "https://ex.com/a%23next=%2F%2Fuser%3Ap%252Fw%40e.com%2F",
            "https://ex.com/a%3F//user:p%2Fw@e.com/",
        ];
        for url in rev4_01 {
            assert!(
                link_carries_credential(url),
                "REV4-01 bypass must be rejected: {url}"
            );
        }

        // 2. C1 BUG-I4-01: All %3B+%2F matrix & per-pass path decode URLs MUST return true
        let c1_bug_i4_01 = [
            "https://ex.com/a%3Bapi%2Fkey=xyz",
            "https://ex.com/a%3Baccess%2Fkey=xyz",
            "https://ex.com/a%3B%2Fsig=deadbeef",
            "https://ex.com/a%3Bx=1%2F%26sig=deadbeef",
            "https://ex.com/a%3Bx=1%2F%26session=xyz",
            "https://ex.com/a%3Bnext=https%3A%2F%2Fe.com%2F%26session=xyz",
            "https://ex.com/a%3Bnext=foo%2FBearer+token1234567890",
            "https://ex.com/a%3Bnext=foo%2F%5Borca+webhook+key%5D",
            "https://ex.com/a%3Fnext=%2F%2Fuser%3Ap%252Fw%40e.com%2F",
            "https://ex.com/a%3Bnext=%2F%2Fuser%3Ap%252Fw%40e.com%2F",
            "https://ex.com/a%23next=%2F%2Fuser%3Ap%252Fw%40e.com%2F",
            "https://ex.com/a%3Fnext=%2F%2Fuser%3Ap%253Fw%40e.com%2F",
            "https://ex.com/a%3Fnext=%2F%2Fuser%3Ap%2523w%40e.com%2F",
        ];
        for url in c1_bug_i4_01 {
            assert!(
                link_carries_credential(url),
                "C1 BUG-I4-01 bypass must be rejected: {url}"
            );
        }

        // 3. C1 BUG-I4-02: All 13 WHATWG extra-slash & backslash userinfo URLs MUST return true
        let c1_bug_i4_02 = [
            "https://ex.com/?next=https:///u:p@e.com/",
            "https://ex.com/?next=https%3A%2F%2F%2Fu%3Ap%40e.com%2F",
            "https://ex.com/?next=http:///u:p@e.com/",
            "https://ex.com/?next=https:////u:p@e.com/",
            "https://ex.com/proxy/https:///u:p@e.com/",
            "https://ex.com/?next=///u:p@e.com/",
            "https://ex.com/?next=%2F%2F%2Fu%3Ap%40e.com%2F",
            "https://ex.com/?next=https:\\\\u:p@e.com/",
            "https://ex.com/?next=https%3A%5C%5Cu%3Ap%40e.com%2F",
            "https://ex.com/?next=https:/\\u:p@e.com/",
            "https://ex.com/?next=/\\u:p@e.com/",
            "https://ex.com/?next=%2F%5Cu%3Ap%40e.com%2F",
            "https://ex.com/?next=\\\\u:p@e.com/",
        ];
        for url in c1_bug_i4_02 {
            assert!(
                link_carries_credential(url),
                "C1 BUG-I4-02 WHATWG bypass must be rejected: {url}"
            );
        }

        // 4. C2 BUG-I4-01: All 11 RFC 3986 sub-delims across top-level, path, Wayback,
        //    encodeURIComponent query, Postgres DSN, and protocol-relative + %20 MUST return true
        let sub_delims = [
            ('!', "%21"),
            ('$', "%24"),
            ('&', "%26"),
            ('\'', "%27"),
            ('(', "%28"),
            (')', "%29"),
            ('*', "%2A"),
            ('+', "%2B"),
            (',', "%2C"),
            (';', "%3B"),
            ('=', "%3D"),
        ];
        for (ch, pct) in sub_delims {
            let variants = [
                format!("https://user:p{ch}ass@evil.com/path"),
                format!("https://ex.com/redirect/https://user:p{ch}ass@evil.com/path"),
                format!("https://web.archive.org/web/20260101/https://user:p{ch}ass@example.com/"),
                format!("https://ex.com/?next=https%3A%2F%2Fuser%3Ap{pct}ass%40evil.com%2Fpath"),
                format!("https://ex.com/?db=postgres%3A%2F%2Fadmin%3Ap{pct}w%40db.internal%3A5432%2Fprod"),
                format!("https://ex.com/?next=%2F%2Fuser%3Ap{pct}ass%40evil.com%2Fpath"),
            ];
            for url in variants {
                assert!(
                    link_carries_credential(&url),
                    "C2 BUG-I4-01 sub-delim '{ch}' must be rejected: {url}"
                );
            }
        }
        let sub_delim_extras = [
            "https://ex.com/?next=https%3A%2F%2Fuser%3Ap%20ass%40evil.com%2Fpath",
            "https://ex.com/?db=postgres://admin:p,w@db.internal:5432/prod",
            "https://ex.com/?urls=https://u:p@e.com,https://ok.com",
            "https://ex.com/?q=(https://u:p@e.com)",
            "https://ex.com/?q='https://u:p@e.com'",
            "https://ex.com/?q=https://u:p@e.com+more",
            "https://ex.com/?r=http://ok.com:https://u:p@e.com/",
        ];
        for url in sub_delim_extras {
            assert!(
                link_carries_credential(url),
                "C2 BUG-I4-01 extra must be rejected: {url}"
            );
        }

        // 5. C2 BUG-I4-02: All 5 benign false-positive URLs + extra benign guards MUST return false
        let c2_bug_i4_02_benign = [
            "https://ex.com/?next=https://mastodon.social%2F@alice",
            "https://ex.com/?next=//github.com%2F@alice",
            "https://ex.com/?data=%7B%22url%22%3A%22https://ok.com%22%2C%22email%22%3A%22alice@example.com%22%7D",
            "https://ex.com/?q=see%20https://docs.rs%20alice@example.com",
            "https://github.com/search?q=https://example.com+author:alice@example.com",
            "https://ex.com/?next=https://ok.com&email=alice@example.com",
            "https://ex.com/?next=https://ok.com&email=alice%40example.com",
            "https://ex.com/?next=https://ok.com:443&email=alice%40example.com",
            "https://ex.com/?q=see%20https://docs.rs:443%20alice@example.com",
            "https://ex.com/?next=https://mastodon.social:443%2F@alice",
            "https://ex.com/?next=file:///home/user@host",
            "https://example.com///@alice",
            "https://ex.com/?b64=YWJj==",
            "https://ex.com/?next=https://github.com/acme/auth-service/pull/42",
            "https://ex.com/?next=https%3A%2F%2Fgithub.com%2Facme%2Fauth-service%2Fx=1",
            "https://ex.com/?q=(https://ok.com)+alice@example.com",
            "https://ex.com/?q='https://ok.com'+alice@example.com",
            "https://ex.com/?q=https://ok.com,alice@example.com",
        ];
        for url in c2_bug_i4_02_benign {
            assert!(
                !link_carries_credential(url),
                "C2 BUG-I4-02 false positive on benign URL: {url}"
            );
            assert_eq!(sanitize_suggestion_link(Some(url)).as_deref(), Some(url));
        }

        // 6. C2 BUG-I4-03: All 10 multi-=/double-encoded/comma secret keys MUST return true
        let c2_bug_i4_03 = [
            "https://ex.com/?next=https%3A%2F%2Fe.com%2F%3Ffoo%253Dsig%3Ddeadbeef",
            "https://ex.com/a%3Ffoo%253Dsig=1",
            "https://ex.com/?foo=sig%3Ddeadbeef",
            "https://ex.com/?foo=session%3Dxyz",
            "https://ex.com/?foo=auth%3Dsecret",
            "https://ex.com/?foo=apiKey%3Dxyz",
            "https://ex.com/?foo=access_key%3Dxyz",
            "https://ex.com/?foo=accessToken%3Dxyz",
            "https://ex.com/?a=1,sig=deadbeef",
            "https://ex.com/?a=1,session=xyz",
            "https://ex.com/?foo=sig=deadbeef",
            "https://ex.com/?foo=session=xyz",
        ];
        for url in c2_bug_i4_03 {
            assert!(
                link_carries_credential(url),
                "C2 BUG-I4-03 secret param key must be rejected: {url}"
            );
        }

        // 7. Linear O(N) performance sanity: 31KB #a{i} bomb & 50KB 8x-encoded bomb (< 150ms dev)
        let mut hash_kv_bomb_31kb = String::from("https://ex.com/");
        for i in 0..1_200 {
            hash_kv_bomb_31kb.push_str(&format!("#a{i}=https://ex.com/x%20y"));
        }
        let t0 = Instant::now();
        assert!(!link_carries_credential(&hash_kv_bomb_31kb));
        let d_31kb = t0.elapsed();
        assert!(
            d_31kb < Duration::from_millis(150),
            "31KB #a{{i}} bomb exceeded 150ms in debug: {d_31kb:?}"
        );

        let mut pct8_bomb_50kb = String::from("https://ex.com/?q=");
        let unit = "%2525252525252520";
        while pct8_bomb_50kb.len() < 50_000 {
            pct8_bomb_50kb.push_str(unit);
        }
        let t1 = Instant::now();
        assert!(!link_carries_credential(&pct8_bomb_50kb));
        let d_50kb = t1.elapsed();
        assert!(
            d_50kb < Duration::from_millis(150),
            "50KB 8x-encoded bomb exceeded 150ms in debug: {d_50kb:?}"
        );
    }

    #[test]
    fn la_13_iter5_remediation_regressions() {
        use std::time::{Duration, Instant};

        // 1. R1/R2 REV5-01 & C1 BUG-I5-01: All quadratic 50KB & 16KB/32KB bombs MUST be strictly
        //    linear O(N) (< 150ms in unoptimized debug) and return false.
        let bomb_1a = format!(
            "https://ex.com/?q=https://[a:{}%2525252525252520@",
            ",".repeat(50_000)
        );
        let bomb_1b = format!(
            "https://ex.com/?q=https://{}:{}%2525252525252520@",
            "a".repeat(25_000),
            ",".repeat(25_000)
        );
        let bomb_1c = format!(
            "https://ex.com/?q=https://u:p@{}={}%2525252525252520",
            "a".repeat(25_000),
            ",".repeat(25_000)
        );
        let bomb_q1_16k = format!(
            "https://ex.com/?a=https://[:{}%2525252525252520@=",
            ",".repeat(16_000)
        );
        let bomb_q2_32k = format!(
            "https://ex.com/?a=https://a:@{}={}%2525252525252520",
            "x".repeat(16_000),
            ",".repeat(16_000)
        );
        let bomb_pct8_eq_50k = format!(
            "https://ex.com/?a=1&{}",
            "%2525252525252520=1&".repeat(2_500)
        );
        for (name, payload) in [
            ("bomb_1a_50kb", &bomb_1a),
            ("bomb_1b_50kb", &bomb_1b),
            ("bomb_1c_50kb", &bomb_1c),
            ("bomb_q1_16kb", &bomb_q1_16k),
            ("bomb_q2_32kb", &bomb_q2_32k),
            ("bomb_pct8_eq_50kb", &bomb_pct8_eq_50k),
        ] {
            let t = Instant::now();
            assert!(!link_carries_credential(payload), "{name} should be benign");
            let d = t.elapsed();
            assert!(
                d < Duration::from_millis(500),
                "{name} exceeded 500ms in debug: {d:?}"
            );
        }

        // 2. R1 REV5-02, C1 BUG-I5-02 & C2 BUG-I5-02: Raw query/fragment key, =, ;, & scheme userinfo
        let raw_query_frag_userinfo = [
            "https://ex.com/?https://user:pass@evil.com/path=1",
            "https://ex.com/#https://user:pass@evil.com/path=1",
            "https://ex.com/?foo=1&https://user:pass@evil.com/a=1",
            "https://ex.com/?https://user:p=ass@evil.com/path",
            "https://ex.com/#https://user:p=ass@evil.com/path",
            "https://ex.com/?https://user:p%2Fw@evil.com/path=1",
            "https://ex.com/?next=https://user:p;ass@evil.com/path",
            "https://ex.com/#next=https://user:p;ass@evil.com/path",
            "https://ex.com/?next=https://user:p&ass@evil.com/path",
            "https://ex.com/#next=https://user:p&ass@evil.com/path",
            "https://ex.com/?db=postgres://admin:p;w@db.internal:5432/prod",
            "https://ex.com/?db=postgres://admin:p&w@db.internal:5432/prod",
            "https://ex.com/?https://u:p@e.com/=1",
            "https://ex.com/?https://u:p@e.com/a=b",
            "https://ex.com/#https://u:p@e.com/a=b",
            "https://ex.com/?https://user:pass=1@e.com/",
            "https://ex.com/#https://user:dG9rZW4=@e.com/",
            "https://ex.com/?{\"url\":\"https://u:p@e.com/\",\"q\":\"a=1\"}",
            "https://ex.com/?postgres://admin:pass=1@db.internal:5432/app",
            "https://ex.com/?postgres://admin:p=w@db.internal:5432/prod",
            "https://ex.com/#postgres://admin:p=w@db.internal:5432/prod",
            "https://ex.com/?a=1&https://user:p=ass@evil.com/path",
            "https://ex.com/?next=https://user:p;w@e.com/",
            "https://ex.com/#next=https://user:p;w@e.com/",
            "https://ex.com/?next%3Dhttps://user:p%2Fw@e.com/a=b",
            "https://ex.com/?a=1&https://user:p%2Fw@e.com/=1",
            "https://ex.com/?next=https%3A%2F%2Fuser%3Ap%26a%3Db%40evil.com%2Fpath",
        ];
        for url in raw_query_frag_userinfo {
            assert!(
                link_carries_credential(url),
                "REV5-02 / BUG-I5-02 raw query/frag credential must be rejected: {url}"
            );
            assert_eq!(sanitize_suggestion_link(Some(url)), None);
        }

        // 3. R2 REV5-02 & C2 BUG-I5-01(1): Percent-encoded %22/%3C/%3E/%7B/%7D passwords
        let pct_delim_passwords = [
            "https://ex.com/?next=https://user:p%22w@evil.com/",
            "https://ex.com/?next=https://user:p%22w@evil.com/path",
            "https://ex.com/?next=https%3A%2F%2Fuser%3Ap%2522w%40evil.com%2F",
            "https://ex.com/?next=https%3A%2F%2Fuser%3Ap%2522w%40evil.com%2Fpath",
            "https://ex.com/?next=https://user:p%3Cw@evil.com/",
            "https://ex.com/?next=https://user:p%3Cw@evil.com/path",
            "https://ex.com/?next=https%3A%2F%2Fuser%3Ap%253Ew%40evil.com%2F",
            "https://ex.com/?next=https://user:p%3Ew@evil.com/path",
            "https://ex.com/redirect/https://user:p%7Bw@evil.com/",
            "https://ex.com/?next=https://user:p%7Bw@evil.com/path",
            "https://ex.com/?next=https://user:p%7Dw@evil.com/path",
            "https://ex.com/?db=postgres%3A%2F%2Fadmin%3Ap%257Dw%40db.internal%3A5432%2Fprod",
            "https://ex.com/?db=postgres%3A%2F%2Fadmin%3Ap%257B1%257Dw%40db.internal%3A5432%2Fprod",
            "https://ex.com/?next=%2F%2Fuser%3Ap%2522w%40evil.com%2F",
            "https://ex.com/redirect/https://user:p%22w@evil.com/path",
            "https://web.archive.org/web/20260101/https://user:p%7B1%7D@example.com/",
            "https://ex.com/?next=//user:p%22w@evil.com/path",
        ];
        for url in pct_delim_passwords {
            assert!(
                link_carries_credential(url),
                "R2 REV5-02 / C2 BUG-I5-01(1) pct password must be rejected: {url}"
            );
            assert_eq!(sanitize_suggestion_link(Some(url)), None);
        }

        // 4. C1 BUG-I5-03(B) & C2 BUG-I5-01(2,3): Username-only PATs, AD DOMAIN\user,
        //    pre-colon sub-delims, and dotted-username numeric-prefix passwords
        let userinfo_subdelim_and_pat = [
            "https://ex.com/?next=https://mykey%2Fsecret@e.com/",
            "https://ex.com/?next=https://mykey+secret@e.com/",
            "https://ex.com/?next=https://mykey%2Bsecret@e.com/",
            "https://ex.com/?next=https://mykey)secret@e.com/",
            "https://ex.com/?next=https://mykey%26secret@e.com/",
            "https://ex.com/?next=https://CORP%5Calice:hunter2@e.com/",
            "https://ex.com/?next=https://org%2Fbot:hunter2@e.com/",
            "https://ex.com/?next=https://user%20name:hunter2@e.com/",
            "https://ex.com/?next=https://user+name:hunter2@e.com/",
            "https://ex.com/?next=https://alice.smith:1234%2Fsecret@e.com/",
            "https://ex.com/?next=https://a.b:123)pass@e.com/",
            "https://ex.com/?next=https://a.b:123%26pass@e.com/",
            "https://ex.com/?next=https://a.b:123%3Bpass@e.com/",
            "https://ex.com/?next=https://user.name:99999%2Fpw@e.com/",
            "https://ex.com/proxy/https://alice.smith:1234%2Fsecret@e.com/",
            "https://ex.com/?next=//alice.smith:1234%2Fsecret@e.com/",
            "https://ex.com/?next=//mykey%2Fsecret@e.com/",
            "https://ex.com/?next=//CORP%5Calice:hunter2@e.com/",
            "https://ex.com/redirect/https://u,1:pass@evil.com/path",
            "https://ex.com/redirect/https://u&1:pass@evil.com/path",
            "https://ex.com/redirect/https://u;1:pass@evil.com/path",
            "https://ex.com/?next=https://alice+prod:secret@evil.com/path",
            "https://ex.com/?db=postgres://admin+ro:secret@db.internal:5432/prod",
            "https://ex.com/?next=https%3A%2F%2Fu%261%3Apass%40evil.com%2Fpath",
            "https://ex.com/?next=https%3A%2F%2Fu%252F1%3Apass%40evil.com%2Fpath",
            "https://ex.com/?next=https://token+1@evil.com/path",
            "https://ex.com/?next=https%3A%2F%2Ftoken%261%40evil.com%2Fpath",
            "https://ex.com/?next=https%3A%2F%2Fjohn.doe%3A12345%26secret%40evil.com%2Fpath",
            "https://ex.com/redirect/https://john.doe:1234,secret@evil.com/path",
            "https://web.archive.org/web/20260101/https://svc.admin:9999;pw@example.com/",
            "https://ex.com/?db=postgres%3A%2F%2Fsvc.admin%3A1234%26pw%40db.internal%3A5432%2Fprod",
            "https://ex.com/?next=https://john.doe:1234+pw@evil.com/path",
            "https://ex.com/?next=https%3A%2F%2Fjohn.doe%3A1234%252Fpw%40evil.com%2Fpath",
            "https://ex.com/?next=https://user%20name:pass@[::1]:443/",
            "https://ex.com/?next=https://user:pass%2F@e.com/",
            "https://ex.com/?next=https://user:p%20ass@evil.com",
        ];
        for url in userinfo_subdelim_and_pat {
            assert!(
                link_carries_credential(url),
                "C1 BUG-I5-03(B) / C2 BUG-I5-01(2,3) credential must be rejected: {url}"
            );
            assert_eq!(sanitize_suggestion_link(Some(url)), None);
        }

        // 5. C1 BUG-I5-03(A), C2 BUG-I5-03 & R2 NB: Benign localhost:3000 / intranet:8080 /
        //    trailing-colon / raw-scheme JSON / token-service+x=1 / auth-service%3Fx=1 MUST be false
        let benign_iter5 = [
            "https://ex.com/?url=http://localhost:3000&email=alice@example.com&note=hello%20world",
            "https://ex.com/?url=http://localhost:3000&email=alice%40example.com",
            "https://ex.com/?next=http://localhost:3000&email=alice%40example.com",
            "https://ex.com/?q=http://localhost:3000+alice@example.com",
            "https://ex.com/?q=http://localhost:3000%20@alice",
            "https://ex.com/?links=http://localhost:3000,mailto:alice@example.com",
            "https://ex.com/?url=http://intranet:8080&owner=bob@corp.com&q=a%20b",
            "https://ex.com/?q=(http://localhost:3000)+by+@alice",
            "https://ex.com/?next=http://localhost:3000%2F@alice",
            "https://ex.com/?next=http://localhost:8080%2F@alice",
            "https://ex.com/?q=see%20http://localhost:3000%20alice@example.com",
            "https://github.com/search?q=http://localhost:3000+author:alice@example.com",
            "https://ex.com/?q=http://localhost:3000,alice@example.com",
            "https://ex.com/?next=//localhost:3000%2F@alice",
            "https://ex.com/?q=[http://localhost:3000]+alice@example.com",
            "https://ex.com/?next=http://intranet:8080&contact=alice%40example.com",
            "https://ex.com/?q=see%20https://example.com:%20alice@example.com",
            "https://ex.com/?q=https://github.com/acme/token-service+x=1",
            "https://ex.com/?foo=https%3A%2F%2Fexample.com%2Fauth-service%3Fx=1",
            "https://ex.com/?data=%7B%22url%22%3A%22http://localhost:3000%22%2C%22email%22%3A%22alice@example.com%22%7D",
            "https://ex.com/?data=%7B%22url%22%3A%22https://[::1]:443%22%2C%22email%22%3A%22alice@example.com%22%7D",
            "https://ex.com/?q=[http://[::1]:3000]+alice@example.com",
            "https://ex.com/?q=see%20https://docs.rs%20alice@example.com%20check%20/home",
        ];
        for url in benign_iter5 {
            assert!(
                !link_carries_credential(url),
                "C1/C2 BUG-I5-03 false positive on benign URL: {url}"
            );
            assert_eq!(sanitize_suggestion_link(Some(url)).as_deref(), Some(url));
        }

        // 6. C1 BUG-I5-04 & C2 BUG-I5-04(1): WHATWG single-slash/backslash special schemes
        let whatwg_single_slash = [
            "https://ex.com/?next=https:/u:p@e.com/",
            "https://ex.com/?next=https:\\u:p@e.com/",
            "https://ex.com/?next=http:/u:p@e.com/",
            "https://ex.com/?next=ws:/u:p@e.com/",
            "https://ex.com/?next=wss:/u:p@e.com/",
            "https://ex.com/?next=ftp:/u:p@e.com/",
            "https://ex.com/?next=https%3A%2Fu%3Ap%40e.com%2F",
            "https://ex.com/proxy/https:/u:p@e.com/",
        ];
        for url in whatwg_single_slash {
            assert!(
                link_carries_credential(url),
                "C1/C2 BUG-I5-04 WHATWG single-slash/backslash must be rejected: {url}"
            );
            assert_eq!(sanitize_suggestion_link(Some(url)), None);
        }

        // 7. C1 BUG-I5-05 & C2 BUG-I5-04(3): Multi-pass %25XX & UTF-8 whitespace proto-rel prefix
        let multipass_proto_rel = [
            "https://ex.com/a%253B//user:p%2Fw@e.com/",
            "https://ex.com/a%253F//user:p%2Fw@e.com/",
            "https://ex.com/a%2523//user:p%2Fw@e.com/",
            "https://ex.com/?next%253D//user:p%2Fw@e.com/",
            "https://ex.com/?next=%2520//user:p%2Fw@e.com/",
            "https://ex.com/?next=%C2%A0//user:p%2Fw@e.com/",
            "https://ex.com/?next=%E3%80%80//user:p%2Fw@e.com/",
        ];
        for url in multipass_proto_rel {
            assert!(
                link_carries_credential(url),
                "C1 BUG-I5-05 / C2 BUG-I5-04(3) multi-pass proto-rel must be rejected: {url}"
            );
            assert_eq!(sanitize_suggestion_link(Some(url)), None);
        }

        // 8. C2 BUG-I5-04(2): Multi-= api/key & access/key + R1 NB 8x-encoded %2B Bearer
        let multi_eq_and_8x_plus = [
            "https://ex.com/?foo=api/key=xyz",
            "https://ex.com/?foo=access/key=xyz",
            "https://ex.com/#foo=api/key=xyz",
            "https://ex.com/a;foo=api/key=xyz",
            "https://ex.com/a;foo=access/key=xyz",
            "https://ex.com/a;api/key=xyz",
            "https://ex.com/a%3Bfoo=api/key=xyz",
            "https://ex.com/?foo=https://e.com/path/sig=deadbeef",
            "https://ex.com/?q=Bearer%252525252525252Bmysecrettoken12345",
        ];
        for url in multi_eq_and_8x_plus {
            assert!(
                link_carries_credential(url),
                "C2 BUG-I5-04(2) / R1 NB secret must be rejected: {url}"
            );
            assert_eq!(sanitize_suggestion_link(Some(url)), None);
        }

        // Extra benign guards for matrix path segments and double-encoded auth-service URLs
        let extra_benign = [
            "https://ex.com/v1;version=2/auth-service/x=1",
            "https://ex.com/v1;version=2/docs/hello+world",
            "https://ex.com/?next=https%253A%252F%252Fgithub.com%252Facme%252Fauth-service%252Fx=1",
        ];
        for url in extra_benign {
            assert!(
                !link_carries_credential(url),
                "unexpected false positive on extra benign URL: {url}"
            );
            assert_eq!(sanitize_suggestion_link(Some(url)).as_deref(), Some(url));
        }
    }

    #[test]
    fn la_14_iter6_whatwg_url_regressions() {
        // 1. R1 REV6-01, R2 REV6-02, C1 BUG-I6-01(A), C2 BUG-I6-01:
        //    No-trailing-slash, ?, #, &, comma-list, JSON, DSN, proto-rel & path credentials
        let noslash_and_sep_creds = [
            "https://ex.com/?next=https://alice+prod:hunter2@evil.com",
            "https://ex.com/?next=https://alice+prod:hunter2@evil.com?ref=1",
            "https://ex.com/?next=https://alice+prod:hunter2@evil.com#top",
            "https://ex.com/?next=https://alice+prod:hunter2@evil.com&tab=1",
            "https://ex.com/?next=https%3A%2F%2Falice%2Bprod%3Asecret%40evil.com",
            "https://ex.com/?next=https://user%20name:hunter2@evil.com",
            "https://ex.com/?next=https://user%20name:pass@evil.com?a=1",
            "https://ex.com/?next=https%3A%2F%2Fuser%2520name%3Apass%40evil.com",
            "https://ex.com/?next=https%3A%2F%2Fuser%20name%3Apass%40evil.com%3Fpage%3D1",
            "https://ex.com/?next=https://user+name:hunter2@e.com",
            "https://ex.com/?next=https://user+name:pw@evil.com?page=1",
            "https://ex.com/?next=https://user+name:pw@evil.com%23top",
            "https://ex.com/?next=https%3A%2F%2Fuser%2Bname%3Apass%40evil.com%23top",
            "https://ex.com/?next=https://alice.smith:1234&pw@evil.com",
            "https://ex.com/?next=https%3A%2F%2Falice.smith%3A1234%26pw%40evil.com",
            "https://ex.com/?next=https%3A%2F%2Fjohn.doe%3A1234%26pw%40evil.com%3Fpage%3D1",
            "https://ex.com/?next=https://alice.smith:1234,pw@evil.com",
            "https://ex.com/?next=https://alice.smith:1234%2Cpw@evil.com",
            "https://ex.com/?next=https://alice.smith:1234+pw@evil.com",
            "https://ex.com/?next=https://alice.smith:1234%20pw@evil.com",
            "https://ex.com/?next=https://alice.smith:1234%3Fpw@evil.com",
            "https://ex.com/?next=https://alice.smith:1234%23pw@evil.com",
            "https://ex.com/?next=https://user:1234%3Fpw@evil.com",
            "https://ex.com/?next=https://user:1234%23pw@evil.com",
            "https://ex.com/?next=https://user%3F1:pass@e.com",
            "https://ex.com/?next=https://user%231:pass@e.com",
            "https://ex.com/?next=https://mykey+secret@evil.com",
            "https://ex.com/?next=https://token+1@evil.com",
            "https://ex.com/?next=https://token&1@evil.com",
            "https://ex.com/?next=https://u,1:pass@evil.com",
            "https://ex.com/?next=https://u&1:pass@evil.com",
            "https://ex.com/?next=https://user:,pw@evil.com",
            "https://ex.com/?next=https://user:&pw@evil.com",
            "https://ex.com/?next=https://user:%20pw@evil.com",
            "https://ex.com/?next=https://user:+pw@evil.com",
            "https://ex.com/?db=redis://admin+ro:hunter2@redis.internal:6379",
            "https://ex.com/?db=redis://default:1234&pw@cache.internal:6379",
            "https://ex.com/?dsn=redis://default:1234,pass@cache.internal:6379",
            "https://ex.com/?db=postgres://svc.admin:1234&pw@db.internal:5432",
            "https://ex.com/?db=postgres%3A%2F%2Fsvc.admin%3A1234%26pw%40db.internal%3A5432",
            "https://ex.com/?db=postgres://admin+ro:secret@db.internal:5432",
            "https://ex.com/?db=postgres://admin+ro:pw@db.internal:5432?sslmode=require",
            "https://ex.com/?db=postgres://admin:1234+pass@db.internal:5432",
            "https://ex.com/?urls=https://user%20name:hunter2@e.com,https://ok.com",
            "https://ex.com/?urls=https://ok.com/a,https://alice.smith:1234,pw@evil.com",
            "https://ex.com/?next=//alice+prod:secret@evil.com",
            "https://ex.com/?next=//mykey+secret@e.com",
            "https://ex.com/?next=//user%20name:hunter2@e.com",
            "https://ex.com/?next=//alice.smith:1234,secret@e.com",
            "https://ex.com/?data=%7B%22url%22%3A%22https://user%20name:pass@evil.com%22%7D",
            "https://ex.com/redirect/https://u,1:pass@evil.com",
            "https://ex.com/redirect/https://u&1:pass@evil.com",
            "https://ex.com/redirect/https://john.doe:1234,secret@evil.com",
            "https://web.archive.org/web/20260101/https://user:1234&pw@example.com",
            "https://ex.com/a%20%2F%2Fmykey+secret@e.com",
            "https://ex.com/a%20%2F%2Fuser+name:hunter2@e.com",
            "https://ex.com/proxy/https://u:p@e.com/../../safe",
        ];
        for url in noslash_and_sep_creds {
            assert!(
                link_carries_credential(url),
                "Iter-6 no-slash/sep credential must be rejected: {url}"
            );
            assert_eq!(sanitize_suggestion_link(Some(url)), None);
        }

        // 2. R1 REV6-02, R2 REV6-01, C1 BUG-I6-02(A,B), C2 BUG-I6-02:
        //    %22/%3C/%3E/%7B/%7D userinfo, trailing %2F/%5C/%20 before '@', and outer delimiters
        let pct_delim_and_wrapper_creds = [
            "https://ex.com/?next=https://user%22:pass@evil.com/",
            "https://ex.com/?next=https://user%3C:pass@evil.com/",
            "https://ex.com/?next=https://user%3E:pass@evil.com/",
            "https://ex.com/?next=https://user%7B:pass@evil.com/",
            "https://ex.com/?next=https://user%7D:pass@evil.com/",
            "https://ex.com/?next=https%3A%2F%2Fuser%2522%3Apass%40evil.com%2F",
            "https://ex.com/?next=https://user:%7B%22k%22%3A1%7D@evil.com/",
            "https://ex.com/?next=https://user:%7B%7Dpass@evil.com/",
            "https://ex.com/?next=https://user:%22%22pass@evil.com/",
            "https://ex.com/?next=https://user:%3C%3Epass@evil.com/",
            "https://ex.com/?next=https://user:%22:pass@evil.com/",
            "https://ex.com/?next=https://user:%22,@evil.com/",
            "https://ex.com/?next=https://user:%22%22@evil.com/",
            "https://ex.com/?next=https://user:%7B%7D@evil.com/",
            "https://ex.com/?next=https://user:%3C%3E@evil.com/",
            "https://ex.com/?next=https://user:1234%22,w@evil.com/",
            "https://ex.com/?next=https://user:1234%7D;w@evil.com/",
            "https://ex.com/?next=https://user:1234%7B+w@evil.com/",
            "https://ex.com/?next=https://user:1234%22&w@evil.com/",
            "https://ex.com/?next=https://user:1234%7B%7D@evil.com/",
            "https://ex.com/?next=https://user:1234%3C%3E@evil.com/",
            "https://ex.com/?next=https://user:1234%7D%3B@evil.com/",
            "https://ex.com/?next=https://user:%22,pass@evil.com/",
            "https://ex.com/?next=https://user:%7B&pass@evil.com/",
            "https://ex.com/?next=https://user%22,x:pass@evil.com/",
            "https://ex.com/?next=https://tok%7B%7D@evil.com/",
            "https://ex.com/?next=https%3A%2F%2Fuser%3A1234%2522%2522%40evil.com%2F",
            "https://ex.com/?next=https%3A%2F%2Fuser%3A1234%257D%252Cpw%40evil.com%2Fpath",
            "https://ex.com/?db=postgres%3A%2F%2Fadmin%257D%3Apw%40db.internal%3A5432%2Fprod",
            "https://ex.com/redirect/https://user%22:pass@evil.com/",
            "https://ex.com/proxy/https://user:%7B%22k%22%3A1%7D@evil.com/",
            "https://ex.com/?next=//user%22:pass@evil.com/",
            "https://ex.com/?next=https://user:1234%2F@evil.com/",
            "https://ex.com/?next=https://user:1234%5C@evil.com/",
            "https://ex.com/?next=https://user:%2F@evil.com/",
            "https://ex.com/?next=https://user:%5C@evil.com/",
            "https://ex.com/?next=https://user:1234%2F%2F@evil.com/",
            "https://ex.com/?next=https://user:%20@evil.com/",
            "https://ex.com/?next=https://user:1234%20@evil.com/",
            "https://ex.com/?next=https://mykey%2F@evil.com/",
            "https://ex.com/?next=https://my.key%2F@evil.com/",
            "https://ex.com/?next=https://CORP%5C@evil.com/",
            "https://ex.com/?next=//user:1234%2F@evil.com/",
            "https://ex.com/?next=https://user:p%2Fw@evil.com%2Fpath",
            "https://ex.com/?q=(https://user)name:pass@evil.com/)",
            "https://ex.com/?q=(https://user:1234)pass@evil.com/)",
            "https://ex.com/?q='https://user'name:pass@evil.com/'",
            "https://ex.com/?q='https://user:1234'pass@evil.com/'",
            "https://ex.com/?q=(https://mykey)secret@evil.com/)",
            "https://ex.com/?q=[https://user%5D:pass@evil.com/]",
            "https://ex.com/?q=(https://user:)pass@evil.com/)",
            "https://ex.com/?q=(https://user+1:p@e.com)",
            "https://ex.com/?q=(https://user:1234%29pass@evil.com/)",
            "https://ex.com/?q=%3Chttps://user%3E:pass@evil.com/%3E",
            "https://ex.com/?q=see%20https://user%20name:pass@evil.com/",
        ];
        for url in pct_delim_and_wrapper_creds {
            assert!(
                link_carries_credential(url),
                "Iter-6 pct-delim/wrapper credential must be rejected: {url}"
            );
            assert_eq!(sanitize_suggestion_link(Some(url)), None);
        }
        // 3. R1 REV6-04, C1 BUG-I6-03(B), C2 BUG-I6-03: Raw &k=v passwords in query & fragment
        let raw_amp_eq_creds = [
            "https://ex.com/?next=https://user:p&a=b@evil.com/path",
            "https://ex.com/#next=https://user:p&a=b@evil.com/path",
            "https://ex.com/?https://user:p&a=b@evil.com/path",
            "https://ex.com/#https://user:p&a=b@evil.com/path",
            "https://ex.com/?db=postgres://admin:p&k=v@db.internal:5432/prod",
            "https://ex.com/#db=postgres://admin:p&k=v@db.internal:5432/prod",
            "https://ex.com/#db=postgres://admin:p&x=1@db.internal:5432/prod",
            "https://ex.com/#next=//user:p&a=b@evil.com/path",
            "https://ex.com/?next=https://user:p;x&a=b@evil.com/path",
        ];
        for url in raw_amp_eq_creds {
            assert!(
                link_carries_credential(url),
                "Iter-6 raw &k=v password must be rejected: {url}"
            );
            assert_eq!(sanitize_suggestion_link(Some(url)), None);
        }

        // 4. C1 BUG-I6-04 & C2 BUG-I6-05: api/access key multi-=, <//...>, WHATWG 0-slash, raw ws
        let secret_keys_0slash_and_angles = [
            "https://ex.com/?next=https://e.com/api/key=xyz",
            "https://ex.com/?next=https://e.com/access/key=xyz",
            "https://ex.com/?next=https%3A%2F%2Fe.com%2Fapi%2Fkey%3Dxyz",
            "https://ex.com/?foo=https://e.com/api/key=xyz",
            "https://ex.com/?foo=https://e.com/access/key=xyz",
            "https://ex.com/?foo=access+key=xyz",
            "https://ex.com/?foo=access%20key=xyz",
            "https://ex.com/?foo=access%2Bkey=xyz",
            "https://ex.com/?foo=https://e.com/access%20key=xyz",
            "https://ex.com/?q=<//u:p@e.com/>",
            "https://ex.com/?q=%3C//u:p@e.com/%3E",
            "https://ex.com/?q=%3C//user:p%2Fw@e.com/%3E",
            "https://ex.com/?next=https:u:p@e.com/",
            "https://ex.com/?next=http:u:p@e.com/",
            "https://ex.com/?next=ws:u:p@e.com/",
            "https://ex.com/?next=ftp:u:p@e.com/",
            "https://ex.com/a\tb",
            "https://ex.com/a\nb",
            "https://ex.com/a b",
            "https://@ex.com/a",
        ];
        for url in secret_keys_0slash_and_angles {
            assert!(
                link_carries_credential(url),
                "Iter-6 key/0-slash/angle/ws must be rejected: {url}"
            );
            assert_eq!(sanitize_suggestion_link(Some(url)), None);
        }

        // 5. R1 REV6-03, R2 NB, C1 BUG-I6-01(B), C2 BUG-I6-04: All 45+ Iter-6 benign URLs MUST pass
        let benign_iter6 = [
            // Multi-segment %2F & %3F paths
            "https://ex.com/?next=https://mastodon.social%2Fusers%2F@alice",
            "https://ex.com/?next=https%3A%2F%2Fmastodon.social%252Fusers%252F%40alice",
            "https://ex.com/?next=https://npmjs.com%2Fpackage%2F@types/node",
            "https://ex.com/?next=https://npmjs.com%2Fpackage%2F@types%2Fnode",
            "https://ex.com/?next=https://example.com%2Fusers%2Falice@example.com",
            "https://ex.com/?next=https://example.com%2Fpath%3Femail=alice@example.com",
            "https://ex.com/?next=https%3A%2F%2Fexample.com%252Fpath%253Femail%3Dalice%40example.com",
            "https://ex.com/?next=https://example.com%3Fref=alice@example.com%2Fdocs",
            "https://ex.com/?next=https://ok.com%3Fpkg=@types/node",
            "https://ex.com/?next=https%3A%2F%2Fok.com%253Fpkg%3D%40types%2Fnode",
            "https://ex.com/?next=https://ok.com%3Fpkg=@types%2Fnode",
            "https://ex.com/?next=//mastodon.social%2Fusers%2F@alice",
            "https://ex.com/?next=//github.com%2Forg%2F@alice",
            // Search queries, prose, CSV & encoded sibling params
            "https://ex.com/?q=see%20https://docs.rs%20alice@example.com%2Fnotes",
            "https://ex.com/?q=see%20https://docs.rs%20alice@example.com/notes",
            "https://ex.com/?q=see%20https://docs.rs%20alice@example.com/team",
            "https://ex.com/?q=see%20https://docs.rs%20alice@example.com%2Fdocs",
            "https://ex.com/?q=see%20https://docs.rs%20alice@example.com/profile",
            "https://ex.com/?q=https://ok.com%20alice@example.com%2Finfo",
            "https://ex.com/?q=https://ok.com%20npm:@types/node",
            "https://github.com/search?q=https://npmjs.com+@types/node",
            "https://www.google.com/search?q=https://docs.rs+@babel/core",
            "https://ex.com/?q=https://ok.com+by+@alice/posts",
            "https://ex.com/?q=http://localhost:3000+by+@alice/posts",
            "https://ex.com/?q=http://localhost:3000+alice@example.com/profile",
            "https://github.com/search?q=http://localhost:3000+author:alice@example.com%2Frepo",
            "https://ex.com/?q=https://ok.com+author:alice@example.com/team",
            "https://ex.com/?q=https://ok.com,@types/node",
            "https://ex.com/?q=https://example.com,alice@example.com%2Fpath",
            "https://ex.com/?next=https%3A%2F%2Fapp.com%2F%3Furl%3Dhttps%3A%2F%2Fok.com%26ref%3Dalice%40example.com%2Fdocs",
            "https://ex.com/?next=https%3A%2F%2Fok.com%26pkg%3D%40types%2Fnode",
            "https://ex.com/?next=http%3A%2F%2Flocalhost%3A3000%26email%3Dalice%40example.com%2Ffoo",
            // %2F@handle followed by second @handle/email
            "https://ex.com/?q=https://mastodon.social%2F@alice%20@bob",
            "https://ex.com/?q=https://mastodon.social%2F@alice+bob@example.com",
            "https://ex.com/?q=http://localhost:3000%2F@alice+bob@example.com",
            "https://ex.com/?links=https://mastodon.social%2F@alice,bob@example.com",
            // Underscore hostnames (my_host:8080, dev_box)
            "https://ex.com/?q=http://my_host:8080+alice@example.com",
            "https://ex.com/?q=http://dev_box%20@alice",
            "https://ex.com/?next=http://my_host:8080%2F@alice",
            "https://ex.com/?links=http://my_host:8080,mailto:alice@example.com",
            "https://ex.com/?data=%7B%22url%22%3A%22http://my_host:8080%22%2C%22email%22%3A%22alice@example.com%22%7D",
            // HTML & JSON with %2Fdocs and angle brackets
            "https://ex.com/?html=%3Ca%20href%3D%22https://ok.com%2Fdocs%22%3Ealice@example.com%3C/a%3E",
            "https://ex.com/?data=%7B%22url%22%3A%22https://ok.com%2Fdocs%22%2C%22email%22%3A%22alice@example.com%22%7D",
            "https://ex.com/?html=%3Chttps://ok.com%3Ealice@example.com",
            // R2 NB: schemeless auth-service%3Fx=1
            "https://ex.com/?foo=auth-service%3Fx=1",
        ];
        for url in benign_iter6 {
            assert!(
                !link_carries_credential(url),
                "Iter-6 false positive on benign URL: {url}"
            );
            assert_eq!(sanitize_suggestion_link(Some(url)).as_deref(), Some(url));
        }

        // 6. Red-team hardening: JSON array >1000 padding, JSON prose/CSV leaves, HTML single/unquoted href,
        //    prose CSV, double-encoded angle proto-rel, uppercase scheme, 8x access+key, and O(N) bombs
        let mut big_json_arr = String::from("https://ex.com/?data=[");
        for _ in 0..1_200 {
            big_json_arr.push_str("\"x\",");
        }
        big_json_arr.push_str("\"https://u:p@e.com/\"]");
        assert!(link_carries_credential(&big_json_arr));

        let redteam_creds = [
            "https://ex.com/?data=%7B%22text%22%3A%22see%20https%3A%2F%2Fu%3Ap%40e.com%2F%22%7D",
            "https://ex.com/?data=%7B%22urls%22%3A%22https://ok.com,https://u:p@e.com/%22%7D",
            "https://ex.com/?html=%3Ca%20href%3D%27https://u:p@e.com/%27%3Eclick%3C/a%3E",
            "https://ex.com/?html=%3Ca%20href%3Dhttps://u:p@e.com/%3Eclick%3C/a%3E",
            "https://ex.com/?q=see%20https://u:p@e.com,https://ok.com",
            "https://ex.com/?q=%253C//user:p%2Fw@e.com/%253E",
            "https://ex.com/?next=HTTPS:/u:p@e.com/",
            "https://ex.com/?foo=access%252525252525252Bkey=xyz",
        ];
        for url in redteam_creds {
            assert!(
                link_carries_credential(url),
                "Red-team credential must be rejected: {url}"
            );
            assert_eq!(sanitize_suggestion_link(Some(url)), None);
        }

        // Strict O(N) bombs (with '@' to defeat the no-'@' fast path) targeting unclosed non-port `&`,
        // colon-scheme loops, and proto-rel 72B prefixes
        let amp_bomb = format!(
            "https://ex.com/?next=https://user:p{}&x=1@",
            "&a=1".repeat(8_000)
        );
        let colon_bomb = format!("https://ex.com/?q={}@", "x:a:".repeat(6_000));
        let proto72_bomb = format!("https://ex.com/?q={}@", "%2520//a".repeat(6_000));
        let t = std::time::Instant::now();
        assert!(!link_carries_credential(&amp_bomb));
        assert!(!link_carries_credential(&colon_bomb));
        assert!(!link_carries_credential(&proto72_bomb));
        assert!(
            t.elapsed() < std::time::Duration::from_millis(500),
            "Linearity stress bombs exceeded 500ms in debug: {:?}",
            t.elapsed()
        );
    }

    #[test]
    fn la_15_iter7_remediation_regressions() {
        use std::time::{Duration, Instant};

        // 1. CRITICAL O(N^2) DoS bombs (R1/R2 REV7-01, C1/C2 BUG-I7-01) MUST all be strictly
        //    linear O(N) (< 500ms in unoptimized debug) and return false.
        let w7_q1b_48kb = format!(
            "https://ex.com/?{}/@%2525252525252520",
            "&a:b:c".repeat(8_000)
        );
        let w7_q2b_32kb = format!("https://ex.com/?{}x=1", "k=a:b:c&".repeat(4_000));
        let w7_q3_64kb = format!(
            "https://ex.com/?{}@%2525252525252520",
            "&a".repeat(32_000)
        );
        let w7_q3b_64kb = format!("https://ex.com/?{}", "?a".repeat(32_000));
        let r2_0slash_48kb = format!("https://ex.com/?{}", "&a=https:u:p".repeat(4_000));
        let r2_0slash_at_24kb = format!(
            "https://ex.com/?{}/@%2525252525252520",
            "&a=https:u:p".repeat(2_000)
        );
        let i7_nonport_amp_48kb = format!("https://ex.com/?{}", "a:b:x&".repeat(8_000));
        let i7_nonport_amp_pct8_24kb = format!(
            "https://ex.com/?x=@&{}%2525252525252520",
            "a:b:c&".repeat(4_000)
        );
        let i7_hash_no_eq_32kb = format!("https://ex.com/?{}", "#".repeat(32_000));
        let i7_amp_kv_at_48kb = format!(
            "https://ex.com/?{}@%2525252525252520",
            "&a=1".repeat(12_000)
        );
        let i7_prose_48kb = format!(
            "https://ex.com/?q=see{}%20alice@example.com",
            "%20https://ok.com".repeat(2_500)
        );
        let i7_semi_http_45kb = format!("https://ex.com/?q={}@", ";http://a".repeat(5_000));
        let i7_paren_0slash_45kb = format!("https://ex.com/?q={}@", "(https:a)".repeat(5_000));
        for (name, payload) in [
            ("w7_q1b_48kb", &w7_q1b_48kb),
            ("w7_q2b_32kb", &w7_q2b_32kb),
            ("w7_q3_64kb", &w7_q3_64kb),
            ("w7_q3b_64kb", &w7_q3b_64kb),
            ("r2_0slash_48kb", &r2_0slash_48kb),
            ("r2_0slash_at_24kb", &r2_0slash_at_24kb),
            ("i7_nonport_amp_48kb", &i7_nonport_amp_48kb),
            ("i7_nonport_amp_pct8_24kb", &i7_nonport_amp_pct8_24kb),
            ("i7_hash_no_eq_32kb", &i7_hash_no_eq_32kb),
            ("i7_amp_kv_at_48kb", &i7_amp_kv_at_48kb),
            ("i7_prose_48kb", &i7_prose_48kb),
            ("i7_semi_http_45kb", &i7_semi_http_45kb),
            ("i7_paren_0slash_45kb", &i7_paren_0slash_45kb),
        ] {
            let t = Instant::now();
            assert!(!link_carries_credential(payload), "{name} should be benign");
            let d = t.elapsed();
            assert!(
                d < Duration::from_millis(500),
                "{name} exceeded 500ms in debug: {d:?}"
            );
        }

        // 2. Path adjacent-scheme skips, backslash paths, delim-schemes, proto-rel & raw `..` collapse
        let path_delim_proto_creds = [
            "https://ex.com/foo:bar/https://u:p@evil.com/",
            "https://ex.com/a:b/https://u:p@e.com/",
            "https://ex.com/v1:2/https://user:pass@evil.com/secret",
            "https://ex.com/a:b/http://admin:s3cr3t@10.0.0.1/metrics",
            "https://ex.com/v1:2/postgres://admin:hunter2@db.internal:5432/prod",
            "https://ex.com/a:b/https%3A%2F%2Fu%3Ap%40evil.com%2F",
            "https://ex.com/https://ok.com/https://u:p@evil.com/",
            "https://ex.com/http://ok.com/https://u:p@evil.com/",
            "https://ex.com/proxy/https://ok.com/https://u:p@evil.com/",
            "https://ex.com/redirect/https://ok.com/https://user:pass@evil.com/",
            "https://web.archive.org/web/20260101/https://ok.com/https://user:pass@evil.com/",
            "https://web.archive.org/web/20260101/https://proxy.example.com/https://user:pass@evil.com/",
            "https://ex.com/?next=https://ok.com/a:b/https://user:pass@evil.com/",
            "https://ex.com/?next=https://ok.com/https://ok2.com/https://u:p@evil.com/",
            "https://ex.com/?next=https%3A%2F%2Fok.com%2Fa%3Ab%2Fhttps%3A%2F%2Fuser%3Apass%40evil.com%2F",
            "https://ex.com/?r=a:b:https://u:p@evil.com/",
            "https://ex.com/?r=urn:redirect:https://user:pass@evil.com/",
            "https://ex.com/?r=a:b:c;https://user:pass@evil.com/",
            "https://ex.com/?q=//ok.com;//u:p@e.com/",
            "https://ex.com/?q=//ok.com%20//user:p%2Fw@e.com/",
            "https://ex.com/proxy\\https://user:pass@evil.com/",
            "https://ex.com/proxy%5Chttps://user:pass@evil.com/",
            "https://ex.com/?next=https://ok.com/proxy/https://u:p@evil.com/../../safe",
            "https://ex.com/?data=%7B%22url%22%3A%22https://ok.com/proxy/https://u:p@evil.com/../../safe%22%7D",
        ];
        for url in path_delim_proto_creds {
            assert!(
                link_carries_credential(url),
                "Iter-7 path/delim/proto/dotdot credential must be rejected: {url}"
            );
            assert_eq!(sanitize_suggestion_link(Some(url)), None);
        }

        // 3. Tightened is_benign_embedded_userinfo & raw/encoded &a=b credentials (Branches 1/2/3)
        let benign_whitelist_bypasses = [
            // Branch 3: dotted user + sep + password/path, http://user:port+pw, localhost:1234+pw
            "https://ex.com/?next=https://alice.smith+prod:hunter2@evil.com",
            "https://ex.com/?next=https://alice.smith+prod:hunter2@evil.com/secret",
            "https://ex.com/?next=https://alice.smith+prod:secret@evil.com/admin",
            "https://ex.com/?next=https://alice.smith,prod:hunter2@evil.com/secret",
            "https://ex.com/?next=https://alice.smith%20prod:hunter2@evil.com/secret",
            "https://ex.com/?next=https%3A%2F%2Falice.smith%2520prod%3Ahunter2%40evil.com%2Fsecret",
            "https://ex.com/?next=https://john.doe%20admin:hunter2@evil.com/api",
            "https://ex.com/?next=https://svc.bot,ro:s3cr3t@evil.com/v1",
            "https://ex.com/?next=https://alice.smith+secret@evil.com/",
            "https://ex.com/?next=https://alice.smith,secret@evil.com/api",
            "https://ex.com/?next=https://alice.smith%20secret@evil.com/api",
            "https://ex.com/?next=https://api.token+secret@evil.com/",
            "https://ex.com/?next=https://api.token,secret@evil.com/admin",
            "https://ex.com/?next=https://api.token%20secret@evil.com/admin",
            "https://ex.com/?next=https%3A%2F%2Fapi.token%2520secret%40evil.com%2Fadmin",
            "https://ex.com/?next=https://tok.v1+secret@evil.com/",
            "https://ex.com/?next=https://alice.smith,ro@evil.com/admin",
            "https://ex.com/?next=https://alice.smith%20admin@evil.com/",
            "https://ex.com/?next=https://my.key+secret@evil.com/?k=1",
            "https://ex.com/?next=https://mykey+@intranet/",
            "https://ex.com/?next=http://admin:1234+secret@evil.com/admin",
            "https://ex.com/?next=http://admin:1234,secret@evil.com/admin",
            "https://ex.com/?next=http://root:8080+hunter2@10.0.0.1/metrics",
            "https://ex.com/?next=http://root:9999+pass@10.0.0.1/metrics",
            "https://ex.com/?next=http://user:1234+hunter2@evil.com/",
            "https://ex.com/?next=http://user:1234+pass@evil.com/",
            "https://ex.com/?next=http://user:1234,pass@evil.com/",
            "https://ex.com/?next=http://user:1234+secret@evil.com/",
            "https://ex.com/?next=http://user:1234,secret@evil.com/",
            "https://ex.com/?next=http://user:1234+secret@evil.com/admin?x=1",
            "https://ex.com/?next=http://user:9999,pw@10.0.0.1/metrics",
            "https://ex.com/?next=https://localhost:1234+secret@evil.com/",
            "https://ex.com/?next=https://localhost:1234+secret@evil.com/admin",
            "https://ex.com/?next=https://localhost:1234,secret@evil.com/",
            "https://ex.com/?next=https://user:1234+@intranet/",
            // Branch 1: %2F/%3F bypasses across scheme/port/path/dotless
            "https://ex.com/?next=https://user:1234%2F@evil.com/@ok",
            "https://ex.com/?next=https://user:1234%2F@intranet",
            "https://ex.com/?next=https://user:1234%2F@intranet/secret",
            "https://ex.com/?next=https://user:1234%2F@db-internal:5432/prod",
            "https://ex.com/?next=https://user:1234%2F@[::1]/admin",
            "https://ex.com/?next=https://admin:8080%2F@redis:6379",
            "https://ex.com/?next=postgres://admin:1234%2F@db",
            "https://ex.com/?db=postgres://admin:1234%2F@db-internal:5432/prod",
            "https://ex.com/?db=postgres://admin:5432%2F@db:5432",
            "https://ex.com/?dsn=redis://default:1234%2F@redis:6379/0",
            "https://ex.com/?next=https://my.key%2Ffoo%2Fbar@evil.com/secret",
            "https://ex.com/?next=https://alice.smith%2Ffoo%2Fbar@evil.com/api",
            "https://ex.com/?next=https://my.key%2Fa%2Fb@evil.com:8443/admin",
            "https://ex.com/?next=https://my.key%2Ffoo%2Fsecret@evil.com:8443",
            "https://ex.com/?next=https://my.key%2Ffoo%2Fsecret@evil.com",
            "https://ex.com/?db=postgres://svc.key%2Fa%2Fb@db.internal:5432/prod",
            "https://ex.com/?db=postgres://svc.ro%2Fa%2Fb@db.internal:5432",
            "https://ex.com/redirect/https://my.key%2Ffoo%2Fsecret@evil.com",
            "https://ex.com/?next=https://my.key%2Fsecret@evil.com/@team",
            "https://ex.com/?next=https://a.b%2Fc@evil.com/@x",
            "https://ex.com/?next=https://my.key%2F@localhost/admin",
            "https://ex.com/?next=https://my.key%2F@internal:8080/admin",
            "https://ex.com/?next=https://my.key%3Fa=1@evil.com/secret",
            "https://ex.com/?next=https://my.key%3Fa=b@evil.com/path",
            "https://ex.com/?next=https://a.b%3Fc=d@evil.com/secret",
            "https://ex.com/?next=https://my.key%2Fsecret@evil.com%3Fx=1",
            "https://ex.com/?db=postgres://svc.ro%3Fk=v@db.internal:5432/prod",
            // Branch 2 & split_outer_query_chunks: raw/encoded &a=b credentials
            "https://ex.com/?next=https://user:1234&a=b@evil.com/secret",
            "https://ex.com/?next=https%3A%2F%2Fuser%3A1234%26a%3Db%40evil.com%2Fpath",
            "https://ex.com/?next=https%3A%2F%2Fuser%3A1234%26a%3Db%40evil.com%2Fsecret",
            "https://ex.com/#next=https://user:1234&a=b@evil.com/path",
            "https://ex.com/#next=https://user:1234&a=b@evil.com/secret",
            "https://ex.com/?next=https://u&a=b:pass@evil.com/path",
            "https://ex.com/#next=https://u&a=b:pass@evil.com/path",
            "https://ex.com/?next=https%3A%2F%2Fu%26a%3Db%3Apass%40evil.com%2Fpath",
            "https://ex.com/?next=https%3A%2F%2Fuser%26a%3Db%40evil.com%2Fpath",
            "https://ex.com/?next=https%3A%2F%2Ftoken%26a%3Db%40evil.com%2Fpath",
            "https://ex.com/#next=https://user&a=b@evil.com/path",
            "https://ex.com/#next=https://user&a=b@evil.com/secret",
            "https://ex.com/?db=postgres://admin:1234&k=v@db.internal:5432/prod",
            "https://ex.com/?q=(https://user:p&a=b@evil.com/path)",
            "https://ex.com/?q='https://user:p&a=b@evil.com/path'",
            "https://ex.com/?q=https://ok.com,https://user:p&a=b@evil.com/path",
        ];
        for url in benign_whitelist_bypasses {
            assert!(
                link_carries_credential(url),
                "Iter-7 whitelist/&a=b bypass must be rejected: {url}"
            );
            assert_eq!(sanitize_suggestion_link(Some(url)), None);
        }

        // 4. JSON keys, \u0040 escapes & \u003f/\u0023 leaf tails (R1/R2 REV7-02/04, C1/C2 BUG-I7-04)
        let json_bypasses = [
            "https://ex.com/?data={\"https://u:p@evil.com/\":1}",
            "https://ex.com/?data={\"https://user:pass@evil.com/\":1}",
            "https://ex.com/?data=%7B%22https://u:p@evil.com/%22%3A1%7D",
            "https://ex.com/?data=%7B%22https://user:pass@evil.com/%22%3A1%7D",
            "https://ex.com/?data=%7B%22https%3A%2F%2Fu%3Ap%40evil.com%2F%22%3A1%7D",
            "https://ex.com/?data=%7B%22https%3A%2F%2Fuser%3Apass%40evil.com%2F%22%3A1%7D",
            "https://ex.com/?data=%7B%22postgres://admin:hunter2@db.internal:5432/prod%22%3Atrue%7D",
            "https://ex.com/?data=%7B%22//user:pass@evil.com/%22%3A1%7D",
            "https://ex.com/?data=%7B%22nested%22%3A%7B%22https://user:pass@evil.com/%22%3A1%7D%7D",
            "https://ex.com/?data=%7B%22see%20https://user:pass@evil.com/%22%3A1%7D",
            "https://ex.com/?data={\"outer\":{\"https://admin:s3cr3t@db.internal/\":true}}",
            "https://ex.com/?data={\"postgres://admin:hunter2@db.internal:5432/prod\":1}",
            "https://ex.com/?data={\"//user:pass@evil.com/\":1}",
            "https://ex.com/?data={\"https://user:pass@evil.com\":1}",
            "https://ex.com/?data={\"url\":\"https://u:p\\u0040evil.com/\"}",
            "https://ex.com/?data={\"url\":\"https://user:pass\\u0040evil.com/\"}",
            "https://ex.com/?data=%7B%22url%22%3A%22https://u:p%5Cu0040evil.com/%22%7D",
            "https://ex.com/?data=%7B%22url%22%3A%22https://user:pass%5Cu0040evil.com/%22%7D",
            "https://ex.com/?data={\"url\":\"https://ok.com/\\u003fhttps://u:p@evil.com/\"}",
            "https://ex.com/?data={\"url\":\"https://ok.com/\\u0023https://u:p@evil.com/\"}",
            "https://ex.com/?data={\"url\":\"https://ok.com/\\u003fnext=(https://u:p@evil.com/)\"}",
            "https://ex.com/?data={\"note\":\"[link](https://u:p@e.com/)\"}",
        ];
        for url in json_bypasses {
            assert!(
                link_carries_credential(url),
                "Iter-7 JSON bypass must be rejected: {url}"
            );
            assert_eq!(sanitize_suggestion_link(Some(url)), None);
        }

        // 5. HTML containers, %20-prose, wrappers, Markdown/CSS & trailing host delimiters
        let html_prose_wrapper_tail_bypasses = [
            // HTML containers
            "https://ex.com/?html=<p>a,https://u:p@evil.com/</p>",
            "https://ex.com/?html=<p>https://ok.com,https://u:p@e.com/</p>",
            "https://ex.com/?html=<p>see+//u:p@evil.com/</p>",
            "https://ex.com/?html=<p>a+//u:p@e.com/</p>",
            "https://ex.com/?html=<p>a,//u:p@e.com/</p>",
            "https://ex.com/?html=<p>1:https://u:p@e.com/</p>",
            "https://ex.com/?html=<p>foo;https://u:p@e.com/</p>",
            "https://ex.com/?html=<p>foo&https://u:p@e.com/</p>",
            "https://ex.com/?html=<p>https://user:p'w@evil.com/</p>",
            "https://ex.com/?html=%3Cp%3Ea%2Chttps%3A%2F%2Fu%3Ap%40e.com%2F%3C/p%3E",
            "https://ex.com/?html=%3Ca%20href%3D%22https://user:p%22w@evil.com/%22%3Ex%3C/a%3E",
            "https://ex.com/?html=%3Ca%20href%3D%22https://user:p%27w@evil.com/%22%3Ex%3C/a%3E",
            "https://ex.com/?html=%3Ca%20href%3D%22https://user:p%20w@evil.com/%22%3Ex%3C/a%3E",
            "https://ex.com/?html=%3Ca%20href%3D%22https://user%20name:pass@evil.com/%22%3Eclick%3C/a%3E",
            "https://ex.com/?html=%3Ca%20href%3D%22https://user:p%3Cw@evil.com/%22%3Eclick%3C/a%3E",
            "https://ex.com/?html=%3Ca%20href%3D%22https://user:p%3Ew@evil.com/%22%3Eclick%3C/a%3E",
            "https://ex.com/?html=<a%20href=\"https://user:p'w@evil.com/\">x</a>",
            "https://ex.com/?html=<a+href=\"https://user:p'w@evil.com/\">x</a>",
            "https://ex.com/?html=<img+srcset=\"https://ok.com/a.png,https://u:p@e.com/b.png\">",
            "https://ex.com/?q=<https://user:p'w@evil.com/>+text",
            "https://ex.com/?q=<>foo,https://u:p@e.com/",
            "https://ex.com/?q=<https://ok.com+https://u:p@e.com/",
            // %20-prose credentials
            "https://ex.com/?q=see%20https://user:p%20w@evil.com/",
            "https://ex.com/?q=see%20https://user:p%20ass@evil.com/",
            "https://ex.com/?q=see%20https://user:%20pass@evil.com/",
            "https://ex.com/?q=see%20https://user:%20pw@evil.com/",
            "https://ex.com/?q=see%20https://user:1234%20pass@evil.com/",
            "https://ex.com/?q=see%20https://user:1234%20pw@evil.com/",
            "https://ex.com/?q=see%20https://alice.smith%20prod:pass@evil.com/",
            "https://ex.com/?q=see%20https://alice.smith%20jr:pass@evil.com/",
            "https://ex.com/?q=see%20https://john.doe%20admin:pass@evil.com/",
            "https://ex.com/?q=see%20https://user%20a%20b:pass@evil.com/",
            "https://ex.com/?q=see%20https://u%20m%20n:pass@evil.com/",
            "https://ex.com/?q=see%20https://user%20mid%20name:pass@evil.com/",
            "https://ex.com/?q=see%20https://mykey%20secret@evil.com/",
            "https://ex.com/?q=see%20https://token%201@evil.com/",
            "https://ex.com/?q=see%20postgres://admin:p%20w@db.internal:5432/prod",
            "https://ex.com/?q=see%20//user:p%20w@evil.com/",
            // Wrappers, Markdown, CSS, + prefixes & %28/%5B/%3F/%23 before %2F-password
            "https://ex.com/?q=see_this+https://user:p+w@evil.com/",
            "https://ex.com/?q=1+https://user+name:pass@evil.com/",
            "https://ex.com/?q=git+https://user:pass@github.com/org/repo.git",
            "https://ex.com/?q=prefix(https://u:p@e.com/)",
            "https://ex.com/?q=prefix[https://u:p@e.com/]",
            "https://ex.com/?q=prefix'https://u:p@e.com/'",
            "https://ex.com/?q=foo(https://u:p@evil.com/)",
            "https://ex.com/?q=foo[https://user:pass@evil.com/]",
            "https://ex.com/?q=foo{https://user:pass@evil.com/}",
            "https://ex.com/?q=foo'https://user:pass@evil.com/'",
            "https://ex.com/?q=((https://u:p@evil.com/))",
            "https://ex.com/?q=[[https://user:pass@evil.com/]]",
            "https://ex.com/?q=('https://user:pass@evil.com/')",
            "https://ex.com/?q={https://u:p@e.com/}",
            "https://ex.com/?q=(https://u:p@evil.com/).",
            "https://ex.com/?q=[https://u:p@evil.com/].",
            "https://ex.com/?q='https://u:p@evil.com/'.",
            "https://ex.com/?q=[click](https://user:pass@evil.com/)",
            "https://ex.com/?text=check%20[link](https://user:pass@evil.com/)",
            "https://ex.com/?q=url(https://u:p@e.com/)",
            "https://ex.com/?q=see(https://user:pass@evil.com/)",
            "https://ex.com/?q=(see%20https://user:pass@evil.com)",
            "https://ex.com/a(https://u:p@e.com/)",
            "https://ex.com/?next=%28https://user:p%2Fw@evil.com/%29",
            "https://ex.com/?next=%5Bhttps://user:p%2Fw@evil.com/%5D",
            "https://ex.com/a%3Fhttps://user:p%2Fw@evil.com/",
            "https://ex.com/a%23https://user:p%2Fw@evil.com/",
            "https://ex.com/?next=https://ok.com%3Fhttps://user:p%2Fw@evil.com/",
            "https://ex.com/?next=https://ok.com%23https://user:p%2Fw@evil.com/",
            // Trailing host delimiters & %2F-password host fallbacks
            "https://ex.com/?next=https://user:pass@evil.com&debug",
            "https://ex.com/?next=https://user:pass@evil.com&flag&a=1",
            "https://ex.com/?next=http://admin:hunter2@10.0.0.1&debug",
            "https://ex.com/?db=postgres://admin:hunter2@db.internal:5432&ssl",
            "https://ex.com/?db=postgres://admin:secret@db.internal&flag",
            "https://ex.com/?next=//user:pass@evil.com&debug",
            "https://ex.com/?next=https://user:pass@evil.com;foo",
            "https://ex.com/?db=postgres://admin:hunter2@db.internal;foo",
            "https://ex.com/?db=postgres://admin:secret@db.internal;foo",
            "https://ex.com/?postgres://admin:secret@db.internal=1",
            "https://ex.com/?next=https://user:pass@evil.com%26debug",
            "https://ex.com/?next=https://user:pass@evil.com%3Bfoo",
            "https://ex.com/?next=https://user:pass@evil.com%29",
            "https://ex.com/?next=https://user:pass@evil.com%27",
            "https://ex.com/?next=https://user:pass@evil.com%5D",
            "https://ex.com/?q=https://user:p%2Fw@evil.com%2Chttps://ok.com",
            "https://ex.com/?q=https://user:p%2Fw@evil.com%29",
            "https://ex.com/?q=https://user:p%2Fw@evil.com%27",
            "https://ex.com/?q=https://user:p%2Fw@evil.com%5D",
            "https://ex.com/?q=https://user:p%2Fw@evil.com%26x=1",
            "https://ex.com/?q=https://user:p%2Fw@evil.com%3Bx=1",
            "https://ex.com/?q=https://user:p%2Fw@evil.com%2Bmore",
            "https://ex.com/?q=https://user:p%2Fw@evil.com)",
        ];
        for url in html_prose_wrapper_tail_bypasses {
            assert!(
                link_carries_credential(url),
                "Iter-7 HTML/prose/wrapper/tail bypass must be rejected: {url}"
            );
            assert_eq!(sanitize_suggestion_link(Some(url)), None);
        }

        // 6. Bare-origin `://` secret param keys (C1 BUG-I7-05, C2 BUG-I7-06(4))
        let bare_origin_secret_keys = [
            "https://ex.com/?foo=https://e.com+sig=deadbeef",
            "https://ex.com/?foo=https://e.com+session=xyz",
            "https://ex.com/?foo=https://e.com+auth=secret",
            "https://ex.com/?foo=https://e.com%20sig=deadbeef",
            "https://ex.com/?foo=https://e.com%20access%20key=xyz",
            "https://ex.com/?q=https://ok.com+sig=deadbeef",
            "https://ex.com/?q=https://ok.com+signature=deadbeef",
            "https://ex.com/?q=https://ok.com+session=xyz",
            "https://ex.com/?q=https://ok.com+auth=1",
            "https://ex.com/?q=https://ok.com+access_key=xyz",
            "https://ex.com/?q=https://ok.com+apiKey=xyz",
            "https://ex.com/?q=https://ok.com%20sig=deadbeef",
            "https://ex.com/?q=https://ok.com%20session=xyz",
            "https://ex.com/?q=https://ok.com%20auth=secret",
        ];
        for url in bare_origin_secret_keys {
            assert!(
                link_carries_credential(url),
                "Iter-7 bare-origin secret key must be rejected: {url}"
            );
            assert_eq!(sanitize_suggestion_link(Some(url)), None);
        }

        // 7. All Iteration-7 benign false-positive fixes MUST return false & preserve verbatim
        let benign_iter7 = [
            // `+`-joined search queries (C1 BUG-I7-06, R2 REV7-04B)
            "https://github.com/search?q=from+https://example.com+alice@example.com",
            "https://www.google.com/search?q=see+https://docs.rs+alice@example.com",
            "https://github.com/search?q=check+https://npmjs.com+@types/node",
            "https://ex.com/?q=from+https://ok.com+contact+alice@example.com",
            "https://ex.com/?q=ref+http://localhost:3000+alice@example.com",
            "https://ex.com/?q=check+http://localhost:3000+alice@example.com",
            "https://ex.com/?q=see+https://example.com+author:alice@example.com",
            "https://ex.com/?q=see+https://ok.com+@alice",
            "https://ex.com/?q=see+https://example.com,alice@example.com",
            "https://ex.com/?q=see+https://example.com%20alice@example.com",
            "https://ex.com/?q=see+https://docs.rs%20alice@example.com",
            "https://ex.com/?q=see+http://[::1]:3000+alice@example.com",
            // Dotted hosts with explicit standard `:port` (R1 REV7-03, R2 REV7-04B)
            "https://ex.com/?q=https://example.com:8443+alice@example.com",
            "https://ex.com/?q=https://docs.rs:443%20alice@example.com",
            "https://ex.com/?next=https://mastodon.social:443%2Fusers%2F@alice",
            "https://ex.com/?next=https://example.com:443%3Femail=alice@example.com",
            "https://ex.com/?next=https://ok.com:443%20alice@example.com",
            "https://ex.com/?next=https://ok.com:443+alice@example.com",
            "https://ex.com/?next=https://ok.com:443,alice@example.com",
            "https://ex.com/?next=https://ok.com:443%2Fusers%2F@alice",
            // Qualifier & localhost prose + benign JSON key + bare-origin non-secret param
            "https://ex.com/?q=https://docs.rs%20contact:%20alice@example.com",
            "https://ex.com/?q=see%20http://localhost%20mailto:alice@example.com",
            "https://ex.com/?data={\"https://ok.com\":\"alice@example.com\"}",
            "https://ex.com/?data={\"url\":\"https://ok.com/docs\",\"email\":\"alice@example.com\"}",
            "https://ex.com/?q=https://auth.example.com+x=1",
            "https://ex.com/?q=see%20(https://docs.rs)%20alice@example.com",
        ];
        for url in benign_iter7 {
            assert!(
                !link_carries_credential(url),
                "Iter-7 benign URL must NOT be flagged: {url}"
            );
            assert_eq!(sanitize_suggestion_link(Some(url)).as_deref(), Some(url));
        }
    }

    #[test]
    fn la_16_iter8_remediation_regressions() {
        use std::time::{Duration, Instant};

        // 1. CRITICAL REV8-01 O(N^2) `+a` bombs & 0-slash/colon bombs MUST all be strictly O(N) (< 250ms debug).
        let plus_a_query_32kb = format!("https://ex.com/?q={}@", "+a".repeat(16_000));
        let plus_a_query_pct8_16kb = format!(
            "https://ex.com/?q={}@%2525252525252520",
            "+a".repeat(8_000)
        );
        let plus_a_path_32kb = format!("https://ex.com/{}@", "+a".repeat(16_000));
        let plus_a_path_pct8_16kb = format!(
            "https://ex.com/{}@%2525252525252520",
            "+a".repeat(8_000)
        );
        let plus_a_html_32kb = format!("https://ex.com/?html=<p>{}@</p>", "+a".repeat(16_000));
        let plus_a_json_32kb = format!(
            "https://ex.com/?data={{\"k\":\"{}@\"}}",
            "+a".repeat(16_000)
        );
        let paren_0slash_nonhost = format!("https://ex.com/?q={}@", "(https:user:p)".repeat(4_000));
        let zero_slash_colon_chain = format!("https://ex.com/?q={}@", "https:a:".repeat(4_000));
        let non_scheme_colons = format!("https://ex.com/?q={}@", "a:b:c&".repeat(8_000));
        for (name, payload) in [
            ("plus_a_query_32kb", &plus_a_query_32kb),
            ("plus_a_query_pct8_16kb", &plus_a_query_pct8_16kb),
            ("plus_a_path_32kb", &plus_a_path_32kb),
            ("plus_a_path_pct8_16kb", &plus_a_path_pct8_16kb),
            ("plus_a_html_32kb", &plus_a_html_32kb),
            ("plus_a_json_32kb", &plus_a_json_32kb),
            ("paren_0slash_nonhost", &paren_0slash_nonhost),
            ("zero_slash_colon_chain", &zero_slash_colon_chain),
            ("non_scheme_colons", &non_scheme_colons),
        ] {
            let t = Instant::now();
            assert!(!link_carries_credential(payload), "{name} should be benign");
            let d = t.elapsed();
            assert!(
                d < Duration::from_millis(250),
                "{name} exceeded 250ms in debug: {d:?}"
            );
        }

        // 2. R1 REV8-02, R2 REV8-03, C1 BUG-I8-01, C2 BUG-I8-03: %20-prose, Pass-1 ws & HTML bypasses
        let prose_and_html_bypasses = [
            "https://ex.com/?q=see_1+https://user:p%20w@evil.com/",
            "https://ex.com/?q=1+https://user:p%20w@evil.com/",
            "https://ex.com/?q=foo,https://user:p%20w@evil.com/",
            "https://ex.com/?q=foo(https://user:p%20w@evil.com/)",
            "https://ex.com/?q=see%20[click](https://user:p%20w@evil.com/)",
            "https://ex.com/?q=check%20[link](https://user:p%20w@evil.com/)",
            "https://ex.com/?q=see%20[link](https://user:p%20w@evil.com/)",
            "https://ex.com/?q=see%20url(https://user:p%20w@evil.com/)",
            "https://ex.com/?q=see%20href=https://user:p%20w@evil.com/",
            "https://ex.com/?q=see%20foo,https://user:p%20w@evil.com/",
            "https://ex.com/?q=see%20foo+https://user:p%20w@evil.com/",
            "https://ex.com/?q=see%20a,https://user:p%20w@evil.com/",
            "https://ex.com/?q=see%20a+https://user:p%20w@evil.com/",
            "https://ex.com/?q=see%201:https://user:p%20w@evil.com/",
            "https://ex.com/?q=see%20foo:https://user:p%20w@evil.com/",
            "https://ex.com/?q=see%20go:https://user:p%20w@evil.com/",
            "https://ex.com/?q=see%20go(https://user:p%20w@evil.com/)",
            "https://ex.com/?q=see%20https://ok.com,https://user:p%20w@evil.com/",
            "https://ex.com/?q=1:bar/https://user:p%20w@evil.com/",
            "https://ex.com/?q=see%20https:user:p%20w@evil.com/",
            "https://ex.com/?q=see%20http:user:p%20w@evil.com/",
            "https://ex.com/?q=see%20ws:user:p%20w@evil.com/",
            "https://ex.com/?q=see%20%28https://user:p%20w@evil.com/%29",
            "https://ex.com/?q=see%20%2F%2Fuser:p%20w@evil.com/",
            "https://ex.com/?q=see%20https%3A%2F%2Fuser%3Ap%20w%40evil.com%2F",
            "https://ex.com/?q=see%20https%3A%2F%2Fuser%20name%3Apass%40evil.com%2F",
            "https://ex.com/?q=see%20postgres%3A%2F%2Fadmin%3Ap%20w%40db.internal%3A5432%2Fprod",
            "https://ex.com/?q=see+https://user:p%20w%40evil.com/",
            "https://ex.com/?q=see+https%3A%2F%2Fuser%3Ap%20w%40evil.com%2F",
            "https://ex.com/?q=[click](https://user:p%20w%40evil.com/)",
            "https://ex.com/?q=foo,https://user:p%20w%40evil.com/",
            "https://ex.com/?html=<a%20href=%22https://user:p%20w@evil.com/%22>click</a>",
            "https://ex.com/?html=<a+href=%22https://user:p%20w@evil.com/%22>click</a>",
            "https://ex.com/?html=<a%20href=%27https://user:p%20w@evil.com/%27>click</a>",
            "https://ex.com/?html=<a%20href=https://user:p%20w@evil.com/>x</a>",
            "https://ex.com/?html=<p>foo,https://user:p%20w@evil.com/</p>",
            "https://ex.com/?html=<p>a,https://user:p%20w@evil.com/</p>",
            "https://ex.com/?html=<p>see_1+https://user:p%20w@evil.com/</p>",
            "https://ex.com/?html=<p>see%20[link](https://user:p%20w@evil.com/)</p>",
            "https://ex.com/?html=%3Ca%20href%3D%22https%3A%2F%2Fuser%3Ap%20w%40evil.com%2F%22%3Ex%3C/a%3E",
            "https://ex.com/?html=%3Ca%20href%3D%22https%3A%2F%2Fuser%20name%3Apass%40evil.com%2F%22%3Eclick%3C/a%3E",
            "https://ex.com/?html=%3Cp%3Ehttps%3A%2F%2Fuser%3Ap%20w%40evil.com%2F%3C/p%3E",
            "https://ex.com/?html=%3Cp%3Esee%20https%3A%2F%2Fuser%3Ap%20w%40evil.com%2F%3C/p%3E",
            "https://ex.com/?html=<a+href+=+\"https://user:p%20w@evil.com/\">x</a>",
            "https://ex.com/?html=<a+href+=+\"https://user:p+w@evil.com/\">x</a>",
            "https://ex.com/?html=%3Ca%20href%3D%22https://user:p%22+w@evil.com/%22%3Ex%3C/a%3E",
            "https://ex.com/?html=%3Ca%20href%3D%22https://user:p%22%20w@evil.com/%22%3Ex%3C/a%3E",
            "https://ex.com/?html=%3Ca%20href%3D%22https://user:p%22%3Ew@evil.com/%22%3Ex%3C/a%3E",
            "https://ex.com/?html=%3Ca%20href%3D%27https://user:p%27+w@evil.com/%27%3Ex%3C/a%3E",
            "https://ex.com/?html=<a+href=\"https://user:p%20w%40evil.com/\">x</a>",
            "https://ex.com/?html=<a+href=\"https://user%20name:pass%40evil.com/\">x</a>",
            "https://ex.com/?html=<img+src=\"https://user:p%20w%40evil.com/a.png\">",
            "https://ex.com/?html=<p>https://user:p%20w%40evil.com/</p>",
            "https://ex.com/?html=<https://user:p%20w%40evil.com/>+text",
            "https://ex.com/?html=hi%20%3Ca%20href%3D%22https://user:p%20w@evil.com/%22%3Ex%3C/a%3E",
            "https://ex.com/?html=hi%20<a%20href=\"https://user:p%20w@evil.com/\">x</a>",
        ];
        for url in prose_and_html_bypasses {
            assert!(
                link_carries_credential(url),
                "Iter-8 prose/HTML bypass must be rejected: {url}"
            );
            assert_eq!(sanitize_suggestion_link(Some(url)), None);
        }

        // 3. C1 BUG-I8-02, C2 BUG-I8-01/02, R1 REV8-04(1), R2 REV8-04A/B/C:
        //    Markdown, HTML comments, pipes, brackets, host tails, wrappers, backslash & rfind('@')
        let delim_wrapper_and_tail_bypasses = [
            "https://ex.com/?urls=https://ok.com|https://u:p@evil.com/",
            "https://ex.com/?urls=https://ok.com/a|https://u:p@evil.com/",
            "https://ex.com/?urls=foo|https://u:p@evil.com/",
            "https://ex.com/?urls=foo%7Chttps://u:p@evil.com/",
            "https://ex.com/?urls=https://ok.com|//u:p@evil.com/",
            "https://ex.com/?urls=https://u:p@evil.com|https://ok.com",
            "https://ex.com/?q=|https://u:p@evil.com/|",
            "https://ex.com/?q=`https://user:pass@evil.com/`",
            "https://ex.com/?q=**https://user:pass@evil.com/**",
            "https://ex.com/?q=*https://user:pass@evil.com/*",
            "https://ex.com/?q=_https://user:pass@evil.com/_",
            "https://ex.com/?q=~~https://user:pass@evil.com/~~",
            "https://ex.com/?html=<!--https://user:pass@evil.com/-->",
            "https://ex.com/?q=[1]https://u:p@evil.com/",
            "https://ex.com/?q=(note)https://u:p@evil.com/",
            "https://ex.com/?q={x}https://user:pass@evil.com/",
            "https://ex.com/?q=!https://user:pass@evil.com/",
            "https://ex.com/?q=%60https://user:pass@evil.com/%60",
            "https://ex.com/?q=a%7Chttps://user:pass@evil.com/",
            "https://ex.com/?q=https://user:p%2Fw@evil.com%7Cfoo",
            "https://ex.com/?q=`//user:pass@evil.com/`",
            "https://ex.com/?q=**//user:pass@evil.com/**",
            "https://ex.com/?q=|//user:pass@evil.com/|",
            "https://ex.com/?html=<!--//user:pass@evil.com/-->",
            "https://ex.com/?next=https://user:pass@evil.com(note)",
            "https://ex.com/?next=https://user:pass@evil.com[1]",
            "https://ex.com/?next=https://user:pass@evil.com{a}",
            "https://ex.com/?next=https://user:pass@evil.com%28note%29",
            "https://ex.com/?next=https://user:pass@evil.com%5B1%5D",
            "https://ex.com/?next=https://user:pass@evil.com%7Cfoo",
            "https://ex.com/?next=https://user:pass@evil.com|foo",
            "https://ex.com/?next=https://user:pass@evil.com^foo",
            "https://ex.com/a%C2%A0https://user:p%2Fw@evil.com/",
            "https://ex.com/?q=[link](https://user:p).w@evil.com/)",
            "https://ex.com/?q=[link](https://user:p)+w@evil.com/)",
            "https://ex.com/?q=[link](https://user:p),w@evil.com/)",
            "https://ex.com/?q=[link](https://user:p);w@evil.com/)",
            "https://ex.com/?q=[link](https://user:p)&w@evil.com/)",
            "https://ex.com/?q=[link](https://user:p))w@evil.com/)",
            "https://ex.com/?q=[link](https://user:p)'w@evil.com/)",
            "https://ex.com/?q=[link](https://user:p)%20w@evil.com/)",
            "https://ex.com/?q=foo'https://user:p'.w@evil.com/'",
            "https://ex.com/?q=foo'https://user:p'+w@evil.com/'",
            "https://ex.com/?q=((https://user:p).w@evil.com/)",
            "https://ex.com/?q=see%20(https://user:p)%20w@evil.com/)",
            "https://ex.com/?q=see%20'https://user:p'%20w@evil.com/'",
            "https://ex.com/?q=(https://ok.com;https:\\\\u:p@evil.com/)",
            "https://ex.com/?q=a:https://ok.com;https:\\\\u:p@evil.com/",
            "https://ex.com/?q=(https://ok.com;https:\\u:p@evil.com/)",
            "https://ex.com/?q=(https://ok.com;http:\\\\admin:pass@evil.com/)",
            "https://ex.com/?q=https://user:p%2Fw@evil.com%2Chttps://ok.com/@alice",
            "https://ex.com/?q=https://user:p%2Fw@evil.com%29%20https://ok.com/@alice",
            "https://ex.com/?q=https://user:p%2Fw@evil.com%26next=https://ok.com/@alice",
            "https://ex.com/?db=postgres://admin:p%2Fw@db.internal:5432%26next=https://ok.com/@alice",
            "https://ex.com/?q=https://user:p%2Fw@evil.com%3Bhttps://ok.com/@alice",
            "https://ex.com/?q=https://user:p%2Fw@evil.com%2Bhttps://ok.com/@alice",
            "https://ex.com/?q=https://user:p%2Fw@evil.com)https://ok.com/@alice",
            "https://ex.com/?q=https://user:p%2Fw@evil.com%29https://ok.com/@alice",
            "https://ex.com/?q=https://user:p%2Fw@evil.com%20https://ok.com/@alice",
            "https://ex.com/?q=https://user:p%2Fw@evil.com%5Dhttps://ok.com/@alice",
            "https://ex.com/?q=https://user:p%2Fw@evil.com%27https://ok.com/@alice",
            "https://ex.com/?db=postgres://admin:p%2Fw@db.internal:5432%2Chttps://ok.com/@alice",
        ];
        for url in delim_wrapper_and_tail_bypasses {
            assert!(
                link_carries_credential(url),
                "Iter-8 delim/wrapper/tail/backslash bypass must be rejected: {url}"
            );
            assert_eq!(sanitize_suggestion_link(Some(url)), None);
        }

        // 4. R1 REV8-03, R2 REV8-02, C1 BUG-I8-03, C2 BUG-I8-04: JSON shattering, string literals,
        //    nested JSON, \uXXXX secret tokens/matrix/query & mid-password "/</>/"\u0020
        let json_and_unicode_bypasses = [
            "https://ex.com/?data={\"ok\":\"https://ok.com/?a=1\",\"bad\":\"https://u:p\\u0040evil.com/\"}",
            "https://ex.com/?data=%7B%22ok%22%3A%22https://ok.com/?a=1%22%2C%22bad%22%3A%22https://u:p%5Cu0040evil.com/%22%7D",
            "https://ex.com/?data=[\"https://ok.com/?a=1\",\"https://u:p\\u0040evil.com/\"]",
            "https://ex.com/?data=%5B%22https://ok.com/?a=1%22%2C%22https://u:p%5Cu0040evil.com/%22%5D",
            "https://ex.com/?data={\"url\":\"https://ok.com/?\\u0074oken=abc\"}",
            "https://ex.com/?data=%7B%22url%22%3A%22https://ok.com/?%5Cu0074oken=abc%22%7D",
            "https://ex.com/?data={\"?\":1,\"https://u:p\\u0040evil.com/\":2}",
            "https://ex.com/?data=%7B%22?%22%3A1%2C%22https://u:p%5Cu0040evil.com/%22%3A2%7D",
            "https://ex.com/?data=\"https://u:p\\u0040evil.com/\"",
            "https://ex.com/?data=%22https://u:p%5Cu0040evil.com/%22",
            "https://ex.com/?data={\"url\":\"https://ok.com/\\u003fdata=%257B%2522u%2522%253A%2522https://u:p%255Cu0040evil.com/%2522%257D\"}",
            "https://ex.com/?data={\"url\":\"https://u:p\\u0040evil.com/?x=1\"}",
            "https://ex.com/?data={\"a\":\"?\",\"url\":\"https://u:p\\u0040evil.com/\"}",
            "https://ex.com/?data={\"a\":\"?\",\"https://u:p\\u0040evil.com/\":1}",
            "https://ex.com/?data={\"a\":\"?\",\"url\":\"https://user:p\\\"w@evil.com/\"}",
            "https://ex.com/?data={\"a\":\"?\",\"url\":\"https://user:p>w@evil.com/\"}",
            "https://ex.com/?data={\"a\":\"?\",\"url\":\"https://user:p<w@evil.com/\"}",
            "https://ex.com/?data=[\"?\",\"https://u:p\\u0040evil.com/\"]",
            "https://ex.com/?data=%7B%22a%22%3A%22?%22%2C%22url%22%3A%22https://u:p%5Cu0040evil.com/%22%7D",
            "https://ex.com/?data={\"url\":\"https://ok.com/?sig\\u003ddeadbeef\"}",
            "https://ex.com/#data={\"a\":\"?\",\"url\":\"https://u:p\\u0040evil.com/\"}",
            "https://ex.com/#data={\"a\":\"x&b=1\",\"url\":\"https://u:p\\u0040evil.com/\"}",
            "https://ex.com/?data={\"k\":\"sk\\u002dant-1234567890\"}",
            "https://ex.com/?data={\"k\":\"ghp\\u005f12345678901234567890\"}",
            "https://ex.com/?data={\"k\":\"Bearer\\u0020mysecrettoken12345\"}",
            "https://ex.com/?data={\"url\":\"https://e.com/api\\u003bsession\\u003dxyz\"}",
            "https://ex.com/?data={\"inner\":\"https://ok.com/?d={\\\"url\\\":\\\"https://u:p\\\\u0040evil.com/\\\"}\"}",
            "https://ex.com/?data=%7B%22note%22%3A%22see%20https%3A%2F%2Fuser%3Ap%20w%40evil.com%2F%22%7D",
            "https://ex.com/?data={\"note\":\"see\\u0020https://user:p\\u0020w@evil.com/\"}",
            "https://ex.com/?data={\"note\":\"see%20https://user:p\\\"w@evil.com/\"}",
            "https://ex.com/?data={\"note\":\"see%20https://user:p<w@evil.com/\"}",
            "https://ex.com/?data={\"note\":\"see%20https://user:p>w@evil.com/\"}",
            "https://ex.com/?data={\"a\":\"1&b=2\",\"url\":\"https://user:pass\\u0040evil.com/\"}",
            "https://ex.com/?data={\"url\":\"https://user:pass\\u0040evil.com/?x=1\"}",
            "https://ex.com/?data={\"url\":\"https://user:pass\\u0040evil.com/#top\"}",
            "https://ex.com/?data=%7B%22url%22%3A%22https://user:pass%5Cu0040evil.com/?x=1%22%7D",
            "https://ex.com/?data=%7B%22a%22%3A%221&b=2%22%2C%22url%22%3A%22https://user:pass%5Cu0040evil.com/%22%7D",
            "https://ex.com/?data={\"url\":\"https://ok.com/api;token\\u003dsecret123\"}",
            "https://ex.com/?data={\"v\":\"gh\\u0070_AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA\"}",
        ];
        for url in json_and_unicode_bypasses {
            assert!(
                link_carries_credential(url),
                "Iter-8 JSON/unicode bypass must be rejected: {url}"
            );
            assert_eq!(sanitize_suggestion_link(Some(url)), None);
        }

        // 5. R1 REV8-04(2), R2 NB, C1 BUG-I8-04, C2 BUG-I8-05: Residual Branch 1/2/3 & &a=b bypasses
        let branch_and_amp_bypasses = [
            "https://ex.com/?next=https://my_user:1234%2F@db",
            "https://ex.com/?next=https://svc_admin:9999%2F@db-internal",
            "https://ex.com/?next=https://my_user:1234%3Fa=1@evil.com",
            "https://ex.com/?next=http://my_user:1234%3Fa=1@evil.com",
            "https://ex.com/?next=https://localhost:1234%2F@evil",
            "https://ex.com/?next=https://10.0.0.1:1234%2F@evil",
            "https://ex.com/?next=https://ok.com%2Fu%2F@alice+tok@evil.com/admin",
            "https://ex.com/?next=https://ok.com%2Fu%2F@alice,tok@evil.com/admin",
            "https://ex.com/?next=https://ok.com%2Fu%2F@alice%20tok@evil.com/admin",
            "https://ex.com/?next=https://a.b:443%2F@x+user:hunter2@evil.com",
            "https://ex.com/?next=https://localhost:1234%2F@x+user:hunter2@evil.com",
            "https://ex.com/?next=https://a.b%2F@x+p!w@evil.com",
            "https://ex.com/?next=https://a.b%2Ffoo%2F@x+token@evil.com/secret",
            "https://ex.com/?next=https://a.b:443%3Femail=x@y.com:secret@evil.com",
            "https://ex.com/?next=https://localhost:1234%3Fk=v@x:secret@evil.com",
            "https://ex.com/?next=https://a.b:443%3Fk=v=@intranet/secret",
            "https://ex.com/?next=https%3A%2F%2Flocalhost%3A1234%26a%3Db%40evil.com%2Fadmin",
            "https://ex.com/?next=https%3A%2F%2Flocalhost%3A1234%26a%3Db%40evil.com%2Fsecret",
            "https://ex.com/#next=https://example.com:443&a=b@evil.com/secret",
            "https://ex.com/?next=http%3A%2F%2Fadmin%3A1234%26a%3Db%4010.0.0.1",
            "https://ex.com/?next=http%3A%2F%2Fadmin%3A1234%26a%3Db%40evil.com",
            "https://ex.com/?next=https%3A%2F%2Falice.smith%26a%3Db%4010.0.0.1%2Fsecret",
            "https://ex.com/?next=https%3A%2F%2Falice.smith%26a%3Dfrom%3A1%26b%3Dc%3Ahunter2%40evil.com%2Fsecret",
            "https://ex.com/?next=//alice.smith&a=b:pass@evil.com/path",
            "https://ex.com/?next=%2F%2Falice.smith%26a%3Db%3Apass%40evil.com%2Fpath",
            "https://ex.com/?next=//alice.smith&a=b@evil.com/admin",
            "https://ex.com/?q=foo,https:user:p&a=b@evil.com/path",
            "https://ex.com/?q=see+https:user:p&a=b@evil.com/path",
            "https://ex.com/?q=foo;https:user:p&a=b@evil.com/path",
            "https://ex.com/?next=https://alice.smith&a=to:pass@evil.com/path",
            "https://ex.com/?next=https%3A%2F%2Falice.smith%26a%3Dto%3Apass%40evil.com%2Fpath",
            "https://ex.com/?next=https%3A%2F%2Falice.smith%3A443%26a%3Db%40evil.com%2Fadmin",
            "https://ex.com/?next=https%3A%2F%2Fapi.token%20secret%40evil.com%2Fadmin",
            "https://ex.com/?next=http://localhost:3000+p!w@evil.com/admin/keys",
            "https://ex.com/?next=http://localhost:3000+p$w:hunter2@evil.com/admin",
            "https://ex.com/?next=https://alice.smith:%20p!w:secret@evil.com",
        ];
        for url in branch_and_amp_bypasses {
            assert!(
                link_carries_credential(url),
                "Iter-8 Branch 1/2/3 or &a=b bypass must be rejected: {url}"
            );
            assert_eq!(sanitize_suggestion_link(Some(url)), None);
        }

        // 6. R1 REV8-04(3), C1 BUG-I8-05, C2 BUG-I8-06: Generalized bare-origin secret-key bypasses
        let bare_origin_secret_keys_iter8 = [
            "https://ex.com/?q=https://ok.com\\sig=deadbeef",
            "https://ex.com/?q=https://ok.com%5Csig=deadbeef",
            "https://ex.com/?q=https://ok.com(sig=deadbeef)",
            "https://ex.com/?q=https://ok.com%28sig=deadbeef%29",
            "https://ex.com/?q=https://ok.com[sig=deadbeef]",
            "https://ex.com/?q=https://ok.com:sig=deadbeef",
            "https://ex.com/?foo=https://e.com%C2%A0sig=deadbeef",
            "https://ex.com/?foo=https://e.com%E3%80%80session=xyz",
            "https://ex.com/?foo=https://e.com)sig=deadbeef",
            "https://ex.com/?foo=https://e.com|sig=deadbeef",
            "https://ex.com/?foo=https://e.com:sig=deadbeef",
            "https://ex.com/?q=(https://ok.com)sig=deadbeef",
            "https://ex.com/?q=[https://ok.com]sig=deadbeef",
            "https://ex.com/?q=https://ok.com|sig=deadbeef",
            "https://ex.com/?q=\"https://ok.com\"sig=deadbeef",
            "https://ex.com/?q=`https://ok.com`auth=secret",
        ];
        for url in bare_origin_secret_keys_iter8 {
            assert!(
                link_carries_credential(url),
                "Iter-8 bare-origin secret key must be rejected: {url}"
            );
            assert_eq!(sanitize_suggestion_link(Some(url)), None);
        }

        // 7. All Iteration-8 benign false-positive fixes MUST return false & preserve verbatim
        let benign_iter8 = [
            // C2 BUG-I8-04 JSON FPs
            "https://ex.com/?data={\"url\":\"https://ex.com/%3Fnext=https://ok.com%26contact=alice@example.com\"}",
            "https://ex.com/?data={\"url\":\"https://www.google.com/search%3Fq=https://docs.rs+alice@example.com\"}",
            // C1 BUG-I8-05 email: qualifier FP
            "https://ex.com/?q=https://example.com+email:alice@example.com",
            // R2 NB 3 benign search FPs
            "https://github.com/search?q=see+https://docs.rs+repo+author:alice@example.com",
            "https://ex.com/?q=https://example.com+contact+author:alice@example.com",
            "https://ex.com/?q=https://docs.rs:443+alice@example.com/profile",
            // Benign JSON with \u0040 in email leaf & bare-origin non-secret guards
            "https://ex.com/?data={\"url\":\"https://ok.com/docs\",\"email\":\"alice\\u0040example.com\"}",
            "https://ex.com/?q=https://github.com/acme/token-service+x=1",
            "https://ex.com/?foo=auth-service%3Fx=1",
            "https://ex.com/?q=https://auth.example.com+x=1",
            "https://github.com/acme/token-service/pull/55",
        ];
        for url in benign_iter8 {
            assert!(
                !link_carries_credential(url),
                "Iter-8 benign URL must NOT be flagged: {url}"
            );
            assert_eq!(sanitize_suggestion_link(Some(url)).as_deref(), Some(url));
        }
    }

    #[test]
    fn la_17_iter9_remediation_regressions() {
        use std::time::{Duration, Instant};

        // 1. CRITICAL DoS Bomb Family 1: `closed_by_wrapper` 0-slash bombs (R1/R2 REV9-01, C1 BUG-I9-02, C2 BUG-I9-01)
        let dos1_paren_56kb = format!("https://ex.com/?q={}@)", "(https:user:p)".repeat(4_000));
        let dos1_squote_16kb = format!("https://ex.com/?q={}@!'", "'https:a".repeat(2_000));
        let dos1_squote_pct8_9kb = format!(
            "https://ex.com/?q={}@!'%2525252525252520",
            "'https:a".repeat(1_000)
        );
        let dos1_dquote_36kb = format!("https://ex.com/?q={}@\"", "\"https:a:".repeat(4_000));
        let dos1_paren_bal_40kb = format!(
            "https://ex.com/?q={}@{}",
            "(https:a:".repeat(4_000),
            ")".repeat(4_000)
        );
        let dos1_paren_multi_at = format!(
            "https://ex.com/?q={}{}",
            "(https:a@!".repeat(2_000),
            ")".repeat(2_000)
        );
        let dos1_paren_pct8_14kb = format!(
            "https://ex.com/?q={}@)%2525252525252520",
            "(https:user:p)".repeat(1_000)
        );

        // 2. CRITICAL DoS Bomb Family 2: `authority_or_wrapped_span_end` peek / split_once / non-port (R1 REV9-02, R2 REV9-03, C1 BUG-I9-03, C2 BUG-I9-02)
        let dos2_sp_slash_at = format!("https://ex.com/?q={}/@", "https:ok.com%20".repeat(4_000));
        let dos2_sp_slash_pct8 = format!(
            "https://ex.com/?q={}/@%2525252525252520",
            "https:a%20".repeat(4_000)
        );
        let dos2_sp_author = format!(
            "https://ex.com/?q={}@",
            "https:ok.com%20author:a%20".repeat(4_000)
        );
        let dos2_sp_author_pct8 = format!(
            "https://ex.com/?q={}@%2525252525252520",
            "https:ok.com%20author:".repeat(2_000)
        );
        let dos2_sp_long_qual = format!(
            "https://ex.com/?q={}{}:x@",
            "https:ok.com%20".repeat(2_000),
            "a".repeat(16_000)
        );
        let dos2_squote_bang_64kb = format!("https://ex.com/?q='https://u!{}@", "'".repeat(64_000));
        let dos2_rparen_bang_64kb = format!("https://ex.com/?q=(https://u!{}@", ")".repeat(64_000));
        let dos2_dquote_nonport_32kb = format!(
            "https://ex.com/?q=https://{}:{}@%2525252525252520",
            "a".repeat(16_000),
            "\"".repeat(16_000)
        );

        // 3. CRITICAL DoS Bomb Family 3: Nested-JSON 4-deep & 2-branch amplification + unbalanced `{` (R1 REV9-03, R2 REV9-02, C1 BUG-I9-01, C2 BUG-I9-04)
        let wrap_json_depth = |inner: String, depth: usize| -> String {
            let mut cur = inner;
            for _ in 0..depth {
                let mut map = serde_json::Map::new();
                map.insert("k".to_string(), serde_json::Value::String(cur));
                cur = serde_json::Value::Object(map).to_string();
            }
            cur
        };
        let mut pct45 = String::from("%");
        for _ in 0..45 {
            pct45.push_str("25");
        }
        pct45.push_str("20");
        let dos3_bug_i9_01 = format!(
            "https://ex.com/?data={}",
            wrap_json_depth(format!("{}@{pct45}", "a".repeat(100)), 5)
        );
        let dos3_rev9_03 = format!(
            "https://ex.com/?data={}",
            wrap_json_depth(format!("{}@%2525252525252520", "z".repeat(500)), 5)
        );
        let dos3_nest4_1279b = format!(
            "https://ex.com/?data={}",
            wrap_json_depth(format!("{}@%2525252525252520", "a".repeat(1_000)), 5)
        );
        let mut two_br = format!("{}@%2525252525252520", "a".repeat(32));
        for _ in 0..4 {
            let mut map = serde_json::Map::new();
            map.insert("a".to_string(), serde_json::Value::String(two_br.clone()));
            map.insert("b".to_string(), serde_json::Value::String(two_br));
            two_br = serde_json::Value::Object(map).to_string();
        }
        let dos3_two_branch_d4 = format!("https://ex.com/?data={two_br}");
        let dos3_adv_pct7b_d4 = format!(
            "https://ex.com/?data={}",
            wrap_json_depth(
                "%2525257B%25257B%257B%7B@%2525252525252520".to_string(),
                5
            )
        );
        let dos3_unbalanced_braces = format!("https://ex.com/?{}@", "&a={".repeat(8_000));

        // 4. HIGH DoS Bomb Family 4: `extract_html_segments` unquoted `+` & `%22` (R1 REV9-04, R2 REV9-03(4), C1 BUG-I9-04, C2 BUG-I9-03)
        let dos4_html_unq_64kb =
            format!("https://ex.com/?html=<a+href=https://u:p{}>@", "+".repeat(64_000));
        let dos4_html_unq_pct8 = format!(
            "https://ex.com/?html=<a+href=https://u:p{}>@%2525252525252520",
            "+".repeat(16_000)
        );
        let dos4_html_pct22_48kb = format!(
            "https://ex.com/?html=%3Ca%20href%3D%22https://u:p{}%3E@",
            "%22".repeat(16_000)
        );

        for (name, payload, max_ms) in [
            ("dos1_paren_56kb", &dos1_paren_56kb, 250u64),
            ("dos1_squote_16kb", &dos1_squote_16kb, 250),
            ("dos1_squote_pct8_9kb", &dos1_squote_pct8_9kb, 250),
            ("dos1_dquote_36kb", &dos1_dquote_36kb, 250),
            ("dos1_paren_bal_40kb", &dos1_paren_bal_40kb, 250),
            ("dos1_paren_multi_at", &dos1_paren_multi_at, 250),
            ("dos1_paren_pct8_14kb", &dos1_paren_pct8_14kb, 250),
            ("dos2_sp_slash_at", &dos2_sp_slash_at, 250),
            ("dos2_sp_slash_pct8", &dos2_sp_slash_pct8, 250),
            ("dos2_sp_author", &dos2_sp_author, 350),
            ("dos2_sp_author_pct8", &dos2_sp_author_pct8, 350),
            ("dos2_sp_long_qual", &dos2_sp_long_qual, 250),
            ("dos2_squote_bang_64kb", &dos2_squote_bang_64kb, 250),
            ("dos2_rparen_bang_64kb", &dos2_rparen_bang_64kb, 250),
            ("dos2_dquote_nonport_32kb", &dos2_dquote_nonport_32kb, 250),
            ("dos3_bug_i9_01", &dos3_bug_i9_01, 50),
            ("dos3_rev9_03", &dos3_rev9_03, 50),
            ("dos3_nest4_1279b", &dos3_nest4_1279b, 50),
            ("dos3_two_branch_d4", &dos3_two_branch_d4, 50),
            ("dos3_adv_pct7b_d4", &dos3_adv_pct7b_d4, 100),
            ("dos3_unbalanced_braces", &dos3_unbalanced_braces, 250),
            ("dos4_html_unq_64kb", &dos4_html_unq_64kb, 250),
            ("dos4_html_unq_pct8", &dos4_html_unq_pct8, 250),
            ("dos4_html_pct22_48kb", &dos4_html_pct22_48kb, 250),
        ] {
            let t = Instant::now();
            assert!(!link_carries_credential(payload), "{name} should be benign");
            let d = t.elapsed();
            assert!(
                d < Duration::from_millis(max_ms),
                "{name} exceeded {max_ms}ms in debug: {d:?}"
            );
        }

        // 5. Fix 5 (R1 REV9-05A): Dual-`@` / trailing-`@` poisoning (14/14)
        let dual_at_bypasses = [
            "https://ex.com/?q=https://u:p@evil.com,@",
            "https://ex.com/?q=https://user:pass@evil.com;@",
            "https://ex.com/?q=https://user:pass@evil.com&@",
            "https://ex.com/?q=https://user:pass@evil.com+@",
            "https://ex.com/?q=https://user:pass@evil.com)@",
            "https://ex.com/?q=https://user:pass@evil.com|@",
            "https://ex.com/?q=https://user:pass@evil.com%2C@",
            "https://ex.com/?q=https://user:p%2Fw@evil.com%2C@",
            "https://ex.com/?q=postgres://admin:secret@db.internal:5432,@",
            "https://ex.com/?q=//user:pass@evil.com,@",
            "https://ex.com/?q=https:user:pass@evil.com,@",
            "https://ex.com/?q=(https://user:pass@evil.com,@)",
            "https://ex.com/?q=https://user:pass@evil.com,a@!",
            "https://ex.com/?q=https://user:pass@evil.com,@!",
        ];
        for url in dual_at_bypasses {
            assert!(
                link_carries_credential(url),
                "REV9-05A dual-@ credential must be rejected: {url}"
            );
            assert_eq!(sanitize_suggestion_link(Some(url)), None);
        }

        // 6. Fix 6 (R1 REV9-05B, R2 REV9-04, C1 BUG-I9-05, C2 BUG-I9-06 Part 2, R2 NB, long scheme)
        let nested_cred_bypasses = [
            // R2 REV9-04(1): %2B/%2C encodeURIComponent credentials
            "https://ex.com/?next=https%3A%2F%2Falice.smith%3A443%2Bhunter2%40evil.com%2Fsecret",
            "https://ex.com/?next=https%3A%2F%2Falice.smith%3A443%2Bhunter2%40evil.com",
            "https://ex.com/?next=https%3A%2F%2Falice.smith%3A443%2Chunter2%40evil.com%2Fsecret",
            "https://ex.com/?next=https%3A%2F%2Falice.smith%2Bsecret%40evil.com",
            "https://ex.com/?next=https%3A%2F%2Falice.smith%2Csecret%40evil.com",
            "https://ex.com/?next=http%3A%2F%2Fmy_user%3A1234%2Bsecret%40evil.com",
            "https://ex.com/?next=http%3A%2F%2Flocalhost%3A1234%2Bsecret%40evil.com",
            "https://ex.com/?next=http%3A%2F%2Flocalhost%3A3000%2Bsecret%40evil.com%2Fadmin",
            // R1 REV9-05B & C1 BUG-I9-05(1,2,3): Branch 3a/3b/3c
            "https://ex.com/?next=http://[::1]:1234+secret@evil.com",
            "https://ex.com/?next=http://[::1]:3000+user:hunter2@evil.com",
            "https://ex.com/?next=https://[::1]:443+admin:s3cr3t!@evil.com",
            "https://ex.com/?next=https://[::1]+admin:hunter2@evil.com",
            "https://ex.com/?next=http://[::1]:1234+admin:hunter2@evil.com",
            "https://ex.com/?next=http://[::1]:3000,admin:hunter2@evil.com",
            "https://ex.com/?next=http://localhost:1234+secret@evil.com",
            "https://ex.com/?next=http://localhost:1234+p!w:hunter2@evil.com",
            "https://ex.com/?next=http://localhost:3000+admin:hunter2@evil.com",
            "https://ex.com/?next=http://my_user:1234+secret@evil.com",
            "https://ex.com/?next=http://my_user:9999+hunter2@evil.com",
            "https://ex.com/?next=http://my_user:1234+p!w:hunter2@evil.com",
            "https://ex.com/?next=https://alice.smith:443+p!w:hunter2@evil.com",
            "https://ex.com/?next=https://alice.smith:8443+p$w:s3cr3t@evil.com",
            "https://ex.com/?next=https://alice.smith:443,admin:hunter2@evil.com",
            "https://ex.com/?next=https://alice.smith:443%20admin:hunter2@evil.com",
            "https://ex.com/?next=https://alice.smith+p!w$secret@evil.com",
            "https://ex.com/?q=see%20https://docs.rs%20p!w@evil.com/admin/keys",
            "https://ex.com/?q=see%20https://alice.smith%20p$w@evil.com/v1/secret",
            // R2 REV9-04(3), C1 BUG-I9-05(4), C2 BUG-I9-06 Part 2: Branch 2 & amp-host & Branch 1 %3F IP
            "https://ex.com/?next=https%3A%2F%2Falice.smith%26a%3Db%40evil.com%3A8443%2Fsecret",
            "https://ex.com/?next=http%3A%2F%2Fadmin%3A8080%26a%3Db%40evil.com",
            "https://ex.com/?next=https%3A%2F%2F%5B%3A%3A1%5D%3A443%26a%3Db%40evil.com%2Fsecret",
            "https://ex.com/?next=https%3A%2F%2Fexample.com%26a%3Db%40evil.com%2Fsecret",
            "https://ex.com/?next=https%3A%2F%2Fexample.com%26a%3Db%40evil.com%3A8443%2Fsecret",
            "https://ex.com/?next=https%3A%2F%2Flocalhost%26a%3Db%40evil.com%2Fadmin",
            "https://ex.com/?next=https%3A%2F%2Flocalhost%3A443%26a%3Db%40evil.com%2Fadmin",
            "https://ex.com/?next=https%3A%2F%2Flocalhost%3A8443%26a%3Db%40evil.com%2Fadmin",
            "https://ex.com/?next=http%3A%2F%2Flocalhost%3A8080%26a%3Db%40evil.com%2Fadmin",
            "https://ex.com/?next=https%3A%2F%2F%5B%3A%3A1%5D%3A443%26a%3Db%40evil.com%2Fadmin",
            "https://ex.com/?next=https://example.com&a=b@evil.com:8443/admin",
            "https://ex.com/?next=http://localhost:8080&a=b@evil.com/admin",
            "https://ex.com/?next=https://localhost:443&a=b@evil.com/admin",
            "https://ex.com/?next=https://a.b%3Fx=y@10.0.0.1",
            "https://ex.com/?next=https://a.b%3Fx=y@[::1]",
            // R2 NB unbalanced `{` before valid JSON & >32B scheme
            "https://ex.com/?a={&data={\"a\":\"?\",\"url\":\"https://u:p\\u0040evil.com/\"}}",
            "https://ex.com/?q=aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa://u:p@evil.com/",
        ];
        for url in nested_cred_bypasses {
            assert!(
                link_carries_credential(url),
                "Iter-9 nested/Branch-1/2/3/amp credential must be rejected: {url}"
            );
            assert_eq!(sanitize_suggestion_link(Some(url)), None);
        }

        // 7. Fix 2d & Fix 7 (C2 BUG-I9-06 Part 1 Unicode whitespace passwords + C2 BUG-I9-05 path-start/segment proto-rel)
        let unicode_ws_and_path_proto_rel = [
            // C2 BUG-I9-06 Part 1 (7/7)
            "https://ex.com/go/https%3A%2F%2Fuser%3Ap%C2%A0w%40evil.com%2F",
            "https://ex.com/go/https://user:p%C2%A0w%40evil.com/",
            "https://ex.com/go/https%3A%2F%2Fuser%3Ap%E3%80%80w%40evil.com%2F",
            "https://ex.com/go/https%3A%2F%2Fuser%3Ap%E2%80%83w%40evil.com%2F",
            "https://ex.com/?urls=https://ok.com,https%3A%2F%2Fuser%3Ap%C2%A0w%40evil.com%2F",
            "https://ex.com/?urls=https://ok.com,https%3A%2F%2Fuser%3Ap%E3%80%80w%40evil.com%2F",
            "https://ex.com/?urls=https://ok.com,%2F%2Fuser%3Ap%C2%A0w%40evil.com%2F",
            // C2 BUG-I9-05 (13/13)
            "https://ex.com///user:pass@evil.com/",
            "https://ex.com///user:pass@evil.com/secret",
            "https://ex.com/%2F%2Fuser:pass@evil.com/",
            "https://ex.com/%2F%2Fuser%3Apass%40evil.com%2Fsecret",
            "https://ex.com/\\\\user:pass@evil.com/",
            "https://ex.com/%5C%5Cuser:pass@evil.com/",
            "https://ex.com/?next=https://ok.com///user:pass@evil.com/",
            "https://ex.com/?next=https://ok.com/%2F%2Fuser:pass@evil.com/",
            "https://ex.com/?next=https%3A%2F%2Fok.com%2F%2F%2Fuser%3Apass%40evil.com%2F",
            "https://ex.com/?next=https%3A%2F%2Fok.com%2F%252F%252Fuser%253Apass%2540evil.com%252F",
            "https://ex.com/redirect//user:pass@evil.com/",
            "https://ex.com/redirect%2F%2Fuser:pass@evil.com/",
            "https://ex.com//user:pass@evil.com/",
        ];
        for url in unicode_ws_and_path_proto_rel {
            assert!(
                link_carries_credential(url),
                "Iter-9 Unicode-ws/path-proto-rel credential must be rejected: {url}"
            );
            assert_eq!(sanitize_suggestion_link(Some(url)), None);
        }

        // 8. Benign guards ensuring zero false positives from Iteration-9 tightening
        let benign_iter9 = [
            "https://example.com///@alice",
            "https://ex.com/redirect//@alice",
            "https://ex.com//alice@example.com",
            "https://example.com/docs/hello%C2%A0world",
            "https://ex.com/?q=see%C2%A0https://ok.com%20alice@example.com",
            "https://ex.com/?next=https%3A%2F%2Fapp.com%2F%3Furl%3Dhttps%3A%2F%2Fok.com%26ref%3Dalice%40example.com%2Fdocs",
            "https://ex.com/?next=https%3A%2F%2Fok.com%26pkg%3D%40types%2Fnode",
            "https://ex.com/?next=http%3A%2F%2Flocalhost%3A3000%26email%3Dalice%40example.com%2Ffoo",
            "https://ex.com/?q=see%20https://docs.rs%20alice@example.com/profile",
            "https://ex.com/?q=see%20https://docs.rs%20alice@example.com/notes",
            "https://ex.com/?url=http://intranet:8080&owner=bob@corp.com&q=a%20b",
            "https://ex.com/?next=http://intranet:8080&contact=alice%40example.com",
        ];
        for url in benign_iter9 {
            assert!(
                !link_carries_credential(url),
                "Iter-9 benign URL must NOT be flagged: {url}"
            );
            assert_eq!(sanitize_suggestion_link(Some(url)).as_deref(), Some(url));
        }
    }
}
