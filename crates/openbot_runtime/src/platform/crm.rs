//! 13-table SQLite CRM service (`CrmService`), Stripe sync, 17-field agent redaction,
//! and human-gated `crm_propose` orchestration (`OB-F12`).

use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::sync::{Arc, OnceLock};

use chrono::{SecondsFormat, TimeZone, Utc};
use openbot_contracts::crm::{
    CRM_LIMITS, CRM_PRIVATE_FIELDS, CrmActor, CrmActorKind, CrmApplyResult, CrmChange,
    CrmConflictReason, CrmCustomerInput, CrmCustomerPatch, CrmEvery, CrmMethod, CrmPaymentInput,
    CrmPaymentPatch, CrmPlanInput, CrmPlanPatch, CrmPrivateField, CrmProposedChange, CrmSettings,
    CrmSourceKind, CrmSourceSkipped, CrmSourceSyncState, CrmSplitPart, DEFAULT_CRM_SETTINGS,
    is_crm_amount, is_crm_currency,
};
use openbot_contracts::limits::utf16_len;
use openbot_storage::{
    CrmAgentFindInput, CrmAgentPaymentsInput, CrmAgentProposeInput, CrmAgentView,
    CrmCustomerPageInput, CrmProposeInput, CrmSourceStore, CrmStore, InsertCrmSourceInput,
    JsonPreferenceStore, OpenBotDatabase, SealedKeyStore, SourceBatch, SourceCustomer,
    SourcePayment, SourcePlan, SourceStatePatch, StorageError, local_crm_date_from_ms,
    preference_files, register_secret_value,
};
use regex::Regex;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};

use crate::error::{OpenBotRuntimeError, RuntimeResult};

const HUNDREDTHS_ONLY: [&str; 2] = ["ISK", "UGX"];
const BUILTIN_STATUS_IDS: [&str; 3] = ["lead", "customer", "former"];

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CrmSyncRequest {
    pub source: String,
    pub api_key: Option<String>,
    pub fixture_json: Option<Value>,
    pub private_fields: Option<Vec<String>>,
    pub for_agent_id: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CrmSyncOutcome {
    pub tables_count: usize,
    pub builtin_statuses: Vec<String>,
    pub synced_customers: usize,
    pub synced_plans: usize,
    pub synced_payments: usize,
    pub source_id: Option<String>,
    pub private_fields: Vec<String>,
    pub agent_view: Option<Value>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CrmProposeRequest {
    pub agent_id: Option<String>,
    pub operation: Option<String>,
    pub payload: Option<Value>,
    pub reason: Option<String>,
    pub proposal_id: Option<String>,
    pub approve: bool,
    pub reject: bool,
    pub commit_unapproved: bool,
    pub direct_change: Option<Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CrmProposeOutcome {
    pub proposal_id: Option<String>,
    pub status: String,
    pub approval_required: bool,
    pub committed_id: Option<String>,
}

pub struct CrmService {
    database: OpenBotDatabase,
    store: CrmStore,
    sources: CrmSourceStore,
    agent_view: CrmAgentView,
    settings_store: JsonPreferenceStore<CrmSettings>,
    stripe_key_store: SealedKeyStore,
}

impl CrmService {
    pub fn open(database: OpenBotDatabase, data_dir: impl AsRef<Path>) -> RuntimeResult<Arc<Self>> {
        let dir = data_dir.as_ref();
        let settings_store =
            JsonPreferenceStore::new(dir.join(preference_files::CRM_V1), DEFAULT_CRM_SETTINGS);
        let _ = settings_store.load()?;
        let stripe_key_store =
            SealedKeyStore::new_passthrough(dir.join(preference_files::CRM_STRIPE_KEY));
        if let Some(existing_key) = stripe_key_store.load()? {
            register_secret_value(&existing_key, false);
        }
        let store = CrmStore::new(database.clone());
        let sources = CrmSourceStore::new(store.clone());
        let agent_view = CrmAgentView::new(store.clone());
        Ok(Arc::new(Self {
            database,
            store,
            sources,
            agent_view,
            settings_store,
            stripe_key_store,
        }))
    }

    pub fn settings(&self) -> RuntimeResult<CrmSettings> {
        Ok(self.settings_store.get())
    }

    pub fn set_enabled(&self, enabled: bool) -> RuntimeResult<CrmSettings> {
        let next = self.settings_store.set(CrmSettings { enabled })?;
        Ok(next)
    }

    pub fn sync_stripe(&self, req: CrmSyncRequest) -> RuntimeResult<CrmSyncOutcome> {
        let source_name = req.source.trim();
        if !source_name.is_empty() && !source_name.eq_ignore_ascii_case("stripe") {
            return Err(OpenBotRuntimeError::Validation(format!(
                "Unsupported CRM source '{source_name}'; only 'stripe' is supported."
            )));
        }

        let mut test_mode = true;
        let mut key_hint = String::new();
        if let Some(raw_key) = req.api_key.as_deref() {
            let trimmed = raw_key.trim();
            test_mode = validate_stripe_restricted_key(trimmed)?;
            key_hint = stripe_key_hint(trimmed);
            self.stripe_key_store.set(Some(trimmed))?;
            register_secret_value(trimmed, false);
        }

        if let Some(raw_fields) = req.private_fields {
            let parsed_fields = parse_private_fields(&raw_fields)?;
            self.store
                .apply(
                    &CrmChange::PrivateFields {
                        fields: parsed_fields,
                    },
                    &person_actor(),
                )
                .map_err(map_crm_storage_err)?;
        }

        let mut source_id: Option<String> = self
            .store
            .sources()
            .map_err(map_crm_storage_err)?
            .into_iter()
            .find(|s| s.kind == CrmSourceKind::Stripe)
            .map(|s| s.id);
        let mut synced_customers = 0usize;
        let mut synced_plans = 0usize;
        let mut synced_payments = 0usize;

        if req.fixture_json.is_some() || req.api_key.is_some() {
            let sid = match source_id.clone() {
                Some(existing) => existing,
                None => {
                    let inserted = self
                        .sources
                        .insert(&InsertCrmSourceInput {
                            kind: CrmSourceKind::Stripe,
                            test_mode,
                            account_label: "Stripe".to_string(),
                            key_hint: if key_hint.is_empty() {
                                "sync".to_string()
                            } else {
                                key_hint
                            },
                        })
                        .map_err(map_crm_storage_err)?;
                    inserted.id
                }
            };
            let mut skipped = CrmSourceSkipped {
                currency: 0,
                interval: 0,
                limit: 0,
            };
            if let Some(fixture) = req.fixture_json.as_ref() {
                let today = self.store.today();
                let now_iso = self.store.timestamp();
                let batch = parse_stripe_fixture(fixture, &today, &now_iso)?;
                synced_customers = count_unique_batch_customers(&batch);
                synced_plans = batch.plans.len();
                synced_payments = batch.payments.len();
                self.sources
                    .apply(&sid, &batch, &mut skipped)
                    .map_err(map_crm_storage_err)?;
            }
            self.sources
                .set_state(
                    &sid,
                    &SourceStatePatch {
                        state: CrmSourceSyncState::Ok,
                        error_code: None,
                        synced_at: Some(self.store.timestamp()),
                        skipped: Some(skipped),
                    },
                )
                .map_err(map_crm_storage_err)?;
            source_id = Some(sid);
        }

        let tables_count = self.query_crm_tables_count()?;
        let builtin_statuses = self.query_builtin_statuses()?;
        let private_field_names: Vec<String> = self
            .store
            .private_fields()
            .map_err(map_crm_storage_err)?
            .into_iter()
            .map(|f| private_field_name(f).to_string())
            .collect();

        let agent_view = if let Some(raw_agent_id) = req.for_agent_id.as_deref() {
            let agent_id = raw_agent_id.trim();
            if agent_id.is_empty() {
                return Err(OpenBotRuntimeError::Validation(
                    "forAgentId must not be empty.".to_string(),
                ));
            }
            self.ensure_agent_access(agent_id)?;
            let mut customers_out = Vec::new();
            let mut after: Option<u64> = None;
            loop {
                let page = self
                    .store
                    .customers(&CrmCustomerPageInput { after })
                    .map_err(map_crm_storage_err)?;
                for cust in &page.customers {
                    let view_rec = self
                        .agent_view
                        .customer(agent_id, &cust.id)
                        .map_err(map_crm_storage_err)?;
                    customers_out.push(Value::Object(view_rec));
                }
                match page.next {
                    Some(next_num) => after = Some(next_num),
                    None => break,
                }
            }
            Some(json!({
                "agentId": agent_id,
                "customers": customers_out,
                "redacted_fields": private_field_names.clone(),
                "redactedFields": private_field_names.clone(),
            }))
        } else {
            None
        };

        Ok(CrmSyncOutcome {
            tables_count,
            builtin_statuses,
            synced_customers,
            synced_plans,
            synced_payments,
            source_id,
            private_fields: private_field_names,
            agent_view,
        })
    }

    pub fn propose_or_apply(&self, req: CrmProposeRequest) -> RuntimeResult<CrmProposeOutcome> {
        if req.commit_unapproved {
            return Err(OpenBotRuntimeError::Forbidden(
                "Proposals require human (person) approval and are pending approval; agents cannot commit unapproved proposals."
                    .to_string(),
            ));
        }

        if let Some(raw_direct) = req.direct_change {
            let change: CrmChange = serde_json::from_value(raw_direct).map_err(|err| {
                OpenBotRuntimeError::Validation(format!("Invalid CRM direct change: {err}"))
            })?;
            self.prevalidate_direct_change(&change)?;
            let applied = self
                .store
                .apply(&change, &person_actor())
                .map_err(map_crm_storage_err)?;
            return match applied {
                CrmApplyResult::Applied { id } => Ok(CrmProposeOutcome {
                    proposal_id: None,
                    status: "committed".to_string(),
                    approval_required: false,
                    committed_id: id,
                }),
                CrmApplyResult::Conflict {
                    conflict: CrmConflictReason::Revision,
                } => Err(OpenBotRuntimeError::Conflict(
                    "error.crm.revisionConflict: Customer baseRevision conflict; reload and retry."
                        .to_string(),
                )),
            };
        }

        if req.approve || req.reject {
            if req.approve && req.reject {
                return Err(OpenBotRuntimeError::Validation(
                    "Cannot both approve and reject the same proposal.".to_string(),
                ));
            }
            let pid = req
                .proposal_id
                .as_deref()
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .ok_or_else(|| {
                    OpenBotRuntimeError::Validation(
                        "proposalId is required to approve or reject a CRM proposal.".to_string(),
                    )
                })?
                .to_string();
            let change = CrmChange::ProposalDecide {
                proposal_id: pid.clone(),
                approve: req.approve,
            };
            let applied = self
                .store
                .apply(&change, &person_actor())
                .map_err(map_crm_storage_err)?;
            return match applied {
                CrmApplyResult::Applied { id } => Ok(CrmProposeOutcome {
                    proposal_id: Some(pid),
                    status: if req.approve { "committed" } else { "rejected" }.to_string(),
                    approval_required: false,
                    committed_id: if req.approve { id } else { None },
                }),
                CrmApplyResult::Conflict {
                    conflict: CrmConflictReason::Revision,
                } => Err(OpenBotRuntimeError::Conflict(
                    "error.crm.revisionConflict: Proposal conflicted on revision.".to_string(),
                )),
            };
        }

        let agent_id = req
            .agent_id
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .ok_or_else(|| {
                OpenBotRuntimeError::Validation(
                    "agentId is required to create a CRM proposal.".to_string(),
                )
            })?
            .to_string();
        let operation = req
            .operation
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .ok_or_else(|| {
                OpenBotRuntimeError::Validation(
                    "operation is required to create a CRM proposal.".to_string(),
                )
            })?;
        let payload = req.payload.unwrap_or_else(|| Value::Object(Map::new()));
        let reason = req.reason.unwrap_or_default();
        if utf16_len(&reason) > CRM_LIMITS.reason {
            return Err(OpenBotRuntimeError::Validation(format!(
                "Proposal reason exceeds maximum length of {} characters.",
                CRM_LIMITS.reason
            )));
        }

        let (proposed, customer_id, base_revision) =
            build_proposed_change(operation, &payload)?;
        self.prevalidate_proposed_change(&proposed, base_revision)?;

        let pid = self
            .store
            .propose(&CrmProposeInput {
                agent_id,
                customer_id,
                thread_id: None,
                reason,
                change: proposed,
            })
            .map_err(map_crm_storage_err)?;

        Ok(CrmProposeOutcome {
            proposal_id: Some(pid),
            status: "pending_approval".to_string(),
            approval_required: true,
            committed_id: None,
        })
    }

    pub fn execute_agent_tool(
        &self,
        agent_id: &str,
        tool_name: &str,
        args: &Value,
    ) -> RuntimeResult<Value> {
        if !self.settings()?.enabled {
            return Err(OpenBotRuntimeError::Forbidden(
                "error.crm.off: CRM is disabled.".to_string(),
            ));
        }
        match tool_name {
            "crm_find" => {
                let input: CrmAgentFindInput = serde_json::from_value(args.clone()).map_err(|e| {
                    OpenBotRuntimeError::Validation(format!("Invalid crm_find arguments: {e}"))
                })?;
                let rec = self
                    .agent_view
                    .find(agent_id, &input)
                    .map_err(map_crm_storage_err)?;
                Ok(Value::Object(rec))
            }
            "crm_customer" => {
                let cid = args
                    .get("customerId")
                    .or_else(|| args.get("customer_id"))
                    .and_then(Value::as_str)
                    .map(str::trim)
                    .filter(|s| !s.is_empty())
                    .ok_or_else(|| {
                        OpenBotRuntimeError::Validation(
                            "crm_customer requires customerId.".to_string(),
                        )
                    })?;
                let rec = self
                    .agent_view
                    .customer(agent_id, cid)
                    .map_err(map_crm_storage_err)?;
                Ok(Value::Object(rec))
            }
            "crm_payments" => {
                let input: CrmAgentPaymentsInput =
                    serde_json::from_value(args.clone()).map_err(|e| {
                        OpenBotRuntimeError::Validation(format!(
                            "Invalid crm_payments arguments: {e}"
                        ))
                    })?;
                let rec = self
                    .agent_view
                    .payments(agent_id, &input)
                    .map_err(map_crm_storage_err)?;
                Ok(Value::Object(rec))
            }
            "crm_propose" => {
                let input: CrmAgentProposeInput =
                    serde_json::from_value(args.clone()).map_err(|e| {
                        OpenBotRuntimeError::Validation(format!(
                            "Invalid crm_propose arguments: {e}"
                        ))
                    })?;
                let pid = self
                    .agent_view
                    .propose(agent_id, &input)
                    .map_err(map_crm_storage_err)?;
                Ok(json!({ "proposalId": pid }))
            }
            other => Err(OpenBotRuntimeError::Validation(format!(
                "Unknown CRM agent tool '{other}'."
            ))),
        }
    }

    fn query_crm_tables_count(&self) -> RuntimeResult<usize> {
        let count = self.database.with_conn(|conn| {
            let n: i64 = conn.query_row(
                "SELECT COUNT(*) FROM sqlite_master WHERE type = 'table' AND name LIKE 'crm_%'",
                [],
                |r| r.get(0),
            )?;
            Ok(usize::try_from(n).unwrap_or(0))
        })?;
        Ok(count)
    }

    fn query_builtin_statuses(&self) -> RuntimeResult<Vec<String>> {
        let existing: HashSet<String> = self
            .store
            .statuses()
            .map_err(map_crm_storage_err)?
            .into_iter()
            .filter(|s| s.label.is_none())
            .map(|s| s.id)
            .collect();
        let mut ordered: Vec<String> = BUILTIN_STATUS_IDS
            .iter()
            .filter(|id| existing.contains(**id))
            .map(|id| (*id).to_string())
            .collect();
        if ordered.is_empty() {
            ordered = BUILTIN_STATUS_IDS.iter().map(|s| (*s).to_string()).collect();
        }
        Ok(ordered)
    }

    fn ensure_agent_access(&self, agent_id: &str) -> RuntimeResult<()> {
        if self
            .store
            .has_agent_access(agent_id)
            .map_err(map_crm_storage_err)?
        {
            return Ok(());
        }
        let mut agent_ids = self
            .store
            .overview()
            .map_err(map_crm_storage_err)?
            .agent_ids;
        if !agent_ids.iter().any(|id| id == agent_id) {
            agent_ids.push(agent_id.to_string());
        }
        self.store
            .apply(&CrmChange::AgentAccess { agent_ids }, &person_actor())
            .map_err(map_crm_storage_err)?;
        Ok(())
    }

    fn prevalidate_direct_change(&self, change: &CrmChange) -> RuntimeResult<()> {
        match change {
            CrmChange::StatusRename { status_id, .. }
            | CrmChange::StatusDelete { status_id, .. }
                if BUILTIN_STATUS_IDS.contains(&status_id.as_str()) =>
            {
                return Err(OpenBotRuntimeError::Forbidden(format!(
                    "error.crm.builtinStatus: Built-in status '{status_id}' is immutable and cannot be renamed or deleted."
                )));
            }
            CrmChange::CustomerCreate { customer } => {
                validate_currency(&customer.currency)?;
            }
            CrmChange::CustomerUpdate {
                customer_id, patch, ..
            } => {
                if let Some(ccy) = patch.currency.as_deref() {
                    validate_currency(ccy)?;
                }
                if let Ok(existing) = self.store.customer_record(customer_id)
                    && existing.source_id.is_some()
                    && patch
                        .stripe_customer_id
                        .as_deref()
                        .is_some_and(|next| next != existing.text.stripe_customer_id)
                {
                    return Err(OpenBotRuntimeError::Forbidden(
                        "error.crm.sourceOwned: Stripe owns this record (sourceOwned, read-only)."
                            .to_string(),
                    ));
                }
            }
            CrmChange::PlanCreate { plan } => validate_amount(plan.amount_minor)?,
            CrmChange::PlanUpdate { patch, .. } => {
                if let Some(a) = patch.amount_minor {
                    validate_amount(a)?;
                }
            }
            CrmChange::PaymentCreate { payment } => validate_amount(payment.amount_minor)?,
            CrmChange::PaymentUpdate { patch, .. } => {
                if let Some(a) = patch.amount_minor {
                    validate_amount(a)?;
                }
            }
            CrmChange::PaymentSplit { payment_id, parts } => {
                self.prevalidate_payment_split(payment_id, parts)?;
            }
            _ => {}
        }
        Ok(())
    }

    fn prevalidate_proposed_change(
        &self,
        proposed: &CrmProposedChange,
        base_revision: Option<u64>,
    ) -> RuntimeResult<()> {
        match proposed {
            CrmProposedChange::CustomerCreate { customer } => {
                validate_currency(&customer.currency)?;
            }
            CrmProposedChange::CustomerUpdate { customer_id, patch } => {
                if let Some(ccy) = patch.currency.as_deref() {
                    validate_currency(ccy)?;
                }
                if let Some(expected_rev) = base_revision {
                    let rec = self
                        .store
                        .customer_record(customer_id)
                        .map_err(map_crm_storage_err)?;
                    if rec.revision != expected_rev {
                        return Err(OpenBotRuntimeError::Conflict(format!(
                            "error.crm.revisionConflict: baseRevision {expected_rev} does not match current revision {}.",
                            rec.revision
                        )));
                    }
                }
            }
            CrmProposedChange::PlanCreate { plan } => {
                validate_amount(plan.amount_minor)?;
            }
            CrmProposedChange::PlanUpdate { patch, .. } => {
                if let Some(a) = patch.amount_minor {
                    validate_amount(a)?;
                }
            }
            CrmProposedChange::PaymentCreate { payment } => {
                validate_amount(payment.amount_minor)?;
            }
            CrmProposedChange::PaymentUpdate { patch, .. } => {
                if let Some(a) = patch.amount_minor {
                    validate_amount(a)?;
                }
            }
            CrmProposedChange::PaymentSplit { payment_id, parts } => {
                self.prevalidate_payment_split(payment_id, parts)?;
            }
            CrmProposedChange::CustomerTags { .. }
            | CrmProposedChange::PlanEnd { .. }
            | CrmProposedChange::PaymentPaid { .. } => {}
        }

        if self.store.source_owns(proposed).map_err(map_crm_storage_err)? {
            return Err(OpenBotRuntimeError::Forbidden(
                "error.crm.sourceOwned: Stripe owns this record (sourceOwned, read-only)."
                    .to_string(),
            ));
        }

        if !proposed.is_valid() {
            return Err(OpenBotRuntimeError::Validation(
                "error.crm.invalidChange: Proposed CRM change failed contract validation."
                    .to_string(),
            ));
        }
        Ok(())
    }

    fn prevalidate_payment_split(
        &self,
        payment_id: &str,
        parts: &[CrmSplitPart],
    ) -> RuntimeResult<()> {
        if parts.len() < 2 || parts.len() > CRM_LIMITS.split_parts {
            return Err(OpenBotRuntimeError::Validation(format!(
                "error.crm.invalidChange: payment-split requires between 2 and {} parts.",
                CRM_LIMITS.split_parts
            )));
        }
        let mut total: i64 = 0;
        for part in parts {
            validate_amount(part.amount_minor)?;
            if part.amount_minor <= 0 {
                return Err(OpenBotRuntimeError::Validation(
                    "error.crm.splitTotal: Each split part amountMinor must be positive."
                        .to_string(),
                ));
            }
            total = total.checked_add(part.amount_minor).ok_or_else(|| {
                OpenBotRuntimeError::Validation(
                    "error.crm.splitTotal: Split parts sum overflows bounds.".to_string(),
                )
            })?;
        }
        let rec = self
            .store
            .payment(payment_id)
            .map_err(map_crm_storage_err)?;
        if rec.payment.source.is_some() {
            return Err(OpenBotRuntimeError::Forbidden(
                "error.crm.sourceOwned: Stripe owns this payment (sourceOwned, read-only)."
                    .to_string(),
            ));
        }
        if rec.payment.paid_at.is_some() {
            return Err(OpenBotRuntimeError::Validation(
                "error.crm.paymentPaid: Cannot split an already-paid payment.".to_string(),
            ));
        }
        if total != rec.payment.amount_minor {
            return Err(OpenBotRuntimeError::Validation(format!(
                "error.crm.splitTotal: Split parts sum ({total}) does not match payment amount ({}) (splitTotal mismatch).",
                rec.payment.amount_minor
            )));
        }
        Ok(())
    }
}

fn person_actor() -> CrmActor {
    CrmActor {
        kind: CrmActorKind::Person,
        id: Some("owner".to_string()),
        name: Some("Owner".to_string()),
    }
}

fn map_crm_storage_err(err: StorageError) -> OpenBotRuntimeError {
    match err {
        StorageError::Validation(msg) => {
            if msg.contains("sourceOwned") {
                OpenBotRuntimeError::Forbidden(
                    "error.crm.sourceOwned: Stripe owns this record (sourceOwned, read-only)."
                        .to_string(),
                )
            } else if msg.contains("builtinStatus") {
                OpenBotRuntimeError::Forbidden(
                    "error.crm.builtinStatus: Built-in statuses (lead, customer, former) are immutable and cannot be renamed or deleted."
                        .to_string(),
                )
            } else if msg.contains("splitTotal") {
                OpenBotRuntimeError::Validation(
                    "error.crm.splitTotal: Split parts sum does not match payment amount (splitTotal mismatch)."
                        .to_string(),
                )
            } else {
                OpenBotRuntimeError::Validation(msg)
            }
        }
        StorageError::LimitExceeded(msg) => {
            if msg.contains("tooManyProposals") {
                OpenBotRuntimeError::LimitExceeded(
                    "error.crm.tooManyProposals: Agent pending proposals limit (50) exceeded."
                        .to_string(),
                )
            } else {
                OpenBotRuntimeError::LimitExceeded(msg)
            }
        }
        StorageError::NotFound(msg) => OpenBotRuntimeError::NotFound(msg),
        StorageError::Conflict(msg) => OpenBotRuntimeError::Conflict(msg),
        StorageError::Forbidden(msg) => OpenBotRuntimeError::Forbidden(msg),
        other => OpenBotRuntimeError::Storage(other),
    }
}

fn stripe_restricted_key_regex() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r"^rk_(live|test)_[A-Za-z0-9_]+$").expect("valid stripe key regex"))
}

fn validate_stripe_restricted_key(key: &str) -> RuntimeResult<bool> {
    if key.starts_with("sk_") {
        return Err(OpenBotRuntimeError::Validation(
            "error.crm.sourceSecretKey: Secret keys (sk_*) are not allowed; create a read-only restricted key (rk_live_* / rk_test_*)."
                .to_string(),
        ));
    }
    let Some(caps) = stripe_restricted_key_regex().captures(key) else {
        return Err(OpenBotRuntimeError::Validation(
            "error.crm.invalidKey: Stripe API key must be a restricted key matching rk_live_* or rk_test_*."
                .to_string(),
        ));
    };
    Ok(&caps[1] == "test")
}

fn stripe_key_hint(key: &str) -> String {
    let chars: Vec<char> = key.chars().collect();
    if chars.len() <= 4 {
        key.to_string()
    } else {
        chars[chars.len() - 4..].iter().collect()
    }
}

fn private_field_name(field: CrmPrivateField) -> &'static str {
    match field {
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

fn parse_private_fields(raw: &[String]) -> RuntimeResult<Vec<CrmPrivateField>> {
    let mut out = Vec::new();
    let mut seen = HashSet::new();
    for entry in raw {
        let trimmed = entry.trim();
        let Some(field) = CRM_PRIVATE_FIELDS
            .into_iter()
            .find(|f| private_field_name(*f) == trimmed)
        else {
            return Err(OpenBotRuntimeError::Validation(format!(
                "Unknown CRM private field '{trimmed}'; expected one of the 17 CRM_PRIVATE_FIELDS."
            )));
        };
        if seen.insert(field) {
            out.push(field);
        }
    }
    Ok(out)
}

fn validate_currency(currency: &str) -> RuntimeResult<()> {
    if !is_crm_currency(currency) {
        return Err(OpenBotRuntimeError::Validation(format!(
            "Invalid ISO-4217 three-letter currency code '{currency}'; currency must be 3 uppercase ASCII letters (^[A-Z]{{3}}$)."
        )));
    }
    Ok(())
}

fn validate_amount(amount_minor: i64) -> RuntimeResult<()> {
    if amount_minor < 0 {
        return Err(OpenBotRuntimeError::Validation(format!(
            "Invalid amountMinor ({amount_minor}): negative amount is out of range (bounds 0..={}).",
            CRM_LIMITS.amount_minor
        )));
    }
    if !is_crm_amount(amount_minor) {
        return Err(OpenBotRuntimeError::Validation(format!(
            "Invalid amountMinor ({amount_minor}): exceeds maximum bounds {} (out of range).",
            CRM_LIMITS.amount_minor
        )));
    }
    Ok(())
}

fn stripe_minor(raw: i64, currency: &str) -> i64 {
    let adjusted = if HUNDREDTHS_ONLY.contains(&currency) {
        ((raw as f64) / 100.0).round() as i64
    } else {
        raw
    };
    adjusted.clamp(0, CRM_LIMITS.amount_minor)
}

fn stripe_ref_and_external(raw_id: &str) -> (String, String) {
    if let Some(ext) = raw_id.strip_prefix("stripe:") {
        (raw_id.to_string(), ext.to_string())
    } else {
        (format!("stripe:{raw_id}"), raw_id.to_string())
    }
}

fn count_unique_batch_customers(batch: &SourceBatch) -> usize {
    let mut refs = HashSet::new();
    for c in &batch.customers {
        refs.insert(c.source_ref.as_str());
    }
    for p in &batch.plans {
        refs.insert(p.customer.source_ref.as_str());
    }
    for pay in &batch.payments {
        refs.insert(pay.customer.source_ref.as_str());
    }
    refs.len()
}

fn field_str<'a>(obj: &'a Map<String, Value>, keys: &[&str]) -> Option<&'a str> {
    for k in keys {
        if let Some(s) = obj.get(*k).and_then(Value::as_str) {
            return Some(s);
        }
    }
    None
}

fn field_i64(obj: &Map<String, Value>, keys: &[&str]) -> Option<i64> {
    for k in keys {
        if let Some(v) = obj.get(*k) {
            if let Some(n) = v.as_i64() {
                return Some(n);
            }
            if let Some(f) = v.as_f64()
                && f.is_finite()
            {
                return Some(f.round() as i64);
            }
        }
    }
    None
}

fn epoch_secs_to_date(secs: i64) -> String {
    local_crm_date_from_ms(secs.saturating_mul(1000))
}

fn epoch_secs_to_iso(secs: i64) -> String {
    Utc.timestamp_opt(secs, 0)
        .single()
        .unwrap_or_else(Utc::now)
        .to_rfc3339_opts(SecondsFormat::Millis, true)
}

fn parse_every_value(value: &str) -> Option<CrmEvery> {
    match value.trim().to_ascii_lowercase().as_str() {
        "month" | "monthly" => Some(CrmEvery::Month),
        "quarter" | "quarterly" => Some(CrmEvery::Quarter),
        "year" | "yearly" | "annual" => Some(CrmEvery::Year),
        "once" => Some(CrmEvery::Once),
        _ => None,
    }
}

fn parse_stripe_interval(interval: &str, count: i64) -> Option<CrmEvery> {
    let months = match interval.trim().to_ascii_lowercase().as_str() {
        "year" => count.saturating_mul(12),
        "month" => count,
        _ => 0,
    };
    match months {
        1 => Some(CrmEvery::Month),
        3 => Some(CrmEvery::Quarter),
        12 => Some(CrmEvery::Year),
        _ => None,
    }
}

fn parse_source_customer(obj: &Map<String, Value>, fallback_currency: &str) -> Option<SourceCustomer> {
    if obj.get("deleted").and_then(Value::as_bool) == Some(true) {
        return None;
    }
    let raw_id = field_str(obj, &["id", "customerId", "customer_id"])?.trim();
    if raw_id.is_empty() {
        return None;
    }
    let (source_ref, external_id) = stripe_ref_and_external(raw_id);
    let currency = field_str(obj, &["currency"])
        .map(|s| s.trim().to_ascii_uppercase())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| fallback_currency.to_string());
    Some(SourceCustomer {
        source_ref,
        external_id,
        name: field_str(obj, &["name"]).unwrap_or("").to_string(),
        email: field_str(obj, &["email", "contactEmail", "contact_email"])
            .unwrap_or("")
            .to_string(),
        phone: field_str(obj, &["phone"]).unwrap_or("").to_string(),
        currency,
    })
}

fn resolve_fixture_customer(
    val: Option<&Value>,
    by_ref: &HashMap<String, SourceCustomer>,
    fallback_currency: &str,
) -> Option<SourceCustomer> {
    match val? {
        Value::String(raw) => {
            let trimmed = raw.trim();
            if trimmed.is_empty() {
                return None;
            }
            let (source_ref, external_id) = stripe_ref_and_external(trimmed);
            if let Some(existing) = by_ref.get(&source_ref).or_else(|| by_ref.get(&external_id)) {
                return Some(existing.clone());
            }
            Some(SourceCustomer {
                source_ref,
                external_id,
                name: String::new(),
                email: String::new(),
                phone: String::new(),
                currency: fallback_currency.to_string(),
            })
        }
        Value::Object(map) => parse_source_customer(map, fallback_currency),
        _ => None,
    }
}

fn parse_stripe_fixture(fixture: &Value, today: &str, now_iso: &str) -> RuntimeResult<SourceBatch> {
    let Some(root) = fixture.as_object() else {
        return Err(OpenBotRuntimeError::Validation(
            "Stripe CRM fixture must be a JSON object.".to_string(),
        ));
    };
    let default_currency = field_str(root, &["default_currency", "defaultCurrency"])
        .map(|s| s.trim().to_ascii_uppercase())
        .filter(|s| is_crm_currency(s))
        .unwrap_or_else(|| "USD".to_string());

    let mut batch = SourceBatch::default();
    let mut by_ref: HashMap<String, SourceCustomer> = HashMap::new();

    if let Some(arr) = root.get("customers").and_then(Value::as_array) {
        for item in arr {
            let Some(obj) = item.as_object() else {
                continue;
            };
            if let Some(cust) = parse_source_customer(obj, &default_currency) {
                by_ref.insert(cust.source_ref.clone(), cust.clone());
                by_ref.insert(cust.external_id.clone(), cust.clone());
                batch.customers.push(cust);
            }
        }
    }

    let plan_arrays = [
        root.get("subscriptions").and_then(Value::as_array),
        root.get("plans").and_then(Value::as_array),
    ];
    for arr in plan_arrays.into_iter().flatten() {
        for item in arr {
            let Some(obj) = item.as_object() else {
                continue;
            };
            let status = field_str(obj, &["status"]).unwrap_or("");
            if matches!(status, "incomplete" | "incomplete_expired") {
                continue;
            }
            let Some(raw_id) = field_str(obj, &["id"]).map(str::trim).filter(|s| !s.is_empty()) else {
                continue;
            };
            let (source_ref, ext_id) = stripe_ref_and_external(raw_id);
            let currency = field_str(obj, &["currency"])
                .map(|s| s.trim().to_ascii_uppercase())
                .filter(|s| !s.is_empty())
                .unwrap_or_else(|| default_currency.clone());
            let cust_val = obj
                .get("customer")
                .or_else(|| obj.get("customerId"))
                .or_else(|| obj.get("customer_id"));
            let Some(customer) = resolve_fixture_customer(cust_val, &by_ref, &currency) else {
                continue;
            };

            let (name, amount_minor, every) = parse_plan_pricing(obj, &ext_id, &currency);
            let starts_on = field_str(obj, &["startsOn", "starts_on"])
                .map(str::to_string)
                .or_else(|| field_i64(obj, &["start_date", "startDate"]).map(epoch_secs_to_date))
                .unwrap_or_else(|| today.to_string());
            let ends_on = field_str(obj, &["endsOn", "ends_on"])
                .map(str::to_string)
                .or_else(|| {
                    field_i64(obj, &["ended_at", "endedAt", "cancel_at", "cancelAt", "canceled_at", "canceledAt"])
                        .map(epoch_secs_to_date)
                })
                .map(|d| if d < starts_on { starts_on.clone() } else { d });

            batch.plans.push(SourcePlan {
                source_ref,
                customer,
                name,
                amount_minor,
                currency,
                every,
                starts_on,
                ends_on,
            });
        }
    }

    let payment_arrays = [
        root.get("invoices").and_then(Value::as_array),
        root.get("payments").and_then(Value::as_array),
    ];
    for arr in payment_arrays.into_iter().flatten() {
        for item in arr {
            let Some(obj) = item.as_object() else {
                continue;
            };
            let status = field_str(obj, &["status"]).unwrap_or("");
            if !status.is_empty()
                && !matches!(status, "open" | "paid" | "uncollectible" | "void")
            {
                continue;
            }
            let Some(raw_id) = field_str(obj, &["id"]).map(str::trim).filter(|s| !s.is_empty()) else {
                continue;
            };
            let (source_ref, ext_id) = stripe_ref_and_external(raw_id);
            let currency = field_str(obj, &["currency"])
                .map(|s| s.trim().to_ascii_uppercase())
                .filter(|s| !s.is_empty())
                .unwrap_or_else(|| default_currency.clone());
            let removed = status == "void" || obj.get("removed").and_then(Value::as_bool) == Some(true);
            let paid = status == "paid"
                || obj.get("paid").and_then(Value::as_bool) == Some(true)
                || field_str(obj, &["paidAt", "paid_at"]).is_some()
                || field_i64(obj, &["paid_at", "paidAt"]).is_some();

            let raw_amount = if let Some(am) = field_i64(obj, &["amountMinor", "amount_minor"]) {
                am.clamp(0, CRM_LIMITS.amount_minor)
            } else {
                let due = field_i64(obj, &["amount_due", "amountDue"]).unwrap_or(0);
                let paid_amt = field_i64(obj, &["amount_paid", "amountPaid"]).unwrap_or(0);
                if due == 0 && paid_amt == 0 {
                    continue;
                }
                stripe_minor(if paid { paid_amt } else { due }, &currency)
            };

            let cust_val = obj
                .get("customer")
                .or_else(|| obj.get("customerId"))
                .or_else(|| obj.get("customer_id"));
            let Some(customer) = resolve_fixture_customer(cust_val, &by_ref, &currency) else {
                continue;
            };

            let plan_ref = extract_invoice_plan_ref(obj);
            let label = field_str(obj, &["label", "number"])
                .filter(|s| !s.trim().is_empty())
                .unwrap_or(&ext_id)
                .to_string();
            let due_on = field_str(obj, &["dueOn", "due_on"])
                .map(str::to_string)
                .or_else(|| field_i64(obj, &["due_date", "dueDate", "created"]).map(epoch_secs_to_date))
                .unwrap_or_else(|| today.to_string());
            let paid_at = if paid {
                field_str(obj, &["paidAt", "paid_at"])
                    .map(str::to_string)
                    .or_else(|| {
                        obj.get("status_transitions")
                            .and_then(Value::as_object)
                            .and_then(|st| field_i64(st, &["paid_at", "paidAt"]))
                            .or_else(|| field_i64(obj, &["paid_at", "paidAt", "created"]))
                            .map(epoch_secs_to_iso)
                    })
                    .or_else(|| Some(now_iso.to_string()))
            } else {
                None
            };

            batch.payments.push(SourcePayment {
                source_ref,
                customer,
                plan_ref,
                label,
                amount_minor: raw_amount,
                currency,
                due_on,
                paid_at,
                removed,
            });
        }
    }

    Ok(batch)
}

fn parse_plan_pricing(
    obj: &Map<String, Value>,
    fallback_name: &str,
    currency: &str,
) -> (String, i64, Option<CrmEvery>) {
    if let Some(items_data) = obj
        .get("items")
        .and_then(|v| v.get("data"))
        .and_then(Value::as_array)
    {
        let mut names: Vec<String> = Vec::new();
        let mut total_raw: i64 = 0;
        let mut recurring_every: Option<CrmEvery> = None;
        for (idx, item) in items_data.iter().enumerate() {
            let Some(item_obj) = item.as_object() else {
                continue;
            };
            let Some(price) = item_obj.get("price").and_then(Value::as_object) else {
                continue;
            };
            let recurring = price.get("recurring").and_then(Value::as_object);
            if idx == 0
                && let Some(rec) = recurring
                && let Some(interval) = field_str(rec, &["interval"])
            {
                let count = field_i64(rec, &["interval_count", "intervalCount"]).unwrap_or(1);
                recurring_every = parse_stripe_interval(interval, count);
            }
            let usage_type = recurring.and_then(|r| field_str(r, &["usage_type", "usageType"]));
            if usage_type != Some("metered") {
                let unit = field_i64(price, &["unit_amount", "unitAmount"]).unwrap_or(0);
                let qty = field_i64(item_obj, &["quantity"]).unwrap_or(1);
                total_raw = total_raw.saturating_add(unit.saturating_mul(qty));
            }
            let nick = field_str(price, &["nickname"]).unwrap_or("").trim();
            let prod_name = price
                .get("product")
                .and_then(Value::as_object)
                .and_then(|p| field_str(p, &["name"]))
                .unwrap_or("")
                .trim();
            let chosen = if !nick.is_empty() { nick } else { prod_name };
            if !chosen.is_empty() && !names.iter().any(|n| n == chosen) {
                names.push(chosen.to_string());
            }
        }
        let name = if names.is_empty() {
            field_str(obj, &["name"])
                .filter(|s| !s.trim().is_empty())
                .unwrap_or(fallback_name)
                .to_string()
        } else {
            names.join(" + ")
        };
        return (name, stripe_minor(total_raw, currency), recurring_every);
    }

    let name = field_str(obj, &["name", "nickname"])
        .filter(|s| !s.trim().is_empty())
        .unwrap_or(fallback_name)
        .to_string();
    let amount_minor = if let Some(am) = field_i64(obj, &["amountMinor", "amount_minor"]) {
        am.clamp(0, CRM_LIMITS.amount_minor)
    } else if let Some(unit) = field_i64(obj, &["unit_amount", "unitAmount"]) {
        stripe_minor(unit, currency)
    } else {
        0
    };
    let every = if let Some(ev) = field_str(obj, &["every"]) {
        parse_every_value(ev)
    } else if let Some(interval) = field_str(obj, &["interval"]) {
        let count = field_i64(obj, &["interval_count", "intervalCount"]).unwrap_or(1);
        parse_stripe_interval(interval, count)
    } else {
        Some(CrmEvery::Month)
    };
    (name, amount_minor, every)
}

fn extract_invoice_plan_ref(obj: &Map<String, Value>) -> Option<String> {
    if let Some(pr) = field_str(obj, &["planRef", "plan_ref", "subscriptionId", "subscription_id"])
        .map(str::trim)
        .filter(|s| !s.is_empty())
    {
        return Some(stripe_ref_and_external(pr).0);
    }
    let sub_val = obj
        .get("parent")
        .and_then(|p| p.get("subscription_details"))
        .and_then(|sd| sd.get("subscription"))
        .or_else(|| obj.get("subscription"))?;
    match sub_val {
        Value::String(s) if !s.trim().is_empty() => Some(stripe_ref_and_external(s.trim()).0),
        Value::Object(m) => field_str(m, &["id"])
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(|s| stripe_ref_and_external(s).0),
        _ => None,
    }
}

fn build_proposed_change(
    operation: &str,
    payload: &Value,
) -> RuntimeResult<(CrmProposedChange, Option<String>, Option<u64>)> {
    let obj = payload.as_object().ok_or_else(|| {
        OpenBotRuntimeError::Validation("CRM proposal payload must be a JSON object.".to_string())
    })?;
    let op = operation.trim().to_ascii_lowercase().replace('_', "-");
    match op.as_str() {
        "customer-create" => {
            let src = obj
                .get("customer")
                .and_then(Value::as_object)
                .unwrap_or(obj);
            let customer = parse_customer_input(src)?;
            Ok((CrmProposedChange::CustomerCreate { customer }, None, None))
        }
        "customer-update" => {
            let customer_id = req_str(obj, &["customerId", "customer_id"], "customerId")?;
            let base_rev = obj
                .get("baseRevision")
                .or_else(|| obj.get("base_revision"))
                .and_then(Value::as_u64);
            let patch_src = obj.get("patch").and_then(Value::as_object).unwrap_or(obj);
            let patch = parse_customer_patch(patch_src)?;
            Ok((
                CrmProposedChange::CustomerUpdate {
                    customer_id: customer_id.clone(),
                    patch,
                },
                Some(customer_id),
                base_rev,
            ))
        }
        "customer-tags" => {
            let customer_id = req_str(obj, &["customerId", "customer_id"], "customerId")?;
            let tag_ids = parse_string_array(obj, &["tagIds", "tag_ids", "tags"])?;
            Ok((
                CrmProposedChange::CustomerTags {
                    customer_id: customer_id.clone(),
                    tag_ids,
                },
                Some(customer_id),
                None,
            ))
        }
        "plan-create" => {
            let src = obj.get("plan").and_then(Value::as_object).unwrap_or(obj);
            let customer_id = req_str(src, &["customerId", "customer_id"], "customerId")?;
            let name = req_str(src, &["name"], "name")?;
            let amount_minor = req_i64(src, &["amountMinor", "amount_minor"], "amountMinor")?;
            let every_s = field_str(src, &["every"]).unwrap_or("month");
            let every = parse_every_value(every_s).ok_or_else(|| {
                OpenBotRuntimeError::Validation(format!(
                    "Invalid plan interval '{every_s}'; expected month, quarter, year, or once."
                ))
            })?;
            let starts_on = req_str(src, &["startsOn", "starts_on"], "startsOn")?;
            let plan = CrmPlanInput {
                customer_id: customer_id.clone(),
                name,
                amount_minor,
                every,
                starts_on,
            };
            Ok((
                CrmProposedChange::PlanCreate { plan },
                Some(customer_id),
                None,
            ))
        }
        "plan-update" => {
            let plan_id = req_str(obj, &["planId", "plan_id"], "planId")?;
            let patch_src = obj.get("patch").and_then(Value::as_object).unwrap_or(obj);
            let every = match field_str(patch_src, &["every"]) {
                Some(s) => Some(parse_every_value(s).ok_or_else(|| {
                    OpenBotRuntimeError::Validation(format!("Invalid plan interval '{s}'."))
                })?),
                None => None,
            };
            let patch = CrmPlanPatch {
                name: field_str(patch_src, &["name"]).map(str::to_string),
                amount_minor: field_i64(patch_src, &["amountMinor", "amount_minor"]),
                every,
            };
            Ok((
                CrmProposedChange::PlanUpdate { plan_id, patch },
                field_str(obj, &["customerId", "customer_id"]).map(str::to_string),
                None,
            ))
        }
        "plan-end" => {
            let plan_id = req_str(obj, &["planId", "plan_id"], "planId")?;
            let ends_on = req_str(obj, &["endsOn", "ends_on"], "endsOn")?;
            Ok((
                CrmProposedChange::PlanEnd { plan_id, ends_on },
                field_str(obj, &["customerId", "customer_id"]).map(str::to_string),
                None,
            ))
        }
        "payment-create" => {
            let src = obj.get("payment").and_then(Value::as_object).unwrap_or(obj);
            let customer_id = req_str(src, &["customerId", "customer_id"], "customerId")?;
            let label = req_str(src, &["label"], "label")?;
            let amount_minor = req_i64(src, &["amountMinor", "amount_minor"], "amountMinor")?;
            let due_on = req_str(src, &["dueOn", "due_on"], "dueOn")?;
            let method = parse_opt_method(src)?;
            let paid_at = parse_opt_nullable_str(src, &["paidAt", "paid_at"])?;
            let payment = CrmPaymentInput {
                customer_id: customer_id.clone(),
                label,
                amount_minor,
                due_on,
                method,
                paid_at,
            };
            Ok((
                CrmProposedChange::PaymentCreate { payment },
                Some(customer_id),
                None,
            ))
        }
        "payment-update" => {
            let payment_id = req_str(obj, &["paymentId", "payment_id"], "paymentId")?;
            let patch_src = obj.get("patch").and_then(Value::as_object).unwrap_or(obj);
            let patch = CrmPaymentPatch {
                label: field_str(patch_src, &["label"]).map(str::to_string),
                amount_minor: field_i64(patch_src, &["amountMinor", "amount_minor"]),
                due_on: field_str(patch_src, &["dueOn", "due_on"]).map(str::to_string),
                method: parse_opt_method(patch_src)?,
            };
            Ok((
                CrmProposedChange::PaymentUpdate { payment_id, patch },
                field_str(obj, &["customerId", "customer_id"]).map(str::to_string),
                None,
            ))
        }
        "payment-paid" => {
            let payment_ids = if let Some(single) = field_str(obj, &["paymentId", "payment_id"]) {
                vec![single.to_string()]
            } else {
                parse_string_array(obj, &["paymentIds", "payment_ids"])?
            };
            let paid_at = field_str(obj, &["paidAt", "paid_at"]).map(str::to_string);
            Ok((
                CrmProposedChange::PaymentPaid {
                    payment_ids,
                    paid_at,
                },
                field_str(obj, &["customerId", "customer_id"]).map(str::to_string),
                None,
            ))
        }
        "payment-split" => {
            let payment_id = req_str(obj, &["paymentId", "payment_id"], "paymentId")?;
            let raw_parts = obj
                .get("parts")
                .and_then(Value::as_array)
                .ok_or_else(|| {
                    OpenBotRuntimeError::Validation(
                        "payment-split requires a 'parts' array.".to_string(),
                    )
                })?;
            let mut parts = Vec::with_capacity(raw_parts.len());
            for raw in raw_parts {
                let p_obj = raw.as_object().ok_or_else(|| {
                    OpenBotRuntimeError::Validation(
                        "Each payment-split part must be an object.".to_string(),
                    )
                })?;
                let amount_minor = req_i64(p_obj, &["amountMinor", "amount_minor"], "amountMinor")?;
                let due_on = req_str(p_obj, &["dueOn", "due_on"], "dueOn")?;
                parts.push(CrmSplitPart {
                    amount_minor,
                    due_on,
                });
            }
            Ok((
                CrmProposedChange::PaymentSplit { payment_id, parts },
                field_str(obj, &["customerId", "customer_id"]).map(str::to_string),
                None,
            ))
        }
        other => Err(OpenBotRuntimeError::Validation(format!(
            "Unsupported CRM proposal operation '{other}'; expected one of the 10 CRM proposed change kinds."
        ))),
    }
}

fn parse_customer_input(obj: &Map<String, Value>) -> RuntimeResult<CrmCustomerInput> {
    let name = req_str(obj, &["name"], "name")?;
    let currency = req_str(obj, &["currency"], "currency")?;
    let tag_ids = if obj.contains_key("tagIds") || obj.contains_key("tag_ids") {
        Some(parse_string_array(obj, &["tagIds", "tag_ids"])?)
    } else {
        None
    };
    Ok(CrmCustomerInput {
        name,
        contact_name: field_str(obj, &["contactName", "contact_name"]).map(str::to_string),
        contact_email: field_str(obj, &["contactEmail", "contact_email", "email"])
            .map(str::to_string),
        phone: field_str(obj, &["phone"]).map(str::to_string),
        billing_email: field_str(obj, &["billingEmail", "billing_email"]).map(str::to_string),
        domain: field_str(obj, &["domain"]).map(str::to_string),
        tax_id: field_str(obj, &["taxId", "tax_id"]).map(str::to_string),
        address: field_str(obj, &["address"]).map(str::to_string),
        stripe_customer_id: field_str(obj, &["stripeCustomerId", "stripe_customer_id"])
            .map(str::to_string),
        notes: field_str(obj, &["notes"]).map(str::to_string),
        status_id: field_str(obj, &["statusId", "status_id", "status"]).map(str::to_string),
        owner_agent_id: parse_opt_nullable_str(obj, &["ownerAgentId", "owner_agent_id"])?,
        method: parse_opt_method(obj)?,
        currency,
        tag_ids,
    })
}

fn parse_customer_patch(obj: &Map<String, Value>) -> RuntimeResult<CrmCustomerPatch> {
    Ok(CrmCustomerPatch {
        name: field_str(obj, &["name"]).map(str::to_string),
        contact_name: field_str(obj, &["contactName", "contact_name"]).map(str::to_string),
        contact_email: field_str(obj, &["contactEmail", "contact_email"]).map(str::to_string),
        phone: field_str(obj, &["phone"]).map(str::to_string),
        billing_email: field_str(obj, &["billingEmail", "billing_email"]).map(str::to_string),
        domain: field_str(obj, &["domain"]).map(str::to_string),
        tax_id: field_str(obj, &["taxId", "tax_id"]).map(str::to_string),
        address: field_str(obj, &["address"]).map(str::to_string),
        stripe_customer_id: field_str(obj, &["stripeCustomerId", "stripe_customer_id"])
            .map(str::to_string),
        notes: field_str(obj, &["notes"]).map(str::to_string),
        status_id: field_str(obj, &["statusId", "status_id", "status"]).map(str::to_string),
        owner_agent_id: parse_opt_nullable_str(obj, &["ownerAgentId", "owner_agent_id"])?,
        method: parse_opt_method(obj)?,
        currency: field_str(obj, &["currency"]).map(str::to_string),
    })
}

fn parse_opt_method(obj: &Map<String, Value>) -> RuntimeResult<Option<Option<CrmMethod>>> {
    let Some(val) = obj.get("method") else {
        return Ok(None);
    };
    if val.is_null() {
        return Ok(Some(None));
    }
    let Some(s) = val.as_str() else {
        return Err(OpenBotRuntimeError::Validation(
            "CRM payment method must be a string or null.".to_string(),
        ));
    };
    let parsed = match s.trim().to_ascii_lowercase().as_str() {
        "stripe" => CrmMethod::Stripe,
        "transfer" => CrmMethod::Transfer,
        "crypto" => CrmMethod::Crypto,
        "paypal" => CrmMethod::Paypal,
        "cash" => CrmMethod::Cash,
        other => {
            return Err(OpenBotRuntimeError::Validation(format!(
                "Unknown CRM payment method '{other}'."
            )));
        }
    };
    Ok(Some(Some(parsed)))
}

fn parse_opt_nullable_str(
    obj: &Map<String, Value>,
    keys: &[&str],
) -> RuntimeResult<Option<Option<String>>> {
    for k in keys {
        if let Some(val) = obj.get(*k) {
            if val.is_null() {
                return Ok(Some(None));
            }
            let Some(s) = val.as_str() else {
                return Err(OpenBotRuntimeError::Validation(format!(
                    "Field '{k}' must be a string or null."
                )));
            };
            return Ok(Some(Some(s.to_string())));
        }
    }
    Ok(None)
}

fn parse_string_array(obj: &Map<String, Value>, keys: &[&str]) -> RuntimeResult<Vec<String>> {
    for k in keys {
        if let Some(val) = obj.get(*k) {
            let Some(arr) = val.as_array() else {
                return Err(OpenBotRuntimeError::Validation(format!(
                    "Field '{k}' must be an array of strings."
                )));
            };
            let mut out = Vec::with_capacity(arr.len());
            for item in arr {
                let Some(s) = item.as_str() else {
                    return Err(OpenBotRuntimeError::Validation(format!(
                        "Field '{k}' must contain only strings."
                    )));
                };
                out.push(s.to_string());
            }
            return Ok(out);
        }
    }
    Err(OpenBotRuntimeError::Validation(format!(
        "Missing required array field '{}'.",
        keys[0]
    )))
}

fn req_str(obj: &Map<String, Value>, keys: &[&str], label: &str) -> RuntimeResult<String> {
    field_str(obj, keys)
        .map(str::to_string)
        .ok_or_else(|| OpenBotRuntimeError::Validation(format!("Missing required field '{label}'.")))
}

fn req_i64(obj: &Map<String, Value>, keys: &[&str], label: &str) -> RuntimeResult<i64> {
    field_i64(obj, keys)
        .ok_or_else(|| OpenBotRuntimeError::Validation(format!("Missing required integer field '{label}'.")))
}
