//! Durable SQLite WAL Multi-Agent Orchestration & Federation Engine (`F11`).
//!
//! Backed by the full upstream v51 schema (20 tables + `execution_hosts`), reusing
//! `orca_core::orchestration_protocol` for DAG cycle/readiness validation, monotonic
//! generation-fenced dispatches, non-guessing worker liveness, 9-type typed mailbox
//! routing, review/merge wait-deadline gates, and fail-closed host selection.

mod gates_federation;
mod schema;
#[cfg(test)]
mod tests;
mod workers_mailbox;

use std::path::Path;

use orca_core::OrcaIdeErrorCode;
use orca_core::orchestration_protocol::{
    DispatchRecord, DispatchRegistry, DispatchStatus, GateStatus, MailboxMessageType,
    MailboxPriority, OrchestrationDag, OrchestrationTask, OrchestrationTaskStatus,
};
use parking_lot::Mutex;
use rusqlite::{Connection, OptionalExtension, params};
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use tokio::sync::broadcast;
use uuid::Uuid;

pub use gates_federation::{
    CreateGateRequest, DecisionGateKind, DecisionGatePayload, DecisionGateRecord,
    DispatchFederatedRequest, FederatedDispatchRecord, FederationRelayDirection,
    FederationRelayItem, PeerMailConversation, PeerMailMessage, PeerRunRecord,
    RemoteDispatchAttachment, RemoteQuestionRecord, ResolveGateAction, SendPeerMailRequest,
};
pub use schema::ORCHESTRATION_SCHEMA_VERSION;
pub use workers_mailbox::{
    CheckMailboxRequest, CompleteWorkerOutcome, CompleteWorkerRequest, DeliveryWithMessage,
    DispatchContextRecord, FollowUpReopenOutcome, LivenessReportOutcome, ObservationFact,
    OrchestrationQuestion, SendMessageOutcome, SendMessageRequest, StartWorkerRequest,
    StartWorkerResult, WorkerTerminalArchive, WorkerTerminalResource, parse_mailbox_target,
};

use crate::{OrcaIdeRuntimeError, Result, canonical_json_sha256, now_epoch_ms};
use schema::{
    cols_to_liveness, initialize_schema, str_to_dispatch_status, str_to_task_status,
    task_status_to_str,
};

const WAKE_CHANNEL_CAPACITY: usize = 256;

/// Run lifecycle status persisted in `runs.status`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OrchestrationRunStatus {
    Active,
    Paused,
    Completed,
    Failed,
    Canceled,
}

impl OrchestrationRunStatus {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Active => "active",
            Self::Paused => "paused",
            Self::Completed => "completed",
            Self::Failed => "failed",
            Self::Canceled => "canceled",
        }
    }

    pub fn parse(raw: &str) -> Result<Self> {
        match raw {
            "active" => Ok(Self::Active),
            "paused" => Ok(Self::Paused),
            "completed" => Ok(Self::Completed),
            "failed" => Ok(Self::Failed),
            "canceled" => Ok(Self::Canceled),
            other => Err(OrcaIdeRuntimeError::invalid_argument(format!(
                "unknown run status '{other}'"
            ))),
        }
    }
}

/// Persisted orchestration run record (`runs` + `coordinator_runs`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OrchestrationRunRecord {
    pub run_id: String,
    pub title: String,
    pub consumer_generation: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub coordinator_orca_session_id: Option<String>,
    pub status: OrchestrationRunStatus,
    pub mailbox_is_local_only: bool,
    pub wake_state: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_wake_at_ms: Option<i64>,
    pub created_at_ms: i64,
    pub updated_at_ms: i64,
}

/// Request to create an orchestration run.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CreateRunRequest {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub run_id: Option<String>,
    pub title: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub coordinator_orca_session_id: Option<String>,
    #[serde(default = "default_local_only")]
    pub mailbox_is_local_only: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub now_ms: Option<i64>,
}

const fn default_local_only() -> bool {
    true
}

/// Request to add a DAG task into an existing run.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CreateTaskRequest {
    pub run_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub task_id: Option<String>,
    pub title: String,
    #[serde(default)]
    pub depends_on: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub nesting_depth: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub now_ms: Option<i64>,
}

/// Wake-driven push notifications emitted by `OrchestrationEngine` (zero polling).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum OrchestrationWakeEvent {
    RunCreated {
        run_id: String,
        timestamp_ms: i64,
    },
    TasksBecameReady {
        run_id: String,
        task_ids: Vec<String>,
        timestamp_ms: i64,
    },
    WorkerStarted {
        run_id: String,
        task_id: String,
        dispatch_id: String,
        host_id: String,
        generation: u64,
        timestamp_ms: i64,
    },
    WorkerLivenessUnverifiable {
        run_id: String,
        task_id: String,
        dispatch_id: String,
        generation: u64,
        timestamp_ms: i64,
    },
    WorkerCompleted {
        run_id: String,
        task_id: String,
        dispatch_id: String,
        status: DispatchStatus,
        released_resources: usize,
        timestamp_ms: i64,
    },
    MailboxMessageArrived {
        run_id: String,
        message_id: String,
        message_type: MailboxMessageType,
        priority: MailboxPriority,
        recipients: Vec<String>,
        timestamp_ms: i64,
    },
    FollowUpReopened {
        run_id: String,
        original_dispatch_id: String,
        follow_up_task_id: String,
        follow_up_dispatch_id: String,
        timestamp_ms: i64,
    },
    QuestionUpdated {
        run_id: String,
        question_id: String,
        answered: bool,
        timestamp_ms: i64,
    },
    GateUpdated {
        run_id: String,
        gate_id: String,
        task_id: String,
        status: GateStatus,
        timestamp_ms: i64,
    },
    FederationRelayEnqueued {
        dispatch_id: String,
        seq: i64,
        direction: FederationRelayDirection,
        timestamp_ms: i64,
    },
}

/// Thread-safe (`Send + Sync`), WAL-backed durable multi-agent orchestration engine.
pub struct OrchestrationEngine {
    pub(crate) conn: Mutex<Connection>,
    pub(crate) wake_tx: broadcast::Sender<OrchestrationWakeEvent>,
}

impl std::fmt::Debug for OrchestrationEngine {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("OrchestrationEngine")
            .field("wake_subscribers", &self.wake_tx.receiver_count())
            .finish()
    }
}

impl OrchestrationEngine {
    /// Opens (or creates) a durable WAL-mode SQLite orchestration database at `path`.
    pub fn open(path: impl AsRef<Path>) -> Result<Self> {
        let path = path.as_ref();
        if let Some(parent) = path.parent()
            && !parent.as_os_str().is_empty()
        {
            std::fs::create_dir_all(parent)?;
        }
        let conn = Connection::open(path)?;
        initialize_schema(&conn)?;
        let (wake_tx, _) = broadcast::channel(WAKE_CHANNEL_CAPACITY);
        Ok(Self {
            conn: Mutex::new(conn),
            wake_tx,
        })
    }

    /// Opens an isolated in-memory orchestration engine (useful for tests or ephemeral sessions).
    pub fn in_memory() -> Result<Self> {
        let conn = Connection::open_in_memory()?;
        initialize_schema(&conn)?;
        let (wake_tx, _) = broadcast::channel(WAKE_CHANNEL_CAPACITY);
        Ok(Self {
            conn: Mutex::new(conn),
            wake_tx,
        })
    }

    /// Subscribes to wake-driven push notifications emitted on every state mutation.
    pub fn subscribe_wakes(&self) -> broadcast::Receiver<OrchestrationWakeEvent> {
        self.wake_tx.subscribe()
    }

    pub(crate) fn emit_wake(&self, event: OrchestrationWakeEvent) {
        let _ = self.wake_tx.send(event);
    }

    pub(crate) fn touch_coordinator_wake(
        conn: &Connection,
        run_id: &str,
        wake_state: &str,
        now_ms: i64,
    ) -> Result<()> {
        conn.execute(
            "UPDATE coordinator_runs SET wake_state = ?1, last_wake_at_ms = ?2 WHERE run_id = ?3",
            params![wake_state, now_ms, run_id],
        )?;
        Ok(())
    }

    /// Executes a durable mutation strictly idempotently against `mutation_receipts`
    /// keyed by `(caller_fingerprint, request_id)`:
    /// - Replaying the same `(caller_fingerprint, request_id)` with an identical canonical
    ///   payload returns the cached response WITHOUT re-executing `op`.
    /// - Reusing `(caller_fingerprint, request_id)` with a different payload fails with `Conflict`.
    pub fn execute_idempotent<P, R, F>(
        &self,
        caller_fingerprint: &str,
        request_id: &str,
        payload: &P,
        now_ms: Option<i64>,
        op: F,
    ) -> Result<(R, bool)>
    where
        P: Serialize,
        R: Serialize + DeserializeOwned,
        F: FnOnce(&Self) -> Result<R>,
    {
        if caller_fingerprint.trim().is_empty() || request_id.trim().is_empty() {
            return Err(OrcaIdeRuntimeError::invalid_argument(
                "caller_fingerprint and request_id must be non-empty",
            ));
        }
        let payload_val = serde_json::to_value(payload)?;
        let payload_hash = canonical_json_sha256(&payload_val);

        {
            let conn = self.conn.lock();
            let existing: Option<(String, String)> = conn
                .query_row(
                    "SELECT payload_hash, response_json FROM mutation_receipts
                     WHERE caller_fingerprint = ?1 AND request_id = ?2",
                    params![caller_fingerprint, request_id],
                    |row| Ok((row.get(0)?, row.get(1)?)),
                )
                .optional()?;
            if let Some((stored_hash, stored_json)) = existing {
                if stored_hash != payload_hash {
                    return Err(OrcaIdeRuntimeError::conflict(format!(
                        "idempotency key ({caller_fingerprint}, {request_id}) reused with mismatched payload"
                    )));
                }
                let cached: R = serde_json::from_str(&stored_json)?;
                return Ok((cached, true));
            }
        }

        let result = op(self)?;
        let response_json = serde_json::to_string(&result)?;
        let ts = now_ms.unwrap_or_else(now_epoch_ms);

        let conn = self.conn.lock();
        let inserted = conn.execute(
            "INSERT OR IGNORE INTO mutation_receipts
             (caller_fingerprint, request_id, payload_hash, response_json, created_at_ms)
             VALUES (?1, ?2, ?3, ?4, ?5)",
            params![caller_fingerprint, request_id, payload_hash, response_json, ts],
        )?;
        if inserted == 0 {
            let (stored_hash, stored_json): (String, String) = conn.query_row(
                "SELECT payload_hash, response_json FROM mutation_receipts
                 WHERE caller_fingerprint = ?1 AND request_id = ?2",
                params![caller_fingerprint, request_id],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )?;
            if stored_hash != payload_hash {
                return Err(OrcaIdeRuntimeError::conflict(format!(
                    "idempotency key ({caller_fingerprint}, {request_id}) raced with mismatched payload"
                )));
            }
            let cached: R = serde_json::from_str(&stored_json)?;
            return Ok((cached, true));
        }

        Ok((result, false))
    }

    /// Creates a new orchestration run and initializes its coordinator wake row.
    pub fn create_run(&self, req: CreateRunRequest) -> Result<OrchestrationRunRecord> {
        if req.title.trim().is_empty() {
            return Err(OrcaIdeRuntimeError::invalid_argument(
                "run title cannot be empty",
            ));
        }
        let now_ms = req.now_ms.unwrap_or_else(now_epoch_ms);
        let run_id = req
            .run_id
            .filter(|id| !id.trim().is_empty())
            .unwrap_or_else(|| format!("run-{}", Uuid::new_v4()));
        let coord_session = req
            .coordinator_orca_session_id
            .clone()
            .unwrap_or_else(|| format!("coord-{run_id}"));

        let conn = self.conn.lock();
        let exists: bool = conn
            .query_row(
                "SELECT 1 FROM runs WHERE run_id = ?1",
                params![&run_id],
                |_| Ok(true),
            )
            .optional()?
            .unwrap_or(false);
        if exists {
            return Err(OrcaIdeRuntimeError::conflict(format!(
                "run '{run_id}' already exists"
            )));
        }

        conn.execute(
            "INSERT INTO runs
             (run_id, title, consumer_generation, coordinator_orca_session_id, status,
              mailbox_is_local_only, created_at_ms, updated_at_ms)
             VALUES (?1, ?2, 1, ?3, 'active', ?4, ?5, ?5)",
            params![
                &run_id,
                req.title.trim(),
                &req.coordinator_orca_session_id,
                i32::from(req.mailbox_is_local_only),
                now_ms
            ],
        )?;
        conn.execute(
            "INSERT INTO coordinator_runs (run_id, coordinator_session_id, wake_state, last_wake_at_ms)
             VALUES (?1, ?2, 'awake', ?3)",
            params![&run_id, &coord_session, now_ms],
        )?;
        drop(conn);

        let record = OrchestrationRunRecord {
            run_id: run_id.clone(),
            title: req.title.trim().to_string(),
            consumer_generation: 1,
            coordinator_orca_session_id: req.coordinator_orca_session_id,
            status: OrchestrationRunStatus::Active,
            mailbox_is_local_only: req.mailbox_is_local_only,
            wake_state: "awake".to_string(),
            last_wake_at_ms: Some(now_ms),
            created_at_ms: now_ms,
            updated_at_ms: now_ms,
        };
        self.emit_wake(OrchestrationWakeEvent::RunCreated {
            run_id,
            timestamp_ms: now_ms,
        });
        Ok(record)
    }
}

impl OrchestrationEngine {
    /// Fetches a persisted run record by `run_id`.
    pub fn get_run(&self, run_id: &str) -> Result<OrchestrationRunRecord> {
        let conn = self.conn.lock();
        Self::get_run_inner(&conn, run_id)
    }

    pub(crate) fn get_run_inner(conn: &Connection, run_id: &str) -> Result<OrchestrationRunRecord> {
        let row = conn
            .query_row(
                "SELECT r.run_id, r.title, r.consumer_generation, r.coordinator_orca_session_id,
                        r.status, r.mailbox_is_local_only, COALESCE(c.wake_state, 'idle'),
                        c.last_wake_at_ms, r.created_at_ms, r.updated_at_ms
                 FROM runs r
                 LEFT JOIN coordinator_runs c ON c.run_id = r.run_id
                 WHERE r.run_id = ?1",
                params![run_id],
                |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, i64>(2)?,
                        row.get::<_, Option<String>>(3)?,
                        row.get::<_, String>(4)?,
                        row.get::<_, i64>(5)?,
                        row.get::<_, String>(6)?,
                        row.get::<_, Option<i64>>(7)?,
                        row.get::<_, i64>(8)?,
                        row.get::<_, i64>(9)?,
                    ))
                },
            )
            .optional()?
            .ok_or_else(|| OrcaIdeRuntimeError::not_found(format!("unknown run '{run_id}'")))?;

        Ok(OrchestrationRunRecord {
            run_id: row.0,
            title: row.1,
            consumer_generation: u64::try_from(row.2.max(0)).unwrap_or(1),
            coordinator_orca_session_id: row.3,
            status: OrchestrationRunStatus::parse(&row.4)?,
            mailbox_is_local_only: row.5 != 0,
            wake_state: row.6,
            last_wake_at_ms: row.7,
            created_at_ms: row.8,
            updated_at_ms: row.9,
        })
    }

    /// Lists all orchestration runs ordered by `created_at_ms DESC, run_id ASC`.
    pub fn list_runs(&self, status: Option<OrchestrationRunStatus>) -> Result<Vec<OrchestrationRunRecord>> {
        let conn = self.conn.lock();
        let mut out = Vec::new();
        let sql = match status {
            Some(_) => {
                "SELECT r.run_id, r.title, r.consumer_generation, r.coordinator_orca_session_id,
                        r.status, r.mailbox_is_local_only, COALESCE(c.wake_state, 'idle'),
                        c.last_wake_at_ms, r.created_at_ms, r.updated_at_ms
                 FROM runs r
                 LEFT JOIN coordinator_runs c ON c.run_id = r.run_id
                 WHERE r.status = ?1
                 ORDER BY r.created_at_ms DESC, r.run_id ASC"
            }
            None => {
                "SELECT r.run_id, r.title, r.consumer_generation, r.coordinator_orca_session_id,
                        r.status, r.mailbox_is_local_only, COALESCE(c.wake_state, 'idle'),
                        c.last_wake_at_ms, r.created_at_ms, r.updated_at_ms
                 FROM runs r
                 LEFT JOIN coordinator_runs c ON c.run_id = r.run_id
                 ORDER BY r.created_at_ms DESC, r.run_id ASC"
            }
        };
        let mut stmt = conn.prepare(sql)?;
        let rows = if let Some(st) = status {
            stmt.query_map(params![st.as_str()], Self::map_run_row)?
                .collect::<std::result::Result<Vec<_>, _>>()?
        } else {
            stmt.query_map([], Self::map_run_row)?
                .collect::<std::result::Result<Vec<_>, _>>()?
        };
        for row in rows {
            out.push(OrchestrationRunRecord {
                run_id: row.0,
                title: row.1,
                consumer_generation: u64::try_from(row.2.max(0)).unwrap_or(1),
                coordinator_orca_session_id: row.3,
                status: OrchestrationRunStatus::parse(&row.4)?,
                mailbox_is_local_only: row.5 != 0,
                wake_state: row.6,
                last_wake_at_ms: row.7,
                created_at_ms: row.8,
                updated_at_ms: row.9,
            });
        }
        Ok(out)
    }

    #[allow(clippy::type_complexity)]
    fn map_run_row(
        row: &rusqlite::Row<'_>,
    ) -> rusqlite::Result<(
        String,
        String,
        i64,
        Option<String>,
        String,
        i64,
        String,
        Option<i64>,
        i64,
        i64,
    )> {
        Ok((
            row.get(0)?,
            row.get(1)?,
            row.get(2)?,
            row.get(3)?,
            row.get(4)?,
            row.get(5)?,
            row.get(6)?,
            row.get(7)?,
            row.get(8)?,
            row.get(9)?,
        ))
    }

    /// Adds a new task into a run's DAG, validating dependency existence & Kahn acyclicity
    /// via `orca_core::orchestration_protocol::OrchestrationDag` (`UnknownTask` on cycle/missing dep).
    pub fn create_task(&self, req: CreateTaskRequest) -> Result<OrchestrationTask> {
        if req.title.trim().is_empty() {
            return Err(OrcaIdeRuntimeError::invalid_argument(
                "task title cannot be empty",
            ));
        }
        let now_ms = req.now_ms.unwrap_or_else(now_epoch_ms);
        let task_id = req
            .task_id
            .filter(|id| !id.trim().is_empty())
            .unwrap_or_else(|| format!("task-{}", Uuid::new_v4()));
        // Fail-closed depth default is 1.
        let nesting_depth = req.nesting_depth.unwrap_or(1).max(1);

        let conn = self.conn.lock();
        let _run = Self::get_run_inner(&conn, &req.run_id)?;
        let mut dag = Self::load_dag(&conn, &req.run_id)?;

        let candidate = OrchestrationTask {
            task_id: task_id.clone(),
            run_id: req.run_id.clone(),
            title: req.title.trim().to_string(),
            depends_on: req.depends_on.clone(),
            status: OrchestrationTaskStatus::Pending,
            assigned_host_id: None,
            assigned_dispatch_id: None,
            nesting_depth,
            created_at_ms: now_ms,
            updated_at_ms: now_ms,
        };
        dag.add_task(candidate)?;
        dag.validate_acyclic()?;
        let newly_ready = dag.refresh_readiness(now_ms);
        let stored = dag.tasks.get(&task_id).cloned().ok_or_else(|| {
            OrcaIdeRuntimeError::ide(
                OrcaIdeErrorCode::UnknownTask,
                format!("task '{task_id}' missing after insert"),
            )
        })?;

        let deps_json = serde_json::to_string(&stored.depends_on)?;
        conn.execute(
            "INSERT INTO tasks
             (task_id, run_id, title, deps, parent_id, status, assigned_host_id,
              assigned_dispatch_id, nesting_depth, created_at_ms, updated_at_ms)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, NULL, NULL, ?7, ?8, ?8)",
            params![
                &stored.task_id,
                &stored.run_id,
                &stored.title,
                &deps_json,
                &req.parent_id,
                task_status_to_str(stored.status),
                i64::from(stored.nesting_depth),
                now_ms
            ],
        )?;
        if !newly_ready.is_empty() {
            Self::touch_coordinator_wake(&conn, &req.run_id, "tasks_ready", now_ms)?;
        }
        drop(conn);

        if !newly_ready.is_empty() {
            self.emit_wake(OrchestrationWakeEvent::TasksBecameReady {
                run_id: req.run_id,
                task_ids: newly_ready,
                timestamp_ms: now_ms,
            });
        }
        Ok(stored)
    }

    /// Transitions a task's status using `OrchestrationDag::transition_task` and promotes
    /// newly unlocked downstream tasks to `Ready`.
    pub fn transition_task(
        &self,
        run_id: &str,
        task_id: &str,
        next: OrchestrationTaskStatus,
        now_ms: Option<i64>,
    ) -> Result<(OrchestrationTask, Vec<String>)> {
        let now_ms = now_ms.unwrap_or_else(now_epoch_ms);
        let conn = self.conn.lock();
        let _run = Self::get_run_inner(&conn, run_id)?;
        let mut dag = Self::load_dag(&conn, run_id)?;

        let prev_ready: std::collections::BTreeSet<String> = dag
            .tasks
            .values()
            .filter(|t| t.status == OrchestrationTaskStatus::Ready)
            .map(|t| t.task_id.clone())
            .collect();

        dag.transition_task(task_id, next, now_ms)?;
        Self::persist_dag_statuses(&conn, &dag)?;

        let newly_ready: Vec<String> = dag
            .tasks
            .values()
            .filter(|t| t.status == OrchestrationTaskStatus::Ready && !prev_ready.contains(&t.task_id))
            .map(|t| t.task_id.clone())
            .collect();
        if !newly_ready.is_empty() {
            Self::touch_coordinator_wake(&conn, run_id, "tasks_ready", now_ms)?;
        }
        let updated = dag.tasks.get(task_id).cloned().ok_or_else(|| {
            OrcaIdeRuntimeError::ide(
                OrcaIdeErrorCode::UnknownTask,
                format!("unknown task '{task_id}'"),
            )
        })?;
        drop(conn);

        if !newly_ready.is_empty() {
            self.emit_wake(OrchestrationWakeEvent::TasksBecameReady {
                run_id: run_id.to_string(),
                task_ids: newly_ready.clone(),
                timestamp_ms: now_ms,
            });
        }
        Ok((updated, newly_ready))
    }

    /// Returns all tasks in `run_id` currently eligible to run (`OrchestrationDag::ready_tasks`).
    pub fn ready_tasks(&self, run_id: &str) -> Result<Vec<OrchestrationTask>> {
        let conn = self.conn.lock();
        let _run = Self::get_run_inner(&conn, run_id)?;
        let dag = Self::load_dag(&conn, run_id)?;
        Ok(dag.ready_tasks().into_iter().cloned().collect())
    }

    pub(crate) fn load_dag(conn: &Connection, run_id: &str) -> Result<OrchestrationDag> {
        let mut dag = OrchestrationDag::new(run_id);
        let mut stmt = conn.prepare(
            "SELECT task_id, run_id, title, deps, status, assigned_host_id,
                    assigned_dispatch_id, nesting_depth, created_at_ms, updated_at_ms
             FROM tasks WHERE run_id = ?1 ORDER BY task_id ASC",
        )?;
        let rows = stmt
            .query_map(params![run_id], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, String>(3)?,
                    row.get::<_, String>(4)?,
                    row.get::<_, Option<String>>(5)?,
                    row.get::<_, Option<String>>(6)?,
                    row.get::<_, i64>(7)?,
                    row.get::<_, i64>(8)?,
                    row.get::<_, i64>(9)?,
                ))
            })?
            .collect::<std::result::Result<Vec<_>, _>>()?;

        for r in rows {
            let deps: Vec<String> = serde_json::from_str(&r.3)?;
            let task = OrchestrationTask {
                task_id: r.0,
                run_id: r.1,
                title: r.2,
                depends_on: deps,
                status: str_to_task_status(&r.4)?,
                assigned_host_id: r.5,
                assigned_dispatch_id: r.6,
                nesting_depth: u32::try_from(r.7.max(1)).unwrap_or(1),
                created_at_ms: r.8,
                updated_at_ms: r.9,
            };
            dag.add_task(task)?;
        }
        Ok(dag)
    }

    pub(crate) fn persist_dag_statuses(conn: &Connection, dag: &OrchestrationDag) -> Result<()> {
        for task in dag.tasks.values() {
            conn.execute(
                "UPDATE tasks SET status = ?1, updated_at_ms = ?2 WHERE task_id = ?3",
                params![task_status_to_str(task.status), task.updated_at_ms, &task.task_id],
            )?;
        }
        Ok(())
    }

    pub(crate) fn load_dispatch_registry(conn: &Connection) -> Result<DispatchRegistry> {
        let mut reg = DispatchRegistry::default();
        let mut stmt = conn.prepare(
            "SELECT w.dispatch_id, w.task_id, w.host_id, w.generation, w.status, w.liveness,
                    w.exit_code, c.depth, c.follow_up_of_dispatch_id, w.created_at_ms, w.updated_at_ms
             FROM worker_dispatches w
             JOIN dispatch_contexts c ON c.dispatch_id = w.dispatch_id",
        )?;
        let rows = stmt
            .query_map([], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, i64>(3)?,
                    row.get::<_, String>(4)?,
                    row.get::<_, String>(5)?,
                    row.get::<_, Option<i32>>(6)?,
                    row.get::<_, i64>(7)?,
                    row.get::<_, Option<String>>(8)?,
                    row.get::<_, i64>(9)?,
                    row.get::<_, i64>(10)?,
                ))
            })?
            .collect::<std::result::Result<Vec<_>, _>>()?;

        for r in rows {
            let rec = DispatchRecord {
                dispatch_id: r.0,
                task_id: r.1,
                host_id: r.2,
                generation: u64::try_from(r.3.max(1)).unwrap_or(1),
                status: str_to_dispatch_status(&r.4)?,
                liveness: cols_to_liveness(&r.5, r.6)?,
                nesting_depth: u32::try_from(r.7.max(1)).unwrap_or(1),
                follow_up_of: r.8,
                created_at_ms: r.9,
                updated_at_ms: r.10,
            };
            reg.register(rec)?;
        }
        Ok(reg)
    }
}
