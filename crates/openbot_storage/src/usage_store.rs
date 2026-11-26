//! Agent & Host token/cost/skill telemetry store (`AgentUsageStore`).
//! Faithful 1:1 port of `src/backend/database/agent-usage.ts`, `src/backend/agent/usage-pricing.ts`,
//! and `packages/contracts/src/{ipc-agent-analytics,ipc-host-analytics}.ts`.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::fmt::Write as _;

use chrono::{Days, NaiveDate, TimeZone, Utc};
use rusqlite::{Connection, OptionalExtension, params};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use uuid::Uuid;

use crate::database::{OpenBotDatabase, OrchestrationEventInput};
use crate::error::{StorageError, StorageResult};
use crate::workflow_store::{ms_to_iso, now_millis, parse_iso_ms};
use openbot_contracts::conversation::{
    ConversationMessage, ConversationMessageAuthor, ConversationMessageStatus,
};
use openbot_contracts::limits::{is_calendar_date, utf16_len};
use openbot_contracts::routines::{is_valid_timezone, routine_once_instant};

pub const WORKFLOWS_USAGE_ID: &str = "openbot:workflows";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UsageTokens {
    pub uncached_input: Option<u64>,
    pub cached_input: Option<u64>,
    pub cache_creation: Option<u64>,
    pub output: Option<u64>,
}

impl UsageTokens {
    pub fn zero() -> Self {
        Self {
            uncached_input: Some(0),
            cached_input: Some(0),
            cache_creation: Some(0),
            output: Some(0),
        }
    }

    pub fn any_present(&self) -> bool {
        self.uncached_input.is_some()
            || self.cached_input.is_some()
            || self.cache_creation.is_some()
            || self.output.is_some()
    }

    pub fn any_missing(&self) -> bool {
        self.uncached_input.is_none()
            || self.cached_input.is_none()
            || self.cache_creation.is_none()
            || self.output.is_none()
    }

    pub fn sum_present(&self) -> u64 {
        self.uncached_input.unwrap_or(0)
            + self.cached_input.unwrap_or(0)
            + self.cache_creation.unwrap_or(0)
            + self.output.unwrap_or(0)
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UsageSample {
    pub agent_id: String,
    pub session_id: String,
    pub turn_id: String,
    pub provider: String,
    pub model: String,
    pub counter_id: String,
    pub tokens: UsageTokens,
    pub baseline: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reported_cost: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub occurred_at: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_request_input_tokens: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub smallest_request_input_tokens: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub previous_tokens: Option<UsageTokens>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cost_counter_id: Option<String>,
    #[serde(default)]
    pub zero_cost_unpriced: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SkillLoadSample {
    pub agent_id: String,
    pub session_id: String,
    pub turn_id: String,
    pub item_id: String,
    pub provider: String,
    pub model: String,
    pub skill: String,
    pub tokens: Option<u64>,
    pub occurred_at: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TurnModelUsage {
    pub provider: String,
    pub model: String,
    pub tokens: UsageTokens,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TurnUsageTotals {
    pub tokens: u64,
    pub cached_input: u64,
    pub cost_usd: Option<f64>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AnalyticsTotals {
    pub uncached_input: Option<u64>,
    pub cached_input: Option<u64>,
    pub cache_creation: Option<u64>,
    pub output: Option<u64>,
    pub processed_tokens: u64,
    pub estimated_cost_usd: Option<f64>,
    pub sessions: usize,
    pub user_messages: usize,
    pub assistant_messages: usize,
    pub turns: usize,
    pub missing_usage_turns: usize,
    pub unpriced_records: usize,
    pub incomplete_records: usize,
}

impl Default for AnalyticsTotals {
    fn default() -> Self {
        Self::empty()
    }
}

impl AnalyticsTotals {
    pub fn empty() -> Self {
        Self {
            uncached_input: None,
            cached_input: None,
            cache_creation: None,
            output: None,
            processed_tokens: 0,
            estimated_cost_usd: None,
            sessions: 0,
            user_messages: 0,
            assistant_messages: 0,
            turns: 0,
            missing_usage_turns: 0,
            unpriced_records: 0,
            incomplete_records: 0,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AnalyticsDay {
    pub date: String,
    #[serde(flatten)]
    pub totals: AnalyticsTotals,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AnalyticsModel {
    pub provider: String,
    pub model: String,
    pub share: f64,
    #[serde(flatten)]
    pub totals: AnalyticsTotals,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AnalyticsAgent {
    pub agent_id: String,
    pub share: f64,
    #[serde(flatten)]
    pub totals: AnalyticsTotals,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AnalyticsProviderDay {
    pub date: String,
    pub provider: String,
    pub processed_tokens: u64,
    pub cached_input: Option<u64>,
    pub estimated_cost_usd: Option<f64>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AnalyticsSkill {
    pub skill: String,
    pub loads: usize,
    pub tokens: u64,
    pub estimated_cost_usd: Option<f64>,
    pub agents: usize,
    pub share: f64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentAnalyticsInput {
    pub agent_id: String,
    pub start_date: String,
    pub end_date: String,
    pub time_zone: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentAnalytics {
    pub agent_id: String,
    pub start_date: String,
    pub end_date: String,
    pub time_zone: String,
    pub collection_started_at: String,
    pub updated_at: Option<String>,
    pub totals: AnalyticsTotals,
    pub daily: Vec<AnalyticsDay>,
    pub models: Vec<AnalyticsModel>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HostAnalyticsInput {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent_id: Option<String>,
    pub start_date: String,
    pub end_date: String,
    pub time_zone: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HostAnalytics {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent_id: Option<String>,
    pub start_date: String,
    pub end_date: String,
    pub time_zone: String,
    pub collection_started_at: String,
    pub updated_at: Option<String>,
    pub totals: AnalyticsTotals,
    pub daily: Vec<AnalyticsDay>,
    pub models: Vec<AnalyticsModel>,
    pub agents: Vec<AnalyticsAgent>,
    pub provider_daily: Vec<AnalyticsProviderDay>,
    pub skills: Vec<AnalyticsSkill>,
}

// ---------------------------------------------------------------------------
// Pricing Catalog (`src/backend/agent/usage-pricing.ts`)
// ---------------------------------------------------------------------------

const EFFECTIVE_DATE: &str = "2026-10-21";
const SRC_OPENAI: &str = "https://developers.openai.com/api/docs/pricing";
const SRC_CLAUDE: &str = "https://platform.claude.com/docs/en/about-claude/pricing";
const SRC_GROK: &str = "https://docs.x.ai/developers/pricing";
const SRC_GEMINI: &str = "https://ai.google.dev/gemini-api/docs/pricing";
const OPENAI_LONG: u64 = 272_001;
const GROK_LONG: u64 = 200_000;
const HAIKU_5_5_LONG: u64 = 100_001;

#[derive(Clone, Copy)]
struct Rates {
    input: f64,
    cached: f64,
    cache_write: Option<f64>,
    output: f64,
}

#[derive(Clone, Copy)]
struct LongRates {
    from: u64,
    rates: Rates,
}

#[derive(Clone, Copy)]
struct PriceChange {
    on: &'static str,
    short: Rates,
}

#[derive(Clone, Copy)]
struct ModelPrice {
    source: &'static str,
    verified: Option<&'static str>,
    short: Rates,
    long: Option<LongRates>,
    change: Option<PriceChange>,
}

const fn claude_price(input: f64, cached: f64, output: f64) -> ModelPrice {
    ModelPrice {
        source: SRC_CLAUDE,
        verified: None,
        short: Rates {
            input,
            cached,
            cache_write: Some(input * 2.0),
            output,
        },
        long: None,
        change: None,
    }
}

const fn openai_price(
    verified: Option<&'static str>,
    input: f64,
    cached: f64,
    cw: f64,
    output: f64,
    l_in: f64,
    l_cached: f64,
    l_cw: f64,
    l_out: f64,
) -> ModelPrice {
    ModelPrice {
        source: SRC_OPENAI,
        verified,
        short: Rates {
            input,
            cached,
            cache_write: Some(cw),
            output,
        },
        long: Some(LongRates {
            from: OPENAI_LONG,
            rates: Rates {
                input: l_in,
                cached: l_cached,
                cache_write: Some(l_cw),
                output: l_out,
            },
        }),
        change: None,
    }
}

fn lookup_exact_price(key: &str) -> Option<ModelPrice> {
    match key {
        "codex:gpt-6-astra" => Some(openai_price(None, 10.0, 1.0, 12.5, 50.0, 20.0, 2.0, 25.0, 75.0)),
        "codex:gpt-6.2-astra" => Some(openai_price(Some("2026-11-17"), 8.0, 0.4, 10.0, 40.0, 16.0, 0.8, 20.0, 60.0)),
        "codex:gpt-6.2-sol" => Some(openai_price(Some("2026-11-17"), 2.0, 0.1, 2.5, 10.0, 4.0, 0.2, 5.0, 15.0)),
        "codex:gpt-6.1-sol" => Some(openai_price(None, 2.0, 0.1, 2.5, 10.0, 4.0, 0.2, 5.0, 15.0)),
        "codex:gpt-6.1-terra" => Some(openai_price(None, 1.0, 0.05, 1.25, 5.0, 2.0, 0.1, 2.5, 7.5)),
        "codex:gpt-6-sol" => Some(openai_price(None, 2.0, 0.2, 2.5, 10.0, 4.0, 0.4, 5.0, 15.0)),
        "codex:gpt-6-luna" => Some(openai_price(None, 0.1, 0.01, 0.125, 0.5, 0.2, 0.02, 0.25, 0.75)),
        "codex:gpt-5.6-sol" => Some(openai_price(None, 4.0, 0.4, 5.0, 20.0, 8.0, 0.8, 10.0, 30.0)),
        "codex:gpt-5.6-terra" => Some(openai_price(None, 2.0, 0.2, 2.5, 12.0, 4.0, 0.4, 5.0, 18.0)),
        "codex:gpt-5.6-luna" => Some(openai_price(None, 0.2, 0.02, 0.25, 1.2, 0.4, 0.04, 0.5, 1.8)),
        "codex:gpt-5.5" => Some(openai_price(None, 5.0, 0.5, 5.0, 30.0, 10.0, 1.0, 10.0, 45.0)),
        "grok:grok-4.6" => Some(ModelPrice {
            source: SRC_GROK,
            verified: None,
            short: Rates { input: 2.0, cached: 0.5, cache_write: None, output: 6.0 },
            long: Some(LongRates {
                from: GROK_LONG,
                rates: Rates { input: 4.0, cached: 1.0, cache_write: None, output: 12.0 },
            }),
            change: None,
        }),
        "grok:grok-build-0.1" => Some(ModelPrice {
            source: SRC_GROK,
            verified: None,
            short: Rates { input: 1.0, cached: 0.2, cache_write: None, output: 2.0 },
            long: Some(LongRates {
                from: GROK_LONG,
                rates: Rates { input: 2.0, cached: 0.4, cache_write: None, output: 4.0 },
            }),
            change: None,
        }),
        "claude:claude-fable-5-1" => Some(claude_price(10.0, 0.25, 50.0)),
        "claude:claude-fable-5" => Some(claude_price(10.0, 1.0, 50.0)),
        "claude:claude-opus-5-5" => Some(claude_price(4.0, 0.2, 20.0)),
        "claude:claude-opus-5" | "claude:claude-opus-4-8" | "claude:claude-opus-4-7" | "claude:claude-opus-4-6" => {
            Some(claude_price(5.0, 0.5, 25.0))
        }
        "claude:claude-sonnet-5-5" => Some(claude_price(2.0, 0.1, 10.0)),
        "claude:claude-sonnet-5" => Some(claude_price(2.0, 0.2, 10.0)),
        "claude:claude-sonnet-4-6" => Some(claude_price(3.0, 0.3, 15.0)),
        "claude:claude-haiku-5-5" => Some(ModelPrice {
            source: SRC_CLAUDE,
            verified: None,
            short: Rates { input: 0.1, cached: 0.01, cache_write: Some(0.2), output: 0.5 },
            long: Some(LongRates {
                from: HAIKU_5_5_LONG,
                rates: Rates { input: 0.5, cached: 0.05, cache_write: Some(1.0), output: 2.5 },
            }),
            change: None,
        }),
        "claude:claude-haiku-4-5" => Some(claude_price(1.0, 0.1, 5.0)),
        "antigravity:gemini-4-argon" => Some(ModelPrice {
            source: SRC_GEMINI,
            verified: Some("2026-10-28"),
            short: Rates { input: 2.0, cached: 0.1, cache_write: None, output: 10.0 },
            long: None,
            change: Some(PriceChange {
                on: "2026-12-01",
                short: Rates { input: 4.0, cached: 0.2, cache_write: None, output: 20.0 },
            }),
        }),
        "antigravity:gemini-4-hydrogen" => Some(ModelPrice {
            source: SRC_GEMINI,
            verified: Some("2026-11-18"),
            short: Rates { input: 1.5, cached: 0.075, cache_write: None, output: 7.5 },
            long: None,
            change: None,
        }),
        "antigravity:gemini-4-neon" => Some(ModelPrice {
            source: SRC_GEMINI,
            verified: Some("2026-11-22"),
            short: Rates { input: 3.0, cached: 0.15, cache_write: None, output: 15.0 },
            long: None,
            change: None,
        }),
        "antigravity:gemini-3.8-flash" => Some(ModelPrice {
            source: SRC_GEMINI,
            verified: Some("2026-10-28"),
            short: Rates { input: 0.75, cached: 0.075, cache_write: None, output: 3.75 },
            long: None,
            change: Some(PriceChange {
                on: "2027-01-01",
                short: Rates { input: 1.5, cached: 0.15, cache_write: None, output: 7.5 },
            }),
        }),
        "antigravity:claude-opus-5-5" => Some(ModelPrice {
            source: SRC_CLAUDE,
            verified: Some("2026-10-28"),
            short: Rates { input: 4.0, cached: 0.2, cache_write: None, output: 20.0 },
            long: None,
            change: None,
        }),
        "antigravity:claude-sonnet-5-5" => Some(ModelPrice {
            source: SRC_CLAUDE,
            verified: Some("2026-10-28"),
            short: Rates { input: 2.0, cached: 0.1, cache_write: None, output: 10.0 },
            long: None,
            change: None,
        }),
        _ => None,
    }
}

fn price_of(provider: &str, model: &str) -> Option<ModelPrice> {
    let normalized = if provider == "claude" {
        model.strip_suffix("[1m]").unwrap_or(model)
    } else {
        model
    };
    let key = format!("{provider}:{normalized}");
    if let Some(found) = lookup_exact_price(&key) {
        return Some(found);
    }
    if provider == "antigravity" {
        for suffix in ["-minimal", "-low", "-medium", "-high"] {
            if let Some(base) = model.strip_suffix(suffix) {
                return lookup_exact_price(&format!("{provider}:{base}"));
            }
        }
    }
    None
}

pub fn input_price(provider: &str, model: &str) -> Option<f64> {
    price_of(provider, model).map(|p| p.short.input)
}

#[derive(Debug, Clone, PartialEq)]
pub struct UsageCostEstimate {
    pub cost: Option<f64>,
    pub basis: Option<String>,
}

pub fn estimate_usage_cost(
    provider: &str,
    model: &str,
    tokens: &UsageTokens,
    last_request_input_tokens: Option<u64>,
    smallest_request_input_tokens: Option<u64>,
    occurred_at: &str,
) -> UsageCostEstimate {
    let (Some(price), Some(u_in), Some(c_in), Some(c_wr), Some(out)) = (
        price_of(provider, model),
        tokens.uncached_input,
        tokens.cached_input,
        tokens.cache_creation,
        tokens.output,
    ) else {
        return UsageCostEstimate {
            cost: None,
            basis: None,
        };
    };
    let input = u_in + c_in + c_wr;
    let context = match price.long {
        None => Some("short"),
        Some(l) if input < l.from => Some("short"),
        Some(l)
            if Some(input) == last_request_input_tokens
                || smallest_request_input_tokens.is_some_and(|s| s >= l.from) =>
        {
            Some("long")
        }
        Some(_) => None,
    };
    let changed = price.change.filter(|c| occurred_at >= c.on);
    let rates = match context {
        Some("short") => Some(changed.map_or(price.short, |c| c.short)),
        Some("long") => price.long.map(|l| l.rates),
        _ => None,
    };
    let (Some(ctx), Some(r)) = (context, rates) else {
        return UsageCostEstimate {
            cost: None,
            basis: None,
        };
    };
    if c_wr != 0 && r.cache_write.is_none() {
        return UsageCostEstimate {
            cost: None,
            basis: None,
        };
    }
    let cost = ((u_in as f64) * r.input
        + (c_in as f64) * r.cached
        + (c_wr as f64) * r.cache_write.unwrap_or(0.0)
        + (out as f64) * r.output)
        / 1_000_000.0;
    let eff = changed
        .map(|c| c.on)
        .or(price.verified)
        .unwrap_or(EFFECTIVE_DATE);
    let mut map = serde_json::Map::new();
    map.insert("effectiveDate".into(), serde_json::Value::from(eff));
    map.insert("model".into(), serde_json::Value::from(model));
    map.insert("context".into(), serde_json::Value::from(ctx));
    map.insert("input".into(), js_float_value(r.input));
    map.insert("cached".into(), js_float_value(r.cached));
    if let Some(cw) = r.cache_write {
        map.insert("cacheWrite".into(), js_float_value(cw));
    }
    map.insert("output".into(), js_float_value(r.output));
    map.insert("source".into(), serde_json::Value::from(price.source));
    let basis = serde_json::to_string(&serde_json::Value::Object(map)).ok();
    UsageCostEstimate {
        cost: Some(cost),
        basis,
    }
}

fn js_float_value(n: f64) -> serde_json::Value {
    if n.is_finite() && n.fract() == 0.0 && n.abs() < (i64::MAX as f64) {
        serde_json::Value::from(n as i64)
    } else {
        serde_json::Number::from_f64(n)
            .map_or(serde_json::Value::Null, serde_json::Value::Number)
    }
}

fn reported_cost_basis(provider: &str) -> &'static str {
    match provider {
        "opencode" => "OpenCode session cost report; API-equivalent rates, not subscription charges",
        "grok" => "Grok CLI prompt cost report",
        "pi" | "inception" | "deepseek" | "openrouter" => {
            "Pi session cost report; rates of Pi's model list, not subscription charges"
        }
        "acp" => "Custom agent session cost report",
        _ => "Claude SDK list-price estimate; https://platform.claude.com/docs/en/about-claude/pricing; verified 2026-09-07",
    }
}

pub fn usage_record_digest(
    counter_id: &str,
    turn_id: &str,
    tokens: &UsageTokens,
    reported_cost: Option<f64>,
) -> StorageResult<String> {
    let cost_val = reported_cost.map_or(serde_json::Value::Null, js_float_value);
    let payload = serde_json::json!([counter_id, turn_id, tokens, cost_val]);
    let encoded = serde_json::to_string(&payload)?;
    let digest = Sha256::digest(encoded.as_bytes());
    let mut hex = String::with_capacity(64);
    for b in digest {
        let _ = write!(&mut hex, "{b:02x}");
    }
    Ok(hex)
}

/// Computes exact `YYYY-MM-DD` local calendar date of `instant_ms` in `timezone`.
pub fn analytics_date(instant_ms: i64, timezone: &str) -> StorageResult<String> {
    if !is_valid_timezone(timezone) {
        return Err(StorageError::Validation(format!(
            "invalid analytics time zone: {timezone}"
        )));
    }
    let utc_dt = Utc
        .timestamp_millis_opt(instant_ms)
        .single()
        .ok_or_else(|| StorageError::Validation("analytics timestamp out of range".to_string()))?;
    let base_date = utc_dt.date_naive();
    let candidates = [
        base_date.checked_sub_days(Days::new(1)),
        Some(base_date),
        base_date.checked_add_days(Days::new(1)),
        base_date.checked_add_days(Days::new(2)),
    ];
    let mut chosen = base_date;
    for d in candidates.into_iter().flatten() {
        let ds = d.format("%Y-%m-%d").to_string();
        if let Ok(start_ms) = routine_once_instant(&ds, "00:00", timezone)
            && start_ms <= instant_ms
        {
            chosen = d;
        }
    }
    Ok(chosen.format("%Y-%m-%d").to_string())
}

#[derive(Clone)]
pub struct AgentUsageStore {
    db: OpenBotDatabase,
}

impl AgentUsageStore {
    pub fn new(db: OpenBotDatabase) -> Self {
        Self { db }
    }

    pub fn collection_started_at(&self) -> StorageResult<String> {
        self.db.with_conn(|conn| {
            conn.query_row(
                "SELECT applied_at FROM schema_migrations WHERE version = 15",
                [],
                |r| r.get(0),
            )
            .optional()?
            .ok_or_else(|| StorageError::Validation("analytics schema v15 unavailable".to_string()))
        })
    }

    pub fn reroute(
        &self,
        agent_id: &str,
        session_id: &str,
        turn_id: &str,
        model: &str,
    ) -> StorageResult<()> {
        let activity_id = format!("turn:{session_id}:{turn_id}");
        self.db.with_conn(|conn| {
            conn.execute(
                "UPDATE agent_usage_activity SET model = ?1 WHERE agent_id = ?2 AND activity_id = ?3",
                params![model, agent_id, activity_id],
            )?;
            Ok(())
        })
    }

    pub fn turn_model(
        &self,
        agent_id: &str,
        session_id: &str,
        turn_id: &str,
    ) -> StorageResult<Option<String>> {
        let activity_id = format!("turn:{session_id}:{turn_id}");
        self.db.with_conn(|conn| {
            conn.query_row(
                "SELECT model FROM agent_usage_activity WHERE agent_id = ?1 AND activity_id = ?2",
                params![agent_id, activity_id],
                |r| r.get(0),
            )
            .optional()
            .map_err(StorageError::from)
        })
    }

    pub fn record(&self, sample: &UsageSample) -> StorageResult<()> {
        let mut tokens = sample.tokens;
        let digest = usage_record_digest(
            &sample.counter_id,
            &sample.turn_id,
            &tokens,
            sample.reported_cost,
        )?;
        let cmd_id = format!("analytics:{}:{digest}", sample.agent_id);
        let now_iso = ms_to_iso(now_millis());
        let event = OrchestrationEventInput {
            event_id: Uuid::new_v4().to_string(),
            aggregate_type: "agent-usage".to_string(),
            aggregate_id: sample.agent_id.clone(),
            event_type: "usage.recorded".to_string(),
            occurred_at: sample
                .occurred_at
                .clone()
                .unwrap_or_else(|| now_iso.clone()),
            payload: serde_json::json!({ "digest": digest }),
        };
        self.db.dispatch_with_projection(
            &cmd_id,
            None,
            &[event],
            |conn, _| {
                let prev_json: Option<String> = conn
                    .query_row(
                        "SELECT tokens_json FROM agent_usage_checkpoints \
                         WHERE agent_id = ?1 AND counter_id = ?2",
                        params![sample.agent_id, sample.counter_id],
                        |r| r.get(0),
                    )
                    .optional()?;
                let cost_cp_json: Option<String> = match &sample.cost_counter_id {
                    Some(cc_id) => conn
                        .query_row(
                            "SELECT tokens_json FROM agent_usage_checkpoints \
                             WHERE agent_id = ?1 AND counter_id = ?2",
                            params![sample.agent_id, cc_id],
                            |r| r.get(0),
                        )
                        .optional()?,
                    None => prev_json.clone(),
                };
                let prev_cost: Option<f64> = cost_cp_json
                    .as_deref()
                    .and_then(|s| serde_json::from_str::<serde_json::Value>(s).ok())
                    .and_then(|v| v.get("reportedCost")?.as_f64());
                let reported_cost = match sample.reported_cost {
                    None => prev_cost,
                    Some(rc) => Some(rc.max(prev_cost.unwrap_or(0.0))),
                };
                let before: Option<UsageTokens> = match &prev_json {
                    Some(s) => Some(serde_json::from_str::<UsageTokens>(s)?),
                    None if sample.baseline => None,
                    None => sample.previous_tokens,
                };
                let delta = compute_monotonic_delta(&mut tokens, before.as_ref());
                let cp_val = if sample.cost_counter_id.is_some() {
                    serde_json::to_string(&tokens)?
                } else {
                    let mut obj = serde_json::to_value(tokens)?;
                    if let Some(map) = obj.as_object_mut() {
                        map.insert(
                            "reportedCost".into(),
                            reported_cost.map_or(serde_json::Value::Null, js_float_value),
                        );
                    }
                    serde_json::to_string(&obj)?
                };
                conn.execute(
                    "INSERT INTO agent_usage_checkpoints (agent_id, counter_id, tokens_json) \
                     VALUES (?1, ?2, ?3) \
                     ON CONFLICT(agent_id, counter_id) DO UPDATE SET tokens_json = excluded.tokens_json",
                    params![sample.agent_id, sample.counter_id, cp_val],
                )?;
                let cost_start_unknown = if sample.cost_counter_id.is_some()
                    && sample.reported_cost.is_some()
                    && prev_cost.is_none()
                {
                    conn.query_row(
                        "SELECT 1 FROM agent_usage_records WHERE agent_id = ?1 AND session_id = ?2 LIMIT 1",
                        params![sample.agent_id, sample.session_id],
                        |r| r.get::<_, i64>(0),
                    )
                    .optional()?
                    .is_some()
                } else {
                    false
                };
                if let (Some(cc_id), Some(rc)) = (&sample.cost_counter_id, reported_cost)
                    && sample.reported_cost.is_some()
                {
                    let cc_json = serde_json::to_string(&serde_json::json!({
                        "reportedCost": js_float_value(rc)
                    }))?;
                    conn.execute(
                        "INSERT INTO agent_usage_checkpoints (agent_id, counter_id, tokens_json) \
                         VALUES (?1, ?2, ?3) \
                         ON CONFLICT(agent_id, counter_id) DO UPDATE SET tokens_json = excluded.tokens_json",
                        params![sample.agent_id, cc_id, cc_json],
                    )?;
                }
                if (before.is_none() && sample.baseline) || sample.turn_id == "baseline" {
                    return Ok(());
                }
                let reported_delta = match sample.reported_cost {
                    None => None,
                    Some(_)
                        if cost_start_unknown
                            || (sample.cost_counter_id.is_none()
                                && before.is_some()
                                && prev_cost.is_none()) =>
                    {
                        None
                    }
                    Some(rc) => Some((rc - prev_cost.unwrap_or(0.0)).max(0.0)),
                };
                let reported_est = match reported_delta {
                    None => None,
                    Some(d) if sample.zero_cost_unpriced && d == 0.0 => None,
                    Some(d) => Some(UsageCostEstimate {
                        cost: Some(d),
                        basis: Some(reported_cost_basis(&sample.provider).to_string()),
                    }),
                };
                let occurred_at = match &sample.occurred_at {
                    Some(ts) => ts.clone(),
                    None => conn
                        .query_row(
                            "SELECT occurred_at FROM agent_usage_activity \
                             WHERE agent_id = ?1 AND activity_id = ?2",
                            params![
                                sample.agent_id,
                                format!("turn:{}:{}", sample.session_id, sample.turn_id)
                            ],
                            |r| r.get::<_, String>(0),
                        )
                        .optional()?
                        .unwrap_or_else(|| now_iso.clone()),
                };
                let est = reported_est.unwrap_or_else(|| {
                    estimate_usage_cost(
                        &sample.provider,
                        &sample.model,
                        &delta,
                        sample.last_request_input_tokens,
                        sample.smallest_request_input_tokens,
                        &occurred_at,
                    )
                });
                let delta_json = serde_json::to_string(&delta)?;
                conn.execute(
                    "INSERT INTO agent_usage_records \
                       (agent_id, record_id, session_id, turn_id, provider, model, occurred_at, \
                        tokens_json, estimated_cost_usd, rate_basis, recorded_at) \
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)",
                    params![
                        sample.agent_id,
                        digest,
                        sample.session_id,
                        sample.turn_id,
                        sample.provider,
                        sample.model,
                        occurred_at,
                        delta_json,
                        est.cost,
                        est.basis,
                        ms_to_iso(now_millis()),
                    ],
                )?;
                Ok(())
            },
        )?;
        Ok(())
    }

    pub fn record_skill_load(&self, sample: &SkillLoadSample) -> StorageResult<()> {
        if sample.skill.is_empty() || sample.skill.chars().count() > 128 {
            return Err(StorageError::Validation("invalid skill name length".to_string()));
        }
        let cost = sample.tokens.and_then(|toks| {
            estimate_usage_cost(
                &sample.provider,
                &sample.model,
                &UsageTokens {
                    uncached_input: Some(toks),
                    cached_input: Some(0),
                    cache_creation: Some(0),
                    output: Some(0),
                },
                None,
                None,
                &sample.occurred_at,
            )
            .cost
        });
        let load_id = serde_json::to_string(&serde_json::json!([
            sample.session_id,
            sample.item_id,
            sample.skill,
        ]))?;
        self.db.with_conn(|conn| {
            conn.execute(
                "INSERT OR IGNORE INTO agent_usage_skill_loads \
                   (agent_id, load_id, session_id, turn_id, provider, model, skill, tokens, \
                    estimated_cost_usd, occurred_at) \
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
                params![
                    sample.agent_id,
                    load_id,
                    sample.session_id,
                    sample.turn_id,
                    sample.provider,
                    sample.model,
                    sample.skill,
                    sample.tokens.map(|t| t as i64),
                    cost,
                    sample.occurred_at,
                ],
            )?;
            Ok(())
        })
    }

    pub fn start_turn(
        &self,
        agent_id: &str,
        session_id: &str,
        turn_id: &str,
        provider: &str,
        model: &str,
    ) -> StorageResult<()> {
        let activity_id = format!("turn:{session_id}:{turn_id}");
        let occurred_at = ms_to_iso(now_millis());
        self.db.with_conn(|conn| {
            conn.execute(
                "INSERT OR IGNORE INTO agent_usage_activity \
                   (agent_id, activity_id, session_id, turn_id, provider, model, kind, occurred_at) \
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, 'turn', ?7)",
                params![agent_id, activity_id, session_id, turn_id, provider, model, occurred_at],
            )?;
            Ok(())
        })
    }

    pub fn record_message(
        &self,
        agent_id: &str,
        message: &ConversationMessage,
        provider: &str,
        model: &str,
    ) -> StorageResult<()> {
        self.db
            .with_conn(|conn| record_usage_message(conn, agent_id, message, provider, model))
    }

    pub fn read_turn(
        &self,
        agent_id: &str,
        turn_id: &str,
        since: &str,
    ) -> StorageResult<Vec<TurnModelUsage>> {
        self.db.with_conn(|conn| {
            let mut stmt = conn.prepare(
                "SELECT provider, model, tokens_json FROM agent_usage_records \
                 WHERE agent_id = ?1 AND occurred_at >= ?2 AND turn_id = ?3",
            )?;
            let rows = stmt.query_map(params![agent_id, since, turn_id], |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    r.get::<_, String>(1)?,
                    r.get::<_, String>(2)?,
                ))
            })?;
            let mut order: Vec<(String, String)> = Vec::new();
            let mut map: HashMap<(String, String), UsageTokens> = HashMap::new();
            for row in rows {
                let (provider, model, tj) = row?;
                let toks: UsageTokens = serde_json::from_str(&tj)?;
                let key = (provider.clone(), model.clone());
                let entry = map.entry(key.clone()).or_insert_with(|| {
                    order.push(key);
                    UsageTokens::zero()
                });
                entry.uncached_input = add_nullable(entry.uncached_input, toks.uncached_input);
                entry.cached_input = add_nullable(entry.cached_input, toks.cached_input);
                entry.cache_creation = add_nullable(entry.cache_creation, toks.cache_creation);
                entry.output = add_nullable(entry.output, toks.output);
            }
            Ok(order
                .into_iter()
                .filter_map(|(provider, model)| {
                    let tokens = map.remove(&(provider.clone(), model.clone()))?;
                    Some(TurnModelUsage {
                        provider,
                        model,
                        tokens,
                    })
                })
                .collect())
        })
    }

    pub fn read_turn_totals(
        &self,
        agent_id: &str,
        turn_id: &str,
        since: &str,
    ) -> StorageResult<Option<TurnUsageTotals>> {
        self.db.with_conn(|conn| {
            let mut stmt = conn.prepare(
                "SELECT provider, model, occurred_at, tokens_json, estimated_cost_usd \
                 FROM agent_usage_records \
                 WHERE agent_id = ?1 AND occurred_at >= ?2 AND turn_id = ?3",
            )?;
            let rows = stmt
                .query_map(params![agent_id, since, turn_id], |r| {
                    Ok((
                        r.get::<_, String>(0)?,
                        r.get::<_, String>(1)?,
                        r.get::<_, String>(2)?,
                        r.get::<_, String>(3)?,
                        r.get::<_, Option<f64>>(4)?,
                    ))
                })?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            if rows.is_empty() {
                return Ok(None);
            }
            let mut total_tokens = 0u64;
            let mut cached_input = 0u64;
            let mut cost_usd: Option<f64> = Some(0.0);
            for (provider, model, occurred_at, tj, stored_cost) in rows {
                let counts: UsageTokens = serde_json::from_str(&tj)?;
                total_tokens += counts.sum_present();
                cached_input += counts.cached_input.unwrap_or(0);
                let row_cost = stored_cost.or_else(|| {
                    estimate_usage_cost(&provider, &model, &counts, None, None, &occurred_at).cost
                });
                cost_usd = match (cost_usd, row_cost) {
                    (Some(acc), Some(c)) => Some(acc + c),
                    _ => None,
                };
            }
            Ok(Some(TurnUsageTotals {
                tokens: total_tokens,
                cached_input,
                cost_usd,
            }))
        })
    }

    pub fn read_agent_analytics(
        &self,
        input: &AgentAnalyticsInput,
    ) -> StorageResult<AgentAnalytics> {
        if input.agent_id.is_empty() || utf16_len(&input.agent_id) > 256 {
            return Err(StorageError::Validation(
                "invalid analytics agent_id".to_string(),
            ));
        }
        let host = self.read_host_analytics(&HostAnalyticsInput {
            agent_id: Some(input.agent_id.clone()),
            start_date: input.start_date.clone(),
            end_date: input.end_date.clone(),
            time_zone: input.time_zone.clone(),
        })?;
        Ok(AgentAnalytics {
            agent_id: input.agent_id.clone(),
            start_date: host.start_date,
            end_date: host.end_date,
            time_zone: host.time_zone,
            collection_started_at: host.collection_started_at,
            updated_at: host.updated_at,
            totals: host.totals,
            daily: host.daily,
            models: host.models,
        })
    }

    pub fn read_host_analytics(&self, input: &HostAnalyticsInput) -> StorageResult<HostAnalytics> {
        validate_analytics_input(input)?;
        let start_d = NaiveDate::parse_from_str(&input.start_date, "%Y-%m-%d")
            .map_err(|_| StorageError::Validation("invalid start_date".to_string()))?;
        let end_d = NaiveDate::parse_from_str(&input.end_date, "%Y-%m-%d")
            .map_err(|_| StorageError::Validation("invalid end_date".to_string()))?;
        let start_ms = start_d
            .and_hms_opt(0, 0, 0)
            .expect("midnight")
            .and_utc()
            .timestamp_millis();
        let end_ms = end_d
            .and_hms_opt(0, 0, 0)
            .expect("midnight")
            .and_utc()
            .timestamp_millis();
        let range_start = ms_to_iso(start_ms - 86_400_000);
        let range_end = ms_to_iso(end_ms + 2 * 86_400_000);

        let mut day_list: Vec<String> = Vec::new();
        let mut cur = start_d;
        while cur <= end_d {
            day_list.push(cur.format("%Y-%m-%d").to_string());
            let Some(next) = cur.checked_add_days(Days::new(1)) else {
                break;
            };
            cur = next;
        }

        let collection_started_at = self.collection_started_at()?;
        self.db.with_conn(|conn| {
            let records = query_usage_records_range(
                conn,
                input.agent_id.as_deref(),
                &range_start,
                &range_end,
            )?;
            let activities = query_usage_activity_range(
                conn,
                input.agent_id.as_deref(),
                &range_start,
                &range_end,
            )?;
            let skill_rows = query_skill_loads_range(
                conn,
                input.agent_id.as_deref(),
                &range_start,
                &range_end,
            )?;

            let mut totals = Bucket::default();
            let mut days: BTreeMap<String, Bucket> =
                day_list.iter().map(|d| (d.clone(), Bucket::default())).collect();
            let mut models: BTreeMap<(String, String), Bucket> = BTreeMap::new();
            let mut agents: BTreeMap<String, Bucket> = BTreeMap::new();
            let mut provider_days: BTreeMap<(String, String), AnalyticsProviderDay> =
                BTreeMap::new();
            let mut updated_at: Option<String> = None;

            for rec in records {
                let Some(occ_ms) = parse_iso_ms(&rec.occurred_at) else {
                    continue;
                };
                let date = analytics_date(occ_ms, &input.time_zone)?;
                let Some(day_bucket) = days.get_mut(&date) else {
                    continue;
                };
                if updated_at
                    .as_deref()
                    .is_none_or(|prev| rec.recorded_at.as_str() > prev)
                {
                    updated_at = Some(rec.recorded_at.clone());
                }
                let toks: UsageTokens = serde_json::from_str(&rec.tokens_json)?;
                let cost = rec.estimated_cost_usd.or_else(|| {
                    estimate_usage_cost(
                        &rec.provider,
                        &rec.model,
                        &toks,
                        None,
                        None,
                        &rec.occurred_at,
                    )
                    .cost
                });
                let sess_key = format!("{}\0{}\0{}", rec.agent_id, rec.provider, rec.session_id);
                let turn_key = format!(
                    "{}\0{}\0{}\0{}",
                    rec.agent_id, rec.provider, rec.session_id, rec.turn_id
                );

                let model_bucket = models
                    .entry((rec.provider.clone(), rec.model.clone()))
                    .or_default();
                let agent_bucket = agents.entry(rec.agent_id.clone()).or_default();
                for b in [&mut totals, day_bucket, model_bucket, agent_bucket] {
                    apply_record_to_bucket(b, &toks, cost, &sess_key, &turn_key);
                }

                let cell = provider_days
                    .entry((date.clone(), rec.provider.clone()))
                    .or_insert_with(|| AnalyticsProviderDay {
                        date,
                        provider: rec.provider.clone(),
                        processed_tokens: 0,
                        cached_input: None,
                        estimated_cost_usd: None,
                    });
                cell.processed_tokens += toks.sum_present();
                if let Some(ci) = toks.cached_input {
                    cell.cached_input = Some(cell.cached_input.unwrap_or(0) + ci);
                }
                if let Some(c) = cost {
                    cell.estimated_cost_usd = Some(cell.estimated_cost_usd.unwrap_or(0.0) + c);
                }
            }

            for act in activities {
                let Some(occ_ms) = parse_iso_ms(&act.occurred_at) else {
                    continue;
                };
                let date = analytics_date(occ_ms, &input.time_zone)?;
                let Some(day_bucket) = days.get_mut(&date) else {
                    continue;
                };
                if updated_at
                    .as_deref()
                    .is_none_or(|prev| act.occurred_at.as_str() > prev)
                {
                    updated_at = Some(act.occurred_at.clone());
                }
                let sess_key = format!("{}\0{}\0{}", act.agent_id, act.provider, act.session_id);
                let turn_key = format!(
                    "{}\0{}\0{}\0{}",
                    act.agent_id, act.provider, act.session_id, act.turn_id
                );
                let model_bucket = models
                    .entry((act.provider.clone(), act.model.clone()))
                    .or_default();
                let agent_bucket = agents.entry(act.agent_id.clone()).or_default();
                for b in [&mut totals, day_bucket, model_bucket, agent_bucket] {
                    apply_activity_to_bucket(b, &act.kind, &sess_key, &turn_key);
                }
            }

            let summary = totals.finish();
            let skills = build_skill_analytics(skill_rows, &days, &input.time_zone)?;
            let mut model_rows: Vec<AnalyticsModel> = models
                .into_iter()
                .map(|((provider, model), b)| {
                    let t = b.finish();
                    let share = if summary.processed_tokens > 0 {
                        (t.processed_tokens as f64) / (summary.processed_tokens as f64)
                    } else {
                        0.0
                    };
                    AnalyticsModel {
                        provider,
                        model,
                        share,
                        totals: t,
                    }
                })
                .collect();
            model_rows.sort_by(|a, b| b.totals.processed_tokens.cmp(&a.totals.processed_tokens));

            let mut agent_rows: Vec<AnalyticsAgent> = agents
                .into_iter()
                .map(|(agent_id, b)| {
                    let t = b.finish();
                    let share = if summary.processed_tokens > 0 {
                        (t.processed_tokens as f64) / (summary.processed_tokens as f64)
                    } else {
                        0.0
                    };
                    AnalyticsAgent {
                        agent_id,
                        share,
                        totals: t,
                    }
                })
                .collect();
            agent_rows.sort_by(|a, b| {
                b.totals
                    .processed_tokens
                    .cmp(&a.totals.processed_tokens)
                    .then_with(|| a.agent_id.cmp(&b.agent_id))
            });

            let daily: Vec<AnalyticsDay> = day_list
                .into_iter()
                .map(|date| {
                    let t = days.remove(&date).unwrap_or_default().finish();
                    AnalyticsDay { date, totals: t }
                })
                .collect();

            Ok(HostAnalytics {
                agent_id: input.agent_id.clone(),
                start_date: input.start_date.clone(),
                end_date: input.end_date.clone(),
                time_zone: input.time_zone.clone(),
                collection_started_at,
                updated_at,
                totals: summary,
                daily,
                models: model_rows,
                agents: agent_rows,
                provider_daily: provider_days.into_values().collect(),
                skills,
            })
        })
    }
}

pub fn record_usage_message(
    conn: &Connection,
    agent_id: &str,
    message: &ConversationMessage,
    provider: &str,
    model: &str,
) -> StorageResult<()> {
    let kind = match message.author {
        ConversationMessageAuthor::User => "user",
        ConversationMessageAuthor::Assistant => {
            if message.status != ConversationMessageStatus::Completed {
                return Ok(());
            }
            if let Some(it) = message.item_type.as_deref()
                && it != "agentMessage"
                && it != "final_answer"
            {
                return Ok(());
            }
            "assistant"
        }
        _ => return Ok(()),
    };
    let activity_id = format!("message:{}", message.id);
    let turn_id = message.turn_id.as_deref().unwrap_or("");
    conn.execute(
        "INSERT OR IGNORE INTO agent_usage_activity \
           (agent_id, activity_id, session_id, turn_id, provider, model, kind, occurred_at) \
         SELECT ?1, ?2, '', ?3, ?4, ?5, ?6, ?7 \
         WHERE ?8 >= (SELECT applied_at FROM schema_migrations WHERE version = 15)",
        params![
            agent_id,
            activity_id,
            turn_id,
            provider,
            model,
            kind,
            message.created_at,
            message.created_at,
        ],
    )?;
    Ok(())
}

fn add_nullable(a: Option<u64>, b: Option<u64>) -> Option<u64> {
    match (a, b) {
        (Some(x), Some(y)) => Some(x + y),
        _ => None,
    }
}

fn monotonic_pair(
    cur: &mut Option<u64>,
    prior: Option<Option<u64>>,
    has_before: bool,
) -> Option<u64> {
    let current = *cur;
    let delta = match (current, has_before, prior) {
        (None, _, _) | (_, _, Some(None)) => None,
        (Some(c), _, None) => Some(c),
        (Some(c), _, Some(Some(p))) => Some(c.saturating_sub(p)),
    };
    if let Some(Some(p)) = prior {
        *cur = Some(current.map_or(p, |c| c.max(p)));
    }
    delta
}

fn compute_monotonic_delta(
    tokens: &mut UsageTokens,
    before: Option<&UsageTokens>,
) -> UsageTokens {
    let hb = before.is_some();
    UsageTokens {
        uncached_input: monotonic_pair(
            &mut tokens.uncached_input,
            before.map(|b| b.uncached_input),
            hb,
        ),
        cached_input: monotonic_pair(
            &mut tokens.cached_input,
            before.map(|b| b.cached_input),
            hb,
        ),
        cache_creation: monotonic_pair(
            &mut tokens.cache_creation,
            before.map(|b| b.cache_creation),
            hb,
        ),
        output: monotonic_pair(&mut tokens.output, before.map(|b| b.output), hb),
    }
}

fn validate_analytics_input(input: &HostAnalyticsInput) -> StorageResult<()> {
    if input.time_zone.is_empty()
        || utf16_len(&input.time_zone) > 100
        || !is_valid_timezone(&input.time_zone)
    {
        return Err(StorageError::Validation(
            "invalid analytics time zone".to_string(),
        ));
    }
    if !is_calendar_date(&input.start_date) || !is_calendar_date(&input.end_date) {
        return Err(StorageError::Validation("invalid analytics date".to_string()));
    }
    if input.start_date > input.end_date {
        return Err(StorageError::Validation(
            "analytics start_date must be <= end_date".to_string(),
        ));
    }
    let sd = NaiveDate::parse_from_str(&input.start_date, "%Y-%m-%d")
        .map_err(|_| StorageError::Validation("invalid analytics start_date".to_string()))?;
    let ed = NaiveDate::parse_from_str(&input.end_date, "%Y-%m-%d")
        .map_err(|_| StorageError::Validation("invalid analytics end_date".to_string()))?;
    if (ed - sd).num_days() > 366 {
        return Err(StorageError::Validation(
            "analytics range must be at most 367 days".to_string(),
        ));
    }
    if let Some(aid) = &input.agent_id
        && (aid.is_empty() || utf16_len(aid) > 256)
    {
        return Err(StorageError::Validation(
            "invalid analytics agent filter".to_string(),
        ));
    }
    Ok(())
}

#[derive(Default)]
struct Bucket {
    values: AnalyticsTotals,
    sessions: HashSet<String>,
    turns: HashSet<String>,
    usage_turns: HashSet<String>,
}

impl Bucket {
    fn finish(mut self) -> AnalyticsTotals {
        let missing = self
            .turns
            .iter()
            .filter(|t| !self.usage_turns.contains(*t))
            .count();
        self.values.sessions = self.sessions.len();
        self.values.turns = self.turns.len();
        self.values.missing_usage_turns = missing;
        self.values
    }
}

fn add_opt_token(slot: &mut Option<u64>, processed: &mut u64, value: Option<u64>) {
    if let Some(v) = value {
        *slot = Some(slot.unwrap_or(0) + v);
        *processed += v;
    }
}

fn apply_record_to_bucket(
    b: &mut Bucket,
    toks: &UsageTokens,
    cost: Option<f64>,
    sess_key: &str,
    turn_key: &str,
) {
    if toks.any_present() {
        b.usage_turns.insert(turn_key.to_string());
    }
    b.sessions.insert(sess_key.to_string());
    add_opt_token(
        &mut b.values.uncached_input,
        &mut b.values.processed_tokens,
        toks.uncached_input,
    );
    add_opt_token(
        &mut b.values.cached_input,
        &mut b.values.processed_tokens,
        toks.cached_input,
    );
    add_opt_token(
        &mut b.values.cache_creation,
        &mut b.values.processed_tokens,
        toks.cache_creation,
    );
    add_opt_token(
        &mut b.values.output,
        &mut b.values.processed_tokens,
        toks.output,
    );
    if toks.any_missing() {
        b.values.incomplete_records += 1;
    }
    if let Some(c) = cost {
        b.values.estimated_cost_usd = Some(b.values.estimated_cost_usd.unwrap_or(0.0) + c);
    } else {
        b.values.unpriced_records += 1;
    }
}

fn apply_activity_to_bucket(b: &mut Bucket, kind: &str, sess_key: &str, turn_key: &str) {
    match kind {
        "turn" => {
            b.turns.insert(turn_key.to_string());
            b.sessions.insert(sess_key.to_string());
        }
        "user" => b.values.user_messages += 1,
        _ => b.values.assistant_messages += 1,
    }
}

struct RawUsageRecord {
    agent_id: String,
    session_id: String,
    turn_id: String,
    provider: String,
    model: String,
    occurred_at: String,
    tokens_json: String,
    estimated_cost_usd: Option<f64>,
    recorded_at: String,
}

struct RawUsageActivity {
    agent_id: String,
    session_id: String,
    turn_id: String,
    provider: String,
    model: String,
    kind: String,
    occurred_at: String,
}

struct RawSkillLoadRow {
    agent_id: String,
    provider: String,
    model: String,
    skill: String,
    tokens: Option<i64>,
    estimated_cost_usd: Option<f64>,
    occurred_at: String,
}

fn build_skill_analytics(
    rows: Vec<RawSkillLoadRow>,
    days: &BTreeMap<String, Bucket>,
    time_zone: &str,
) -> StorageResult<Vec<AnalyticsSkill>> {
    let mut map: HashMap<String, (AnalyticsSkill, HashSet<String>)> = HashMap::new();
    let mut total_skill_tokens = 0u64;
    for r in rows {
        let Some(occ_ms) = parse_iso_ms(&r.occurred_at) else {
            continue;
        };
        let d = analytics_date(occ_ms, time_zone)?;
        if !days.contains_key(&d) {
            continue;
        }
        let (entry, agents) = map.entry(r.skill.clone()).or_insert_with(|| {
            (
                AnalyticsSkill {
                    skill: r.skill.clone(),
                    loads: 0,
                    tokens: 0,
                    estimated_cost_usd: None,
                    agents: 0,
                    share: 0.0,
                },
                HashSet::new(),
            )
        });
        entry.loads += 1;
        let load_toks = r.tokens.map(|t| t.max(0) as u64);
        let t_val = load_toks.unwrap_or(0);
        entry.tokens += t_val;
        total_skill_tokens += t_val;
        let cost = match load_toks {
            None => None,
            Some(lt) => r.estimated_cost_usd.or_else(|| {
                estimate_usage_cost(
                    &r.provider,
                    &r.model,
                    &UsageTokens {
                        uncached_input: Some(lt),
                        cached_input: Some(0),
                        cache_creation: Some(0),
                        output: Some(0),
                    },
                    None,
                    None,
                    &r.occurred_at,
                )
                .cost
            }),
        };
        if let Some(c) = cost {
            entry.estimated_cost_usd = Some(entry.estimated_cost_usd.unwrap_or(0.0) + c);
        }
        agents.insert(r.agent_id);
    }
    let mut out: Vec<AnalyticsSkill> = map
        .into_values()
        .map(|(mut row, loaded_by)| {
            row.agents = loaded_by.len();
            row.share = if total_skill_tokens > 0 {
                (row.tokens as f64) / (total_skill_tokens as f64)
            } else {
                0.0
            };
            row
        })
        .collect();
    out.sort_by(|a, b| {
        b.tokens
            .cmp(&a.tokens)
            .then_with(|| b.loads.cmp(&a.loads))
            .then_with(|| a.skill.cmp(&b.skill))
    });
    Ok(out)
}

fn query_usage_records_range(
    conn: &Connection,
    agent_id: Option<&str>,
    start: &str,
    end: &str,
) -> StorageResult<Vec<RawUsageRecord>> {
    let map_fn = |r: &rusqlite::Row<'_>| {
        Ok(RawUsageRecord {
            agent_id: r.get(0)?,
            session_id: r.get(1)?,
            turn_id: r.get(2)?,
            provider: r.get(3)?,
            model: r.get(4)?,
            occurred_at: r.get(5)?,
            tokens_json: r.get(6)?,
            estimated_cost_usd: r.get(7)?,
            recorded_at: r.get(8)?,
        })
    };
    let cols = "agent_id, session_id, turn_id, provider, model, occurred_at, tokens_json, \
                estimated_cost_usd, recorded_at";
    if let Some(aid) = agent_id {
        let mut stmt = conn.prepare(&format!(
            "SELECT {cols} FROM agent_usage_records \
             WHERE agent_id = ?1 AND occurred_at >= ?2 AND occurred_at < ?3"
        ))?;
        let rows = stmt.query_map(params![aid, start, end], map_fn)?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    } else {
        let mut stmt = conn.prepare(&format!(
            "SELECT {cols} FROM agent_usage_records WHERE occurred_at >= ?1 AND occurred_at < ?2"
        ))?;
        let rows = stmt.query_map(params![start, end], map_fn)?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    }
}

fn query_usage_activity_range(
    conn: &Connection,
    agent_id: Option<&str>,
    start: &str,
    end: &str,
) -> StorageResult<Vec<RawUsageActivity>> {
    let map_fn = |r: &rusqlite::Row<'_>| {
        Ok(RawUsageActivity {
            agent_id: r.get(0)?,
            session_id: r.get(1)?,
            turn_id: r.get(2)?,
            provider: r.get(3)?,
            model: r.get(4)?,
            kind: r.get(5)?,
            occurred_at: r.get(6)?,
        })
    };
    let cols = "agent_id, session_id, turn_id, provider, model, kind, occurred_at";
    if let Some(aid) = agent_id {
        let mut stmt = conn.prepare(&format!(
            "SELECT {cols} FROM agent_usage_activity \
             WHERE agent_id = ?1 AND occurred_at >= ?2 AND occurred_at < ?3"
        ))?;
        let rows = stmt.query_map(params![aid, start, end], map_fn)?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    } else {
        let mut stmt = conn.prepare(&format!(
            "SELECT {cols} FROM agent_usage_activity WHERE occurred_at >= ?1 AND occurred_at < ?2"
        ))?;
        let rows = stmt.query_map(params![start, end], map_fn)?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    }
}

fn query_skill_loads_range(
    conn: &Connection,
    agent_id: Option<&str>,
    start: &str,
    end: &str,
) -> StorageResult<Vec<RawSkillLoadRow>> {
    let map_fn = |r: &rusqlite::Row<'_>| {
        Ok(RawSkillLoadRow {
            agent_id: r.get(0)?,
            provider: r.get(1)?,
            model: r.get(2)?,
            skill: r.get(3)?,
            tokens: r.get(4)?,
            estimated_cost_usd: r.get(5)?,
            occurred_at: r.get(6)?,
        })
    };
    let cols = "agent_id, provider, model, skill, tokens, estimated_cost_usd, occurred_at";
    if let Some(aid) = agent_id {
        let mut stmt = conn.prepare(&format!(
            "SELECT {cols} FROM agent_usage_skill_loads \
             WHERE agent_id = ?1 AND occurred_at >= ?2 AND occurred_at < ?3"
        ))?;
        let rows = stmt.query_map(params![aid, start, end], map_fn)?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    } else {
        let mut stmt = conn.prepare(&format!(
            "SELECT {cols} FROM agent_usage_skill_loads WHERE occurred_at >= ?1 AND occurred_at < ?2"
        ))?;
        let rows = stmt.query_map(params![start, end], map_fn)?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    }
}
