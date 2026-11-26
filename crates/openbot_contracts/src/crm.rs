//! CRM entity contracts, `CRM_LIMITS` (`amount_minor <= 10^12`), 17 `CRM_PRIVATE_FIELDS`,
//! `CrmProposal`, 10 `CrmProposedChange` variants, and 21 `CrmChange` variants.
//! Mirrors `packages/contracts/src/ipc-crm.ts`.

use std::collections::{BTreeMap, HashSet};

use serde::ser::SerializeMap;
use serde::{Deserialize, Deserializer, Serialize, Serializer};

use crate::limits::{
    double_option, integer_in_range, is_bounded_string, is_calendar_date, is_identifier,
    is_iso_timestamp, non_null_option, utf16_len,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CrmLimits {
    pub name: usize,
    pub text: usize,
    pub notes: usize,
    pub label: usize,
    pub tag: usize,
    pub reason: usize,
    pub statuses: usize,
    pub tags: usize,
    pub customer_tags: usize,
    pub customer_plans: usize,
    pub agents: usize,
    pub page: usize,
    pub payment_ids: usize,
    pub split_parts: usize,
    pub pending_proposals: usize,
    pub amount_minor: i64,
}

pub const CRM_LIMITS: CrmLimits = CrmLimits {
    name: 200,
    text: 500,
    notes: 4_000,
    label: 80,
    tag: 40,
    reason: 1_000,
    statuses: 50,
    tags: 500,
    customer_tags: 50,
    customer_plans: 50,
    agents: 500,
    page: 500,
    payment_ids: 500,
    split_parts: 24,
    pending_proposals: 500,
    amount_minor: 1_000_000_000_000,
};

pub fn is_crm_currency(value: &str) -> bool {
    value.len() == 3 && value.bytes().all(|b| b.is_ascii_uppercase())
}

pub fn is_crm_amount(value: i64) -> bool {
    integer_in_range(value, 0, CRM_LIMITS.amount_minor)
}

pub fn is_crm_date(value: &str) -> bool {
    is_calendar_date(value)
}

pub fn is_crm_timestamp(value: &str) -> bool {
    utf16_len(value) <= 64 && is_iso_timestamp(value)
}

fn is_crm_label(value: &str, max: usize) -> bool {
    !value.trim().is_empty() && value.trim() == value && is_bounded_string(value, max)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum CrmBuiltinStatusId {
    Lead,
    Customer,
    Former,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum CrmTone {
    InfoLight,
    Secondary,
    Outline,
    WarningLight,
    SuccessLight,
    PrimaryLight,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum CrmMethod {
    Stripe,
    Transfer,
    Crypto,
    Paypal,
    Cash,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum CrmEvery {
    Month,
    Quarter,
    Year,
    Once,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum CrmSourceKind {
    Stripe,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum CrmSourceSyncState {
    Syncing,
    Ok,
    Error,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum CrmSourceError {
    InvalidKey,
    Permission,
    RateLimited,
    Network,
    Stripe,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum CrmActorKind {
    Person,
    Agent,
    Source,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum CrmPaymentState {
    Open,
    Paid,
}

/// All 17 `CRM_PRIVATE_FIELDS` (`10` text fields + `7` private parts).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum CrmPrivateField {
    Name,
    ContactName,
    ContactEmail,
    Phone,
    BillingEmail,
    Domain,
    TaxId,
    Address,
    StripeCustomerId,
    Notes,
    Amounts,
    Dates,
    Status,
    Tags,
    Plans,
    Method,
    Owner,
}

pub const CRM_TEXT_FIELDS: [&str; 10] = [
    "name",
    "contactName",
    "contactEmail",
    "phone",
    "billingEmail",
    "domain",
    "taxId",
    "address",
    "stripeCustomerId",
    "notes",
];

pub const CRM_PRIVATE_PARTS: [&str; 7] =
    ["amounts", "dates", "status", "tags", "plans", "method", "owner"];

pub const CRM_PRIVATE_FIELDS: [CrmPrivateField; 17] = [
    CrmPrivateField::Name,
    CrmPrivateField::ContactName,
    CrmPrivateField::ContactEmail,
    CrmPrivateField::Phone,
    CrmPrivateField::BillingEmail,
    CrmPrivateField::Domain,
    CrmPrivateField::TaxId,
    CrmPrivateField::Address,
    CrmPrivateField::StripeCustomerId,
    CrmPrivateField::Notes,
    CrmPrivateField::Amounts,
    CrmPrivateField::Dates,
    CrmPrivateField::Status,
    CrmPrivateField::Tags,
    CrmPrivateField::Plans,
    CrmPrivateField::Method,
    CrmPrivateField::Owner,
];

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CrmSettings {
    pub enabled: bool,
}

pub const DEFAULT_CRM_SETTINGS: CrmSettings = CrmSettings { enabled: false };

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CrmSourceSkipped {
    pub currency: u64,
    pub interval: u64,
    pub limit: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CrmSource {
    pub id: String,
    pub kind: CrmSourceKind,
    pub test_mode: bool,
    pub account_label: String,
    pub key_hint: String,
    pub state: CrmSourceSyncState,
    #[serde(deserialize_with = "Option::deserialize")]
    pub error_code: Option<CrmSourceError>,
    #[serde(deserialize_with = "Option::deserialize")]
    pub synced_at: Option<String>,
    pub connected_at: String,
    pub skipped: CrmSourceSkipped,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CrmStatus {
    pub id: String,
    #[serde(deserialize_with = "Option::deserialize")]
    pub label: Option<String>,
    pub tone: CrmTone,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CrmTag {
    pub id: String,
    pub label: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CrmPlan {
    pub id: String,
    pub customer_id: String,
    pub name: String,
    pub amount_minor: i64,
    pub every: CrmEvery,
    pub starts_on: String,
    #[serde(deserialize_with = "Option::deserialize")]
    pub ends_on: Option<String>,
    #[serde(deserialize_with = "Option::deserialize")]
    pub source: Option<CrmSourceKind>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CrmCustomer {
    pub id: String,
    pub number: u64,
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
    pub status_id: String,
    #[serde(deserialize_with = "Option::deserialize")]
    pub owner_agent_id: Option<String>,
    #[serde(deserialize_with = "Option::deserialize")]
    pub method: Option<CrmMethod>,
    pub currency: String,
    pub tag_ids: Vec<String>,
    pub plans: Vec<CrmPlan>,
    pub created_at: String,
    pub updated_at: String,
    pub revision: u64,
    pub paid_minor: i64,
    pub open_minor: i64,
    pub late_minor: i64,
    #[serde(deserialize_with = "Option::deserialize")]
    pub next_due_on: Option<String>,
    #[serde(deserialize_with = "Option::deserialize")]
    pub last_paid_at: Option<String>,
    #[serde(deserialize_with = "Option::deserialize")]
    pub source: Option<CrmSourceKind>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CrmPayment {
    pub id: String,
    pub number: u64,
    pub customer_id: String,
    #[serde(deserialize_with = "Option::deserialize")]
    pub plan_id: Option<String>,
    pub label: String,
    pub amount_minor: i64,
    pub currency: String,
    pub due_on: String,
    #[serde(deserialize_with = "Option::deserialize")]
    pub paid_at: Option<String>,
    #[serde(deserialize_with = "Option::deserialize")]
    pub method: Option<CrmMethod>,
    #[serde(deserialize_with = "Option::deserialize")]
    pub source: Option<CrmSourceKind>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CrmActor {
    pub kind: CrmActorKind,
    #[serde(deserialize_with = "Option::deserialize")]
    pub id: Option<String>,
    #[serde(deserialize_with = "Option::deserialize")]
    pub name: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum CrmActivityDetailValue {
    Null,
    Number(f64),
    Text(String),
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CrmActivity {
    pub id: String,
    pub customer_id: String,
    pub actor: CrmActor,
    pub kind: String,
    pub details: BTreeMap<String, CrmActivityDetailValue>,
    #[serde(deserialize_with = "Option::deserialize")]
    pub proposed_by: Option<String>,
    pub created_at: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CrmCustomerInput {
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none", deserialize_with = "non_null_option")]
    pub contact_name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none", deserialize_with = "non_null_option")]
    pub contact_email: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none", deserialize_with = "non_null_option")]
    pub phone: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none", deserialize_with = "non_null_option")]
    pub billing_email: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none", deserialize_with = "non_null_option")]
    pub domain: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none", deserialize_with = "non_null_option")]
    pub tax_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none", deserialize_with = "non_null_option")]
    pub address: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none", deserialize_with = "non_null_option")]
    pub stripe_customer_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none", deserialize_with = "non_null_option")]
    pub notes: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none", deserialize_with = "non_null_option")]
    pub status_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none", with = "double_option")]
    pub owner_agent_id: Option<Option<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none", with = "double_option")]
    pub method: Option<Option<CrmMethod>>,
    pub currency: String,
    #[serde(default, skip_serializing_if = "Option::is_none", deserialize_with = "non_null_option")]
    pub tag_ids: Option<Vec<String>>,
}

impl CrmCustomerInput {
    pub fn is_valid(&self) -> bool {
        is_crm_label(&self.name, CRM_LIMITS.name)
            && is_crm_currency(&self.currency)
            && self.contact_name.as_deref().is_none_or(|v| is_bounded_string(v, CRM_LIMITS.text))
            && self.contact_email.as_deref().is_none_or(|v| is_bounded_string(v, CRM_LIMITS.text))
            && self.phone.as_deref().is_none_or(|v| is_bounded_string(v, CRM_LIMITS.text))
            && self.billing_email.as_deref().is_none_or(|v| is_bounded_string(v, CRM_LIMITS.text))
            && self.domain.as_deref().is_none_or(|v| is_bounded_string(v, CRM_LIMITS.text))
            && self.tax_id.as_deref().is_none_or(|v| is_bounded_string(v, CRM_LIMITS.text))
            && self.address.as_deref().is_none_or(|v| is_bounded_string(v, CRM_LIMITS.text))
            && self.stripe_customer_id.as_deref().is_none_or(|v| is_bounded_string(v, CRM_LIMITS.text))
            && self.notes.as_deref().is_none_or(|v| is_bounded_string(v, CRM_LIMITS.notes))
            && self.status_id.as_deref().is_none_or(is_identifier)
            && self
                .owner_agent_id
                .as_ref()
                .is_none_or(|opt| opt.as_deref().is_none_or(is_identifier))
            && self
                .tag_ids
                .as_deref()
                .is_none_or(|t| valid_id_list(t, CRM_LIMITS.customer_tags))
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CrmCustomerPatch {
    #[serde(default, skip_serializing_if = "Option::is_none", deserialize_with = "non_null_option")]
    pub name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none", deserialize_with = "non_null_option")]
    pub contact_name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none", deserialize_with = "non_null_option")]
    pub contact_email: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none", deserialize_with = "non_null_option")]
    pub phone: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none", deserialize_with = "non_null_option")]
    pub billing_email: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none", deserialize_with = "non_null_option")]
    pub domain: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none", deserialize_with = "non_null_option")]
    pub tax_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none", deserialize_with = "non_null_option")]
    pub address: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none", deserialize_with = "non_null_option")]
    pub stripe_customer_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none", deserialize_with = "non_null_option")]
    pub notes: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none", deserialize_with = "non_null_option")]
    pub status_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none", with = "double_option")]
    pub owner_agent_id: Option<Option<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none", with = "double_option")]
    pub method: Option<Option<CrmMethod>>,
    #[serde(default, skip_serializing_if = "Option::is_none", deserialize_with = "non_null_option")]
    pub currency: Option<String>,
}

impl CrmCustomerPatch {
    pub fn is_empty(&self) -> bool {
        self.name.is_none()
            && self.contact_name.is_none()
            && self.contact_email.is_none()
            && self.phone.is_none()
            && self.billing_email.is_none()
            && self.domain.is_none()
            && self.tax_id.is_none()
            && self.address.is_none()
            && self.stripe_customer_id.is_none()
            && self.notes.is_none()
            && self.status_id.is_none()
            && self.owner_agent_id.is_none()
            && self.method.is_none()
            && self.currency.is_none()
    }

    pub fn is_valid(&self) -> bool {
        !self.is_empty()
            && self.name.as_deref().is_none_or(|n| is_crm_label(n, CRM_LIMITS.name))
            && self.contact_name.as_deref().is_none_or(|v| is_bounded_string(v, CRM_LIMITS.text))
            && self.contact_email.as_deref().is_none_or(|v| is_bounded_string(v, CRM_LIMITS.text))
            && self.phone.as_deref().is_none_or(|v| is_bounded_string(v, CRM_LIMITS.text))
            && self.billing_email.as_deref().is_none_or(|v| is_bounded_string(v, CRM_LIMITS.text))
            && self.domain.as_deref().is_none_or(|v| is_bounded_string(v, CRM_LIMITS.text))
            && self.tax_id.as_deref().is_none_or(|v| is_bounded_string(v, CRM_LIMITS.text))
            && self.address.as_deref().is_none_or(|v| is_bounded_string(v, CRM_LIMITS.text))
            && self.stripe_customer_id.as_deref().is_none_or(|v| is_bounded_string(v, CRM_LIMITS.text))
            && self.notes.as_deref().is_none_or(|v| is_bounded_string(v, CRM_LIMITS.notes))
            && self.status_id.as_deref().is_none_or(is_identifier)
            && self
                .owner_agent_id
                .as_ref()
                .is_none_or(|opt| opt.as_deref().is_none_or(is_identifier))
            && self.currency.as_deref().is_none_or(is_crm_currency)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CrmPlanInput {
    pub customer_id: String,
    pub name: String,
    pub amount_minor: i64,
    pub every: CrmEvery,
    pub starts_on: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CrmPlanPatch {
    #[serde(default, skip_serializing_if = "Option::is_none", deserialize_with = "non_null_option")]
    pub name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none", deserialize_with = "non_null_option")]
    pub amount_minor: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none", deserialize_with = "non_null_option")]
    pub every: Option<CrmEvery>,
}

impl CrmPlanPatch {
    pub fn is_empty(&self) -> bool {
        self.name.is_none() && self.amount_minor.is_none() && self.every.is_none()
    }

    pub fn is_valid(&self) -> bool {
        !self.is_empty()
            && self.name.as_deref().is_none_or(|n| is_crm_label(n, CRM_LIMITS.label))
            && self.amount_minor.is_none_or(is_crm_amount)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CrmPaymentInput {
    pub customer_id: String,
    pub label: String,
    pub amount_minor: i64,
    pub due_on: String,
    #[serde(default, skip_serializing_if = "Option::is_none", with = "double_option")]
    pub method: Option<Option<CrmMethod>>,
    #[serde(default, skip_serializing_if = "Option::is_none", with = "double_option")]
    pub paid_at: Option<Option<String>>,
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CrmPaymentPatch {
    #[serde(default, skip_serializing_if = "Option::is_none", deserialize_with = "non_null_option")]
    pub label: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none", deserialize_with = "non_null_option")]
    pub amount_minor: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none", deserialize_with = "non_null_option")]
    pub due_on: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none", with = "double_option")]
    pub method: Option<Option<CrmMethod>>,
}

impl CrmPaymentPatch {
    pub fn is_empty(&self) -> bool {
        self.label.is_none()
            && self.amount_minor.is_none()
            && self.due_on.is_none()
            && self.method.is_none()
    }

    pub fn is_valid(&self) -> bool {
        !self.is_empty()
            && self.label.as_deref().is_none_or(|l| is_crm_label(l, CRM_LIMITS.label))
            && self.amount_minor.is_none_or(is_crm_amount)
            && self.due_on.as_deref().is_none_or(is_crm_date)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CrmSplitPart {
    pub amount_minor: i64,
    pub due_on: String,
}

/// All 10 agent-proposable `CrmProposedChange` variants (`ipc-crm.ts:L277-L300`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case", deny_unknown_fields)]
pub enum CrmProposedChange {
    CustomerCreate {
        customer: CrmCustomerInput,
    },
    CustomerUpdate {
        #[serde(rename = "customerId")]
        customer_id: String,
        patch: CrmCustomerPatch,
    },
    CustomerTags {
        #[serde(rename = "customerId")]
        customer_id: String,
        #[serde(rename = "tagIds")]
        tag_ids: Vec<String>,
    },
    PlanCreate {
        plan: CrmPlanInput,
    },
    PlanUpdate {
        #[serde(rename = "planId")]
        plan_id: String,
        patch: CrmPlanPatch,
    },
    PlanEnd {
        #[serde(rename = "planId")]
        plan_id: String,
        #[serde(rename = "endsOn")]
        ends_on: String,
    },
    PaymentCreate {
        payment: CrmPaymentInput,
    },
    PaymentUpdate {
        #[serde(rename = "paymentId")]
        payment_id: String,
        patch: CrmPaymentPatch,
    },
    PaymentPaid {
        #[serde(rename = "paymentIds")]
        payment_ids: Vec<String>,
        #[serde(rename = "paidAt", deserialize_with = "Option::deserialize")]
        paid_at: Option<String>,
    },
    PaymentSplit {
        #[serde(rename = "paymentId")]
        payment_id: String,
        parts: Vec<CrmSplitPart>,
    },
}

pub const CRM_PROPOSED_KINDS: [&str; 10] = [
    "customer-create",
    "customer-update",
    "customer-tags",
    "plan-create",
    "plan-update",
    "plan-end",
    "payment-create",
    "payment-update",
    "payment-paid",
    "payment-split",
];

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CrmProposal {
    pub id: String,
    pub agent_id: String,
    #[serde(deserialize_with = "Option::deserialize")]
    pub customer_id: Option<String>,
    #[serde(deserialize_with = "Option::deserialize")]
    pub thread_id: Option<String>,
    pub reason: String,
    pub change: CrmProposedChange,
    pub created_at: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CrmOverview {
    pub statuses: Vec<CrmStatus>,
    pub tags: Vec<CrmTag>,
    pub private_fields: Vec<CrmPrivateField>,
    pub agent_ids: Vec<String>,
    pub proposals: Vec<CrmProposal>,
    pub sources: Vec<CrmSource>,
}

/// All 21 direct UI/admin `CrmChange` variants (`ipc-crm.ts:L302-L330`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case", deny_unknown_fields)]
pub enum CrmChange {
    CustomerCreate {
        customer: CrmCustomerInput,
    },
    CustomerUpdate {
        #[serde(rename = "customerId")]
        customer_id: String,
        patch: CrmCustomerPatch,
        #[serde(rename = "baseRevision")]
        base_revision: u64,
    },
    CustomerTags {
        #[serde(rename = "customerId")]
        customer_id: String,
        #[serde(rename = "tagIds")]
        tag_ids: Vec<String>,
    },
    PlanCreate {
        plan: CrmPlanInput,
    },
    PlanUpdate {
        #[serde(rename = "planId")]
        plan_id: String,
        patch: CrmPlanPatch,
    },
    PlanEnd {
        #[serde(rename = "planId")]
        plan_id: String,
        #[serde(rename = "endsOn")]
        ends_on: String,
    },
    PaymentCreate {
        payment: CrmPaymentInput,
    },
    PaymentUpdate {
        #[serde(rename = "paymentId")]
        payment_id: String,
        patch: CrmPaymentPatch,
    },
    PaymentPaid {
        #[serde(rename = "paymentIds")]
        payment_ids: Vec<String>,
        #[serde(rename = "paidAt", deserialize_with = "Option::deserialize")]
        paid_at: Option<String>,
    },
    PaymentSplit {
        #[serde(rename = "paymentId")]
        payment_id: String,
        parts: Vec<CrmSplitPart>,
    },
    CustomerDelete {
        #[serde(rename = "customerId")]
        customer_id: String,
    },
    TagCreate {
        label: String,
    },
    TagDelete {
        #[serde(rename = "tagId")]
        tag_id: String,
    },
    StatusCreate {
        label: String,
    },
    StatusRename {
        #[serde(rename = "statusId")]
        status_id: String,
        label: String,
    },
    StatusDelete {
        #[serde(rename = "statusId")]
        status_id: String,
        #[serde(rename = "replaceWith")]
        replace_with: String,
    },
    PlanDelete {
        #[serde(rename = "planId")]
        plan_id: String,
    },
    PaymentDelete {
        #[serde(rename = "paymentId")]
        payment_id: String,
    },
    PrivateFields {
        fields: Vec<CrmPrivateField>,
    },
    AgentAccess {
        #[serde(rename = "agentIds")]
        agent_ids: Vec<String>,
    },
    ProposalDecide {
        #[serde(rename = "proposalId")]
        proposal_id: String,
        approve: bool,
    },
}

pub const CRM_CHANGE_KINDS: [&str; 21] = [
    "customer-create",
    "customer-update",
    "customer-tags",
    "plan-create",
    "plan-update",
    "plan-end",
    "payment-create",
    "payment-update",
    "payment-paid",
    "payment-split",
    "customer-delete",
    "tag-create",
    "tag-delete",
    "status-create",
    "status-rename",
    "status-delete",
    "plan-delete",
    "payment-delete",
    "private-fields",
    "agent-access",
    "proposal-decide",
];

fn valid_id_list(ids: &[String], max: usize) -> bool {
    if ids.len() > max {
        return false;
    }
    let mut seen = HashSet::new();
    ids.iter().all(|id| is_identifier(id) && seen.insert(id.as_str()))
}

impl CrmProposedChange {
    pub fn is_valid(&self) -> bool {
        match self {
            Self::CustomerCreate { customer } => customer.is_valid(),
            Self::CustomerUpdate { customer_id, patch } => {
                is_identifier(customer_id) && patch.is_valid()
            }
            Self::CustomerTags { customer_id, tag_ids } => {
                is_identifier(customer_id) && valid_id_list(tag_ids, CRM_LIMITS.customer_tags)
            }
            Self::PlanCreate { plan } => {
                is_identifier(&plan.customer_id)
                    && is_crm_label(&plan.name, CRM_LIMITS.label)
                    && is_crm_amount(plan.amount_minor)
                    && is_crm_date(&plan.starts_on)
            }
            Self::PlanUpdate { plan_id, patch } => is_identifier(plan_id) && patch.is_valid(),
            Self::PlanEnd { plan_id, ends_on } => is_identifier(plan_id) && is_crm_date(ends_on),
            Self::PaymentCreate { payment } => {
                is_identifier(&payment.customer_id)
                    && is_crm_label(&payment.label, CRM_LIMITS.label)
                    && is_crm_amount(payment.amount_minor)
                    && is_crm_date(&payment.due_on)
                    && payment
                        .paid_at
                        .as_ref()
                        .is_none_or(|opt| opt.as_deref().is_none_or(is_crm_timestamp))
            }
            Self::PaymentUpdate { payment_id, patch } => {
                is_identifier(payment_id) && patch.is_valid()
            }
            Self::PaymentPaid { payment_ids, paid_at } => {
                !payment_ids.is_empty()
                    && valid_id_list(payment_ids, CRM_LIMITS.payment_ids)
                    && paid_at.as_deref().is_none_or(is_crm_timestamp)
            }
            Self::PaymentSplit { payment_id, parts } => {
                is_identifier(payment_id)
                    && (2..=CRM_LIMITS.split_parts).contains(&parts.len())
                    && parts.iter().all(|p| {
                        p.amount_minor > 0 && is_crm_amount(p.amount_minor) && is_crm_date(&p.due_on)
                    })
            }
        }
    }
}

impl CrmChange {
    pub fn is_valid(&self) -> bool {
        match self {
            Self::StatusDelete {
                status_id,
                replace_with,
            } => is_identifier(status_id) && is_identifier(replace_with) && status_id != replace_with,
            Self::CustomerUpdate {
                customer_id,
                patch,
                base_revision,
            } => {
                is_identifier(customer_id)
                    && patch.is_valid()
                    && (1..=9_007_199_254_740_991).contains(base_revision)
            }
            Self::TagCreate { label } => is_crm_label(label, CRM_LIMITS.tag),
            Self::StatusCreate { label } => is_crm_label(label, CRM_LIMITS.label),
            Self::StatusRename { status_id, label } => {
                is_identifier(status_id) && is_crm_label(label, CRM_LIMITS.label)
            }
            Self::AgentAccess { agent_ids } => valid_id_list(agent_ids, CRM_LIMITS.agents),
            Self::CustomerDelete { customer_id } => is_identifier(customer_id),
            Self::TagDelete { tag_id } => is_identifier(tag_id),
            Self::PlanDelete { plan_id } => is_identifier(plan_id),
            Self::PaymentDelete { payment_id } => is_identifier(payment_id),
            Self::ProposalDecide { proposal_id, .. } => is_identifier(proposal_id),
            Self::PrivateFields { fields } => {
                let mut seen = HashSet::new();
                fields.len() <= CRM_PRIVATE_FIELDS.len() && fields.iter().all(|f| seen.insert(*f))
            }
            Self::CustomerCreate { customer } => customer.is_valid(),
            Self::CustomerTags { customer_id, tag_ids } => {
                is_identifier(customer_id) && valid_id_list(tag_ids, CRM_LIMITS.customer_tags)
            }
            Self::PlanCreate { plan } => CrmProposedChange::PlanCreate { plan: plan.clone() }.is_valid(),
            Self::PlanUpdate { plan_id, patch } => is_identifier(plan_id) && patch.is_valid(),
            Self::PlanEnd { plan_id, ends_on } => is_identifier(plan_id) && is_crm_date(ends_on),
            Self::PaymentCreate { payment } => CrmProposedChange::PaymentCreate { payment: payment.clone() }.is_valid(),
            Self::PaymentUpdate { payment_id, patch } => is_identifier(payment_id) && patch.is_valid(),
            Self::PaymentPaid { payment_ids, paid_at } => {
                !payment_ids.is_empty()
                    && valid_id_list(payment_ids, CRM_LIMITS.payment_ids)
                    && paid_at.as_deref().is_none_or(is_crm_timestamp)
            }
            Self::PaymentSplit { payment_id, parts } => CrmProposedChange::PaymentSplit { payment_id: payment_id.clone(), parts: parts.clone() }.is_valid(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum CrmConflictReason {
    Revision,
}

/// Discriminated strictly on `"ok": true` (`id: string | null`) vs `"ok": false` (`conflict: "revision"`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CrmApplyResult {
    Applied { id: Option<String> },
    Conflict { conflict: CrmConflictReason },
}

impl CrmApplyResult {
    pub fn is_valid(&self) -> bool {
        match self {
            Self::Applied { id } => id.as_deref().is_none_or(is_identifier),
            Self::Conflict {
                conflict: CrmConflictReason::Revision,
            } => true,
        }
    }
}

impl Serialize for CrmApplyResult {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let mut map = serializer.serialize_map(Some(2))?;
        match self {
            Self::Applied { id } => {
                map.serialize_entry("ok", &true)?;
                map.serialize_entry("id", id)?;
            }
            Self::Conflict { conflict } => {
                map.serialize_entry("ok", &false)?;
                map.serialize_entry("conflict", conflict)?;
            }
        }
        map.end()
    }
}

impl<'de> Deserialize<'de> for CrmApplyResult {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let val = serde_json::Value::deserialize(deserializer)?;
        let Some(obj) = val.as_object() else {
            return Err(serde::de::Error::custom("CrmApplyResult must be an object"));
        };
        match obj.get("ok").and_then(serde_json::Value::as_bool) {
            Some(true) => {
                let id = match obj.get("id") {
                    Some(serde_json::Value::Null) => None,
                    Some(serde_json::Value::String(s)) if is_identifier(s) => Some(s.clone()),
                    _ => {
                        return Err(serde::de::Error::custom(
                            "CrmApplyResult.id must be null or a valid identifier",
                        ));
                    }
                };
                Ok(Self::Applied { id })
            }
            Some(false) => {
                if obj.get("conflict").and_then(serde_json::Value::as_str) == Some("revision") {
                    Ok(Self::Conflict {
                        conflict: CrmConflictReason::Revision,
                    })
                } else {
                    Err(serde::de::Error::custom(
                        "CrmApplyResult conflict must be \"revision\"",
                    ))
                }
            }
            None => Err(serde::de::Error::custom("CrmApplyResult.ok must be a boolean")),
        }
    }
}
