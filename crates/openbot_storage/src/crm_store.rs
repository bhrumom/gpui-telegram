//! 13-table SQLite CRM persistence (`CrmStore`, `CrmWrite`, `CrmSourceStore`, `CrmAgentView`).
//! Faithful 1:1 port of `src/backend/crm/{crm-store,crm-source-store,crm-agent-view}.ts`
//! over `openbot.db` schema `v69`.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::fmt::Write as _;
use std::sync::Arc;

use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use chrono::{Datelike, NaiveDate, TimeZone, Utc};
use rusqlite::types::Value as SqlValue;
use rusqlite::{Connection, OptionalExtension, params, params_from_iter};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::database::OpenBotDatabase;
use crate::error::{StorageError, StorageResult};
use crate::workflow_store::{ms_to_iso, now_millis, parse_iso_ms};
use openbot_contracts::crm::{
    CRM_LIMITS, CRM_PRIVATE_FIELDS, CRM_TEXT_FIELDS, CrmActivity, CrmActivityDetailValue, CrmActor,
    CrmActorKind, CrmApplyResult, CrmChange, CrmConflictReason, CrmCustomer, CrmCustomerInput,
    CrmCustomerPatch, CrmEvery, CrmMethod, CrmOverview, CrmPayment, CrmPaymentInput,
    CrmPaymentPatch, CrmPaymentState, CrmPlan, CrmPlanInput, CrmPlanPatch, CrmPrivateField,
    CrmProposal, CrmProposedChange, CrmSource, CrmSourceError, CrmSourceKind, CrmSourceSkipped,
    CrmSourceSyncState, CrmSplitPart, CrmStatus, CrmTag, CrmTone,
};
use openbot_contracts::limits::utf16_len;
use openbot_contracts::mentions::fold_mention_text;

pub const AGENT_PENDING_PROPOSALS_LIMIT: usize = 50;
pub const DEFAULT_TOTAL_PENDING_PROPOSALS_LIMIT: usize = 200;
pub const CRM_ACTIVITY_PAGE_SIZE: usize = 100;

const CRM_TONE_CYCLE: [CrmTone; 6] = [
    CrmTone::InfoLight,
    CrmTone::Secondary,
    CrmTone::Outline,
    CrmTone::WarningLight,
    CrmTone::SuccessLight,
    CrmTone::PrimaryLight,
];

/// Generates an unbiased 128-bit (`32`-char lowercase hex) CRM entity identifier.
pub fn new_crm_id() -> String {
    let a = Uuid::new_v4().into_bytes();
    let b = Uuid::new_v4().into_bytes();
    let mut out = String::with_capacity(32);
    for i in 0..16 {
        let _ = write!(&mut out, "{:02x}", a[i] ^ b[i]);
    }
    out
}

/// NFKD compatibility + diacritic-mark stripping + lowercase folding for `crm_customers.search_text`.
pub fn normalize_crm_search(value: &str) -> String {
    let mut compat = String::with_capacity(value.len());
    for ch in value.chars() {
        let code = ch as u32;
        if (0xFF01..=0xFF5E).contains(&code) {
            compat.push(char::from_u32(code - 0xFEE0).unwrap_or(ch));
        } else {
            match ch {
                '\u{00A0}' | '\u{3000}' => compat.push(' '),
                'ﬀ' => compat.push_str("ff"),
                'ﬁ' => compat.push_str("fi"),
                'ﬂ' => compat.push_str("fl"),
                'ﬃ' => compat.push_str("ffi"),
                'ﬄ' => compat.push_str("ffl"),
                '¹' => compat.push('1'),
                '²' => compat.push('2'),
                '³' => compat.push('3'),
                _ => compat.push(ch),
            }
        }
    }
    fold_mention_text(&compat)
}

pub fn local_crm_date_from_ms(ms: i64) -> String {
    let dt = Utc
        .timestamp_millis_opt(ms)
        .single()
        .unwrap_or_else(Utc::now);
    format!("{:04}-{:02}-{:02}", dt.year(), dt.month(), dt.day())
}

pub fn add_crm_months(date: &str, months: i32) -> Option<String> {
    if date.len() != 10 {
        return None;
    }
    let year: i32 = date[0..4].parse().ok()?;
    let month: i32 = date[5..7].parse().ok()?;
    let day: u32 = date[8..10].parse().ok()?;
    let total = year.checked_mul(12)?.checked_add(month - 1)?.checked_add(months)?;
    if total < 0 {
        return None;
    }
    let next_year = total.div_euclid(12);
    let next_month = (total.rem_euclid(12) + 1) as u32;
    if !(1..=9999).contains(&next_year) {
        return None;
    }
    let last_day = days_in_month(next_year, next_month);
    let clamped_day = day.min(last_day);
    Some(format!("{next_year:04}-{next_month:02}-{clamped_day:02}"))
}

fn days_in_month(year: i32, month: u32) -> u32 {
    let (ny, nm) = if month == 12 {
        (year + 1, 1)
    } else {
        (year, month + 1)
    };
    NaiveDate::from_ymd_opt(ny, nm, 1)
        .and_then(|d| d.pred_opt())
        .map_or(28, |d| d.day())
}

pub fn encode_crm_number_cursor(key: &str, number: u64) -> String {
    let raw = serde_json::to_string(&serde_json::json!([key, number])).unwrap_or_default();
    URL_SAFE_NO_PAD.encode(raw.as_bytes())
}

pub fn encode_crm_text_cursor(key: &str, text: &str) -> String {
    let raw = serde_json::to_string(&serde_json::json!([key, text])).unwrap_or_default();
    URL_SAFE_NO_PAD.encode(raw.as_bytes())
}

pub fn decode_crm_number_cursor(cursor: &str) -> StorageResult<(String, u64)> {
    let bytes = decode_base64url(cursor)?;
    let arr: serde_json::Value = serde_json::from_slice(&bytes)
        .map_err(|_| StorageError::Validation("error.crm.badCursor".to_string()))?;
    let Some(items) = arr.as_array() else {
        return Err(StorageError::Validation("error.crm.badCursor".to_string()));
    };
    if items.len() != 2 {
        return Err(StorageError::Validation("error.crm.badCursor".to_string()));
    }
    let key = items[0]
        .as_str()
        .ok_or_else(|| StorageError::Validation("error.crm.badCursor".to_string()))?;
    let num = items[1]
        .as_u64()
        .ok_or_else(|| StorageError::Validation("error.crm.badCursor".to_string()))?;
    Ok((key.to_string(), num))
}

pub fn decode_crm_text_cursor(cursor: &str) -> StorageResult<(String, String)> {
    let bytes = decode_base64url(cursor)?;
    let arr: serde_json::Value = serde_json::from_slice(&bytes)
        .map_err(|_| StorageError::Validation("error.crm.badCursor".to_string()))?;
    let Some(items) = arr.as_array() else {
        return Err(StorageError::Validation("error.crm.badCursor".to_string()));
    };
    if items.len() != 2 {
        return Err(StorageError::Validation("error.crm.badCursor".to_string()));
    }
    let k = items[0]
        .as_str()
        .ok_or_else(|| StorageError::Validation("error.crm.badCursor".to_string()))?;
    let v = items[1]
        .as_str()
        .ok_or_else(|| StorageError::Validation("error.crm.badCursor".to_string()))?;
    Ok((k.to_string(), v.to_string()))
}

fn decode_base64url(cursor: &str) -> StorageResult<Vec<u8>> {
    let trimmed = cursor.trim_end_matches('=');
    URL_SAFE_NO_PAD
        .decode(trimmed)
        .map_err(|_| StorageError::Validation("error.crm.badCursor".to_string()))
}

/// ISO-4217 / CLDR fraction digits matching V8 `Intl.NumberFormat("en", { style: "currency", currency })`.
pub fn crm_currency_exponent(currency: &str) -> usize {
    match currency.to_ascii_uppercase().as_str() {
        "BIF" | "CLP" | "DJF" | "GNF" | "ISK" | "JPY" | "KMF" | "KRW" | "PYG" | "RWF" | "UGX"
        | "UYI" | "VND" | "VUV" | "XAF" | "XOF" | "XPF" => 0,
        "BHD" | "IQD" | "JOD" | "KWD" | "LYD" | "OMR" | "TND" => 3,
        "CLF" | "UYW" => 4,
        _ => 2,
    }
}

pub fn format_crm_amount(minor: i64, currency: &str) -> String {
    let exp = crm_currency_exponent(currency);
    let raw = minor.unsigned_abs().to_string();
    let padded = if raw.len() < exp + 1 {
        format!("{}{raw}", "0".repeat(exp + 1 - raw.len()))
    } else {
        raw
    };
    let sign = if minor < 0 { "-" } else { "" };
    if exp == 0 {
        format!("{sign}{padded} {currency}")
    } else {
        let split = padded.len() - exp;
        format!("{sign}{}.{} {currency}", &padded[..split], &padded[split..])
    }
}

pub fn parse_crm_amount(text: &str, currency: &str) -> StorageResult<i64> {
    let trimmed = text.trim();
    let (amount_part, suffix_ccy) = match trimmed.rsplit_once(char::is_whitespace) {
        Some((left, right))
            if right.len() == 3 && right.bytes().all(|b| b.is_ascii_alphabetic()) =>
        {
            (left.trim(), Some(right.to_ascii_uppercase()))
        }
        _ => (trimmed, None),
    };
    if amount_part.chars().any(char::is_whitespace) {
        return Err(StorageError::Validation("error.crm.amountInvalid".to_string()));
    }
    if let Some(ccy) = suffix_ccy
        && ccy != currency
    {
        return Err(StorageError::Validation("error.crm.amountInvalid".to_string()));
    }
    let (raw_whole, raw_frac) = match amount_part.split_once('.') {
        Some((w, f)) => {
            if f.contains('.') || !f.bytes().all(|b| b.is_ascii_digit()) {
                return Err(StorageError::Validation("error.crm.amountInvalid".to_string()));
            }
            (w, f)
        }
        None => (amount_part, ""),
    };
    let whole = strip_valid_thousands_commas(raw_whole)
        .ok_or_else(|| StorageError::Validation("error.crm.amountInvalid".to_string()))?;
    let exp = crm_currency_exponent(currency);
    let trimmed_frac = raw_frac.trim_end_matches('0');
    if trimmed_frac.len() > exp {
        return Err(StorageError::Validation(format!(
            "error.crm.amountDecimals:{currency}"
        )));
    }
    let mut frac_padded = trimmed_frac.to_string();
    while frac_padded.len() < exp {
        frac_padded.push('0');
    }
    let combined = format!("{whole}{frac_padded}");
    let normalized = combined.trim_start_matches('0');
    let digits = if normalized.is_empty() { "0" } else { normalized };
    if digits.len() > 13 {
        return Err(StorageError::Validation("error.crm.amountInvalid".to_string()));
    }
    let parsed: i64 = digits
        .parse()
        .map_err(|_| StorageError::Validation("error.crm.amountInvalid".to_string()))?;
    if parsed > CRM_LIMITS.amount_minor {
        return Err(StorageError::Validation("error.crm.amountInvalid".to_string()));
    }
    Ok(parsed)
}

fn strip_valid_thousands_commas(raw: &str) -> Option<String> {
    if raw.is_empty() {
        return None;
    }
    if !raw.contains(',') {
        return raw.bytes().all(|b| b.is_ascii_digit()).then(|| raw.to_string());
    }
    let groups: Vec<&str> = raw.split(',').collect();
    let first = groups.first().copied()?;
    if !(1..=3).contains(&first.len()) || !first.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    for g in &groups[1..] {
        if g.len() != 3 || !g.bytes().all(|b| b.is_ascii_digit()) {
            return None;
        }
    }
    Some(groups.concat())
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct CrmCustomerText {
    pub name: String,
    pub contact_name: String,
    pub contact_email: String,
    pub phone: String,
    pub billing_email: String,
    pub domain: String,
    pub tax_id: String,
    pub address: String,
    pub stripe_customer_id: String,
    pub notes: String,
}

impl CrmCustomerText {
    pub fn get(&self, field: &str) -> &str {
        match field {
            "name" => &self.name,
            "contactName" => &self.contact_name,
            "contactEmail" => &self.contact_email,
            "phone" => &self.phone,
            "billingEmail" => &self.billing_email,
            "domain" => &self.domain,
            "taxId" => &self.tax_id,
            "address" => &self.address,
            "stripeCustomerId" => &self.stripe_customer_id,
            "notes" => &self.notes,
            _ => "",
        }
    }

    pub fn set(&mut self, field: &str, value: String) {
        match field {
            "name" => self.name = value,
            "contactName" => self.contact_name = value,
            "contactEmail" => self.contact_email = value,
            "phone" => self.phone = value,
            "billingEmail" => self.billing_email = value,
            "domain" => self.domain = value,
            "taxId" => self.tax_id = value,
            "address" => self.address = value,
            "stripeCustomerId" => self.stripe_customer_id = value,
            "notes" => self.notes = value,
            _ => {}
        }
    }

    pub fn search_text_json(&self) -> StorageResult<String> {
        let mut map = serde_json::Map::new();
        for field in CRM_TEXT_FIELDS {
            map.insert(
                field.to_string(),
                serde_json::Value::String(normalize_crm_search(self.get(field))),
            );
        }
        Ok(serde_json::to_string(&serde_json::Value::Object(map))?)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CrmCustomerRecord {
    pub id: String,
    pub number: u64,
    pub text: CrmCustomerText,
    pub status_id: String,
    pub owner_agent_id: Option<String>,
    pub method: Option<CrmMethod>,
    pub currency: String,
    pub created_at: String,
    pub updated_at: String,
    pub revision: u64,
    pub source_id: Option<String>,
    pub source_ref: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CrmStatusRecord {
    pub id: String,
    pub label: Option<String>,
    pub tone: CrmTone,
    pub position: i64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CrmPaymentRecord {
    pub payment: CrmPayment,
    pub customer_number: u64,
    pub customer_name: String,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CrmCustomerPageInput {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub after: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CrmCustomerPage {
    pub customers: Vec<CrmCustomer>,
    pub next: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CrmPaymentPageInput {
    pub state: CrmPaymentState,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub customer_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub after: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CrmPaymentPage {
    pub payments: Vec<CrmPayment>,
    pub next: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CrmActivityPageInput {
    pub customer_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub after: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CrmActivityPage {
    pub activity: Vec<CrmActivity>,
    pub next: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CrmProposeInput {
    pub agent_id: String,
    pub customer_id: Option<String>,
    pub thread_id: Option<String>,
    pub reason: String,
    pub change: CrmProposedChange,
}

pub(crate) const CRM_CUSTOMER_SELECT_COLS: &str = "\
  c.customer_id, c.number, c.name, c.status_id, c.owner_agent_id, c.method, c.currency, \
  c.contact_name, c.contact_email, c.phone, c.billing_email, c.domain, c.tax_id, c.address, \
  c.stripe_customer_id, c.notes, c.source_id, c.source_ref, c.created_at, c.updated_at, c.revision, \
  (SELECT json_group_array(tag_id) FROM (SELECT ct.tag_id FROM crm_customer_tags AS ct \
    JOIN crm_tags AS t ON t.tag_id = ct.tag_id WHERE ct.customer_id = c.customer_id ORDER BY t.label, t.tag_id)) AS tag_ids, \
  (SELECT json_group_array(json_object('plan_id', plan_id, 'customer_id', customer_id, 'name', name, \
    'amount_minor', amount_minor, 'every', every, 'starts_on', starts_on, 'ends_on', ends_on, 'source_kind', source_kind)) \
    FROM (SELECT p.*, (SELECT s.kind FROM crm_sources AS s WHERE s.source_id = p.source_id) AS source_kind \
      FROM crm_plans AS p WHERE p.customer_id = c.customer_id \
      ORDER BY p.starts_on, p.created_at, p.plan_id)) AS plans, \
  (SELECT s.kind FROM crm_sources AS s WHERE s.source_id = c.source_id) AS source_kind, \
  (SELECT coalesce(sum(amount_minor), 0) FROM crm_payments \
    WHERE customer_id = c.customer_id AND paid_at IS NOT NULL) AS paid_minor, \
  (SELECT coalesce(sum(amount_minor), 0) FROM crm_payments \
    WHERE customer_id = c.customer_id AND paid_at IS NULL) AS open_minor, \
  (SELECT coalesce(sum(amount_minor), 0) FROM crm_payments \
    WHERE customer_id = c.customer_id AND paid_at IS NULL AND due_on < ?1) AS late_minor, \
  (SELECT min(due_on) FROM crm_payments WHERE customer_id = c.customer_id AND paid_at IS NULL) AS next_due_on, \
  (SELECT max(paid_at) FROM crm_payments WHERE customer_id = c.customer_id AND paid_at IS NOT NULL) AS last_paid_at";

pub(crate) const CRM_PAYMENT_SELECT_SQL: &str = "\
  SELECT p.payment_id, p.number, p.customer_id, p.plan_id, p.label, p.amount_minor, \
         c.currency, p.due_on, p.paid_at, p.method, \
         (SELECT s.kind FROM crm_sources AS s WHERE s.source_id = p.source_id) AS source_kind, \
         c.number AS customer_number, c.name AS customer_name \
  FROM crm_payments AS p JOIN crm_customers AS c ON c.customer_id = p.customer_id";

type ClockFn = Arc<dyn Fn() -> i64 + Send + Sync>;

#[derive(Clone)]
pub struct CrmStore {
    db: OpenBotDatabase,
    now_fn: ClockFn,
    per_agent_proposal_limit: usize,
    total_proposal_limit: usize,
}

impl CrmStore {
    pub fn new(db: OpenBotDatabase) -> Self {
        Self {
            db,
            now_fn: Arc::new(now_millis),
            per_agent_proposal_limit: AGENT_PENDING_PROPOSALS_LIMIT,
            total_proposal_limit: DEFAULT_TOTAL_PENDING_PROPOSALS_LIMIT,
        }
    }

    pub fn with_clock(db: OpenBotDatabase, now_fn: ClockFn) -> Self {
        Self {
            db,
            now_fn,
            per_agent_proposal_limit: AGENT_PENDING_PROPOSALS_LIMIT,
            total_proposal_limit: DEFAULT_TOTAL_PENDING_PROPOSALS_LIMIT,
        }
    }

    pub fn with_proposal_limits(mut self, per_agent: usize, total: usize) -> Self {
        self.per_agent_proposal_limit = per_agent.max(1);
        self.total_proposal_limit = total.clamp(1, CRM_LIMITS.pending_proposals);
        self
    }

    pub fn database(&self) -> &OpenBotDatabase {
        &self.db
    }

    pub fn today(&self) -> String {
        local_crm_date_from_ms((self.now_fn)())
    }

    pub fn timestamp(&self) -> String {
        ms_to_iso((self.now_fn)())
    }

    pub fn customer_record(&self, customer_id: &str) -> StorageResult<CrmCustomerRecord> {
        self.db
            .with_conn(|conn| customer_record_on_conn(conn, customer_id))
    }

    pub fn customer(&self, customer_id: &str) -> StorageResult<CrmCustomer> {
        let today = self.today();
        self.db.with_conn(|conn| {
            conn.query_row(
                &format!("SELECT {CRM_CUSTOMER_SELECT_COLS} FROM crm_customers AS c WHERE c.customer_id = ?2"),
                params![today, customer_id],
                read_crm_customer_row,
            )
            .optional()?
            .transpose()?
            .ok_or_else(|| StorageError::NotFound("error.crm.customerMissing".to_string()))
        })
    }

    pub fn plan(&self, plan_id: &str) -> StorageResult<CrmPlan> {
        self.db.with_conn(|conn| plan_on_conn(conn, plan_id))
    }

    pub fn payment(&self, payment_id: &str) -> StorageResult<CrmPaymentRecord> {
        self.db.with_conn(|conn| payment_on_conn(conn, payment_id))
    }

    pub fn status(&self, status_id: &str) -> StorageResult<CrmStatusRecord> {
        self.db.with_conn(|conn| status_on_conn(conn, status_id))
    }

    pub fn tag(&self, tag_id: &str) -> StorageResult<CrmTag> {
        self.db.with_conn(|conn| tag_on_conn(conn, tag_id))
    }

    pub fn statuses(&self) -> StorageResult<Vec<CrmStatusRecord>> {
        self.db.with_conn(statuses_on_conn)
    }

    pub fn tags(&self) -> StorageResult<Vec<CrmTag>> {
        self.db.with_conn(tags_on_conn)
    }

    pub fn private_fields(&self) -> StorageResult<Vec<CrmPrivateField>> {
        self.db.with_conn(|conn| {
            let mut stmt = conn.prepare("SELECT field FROM crm_private_fields")?;
            let rows = stmt
                .query_map([], |r| r.get::<_, String>(0))?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            let mut set = HashSet::new();
            for raw in rows {
                if let Some(f) = parse_private_field(&raw) {
                    set.insert(f);
                }
            }
            Ok(CRM_PRIVATE_FIELDS
                .into_iter()
                .filter(|f| set.contains(f))
                .collect())
        })
    }

    pub fn sources(&self) -> StorageResult<Vec<CrmSource>> {
        self.db.with_conn(|conn| {
            let mut stmt = conn.prepare(
                "SELECT source_id, kind, test_mode, account_label, key_hint, state, error_code, \
                        synced_at, skipped_json, connected_at \
                 FROM crm_sources ORDER BY kind",
            )?;
            let rows = stmt.query_map([], read_crm_source_row)?;
            let mut out = Vec::new();
            for r in rows {
                out.push(r??);
            }
            Ok(out)
        })
    }

    pub fn source(&self, source_id: &str) -> StorageResult<CrmSource> {
        self.db.with_conn(|conn| source_on_conn(conn, source_id))
    }

    pub fn source_owns(&self, change: &CrmProposedChange) -> StorageResult<bool> {
        self.db
            .with_conn(|conn| source_owns_on_conn(conn, change))
    }

    pub fn has_agent_access(&self, agent_id: &str) -> StorageResult<bool> {
        self.db.with_conn(|conn| {
            let found: Option<i64> = conn
                .query_row(
                    "SELECT 1 FROM crm_agent_access WHERE agent_id = ?1",
                    params![agent_id],
                    |r| r.get(0),
                )
                .optional()?;
            Ok(found.is_some())
        })
    }

    pub fn tag_ids_of(&self, customer_id: &str) -> StorageResult<Vec<String>> {
        self.db
            .with_conn(|conn| tag_ids_of_on_conn(conn, customer_id))
    }

    pub fn overview(&self) -> StorageResult<CrmOverview> {
        let statuses = self
            .statuses()?
            .into_iter()
            .map(|s| CrmStatus {
                id: s.id,
                label: s.label,
                tone: s.tone,
            })
            .collect();
        let tags = self.tags()?;
        let private_fields = self.private_fields()?;
        let sources = self.sources()?;
        self.db.with_conn(|conn| {
            let mut p_stmt = conn.prepare(&format!(
                "SELECT proposal_id, agent_id, customer_id, thread_id, reason, change_json, created_at \
                 FROM crm_proposals WHERE state = 'pending' \
                 ORDER BY created_at DESC, proposal_id DESC LIMIT {}",
                CRM_LIMITS.pending_proposals
            ))?;
            let p_rows = p_stmt.query_map([], |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    r.get::<_, String>(1)?,
                    r.get::<_, Option<String>>(2)?,
                    r.get::<_, Option<String>>(3)?,
                    r.get::<_, String>(4)?,
                    r.get::<_, String>(5)?,
                    r.get::<_, String>(6)?,
                ))
            })?;
            let mut proposals = Vec::new();
            for r in p_rows {
                let (id, agent_id, customer_id, thread_id, reason, change_json, created_at) = r?;
                if let Ok(change) = serde_json::from_str::<CrmProposedChange>(&change_json)
                    && change.is_valid()
                {
                    proposals.push(CrmProposal {
                        id,
                        agent_id,
                        customer_id,
                        thread_id,
                        reason,
                        change,
                        created_at,
                    });
                }
            }
            let mut a_stmt =
                conn.prepare("SELECT agent_id FROM crm_agent_access ORDER BY granted_at, agent_id")?;
            let agent_ids = a_stmt
                .query_map([], |r| r.get::<_, String>(0))?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            Ok(CrmOverview {
                statuses,
                tags,
                private_fields,
                agent_ids,
                proposals,
                sources,
            })
        })
    }

    pub fn customers(&self, input: &CrmCustomerPageInput) -> StorageResult<CrmCustomerPage> {
        let today = self.today();
        let after = input.after.unwrap_or(0) as i64;
        let limit = CRM_LIMITS.page;
        self.db.with_conn(|conn| {
            let mut stmt = conn.prepare(&format!(
                "SELECT {CRM_CUSTOMER_SELECT_COLS} FROM crm_customers AS c \
                 WHERE c.number > ?2 ORDER BY c.number LIMIT ?3"
            ))?;
            let rows = stmt.query_map(params![today, after, (limit + 1) as i64], read_crm_customer_row)?;
            let mut all = Vec::new();
            for r in rows {
                all.push(r??);
            }
            let has_more = all.len() > limit;
            all.truncate(limit);
            let next = has_more.then(|| all.last().map(|c| c.number)).flatten();
            Ok(CrmCustomerPage {
                customers: all,
                next,
            })
        })
    }

    pub fn payments(&self, input: &CrmPaymentPageInput) -> StorageResult<CrmPaymentPage> {
        let open = input.state == CrmPaymentState::Open;
        let mut where_clauses = vec![if open {
            "p.paid_at IS NULL".to_string()
        } else {
            "p.paid_at IS NOT NULL".to_string()
        }];
        let mut values: Vec<SqlValue> = Vec::new();
        if let Some(cid) = &input.customer_id {
            where_clauses.push("p.customer_id = ?".to_string());
            values.push(SqlValue::Text(cid.clone()));
        }
        if let Some(after) = &input.after {
            let (key, number) = decode_crm_number_cursor(after)?;
            where_clauses.push(if open {
                "(p.due_on, p.number) > (?, ?)".to_string()
            } else {
                "(p.paid_at, p.number) < (?, ?)".to_string()
            });
            values.push(SqlValue::Text(key));
            values.push(SqlValue::Integer(number as i64));
        }
        let limit = CRM_LIMITS.page;
        values.push(SqlValue::Integer((limit + 1) as i64));
        let order = if open {
            "p.due_on, p.number"
        } else {
            "p.paid_at DESC, p.number DESC"
        };
        self.db.with_conn(|conn| {
            let mut stmt = conn.prepare(&format!(
                "{CRM_PAYMENT_SELECT_SQL} WHERE {} ORDER BY {order} LIMIT ?",
                where_clauses.join(" AND ")
            ))?;
            let rows = stmt.query_map(params_from_iter(values), read_crm_payment_row)?;
            let mut all = Vec::new();
            for r in rows {
                all.push(r??);
            }
            let has_more = all.len() > limit;
            all.truncate(limit);
            let next = if has_more {
                all.last().map(|last| {
                    let k = if open {
                        last.payment.due_on.as_str()
                    } else {
                        last.payment.paid_at.as_deref().unwrap_or("")
                    };
                    encode_crm_number_cursor(k, last.payment.number)
                })
            } else {
                None
            };
            Ok(CrmPaymentPage {
                payments: all.into_iter().map(|r| r.payment).collect(),
                next,
            })
        })
    }

    pub fn activity(&self, input: &CrmActivityPageInput) -> StorageResult<CrmActivityPage> {
        self.customer_record(&input.customer_id)?;
        let mut values: Vec<SqlValue> = vec![SqlValue::Text(input.customer_id.clone())];
        let after_sql = if let Some(after) = &input.after {
            let (created_at, activity_id) = decode_crm_text_cursor(after)?;
            values.push(SqlValue::Text(created_at));
            values.push(SqlValue::Text(activity_id));
            "AND (created_at, activity_id) < (?, ?)"
        } else {
            ""
        };
        values.push(SqlValue::Integer((CRM_ACTIVITY_PAGE_SIZE + 1) as i64));
        self.db.with_conn(|conn| {
            let mut stmt = conn.prepare(&format!(
                "SELECT activity_id, customer_id, actor_kind, actor_id, actor_name, kind, \
                        details_json, proposed_by, created_at \
                 FROM crm_activity WHERE customer_id = ? {after_sql} \
                 ORDER BY created_at DESC, activity_id DESC LIMIT ?"
            ))?;
            let rows = stmt.query_map(params_from_iter(values), read_crm_activity_row)?;
            let mut all = Vec::new();
            for r in rows {
                all.push(r??);
            }
            let has_more = all.len() > CRM_ACTIVITY_PAGE_SIZE;
            all.truncate(CRM_ACTIVITY_PAGE_SIZE);
            let next = if has_more {
                all.last()
                    .map(|last| encode_crm_text_cursor(&last.created_at, &last.id))
            } else {
                None
            };
            Ok(CrmActivityPage {
                activity: all,
                next,
            })
        })
    }

    pub fn apply(&self, change: &CrmChange, actor: &CrmActor) -> StorageResult<CrmApplyResult> {
        if actor.kind != CrmActorKind::Person {
            return Err(StorageError::Forbidden(
                "only person actors may apply direct CRM changes".to_string(),
            ));
        }
        if !change.is_valid() {
            return Err(StorageError::Validation(
                "error.crm.invalidChange".to_string(),
            ));
        }
        let at = self.timestamp();
        let outcome = self.db.with_tx(|conn| match change {
            CrmChange::ProposalDecide {
                proposal_id,
                approve,
            } => self.decide_proposal_on_conn(conn, proposal_id, *approve, actor, &at),
            _ => {
                let res = CrmWrite::new(conn, actor.clone(), None, at).apply_change(change)?;
                Ok((res, None))
            }
        })?;
        if let Some(deferred_err) = outcome.1 {
            return Err(deferred_err);
        }
        Ok(outcome.0)
    }

    fn decide_proposal_on_conn(
        &self,
        conn: &Connection,
        proposal_id: &str,
        approve: bool,
        actor: &CrmActor,
        at: &str,
    ) -> StorageResult<(CrmApplyResult, Option<StorageError>)> {
        let row = conn
            .query_row(
                "SELECT agent_id, customer_id, reason, change_json \
                 FROM crm_proposals WHERE proposal_id = ?1 AND state = 'pending'",
                params![proposal_id],
                |r| {
                    Ok((
                        r.get::<_, String>(0)?,
                        r.get::<_, Option<String>>(1)?,
                        r.get::<_, String>(2)?,
                        r.get::<_, String>(3)?,
                    ))
                },
            )
            .optional()?
            .ok_or_else(|| StorageError::NotFound("error.crm.proposalMissing".to_string()))?;
        let (agent_id, customer_id, reason, change_json) = row;

        let mark_decided = |state: &str| -> StorageResult<()> {
            conn.execute(
                "UPDATE crm_proposals SET state = ?1, decided_at = ?2, decided_by_id = ?3, decided_by_name = ?4 \
                 WHERE proposal_id = ?5",
                params![state, at, actor.id, actor.name, proposal_id],
            )?;
            Ok(())
        };
        let reject_proposal = |kind_opt: Option<String>| -> StorageResult<()> {
            mark_decided("rejected")?;
            if let Some(cid) = &customer_id
                && customer_record_on_conn(conn, cid).is_ok()
            {
                let mut details = BTreeMap::new();
                details.insert(
                    "change".to_string(),
                    kind_opt.map_or(CrmActivityDetailValue::Null, CrmActivityDetailValue::Text),
                );
                details.insert("reason".to_string(), CrmActivityDetailValue::Text(reason.clone()));
                CrmWrite::new(conn, actor.clone(), Some(agent_id.clone()), at.to_string())
                    .log(cid, "proposal-rejected", &details)?;
            }
            Ok(())
        };

        let parsed = serde_json::from_str::<CrmProposedChange>(&change_json)
            .ok()
            .filter(CrmProposedChange::is_valid);
        let raw_kind = serde_json::from_str::<serde_json::Value>(&change_json)
            .ok()
            .and_then(|v| v.get("kind")?.as_str().map(str::to_string));

        if !approve || parsed.is_none() {
            reject_proposal(raw_kind)?;
            let err = approve.then(|| StorageError::NotFound("error.crm.proposalMissing".into()));
            return Ok((CrmApplyResult::Applied { id: None }, err));
        }
        let proposed = parsed.expect("checked above");
        if source_owns_on_conn(conn, &proposed).unwrap_or(false) {
            reject_proposal(raw_kind.clone())?;
            return Ok((
                CrmApplyResult::Applied { id: None },
                Some(StorageError::Validation("error.crm.sourceOwned".to_string())),
            ));
        }

        match self.db.with_tx(|nested_conn| {
            CrmWrite::new(
                nested_conn,
                actor.clone(),
                Some(agent_id.clone()),
                at.to_string(),
            )
            .apply_proposed(&proposed)
        }) {
            Ok(applied) => {
                mark_decided("approved")?;
                Ok((applied, None))
            }
            Err(err @ StorageError::NotFound(_)) => {
                reject_proposal(raw_kind)?;
                Ok((CrmApplyResult::Applied { id: None }, Some(err)))
            }
            Err(other) => Err(other),
        }
    }

    pub fn propose(&self, input: &CrmProposeInput) -> StorageResult<String> {
        if !input.change.is_valid() || utf16_len(&input.reason) > CRM_LIMITS.reason {
            return Err(StorageError::Validation(
                "error.crm.invalidChange".to_string(),
            ));
        }
        let at = self.timestamp();
        let change_json = serde_json::to_string(&input.change)?;
        self.db.with_tx(|conn| {
            let agent_cnt: i64 = conn.query_row(
                "SELECT count(*) FROM crm_proposals WHERE state = 'pending' AND agent_id = ?1",
                params![input.agent_id],
                |r| r.get(0),
            )?;
            let total_cnt: i64 = conn.query_row(
                "SELECT count(*) FROM crm_proposals WHERE state = 'pending'",
                [],
                |r| r.get(0),
            )?;
            if (agent_cnt as usize) >= self.per_agent_proposal_limit
                || (total_cnt as usize) >= self.total_proposal_limit
            {
                return Err(StorageError::LimitExceeded(
                    "error.crm.tooManyProposals".to_string(),
                ));
            }
            let pid = new_crm_id();
            conn.execute(
                "INSERT INTO crm_proposals \
                   (proposal_id, agent_id, customer_id, thread_id, reason, change_json, created_at) \
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
                params![
                    pid,
                    input.agent_id,
                    input.customer_id,
                    input.thread_id,
                    input.reason,
                    change_json,
                    at,
                ],
            )?;
            Ok(pid)
        })
    }
}

// ---------------------------------------------------------------------------
// CrmWrite — Transactional CRM Mutator (`src/backend/crm/crm-store.ts:L726-L1380`)
// ---------------------------------------------------------------------------

pub type CrmDetailsMap = BTreeMap<String, CrmActivityDetailValue>;

fn detail_str(v: impl Into<String>) -> CrmActivityDetailValue {
    CrmActivityDetailValue::Text(v.into())
}

fn detail_opt_str(v: Option<impl Into<String>>) -> CrmActivityDetailValue {
    match v {
        Some(s) => CrmActivityDetailValue::Text(s.into()),
        None => CrmActivityDetailValue::Null,
    }
}

fn detail_i64(v: i64) -> CrmActivityDetailValue {
    CrmActivityDetailValue::Number(v as f64)
}

fn detail_u64(v: u64) -> CrmActivityDetailValue {
    CrmActivityDetailValue::Number(v as f64)
}

pub struct CrmWrite<'a> {
    conn: &'a Connection,
    actor: CrmActor,
    proposed_by: Option<String>,
    at: String,
    touched: HashSet<String>,
}

impl<'a> CrmWrite<'a> {
    pub fn new(
        conn: &'a Connection,
        actor: CrmActor,
        proposed_by: Option<String>,
        at: String,
    ) -> Self {
        Self {
            conn,
            actor,
            proposed_by,
            at,
            touched: HashSet::new(),
        }
    }

    pub fn at(&self) -> &str {
        &self.at
    }

    pub fn apply_change(mut self, change: &CrmChange) -> StorageResult<CrmApplyResult> {
        if let CrmChange::CustomerUpdate {
            customer_id,
            base_revision,
            ..
        } = change
            && customer_record_on_conn(self.conn, customer_id)?.revision != *base_revision
        {
            return Ok(CrmApplyResult::Conflict {
                conflict: CrmConflictReason::Revision,
            });
        }
        let id = self.execute_change(change)?;
        self.finish()?;
        Ok(CrmApplyResult::Applied { id })
    }

    pub fn apply_proposed(mut self, change: &CrmProposedChange) -> StorageResult<CrmApplyResult> {
        let id = match change {
            CrmProposedChange::CustomerCreate { customer } => Some(self.create_customer(customer)?),
            CrmProposedChange::CustomerUpdate { customer_id, patch } => {
                let cur = customer_record_on_conn(self.conn, customer_id)?;
                self.update_customer(&cur, patch)?;
                None
            }
            CrmProposedChange::CustomerTags {
                customer_id,
                tag_ids,
            } => {
                self.set_tags(customer_id, tag_ids)?;
                None
            }
            CrmProposedChange::PlanCreate { plan } => Some(self.create_plan(plan)?),
            CrmProposedChange::PlanUpdate { plan_id, patch } => {
                self.update_plan(plan_id, patch)?;
                None
            }
            CrmProposedChange::PlanEnd { plan_id, ends_on } => {
                self.end_plan(plan_id, ends_on)?;
                None
            }
            CrmProposedChange::PaymentCreate { payment } => Some(self.create_payment(payment)?),
            CrmProposedChange::PaymentUpdate { payment_id, patch } => {
                self.update_payment(payment_id, patch)?;
                None
            }
            CrmProposedChange::PaymentPaid {
                payment_ids,
                paid_at,
            } => {
                for pid in payment_ids {
                    self.mark_paid(pid, paid_at.as_deref())?;
                }
                None
            }
            CrmProposedChange::PaymentSplit { payment_id, parts } => {
                self.split_payment(payment_id, parts)?;
                None
            }
        };
        self.finish()?;
        Ok(CrmApplyResult::Applied { id })
    }

    pub fn finish(&mut self) -> StorageResult<()> {
        for cid in self.touched.drain() {
            self.conn.execute(
                "UPDATE crm_customers SET revision = revision + 1, updated_at = ?1 WHERE customer_id = ?2",
                params![self.at, cid],
            )?;
        }
        Ok(())
    }

    pub fn log(
        &self,
        customer_id: &str,
        kind: &str,
        details: &CrmDetailsMap,
    ) -> StorageResult<()> {
        let details_json = serde_json::to_string(details)?;
        self.conn.execute(
            "INSERT INTO crm_activity \
               (activity_id, customer_id, actor_kind, actor_id, actor_name, kind, details_json, proposed_by, created_at) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
            params![
                new_crm_id(),
                customer_id,
                actor_kind_str(self.actor.kind),
                self.actor.id,
                self.actor.name,
                kind,
                details_json,
                self.proposed_by,
                self.at,
            ],
        )?;
        Ok(())
    }

    pub fn record(
        &mut self,
        customer_id: &str,
        kind: &str,
        details: &CrmDetailsMap,
    ) -> StorageResult<()> {
        self.log(customer_id, kind, details)?;
        self.touched.insert(customer_id.to_string());
        Ok(())
    }

    pub fn next_number(&self, counter_name: &str) -> StorageResult<u64> {
        let val: i64 = self.conn.query_row(
            "INSERT INTO crm_counters (name, value) VALUES (?1, 1) \
             ON CONFLICT(name) DO UPDATE SET value = value + 1 RETURNING value",
            params![counter_name],
            |r| r.get(0),
        )?;
        Ok(val.max(1) as u64)
    }

    fn execute_change(&mut self, change: &CrmChange) -> StorageResult<Option<String>> {
        match change {
            CrmChange::CustomerCreate { customer } => Ok(Some(self.create_customer(customer)?)),
            CrmChange::CustomerUpdate {
                customer_id, patch, ..
            } => {
                let cur = customer_record_on_conn(self.conn, customer_id)?;
                self.update_customer(&cur, patch)?;
                Ok(None)
            }
            CrmChange::CustomerTags {
                customer_id,
                tag_ids,
            } => {
                self.set_tags(customer_id, tag_ids)?;
                Ok(None)
            }
            CrmChange::CustomerDelete { customer_id } => {
                let cur = customer_record_on_conn(self.conn, customer_id)?;
                if let Some(sref) = &cur.source_ref {
                    self.conn.execute(
                        "INSERT OR IGNORE INTO crm_source_ignored (source_ref) VALUES (?1)",
                        params![sref],
                    )?;
                }
                self.conn.execute(
                    "DELETE FROM crm_customers WHERE customer_id = ?1",
                    params![cur.id],
                )?;
                Ok(None)
            }
            CrmChange::PlanCreate { plan } => Ok(Some(self.create_plan(plan)?)),
            CrmChange::PlanUpdate { plan_id, patch } => {
                self.update_plan(plan_id, patch)?;
                Ok(None)
            }
            CrmChange::PlanEnd { plan_id, ends_on } => {
                self.end_plan(plan_id, ends_on)?;
                Ok(None)
            }
            CrmChange::PlanDelete { plan_id } => {
                self.delete_plan(plan_id)?;
                Ok(None)
            }
            CrmChange::PaymentCreate { payment } => Ok(Some(self.create_payment(payment)?)),
            CrmChange::PaymentUpdate { payment_id, patch } => {
                self.update_payment(payment_id, patch)?;
                Ok(None)
            }
            CrmChange::PaymentPaid {
                payment_ids,
                paid_at,
            } => {
                for pid in payment_ids {
                    self.mark_paid(pid, paid_at.as_deref())?;
                }
                Ok(None)
            }
            CrmChange::PaymentSplit { payment_id, parts } => {
                self.split_payment(payment_id, parts)?;
                Ok(None)
            }
            CrmChange::PaymentDelete { payment_id } => {
                let rec = payment_on_conn(self.conn, payment_id)?;
                ensure_editable(rec.payment.source)?;
                self.conn.execute(
                    "DELETE FROM crm_payments WHERE payment_id = ?1",
                    params![rec.payment.id],
                )?;
                let mut d = BTreeMap::new();
                d.insert("number".into(), detail_u64(rec.payment.number));
                d.insert("label".into(), detail_str(rec.payment.label));
                d.insert("amountMinor".into(), detail_i64(rec.payment.amount_minor));
                d.insert("currency".into(), detail_str(rec.payment.currency));
                d.insert("paidAt".into(), detail_opt_str(rec.payment.paid_at));
                self.record(&rec.payment.customer_id, "payment-deleted", &d)?;
                Ok(None)
            }
            CrmChange::TagCreate { label } => Ok(Some(self.create_tag(label)?)),
            CrmChange::TagDelete { tag_id } => {
                self.delete_tag(tag_id)?;
                Ok(None)
            }
            CrmChange::StatusCreate { label } => Ok(Some(self.create_status(label)?)),
            CrmChange::StatusRename { status_id, label } => {
                self.rename_status(status_id, label)?;
                Ok(None)
            }
            CrmChange::StatusDelete {
                status_id,
                replace_with,
            } => {
                self.delete_status(status_id, replace_with)?;
                Ok(None)
            }
            CrmChange::PrivateFields { fields } => {
                self.conn.execute("DELETE FROM crm_private_fields", [])?;
                for f in fields {
                    self.conn.execute(
                        "INSERT INTO crm_private_fields (field) VALUES (?1)",
                        params![private_field_str(*f)],
                    )?;
                }
                Ok(None)
            }
            CrmChange::AgentAccess { agent_ids } => {
                let kept_json = serde_json::to_string(agent_ids)?;
                self.conn.execute(
                    "DELETE FROM crm_agent_access WHERE agent_id NOT IN (SELECT value FROM json_each(?1))",
                    params![kept_json],
                )?;
                for aid in agent_ids {
                    self.conn.execute(
                        "INSERT OR IGNORE INTO crm_agent_access (agent_id, granted_at) VALUES (?1, ?2)",
                        params![aid, self.at],
                    )?;
                }
                Ok(None)
            }
            CrmChange::ProposalDecide { .. } => Err(StorageError::Validation(
                "error.crm.invalidChange".to_string(),
            )),
        }
    }

    pub fn create_customer(&mut self, input: &CrmCustomerInput) -> StorageResult<String> {
        let status_id = input.status_id.as_deref().unwrap_or("lead");
        status_on_conn(self.conn, status_id)?;
        let tag_ids = input.tag_ids.as_deref().unwrap_or(&[]);
        for tid in tag_ids {
            tag_on_conn(self.conn, tid)?;
        }
        let text = CrmCustomerText {
            name: input.name.clone(),
            contact_name: input.contact_name.clone().unwrap_or_default(),
            contact_email: input.contact_email.clone().unwrap_or_default(),
            phone: input.phone.clone().unwrap_or_default(),
            billing_email: input.billing_email.clone().unwrap_or_default(),
            domain: input.domain.clone().unwrap_or_default(),
            tax_id: input.tax_id.clone().unwrap_or_default(),
            address: input.address.clone().unwrap_or_default(),
            stripe_customer_id: input.stripe_customer_id.clone().unwrap_or_default(),
            notes: input.notes.clone().unwrap_or_default(),
        };
        let customer_id = new_crm_id();
        let number = self.next_number("customer")?;
        let owner_agent_id = input.owner_agent_id.as_ref().and_then(Clone::clone);
        let method = input.method.as_ref().and_then(|m| *m);
        let search_json = text.search_text_json()?;
        self.conn.execute(
            "INSERT INTO crm_customers (customer_id, number, name, status_id, owner_agent_id, method, currency, \
               contact_name, contact_email, phone, billing_email, domain, tax_id, address, stripe_customer_id, \
               notes, search_text, created_at, updated_at, revision) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17, ?18, ?19, 1)",
            params![
                customer_id,
                number as i64,
                text.name,
                status_id,
                owner_agent_id,
                method.map(method_str),
                input.currency,
                text.contact_name,
                text.contact_email,
                text.phone,
                text.billing_email,
                text.domain,
                text.tax_id,
                text.address,
                text.stripe_customer_id,
                text.notes,
                search_json,
                self.at,
                self.at,
            ],
        )?;
        for tid in tag_ids {
            self.conn.execute(
                "INSERT INTO crm_customer_tags (customer_id, tag_id) VALUES (?1, ?2)",
                params![customer_id, tid],
            )?;
        }
        let mut d = BTreeMap::new();
        d.insert("number".into(), detail_u64(number));
        self.log(&customer_id, "customer-created", &d)?;
        Ok(customer_id)
    }

    fn update_customer(
        &mut self,
        customer: &CrmCustomerRecord,
        patch: &CrmCustomerPatch,
    ) -> StorageResult<()> {
        if linked_id_change(customer, patch) {
            return Err(StorageError::Validation("error.crm.sourceOwned".to_string()));
        }
        let mut sets: Vec<String> = Vec::new();
        let mut values: Vec<SqlValue> = Vec::new();
        let mut text = customer.text.clone();
        let mut text_changed = false;

        let patch_text: [(&str, &str, Option<&String>); 10] = [
            ("name", "name", patch.name.as_ref()),
            ("contactName", "contact_name", patch.contact_name.as_ref()),
            ("contactEmail", "contact_email", patch.contact_email.as_ref()),
            ("phone", "phone", patch.phone.as_ref()),
            ("billingEmail", "billing_email", patch.billing_email.as_ref()),
            ("domain", "domain", patch.domain.as_ref()),
            ("taxId", "tax_id", patch.tax_id.as_ref()),
            ("address", "address", patch.address.as_ref()),
            ("stripeCustomerId", "stripe_customer_id", patch.stripe_customer_id.as_ref()),
            ("notes", "notes", patch.notes.as_ref()),
        ];
        for (field, col, opt_val) in patch_text {
            let Some(val) = opt_val else { continue };
            let old = customer.text.get(field);
            if val == old {
                continue;
            }
            sets.push(format!("{col} = ?"));
            values.push(SqlValue::Text(val.clone()));
            text.set(field, val.clone());
            text_changed = true;
            let mut d = BTreeMap::new();
            d.insert("field".into(), detail_str(field));
            d.insert("from".into(), detail_str(old));
            d.insert("to".into(), detail_str(val.clone()));
            self.record(&customer.id, "field-changed", &d)?;
        }
        if text_changed {
            sets.push("search_text = ?".to_string());
            values.push(SqlValue::Text(text.search_text_json()?));
        }
        if let Some(new_sid) = &patch.status_id
            && new_sid != &customer.status_id
        {
            let from = status_on_conn(self.conn, &customer.status_id)?;
            let to = status_on_conn(self.conn, new_sid)?;
            sets.push("status_id = ?".to_string());
            values.push(SqlValue::Text(to.id.clone()));
            self.record_status_changed(&customer.id, &from, &to)?;
        }
        if let Some(new_owner_opt) = &patch.owner_agent_id
            && new_owner_opt != &customer.owner_agent_id
        {
            sets.push("owner_agent_id = ?".to_string());
            values.push(match new_owner_opt {
                Some(s) => SqlValue::Text(s.clone()),
                None => SqlValue::Null,
            });
            let mut d = BTreeMap::new();
            d.insert("field".into(), detail_str("ownerAgentId"));
            d.insert("from".into(), detail_opt_str(customer.owner_agent_id.clone()));
            d.insert("to".into(), detail_opt_str(new_owner_opt.clone()));
            self.record(&customer.id, "field-changed", &d)?;
        }
        if let Some(new_method_opt) = &patch.method
            && new_method_opt != &customer.method
        {
            sets.push("method = ?".to_string());
            values.push(match new_method_opt {
                Some(m) => SqlValue::Text(method_str(*m).to_string()),
                None => SqlValue::Null,
            });
            let mut d = BTreeMap::new();
            d.insert("field".into(), detail_str("method"));
            d.insert("from".into(), detail_opt_str(customer.method.map(method_str)));
            d.insert("to".into(), detail_opt_str(new_method_opt.map(method_str)));
            self.record(&customer.id, "field-changed", &d)?;
        }
        if let Some(new_ccy) = &patch.currency
            && new_ccy != &customer.currency
        {
            let plans_cnt: i64 = self.conn.query_row(
                "SELECT count(*) FROM crm_plans WHERE customer_id = ?1",
                params![customer.id],
                |r| r.get(0),
            )?;
            let pay_cnt: i64 = self.conn.query_row(
                "SELECT count(*) FROM crm_payments WHERE customer_id = ?1",
                params![customer.id],
                |r| r.get(0),
            )?;
            if plans_cnt + pay_cnt > 0 {
                return Err(StorageError::Validation(
                    "error.crm.currencyInUse".to_string(),
                ));
            }
            sets.push("currency = ?".to_string());
            values.push(SqlValue::Text(new_ccy.clone()));
            let mut d = BTreeMap::new();
            d.insert("field".into(), detail_str("currency"));
            d.insert("from".into(), detail_str(&customer.currency));
            d.insert("to".into(), detail_str(new_ccy));
            self.record(&customer.id, "field-changed", &d)?;
        }
        if sets.is_empty() {
            return Ok(());
        }
        values.push(SqlValue::Text(customer.id.clone()));
        self.conn.execute(
            &format!("UPDATE crm_customers SET {} WHERE customer_id = ?", sets.join(", ")),
            params_from_iter(values),
        )?;
        Ok(())
    }

    fn record_status_changed(
        &mut self,
        customer_id: &str,
        from: &CrmStatusRecord,
        to: &CrmStatusRecord,
    ) -> StorageResult<()> {
        let mut d = BTreeMap::new();
        d.insert("from".into(), detail_str(&from.id));
        d.insert("to".into(), detail_str(&to.id));
        d.insert("fromLabel".into(), detail_opt_str(from.label.clone()));
        d.insert("toLabel".into(), detail_opt_str(to.label.clone()));
        self.record(customer_id, "status-changed", &d)
    }

    fn set_tags(&mut self, customer_id: &str, tag_ids: &[String]) -> StorageResult<()> {
        customer_record_on_conn(self.conn, customer_id)?;
        for tid in tag_ids {
            tag_on_conn(self.conn, tid)?;
        }
        let existing = tag_ids_of_on_conn(self.conn, customer_id)?;
        let cur_set: HashSet<&str> = existing.iter().map(String::as_str).collect();
        let wanted_set: HashSet<&str> = tag_ids.iter().map(String::as_str).collect();
        let added: Vec<&str> = tag_ids
            .iter()
            .map(String::as_str)
            .filter(|t| !cur_set.contains(t))
            .collect();
        let removed: Vec<&str> = existing
            .iter()
            .map(String::as_str)
            .filter(|t| !wanted_set.contains(t))
            .collect();
        if added.is_empty() && removed.is_empty() {
            return Ok(());
        }
        let added_labels = join_tag_labels(self.conn, &added)?;
        let removed_labels = join_tag_labels(self.conn, &removed)?;
        for tid in &removed {
            self.conn.execute(
                "DELETE FROM crm_customer_tags WHERE customer_id = ?1 AND tag_id = ?2",
                params![customer_id, tid],
            )?;
        }
        for tid in &added {
            self.conn.execute(
                "INSERT INTO crm_customer_tags (customer_id, tag_id) VALUES (?1, ?2)",
                params![customer_id, tid],
            )?;
        }
        let mut d = BTreeMap::new();
        d.insert("added".into(), detail_opt_str(added_labels));
        d.insert("removed".into(), detail_opt_str(removed_labels));
        self.record(customer_id, "tags-changed", &d)
    }

    #[allow(clippy::too_many_arguments)]
    pub fn insert_payment(
        &self,
        customer_id: &str,
        plan_id: Option<&str>,
        label: &str,
        amount_minor: i64,
        due_on: &str,
        paid_at: Option<&str>,
        method: Option<CrmMethod>,
        source: Option<(&str, &str)>,
    ) -> StorageResult<(String, u64)> {
        let id = new_crm_id();
        let number = self.next_number("payment")?;
        let (sid, sref) = match source {
            Some((i, r)) => (Some(i), Some(r)),
            None => (None, None),
        };
        self.conn.execute(
            "INSERT INTO crm_payments \
               (payment_id, number, customer_id, plan_id, label, amount_minor, due_on, paid_at, \
                method, source_id, source_ref, created_at, updated_at) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13)",
            params![
                id,
                number as i64,
                customer_id,
                plan_id,
                label,
                amount_minor,
                due_on,
                paid_at,
                method.map(method_str),
                sid,
                sref,
                self.at,
                self.at,
            ],
        )?;
        Ok((id, number))
    }

    fn create_plan(&mut self, input: &CrmPlanInput) -> StorageResult<String> {
        let customer = customer_record_on_conn(self.conn, &input.customer_id)?;
        let cnt: i64 = self.conn.query_row(
            "SELECT count(*) FROM crm_plans WHERE customer_id = ?1",
            params![customer.id],
            |r| r.get(0),
        )?;
        if (cnt as usize) >= CRM_LIMITS.customer_plans {
            return Err(StorageError::LimitExceeded(
                "error.crm.tooManyPlans".to_string(),
            ));
        }
        let plan_id = new_crm_id();
        self.conn.execute(
            "INSERT INTO crm_plans \
               (plan_id, customer_id, name, amount_minor, every, starts_on, ends_on, created_at, updated_at) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, NULL, ?7, ?8)",
            params![
                plan_id,
                customer.id,
                input.name,
                input.amount_minor,
                every_str(input.every),
                input.starts_on,
                self.at,
                self.at,
            ],
        )?;
        let (_, pay_num) = self.insert_payment(
            &customer.id,
            Some(&plan_id),
            &input.name,
            input.amount_minor,
            &input.starts_on,
            None,
            customer.method,
            None,
        )?;
        let mut d = BTreeMap::new();
        d.insert("plan".into(), detail_str(&input.name));
        d.insert("amountMinor".into(), detail_i64(input.amount_minor));
        d.insert("currency".into(), detail_str(&customer.currency));
        d.insert("every".into(), detail_str(every_str(input.every)));
        d.insert("startsOn".into(), detail_str(&input.starts_on));
        d.insert("payment".into(), detail_u64(pay_num));
        self.record(&customer.id, "plan-created", &d)?;
        Ok(plan_id)
    }

    fn update_plan(&mut self, plan_id: &str, patch: &CrmPlanPatch) -> StorageResult<()> {
        let plan = plan_on_conn(self.conn, plan_id)?;
        ensure_editable(plan.source)?;
        let customer = customer_record_on_conn(self.conn, &plan.customer_id)?;
        if let Some(name) = &patch.name
            && name != &plan.name
        {
            self.conn.execute(
                "UPDATE crm_plans SET name = ?1, updated_at = ?2 WHERE plan_id = ?3",
                params![name, self.at, plan_id],
            )?;
            let mut d = BTreeMap::new();
            d.insert("plan".into(), detail_str(&plan.name));
            d.insert("field".into(), detail_str("name"));
            d.insert("from".into(), detail_str(&plan.name));
            d.insert("to".into(), detail_str(name));
            d.insert("currency".into(), detail_str(&customer.currency));
            self.record(&plan.customer_id, "plan-changed", &d)?;
        }
        if let Some(amt) = patch.amount_minor
            && amt != plan.amount_minor
        {
            self.conn.execute(
                "UPDATE crm_plans SET amount_minor = ?1, updated_at = ?2 WHERE plan_id = ?3",
                params![amt, self.at, plan_id],
            )?;
            self.conn.execute(
                "UPDATE crm_payments SET amount_minor = ?1, updated_at = ?2 \
                 WHERE plan_id = ?3 AND paid_at IS NULL AND amount_minor = ?4",
                params![amt, self.at, plan_id, plan.amount_minor],
            )?;
            let mut d = BTreeMap::new();
            d.insert("plan".into(), detail_str(&plan.name));
            d.insert("field".into(), detail_str("amountMinor"));
            d.insert("from".into(), detail_i64(plan.amount_minor));
            d.insert("to".into(), detail_i64(amt));
            d.insert("currency".into(), detail_str(&customer.currency));
            self.record(&plan.customer_id, "plan-changed", &d)?;
        }
        if let Some(ev) = patch.every
            && ev != plan.every
        {
            self.conn.execute(
                "UPDATE crm_plans SET every = ?1, updated_at = ?2 WHERE plan_id = ?3",
                params![every_str(ev), self.at, plan_id],
            )?;
            let mut d = BTreeMap::new();
            d.insert("plan".into(), detail_str(&plan.name));
            d.insert("field".into(), detail_str("every"));
            d.insert("from".into(), detail_str(every_str(plan.every)));
            d.insert("to".into(), detail_str(every_str(ev)));
            d.insert("currency".into(), detail_str(&customer.currency));
            self.record(&plan.customer_id, "plan-changed", &d)?;
        }
        Ok(())
    }

    fn end_plan(&mut self, plan_id: &str, ends_on: &str) -> StorageResult<()> {
        let plan = plan_on_conn(self.conn, plan_id)?;
        ensure_editable(plan.source)?;
        if ends_on < plan.starts_on.as_str() {
            return Err(StorageError::Validation(
                "error.crm.planEndsBeforeStart".to_string(),
            ));
        }
        if plan.ends_on.as_deref() == Some(ends_on) {
            return Ok(());
        }
        self.conn.execute(
            "UPDATE crm_plans SET ends_on = ?1, updated_at = ?2 WHERE plan_id = ?3",
            params![ends_on, self.at, plan_id],
        )?;
        let removed = self.conn.execute(
            "DELETE FROM crm_payments WHERE plan_id = ?1 AND paid_at IS NULL AND due_on > ?2",
            params![plan_id, ends_on],
        )?;
        let mut d = BTreeMap::new();
        d.insert("plan".into(), detail_str(&plan.name));
        d.insert("endsOn".into(), detail_str(ends_on));
        d.insert("removedPayments".into(), detail_u64(removed as u64));
        self.record(&plan.customer_id, "plan-ended", &d)
    }

    fn delete_plan(&mut self, plan_id: &str) -> StorageResult<()> {
        let plan = plan_on_conn(self.conn, plan_id)?;
        ensure_editable(plan.source)?;
        let removed = self.conn.execute(
            "DELETE FROM crm_payments WHERE plan_id = ?1 AND paid_at IS NULL",
            params![plan_id],
        )?;
        self.conn.execute(
            "DELETE FROM crm_plans WHERE plan_id = ?1",
            params![plan_id],
        )?;
        let mut d = BTreeMap::new();
        d.insert("plan".into(), detail_str(&plan.name));
        d.insert("removedPayments".into(), detail_u64(removed as u64));
        self.record(&plan.customer_id, "plan-deleted", &d)
    }

    fn create_payment(&mut self, input: &CrmPaymentInput) -> StorageResult<String> {
        let customer = customer_record_on_conn(self.conn, &input.customer_id)?;
        let paid_at_norm = match input.paid_at.as_ref().and_then(Clone::clone) {
            Some(raw) => {
                let ms = parse_iso_ms(&raw).ok_or_else(|| {
                    StorageError::Validation("invalid payment paidAt timestamp".to_string())
                })?;
                Some(ms_to_iso(ms))
            }
            None => None,
        };
        let method = match &input.method {
            Some(m) => *m,
            None => customer.method,
        };
        let (pay_id, pay_num) = self.insert_payment(
            &customer.id,
            None,
            &input.label,
            input.amount_minor,
            &input.due_on,
            paid_at_norm.as_deref(),
            method,
            None,
        )?;
        let mut d = BTreeMap::new();
        d.insert("number".into(), detail_u64(pay_num));
        d.insert("label".into(), detail_str(&input.label));
        d.insert("amountMinor".into(), detail_i64(input.amount_minor));
        d.insert("currency".into(), detail_str(&customer.currency));
        d.insert("dueOn".into(), detail_str(&input.due_on));
        d.insert("paidAt".into(), detail_opt_str(paid_at_norm));
        self.record(&customer.id, "payment-created", &d)?;
        Ok(pay_id)
    }

    fn update_payment(&mut self, payment_id: &str, patch: &CrmPaymentPatch) -> StorageResult<()> {
        let rec = payment_on_conn(self.conn, payment_id)?;
        let p = &rec.payment;
        ensure_editable(p.source)?;
        if let Some(label) = &patch.label
            && label != &p.label
        {
            self.conn.execute(
                "UPDATE crm_payments SET label = ?1, updated_at = ?2 WHERE payment_id = ?3",
                params![label, self.at, payment_id],
            )?;
            let mut d = BTreeMap::new();
            d.insert("number".into(), detail_u64(p.number));
            d.insert("field".into(), detail_str("label"));
            d.insert("from".into(), detail_str(&p.label));
            d.insert("to".into(), detail_str(label));
            d.insert("currency".into(), detail_str(&p.currency));
            self.record(&p.customer_id, "payment-changed", &d)?;
        }
        if let Some(amt) = patch.amount_minor
            && amt != p.amount_minor
        {
            self.conn.execute(
                "UPDATE crm_payments SET amount_minor = ?1, updated_at = ?2 WHERE payment_id = ?3",
                params![amt, self.at, payment_id],
            )?;
            let mut d = BTreeMap::new();
            d.insert("number".into(), detail_u64(p.number));
            d.insert("field".into(), detail_str("amountMinor"));
            d.insert("from".into(), detail_i64(p.amount_minor));
            d.insert("to".into(), detail_i64(amt));
            d.insert("currency".into(), detail_str(&p.currency));
            self.record(&p.customer_id, "payment-changed", &d)?;
        }
        if let Some(due) = &patch.due_on
            && due != &p.due_on
        {
            self.conn.execute(
                "UPDATE crm_payments SET due_on = ?1, updated_at = ?2 WHERE payment_id = ?3",
                params![due, self.at, payment_id],
            )?;
            let mut d = BTreeMap::new();
            d.insert("number".into(), detail_u64(p.number));
            d.insert("field".into(), detail_str("dueOn"));
            d.insert("from".into(), detail_str(&p.due_on));
            d.insert("to".into(), detail_str(due));
            d.insert("currency".into(), detail_str(&p.currency));
            self.record(&p.customer_id, "payment-changed", &d)?;
        }
        if let Some(m_opt) = &patch.method
            && m_opt != &p.method
        {
            self.conn.execute(
                "UPDATE crm_payments SET method = ?1, updated_at = ?2 WHERE payment_id = ?3",
                params![m_opt.map(method_str), self.at, payment_id],
            )?;
            let mut d = BTreeMap::new();
            d.insert("number".into(), detail_u64(p.number));
            d.insert("field".into(), detail_str("method"));
            d.insert("from".into(), detail_opt_str(p.method.map(method_str)));
            d.insert("to".into(), detail_opt_str(m_opt.map(method_str)));
            d.insert("currency".into(), detail_str(&p.currency));
            self.record(&p.customer_id, "payment-changed", &d)?;
        }
        Ok(())
    }

    fn mark_paid(&mut self, payment_id: &str, paid_at: Option<&str>) -> StorageResult<()> {
        let rec = payment_on_conn(self.conn, payment_id)?;
        let p = &rec.payment;
        ensure_editable(p.source)?;
        let Some(raw_paid) = paid_at else {
            if p.paid_at.is_none() {
                return Ok(());
            }
            self.conn.execute(
                "UPDATE crm_payments SET paid_at = NULL, updated_at = ?1 WHERE payment_id = ?2",
                params![self.at, payment_id],
            )?;
            let mut d = BTreeMap::new();
            d.insert("number".into(), detail_u64(p.number));
            d.insert("label".into(), detail_str(&p.label));
            return self.record(&p.customer_id, "payment-unpaid", &d);
        };
        let ms = parse_iso_ms(raw_paid).ok_or_else(|| {
            StorageError::Validation("invalid payment paidAt timestamp".to_string())
        })?;
        let norm_at = ms_to_iso(ms);
        if let Some(prev_paid) = &p.paid_at {
            if prev_paid == &norm_at {
                return Ok(());
            }
            self.conn.execute(
                "UPDATE crm_payments SET paid_at = ?1, updated_at = ?2 WHERE payment_id = ?3",
                params![norm_at, self.at, payment_id],
            )?;
            let mut d = BTreeMap::new();
            d.insert("number".into(), detail_u64(p.number));
            d.insert("field".into(), detail_str("paidAt"));
            d.insert("from".into(), detail_str(prev_paid));
            d.insert("to".into(), detail_str(norm_at));
            return self.record(&p.customer_id, "payment-changed", &d);
        }
        self.conn.execute(
            "UPDATE crm_payments SET paid_at = ?1, updated_at = ?2 WHERE payment_id = ?3",
            params![norm_at, self.at, payment_id],
        )?;
        let mut d = BTreeMap::new();
        d.insert("number".into(), detail_u64(p.number));
        d.insert("label".into(), detail_str(&p.label));
        d.insert("amountMinor".into(), detail_i64(p.amount_minor));
        d.insert("currency".into(), detail_str(&p.currency));
        d.insert("paidAt".into(), detail_str(norm_at));
        self.record(&p.customer_id, "payment-paid", &d)?;
        if let Some(pid) = &p.plan_id {
            self.next_plan_payment(pid, p.method)?;
        }
        Ok(())
    }

    fn next_plan_payment(
        &mut self,
        plan_id: &str,
        paid_method: Option<CrmMethod>,
    ) -> StorageResult<()> {
        let plan = plan_on_conn(self.conn, plan_id)?;
        let months = match plan.every {
            CrmEvery::Once => return Ok(()),
            CrmEvery::Month => 1,
            CrmEvery::Quarter => 3,
            CrmEvery::Year => 12,
        };
        let open_cnt: i64 = self.conn.query_row(
            "SELECT count(*) FROM crm_payments WHERE plan_id = ?1 AND paid_at IS NULL",
            params![plan_id],
            |r| r.get(0),
        )?;
        if open_cnt > 0 {
            return Ok(());
        }
        let last_due: Option<String> = self
            .conn
            .query_row(
                "SELECT max(due_on) FROM crm_payments WHERE plan_id = ?1",
                params![plan_id],
                |r| r.get(0),
            )
            .optional()?
            .flatten();
        let mut due_on: Option<String> = None;
        for step in 1i32..=120_000 {
            let Some(cand) = add_crm_months(&plan.starts_on, step.saturating_mul(months)) else {
                due_on = None;
                break;
            };
            if last_due.as_deref().is_none_or(|l| cand.as_str() > l) {
                due_on = Some(cand);
                break;
            }
        }
        let Some(due_on) = due_on else {
            return Ok(());
        };
        if plan
            .ends_on
            .as_deref()
            .is_some_and(|end| due_on.as_str() > end)
        {
            return Ok(());
        }
        let customer = customer_record_on_conn(self.conn, &plan.customer_id)?;
        let (_, pay_num) = self.insert_payment(
            &plan.customer_id,
            Some(plan_id),
            &plan.name,
            plan.amount_minor,
            &due_on,
            None,
            paid_method.or(customer.method),
            None,
        )?;
        let mut d = BTreeMap::new();
        d.insert("number".into(), detail_u64(pay_num));
        d.insert("label".into(), detail_str(&plan.name));
        d.insert("amountMinor".into(), detail_i64(plan.amount_minor));
        d.insert("currency".into(), detail_str(&customer.currency));
        d.insert("dueOn".into(), detail_str(due_on));
        d.insert("paidAt".into(), CrmActivityDetailValue::Null);
        d.insert("plan".into(), detail_str(&plan.name));
        self.record(&plan.customer_id, "payment-created", &d)
    }

    fn split_payment(&mut self, payment_id: &str, parts: &[CrmSplitPart]) -> StorageResult<()> {
        let rec = payment_on_conn(self.conn, payment_id)?;
        let p = &rec.payment;
        ensure_editable(p.source)?;
        if p.paid_at.is_some() {
            return Err(StorageError::Validation("error.crm.paymentPaid".to_string()));
        }
        let total: i64 = parts
            .iter()
            .map(|pt| pt.amount_minor)
            .fold(0i64, i64::saturating_add);
        if total != p.amount_minor {
            return Err(StorageError::Validation("error.crm.splitTotal".to_string()));
        }
        let Some((first, rest)) = parts.split_first() else {
            return Err(StorageError::Validation(
                "error.crm.invalidChange".to_string(),
            ));
        };
        self.conn.execute(
            "UPDATE crm_payments SET amount_minor = ?1, due_on = ?2, updated_at = ?3 WHERE payment_id = ?4",
            params![first.amount_minor, first.due_on, self.at, payment_id],
        )?;
        let mut numbers = vec![p.number.to_string()];
        for part in rest {
            let (_, num) = self.insert_payment(
                &p.customer_id,
                p.plan_id.as_deref(),
                &p.label,
                part.amount_minor,
                &part.due_on,
                None,
                p.method,
                None,
            )?;
            numbers.push(num.to_string());
        }
        let mut d = BTreeMap::new();
        d.insert("number".into(), detail_u64(p.number));
        d.insert("parts".into(), detail_u64(parts.len() as u64));
        d.insert("numbers".into(), detail_str(numbers.join(", ")));
        d.insert("amountMinor".into(), detail_i64(p.amount_minor));
        d.insert("currency".into(), detail_str(&p.currency));
        self.record(&p.customer_id, "payment-split", &d)
    }

    fn create_tag(&mut self, label: &str) -> StorageResult<String> {
        let cnt: i64 =
            self.conn
                .query_row("SELECT count(*) FROM crm_tags", [], |r| r.get(0))?;
        if (cnt as usize) >= CRM_LIMITS.tags {
            return Err(StorageError::LimitExceeded(
                "error.crm.tooManyTags".to_string(),
            ));
        }
        let taken: Option<i64> = self
            .conn
            .query_row(
                "SELECT 1 FROM crm_tags WHERE label = ?1",
                params![label],
                |r| r.get(0),
            )
            .optional()?;
        if taken.is_some() {
            return Err(StorageError::Conflict("error.crm.tagExists".to_string()));
        }
        let id = new_crm_id();
        self.conn.execute(
            "INSERT INTO crm_tags (tag_id, label, created_at) VALUES (?1, ?2, ?3)",
            params![id, label, self.at],
        )?;
        Ok(id)
    }

    fn delete_tag(&mut self, tag_id: &str) -> StorageResult<()> {
        let tag = tag_on_conn(self.conn, tag_id)?;
        let cids: Vec<String> = {
            let mut stmt = self
                .conn
                .prepare("SELECT customer_id FROM crm_customer_tags WHERE tag_id = ?1")?;
            stmt.query_map(params![tag_id], |r| r.get(0))?
                .collect::<rusqlite::Result<Vec<_>>>()?
        };
        for cid in cids {
            let mut d = BTreeMap::new();
            d.insert("added".into(), CrmActivityDetailValue::Null);
            d.insert("removed".into(), detail_str(&tag.label));
            self.record(&cid, "tags-changed", &d)?;
        }
        self.conn
            .execute("DELETE FROM crm_tags WHERE tag_id = ?1", params![tag_id])?;
        Ok(())
    }

    fn create_status(&mut self, label: &str) -> StorageResult<String> {
        let cnt: i64 =
            self.conn
                .query_row("SELECT count(*) FROM crm_statuses", [], |r| r.get(0))?;
        if (cnt as usize) >= CRM_LIMITS.statuses {
            return Err(StorageError::LimitExceeded(
                "error.crm.tooManyStatuses".to_string(),
            ));
        }
        if status_label_taken(self.conn, label, None)? {
            return Err(StorageError::Conflict("error.crm.statusExists".to_string()));
        }
        let tone = CRM_TONE_CYCLE[(cnt.max(0) as usize) % CRM_TONE_CYCLE.len()];
        let id = new_crm_id();
        self.conn.execute(
            "INSERT INTO crm_statuses (status_id, label, tone, position, created_at) \
             VALUES (?1, ?2, ?3, (SELECT coalesce(max(position), -1) + 1 FROM crm_statuses), ?4)",
            params![id, label, tone_str(tone), self.at],
        )?;
        Ok(id)
    }

    fn rename_status(&mut self, status_id: &str, label: &str) -> StorageResult<()> {
        ensure_custom_status(self.conn, status_id)?;
        if status_label_taken(self.conn, label, Some(status_id))? {
            return Err(StorageError::Conflict("error.crm.statusExists".to_string()));
        }
        self.conn.execute(
            "UPDATE crm_statuses SET label = ?1 WHERE status_id = ?2",
            params![label, status_id],
        )?;
        Ok(())
    }

    fn delete_status(&mut self, status_id: &str, replace_with: &str) -> StorageResult<()> {
        let from = ensure_custom_status(self.conn, status_id)?;
        let to = status_on_conn(self.conn, replace_with)?;
        let cids: Vec<String> = {
            let mut stmt = self
                .conn
                .prepare("SELECT customer_id FROM crm_customers WHERE status_id = ?1")?;
            stmt.query_map(params![status_id], |r| r.get(0))?
                .collect::<rusqlite::Result<Vec<_>>>()?
        };
        self.conn.execute(
            "UPDATE crm_customers SET status_id = ?1 WHERE status_id = ?2",
            params![to.id, status_id],
        )?;
        for cid in cids {
            self.record_status_changed(&cid, &from, &to)?;
        }
        self.conn.execute(
            "DELETE FROM crm_statuses WHERE status_id = ?1",
            params![status_id],
        )?;
        Ok(())
    }
}

fn ensure_editable(source: Option<CrmSourceKind>) -> StorageResult<()> {
    if source.is_some() {
        return Err(StorageError::Validation("error.crm.sourceOwned".to_string()));
    }
    Ok(())
}

fn linked_id_change(customer: &CrmCustomerRecord, patch: &CrmCustomerPatch) -> bool {
    customer.source_id.is_some()
        && patch
            .stripe_customer_id
            .as_ref()
            .is_some_and(|next| next != &customer.text.stripe_customer_id)
}

fn ensure_custom_status(conn: &Connection, status_id: &str) -> StorageResult<CrmStatusRecord> {
    if matches!(status_id, "lead" | "customer" | "former") {
        return Err(StorageError::Validation(
            "error.crm.builtinStatus".to_string(),
        ));
    }
    status_on_conn(conn, status_id)
}

fn status_label_taken(
    conn: &Connection,
    label: &str,
    except_id: Option<&str>,
) -> StorageResult<bool> {
    let found: Option<i64> = conn
        .query_row(
            "SELECT 1 FROM crm_statuses WHERE label = ?1 AND status_id IS NOT ?2",
            params![label, except_id],
            |r| r.get(0),
        )
        .optional()?;
    Ok(found.is_some())
}

fn join_tag_labels(conn: &Connection, tag_ids: &[&str]) -> StorageResult<Option<String>> {
    if tag_ids.is_empty() {
        return Ok(None);
    }
    let mut labels = Vec::with_capacity(tag_ids.len());
    for tid in tag_ids {
        labels.push(tag_on_conn(conn, tid)?.label);
    }
    Ok(Some(labels.join(", ")))
}

fn source_owns_on_conn(conn: &Connection, change: &CrmProposedChange) -> StorageResult<bool> {
    match change {
        CrmProposedChange::PlanUpdate { plan_id, .. }
        | CrmProposedChange::PlanEnd { plan_id, .. } => Ok(plan_on_conn(conn, plan_id)?.source.is_some()),
        CrmProposedChange::PaymentUpdate { payment_id, .. }
        | CrmProposedChange::PaymentSplit { payment_id, .. } => {
            Ok(payment_on_conn(conn, payment_id)?.payment.source.is_some())
        }
        CrmProposedChange::PaymentPaid { payment_ids, .. } => {
            for pid in payment_ids {
                if payment_on_conn(conn, pid)?.payment.source.is_some() {
                    return Ok(true);
                }
            }
            Ok(false)
        }
        CrmProposedChange::CustomerUpdate { customer_id, patch } => {
            let cur = customer_record_on_conn(conn, customer_id)?;
            Ok(linked_id_change(&cur, patch))
        }
        _ => Ok(false),
    }
}

fn customer_record_on_conn(conn: &Connection, customer_id: &str) -> StorageResult<CrmCustomerRecord> {
    conn.query_row(
        "SELECT customer_id, number, name, status_id, owner_agent_id, method, currency, \
                contact_name, contact_email, phone, billing_email, domain, tax_id, address, \
                stripe_customer_id, notes, source_id, source_ref, created_at, updated_at, revision \
         FROM crm_customers WHERE customer_id = ?1",
        params![customer_id],
        read_customer_record_cols,
    )
    .optional()?
    .transpose()?
    .ok_or_else(|| StorageError::NotFound("error.crm.customerMissing".to_string()))
}

fn read_customer_record_cols(
    row: &rusqlite::Row<'_>,
) -> rusqlite::Result<StorageResult<CrmCustomerRecord>> {
    let method_s: Option<String> = row.get(5)?;
    let method = match method_s.as_deref() {
        None => None,
        Some(m) => match parse_method(m) {
            Some(v) => Some(v),
            None => {
                return Ok(Err(StorageError::Validation(format!(
                    "invalid crm method: {m}"
                ))));
            }
        },
    };
    Ok(Ok(CrmCustomerRecord {
        id: row.get(0)?,
        number: row.get::<_, i64>(1)?.max(1) as u64,
        text: CrmCustomerText {
            name: row.get(2)?,
            contact_name: row.get(7)?,
            contact_email: row.get(8)?,
            phone: row.get(9)?,
            billing_email: row.get(10)?,
            domain: row.get(11)?,
            tax_id: row.get(12)?,
            address: row.get(13)?,
            stripe_customer_id: row.get(14)?,
            notes: row.get(15)?,
        },
        status_id: row.get(3)?,
        owner_agent_id: row.get(4)?,
        method,
        currency: row.get(6)?,
        source_id: row.get(16)?,
        source_ref: row.get(17)?,
        created_at: row.get(18)?,
        updated_at: row.get(19)?,
        revision: row.get::<_, i64>(20)?.max(1) as u64,
    }))
}

fn read_crm_customer_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<StorageResult<CrmCustomer>> {
    let rec = match read_customer_record_cols(row)? {
        Ok(r) => r,
        Err(e) => return Ok(Err(e)),
    };
    let tags_json: String = row.get(21)?;
    let plans_json: String = row.get(22)?;
    let src_s: Option<String> = row.get(23)?;
    let paid_minor: i64 = row.get(24)?;
    let open_minor: i64 = row.get(25)?;
    let late_minor: i64 = row.get(26)?;
    let next_due_on: Option<String> = row.get(27)?;
    let last_paid_at: Option<String> = row.get(28)?;

    let tag_ids: Vec<String> = serde_json::from_str::<Vec<Option<String>>>(&tags_json)
        .map_err(|e| StorageError::Json(e))
        .map(|v| v.into_iter().flatten().collect())
        .unwrap_or_default();
    let raw_plans: Vec<serde_json::Value> = match serde_json::from_str(&plans_json) {
        Ok(v) => v,
        Err(e) => return Ok(Err(StorageError::Json(e))),
    };
    let mut plans = Vec::with_capacity(raw_plans.len());
    for pv in raw_plans {
        let Some(obj) = pv.as_object() else { continue };
        let Some(pid) = obj.get("plan_id").and_then(|v| v.as_str()) else {
            continue;
        };
        let cid = obj.get("customer_id").and_then(|v| v.as_str()).unwrap_or("");
        let name = obj.get("name").and_then(|v| v.as_str()).unwrap_or("");
        let amt = obj.get("amount_minor").and_then(|v| v.as_i64()).unwrap_or(0);
        let every_s = obj.get("every").and_then(|v| v.as_str()).unwrap_or("month");
        let Some(every) = parse_every(every_s) else {
            return Ok(Err(StorageError::Validation(format!(
                "invalid plan every: {every_s}"
            ))));
        };
        let starts_on = obj.get("starts_on").and_then(|v| v.as_str()).unwrap_or("");
        let ends_on = obj
            .get("ends_on")
            .and_then(|v| v.as_str())
            .map(str::to_string);
        let source = obj
            .get("source_kind")
            .and_then(|v| v.as_str())
            .and_then(parse_source_kind);
        plans.push(CrmPlan {
            id: pid.to_string(),
            customer_id: cid.to_string(),
            name: name.to_string(),
            amount_minor: amt,
            every,
            starts_on: starts_on.to_string(),
            ends_on,
            source,
        });
    }
    Ok(Ok(CrmCustomer {
        id: rec.id,
        number: rec.number,
        name: rec.text.name,
        contact_name: rec.text.contact_name,
        contact_email: rec.text.contact_email,
        phone: rec.text.phone,
        billing_email: rec.text.billing_email,
        domain: rec.text.domain,
        tax_id: rec.text.tax_id,
        address: rec.text.address,
        stripe_customer_id: rec.text.stripe_customer_id,
        notes: rec.text.notes,
        status_id: rec.status_id,
        owner_agent_id: rec.owner_agent_id,
        method: rec.method,
        currency: rec.currency,
        tag_ids,
        plans,
        created_at: rec.created_at,
        updated_at: rec.updated_at,
        revision: rec.revision,
        paid_minor,
        open_minor,
        late_minor,
        next_due_on,
        last_paid_at,
        source: src_s.as_deref().and_then(parse_source_kind),
    }))
}

fn plan_on_conn(conn: &Connection, plan_id: &str) -> StorageResult<CrmPlan> {
    conn.query_row(
        "SELECT p.plan_id, p.customer_id, p.name, p.amount_minor, p.every, p.starts_on, p.ends_on, \
                (SELECT s.kind FROM crm_sources AS s WHERE s.source_id = p.source_id) AS source_kind \
         FROM crm_plans AS p WHERE p.plan_id = ?1",
        params![plan_id],
        |r| {
            let ev_s: String = r.get(4)?;
            let src_s: Option<String> = r.get(7)?;
            let Some(every) = parse_every(&ev_s) else {
                return Ok(Err(StorageError::Validation(format!(
                    "invalid crm plan every: {ev_s}"
                ))));
            };
            Ok(Ok(CrmPlan {
                id: r.get(0)?,
                customer_id: r.get(1)?,
                name: r.get(2)?,
                amount_minor: r.get(3)?,
                every,
                starts_on: r.get(5)?,
                ends_on: r.get(6)?,
                source: src_s.as_deref().and_then(parse_source_kind),
            }))
        },
    )
    .optional()?
    .transpose()?
    .ok_or_else(|| StorageError::NotFound("error.crm.planMissing".to_string()))
}

fn payment_on_conn(conn: &Connection, payment_id: &str) -> StorageResult<CrmPaymentRecord> {
    conn.query_row(
        &format!("{CRM_PAYMENT_SELECT_SQL} WHERE p.payment_id = ?1"),
        params![payment_id],
        read_crm_payment_row,
    )
    .optional()?
    .transpose()?
    .ok_or_else(|| StorageError::NotFound("error.crm.paymentMissing".to_string()))
}

pub(crate) fn read_crm_payment_row(
    row: &rusqlite::Row<'_>,
) -> rusqlite::Result<StorageResult<CrmPaymentRecord>> {
    let method_s: Option<String> = row.get(9)?;
    let method = match method_s.as_deref() {
        None => None,
        Some(m) => match parse_method(m) {
            Some(v) => Some(v),
            None => {
                return Ok(Err(StorageError::Validation(format!(
                    "invalid crm method: {m}"
                ))));
            }
        },
    };
    let src_s: Option<String> = row.get(10)?;
    Ok(Ok(CrmPaymentRecord {
        payment: CrmPayment {
            id: row.get(0)?,
            number: row.get::<_, i64>(1)?.max(1) as u64,
            customer_id: row.get(2)?,
            plan_id: row.get(3)?,
            label: row.get(4)?,
            amount_minor: row.get(5)?,
            currency: row.get(6)?,
            due_on: row.get(7)?,
            paid_at: row.get(8)?,
            method,
            source: src_s.as_deref().and_then(parse_source_kind),
        },
        customer_number: row.get::<_, i64>(11)?.max(1) as u64,
        customer_name: row.get(12)?,
    }))
}

pub(crate) fn read_crm_activity_row(
    row: &rusqlite::Row<'_>,
) -> rusqlite::Result<StorageResult<CrmActivity>> {
    let actor_kind_s: String = row.get(2)?;
    let Some(actor_kind) = parse_actor_kind(&actor_kind_s) else {
        return Ok(Err(StorageError::Validation(format!(
            "invalid crm actor kind: {actor_kind_s}"
        ))));
    };
    let details_json: String = row.get(6)?;
    let mut details = BTreeMap::new();
    if let Ok(serde_json::Value::Object(map)) = serde_json::from_str(&details_json) {
        for (k, v) in map {
            match v {
                serde_json::Value::Null => {
                    details.insert(k, CrmActivityDetailValue::Null);
                }
                serde_json::Value::Number(n) => {
                    if let Some(f) = n.as_f64() {
                        details.insert(k, CrmActivityDetailValue::Number(f));
                    }
                }
                serde_json::Value::String(s) => {
                    details.insert(k, CrmActivityDetailValue::Text(s));
                }
                _ => {}
            }
        }
    }
    Ok(Ok(CrmActivity {
        id: row.get(0)?,
        customer_id: row.get(1)?,
        actor: CrmActor {
            kind: actor_kind,
            id: row.get(3)?,
            name: row.get(4)?,
        },
        kind: row.get(5)?,
        details,
        proposed_by: row.get(7)?,
        created_at: row.get(8)?,
    }))
}

fn status_on_conn(conn: &Connection, status_id: &str) -> StorageResult<CrmStatusRecord> {
    conn.query_row(
        "SELECT status_id, label, tone, position FROM crm_statuses WHERE status_id = ?1",
        params![status_id],
        read_status_row,
    )
    .optional()?
    .transpose()?
    .ok_or_else(|| StorageError::NotFound("error.crm.statusMissing".to_string()))
}

fn statuses_on_conn(conn: &Connection) -> StorageResult<Vec<CrmStatusRecord>> {
    let mut stmt = conn.prepare(
        "SELECT status_id, label, tone, position FROM crm_statuses ORDER BY position, status_id",
    )?;
    let rows = stmt.query_map([], read_status_row)?;
    let mut out = Vec::new();
    for r in rows {
        out.push(r??);
    }
    Ok(out)
}

fn read_status_row(
    row: &rusqlite::Row<'_>,
) -> rusqlite::Result<StorageResult<CrmStatusRecord>> {
    let tone_s: String = row.get(2)?;
    let Some(tone) = parse_tone(&tone_s) else {
        return Ok(Err(StorageError::Validation(format!(
            "invalid crm tone: {tone_s}"
        ))));
    };
    Ok(Ok(CrmStatusRecord {
        id: row.get(0)?,
        label: row.get(1)?,
        tone,
        position: row.get(3)?,
    }))
}

fn tag_on_conn(conn: &Connection, tag_id: &str) -> StorageResult<CrmTag> {
    conn.query_row(
        "SELECT tag_id, label FROM crm_tags WHERE tag_id = ?1",
        params![tag_id],
        |r| {
            Ok(CrmTag {
                id: r.get(0)?,
                label: r.get(1)?,
            })
        },
    )
    .optional()?
    .ok_or_else(|| StorageError::NotFound("error.crm.tagMissing".to_string()))
}

fn tags_on_conn(conn: &Connection) -> StorageResult<Vec<CrmTag>> {
    let mut stmt =
        conn.prepare("SELECT tag_id, label FROM crm_tags ORDER BY label, tag_id")?;
    let rows = stmt.query_map([], |r| {
        Ok(CrmTag {
            id: r.get(0)?,
            label: r.get(1)?,
        })
    })?;
    Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
}

fn tag_ids_of_on_conn(conn: &Connection, customer_id: &str) -> StorageResult<Vec<String>> {
    let mut stmt =
        conn.prepare("SELECT tag_id FROM crm_customer_tags WHERE customer_id = ?1")?;
    let rows = stmt.query_map(params![customer_id], |r| r.get::<_, String>(0))?;
    Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
}

fn source_on_conn(conn: &Connection, source_id: &str) -> StorageResult<CrmSource> {
    conn.query_row(
        "SELECT source_id, kind, test_mode, account_label, key_hint, state, error_code, \
                synced_at, skipped_json, connected_at \
         FROM crm_sources WHERE source_id = ?1",
        params![source_id],
        read_crm_source_row,
    )
    .optional()?
    .transpose()?
    .ok_or_else(|| StorageError::NotFound("error.crm.sourceMissing".to_string()))
}

fn read_crm_source_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<StorageResult<CrmSource>> {
    let kind_s: String = row.get(1)?;
    let Some(kind) = parse_source_kind(&kind_s) else {
        return Ok(Err(StorageError::Validation(format!(
            "invalid crm source kind: {kind_s}"
        ))));
    };
    let state_s: String = row.get(5)?;
    let Some(state) = parse_source_state(&state_s) else {
        return Ok(Err(StorageError::Validation(format!(
            "invalid crm source state: {state_s}"
        ))));
    };
    let err_s: Option<String> = row.get(6)?;
    let error_code = match err_s.as_deref() {
        None => None,
        Some(e) => match parse_source_error(e) {
            Some(v) => Some(v),
            None => {
                return Ok(Err(StorageError::Validation(format!(
                    "invalid crm source error: {e}"
                ))));
            }
        },
    };
    let skipped_json: String = row.get(8)?;
    let skip_val: serde_json::Value =
        serde_json::from_str(&skipped_json).unwrap_or_else(|_| serde_json::json!({}));
    let skip_u64 = |k: &str| -> u64 {
        skip_val
            .get(k)
            .and_then(serde_json::Value::as_u64)
            .unwrap_or(0)
    };
    Ok(Ok(CrmSource {
        id: row.get(0)?,
        kind,
        test_mode: row.get::<_, i64>(2)? == 1,
        account_label: row.get(3)?,
        key_hint: row.get(4)?,
        state,
        error_code,
        synced_at: row.get(7)?,
        connected_at: row.get(9)?,
        skipped: CrmSourceSkipped {
            currency: skip_u64("currency"),
            interval: skip_u64("interval"),
            limit: skip_u64("limit"),
        },
    }))
}

fn method_str(m: CrmMethod) -> &'static str {
    match m {
        CrmMethod::Stripe => "stripe",
        CrmMethod::Transfer => "transfer",
        CrmMethod::Crypto => "crypto",
        CrmMethod::Paypal => "paypal",
        CrmMethod::Cash => "cash",
    }
}

fn parse_method(s: &str) -> Option<CrmMethod> {
    match s {
        "stripe" => Some(CrmMethod::Stripe),
        "transfer" => Some(CrmMethod::Transfer),
        "crypto" => Some(CrmMethod::Crypto),
        "paypal" => Some(CrmMethod::Paypal),
        "cash" => Some(CrmMethod::Cash),
        _ => None,
    }
}

fn every_str(e: CrmEvery) -> &'static str {
    match e {
        CrmEvery::Month => "month",
        CrmEvery::Quarter => "quarter",
        CrmEvery::Year => "year",
        CrmEvery::Once => "once",
    }
}

fn parse_every(s: &str) -> Option<CrmEvery> {
    match s {
        "month" => Some(CrmEvery::Month),
        "quarter" => Some(CrmEvery::Quarter),
        "year" => Some(CrmEvery::Year),
        "once" => Some(CrmEvery::Once),
        _ => None,
    }
}

fn tone_str(t: CrmTone) -> &'static str {
    match t {
        CrmTone::InfoLight => "info-light",
        CrmTone::Secondary => "secondary",
        CrmTone::Outline => "outline",
        CrmTone::WarningLight => "warning-light",
        CrmTone::SuccessLight => "success-light",
        CrmTone::PrimaryLight => "primary-light",
    }
}

fn parse_tone(s: &str) -> Option<CrmTone> {
    match s {
        "info-light" => Some(CrmTone::InfoLight),
        "secondary" => Some(CrmTone::Secondary),
        "outline" => Some(CrmTone::Outline),
        "warning-light" => Some(CrmTone::WarningLight),
        "success-light" => Some(CrmTone::SuccessLight),
        "primary-light" => Some(CrmTone::PrimaryLight),
        _ => None,
    }
}

fn actor_kind_str(k: CrmActorKind) -> &'static str {
    match k {
        CrmActorKind::Person => "person",
        CrmActorKind::Agent => "agent",
        CrmActorKind::Source => "source",
    }
}

fn parse_actor_kind(s: &str) -> Option<CrmActorKind> {
    match s {
        "person" => Some(CrmActorKind::Person),
        "agent" => Some(CrmActorKind::Agent),
        "source" => Some(CrmActorKind::Source),
        _ => None,
    }
}

fn source_kind_str(k: CrmSourceKind) -> &'static str {
    match k {
        CrmSourceKind::Stripe => "stripe",
    }
}

fn parse_source_kind(s: &str) -> Option<CrmSourceKind> {
    match s {
        "stripe" => Some(CrmSourceKind::Stripe),
        _ => None,
    }
}

fn source_state_str(s: CrmSourceSyncState) -> &'static str {
    match s {
        CrmSourceSyncState::Syncing => "syncing",
        CrmSourceSyncState::Ok => "ok",
        CrmSourceSyncState::Error => "error",
    }
}

fn parse_source_state(s: &str) -> Option<CrmSourceSyncState> {
    match s {
        "syncing" => Some(CrmSourceSyncState::Syncing),
        "ok" => Some(CrmSourceSyncState::Ok),
        "error" => Some(CrmSourceSyncState::Error),
        _ => None,
    }
}

fn source_error_str(e: CrmSourceError) -> &'static str {
    match e {
        CrmSourceError::InvalidKey => "invalid-key",
        CrmSourceError::Permission => "permission",
        CrmSourceError::RateLimited => "rate-limited",
        CrmSourceError::Network => "network",
        CrmSourceError::Stripe => "stripe",
    }
}

fn parse_source_error(s: &str) -> Option<CrmSourceError> {
    match s {
        "invalid-key" => Some(CrmSourceError::InvalidKey),
        "permission" => Some(CrmSourceError::Permission),
        "rate-limited" => Some(CrmSourceError::RateLimited),
        "network" => Some(CrmSourceError::Network),
        "stripe" => Some(CrmSourceError::Stripe),
        _ => None,
    }
}

fn private_field_str(f: CrmPrivateField) -> &'static str {
    match f {
        CrmPrivateField::Name => "name",
        CrmPrivateField::ContactName => "contactName",
        CrmPrivateField::ContactEmail => "contactEmail",
        CrmPrivateField::Phone => "phone",
        CrmPrivateField::BillingEmail => "billingEmail",
        CrmPrivateField::Domain => "domain",
        CrmPrivateField::TaxId => "taxId",
        CrmPrivateField::Address => "address",
        CrmPrivateField::StripeCustomerId => "stripeCustomerId",
        CrmPrivateField::Notes => "notes",
        CrmPrivateField::Amounts => "amounts",
        CrmPrivateField::Dates => "dates",
        CrmPrivateField::Status => "status",
        CrmPrivateField::Tags => "tags",
        CrmPrivateField::Plans => "plans",
        CrmPrivateField::Method => "method",
        CrmPrivateField::Owner => "owner",
    }
}

fn parse_private_field(s: &str) -> Option<CrmPrivateField> {
    CRM_PRIVATE_FIELDS
        .into_iter()
        .find(|f| private_field_str(*f) == s)
}

// ---------------------------------------------------------------------------
// CrmSourceStore (`crm_sources`, `crm_source_ignored` & Stripe Batch Sync)
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SourceCustomer {
    pub source_ref: String,
    pub external_id: String,
    pub name: String,
    pub email: String,
    pub phone: String,
    pub currency: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SourcePlan {
    pub source_ref: String,
    pub customer: SourceCustomer,
    pub name: String,
    pub amount_minor: i64,
    pub currency: String,
    pub every: Option<CrmEvery>,
    pub starts_on: String,
    pub ends_on: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SourcePayment {
    pub source_ref: String,
    pub customer: SourceCustomer,
    pub plan_ref: Option<String>,
    pub label: String,
    pub amount_minor: i64,
    pub currency: String,
    pub due_on: String,
    pub paid_at: Option<String>,
    pub removed: bool,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SourceBatch {
    pub customers: Vec<SourceCustomer>,
    pub plans: Vec<SourcePlan>,
    pub payments: Vec<SourcePayment>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InsertCrmSourceInput {
    pub kind: CrmSourceKind,
    pub test_mode: bool,
    pub account_label: String,
    pub key_hint: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SourceStatePatch {
    pub state: CrmSourceSyncState,
    pub error_code: Option<CrmSourceError>,
    pub synced_at: Option<String>,
    pub skipped: Option<CrmSourceSkipped>,
}

const SOURCE_LINKED_TABLES: [&str; 3] = ["crm_customers", "crm_plans", "crm_payments"];

fn cut_text(value: &str, max_chars: usize) -> String {
    let trimmed = value.trim();
    let sliced: String = trimmed.chars().take(max_chars).collect();
    sliced.trim().to_string()
}

#[derive(Clone)]
pub struct CrmSourceStore {
    store: CrmStore,
}

impl CrmSourceStore {
    pub fn new(store: CrmStore) -> Self {
        Self { store }
    }

    pub fn insert(&self, input: &InsertCrmSourceInput) -> StorageResult<CrmSource> {
        let at = self.store.timestamp();
        let kind_s = source_kind_str(input.kind);
        self.store.db.with_tx(|conn| {
            let exists: Option<i64> = conn
                .query_row(
                    "SELECT 1 FROM crm_sources WHERE kind = ?1",
                    params![kind_s],
                    |r| r.get(0),
                )
                .optional()?;
            if exists.is_some() {
                return Err(StorageError::Conflict(
                    "error.crm.sourceConnected".to_string(),
                ));
            }
            let source_id = new_crm_id();
            let label = cut_text(&input.account_label, 200);
            let hint = cut_text(&input.key_hint, 8);
            conn.execute(
                "INSERT INTO crm_sources \
                   (source_id, kind, test_mode, account_label, key_hint, state, connected_at) \
                 VALUES (?1, ?2, ?3, ?4, ?5, 'syncing', ?6)",
                params![source_id, kind_s, i64::from(input.test_mode), label, hint, at],
            )?;
            let prefix = format!("{kind_s}:%");
            for table in SOURCE_LINKED_TABLES {
                conn.execute(
                    &format!(
                        "UPDATE {table} SET source_id = ?1 \
                         WHERE source_id IS NULL AND source_ref LIKE ?2 ESCAPE '\\'"
                    ),
                    params![source_id, prefix],
                )?;
            }
            source_on_conn(conn, &source_id)
        })
    }

    pub fn delete(&self, source_id: &str) -> StorageResult<CrmSource> {
        self.store.db.with_tx(|conn| {
            let src = source_on_conn(conn, source_id)?;
            for table in SOURCE_LINKED_TABLES {
                conn.execute(
                    &format!("UPDATE {table} SET source_id = NULL WHERE source_id = ?1"),
                    params![source_id],
                )?;
            }
            conn.execute(
                "DELETE FROM crm_sources WHERE source_id = ?1",
                params![source_id],
            )?;
            Ok(src)
        })
    }

    pub fn set_state(&self, source_id: &str, patch: &SourceStatePatch) -> StorageResult<bool> {
        let mut sets = vec![
            "state = ?".to_string(),
            "error_code = ?".to_string(),
        ];
        let mut values: Vec<SqlValue> = vec![
            SqlValue::Text(source_state_str(patch.state).to_string()),
            match patch.error_code {
                Some(e) => SqlValue::Text(source_error_str(e).to_string()),
                None => SqlValue::Null,
            },
        ];
        if let Some(sa) = &patch.synced_at {
            sets.push("synced_at = ?".to_string());
            values.push(SqlValue::Text(sa.clone()));
        }
        if let Some(sk) = &patch.skipped {
            sets.push("skipped_json = ?".to_string());
            values.push(SqlValue::Text(serde_json::to_string(sk)?));
        }
        values.push(SqlValue::Text(source_id.to_string()));
        self.store.db.with_conn(|conn| {
            let changes = conn.execute(
                &format!("UPDATE crm_sources SET {} WHERE source_id = ?", sets.join(", ")),
                params_from_iter(values),
            )?;
            Ok(changes > 0)
        })
    }

    pub fn open_refs(&self, source_id: &str) -> StorageResult<Vec<String>> {
        let today = self.store.today();
        self.store.db.with_conn(|conn| {
            let mut stmt = conn.prepare(
                "SELECT source_ref FROM crm_plans WHERE source_id = ?1 AND (ends_on IS NULL OR ends_on >= ?2) \
                 UNION ALL SELECT source_ref FROM crm_payments WHERE source_id = ?1 AND paid_at IS NULL",
            )?;
            let rows = stmt.query_map(params![source_id, today], |r| r.get::<_, String>(0))?;
            Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
        })
    }

    pub fn apply(
        &self,
        source_id: &str,
        batch: &SourceBatch,
        skips: &mut CrmSourceSkipped,
    ) -> StorageResult<bool> {
        let today = self.store.today();
        let at = self.store.timestamp();
        self.store.db.with_tx(|conn| {
            let src = source_on_conn(conn, source_id)?;
            let mut sync = SourceSync::new(conn, src, skips, today, at);
            for plan in &batch.plans {
                sync.sync_plan(plan)?;
            }
            for pay in &batch.payments {
                sync.sync_payment(pay)?;
            }
            for cust in &batch.customers {
                sync.sync_customer(cust, false)?;
            }
            sync.finish()
        })
    }
}

struct SourceSync<'a, 'b> {
    conn: &'a Connection,
    source: CrmSource,
    skips: &'b mut CrmSourceSkipped,
    write: CrmWrite<'a>,
    today: String,
    resolved: HashMap<String, Option<String>>,
    emails: Option<HashMap<String, Vec<String>>>,
    changed: bool,
}

impl<'a, 'b> SourceSync<'a, 'b> {
    fn new(
        conn: &'a Connection,
        source: CrmSource,
        skips: &'b mut CrmSourceSkipped,
        today: String,
        at: String,
    ) -> Self {
        let actor = CrmActor {
            kind: CrmActorKind::Source,
            id: Some(source_kind_str(source.kind).to_string()),
            name: None,
        };
        Self {
            conn,
            source,
            skips,
            write: CrmWrite::new(conn, actor, None, at),
            today,
            resolved: HashMap::new(),
            emails: None,
            changed: false,
        }
    }

    fn finish(mut self) -> StorageResult<bool> {
        self.write.finish()?;
        Ok(self.changed)
    }

    fn record_change(
        &mut self,
        customer_id: &str,
        kind: &str,
        details: &CrmDetailsMap,
    ) -> StorageResult<()> {
        self.write.record(customer_id, kind, details)?;
        self.changed = true;
        Ok(())
    }

    fn claim_ownership(&self, table: &str, id_col: &str, id: &str) -> StorageResult<()> {
        self.conn.execute(
            &format!(
                "UPDATE {table} SET source_id = ?1 WHERE {id_col} = ?2 AND source_id IS NOT ?3"
            ),
            params![self.source.id, id, self.source.id],
        )?;
        Ok(())
    }

    fn sync_customer(
        &mut self,
        customer: &SourceCustomer,
        paying: bool,
    ) -> StorageResult<Option<String>> {
        if let Some(known) = self.resolved.get(&customer.source_ref).cloned() {
            if let Some(cid) = &known
                && paying
            {
                self.promote_lead(cid)?;
            }
            return Ok(known);
        }
        let cid_opt = self.resolve_customer(customer, paying)?;
        self.resolved
            .insert(customer.source_ref.clone(), cid_opt.clone());
        Ok(cid_opt)
    }

    fn resolve_customer(
        &mut self,
        customer: &SourceCustomer,
        paying: bool,
    ) -> StorageResult<Option<String>> {
        let ignored: Option<i64> = self
            .conn
            .query_row(
                "SELECT 1 FROM crm_source_ignored WHERE source_ref = ?1",
                params![customer.source_ref],
                |r| r.get(0),
            )
            .optional()?;
        if ignored.is_some() {
            return Ok(None);
        }
        if let Some(cid) = self
            .conn
            .query_row(
                "SELECT customer_id FROM crm_customers WHERE source_ref = ?1",
                params![customer.source_ref],
                |r| r.get::<_, String>(0),
            )
            .optional()?
        {
            self.claim_ownership("crm_customers", "customer_id", &cid)?;
            if paying {
                self.promote_lead(&cid)?;
            }
            return Ok(Some(cid));
        }
        if let Some(matched_id) = self.match_existing_customer(customer)? {
            self.conn.execute(
                "UPDATE crm_customers SET source_id = ?1, source_ref = ?2, \
                   stripe_customer_id = CASE WHEN stripe_customer_id = '' THEN ?3 ELSE stripe_customer_id END \
                 WHERE customer_id = ?4",
                params![
                    self.source.id,
                    customer.source_ref,
                    customer.external_id,
                    matched_id
                ],
            )?;
            let mut d = BTreeMap::new();
            d.insert("source".into(), detail_str(source_kind_str(self.source.kind)));
            d.insert("id".into(), detail_str(&customer.external_id));
            self.record_change(&matched_id, "customer-linked", &d)?;
            if paying {
                self.promote_lead(&matched_id)?;
            }
            return Ok(Some(matched_id));
        }

        let mut disp = cut_text(&customer.name, CRM_LIMITS.name);
        if disp.is_empty() {
            disp = cut_text(&customer.email, CRM_LIMITS.name);
        }
        if disp.is_empty() {
            disp = cut_text(&customer.external_id, CRM_LIMITS.name);
        }
        if disp.is_empty() {
            disp = "Customer".to_string();
        }
        let cid = self.write.create_customer(&CrmCustomerInput {
            name: disp,
            currency: customer.currency.clone(),
            status_id: Some(if paying { "customer" } else { "lead" }.to_string()),
            method: Some(Some(CrmMethod::Stripe)),
            contact_email: Some(cut_text(&customer.email, CRM_LIMITS.text)),
            phone: Some(cut_text(&customer.phone, CRM_LIMITS.text)),
            stripe_customer_id: Some(cut_text(&customer.external_id, CRM_LIMITS.text)),
            ..Default::default()
        })?;
        self.conn.execute(
            "UPDATE crm_customers SET source_id = ?1, source_ref = ?2 WHERE customer_id = ?3",
            params![self.source.id, customer.source_ref, cid],
        )?;
        self.changed = true;
        Ok(Some(cid))
    }

    fn match_existing_customer(
        &mut self,
        customer: &SourceCustomer,
    ) -> StorageResult<Option<String>> {
        if !customer.external_id.is_empty()
            && let Some(by_ext) = self
                .conn
                .query_row(
                    "SELECT customer_id FROM crm_customers \
                     WHERE stripe_customer_id = ?1 AND source_ref IS NULL LIMIT 1",
                    params![customer.external_id],
                    |r| r.get::<_, String>(0),
                )
                .optional()?
        {
            return Ok(Some(by_ext));
        }
        let email = customer.email.trim().to_lowercase();
        if email.is_empty() {
            return Ok(None);
        }
        if self.emails.is_none() {
            let mut idx: HashMap<String, Vec<String>> = HashMap::new();
            let mut stmt = self.conn.prepare(
                "SELECT customer_id, contact_email, billing_email FROM crm_customers \
                 WHERE source_ref IS NULL AND stripe_customer_id = ''",
            )?;
            let rows = stmt.query_map([], |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    r.get::<_, String>(1)?,
                    r.get::<_, String>(2)?,
                ))
            })?;
            for r in rows {
                let (cid, ce, be) = r?;
                let mut own = HashSet::new();
                for raw in [ce, be] {
                    let norm = raw.trim().to_lowercase();
                    if !norm.is_empty() && own.insert(norm.clone()) {
                        idx.entry(norm).or_default().push(cid.clone());
                    }
                }
            }
            self.emails = Some(idx);
        }
        let found = self.emails.as_ref().and_then(|m| m.get(&email));
        Ok(match found.map(Vec::as_slice) {
            Some([only]) => Some(only.clone()),
            _ => None,
        })
    }

    fn promote_lead(&mut self, customer_id: &str) -> StorageResult<()> {
        let cur = customer_record_on_conn(self.conn, customer_id)?;
        if cur.status_id != "lead" {
            return Ok(());
        }
        let from = status_on_conn(self.conn, "lead")?;
        let to = status_on_conn(self.conn, "customer")?;
        self.conn.execute(
            "UPDATE crm_customers SET status_id = ?1 WHERE customer_id = ?2",
            params![to.id, customer_id],
        )?;
        let mut d = BTreeMap::new();
        d.insert("from".into(), detail_str(&from.id));
        d.insert("to".into(), detail_str(&to.id));
        d.insert("fromLabel".into(), detail_opt_str(from.label));
        d.insert("toLabel".into(), detail_opt_str(to.label));
        self.record_change(customer_id, "status-changed", &d)
    }

    fn resolve_owner(
        &mut self,
        customer: &SourceCustomer,
        currency: &str,
        paying: bool,
    ) -> StorageResult<Option<String>> {
        let Some(cid) = self.sync_customer(customer, paying)? else {
            return Ok(None);
        };
        if customer_record_on_conn(self.conn, &cid)?.currency != currency {
            self.skips.currency += 1;
            return Ok(None);
        }
        Ok(Some(cid))
    }

    fn sync_plan(&mut self, plan: &SourcePlan) -> StorageResult<()> {
        let active = plan
            .ends_on
            .as_deref()
            .is_none_or(|end| end >= self.today.as_str());
        let existing = self
            .conn
            .query_row(
                "SELECT plan_id, customer_id, name, amount_minor, every, starts_on, ends_on \
                 FROM crm_plans WHERE source_ref = ?1",
                params![plan.source_ref],
                |r| {
                    Ok((
                        r.get::<_, String>(0)?,
                        r.get::<_, String>(1)?,
                        r.get::<_, String>(2)?,
                        r.get::<_, i64>(3)?,
                        r.get::<_, String>(4)?,
                        r.get::<_, String>(5)?,
                        r.get::<_, Option<String>>(6)?,
                    ))
                },
            )
            .optional()?;
        if existing.is_none() && !active {
            return Ok(());
        }
        let name = cut_text(&plan.name, CRM_LIMITS.label);
        let Some(every) = plan.every.filter(|e| *e != CrmEvery::Once) else {
            if let Some((pid, owner, _, _, _, _, prev_end)) = existing {
                self.sync_plan_end(&pid, &owner, prev_end.as_deref(), plan.ends_on.as_deref(), &name)?;
            } else {
                self.skips.interval += 1;
            }
            return Ok(());
        };
        let Some(cid) = self.resolve_owner(&plan.customer, &plan.currency, active)? else {
            return Ok(());
        };
        let Some((pid, owner, old_name, old_amt, old_ev, old_start, old_end)) = existing else {
            let cnt: i64 = self.conn.query_row(
                "SELECT count(*) FROM crm_plans WHERE customer_id = ?1",
                params![cid],
                |r| r.get(0),
            )?;
            if (cnt as usize) >= CRM_LIMITS.customer_plans {
                self.skips.limit += 1;
                return Ok(());
            }
            let at = self.write.at().to_string();
            self.conn.execute(
                "INSERT INTO crm_plans (plan_id, customer_id, name, amount_minor, every, starts_on, \
                   ends_on, source_id, source_ref, created_at, updated_at) \
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)",
                params![
                    new_crm_id(),
                    cid,
                    name,
                    plan.amount_minor,
                    every_str(every),
                    plan.starts_on,
                    plan.ends_on,
                    self.source.id,
                    plan.source_ref,
                    at,
                    at,
                ],
            )?;
            let mut d = BTreeMap::new();
            d.insert("plan".into(), detail_str(name));
            d.insert("amountMinor".into(), detail_i64(plan.amount_minor));
            d.insert("currency".into(), detail_str(&plan.currency));
            d.insert("every".into(), detail_str(every_str(every)));
            d.insert("startsOn".into(), detail_str(&plan.starts_on));
            d.insert("payment".into(), CrmActivityDetailValue::Null);
            return self.record_change(&cid, "plan-created", &d);
        };

        self.claim_ownership("crm_plans", "plan_id", &pid)?;
        if old_end.as_deref() != plan.ends_on.as_deref() && old_end.is_some() {
            self.conn.execute(
                "UPDATE crm_plans SET ends_on = NULL WHERE plan_id = ?1",
                params![pid],
            )?;
        }
        let at = self.write.at().to_string();
        if old_name != name {
            self.conn.execute(
                "UPDATE crm_plans SET name = ?1, updated_at = ?2 WHERE plan_id = ?3",
                params![name, at, pid],
            )?;
            let mut d = BTreeMap::new();
            d.insert("plan".into(), detail_str(&old_name));
            d.insert("field".into(), detail_str("name"));
            d.insert("from".into(), detail_str(&old_name));
            d.insert("to".into(), detail_str(&name));
            d.insert("currency".into(), detail_str(&plan.currency));
            self.record_change(&owner, "plan-changed", &d)?;
        }
        if old_amt != plan.amount_minor {
            self.conn.execute(
                "UPDATE crm_plans SET amount_minor = ?1, updated_at = ?2 WHERE plan_id = ?3",
                params![plan.amount_minor, at, pid],
            )?;
            let mut d = BTreeMap::new();
            d.insert("plan".into(), detail_str(&old_name));
            d.insert("field".into(), detail_str("amountMinor"));
            d.insert("from".into(), detail_i64(old_amt));
            d.insert("to".into(), detail_i64(plan.amount_minor));
            d.insert("currency".into(), detail_str(&plan.currency));
            self.record_change(&owner, "plan-changed", &d)?;
        }
        let ev_str = every_str(every);
        if old_ev != ev_str {
            self.conn.execute(
                "UPDATE crm_plans SET every = ?1, updated_at = ?2 WHERE plan_id = ?3",
                params![ev_str, at, pid],
            )?;
            let mut d = BTreeMap::new();
            d.insert("plan".into(), detail_str(&old_name));
            d.insert("field".into(), detail_str("every"));
            d.insert("from".into(), detail_str(old_ev));
            d.insert("to".into(), detail_str(ev_str));
            d.insert("currency".into(), detail_str(&plan.currency));
            self.record_change(&owner, "plan-changed", &d)?;
        }
        if old_start != plan.starts_on {
            self.conn.execute(
                "UPDATE crm_plans SET starts_on = ?1, updated_at = ?2 WHERE plan_id = ?3",
                params![plan.starts_on, at, pid],
            )?;
            let mut d = BTreeMap::new();
            d.insert("plan".into(), detail_str(&old_name));
            d.insert("field".into(), detail_str("startsOn"));
            d.insert("from".into(), detail_str(old_start));
            d.insert("to".into(), detail_str(&plan.starts_on));
            d.insert("currency".into(), detail_str(&plan.currency));
            self.record_change(&owner, "plan-changed", &d)?;
        }
        self.sync_plan_end(&pid, &owner, old_end.as_deref(), plan.ends_on.as_deref(), &name)
    }

    fn sync_plan_end(
        &mut self,
        plan_id: &str,
        owner: &str,
        old_end: Option<&str>,
        ends_on: Option<&str>,
        name: &str,
    ) -> StorageResult<()> {
        if old_end == ends_on {
            return Ok(());
        }
        self.conn.execute(
            "UPDATE crm_plans SET ends_on = ?1, updated_at = ?2 WHERE plan_id = ?3",
            params![ends_on, self.write.at(), plan_id],
        )?;
        if let Some(eo) = ends_on {
            let mut d = BTreeMap::new();
            d.insert("plan".into(), detail_str(name));
            d.insert("endsOn".into(), detail_str(eo));
            d.insert("removedPayments".into(), detail_u64(0));
            self.record_change(owner, "plan-ended", &d)?;
        }
        Ok(())
    }

    fn sync_payment(&mut self, payment: &SourcePayment) -> StorageResult<()> {
        let existing = self
            .conn
            .query_row(
                "SELECT payment_id, number, customer_id, plan_id, label, amount_minor, due_on, paid_at \
                 FROM crm_payments WHERE source_ref = ?1",
                params![payment.source_ref],
                |r| {
                    Ok((
                        r.get::<_, String>(0)?,
                        r.get::<_, i64>(1)?.max(1) as u64,
                        r.get::<_, String>(2)?,
                        r.get::<_, Option<String>>(3)?,
                        r.get::<_, String>(4)?,
                        r.get::<_, i64>(5)?,
                        r.get::<_, String>(6)?,
                        r.get::<_, Option<String>>(7)?,
                    ))
                },
            )
            .optional()?;

        if payment.removed {
            let Some((pid, num, cid, _, old_label, old_amt, _, old_paid)) = existing else {
                return Ok(());
            };
            if old_paid.is_some() {
                return Ok(());
            }
            self.conn.execute(
                "DELETE FROM crm_payments WHERE payment_id = ?1",
                params![pid],
            )?;
            let mut d = BTreeMap::new();
            d.insert("number".into(), detail_u64(num));
            d.insert("label".into(), detail_str(old_label));
            d.insert("amountMinor".into(), detail_i64(old_amt));
            d.insert("currency".into(), detail_str(&payment.currency));
            d.insert("paidAt".into(), CrmActivityDetailValue::Null);
            return self.record_change(&cid, "payment-deleted", &d);
        }

        let paying = payment.paid_at.is_some();
        let Some(cid) = self.resolve_owner(&payment.customer, &payment.currency, paying)? else {
            return Ok(());
        };
        let label = cut_text(&payment.label, CRM_LIMITS.label);
        let plan_id = match &payment.plan_ref {
            None => None,
            Some(pref) => self
                .conn
                .query_row(
                    "SELECT plan_id FROM crm_plans WHERE source_ref = ?1 AND customer_id = ?2",
                    params![pref, cid],
                    |r| r.get::<_, String>(0),
                )
                .optional()?,
        };

        let Some((pid, num, owner, old_pid, old_label, old_amt, old_due, old_paid)) = existing
        else {
            let (_, created_num) = self.write.insert_payment(
                &cid,
                plan_id.as_deref(),
                &label,
                payment.amount_minor,
                &payment.due_on,
                payment.paid_at.as_deref(),
                Some(CrmMethod::Stripe),
                Some((&self.source.id, &payment.source_ref)),
            )?;
            let mut d = BTreeMap::new();
            d.insert("number".into(), detail_u64(created_num));
            d.insert("label".into(), detail_str(label));
            d.insert("amountMinor".into(), detail_i64(payment.amount_minor));
            d.insert("currency".into(), detail_str(&payment.currency));
            d.insert("dueOn".into(), detail_str(&payment.due_on));
            d.insert("paidAt".into(), detail_opt_str(payment.paid_at.clone()));
            return self.record_change(&cid, "payment-created", &d);
        };

        self.claim_ownership("crm_payments", "payment_id", &pid)?;
        let at = self.write.at().to_string();
        if old_pid != plan_id
            && let Some(new_pid) = &plan_id
        {
            self.conn.execute(
                "UPDATE crm_payments SET plan_id = ?1, updated_at = ?2 WHERE payment_id = ?3",
                params![new_pid, at, pid],
            )?;
        }
        if old_label != label {
            self.conn.execute(
                "UPDATE crm_payments SET label = ?1, updated_at = ?2 WHERE payment_id = ?3",
                params![label, at, pid],
            )?;
            let mut d = BTreeMap::new();
            d.insert("number".into(), detail_u64(num));
            d.insert("field".into(), detail_str("label"));
            d.insert("from".into(), detail_str(old_label));
            d.insert("to".into(), detail_str(&label));
            d.insert("currency".into(), detail_str(&payment.currency));
            self.record_change(&owner, "payment-changed", &d)?;
        }
        if old_amt != payment.amount_minor {
            self.conn.execute(
                "UPDATE crm_payments SET amount_minor = ?1, updated_at = ?2 WHERE payment_id = ?3",
                params![payment.amount_minor, at, pid],
            )?;
            let mut d = BTreeMap::new();
            d.insert("number".into(), detail_u64(num));
            d.insert("field".into(), detail_str("amountMinor"));
            d.insert("from".into(), detail_i64(old_amt));
            d.insert("to".into(), detail_i64(payment.amount_minor));
            d.insert("currency".into(), detail_str(&payment.currency));
            self.record_change(&owner, "payment-changed", &d)?;
        }
        if old_due != payment.due_on {
            self.conn.execute(
                "UPDATE crm_payments SET due_on = ?1, updated_at = ?2 WHERE payment_id = ?3",
                params![payment.due_on, at, pid],
            )?;
            let mut d = BTreeMap::new();
            d.insert("number".into(), detail_u64(num));
            d.insert("field".into(), detail_str("dueOn"));
            d.insert("from".into(), detail_str(old_due));
            d.insert("to".into(), detail_str(&payment.due_on));
            d.insert("currency".into(), detail_str(&payment.currency));
            self.record_change(&owner, "payment-changed", &d)?;
        }
        if old_paid != payment.paid_at {
            self.conn.execute(
                "UPDATE crm_payments SET paid_at = ?1, updated_at = ?2 WHERE payment_id = ?3",
                params![payment.paid_at, at, pid],
            )?;
            match (&old_paid, &payment.paid_at) {
                (_, None) => {
                    let mut d = BTreeMap::new();
                    d.insert("number".into(), detail_u64(num));
                    d.insert("label".into(), detail_str(label));
                    self.record_change(&owner, "payment-unpaid", &d)?;
                }
                (None, Some(pa)) => {
                    let mut d = BTreeMap::new();
                    d.insert("number".into(), detail_u64(num));
                    d.insert("label".into(), detail_str(label));
                    d.insert("amountMinor".into(), detail_i64(payment.amount_minor));
                    d.insert("currency".into(), detail_str(&payment.currency));
                    d.insert("paidAt".into(), detail_str(pa));
                    self.record_change(&owner, "payment-paid", &d)?;
                }
                (Some(prev), Some(pa)) => {
                    let mut d = BTreeMap::new();
                    d.insert("number".into(), detail_u64(num));
                    d.insert("field".into(), detail_str("paidAt"));
                    d.insert("from".into(), detail_str(prev));
                    d.insert("to".into(), detail_str(pa));
                    self.record_change(&owner, "payment-changed", &d)?;
                }
            }
        }
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// CrmAgentView — ACL & 17-Field Redacted Agent View (`src/backend/crm/crm-agent-view.ts`)
// ---------------------------------------------------------------------------

const CRM_HIDDEN_MARKER: &str = "hidden";
const AGENT_VIEW_DEFAULT_LIMIT: usize = 20;
const AGENT_VIEW_MAX_LIMIT: usize = 100;
const CUSTOMER_OPEN_PAYMENTS_LIMIT: usize = 50;
const CUSTOMER_PAID_PAYMENTS_LIMIT: usize = 10;
const CUSTOMER_ACTIVITY_LIMIT: usize = 20;

pub type CrmAgentRecord = serde_json::Map<String, serde_json::Value>;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum CrmAgentPaymentFilter {
    Late,
    Open,
    PaidUp,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CrmAgentFindInput {
    pub query: Option<String>,
    pub status: Option<String>,
    pub tag: Option<String>,
    pub payment: Option<CrmAgentPaymentFilter>,
    pub cursor: Option<String>,
    pub limit: Option<usize>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum CrmAgentPaymentQueryState {
    Open,
    Late,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CrmAgentPaymentsInput {
    pub state: CrmAgentPaymentQueryState,
    pub customer_id: Option<String>,
    pub cursor: Option<String>,
    pub limit: Option<usize>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CrmAgentCustomerFields {
    pub name: Option<String>,
    pub contact_name: Option<String>,
    pub contact_email: Option<String>,
    pub phone: Option<String>,
    pub billing_email: Option<String>,
    pub domain: Option<String>,
    pub tax_id: Option<String>,
    pub address: Option<String>,
    pub stripe_customer_id: Option<String>,
    pub notes: Option<String>,
    pub status: Option<String>,
    pub owner_agent_id: Option<Option<String>>,
    pub method: Option<Option<CrmMethod>>,
    pub currency: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CrmAgentPlanInput {
    pub customer_id: String,
    pub name: String,
    pub amount: String,
    pub every: CrmEvery,
    pub starts_on: String,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CrmAgentPlanPatch {
    pub name: Option<String>,
    pub amount: Option<String>,
    pub every: Option<CrmEvery>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CrmAgentPaymentInput {
    pub customer_id: String,
    pub label: String,
    pub amount: String,
    pub due_on: String,
    pub method: Option<Option<CrmMethod>>,
    pub paid_at: Option<Option<String>>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CrmAgentPaymentPatch {
    pub label: Option<String>,
    pub amount: Option<String>,
    pub due_on: Option<String>,
    pub method: Option<Option<CrmMethod>>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CrmAgentSplitPart {
    pub amount: String,
    pub due_on: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case")]
pub enum CrmAgentChange {
    CustomerCreate {
        name: String,
        currency: String,
        #[serde(default)]
        fields: CrmAgentCustomerFields,
        #[serde(default)]
        tags: Option<Vec<String>>,
    },
    CustomerUpdate {
        #[serde(rename = "customerId")]
        customer_id: String,
        patch: CrmAgentCustomerFields,
    },
    CustomerTags {
        #[serde(rename = "customerId")]
        customer_id: String,
        tags: Vec<String>,
    },
    PlanCreate {
        plan: CrmAgentPlanInput,
    },
    PlanUpdate {
        #[serde(rename = "planId")]
        plan_id: String,
        patch: CrmAgentPlanPatch,
    },
    PlanEnd {
        #[serde(rename = "planId")]
        plan_id: String,
        #[serde(rename = "endsOn")]
        ends_on: String,
    },
    PaymentCreate {
        payment: CrmAgentPaymentInput,
    },
    PaymentUpdate {
        #[serde(rename = "paymentId")]
        payment_id: String,
        patch: CrmAgentPaymentPatch,
    },
    PaymentPaid {
        #[serde(rename = "paymentIds")]
        payment_ids: Vec<String>,
        #[serde(rename = "paidAt")]
        paid_at: Option<String>,
    },
    PaymentSplit {
        #[serde(rename = "paymentId")]
        payment_id: String,
        parts: Vec<CrmAgentSplitPart>,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CrmAgentProposeInput {
    pub change: CrmAgentChange,
    pub reason: String,
    pub thread_id: Option<String>,
}

#[derive(Clone)]
pub struct CrmAgentView {
    store: CrmStore,
}

impl CrmAgentView {
    pub fn new(store: CrmStore) -> Self {
        Self { store }
    }

    fn allow(&self, agent_id: &str) -> StorageResult<HashSet<CrmPrivateField>> {
        if !self.store.has_agent_access(agent_id)? {
            return Err(StorageError::Forbidden(
                "error.crm.agentNotAllowed".to_string(),
            ));
        }
        Ok(self.store.private_fields()?.into_iter().collect())
    }

    fn masked_name(name: &str, number: u64, hidden: &HashSet<CrmPrivateField>) -> String {
        if hidden.contains(&CrmPrivateField::Name) {
            format!("Customer {number}")
        } else {
            name.to_string()
        }
    }

    fn resolve_status_id(&self, value: &str) -> StorageResult<String> {
        let lower = value.to_lowercase();
        let found = self.store.statuses()?.into_iter().find(|s| {
            s.id == value
                || s.id == lower
                || s.label
                    .as_deref()
                    .is_some_and(|l| l.to_lowercase() == lower)
        });
        found
            .map(|s| s.id)
            .ok_or_else(|| StorageError::NotFound("error.crm.statusMissing".to_string()))
    }

    fn resolve_tag_id(&self, value: &str) -> StorageResult<String> {
        let lower = value.to_lowercase();
        let found = self
            .store
            .tags()?
            .into_iter()
            .find(|t| t.id == value || t.label.to_lowercase() == lower);
        found
            .map(|t| t.id)
            .ok_or_else(|| StorageError::NotFound("error.crm.tagMissing".to_string()))
    }

    fn build_summary(
        &self,
        customer: &CrmCustomer,
        hidden: &HashSet<CrmPrivateField>,
        status_map: &HashMap<String, String>,
        tag_map: &HashMap<String, String>,
    ) -> CrmAgentRecord {
        let amounts = !hidden.contains(&CrmPrivateField::Amounts);
        let dates = !hidden.contains(&CrmPrivateField::Dates);
        let mut rec = CrmAgentRecord::new();
        rec.insert("id".into(), serde_json::Value::from(customer.id.clone()));
        rec.insert("number".into(), serde_json::Value::from(customer.number));
        rec.insert(
            "name".into(),
            serde_json::Value::from(Self::masked_name(&customer.name, customer.number, hidden)),
        );
        if !hidden.contains(&CrmPrivateField::Status) {
            let st_name = status_map
                .get(&customer.status_id)
                .cloned()
                .unwrap_or_else(|| customer.status_id.clone());
            insert_non_empty(&mut rec, "status", serde_json::Value::from(st_name));
        }
        if !hidden.contains(&CrmPrivateField::Tags) {
            let labels: Vec<serde_json::Value> = customer
                .tag_ids
                .iter()
                .filter_map(|tid| tag_map.get(tid).map(|l| serde_json::Value::from(l.clone())))
                .collect();
            if !labels.is_empty() {
                rec.insert("tags".into(), serde_json::Value::Array(labels));
            }
        }
        if amounts && customer.open_minor > 0 {
            rec.insert(
                "open".into(),
                serde_json::Value::from(format_crm_amount(customer.open_minor, &customer.currency)),
            );
        }
        if amounts && dates && customer.late_minor > 0 {
            rec.insert(
                "late".into(),
                serde_json::Value::from(format_crm_amount(customer.late_minor, &customer.currency)),
            );
        }
        if dates
            && let Some(nd) = &customer.next_due_on
        {
            insert_non_empty(&mut rec, "nextDueOn", serde_json::Value::from(nd.clone()));
        }
        rec
    }

    fn build_payment_record(
        rec: &CrmPaymentRecord,
        hidden: &HashSet<CrmPrivateField>,
        include_customer: bool,
    ) -> CrmAgentRecord {
        let p = &rec.payment;
        let mut out = CrmAgentRecord::new();
        out.insert("id".into(), serde_json::Value::from(p.id.clone()));
        out.insert("number".into(), serde_json::Value::from(p.number));
        if include_customer {
            out.insert(
                "customer".into(),
                serde_json::json!({
                    "id": p.customer_id,
                    "name": Self::masked_name(&rec.customer_name, rec.customer_number, hidden),
                }),
            );
        }
        if !(hidden.contains(&CrmPrivateField::Plans) && p.plan_id.is_some()) {
            insert_non_empty(&mut out, "label", serde_json::Value::from(p.label.clone()));
        }
        if !hidden.contains(&CrmPrivateField::Amounts) {
            out.insert(
                "amount".into(),
                serde_json::Value::from(format_crm_amount(p.amount_minor, &p.currency)),
            );
        }
        if !hidden.contains(&CrmPrivateField::Dates) {
            insert_non_empty(&mut out, "dueOn", serde_json::Value::from(p.due_on.clone()));
            if let Some(pa) = &p.paid_at {
                insert_non_empty(&mut out, "paidAt", serde_json::Value::from(pa.clone()));
            }
        }
        if !hidden.contains(&CrmPrivateField::Plans)
            && let Some(pid) = &p.plan_id
        {
            insert_non_empty(&mut out, "planId", serde_json::Value::from(pid.clone()));
        }
        if !hidden.contains(&CrmPrivateField::Method)
            && let Some(m) = p.method
        {
            out.insert("method".into(), serde_json::Value::from(method_str(m)));
        }
        if let Some(src) = p.source {
            out.insert("source".into(), serde_json::Value::from(source_kind_str(src)));
        }
        out
    }

    fn redact_activity(
        activity: &CrmActivity,
        hidden: &HashSet<CrmPrivateField>,
    ) -> Option<CrmAgentRecord> {
        let mut details: CrmAgentRecord = activity
            .details
            .iter()
            .filter_map(|(k, v)| match v {
                CrmActivityDetailValue::Null => None,
                CrmActivityDetailValue::Text(s) => Some((k.clone(), serde_json::Value::from(s.clone()))),
                CrmActivityDetailValue::Number(n) => {
                    let jv = if n.fract() == 0.0 && n.abs() < (i64::MAX as f64) {
                        serde_json::Value::from(*n as i64)
                    } else {
                        serde_json::Value::from(*n)
                    };
                    Some((k.clone(), jv))
                }
            })
            .collect();
        let field_name = details
            .get("field")
            .and_then(serde_json::Value::as_str)
            .map(str::to_string);
        for part in activity_private_parts(&activity.kind, field_name.as_deref()) {
            if hidden.contains(&part) {
                return None;
            }
        }
        if let Some(f) = field_name.as_deref()
            && CRM_TEXT_FIELDS.contains(&f)
            && let Some(pf) = parse_private_field(f)
            && hidden.contains(&pf)
        {
            details.insert("from".into(), serde_json::Value::from(CRM_HIDDEN_MARKER));
            details.insert("to".into(), serde_json::Value::from(CRM_HIDDEN_MARKER));
        }
        if hidden.contains(&CrmPrivateField::Amounts) {
            details.remove("amountMinor");
        }
        if hidden.contains(&CrmPrivateField::Dates) {
            for k in ["dueOn", "paidAt", "startsOn", "endsOn"] {
                details.remove(k);
            }
        }
        if hidden.contains(&CrmPrivateField::Plans) {
            for k in ["plan", "label"] {
                details.remove(k);
            }
        }
        if activity.kind == "customer-linked" && hidden.contains(&CrmPrivateField::StripeCustomerId) {
            details.remove("id");
        }
        if let Some(ccy) = details
            .get("currency")
            .and_then(serde_json::Value::as_str)
            .map(str::to_string)
        {
            if let Some(am) = details.remove("amountMinor").and_then(|v| v.as_i64()) {
                details.insert("amount".into(), serde_json::Value::from(format_crm_amount(am, &ccy)));
            }
            if field_name.as_deref() == Some("amountMinor") {
                for k in ["from", "to"] {
                    if let Some(n) = details.get(k).and_then(|v| v.as_i64()) {
                        details.insert(k.into(), serde_json::Value::from(format_crm_amount(n, &ccy)));
                    }
                }
                details.insert("field".into(), serde_json::Value::from("amount"));
            }
            details.remove("currency");
        }
        let mut out = CrmAgentRecord::new();
        out.insert("at".into(), serde_json::Value::from(activity.created_at.clone()));
        out.insert("kind".into(), serde_json::Value::from(activity.kind.clone()));
        let by = if activity.actor.kind == CrmActorKind::Person {
            activity
                .actor
                .name
                .clone()
                .unwrap_or_else(|| "owner".to_string())
        } else {
            activity.actor.id.clone().unwrap_or_default()
        };
        insert_non_empty(&mut out, "by", serde_json::Value::from(by));
        if let Some(pb) = &activity.proposed_by {
            insert_non_empty(&mut out, "proposedBy", serde_json::Value::from(pb.clone()));
        }
        for (k, v) in details {
            insert_non_empty(&mut out, &k, v);
        }
        Some(out)
    }

    pub fn find(&self, agent_id: &str, input: &CrmAgentFindInput) -> StorageResult<CrmAgentRecord> {
        let hidden = self.allow(agent_id)?;
        if input.status.is_some() && hidden.contains(&CrmPrivateField::Status) {
            return Err(StorageError::Forbidden("error.crm.fieldPrivate".to_string()));
        }
        if input.tag.is_some() && hidden.contains(&CrmPrivateField::Tags) {
            return Err(StorageError::Forbidden("error.crm.fieldPrivate".to_string()));
        }
        if matches!(
            input.payment,
            Some(CrmAgentPaymentFilter::Late | CrmAgentPaymentFilter::PaidUp)
        ) && hidden.contains(&CrmPrivateField::Dates)
        {
            return Err(StorageError::Forbidden("error.crm.fieldPrivate".to_string()));
        }

        let mut values: Vec<SqlValue> = vec![SqlValue::Text(self.store.today())];
        let mut push_param = |v: SqlValue| -> String {
            values.push(v);
            format!("?{}", values.len())
        };
        let mut where_clauses: Vec<String> = Vec::new();
        if let Some(q) = input.query.as_deref().map(str::trim).filter(|s| !s.is_empty()) {
            let mut matches_sql = Vec::new();
            let visible_fields: Vec<&str> = CRM_TEXT_FIELDS
                .into_iter()
                .filter(|f| {
                    parse_private_field(f).is_none_or(|pf| !hidden.contains(&pf))
                })
                .collect();
            if !visible_fields.is_empty() {
                let vis_json = serde_json::to_string(&visible_fields)?;
                let escaped = normalize_crm_search(q)
                    .replace('\\', "\\\\")
                    .replace('%', "\\%")
                    .replace('_', "\\_");
                let p_vis = push_param(SqlValue::Text(vis_json));
                let p_pat = push_param(SqlValue::Text(format!("%{escaped}%")));
                matches_sql.push(format!(
                    "EXISTS (SELECT 1 FROM json_each(c.search_text) AS j \
                     WHERE j.key IN (SELECT value FROM json_each({p_vis})) \
                     AND j.value LIKE {p_pat} ESCAPE '\\')"
                ));
            }
            if let Some(num) = parse_customer_number_query(q) {
                let p_num = push_param(SqlValue::Integer(num as i64));
                matches_sql.push(format!("c.number = {p_num}"));
            }
            if matches_sql.is_empty() {
                where_clauses.push("0".to_string());
            } else {
                where_clauses.push(format!("({})", matches_sql.join(" OR ")));
            }
        }
        if let Some(st) = &input.status {
            let sid = self.resolve_status_id(st)?;
            let p = push_param(SqlValue::Text(sid));
            where_clauses.push(format!("c.status_id = {p}"));
        }
        if let Some(tg) = &input.tag {
            let tid = self.resolve_tag_id(tg)?;
            let p = push_param(SqlValue::Text(tid));
            where_clauses.push(format!(
                "EXISTS (SELECT 1 FROM crm_customer_tags AS ct \
                 WHERE ct.customer_id = c.customer_id AND ct.tag_id = {p})"
            ));
        }
        let late_exists = "EXISTS (SELECT 1 FROM crm_payments AS p \
            WHERE p.customer_id = c.customer_id AND p.paid_at IS NULL AND p.due_on < ?1)";
        match input.payment {
            Some(CrmAgentPaymentFilter::Late) => where_clauses.push(late_exists.to_string()),
            Some(CrmAgentPaymentFilter::PaidUp) => {
                where_clauses.push(format!("NOT {late_exists}"));
            }
            Some(CrmAgentPaymentFilter::Open) => where_clauses.push(
                "EXISTS (SELECT 1 FROM crm_payments AS p \
                 WHERE p.customer_id = c.customer_id AND p.paid_at IS NULL)"
                    .to_string(),
            ),
            None => {}
        }
        if let Some(cur) = &input.cursor {
            let after: u64 = cur
                .parse()
                .map_err(|_| StorageError::Validation("error.crm.badCursor".to_string()))?;
            let p = push_param(SqlValue::Integer(after as i64));
            where_clauses.push(format!("c.number > {p}"));
        }
        let limit = clamp_agent_limit(input.limit);
        let p_limit = push_param(SqlValue::Integer((limit + 1) as i64));
        let where_sql = if where_clauses.is_empty() {
            String::new()
        } else {
            format!("WHERE {}", where_clauses.join(" AND "))
        };

        let status_map = self.status_display_map()?;
        let tag_map = self.tag_display_map()?;
        let mut rows = self.store.db.with_conn(|conn| {
            let mut stmt = conn.prepare(&format!(
                "SELECT {CRM_CUSTOMER_SELECT_COLS} FROM crm_customers AS c {where_sql} \
                 ORDER BY c.number LIMIT {p_limit}"
            ))?;
            let mapped = stmt.query_map(params_from_iter(values), read_crm_customer_row)?;
            let mut out = Vec::new();
            for r in mapped {
                out.push(r??);
            }
            Ok(out)
        })?;
        let has_more = rows.len() > limit;
        rows.truncate(limit);
        let next_cursor = has_more.then(|| rows.last().map(|c| c.number.to_string())).flatten();
        let summaries: Vec<serde_json::Value> = rows
            .iter()
            .map(|c| {
                serde_json::Value::Object(self.build_summary(c, &hidden, &status_map, &tag_map))
            })
            .collect();
        let mut out = CrmAgentRecord::new();
        out.insert("customers".into(), serde_json::Value::Array(summaries));
        if let Some(nc) = next_cursor {
            out.insert("next".into(), serde_json::Value::from(nc));
        }
        insert_hidden_fields(&mut out, &hidden);
        Ok(out)
    }

    pub fn customer(&self, agent_id: &str, customer_id: &str) -> StorageResult<CrmAgentRecord> {
        let hidden = self.allow(agent_id)?;
        let cust = self.store.customer(customer_id)?;
        let status_map = self.status_display_map()?;
        let tag_map = self.tag_display_map()?;
        let mut out = self.build_summary(&cust, &hidden, &status_map, &tag_map);

        let text_pairs: [(&str, &str, CrmPrivateField); 9] = [
            ("contactName", &cust.contact_name, CrmPrivateField::ContactName),
            ("contactEmail", &cust.contact_email, CrmPrivateField::ContactEmail),
            ("phone", &cust.phone, CrmPrivateField::Phone),
            ("billingEmail", &cust.billing_email, CrmPrivateField::BillingEmail),
            ("domain", &cust.domain, CrmPrivateField::Domain),
            ("taxId", &cust.tax_id, CrmPrivateField::TaxId),
            ("address", &cust.address, CrmPrivateField::Address),
            ("stripeCustomerId", &cust.stripe_customer_id, CrmPrivateField::StripeCustomerId),
            ("notes", &cust.notes, CrmPrivateField::Notes),
        ];
        for (key, val, pf) in text_pairs {
            let shown = if hidden.contains(&pf) {
                CRM_HIDDEN_MARKER
            } else {
                val
            };
            insert_non_empty(&mut out, key, serde_json::Value::from(shown));
        }
        if !hidden.contains(&CrmPrivateField::Owner)
            && let Some(owner) = &cust.owner_agent_id
        {
            insert_non_empty(&mut out, "owner", serde_json::Value::from(owner.clone()));
        }
        if !hidden.contains(&CrmPrivateField::Method)
            && let Some(m) = cust.method
        {
            out.insert("method".into(), serde_json::Value::from(method_str(m)));
        }
        out.insert("currency".into(), serde_json::Value::from(cust.currency.clone()));
        let amounts = !hidden.contains(&CrmPrivateField::Amounts);
        let dates = !hidden.contains(&CrmPrivateField::Dates);
        if amounts && cust.paid_minor > 0 {
            out.insert(
                "paid".into(),
                serde_json::Value::from(format_crm_amount(cust.paid_minor, &cust.currency)),
            );
        }
        if dates
            && let Some(lpa) = &cust.last_paid_at
        {
            insert_non_empty(&mut out, "lastPaidAt", serde_json::Value::from(lpa.clone()));
        }
        if !hidden.contains(&CrmPrivateField::Plans) && !cust.plans.is_empty() {
            let plans_arr: Vec<serde_json::Value> = cust
                .plans
                .iter()
                .map(|pl| {
                    let mut pm = CrmAgentRecord::new();
                    pm.insert("id".into(), serde_json::Value::from(pl.id.clone()));
                    pm.insert("name".into(), serde_json::Value::from(pl.name.clone()));
                    if amounts {
                        pm.insert(
                            "amount".into(),
                            serde_json::Value::from(format_crm_amount(
                                pl.amount_minor,
                                &cust.currency,
                            )),
                        );
                    }
                    pm.insert("every".into(), serde_json::Value::from(every_str(pl.every)));
                    if dates {
                        insert_non_empty(&mut pm, "startsOn", serde_json::Value::from(pl.starts_on.clone()));
                        if let Some(eo) = &pl.ends_on {
                            insert_non_empty(&mut pm, "endsOn", serde_json::Value::from(eo.clone()));
                        }
                    }
                    if let Some(src) = pl.source {
                        pm.insert("source".into(), serde_json::Value::from(source_kind_str(src)));
                    }
                    serde_json::Value::Object(pm)
                })
                .collect();
            out.insert("plans".into(), serde_json::Value::Array(plans_arr));
        }

        let (open_pays, paid_pays, acts) = self.store.db.with_conn(|conn| {
            let open_order = if dates { "p.due_on, p.number" } else { "p.number" };
            let paid_order = if dates {
                "p.paid_at DESC, p.number DESC"
            } else {
                "p.number DESC"
            };
            let open_p = query_customer_payments(
                conn,
                &cust.id,
                "p.paid_at IS NULL",
                open_order,
                CUSTOMER_OPEN_PAYMENTS_LIMIT,
            )?;
            let paid_p = query_customer_payments(
                conn,
                &cust.id,
                "p.paid_at IS NOT NULL",
                paid_order,
                CUSTOMER_PAID_PAYMENTS_LIMIT,
            )?;
            let mut a_stmt = conn.prepare(
                "SELECT activity_id, customer_id, actor_kind, actor_id, actor_name, kind, \
                        details_json, proposed_by, created_at \
                 FROM crm_activity WHERE customer_id = ?1 \
                 ORDER BY created_at DESC, activity_id DESC LIMIT ?2",
            )?;
            let a_rows = a_stmt.query_map(
                params![cust.id, CUSTOMER_ACTIVITY_LIMIT as i64],
                read_crm_activity_row,
            )?;
            let mut acts = Vec::new();
            for r in a_rows {
                acts.push(r??);
            }
            Ok((open_p, paid_p, acts))
        })?;

        if !open_pays.is_empty() {
            let arr = open_pays
                .iter()
                .map(|r| serde_json::Value::Object(Self::build_payment_record(r, &hidden, false)))
                .collect();
            out.insert("openPayments".into(), serde_json::Value::Array(arr));
        }
        if !paid_pays.is_empty() {
            let arr = paid_pays
                .iter()
                .map(|r| serde_json::Value::Object(Self::build_payment_record(r, &hidden, false)))
                .collect();
            out.insert("recentPaid".into(), serde_json::Value::Array(arr));
        }
        let redacted_acts: Vec<serde_json::Value> = acts
            .iter()
            .filter_map(|a| Self::redact_activity(a, &hidden).map(serde_json::Value::Object))
            .collect();
        if !redacted_acts.is_empty() {
            out.insert("activity".into(), serde_json::Value::Array(redacted_acts));
        }
        insert_hidden_fields(&mut out, &hidden);
        Ok(out)
    }

    pub fn payments(
        &self,
        agent_id: &str,
        input: &CrmAgentPaymentsInput,
    ) -> StorageResult<CrmAgentRecord> {
        let hidden = self.allow(agent_id)?;
        let dates = !hidden.contains(&CrmPrivateField::Dates);
        let mut where_clauses = vec!["p.paid_at IS NULL".to_string()];
        let mut values: Vec<SqlValue> = Vec::new();
        if input.state == CrmAgentPaymentQueryState::Late {
            if !dates {
                return Err(StorageError::Forbidden("error.crm.fieldPrivate".to_string()));
            }
            where_clauses.push("p.due_on < ?".to_string());
            values.push(SqlValue::Text(self.store.today()));
        }
        if let Some(cid) = &input.customer_id {
            self.store.customer_record(cid)?;
            where_clauses.push("p.customer_id = ?".to_string());
            values.push(SqlValue::Text(cid.clone()));
        }
        if let Some(cur) = &input.cursor {
            let (due_on, num) = decode_crm_number_cursor(cur)?;
            if dates {
                where_clauses.push("(p.due_on, p.number) > (?, ?)".to_string());
                values.push(SqlValue::Text(due_on));
                values.push(SqlValue::Integer(num as i64));
            } else {
                where_clauses.push("p.number > ?".to_string());
                values.push(SqlValue::Integer(num as i64));
            }
        }
        let limit = clamp_agent_limit(input.limit);
        values.push(SqlValue::Integer((limit + 1) as i64));
        let order = if dates { "p.due_on, p.number" } else { "p.number" };

        let mut rows = self.store.db.with_conn(|conn| {
            let mut stmt = conn.prepare(&format!(
                "{CRM_PAYMENT_SELECT_SQL} WHERE {} ORDER BY {order} LIMIT ?",
                where_clauses.join(" AND ")
            ))?;
            let mapped = stmt.query_map(params_from_iter(values), read_crm_payment_row)?;
            let mut out = Vec::new();
            for r in mapped {
                out.push(r??);
            }
            Ok(out)
        })?;
        let has_more = rows.len() > limit;
        rows.truncate(limit);
        let next = if has_more {
            rows.last().map(|last| {
                let k = if dates {
                    last.payment.due_on.as_str()
                } else {
                    ""
                };
                encode_crm_number_cursor(k, last.payment.number)
            })
        } else {
            None
        };
        let arr: Vec<serde_json::Value> = rows
            .iter()
            .map(|r| serde_json::Value::Object(Self::build_payment_record(r, &hidden, true)))
            .collect();
        let mut out = CrmAgentRecord::new();
        out.insert("payments".into(), serde_json::Value::Array(arr));
        if let Some(nc) = next {
            out.insert("next".into(), serde_json::Value::from(nc));
        }
        insert_hidden_fields(&mut out, &hidden);
        Ok(out)
    }

    pub fn propose(&self, agent_id: &str, input: &CrmAgentProposeInput) -> StorageResult<String> {
        self.allow(agent_id)?;
        let (proposed, customer_id) = self.build_proposed_change(&input.change)?;
        if !proposed.is_valid() || utf16_len(&input.reason) > CRM_LIMITS.reason {
            return Err(StorageError::Validation(
                "error.crm.invalidChange".to_string(),
            ));
        }
        if self.store.source_owns(&proposed)? {
            return Err(StorageError::Validation("error.crm.sourceOwned".to_string()));
        }
        let thread_id = input
            .thread_id
            .as_ref()
            .filter(|t| utf16_len(t) <= 128)
            .cloned();
        self.store.propose(&CrmProposeInput {
            agent_id: agent_id.to_string(),
            customer_id,
            thread_id,
            reason: input.reason.clone(),
            change: proposed,
        })
    }

    fn status_display_map(&self) -> StorageResult<HashMap<String, String>> {
        Ok(self
            .store
            .statuses()?
            .into_iter()
            .map(|s| {
                let disp = s.label.unwrap_or_else(|| s.id.clone());
                (s.id, disp)
            })
            .collect())
    }

    fn tag_display_map(&self) -> StorageResult<HashMap<String, String>> {
        Ok(self
            .store
            .tags()?
            .into_iter()
            .map(|t| (t.id, t.label))
            .collect())
    }

    fn build_proposed_change(
        &self,
        change: &CrmAgentChange,
    ) -> StorageResult<(CrmProposedChange, Option<String>)> {
        match change {
            CrmAgentChange::CustomerCreate {
                name,
                currency,
                fields,
                tags,
            } => {
                let status_id = fields
                    .status
                    .as_deref()
                    .map(|s| self.resolve_status_id(s))
                    .transpose()?;
                let tag_ids = tags
                    .as_ref()
                    .map(|list| {
                        list.iter()
                            .map(|t| self.resolve_tag_id(t))
                            .collect::<StorageResult<Vec<_>>>()
                    })
                    .transpose()?;
                let cust = CrmCustomerInput {
                    name: name.clone(),
                    currency: currency.to_ascii_uppercase(),
                    contact_name: fields.contact_name.clone(),
                    contact_email: fields.contact_email.clone(),
                    phone: fields.phone.clone(),
                    billing_email: fields.billing_email.clone(),
                    domain: fields.domain.clone(),
                    tax_id: fields.tax_id.clone(),
                    address: fields.address.clone(),
                    stripe_customer_id: fields.stripe_customer_id.clone(),
                    notes: fields.notes.clone(),
                    status_id,
                    owner_agent_id: fields.owner_agent_id.clone(),
                    method: fields.method,
                    tag_ids,
                };
                Ok((CrmProposedChange::CustomerCreate { customer: cust }, None))
            }
            CrmAgentChange::CustomerUpdate { customer_id, patch } => {
                self.store.customer_record(customer_id)?;
                let status_id = patch
                    .status
                    .as_deref()
                    .map(|s| self.resolve_status_id(s))
                    .transpose()?;
                let p = CrmCustomerPatch {
                    name: patch.name.clone(),
                    contact_name: patch.contact_name.clone(),
                    contact_email: patch.contact_email.clone(),
                    phone: patch.phone.clone(),
                    billing_email: patch.billing_email.clone(),
                    domain: patch.domain.clone(),
                    tax_id: patch.tax_id.clone(),
                    address: patch.address.clone(),
                    stripe_customer_id: patch.stripe_customer_id.clone(),
                    notes: patch.notes.clone(),
                    status_id,
                    owner_agent_id: patch.owner_agent_id.clone(),
                    method: patch.method,
                    currency: patch.currency.as_ref().map(|c| c.to_ascii_uppercase()),
                };
                Ok((
                    CrmProposedChange::CustomerUpdate {
                        customer_id: customer_id.clone(),
                        patch: p,
                    },
                    Some(customer_id.clone()),
                ))
            }
            CrmAgentChange::CustomerTags { customer_id, tags } => {
                self.store.customer_record(customer_id)?;
                let mut seen = HashSet::new();
                let mut tag_ids = Vec::new();
                for t in tags {
                    let tid = self.resolve_tag_id(t)?;
                    if seen.insert(tid.clone()) {
                        tag_ids.push(tid);
                    }
                }
                Ok((
                    CrmProposedChange::CustomerTags {
                        customer_id: customer_id.clone(),
                        tag_ids,
                    },
                    Some(customer_id.clone()),
                ))
            }
            CrmAgentChange::PlanCreate { plan } => {
                let cust = self.store.customer_record(&plan.customer_id)?;
                let amt = parse_crm_amount(&plan.amount, &cust.currency)?;
                Ok((
                    CrmProposedChange::PlanCreate {
                        plan: CrmPlanInput {
                            customer_id: plan.customer_id.clone(),
                            name: plan.name.clone(),
                            amount_minor: amt,
                            every: plan.every,
                            starts_on: plan.starts_on.clone(),
                        },
                    },
                    Some(cust.id),
                ))
            }
            CrmAgentChange::PlanUpdate { plan_id, patch } => {
                let pl = self.store.plan(plan_id)?;
                let cust = self.store.customer_record(&pl.customer_id)?;
                let amt = patch
                    .amount
                    .as_deref()
                    .map(|a| parse_crm_amount(a, &cust.currency))
                    .transpose()?;
                Ok((
                    CrmProposedChange::PlanUpdate {
                        plan_id: pl.id,
                        patch: CrmPlanPatch {
                            name: patch.name.clone(),
                            amount_minor: amt,
                            every: patch.every,
                        },
                    },
                    Some(cust.id),
                ))
            }
            CrmAgentChange::PlanEnd { plan_id, ends_on } => {
                let pl = self.store.plan(plan_id)?;
                Ok((
                    CrmProposedChange::PlanEnd {
                        plan_id: plan_id.clone(),
                        ends_on: ends_on.clone(),
                    },
                    Some(pl.customer_id),
                ))
            }
            CrmAgentChange::PaymentCreate { payment } => {
                let cust = self.store.customer_record(&payment.customer_id)?;
                let amt = parse_crm_amount(&payment.amount, &cust.currency)?;
                Ok((
                    CrmProposedChange::PaymentCreate {
                        payment: CrmPaymentInput {
                            customer_id: payment.customer_id.clone(),
                            label: payment.label.clone(),
                            amount_minor: amt,
                            due_on: payment.due_on.clone(),
                            method: payment.method,
                            paid_at: payment.paid_at.clone(),
                        },
                    },
                    Some(cust.id),
                ))
            }
            CrmAgentChange::PaymentUpdate { payment_id, patch } => {
                let rec = self.store.payment(payment_id)?;
                let amt = patch
                    .amount
                    .as_deref()
                    .map(|a| parse_crm_amount(a, &rec.payment.currency))
                    .transpose()?;
                Ok((
                    CrmProposedChange::PaymentUpdate {
                        payment_id: rec.payment.id,
                        patch: CrmPaymentPatch {
                            label: patch.label.clone(),
                            amount_minor: amt,
                            due_on: patch.due_on.clone(),
                            method: patch.method,
                        },
                    },
                    Some(rec.payment.customer_id),
                ))
            }
            CrmAgentChange::PaymentPaid {
                payment_ids,
                paid_at,
            } => {
                let mut seen = HashSet::new();
                let mut unique = Vec::new();
                let mut cust_ids = HashSet::new();
                for pid in payment_ids {
                    if seen.insert(pid.clone()) {
                        let rec = self.store.payment(pid)?;
                        cust_ids.insert(rec.payment.customer_id);
                        unique.push(pid.clone());
                    }
                }
                let single_cid = (cust_ids.len() == 1)
                    .then(|| cust_ids.into_iter().next())
                    .flatten();
                Ok((
                    CrmProposedChange::PaymentPaid {
                        payment_ids: unique,
                        paid_at: paid_at.clone(),
                    },
                    single_cid,
                ))
            }
            CrmAgentChange::PaymentSplit { payment_id, parts } => {
                let rec = self.store.payment(payment_id)?;
                let mut parsed_parts = Vec::with_capacity(parts.len());
                for pt in parts {
                    parsed_parts.push(CrmSplitPart {
                        amount_minor: parse_crm_amount(&pt.amount, &rec.payment.currency)?,
                        due_on: pt.due_on.clone(),
                    });
                }
                Ok((
                    CrmProposedChange::PaymentSplit {
                        payment_id: rec.payment.id,
                        parts: parsed_parts,
                    },
                    Some(rec.payment.customer_id),
                ))
            }
        }
    }
}

fn activity_private_parts(kind: &str, field: Option<&str>) -> Vec<CrmPrivateField> {
    let mut parts = Vec::new();
    if kind == "status-changed" {
        parts.push(CrmPrivateField::Status);
    }
    if kind == "tags-changed" {
        parts.push(CrmPrivateField::Tags);
    }
    if kind.starts_with("plan-") {
        parts.push(CrmPrivateField::Plans);
    }
    match field {
        Some("ownerAgentId") => parts.push(CrmPrivateField::Owner),
        Some("method") => parts.push(CrmPrivateField::Method),
        Some("amountMinor") => parts.push(CrmPrivateField::Amounts),
        Some("dueOn" | "paidAt") => parts.push(CrmPrivateField::Dates),
        _ => {}
    }
    parts
}

fn insert_non_empty(rec: &mut CrmAgentRecord, key: &str, value: serde_json::Value) {
    match &value {
        serde_json::Value::Null => return,
        serde_json::Value::String(s) if s.is_empty() => return,
        serde_json::Value::Array(a) if a.is_empty() => return,
        _ => {}
    }
    rec.insert(key.to_string(), value);
}

fn insert_hidden_fields(rec: &mut CrmAgentRecord, hidden: &HashSet<CrmPrivateField>) {
    let list: Vec<serde_json::Value> = CRM_PRIVATE_FIELDS
        .into_iter()
        .filter(|f| hidden.contains(f))
        .map(|f| serde_json::Value::from(private_field_str(f)))
        .collect();
    if !list.is_empty() {
        rec.insert("hiddenFields".into(), serde_json::Value::Array(list));
    }
}

fn clamp_agent_limit(limit: Option<usize>) -> usize {
    limit
        .unwrap_or(AGENT_VIEW_DEFAULT_LIMIT)
        .clamp(1, AGENT_VIEW_MAX_LIMIT)
}

fn parse_customer_number_query(query: &str) -> Option<u64> {
    let lower = query.trim().to_ascii_lowercase();
    let rest = lower
        .strip_prefix("customer")
        .map_or(lower.as_str(), str::trim_start);
    let digits = rest.strip_prefix('#').unwrap_or(rest);
    if digits.is_empty() || digits.len() > 15 || !digits.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    digits.parse().ok()
}

fn query_customer_payments(
    conn: &Connection,
    customer_id: &str,
    where_cond: &str,
    order: &str,
    limit: usize,
) -> StorageResult<Vec<CrmPaymentRecord>> {
    let mut stmt = conn.prepare(&format!(
        "{CRM_PAYMENT_SELECT_SQL} WHERE p.customer_id = ?1 AND {where_cond} ORDER BY {order} LIMIT ?2"
    ))?;
    let rows = stmt.query_map(params![customer_id, limit as i64], read_crm_payment_row)?;
    let mut out = Vec::new();
    for r in rows {
        out.push(r??);
    }
    Ok(out)
}
