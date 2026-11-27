//! Decision/Review/Merge gates (`WAIT_DEFAULT_EXPIRY_DAYS` / `WAIT_MAX_DEADLINE_DAYS`,
//! `WaitTimeout`) and cross-machine federation (host selection, federated dispatches,
//! relay queue, remote attachments/questions, peer mail, and peer runs).

use std::collections::BTreeSet;

use orca_core::orchestration_protocol::{
    ExecutionHostRecord, GateStatus, HostSelectionCriteria, MergeGate, OrchestrationTaskStatus,
    ReviewGate, evaluate_merge_gate, evaluate_wait_deadline, select_execution_host,
};
use rusqlite::{OptionalExtension, params};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use super::schema::{gate_status_to_str, str_to_gate_status};
use super::{OrchestrationEngine, OrchestrationWakeEvent};
use crate::{
    OrcaIdeRuntimeError, Result, WAIT_DEFAULT_EXPIRY_DAYS, WAIT_MAX_DEADLINE_DAYS, now_epoch_ms,
};

const MS_PER_DAY: i64 = 86_400_000;

/// Kind of gate persisted in `decision_gates`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DecisionGateKind {
    Review,
    Merge,
    Decision,
}

impl DecisionGateKind {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Review => "review",
            Self::Merge => "merge",
            Self::Decision => "decision",
        }
    }

    pub fn parse(raw: &str) -> Result<Self> {
        match raw {
            "review" => Ok(Self::Review),
            "merge" => Ok(Self::Merge),
            "decision" => Ok(Self::Decision),
            other => Err(OrcaIdeRuntimeError::invalid_argument(format!(
                "unknown decision gate kind '{other}'"
            ))),
        }
    }
}

/// Typed gate payload stored as JSON inside `decision_gates.payload_json`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum DecisionGatePayload {
    Review(ReviewGate),
    Merge(MergeGate),
    Decision {
        prompt: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        resolution_note: Option<String>,
    },
}

/// Request to create a review, merge, or decision gate on a task.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CreateGateRequest {
    pub run_id: String,
    pub task_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub gate_id: Option<String>,
    pub payload: DecisionGatePayload,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub deadline_ms: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub now_ms: Option<i64>,
}

/// Action applied when resolving or updating a gate.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "action", rename_all = "snake_case")]
pub enum ResolveGateAction {
    Approve {
        actor: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        note: Option<String>,
    },
    Reject {
        actor: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        reason: Option<String>,
    },
    UpdateMergeSignals {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        ci_passed: Option<bool>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        conflicts_resolved: Option<bool>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        human_approved: Option<bool>,
    },
    EvaluateOnly,
}

/// Persisted row in `decision_gates`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DecisionGateRecord {
    pub gate_id: String,
    pub run_id: String,
    pub task_id: String,
    pub gate_kind: DecisionGateKind,
    pub payload: DecisionGatePayload,
    pub status: GateStatus,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub deadline_ms: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resolved_at_ms: Option<i64>,
}

/// Persisted cross-machine dispatch (`federated_dispatches`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FederatedDispatchRecord {
    pub dispatch_id: String,
    pub run_id: String,
    pub task_id: String,
    pub home_host_id: String,
    pub worker_host_id: String,
    pub depth: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub home_auto_release_at_ms: Option<i64>,
    pub status: String,
    pub created_at_ms: i64,
    pub updated_at_ms: i64,
}

/// Request to dispatch work to a remote execution host using fail-closed `select_execution_host`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DispatchFederatedRequest {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dispatch_id: Option<String>,
    pub run_id: String,
    pub task_id: String,
    pub home_host_id: String,
    pub selection: HostSelectionCriteria,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub depth: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub home_auto_release_at_ms: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub now_ms: Option<i64>,
}

/// Attachment record on a federated dispatch (`remote_dispatch_attachments`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RemoteDispatchAttachment {
    pub attachment_id: String,
    pub dispatch_id: String,
    pub kind: String,
    pub uri: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sha256: Option<String>,
    pub created_at_ms: i64,
}

/// Direction of an item in `federation_relay_items`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FederationRelayDirection {
    ToHome,
    ToWorker,
}

impl FederationRelayDirection {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::ToHome => "to_home",
            Self::ToWorker => "to_worker",
        }
    }

    pub fn parse(raw: &str) -> Result<Self> {
        match raw {
            "to_home" => Ok(Self::ToHome),
            "to_worker" => Ok(Self::ToWorker),
            other => Err(OrcaIdeRuntimeError::invalid_argument(format!(
                "unknown relay direction '{other}'"
            ))),
        }
    }
}

/// Persisted item in `federation_relay_items`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FederationRelayItem {
    pub seq: i64,
    pub direction: FederationRelayDirection,
    pub dispatch_id: String,
    pub payload_json: serde_json::Value,
    pub acked: bool,
    pub created_at_ms: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub acked_at_ms: Option<i64>,
}

/// Persisted remote question (`remote_questions`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RemoteQuestionRecord {
    pub question_id: String,
    pub dispatch_id: String,
    pub home_host_id: String,
    pub worker_host_id: String,
    pub prompt: String,
    pub status: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub answer: Option<String>,
    pub created_at_ms: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub answered_at_ms: Option<i64>,
}

/// Peer mail conversation metadata (`peer_mail_conversations`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PeerMailConversation {
    pub conversation_id: String,
    pub local_host_id: String,
    pub remote_host_id: String,
    pub subject: String,
    pub created_at_ms: i64,
    pub updated_at_ms: i64,
}

/// Peer mail message (`peer_mail_messages`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PeerMailMessage {
    pub message_id: String,
    pub conversation_id: String,
    pub from_host_id: String,
    pub to_host_id: String,
    pub body: String,
    pub created_at_ms: i64,
}

/// Request to send cross-machine peer mail.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SendPeerMailRequest {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub conversation_id: Option<String>,
    pub local_host_id: String,
    pub remote_host_id: String,
    pub subject: String,
    pub from_host_id: String,
    pub to_host_id: String,
    pub body: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub now_ms: Option<i64>,
}

/// Cross-machine peer run mirror (`peer_runs`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PeerRunRecord {
    pub peer_run_id: String,
    pub home_host_id: String,
    pub remote_host_id: String,
    pub remote_run_id: String,
    pub status: String,
    pub updated_at_ms: i64,
}

impl OrchestrationEngine {
    fn validate_and_resolve_deadline(deadline_ms: Option<i64>, now_ms: i64) -> Result<i64> {
        let default_expiry = now_ms.saturating_add(
            i64::try_from(WAIT_DEFAULT_EXPIRY_DAYS)
                .unwrap_or(30)
                .saturating_mul(MS_PER_DAY),
        );
        let max_deadline = now_ms.saturating_add(
            i64::try_from(WAIT_MAX_DEADLINE_DAYS)
                .unwrap_or(365)
                .saturating_mul(MS_PER_DAY),
        );
        let resolved = deadline_ms.unwrap_or(default_expiry);
        if resolved > max_deadline {
            return Err(OrcaIdeRuntimeError::invalid_argument(format!(
                "gate deadline exceeds max horizon of {WAIT_MAX_DEADLINE_DAYS} days"
            )));
        }
        Ok(resolved)
    }

    /// Creates a review, merge, or decision gate on a task, enforcing wait deadline bounds
    /// (`WAIT_DEFAULT_EXPIRY_DAYS = 30`, `WAIT_MAX_DEADLINE_DAYS = 365`).
    pub fn create_gate(&self, req: CreateGateRequest) -> Result<DecisionGateRecord> {
        let now_ms = req.now_ms.unwrap_or_else(now_epoch_ms);
        let gate_id = req
            .gate_id
            .filter(|id| !id.trim().is_empty())
            .unwrap_or_else(|| format!("gate-{}", Uuid::new_v4()));

        let requested_deadline = req.deadline_ms.or(match &req.payload {
            DecisionGatePayload::Review(r) => r.deadline_ms,
            DecisionGatePayload::Merge(m) => m.deadline_ms,
            DecisionGatePayload::Decision { .. } => None,
        });
        let deadline_ms = Self::validate_and_resolve_deadline(requested_deadline, now_ms)?;

        let mut payload = req.payload;
        let gate_kind = match &mut payload {
            DecisionGatePayload::Review(r) => {
                r.gate_id = gate_id.clone();
                r.task_id = req.task_id.clone();
                r.deadline_ms = Some(deadline_ms);
                DecisionGateKind::Review
            }
            DecisionGatePayload::Merge(m) => {
                m.gate_id = gate_id.clone();
                m.task_id = req.task_id.clone();
                m.deadline_ms = Some(deadline_ms);
                DecisionGateKind::Merge
            }
            DecisionGatePayload::Decision { .. } => DecisionGateKind::Decision,
        };

        let payload_json = serde_json::to_string(&payload)?;
        let conn = self.conn.lock();
        let _run = Self::get_run_inner(&conn, &req.run_id)?;
        let mut dag = Self::load_dag(&conn, &req.run_id)?;
        dag.transition_task(&req.task_id, OrchestrationTaskStatus::WaitingGate, now_ms)?;
        Self::persist_dag_statuses(&conn, &dag)?;

        conn.execute(
            "INSERT INTO decision_gates
             (gate_id, run_id, task_id, gate_kind, payload_json, status, deadline_ms, resolved_at_ms)
             VALUES (?1, ?2, ?3, ?4, ?5, 'pending', ?6, NULL)",
            params![
                &gate_id,
                &req.run_id,
                &req.task_id,
                gate_kind.as_str(),
                &payload_json,
                deadline_ms
            ],
        )?;
        Self::touch_coordinator_wake(&conn, &req.run_id, "gate_created", now_ms)?;
        drop(conn);

        self.emit_wake(OrchestrationWakeEvent::GateUpdated {
            run_id: req.run_id.clone(),
            gate_id: gate_id.clone(),
            task_id: req.task_id.clone(),
            status: GateStatus::Pending,
            timestamp_ms: now_ms,
        });

        Ok(DecisionGateRecord {
            gate_id,
            run_id: req.run_id,
            task_id: req.task_id,
            gate_kind,
            payload,
            status: GateStatus::Pending,
            deadline_ms: Some(deadline_ms),
            resolved_at_ms: None,
        })
    }

    /// Applies an approval/rejection/signal update and evaluates the gate via `ReviewGate::evaluate`,
    /// `evaluate_merge_gate`, and `evaluate_wait_deadline` (persisting `TimedOut` and returning
    /// `OrcaIdeErrorCode::WaitTimeout` if expired).
    pub fn resolve_gate(
        &self,
        gate_id: &str,
        action: ResolveGateAction,
        now_ms: Option<i64>,
    ) -> Result<DecisionGateRecord> {
        let now_ms = now_ms.unwrap_or_else(now_epoch_ms);
        let conn = self.conn.lock();
        let row = conn
            .query_row(
                "SELECT run_id, task_id, gate_kind, payload_json, status, deadline_ms, resolved_at_ms
                 FROM decision_gates WHERE gate_id = ?1",
                params![gate_id],
                |r| {
                    Ok((
                        r.get::<_, String>(0)?,
                        r.get::<_, String>(1)?,
                        r.get::<_, String>(2)?,
                        r.get::<_, String>(3)?,
                        r.get::<_, String>(4)?,
                        r.get::<_, Option<i64>>(5)?,
                        r.get::<_, Option<i64>>(6)?,
                    ))
                },
            )
            .optional()?
            .ok_or_else(|| OrcaIdeRuntimeError::not_found(format!("unknown gate '{gate_id}'")))?;

        let (run_id, task_id, kind_str, payload_str, status_str, deadline_ms, prev_resolved) = row;
        let gate_kind = DecisionGateKind::parse(&kind_str)?;
        let mut status = str_to_gate_status(&status_str)?;
        let mut payload: DecisionGatePayload = serde_json::from_str(&payload_str)?;

        match (&mut payload, &action) {
            (DecisionGatePayload::Review(review), ResolveGateAction::Approve { actor, .. }) => {
                if !review.approvals.iter().any(|a| a == actor) {
                    review.approvals.push(actor.clone());
                }
            }
            (DecisionGatePayload::Review(review), ResolveGateAction::Reject { actor, .. }) => {
                if !review.rejections.iter().any(|a| a == actor) {
                    review.rejections.push(actor.clone());
                }
                review.status = GateStatus::Rejected;
            }
            (DecisionGatePayload::Merge(merge), ResolveGateAction::Approve { .. }) => {
                merge.human_approved = true;
            }
            (DecisionGatePayload::Merge(merge), ResolveGateAction::Reject { .. }) => {
                merge.status = GateStatus::Rejected;
            }
            (
                DecisionGatePayload::Merge(merge),
                ResolveGateAction::UpdateMergeSignals {
                    ci_passed,
                    conflicts_resolved,
                    human_approved,
                },
            ) => {
                if let Some(v) = ci_passed {
                    merge.ci_passed = *v;
                }
                if let Some(v) = conflicts_resolved {
                    merge.conflicts_resolved = *v;
                }
                if let Some(v) = human_approved {
                    merge.human_approved = *v;
                }
            }
            (
                DecisionGatePayload::Decision {
                    resolution_note, ..
                },
                ResolveGateAction::Approve { note, .. },
            ) => {
                *resolution_note = note.clone();
                status = GateStatus::Approved;
            }
            (
                DecisionGatePayload::Decision {
                    resolution_note, ..
                },
                ResolveGateAction::Reject { reason, .. },
            ) => {
                *resolution_note = reason.clone();
                status = GateStatus::Rejected;
            }
            (_, ResolveGateAction::EvaluateOnly) => {}
            _ => {
                return Err(OrcaIdeRuntimeError::invalid_argument(
                    "gate resolution action does not match gate kind",
                ));
            }
        }

        let eval_res = match &payload {
            DecisionGatePayload::Review(review) => review.evaluate(now_ms),
            DecisionGatePayload::Merge(merge) => evaluate_merge_gate(merge, now_ms),
            DecisionGatePayload::Decision { .. } => {
                if status == GateStatus::Pending {
                    evaluate_wait_deadline(deadline_ms, now_ms).map(|()| GateStatus::Pending)
                } else {
                    Ok(status)
                }
            }
        };

        match eval_res {
            Err(ide_err) => {
                // Persist TimedOut before returning WaitTimeout error.
                conn.execute(
                    "UPDATE decision_gates SET status = 'timed_out', resolved_at_ms = ?1
                     WHERE gate_id = ?2",
                    params![now_ms, gate_id],
                )?;
                Self::touch_coordinator_wake(&conn, &run_id, "gate_timed_out", now_ms)?;
                drop(conn);
                self.emit_wake(OrchestrationWakeEvent::GateUpdated {
                    run_id,
                    gate_id: gate_id.to_string(),
                    task_id,
                    status: GateStatus::TimedOut,
                    timestamp_ms: now_ms,
                });
                Err(OrcaIdeRuntimeError::Ide(ide_err))
            }
            Ok(evaluated_status) => {
                match &mut payload {
                    DecisionGatePayload::Review(r) => r.status = evaluated_status,
                    DecisionGatePayload::Merge(m) => m.status = evaluated_status,
                    DecisionGatePayload::Decision { .. } => {}
                }
                let resolved_at_ms = if evaluated_status == GateStatus::Pending {
                    prev_resolved
                } else {
                    Some(prev_resolved.unwrap_or(now_ms))
                };
                let updated_json = serde_json::to_string(&payload)?;
                conn.execute(
                    "UPDATE decision_gates
                     SET payload_json = ?1, status = ?2, resolved_at_ms = ?3
                     WHERE gate_id = ?4",
                    params![
                        &updated_json,
                        gate_status_to_str(evaluated_status),
                        resolved_at_ms,
                        gate_id
                    ],
                )?;
                Self::touch_coordinator_wake(&conn, &run_id, "gate_updated", now_ms)?;
                drop(conn);

                self.emit_wake(OrchestrationWakeEvent::GateUpdated {
                    run_id: run_id.clone(),
                    gate_id: gate_id.to_string(),
                    task_id: task_id.clone(),
                    status: evaluated_status,
                    timestamp_ms: now_ms,
                });

                Ok(DecisionGateRecord {
                    gate_id: gate_id.to_string(),
                    run_id,
                    task_id,
                    gate_kind,
                    payload,
                    status: evaluated_status,
                    deadline_ms,
                    resolved_at_ms,
                })
            }
        }
    }

    /// Upserts an execution host into `execution_hosts` for multi-host scheduling & mail capability checks.
    pub fn register_host(
        &self,
        host: ExecutionHostRecord,
        supports_mail: bool,
    ) -> Result<ExecutionHostRecord> {
        if host.host_id.trim().is_empty() {
            return Err(OrcaIdeRuntimeError::invalid_argument(
                "host_id cannot be empty",
            ));
        }
        let caps_vec: Vec<&String> = host.capabilities.iter().collect();
        let caps_json = serde_json::to_string(&caps_vec)?;
        let conn = self.conn.lock();
        conn.execute(
            "INSERT INTO execution_hosts
             (host_id, display_name, capabilities_json, online, protocol_version,
              active_workers, max_workers, last_heartbeat_ms, supports_mail)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)
             ON CONFLICT(host_id) DO UPDATE SET
               display_name = excluded.display_name,
               capabilities_json = excluded.capabilities_json,
               online = excluded.online,
               protocol_version = excluded.protocol_version,
               active_workers = excluded.active_workers,
               max_workers = excluded.max_workers,
               last_heartbeat_ms = excluded.last_heartbeat_ms,
               supports_mail = excluded.supports_mail",
            params![
                &host.host_id,
                &host.display_name,
                &caps_json,
                i32::from(host.online),
                i64::from(host.protocol_version),
                i64::from(host.active_workers),
                i64::from(host.max_workers),
                host.last_heartbeat_ms,
                i32::from(supports_mail)
            ],
        )?;
        Ok(host)
    }

    /// Lists all registered execution hosts.
    pub fn list_hosts(&self) -> Result<Vec<ExecutionHostRecord>> {
        let conn = self.conn.lock();
        Self::load_hosts(&conn)
    }

    pub(crate) fn load_hosts(conn: &rusqlite::Connection) -> Result<Vec<ExecutionHostRecord>> {
        let mut stmt = conn.prepare(
            "SELECT host_id, display_name, capabilities_json, online, protocol_version,
                    active_workers, max_workers, last_heartbeat_ms
             FROM execution_hosts ORDER BY host_id ASC",
        )?;
        let rows = stmt
            .query_map([], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, i64>(3)? != 0,
                    row.get::<_, i64>(4)?,
                    row.get::<_, i64>(5)?,
                    row.get::<_, i64>(6)?,
                    row.get::<_, i64>(7)?,
                ))
            })?
            .collect::<std::result::Result<Vec<_>, _>>()?;

        let mut hosts = Vec::with_capacity(rows.len());
        for r in rows {
            let caps_vec: Vec<String> = serde_json::from_str(&r.2)?;
            let capabilities: BTreeSet<String> = caps_vec.into_iter().collect();
            hosts.push(ExecutionHostRecord {
                host_id: r.0,
                display_name: r.1,
                capabilities,
                online: r.3,
                protocol_version: u32::try_from(r.4.max(0)).unwrap_or(0),
                active_workers: u32::try_from(r.5.max(0)).unwrap_or(0),
                max_workers: u32::try_from(r.6.max(0)).unwrap_or(0),
                last_heartbeat_ms: r.7,
            });
        }
        Ok(hosts)
    }

    /// Selects a compatible online execution host using `orca_core::select_execution_host`.
    /// Explicit `criteria.preferred_host_id` (`--on <server>`) fails closed (`NotFound`)
    /// instead of silently falling back to another host.
    pub fn select_host(
        &self,
        criteria: &HostSelectionCriteria,
        now_ms: Option<i64>,
    ) -> Result<ExecutionHostRecord> {
        let now_ms = now_ms.unwrap_or_else(now_epoch_ms);
        let conn = self.conn.lock();
        let hosts = Self::load_hosts(&conn)?;
        select_execution_host(&hosts, criteria, now_ms)
            .cloned()
            .ok_or_else(|| {
                if let Some(ref pref) = criteria.preferred_host_id {
                    OrcaIdeRuntimeError::not_found(format!(
                        "requested execution host '{pref}' is unavailable, offline, stale, or incompatible"
                    ))
                } else {
                    OrcaIdeRuntimeError::not_found(
                        "no eligible execution host matched selection criteria",
                    )
                }
            })
    }
}

impl OrchestrationEngine {
    /// Creates a cross-machine federated dispatch after fail-closed host selection.
    pub fn dispatch_federated(
        &self,
        req: DispatchFederatedRequest,
    ) -> Result<FederatedDispatchRecord> {
        let now_ms = req.now_ms.unwrap_or_else(now_epoch_ms);
        let dispatch_id = req
            .dispatch_id
            .filter(|id| !id.trim().is_empty())
            .unwrap_or_else(|| format!("fdisp-{}", Uuid::new_v4()));
        let depth = req.depth.unwrap_or(1).max(1);

        let conn = self.conn.lock();
        let hosts = Self::load_hosts(&conn)?;
        let selected = select_execution_host(&hosts, &req.selection, now_ms).ok_or_else(|| {
            if let Some(ref pref) = req.selection.preferred_host_id {
                OrcaIdeRuntimeError::not_found(format!(
                    "explicit target host '{pref}' unavailable (fail-closed)"
                ))
            } else {
                OrcaIdeRuntimeError::not_found("no eligible remote execution host found")
            }
        })?;
        let worker_host_id = selected.host_id.clone();

        conn.execute(
            "INSERT INTO federated_dispatches
             (dispatch_id, run_id, task_id, home_host_id, worker_host_id, depth,
              home_auto_release_at_ms, status, created_at_ms, updated_at_ms)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, 'dispatched', ?8, ?8)",
            params![
                &dispatch_id,
                &req.run_id,
                &req.task_id,
                &req.home_host_id,
                &worker_host_id,
                i64::from(depth),
                req.home_auto_release_at_ms,
                now_ms
            ],
        )?;
        conn.execute(
            "UPDATE execution_hosts SET active_workers = active_workers + 1 WHERE host_id = ?1",
            params![&worker_host_id],
        )?;

        Ok(FederatedDispatchRecord {
            dispatch_id,
            run_id: req.run_id,
            task_id: req.task_id,
            home_host_id: req.home_host_id,
            worker_host_id,
            depth,
            home_auto_release_at_ms: req.home_auto_release_at_ms,
            status: "dispatched".to_string(),
            created_at_ms: now_ms,
            updated_at_ms: now_ms,
        })
    }

    /// Attaches a remote artifact/bundle URI to a federated dispatch (`remote_dispatch_attachments`).
    pub fn attach_remote_dispatch(
        &self,
        dispatch_id: &str,
        kind: &str,
        uri: &str,
        sha256: Option<String>,
        now_ms: Option<i64>,
    ) -> Result<RemoteDispatchAttachment> {
        let now_ms = now_ms.unwrap_or_else(now_epoch_ms);
        let attachment_id = format!("att-{}", Uuid::new_v4());
        let conn = self.conn.lock();
        conn.execute(
            "INSERT INTO remote_dispatch_attachments
             (attachment_id, dispatch_id, kind, uri, sha256, created_at_ms)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![&attachment_id, dispatch_id, kind, uri, &sha256, now_ms],
        )?;
        Ok(RemoteDispatchAttachment {
            attachment_id,
            dispatch_id: dispatch_id.to_string(),
            kind: kind.to_string(),
            uri: uri.to_string(),
            sha256,
            created_at_ms: now_ms,
        })
    }

    /// Enqueues a durable cross-machine relay envelope (`federation_relay_items`) and wakes subscribers.
    pub fn enqueue_relay_item(
        &self,
        dispatch_id: &str,
        direction: FederationRelayDirection,
        payload_json: serde_json::Value,
        now_ms: Option<i64>,
    ) -> Result<FederationRelayItem> {
        let now_ms = now_ms.unwrap_or_else(now_epoch_ms);
        let serialized = serde_json::to_string(&payload_json)?;
        let conn = self.conn.lock();
        conn.execute(
            "INSERT INTO federation_relay_items
             (direction, dispatch_id, payload_json, acked, created_at_ms, acked_at_ms)
             VALUES (?1, ?2, ?3, 0, ?4, NULL)",
            params![direction.as_str(), dispatch_id, &serialized, now_ms],
        )?;
        let seq = conn.last_insert_rowid();
        drop(conn);

        self.emit_wake(OrchestrationWakeEvent::FederationRelayEnqueued {
            dispatch_id: dispatch_id.to_string(),
            seq,
            direction,
            timestamp_ms: now_ms,
        });

        Ok(FederationRelayItem {
            seq,
            direction,
            dispatch_id: dispatch_id.to_string(),
            payload_json,
            acked: false,
            created_at_ms: now_ms,
            acked_at_ms: None,
        })
    }

    /// Lists unacknowledged relay items for `dispatch_id` ordered by `seq ASC`.
    pub fn list_unacked_relay_items(
        &self,
        dispatch_id: &str,
        direction: Option<FederationRelayDirection>,
    ) -> Result<Vec<FederationRelayItem>> {
        let conn = self.conn.lock();
        let mut out = Vec::new();
        let mut stmt = if direction.is_some() {
            conn.prepare(
                "SELECT seq, direction, dispatch_id, payload_json, acked, created_at_ms, acked_at_ms
                 FROM federation_relay_items
                 WHERE dispatch_id = ?1 AND direction = ?2 AND acked = 0
                 ORDER BY seq ASC",
            )?
        } else {
            conn.prepare(
                "SELECT seq, direction, dispatch_id, payload_json, acked, created_at_ms, acked_at_ms
                 FROM federation_relay_items
                 WHERE dispatch_id = ?1 AND acked = 0
                 ORDER BY seq ASC",
            )?
        };
        let rows = if let Some(dir) = direction {
            stmt.query_map(params![dispatch_id, dir.as_str()], |row| {
                Ok((
                    row.get::<_, i64>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, String>(3)?,
                    row.get::<_, i64>(4)? != 0,
                    row.get::<_, i64>(5)?,
                    row.get::<_, Option<i64>>(6)?,
                ))
            })?
            .collect::<std::result::Result<Vec<_>, _>>()?
        } else {
            stmt.query_map(params![dispatch_id], |row| {
                Ok((
                    row.get::<_, i64>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, String>(3)?,
                    row.get::<_, i64>(4)? != 0,
                    row.get::<_, i64>(5)?,
                    row.get::<_, Option<i64>>(6)?,
                ))
            })?
            .collect::<std::result::Result<Vec<_>, _>>()?
        };
        for r in rows {
            out.push(FederationRelayItem {
                seq: r.0,
                direction: FederationRelayDirection::parse(&r.1)?,
                dispatch_id: r.2,
                payload_json: serde_json::from_str(&r.3)?,
                acked: r.4,
                created_at_ms: r.5,
                acked_at_ms: r.6,
            });
        }
        Ok(out)
    }

    /// Acknowledges a relay item by `seq`.
    pub fn ack_relay_item(&self, seq: i64, now_ms: Option<i64>) -> Result<()> {
        let now_ms = now_ms.unwrap_or_else(now_epoch_ms);
        let conn = self.conn.lock();
        let updated = conn.execute(
            "UPDATE federation_relay_items SET acked = 1, acked_at_ms = ?1 WHERE seq = ?2",
            params![now_ms, seq],
        )?;
        if updated == 0 {
            return Err(OrcaIdeRuntimeError::not_found(format!(
                "unknown relay item seq {seq}"
            )));
        }
        Ok(())
    }

    /// Records a cross-machine remote question (`remote_questions`).
    pub fn ask_remote_question(
        &self,
        dispatch_id: &str,
        home_host_id: &str,
        worker_host_id: &str,
        prompt: &str,
        now_ms: Option<i64>,
    ) -> Result<RemoteQuestionRecord> {
        let now_ms = now_ms.unwrap_or_else(now_epoch_ms);
        let question_id = format!("rq-{}", Uuid::new_v4());
        let conn = self.conn.lock();
        conn.execute(
            "INSERT INTO remote_questions
             (question_id, dispatch_id, home_host_id, worker_host_id, prompt, status,
              answer, created_at_ms, answered_at_ms)
             VALUES (?1, ?2, ?3, ?4, ?5, 'pending', NULL, ?6, NULL)",
            params![&question_id, dispatch_id, home_host_id, worker_host_id, prompt, now_ms],
        )?;
        Ok(RemoteQuestionRecord {
            question_id,
            dispatch_id: dispatch_id.to_string(),
            home_host_id: home_host_id.to_string(),
            worker_host_id: worker_host_id.to_string(),
            prompt: prompt.to_string(),
            status: "pending".to_string(),
            answer: None,
            created_at_ms: now_ms,
            answered_at_ms: None,
        })
    }

    /// Sends a peer-to-peer mail message between execution hosts (`peer_mail_conversations` + `peer_mail_messages`).
    pub fn send_peer_mail(
        &self,
        req: SendPeerMailRequest,
    ) -> Result<(PeerMailConversation, PeerMailMessage)> {
        if req.body.trim().is_empty() {
            return Err(OrcaIdeRuntimeError::invalid_argument(
                "peer mail body cannot be empty",
            ));
        }
        let now_ms = req.now_ms.unwrap_or_else(now_epoch_ms);
        let conversation_id = req
            .conversation_id
            .filter(|id| !id.trim().is_empty())
            .unwrap_or_else(|| format!("pconv-{}", Uuid::new_v4()));
        let message_id = format!("pmsg-{}", Uuid::new_v4());

        let conn = self.conn.lock();
        let existing_created: Option<i64> = conn
            .query_row(
                "SELECT created_at_ms FROM peer_mail_conversations WHERE conversation_id = ?1",
                params![&conversation_id],
                |row| row.get(0),
            )
            .optional()?;
        let conv_created = existing_created.unwrap_or(now_ms);

        if existing_created.is_none() {
            conn.execute(
                "INSERT INTO peer_mail_conversations
                 (conversation_id, local_host_id, remote_host_id, subject, created_at_ms, updated_at_ms)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?5)",
                params![
                    &conversation_id,
                    &req.local_host_id,
                    &req.remote_host_id,
                    &req.subject,
                    now_ms
                ],
            )?;
        } else {
            conn.execute(
                "UPDATE peer_mail_conversations SET updated_at_ms = ?1 WHERE conversation_id = ?2",
                params![now_ms, &conversation_id],
            )?;
        }

        conn.execute(
            "INSERT INTO peer_mail_messages
             (message_id, conversation_id, from_host_id, to_host_id, body, created_at_ms)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![
                &message_id,
                &conversation_id,
                &req.from_host_id,
                &req.to_host_id,
                &req.body,
                now_ms
            ],
        )?;

        let conv = PeerMailConversation {
            conversation_id: conversation_id.clone(),
            local_host_id: req.local_host_id,
            remote_host_id: req.remote_host_id,
            subject: req.subject,
            created_at_ms: conv_created,
            updated_at_ms: now_ms,
        };
        let msg = PeerMailMessage {
            message_id,
            conversation_id,
            from_host_id: req.from_host_id,
            to_host_id: req.to_host_id,
            body: req.body,
            created_at_ms: now_ms,
        };
        Ok((conv, msg))
    }

    /// Lists peer mail messages optionally filtered by `conversation_id` or `remote_host_id`.
    pub fn list_peer_mail(
        &self,
        conversation_id: Option<&str>,
        remote_host_id: Option<&str>,
    ) -> Result<Vec<PeerMailMessage>> {
        let conn = self.conn.lock();
        let rows = match (conversation_id, remote_host_id) {
            (Some(cid), _) => {
                let mut stmt = conn.prepare(
                    "SELECT message_id, conversation_id, from_host_id, to_host_id, body, created_at_ms
                     FROM peer_mail_messages
                     WHERE conversation_id = ?1
                     ORDER BY created_at_ms ASC, message_id ASC",
                )?;
                stmt.query_map(params![cid], Self::map_peer_mail_row)?
                    .collect::<std::result::Result<Vec<_>, _>>()?
            }
            (None, Some(rh)) => {
                let mut stmt = conn.prepare(
                    "SELECT m.message_id, m.conversation_id, m.from_host_id, m.to_host_id, m.body, m.created_at_ms
                     FROM peer_mail_messages m
                     JOIN peer_mail_conversations c ON c.conversation_id = m.conversation_id
                     WHERE c.remote_host_id = ?1
                     ORDER BY m.created_at_ms ASC, m.message_id ASC",
                )?;
                stmt.query_map(params![rh], Self::map_peer_mail_row)?
                    .collect::<std::result::Result<Vec<_>, _>>()?
            }
            (None, None) => {
                let mut stmt = conn.prepare(
                    "SELECT message_id, conversation_id, from_host_id, to_host_id, body, created_at_ms
                     FROM peer_mail_messages
                     ORDER BY created_at_ms ASC, message_id ASC",
                )?;
                stmt.query_map([], Self::map_peer_mail_row)?
                    .collect::<std::result::Result<Vec<_>, _>>()?
            }
        };
        Ok(rows)
    }

    fn map_peer_mail_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<PeerMailMessage> {
        Ok(PeerMailMessage {
            message_id: row.get(0)?,
            conversation_id: row.get(1)?,
            from_host_id: row.get(2)?,
            to_host_id: row.get(3)?,
            body: row.get(4)?,
            created_at_ms: row.get(5)?,
        })
    }

    /// Upserts a cross-machine `peer_runs` mirror row.
    pub fn upsert_peer_run(
        &self,
        peer_run_id: &str,
        home_host_id: &str,
        remote_host_id: &str,
        remote_run_id: &str,
        status: &str,
        now_ms: Option<i64>,
    ) -> Result<PeerRunRecord> {
        let now_ms = now_ms.unwrap_or_else(now_epoch_ms);
        let conn = self.conn.lock();
        conn.execute(
            "INSERT INTO peer_runs
             (peer_run_id, home_host_id, remote_host_id, remote_run_id, status, updated_at_ms)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)
             ON CONFLICT(peer_run_id) DO UPDATE SET
               status = excluded.status,
               updated_at_ms = excluded.updated_at_ms",
            params![peer_run_id, home_host_id, remote_host_id, remote_run_id, status, now_ms],
        )?;
        Ok(PeerRunRecord {
            peer_run_id: peer_run_id.to_string(),
            home_host_id: home_host_id.to_string(),
            remote_host_id: remote_host_id.to_string(),
            remote_run_id: remote_run_id.to_string(),
            status: status.to_string(),
            updated_at_ms: now_ms,
        })
    }
}
