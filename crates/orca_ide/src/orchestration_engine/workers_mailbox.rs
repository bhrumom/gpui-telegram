//! Worker lifecycle, generation-fenced dispatches, non-guessing liveness,
//! terminal auto-teardown/archives, 9-type mailbox routing, follow-up reopen,
//! and worker questions.

use orca_core::orchestration_protocol::{
    DispatchRecord, DispatchStatus, MailboxDeliveryState, MailboxMessage, MailboxMessageType,
    MailboxPriority, MailboxRoute, MailboxTarget, OrchestrationTaskStatus, WorkerLivenessState,
    validate_mailbox_route,
};
use orca_core::{LOCAL_EXECUTION_HOST_ID, OrcaIdeErrorCode};
use rusqlite::{Connection, OptionalExtension, params};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use super::schema::{
    cols_to_route, cols_to_target, delivery_state_to_str, dispatch_status_to_str,
    liveness_to_cols, priority_to_str, route_to_cols, str_to_delivery_state, str_to_message_type,
    str_to_priority, target_to_cols, task_status_to_str,
};
use super::{OrchestrationEngine, OrchestrationWakeEvent};
use crate::{OrcaIdeRuntimeError, Result, now_epoch_ms, sha256_hex};

/// Persisted `dispatch_contexts` row with fail-closed `depth` (default 1) and capability/token hashes.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DispatchContextRecord {
    pub dispatch_id: String,
    pub task_id: String,
    pub run_id: String,
    pub agent_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub worktree_id: Option<String>,
    pub depth: u32,
    pub consumer_generation: u64,
    pub capability_hash: String,
    pub launch_token_hash: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub follow_up_of_dispatch_id: Option<String>,
    pub keep_open: bool,
    pub setup_failed: bool,
    pub created_at_ms: i64,
}

/// Persisted terminal/PTY resource bound to a worker dispatch.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkerTerminalResource {
    pub resource_id: String,
    pub dispatch_id: String,
    pub tab_id: String,
    pub pty_incarnation_id: String,
    pub owner_role: String,
    pub keep_open: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub auto_release_at_ms: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub released_at_ms: Option<i64>,
}

/// Archived worker terminal transcript stored on worker completion.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkerTerminalArchive {
    pub archive_id: String,
    pub dispatch_id: String,
    pub transcript_text: String,
    pub archived_at_ms: i64,
}

/// Structured observation fact attached to a worker attempt (`attempt_observation_facts`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ObservationFact {
    pub fact_id: String,
    pub dispatch_id: String,
    pub kind: String,
    pub payload_json: serde_json::Value,
    pub observed_at_ms: i64,
}

/// Request to start a worker dispatch on a ready task.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StartWorkerRequest {
    #[serde(alias = "runId")]
    pub run_id: String,
    #[serde(alias = "taskId")]
    pub task_id: String,
    #[serde(default, alias = "dispatchId", skip_serializing_if = "Option::is_none")]
    pub dispatch_id: Option<String>,
    #[serde(
        default,
        alias = "hostId",
        alias = "on",
        alias = "targetServer",
        skip_serializing_if = "Option::is_none"
    )]
    pub host_id: Option<String>,
    #[serde(alias = "agent", alias = "agentId")]
    pub agent_id: String,
    #[serde(default, alias = "worktree", alias = "worktreeId", skip_serializing_if = "Option::is_none")]
    pub worktree_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub depth: Option<u32>,
    #[serde(default, alias = "capabilityManifest", skip_serializing_if = "Option::is_none")]
    pub capability_manifest: Option<String>,
    #[serde(default, alias = "launchToken", skip_serializing_if = "Option::is_none")]
    pub launch_token: Option<String>,
    #[serde(default, alias = "followUpOfDispatchId", skip_serializing_if = "Option::is_none")]
    pub follow_up_of_dispatch_id: Option<String>,
    #[serde(default, alias = "keepOpen")]
    pub keep_open: bool,
    #[serde(default, alias = "tabId", skip_serializing_if = "Option::is_none")]
    pub tab_id: Option<String>,
    #[serde(default, alias = "ptyIncarnationId", skip_serializing_if = "Option::is_none")]
    pub pty_incarnation_id: Option<String>,
    #[serde(default, alias = "nowMs", skip_serializing_if = "Option::is_none")]
    pub now_ms: Option<i64>,
}

/// Result returned by `OrchestrationEngine::start_worker`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StartWorkerResult {
    pub context: DispatchContextRecord,
    pub dispatch: DispatchRecord,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub terminal_resource: Option<WorkerTerminalResource>,
}

/// Outcome returned by `OrchestrationEngine::report_liveness`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LivenessReportOutcome {
    pub dispatch_id: String,
    pub generation: u64,
    pub status: DispatchStatus,
    pub liveness: WorkerLivenessState,
    pub confirmed_exit: bool,
    pub retry_authorized: bool,
    pub coordinator_woken: bool,
}

const fn default_expected_gen_1() -> u64 {
    1
}

const fn default_succeeded_true() -> bool {
    true
}

/// Request to settle a worker dispatch and handle terminal archiving/auto-teardown.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CompleteWorkerRequest {
    #[serde(alias = "dispatchId", alias = "dispatch")]
    pub dispatch_id: String,
    #[serde(default = "default_expected_gen_1", alias = "expectedGeneration")]
    pub expected_generation: u64,
    #[serde(default = "default_succeeded_true")]
    pub succeeded: bool,
    #[serde(default, alias = "exitCode", skip_serializing_if = "Option::is_none")]
    pub exit_code: Option<i32>,
    #[serde(
        default,
        alias = "transcriptText",
        alias = "summary",
        skip_serializing_if = "Option::is_none"
    )]
    pub transcript_text: Option<String>,
    #[serde(default, alias = "keepOpen")]
    pub keep_open: bool,
    #[serde(default, alias = "setupFailed")]
    pub setup_failed: bool,
    #[serde(default, alias = "nowMs", skip_serializing_if = "Option::is_none")]
    pub now_ms: Option<i64>,
}

/// Outcome returned by `OrchestrationEngine::complete_worker`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CompleteWorkerOutcome {
    pub dispatch_id: String,
    pub new_generation: u64,
    pub status: DispatchStatus,
    pub task_status: OrchestrationTaskStatus,
    pub newly_ready_tasks: Vec<String>,
    pub resources_released: usize,
    pub kept_open: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub archive: Option<WorkerTerminalArchive>,
}

/// Request to send a typed orchestration mailbox message.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SendMessageRequest {
    pub run_id: String,
    pub from_id: String,
    pub target: MailboxTarget,
    pub message_type: MailboxMessageType,
    #[serde(default)]
    pub priority: MailboxPriority,
    #[serde(default = "default_local_route")]
    pub route: MailboxRoute,
    pub body: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dispatch_generation: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub now_ms: Option<i64>,
}

fn default_local_route() -> MailboxRoute {
    MailboxRoute::LocalOnly
}

/// Details of an automatic follow-up task + dispatch created when addressing a finished `dispatch:<id>`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FollowUpReopenOutcome {
    pub original_dispatch_id: String,
    pub follow_up_task_id: String,
    pub follow_up_dispatch_id: String,
}

/// Outcome of `OrchestrationEngine::send_message`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SendMessageOutcome {
    pub message: MailboxMessage,
    pub delivery_ids: Vec<String>,
    pub recipients: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub follow_up: Option<FollowUpReopenOutcome>,
}

/// Request to check & claim mailbox deliveries for a recipient (optionally generation-fenced).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CheckMailboxRequest {
    pub recipient_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dispatch_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expected_generation: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub now_ms: Option<i64>,
}

/// Mailbox delivery joined with its underlying typed `MailboxMessage`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DeliveryWithMessage {
    pub delivery_id: String,
    pub recipient_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub generation: Option<u64>,
    pub state: MailboxDeliveryState,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub delivered_at_ms: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub acked_at_ms: Option<i64>,
    pub message: MailboxMessage,
}

/// Worker question persisted in `questions`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OrchestrationQuestion {
    pub question_id: String,
    pub run_id: String,
    pub dispatch_id: String,
    pub prompt: String,
    pub status: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub answer: Option<String>,
    pub created_at_ms: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub answered_at_ms: Option<i64>,
}

/// Parses shorthand target strings (`@all`, `@idle`, `@worktree:<id>`, `@agent:<name>`, `@<agent>`, `dispatch:<id>`).
pub fn parse_mailbox_target(raw: &str) -> MailboxTarget {
    let trimmed = raw.trim();
    if trimmed == "@all" {
        MailboxTarget::GroupAll
    } else if trimmed == "@idle" {
        MailboxTarget::GroupIdle
    } else if let Some(wt) = trimmed.strip_prefix("@worktree:") {
        MailboxTarget::GroupWorktree {
            worktree_id: wt.trim().to_string(),
        }
    } else if let Some(agent) = trimmed.strip_prefix("@agent:") {
        MailboxTarget::GroupAgent {
            agent: agent.trim().to_string(),
        }
    } else if let Some(agent) = trimmed.strip_prefix('@') {
        MailboxTarget::GroupAgent {
            agent: agent.trim().to_string(),
        }
    } else {
        MailboxTarget::Direct {
            recipient_id: trimmed.to_string(),
        }
    }
}

impl OrchestrationEngine {
    /// Starts a worker dispatch on a task, verifying DAG readiness, creating fail-closed
    /// `dispatch_contexts` (`depth` >= 1), monotonic generation-fenced `worker_dispatches`,
    /// and optional tracked `worker_terminal_resources`.
    pub fn start_worker(&self, req: StartWorkerRequest) -> Result<StartWorkerResult> {
        if req.agent_id.trim().is_empty() {
            return Err(OrcaIdeRuntimeError::invalid_argument(
                "agent_id cannot be empty",
            ));
        }
        let now_ms = req.now_ms.unwrap_or_else(now_epoch_ms);
        let dispatch_id = req
            .dispatch_id
            .filter(|id| !id.trim().is_empty())
            .unwrap_or_else(|| format!("disp-{}", Uuid::new_v4()));
        let host_id = req
            .host_id
            .filter(|id| !id.trim().is_empty())
            .unwrap_or_else(|| LOCAL_EXECUTION_HOST_ID.to_string());
        let depth = req.depth.unwrap_or(1).max(1);
        let capability_hash = sha256_hex(
            req.capability_manifest
                .as_deref()
                .unwrap_or("default-worker-capabilities")
                .as_bytes(),
        );
        let launch_token_hash = sha256_hex(
            req.launch_token
                .as_deref()
                .unwrap_or(&dispatch_id)
                .as_bytes(),
        );

        let conn = self.conn.lock();
        let run = Self::get_run_inner(&conn, &req.run_id)?;
        let mut dag = Self::load_dag(&conn, &req.run_id)?;
        dag.transition_task(&req.task_id, OrchestrationTaskStatus::Running, now_ms)?;

        let mut registry = Self::load_dispatch_registry(&conn)?;
        let dispatch = DispatchRecord {
            dispatch_id: dispatch_id.clone(),
            task_id: req.task_id.clone(),
            host_id: host_id.clone(),
            generation: 1,
            status: DispatchStatus::Dispatched,
            liveness: WorkerLivenessState::Alive,
            nesting_depth: depth,
            follow_up_of: req.follow_up_of_dispatch_id.clone(),
            created_at_ms: now_ms,
            updated_at_ms: now_ms,
        };
        registry.register(dispatch.clone())?;

        conn.execute(
            "INSERT INTO dispatch_contexts
             (dispatch_id, task_id, run_id, agent_id, worktree_id, depth, consumer_generation,
              capability_hash, launch_token_hash, follow_up_of_dispatch_id, keep_open,
              setup_failed, created_at_ms)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, 0, ?12)",
            params![
                &dispatch_id,
                &req.task_id,
                &req.run_id,
                req.agent_id.trim(),
                &req.worktree_id,
                i64::from(depth),
                i64::try_from(run.consumer_generation).unwrap_or(1),
                &capability_hash,
                &launch_token_hash,
                &req.follow_up_of_dispatch_id,
                i32::from(req.keep_open),
                now_ms
            ],
        )?;

        let (liveness_str, exit_code) = liveness_to_cols(dispatch.liveness);
        conn.execute(
            "INSERT INTO worker_dispatches
             (dispatch_id, task_id, host_id, generation, status, liveness, exit_code,
              is_idle, created_at_ms, updated_at_ms)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, 0, ?8, ?8)",
            params![
                &dispatch_id,
                &req.task_id,
                &host_id,
                i64::try_from(dispatch.generation).unwrap_or(1),
                dispatch_status_to_str(dispatch.status),
                liveness_str,
                exit_code,
                now_ms
            ],
        )?;

        conn.execute(
            "UPDATE tasks
             SET status = ?1, assigned_host_id = ?2, assigned_dispatch_id = ?3, updated_at_ms = ?4
             WHERE task_id = ?5",
            params![
                task_status_to_str(OrchestrationTaskStatus::Running),
                &host_id,
                &dispatch_id,
                now_ms,
                &req.task_id
            ],
        )?;

        let terminal_resource = if let (Some(tab_id), Some(pty_incarnation_id)) =
            (req.tab_id.clone(), req.pty_incarnation_id.clone())
        {
            let resource_id = format!("res-{}", Uuid::new_v4());
            conn.execute(
                "INSERT INTO worker_terminal_resources
                 (resource_id, dispatch_id, tab_id, pty_incarnation_id, owner_role, keep_open,
                  auto_release_at_ms, released_at_ms)
                 VALUES (?1, ?2, ?3, ?4, 'worker', ?5, NULL, NULL)",
                params![
                    &resource_id,
                    &dispatch_id,
                    &tab_id,
                    &pty_incarnation_id,
                    i32::from(req.keep_open)
                ],
            )?;
            Some(WorkerTerminalResource {
                resource_id,
                dispatch_id: dispatch_id.clone(),
                tab_id,
                pty_incarnation_id,
                owner_role: "worker".to_string(),
                keep_open: req.keep_open,
                auto_release_at_ms: None,
                released_at_ms: None,
            })
        } else {
            None
        };

        Self::touch_coordinator_wake(&conn, &req.run_id, "worker_started", now_ms)?;
        drop(conn);

        let context = DispatchContextRecord {
            dispatch_id: dispatch_id.clone(),
            task_id: req.task_id.clone(),
            run_id: req.run_id.clone(),
            agent_id: req.agent_id.trim().to_string(),
            worktree_id: req.worktree_id,
            depth,
            consumer_generation: run.consumer_generation,
            capability_hash,
            launch_token_hash,
            follow_up_of_dispatch_id: req.follow_up_of_dispatch_id,
            keep_open: req.keep_open,
            setup_failed: false,
            created_at_ms: now_ms,
        };

        self.emit_wake(OrchestrationWakeEvent::WorkerStarted {
            run_id: req.run_id,
            task_id: req.task_id,
            dispatch_id,
            host_id,
            generation: dispatch.generation,
            timestamp_ms: now_ms,
        });

        Ok(StartWorkerResult {
            context,
            dispatch,
            terminal_resource,
        })
    }

    /// Appends a structured observation fact for a worker attempt (`attempt_observation_facts`).
    pub fn record_observation(
        &self,
        dispatch_id: &str,
        kind: &str,
        payload_json: serde_json::Value,
        now_ms: Option<i64>,
    ) -> Result<ObservationFact> {
        if kind.trim().is_empty() {
            return Err(OrcaIdeRuntimeError::invalid_argument(
                "observation kind cannot be empty",
            ));
        }
        let now_ms = now_ms.unwrap_or_else(now_epoch_ms);
        let fact_id = format!("obs-{}", Uuid::new_v4());
        let serialized = serde_json::to_string(&payload_json)?;

        let conn = self.conn.lock();
        let exists: bool = conn
            .query_row(
                "SELECT 1 FROM dispatch_contexts WHERE dispatch_id = ?1",
                params![dispatch_id],
                |_| Ok(true),
            )
            .optional()?
            .unwrap_or(false);
        if !exists {
            return Err(OrcaIdeRuntimeError::ide(
                OrcaIdeErrorCode::UnknownDispatch,
                format!("unknown dispatch '{dispatch_id}'"),
            ));
        }

        conn.execute(
            "INSERT INTO attempt_observation_facts
             (fact_id, dispatch_id, kind, payload_json, observed_at_ms)
             VALUES (?1, ?2, ?3, ?4, ?5)",
            params![&fact_id, dispatch_id, kind.trim(), &serialized, now_ms],
        )?;
        Ok(ObservationFact {
            fact_id,
            dispatch_id: dispatch_id.to_string(),
            kind: kind.trim().to_string(),
            payload_json,
            observed_at_ms: now_ms,
        })
    }

    /// Marks whether an active worker is currently idle (used for `@idle` group addressing).
    pub fn set_worker_idle(
        &self,
        dispatch_id: &str,
        expected_generation: u64,
        idle: bool,
        now_ms: Option<i64>,
    ) -> Result<()> {
        let now_ms = now_ms.unwrap_or_else(now_epoch_ms);
        let conn = self.conn.lock();
        let reg = Self::load_dispatch_registry(&conn)?;
        reg.verify_generation(dispatch_id, expected_generation)?;
        conn.execute(
            "UPDATE worker_dispatches SET is_idle = ?1, updated_at_ms = ?2 WHERE dispatch_id = ?3",
            params![i32::from(idle), now_ms, dispatch_id],
        )?;
        Ok(())
    }
}

impl OrchestrationEngine {
    /// Reports worker liveness under strict generation fencing and the non-guessing invariant:
    /// `WorkerLivenessState::Unverifiable` NEVER confirms exit, NEVER settles the task, and
    /// NEVER authorizes autonomous retry — instead it persists `unverifiable`, records an
    /// observation fact, and wakes the coordinator.
    pub fn report_liveness(
        &self,
        dispatch_id: &str,
        expected_generation: u64,
        liveness: WorkerLivenessState,
        now_ms: Option<i64>,
    ) -> Result<LivenessReportOutcome> {
        let now_ms = now_ms.unwrap_or_else(now_epoch_ms);
        let conn = self.conn.lock();
        let registry = Self::load_dispatch_registry(&conn)?;
        let current = registry.verify_generation(dispatch_id, expected_generation)?;
        let current_status = current.status;
        let task_id = current.task_id.clone();

        let run_id: String = conn.query_row(
            "SELECT run_id FROM dispatch_contexts WHERE dispatch_id = ?1",
            params![dispatch_id],
            |row| row.get(0),
        )?;

        let (liveness_str, exit_code) = liveness_to_cols(liveness);
        conn.execute(
            "UPDATE worker_dispatches SET liveness = ?1, exit_code = ?2, updated_at_ms = ?3
             WHERE dispatch_id = ?4",
            params![liveness_str, exit_code, now_ms, dispatch_id],
        )?;

        let fact_id = format!("obs-{}", Uuid::new_v4());
        let fact_payload = serde_json::json!({
            "liveness": liveness,
            "confirmed_exit": liveness.confirms_exit(),
            "retry_authorized": liveness.authorizes_autonomous_retry(),
        });
        conn.execute(
            "INSERT INTO attempt_observation_facts
             (fact_id, dispatch_id, kind, payload_json, observed_at_ms)
             VALUES (?1, ?2, 'liveness_probe', ?3, ?4)",
            params![&fact_id, dispatch_id, fact_payload.to_string(), now_ms],
        )?;

        let wake_coordinator = matches!(liveness, WorkerLivenessState::Unverifiable);
        if wake_coordinator {
            Self::touch_coordinator_wake(&conn, &run_id, "liveness_unverifiable", now_ms)?;
        }
        drop(conn);

        if wake_coordinator {
            self.emit_wake(OrchestrationWakeEvent::WorkerLivenessUnverifiable {
                run_id,
                task_id,
                dispatch_id: dispatch_id.to_string(),
                generation: expected_generation,
                timestamp_ms: now_ms,
            });
        }

        Ok(LivenessReportOutcome {
            dispatch_id: dispatch_id.to_string(),
            generation: expected_generation,
            status: current_status,
            liveness,
            confirmed_exit: liveness.confirms_exit(),
            retry_authorized: liveness.authorizes_autonomous_retry(),
            coordinator_woken: wake_coordinator,
        })
    }

    /// Settles a worker dispatch, archives its transcript into `worker_terminal_archives`,
    /// auto-releases tracked terminal resources UNLESS `keep_open` or `setup_failed` is true,
    /// transitions the task in the DAG, and wakes the coordinator for newly ready downstream tasks.
    pub fn complete_worker(&self, req: CompleteWorkerRequest) -> Result<CompleteWorkerOutcome> {
        let now_ms = req.now_ms.unwrap_or_else(now_epoch_ms);
        let conn = self.conn.lock();
        let mut registry = Self::load_dispatch_registry(&conn)?;

        let next_dispatch_status = if req.succeeded && !req.setup_failed {
            DispatchStatus::Completed
        } else {
            DispatchStatus::Failed
        };
        let exit_code = req
            .exit_code
            .or(Some(if req.succeeded && !req.setup_failed { 0 } else { 1 }));
        let liveness = WorkerLivenessState::Exited { exit_code };

        let new_generation = registry.advance_status(
            &req.dispatch_id,
            req.expected_generation,
            next_dispatch_status,
            liveness,
            now_ms,
        )?;

        let (run_id, task_id, ctx_keep_open, ctx_setup_failed): (String, String, i64, i64) = conn
            .query_row(
                "SELECT run_id, task_id, keep_open, setup_failed
                 FROM dispatch_contexts WHERE dispatch_id = ?1",
                params![&req.dispatch_id],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
            )?;

        let effective_setup_failed = req.setup_failed || ctx_setup_failed != 0;
        let effective_keep_open = req.keep_open || ctx_keep_open != 0 || effective_setup_failed;

        let (liveness_str, liveness_exit) = liveness_to_cols(liveness);
        let kept_open_idle = i32::from(effective_keep_open && next_dispatch_status == DispatchStatus::Completed);
        conn.execute(
            "UPDATE worker_dispatches
             SET generation = ?1, status = ?2, liveness = ?3, exit_code = ?4,
                 is_idle = ?5, updated_at_ms = ?6
             WHERE dispatch_id = ?7",
            params![
                i64::try_from(new_generation).unwrap_or(i64::MAX),
                dispatch_status_to_str(next_dispatch_status),
                liveness_str,
                liveness_exit,
                kept_open_idle,
                now_ms,
                &req.dispatch_id
            ],
        )?;

        if req.keep_open || effective_setup_failed {
            conn.execute(
                "UPDATE dispatch_contexts
                 SET keep_open = ?1, setup_failed = ?2
                 WHERE dispatch_id = ?3",
                params![
                    i32::from(req.keep_open || ctx_keep_open != 0),
                    i32::from(effective_setup_failed),
                    &req.dispatch_id
                ],
            )?;
        }

        let archive = if let Some(transcript) = req.transcript_text {
            let archive_id = format!("arch-{}", Uuid::new_v4());
            conn.execute(
                "INSERT INTO worker_terminal_archives
                 (archive_id, dispatch_id, transcript_text, archived_at_ms)
                 VALUES (?1, ?2, ?3, ?4)",
                params![&archive_id, &req.dispatch_id, &transcript, now_ms],
            )?;
            Some(WorkerTerminalArchive {
                archive_id,
                dispatch_id: req.dispatch_id.clone(),
                transcript_text: transcript,
                archived_at_ms: now_ms,
            })
        } else {
            None
        };

        // Auto-teardown terminal resources unless `--keep-open` or setup failed.
        let resources_released = if !effective_keep_open {
            conn.execute(
                "UPDATE worker_terminal_resources
                 SET released_at_ms = ?1
                 WHERE dispatch_id = ?2 AND keep_open = 0 AND released_at_ms IS NULL",
                params![now_ms, &req.dispatch_id],
            )?
        } else {
            0
        };

        // Advance DAG task status and refresh downstream readiness.
        let mut dag = Self::load_dag(&conn, &run_id)?;
        let prev_ready: std::collections::BTreeSet<String> = dag
            .tasks
            .values()
            .filter(|t| t.status == OrchestrationTaskStatus::Ready)
            .map(|t| t.task_id.clone())
            .collect();
        let next_task_status = if req.succeeded && !effective_setup_failed {
            OrchestrationTaskStatus::Succeeded
        } else {
            OrchestrationTaskStatus::Failed
        };
        dag.transition_task(&task_id, next_task_status, now_ms)?;
        Self::persist_dag_statuses(&conn, &dag)?;

        let newly_ready_tasks: Vec<String> = dag
            .tasks
            .values()
            .filter(|t| t.status == OrchestrationTaskStatus::Ready && !prev_ready.contains(&t.task_id))
            .map(|t| t.task_id.clone())
            .collect();

        Self::touch_coordinator_wake(&conn, &run_id, "worker_completed", now_ms)?;
        drop(conn);

        self.emit_wake(OrchestrationWakeEvent::WorkerCompleted {
            run_id: run_id.clone(),
            task_id,
            dispatch_id: req.dispatch_id.clone(),
            status: next_dispatch_status,
            released_resources: resources_released,
            timestamp_ms: now_ms,
        });
        if !newly_ready_tasks.is_empty() {
            self.emit_wake(OrchestrationWakeEvent::TasksBecameReady {
                run_id,
                task_ids: newly_ready_tasks.clone(),
                timestamp_ms: now_ms,
            });
        }

        Ok(CompleteWorkerOutcome {
            dispatch_id: req.dispatch_id,
            new_generation,
            status: next_dispatch_status,
            task_status: next_task_status,
            newly_ready_tasks,
            resources_released,
            kept_open: effective_keep_open,
            archive,
        })
    }

    /// Reads archived terminal transcripts for `dispatch_id` (newest first).
    pub fn read_worker_archive(&self, dispatch_id: &str) -> Result<Vec<WorkerTerminalArchive>> {
        let conn = self.conn.lock();
        let mut stmt = conn.prepare(
            "SELECT archive_id, dispatch_id, transcript_text, archived_at_ms
             FROM worker_terminal_archives
             WHERE dispatch_id = ?1
             ORDER BY archived_at_ms DESC, archive_id ASC",
        )?;
        let rows = stmt
            .query_map(params![dispatch_id], |row| {
                Ok(WorkerTerminalArchive {
                    archive_id: row.get(0)?,
                    dispatch_id: row.get(1)?,
                    transcript_text: row.get(2)?,
                    archived_at_ms: row.get(3)?,
                })
            })?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        Ok(rows)
    }

    /// Explicitly releases all tracked terminal resources for `dispatch_id` (including `--keep-open`).
    pub fn release_worker_resources(
        &self,
        dispatch_id: &str,
        now_ms: Option<i64>,
    ) -> Result<usize> {
        let now_ms = now_ms.unwrap_or_else(now_epoch_ms);
        let conn = self.conn.lock();
        let exists: bool = conn
            .query_row(
                "SELECT 1 FROM dispatch_contexts WHERE dispatch_id = ?1",
                params![dispatch_id],
                |_| Ok(true),
            )
            .optional()?
            .unwrap_or(false);
        if !exists {
            return Err(OrcaIdeRuntimeError::ide(
                OrcaIdeErrorCode::UnknownDispatch,
                format!("unknown dispatch '{dispatch_id}'"),
            ));
        }
        let released = conn.execute(
            "UPDATE worker_terminal_resources
             SET released_at_ms = ?1
             WHERE dispatch_id = ?2 AND released_at_ms IS NULL",
            params![now_ms, dispatch_id],
        )?;
        conn.execute(
            "UPDATE dispatch_contexts SET keep_open = 0 WHERE dispatch_id = ?1",
            params![dispatch_id],
        )?;
        conn.execute(
            "UPDATE worker_dispatches SET is_idle = 0, updated_at_ms = ?1 WHERE dispatch_id = ?2",
            params![now_ms, dispatch_id],
        )?;
        Ok(released)
    }

    /// Appends a note/transcript line into `worker_terminal_archives` for `dispatch_id`.
    pub fn record_worker_archive(
        &self,
        dispatch_id: &str,
        transcript_text: &str,
        now_ms: Option<i64>,
    ) -> Result<WorkerTerminalArchive> {
        let now_ms = now_ms.unwrap_or_else(now_epoch_ms);
        let archive_id = format!("arch-{}", Uuid::new_v4());
        let conn = self.conn.lock();
        conn.execute(
            "INSERT INTO worker_terminal_archives
             (archive_id, dispatch_id, transcript_text, archived_at_ms)
             VALUES (?1, ?2, ?3, ?4)",
            params![&archive_id, dispatch_id, transcript_text, now_ms],
        )?;
        Ok(WorkerTerminalArchive {
            archive_id,
            dispatch_id: dispatch_id.to_string(),
            transcript_text: transcript_text.to_string(),
            archived_at_ms: now_ms,
        })
    }

    /// Looks up `(run_id, task_id)` for a persisted dispatch context.
    pub fn lookup_dispatch_run_task(&self, dispatch_id: &str) -> Result<(String, String)> {
        let conn = self.conn.lock();
        conn.query_row(
            "SELECT run_id, task_id FROM dispatch_contexts WHERE dispatch_id = ?1",
            params![dispatch_id],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .optional()?
        .ok_or_else(|| {
            OrcaIdeRuntimeError::ide(
                OrcaIdeErrorCode::UnknownDispatch,
                format!("unknown dispatch '{dispatch_id}'"),
            )
        })
    }

    /// Finds the most recent dispatch_id associated with `task_id`.
    pub fn lookup_dispatch_for_task(&self, task_id: &str) -> Result<Option<String>> {
        let conn = self.conn.lock();
        let found = conn
            .query_row(
                "SELECT dispatch_id FROM dispatch_contexts
                 WHERE task_id = ?1
                 ORDER BY created_at_ms DESC LIMIT 1",
                params![task_id],
                |row| row.get::<_, String>(0),
            )
            .optional()?;
        Ok(found)
    }

    /// Returns `true` when `dispatch_id` exists in `worker_dispatches` or `federated_dispatches`
    /// and has not yet reached a terminal settled state (`completed`, `failed`, `canceled`, `circuit_broken`).
    pub fn is_dispatch_unsettled(&self, dispatch_id: &str) -> Result<bool> {
        let trimmed = dispatch_id.trim();
        if trimmed.is_empty() {
            return Ok(false);
        }
        let conn = self.conn.lock();
        let local_status: Option<String> = conn
            .query_row(
                "SELECT status FROM worker_dispatches WHERE dispatch_id = ?1",
                params![trimmed],
                |row| row.get(0),
            )
            .optional()?;
        if let Some(status) = local_status {
            return Ok(!matches!(
                status.as_str(),
                "completed" | "failed" | "canceled" | "circuit_broken"
            ));
        }
        let federated_status: Option<String> = conn
            .query_row(
                "SELECT status FROM federated_dispatches WHERE dispatch_id = ?1",
                params![trimmed],
                |row| row.get(0),
            )
            .optional()?;
        if let Some(status) = federated_status {
            return Ok(!matches!(
                status.as_str(),
                "completed" | "failed" | "canceled" | "circuit_broken"
            ));
        }
        Ok(false)
    }

    /// Inspects live/settled worker state including `terminalClosed` for `orchestration.worker.show`.
    pub fn get_worker_show(&self, dispatch_id: &str) -> Result<serde_json::Value> {
        let conn = self.conn.lock();
        let row = conn
            .query_row(
                "SELECT c.dispatch_id, c.run_id, c.task_id, c.agent_id, c.worktree_id,
                        c.keep_open, c.setup_failed, w.host_id, w.generation, w.status, w.is_idle
                 FROM dispatch_contexts c
                 JOIN worker_dispatches w ON w.dispatch_id = c.dispatch_id
                 WHERE c.dispatch_id = ?1",
                params![dispatch_id],
                |r| {
                    Ok((
                        r.get::<_, String>(0)?,
                        r.get::<_, String>(1)?,
                        r.get::<_, String>(2)?,
                        r.get::<_, String>(3)?,
                        r.get::<_, Option<String>>(4)?,
                        r.get::<_, i64>(5)? != 0,
                        r.get::<_, i64>(6)? != 0,
                        r.get::<_, String>(7)?,
                        r.get::<_, i64>(8)?,
                        r.get::<_, String>(9)?,
                        r.get::<_, i64>(10)? != 0,
                    ))
                },
            )
            .optional()?
            .ok_or_else(|| {
                OrcaIdeRuntimeError::ide(
                    OrcaIdeErrorCode::UnknownDispatch,
                    format!("unknown dispatch '{dispatch_id}'"),
                )
            })?;
        let mut stmt = conn.prepare(
            "SELECT resource_id, released_at_ms FROM worker_terminal_resources WHERE dispatch_id = ?1",
        )?;
        let res_rows = stmt
            .query_map(params![dispatch_id], |r| {
                Ok((r.get::<_, String>(0)?, r.get::<_, Option<i64>>(1)?))
            })?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        let settled = matches!(row.9.as_str(), "completed" | "failed" | "canceled");
        let terminal_closed = if !res_rows.is_empty() {
            res_rows.iter().all(|(_, rel)| rel.is_some())
        } else {
            settled && !row.5
        };
        Ok(serde_json::json!({
            "id": row.0,
            "dispatchId": row.0,
            "runId": row.1,
            "taskId": row.2,
            "agentId": row.3,
            "agent": row.3,
            "worktreeId": row.4,
            "keepOpen": row.5,
            "setupFailed": row.6,
            "hostId": row.7,
            "generation": row.8,
            "status": row.9,
            "isIdle": row.10,
            "terminalClosed": terminal_closed,
        }))
    }

    /// Lists all mailbox messages persisted in `run_id` ordered by monotonic SQLite `rowid` (`seq`).
    pub fn list_run_mailbox(&self, run_id: &str) -> Result<Vec<(i64, MailboxMessage)>> {
        let conn = self.conn.lock();
        let _run = Self::get_run_inner(&conn, run_id)?;
        let mut stmt = conn.prepare(
            "SELECT rowid, message_id, run_id, from_id, target_kind, target_value,
                    message_type, priority, route_kind, server_host_id,
                    body, dispatch_generation, delivery_state, created_at_ms
             FROM messages
             WHERE run_id = ?1
             ORDER BY rowid ASC",
        )?;
        let rows = stmt
            .query_map(params![run_id], |row| {
                Ok((
                    row.get::<_, i64>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, String>(3)?,
                    row.get::<_, String>(4)?,
                    row.get::<_, Option<String>>(5)?,
                    row.get::<_, String>(6)?,
                    row.get::<_, String>(7)?,
                    row.get::<_, String>(8)?,
                    row.get::<_, Option<String>>(9)?,
                    row.get::<_, String>(10)?,
                    row.get::<_, Option<i64>>(11)?,
                    row.get::<_, String>(12)?,
                    row.get::<_, i64>(13)?,
                ))
            })?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        let mut out = Vec::with_capacity(rows.len());
        for r in rows {
            let msg = MailboxMessage {
                message_id: r.1,
                run_id: r.2,
                from_id: r.3,
                target: cols_to_target(&r.4, r.5)?,
                message_type: str_to_message_type(&r.6)?,
                priority: str_to_priority(&r.7)?,
                route: cols_to_route(&r.8, r.9)?,
                body: r.10,
                dispatch_generation: r.11.map(|g| u64::try_from(g.max(0)).unwrap_or(0)),
                delivery_state: str_to_delivery_state(&r.12)?,
                created_at_ms: r.13,
            };
            out.push((r.0, msg));
        }
        Ok(out)
    }

    /// Lists all tracked terminal resources (released and unreleased) for `dispatch_id`.
    pub fn list_worker_resources(&self, dispatch_id: &str) -> Result<Vec<WorkerTerminalResource>> {
        let conn = self.conn.lock();
        let mut stmt = conn.prepare(
            "SELECT resource_id, dispatch_id, tab_id, pty_incarnation_id, owner_role,
                    keep_open, auto_release_at_ms, released_at_ms
             FROM worker_terminal_resources
             WHERE dispatch_id = ?1
             ORDER BY resource_id ASC",
        )?;
        let rows = stmt
            .query_map(params![dispatch_id], |row| {
                Ok(WorkerTerminalResource {
                    resource_id: row.get(0)?,
                    dispatch_id: row.get(1)?,
                    tab_id: row.get(2)?,
                    pty_incarnation_id: row.get(3)?,
                    owner_role: row.get(4)?,
                    keep_open: row.get::<_, i64>(5)? != 0,
                    auto_release_at_ms: row.get(6)?,
                    released_at_ms: row.get(7)?,
                })
            })?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        Ok(rows)
    }
}

impl OrchestrationEngine {
    /// Sends a typed mailbox message (`MailboxMessageType` x9, `MailboxPriority` x3), validating
    /// `MailboxRoute` (`MailboxIsLocal` / `ServerMailUnsupported`), expanding group targets
    /// (`@all`, `@idle`, `@<agent>`, `@worktree:<id>`), and seamlessly reopening a linked
    /// follow-up task + dispatch whenever a message targets a finished `dispatch:<id>`.
    pub fn send_message(&self, req: SendMessageRequest) -> Result<SendMessageOutcome> {
        if req.from_id.trim().is_empty() {
            return Err(OrcaIdeRuntimeError::invalid_argument(
                "from_id cannot be empty",
            ));
        }
        let now_ms = req.now_ms.unwrap_or_else(now_epoch_ms);
        let conn = self.conn.lock();
        let run = Self::get_run_inner(&conn, &req.run_id)?;

        // Check server mail capability when routing via ServerRelay.
        let server_supports_mail = match &req.route {
            MailboxRoute::LocalOnly => false,
            MailboxRoute::ServerRelay { server_host_id } => conn
                .query_row(
                    "SELECT supports_mail FROM execution_hosts WHERE host_id = ?1",
                    params![server_host_id],
                    |row| Ok(row.get::<_, i64>(0)? != 0),
                )
                .optional()?
                .unwrap_or(false),
        };
        validate_mailbox_route(&req.route, run.mailbox_is_local_only, server_supports_mail)?;

        // Resolve recipients and handle seamless follow-up reopen on finished `dispatch:<id>`.
        let mut follow_up: Option<FollowUpReopenOutcome> = None;
        let recipients_with_gen: Vec<(String, Option<u64>)> = match &req.target {
            MailboxTarget::Direct { recipient_id } => {
                if let Some(orig_dispatch_id) = recipient_id.strip_prefix("dispatch:") {
                    if let Some((reopened, new_gen)) = Self::maybe_reopen_finished_dispatch(
                        &conn,
                        &run,
                        orig_dispatch_id,
                        now_ms,
                    )? {
                        let new_recipient = format!("dispatch:{}", reopened.follow_up_dispatch_id);
                        follow_up = Some(reopened);
                        vec![(new_recipient, Some(new_gen))]
                    } else {
                        let current_gen = conn
                            .query_row(
                                "SELECT generation FROM worker_dispatches WHERE dispatch_id = ?1",
                                params![orig_dispatch_id],
                                |row| row.get::<_, i64>(0),
                            )
                            .optional()?
                            .map(|g| u64::try_from(g.max(1)).unwrap_or(1));
                        vec![(recipient_id.clone(), req.dispatch_generation.or(current_gen))]
                    }
                } else {
                    vec![(recipient_id.clone(), req.dispatch_generation)]
                }
            }
            MailboxTarget::GroupAll => Self::resolve_group_recipients(&conn, &req.run_id, None, None, false)?,
            MailboxTarget::GroupIdle => Self::resolve_group_recipients(&conn, &req.run_id, None, None, true)?,
            MailboxTarget::GroupAgent { agent } => {
                Self::resolve_group_recipients(&conn, &req.run_id, Some(agent.as_str()), None, false)?
            }
            MailboxTarget::GroupWorktree { worktree_id } => Self::resolve_group_recipients(
                &conn,
                &req.run_id,
                None,
                Some(worktree_id.as_str()),
                false,
            )?,
        };

        let message_id = format!("msg-{}", Uuid::new_v4());
        let (target_kind, target_value) = target_to_cols(&req.target);
        let (route_kind, server_host_id) = route_to_cols(&req.route);
        let delivery_state = MailboxDeliveryState::Pending;

        conn.execute(
            "INSERT INTO messages
             (message_id, run_id, from_id, target_kind, target_value, message_type, priority,
              route_kind, server_host_id, body, dispatch_generation, delivery_state, created_at_ms)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13)",
            params![
                &message_id,
                &req.run_id,
                req.from_id.trim(),
                target_kind,
                &target_value,
                req.message_type.as_str(),
                priority_to_str(req.priority),
                route_kind,
                &server_host_id,
                &req.body,
                req.dispatch_generation.and_then(|g| i64::try_from(g).ok()),
                delivery_state_to_str(delivery_state),
                now_ms
            ],
        )?;

        let mut delivery_ids = Vec::with_capacity(recipients_with_gen.len());
        let mut recipients = Vec::with_capacity(recipients_with_gen.len());
        for (recipient_id, gen_opt) in recipients_with_gen {
            let delivery_id = format!("deliv-{}", Uuid::new_v4());
            conn.execute(
                "INSERT INTO deliveries
                 (delivery_id, message_id, recipient_id, generation, state, delivered_at_ms, acked_at_ms)
                 VALUES (?1, ?2, ?3, ?4, 'pending', NULL, NULL)",
                params![
                    &delivery_id,
                    &message_id,
                    &recipient_id,
                    gen_opt.and_then(|g| i64::try_from(g).ok())
                ],
            )?;
            delivery_ids.push(delivery_id);
            recipients.push(recipient_id);
        }

        Self::touch_coordinator_wake(&conn, &req.run_id, "mailbox_message", now_ms)?;
        drop(conn);

        if let Some(ref fu) = follow_up {
            self.emit_wake(OrchestrationWakeEvent::FollowUpReopened {
                run_id: req.run_id.clone(),
                original_dispatch_id: fu.original_dispatch_id.clone(),
                follow_up_task_id: fu.follow_up_task_id.clone(),
                follow_up_dispatch_id: fu.follow_up_dispatch_id.clone(),
                timestamp_ms: now_ms,
            });
        }
        self.emit_wake(OrchestrationWakeEvent::MailboxMessageArrived {
            run_id: req.run_id.clone(),
            message_id: message_id.clone(),
            message_type: req.message_type,
            priority: req.priority,
            recipients: recipients.clone(),
            timestamp_ms: now_ms,
        });

        let message = MailboxMessage {
            message_id,
            run_id: req.run_id,
            from_id: req.from_id.trim().to_string(),
            target: req.target,
            message_type: req.message_type,
            priority: req.priority,
            route: req.route,
            body: req.body,
            dispatch_generation: req.dispatch_generation,
            delivery_state,
            created_at_ms: now_ms,
        };
        Ok(SendMessageOutcome {
            message,
            delivery_ids,
            recipients,
            follow_up,
        })
    }

    fn maybe_reopen_finished_dispatch(
        conn: &Connection,
        run: &super::OrchestrationRunRecord,
        orig_dispatch_id: &str,
        now_ms: i64,
    ) -> Result<Option<(FollowUpReopenOutcome, u64)>> {
        let orig = conn
            .query_row(
                "SELECT w.status, w.host_id, c.task_id, c.agent_id, c.worktree_id, c.depth,
                        c.capability_hash, c.keep_open, t.title
                 FROM worker_dispatches w
                 JOIN dispatch_contexts c ON c.dispatch_id = w.dispatch_id
                 JOIN tasks t ON t.task_id = c.task_id
                 WHERE w.dispatch_id = ?1",
                params![orig_dispatch_id],
                |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, String>(2)?,
                        row.get::<_, String>(3)?,
                        row.get::<_, Option<String>>(4)?,
                        row.get::<_, i64>(5)?,
                        row.get::<_, String>(6)?,
                        row.get::<_, i64>(7)?,
                        row.get::<_, String>(8)?,
                    ))
                },
            )
            .optional()?;
        let Some((status_str, host_id, orig_task_id, agent_id, worktree_id, depth, cap_hash, keep_open, orig_title)) = orig else {
            return Ok(None);
        };
        if !matches!(status_str.as_str(), "completed" | "failed" | "circuit_broken") {
            return Ok(None);
        }

        let follow_up_task_id = format!("task-fu-{}", Uuid::new_v4());
        let follow_up_dispatch_id = format!("disp-fu-{}", Uuid::new_v4());
        let follow_up_title = format!("Follow-up: {orig_title}");
        let launch_token_hash = sha256_hex(follow_up_dispatch_id.as_bytes());

        conn.execute(
            "INSERT INTO tasks
             (task_id, run_id, title, deps, parent_id, status, assigned_host_id,
              assigned_dispatch_id, nesting_depth, created_at_ms, updated_at_ms)
             VALUES (?1, ?2, ?3, '[]', ?4, 'running', ?5, ?6, ?7, ?8, ?8)",
            params![
                &follow_up_task_id,
                &run.run_id,
                &follow_up_title,
                &orig_task_id,
                &host_id,
                &follow_up_dispatch_id,
                depth.max(1),
                now_ms
            ],
        )?;
        conn.execute(
            "INSERT INTO dispatch_contexts
             (dispatch_id, task_id, run_id, agent_id, worktree_id, depth, consumer_generation,
              capability_hash, launch_token_hash, follow_up_of_dispatch_id, keep_open,
              setup_failed, created_at_ms)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, 0, ?12)",
            params![
                &follow_up_dispatch_id,
                &follow_up_task_id,
                &run.run_id,
                &agent_id,
                &worktree_id,
                depth.max(1),
                i64::try_from(run.consumer_generation).unwrap_or(1),
                &cap_hash,
                &launch_token_hash,
                orig_dispatch_id,
                keep_open,
                now_ms
            ],
        )?;
        conn.execute(
            "INSERT INTO worker_dispatches
             (dispatch_id, task_id, host_id, generation, status, liveness, exit_code,
              is_idle, created_at_ms, updated_at_ms)
             VALUES (?1, ?2, ?3, 1, 'dispatched', 'alive', NULL, 0, ?4, ?4)",
            params![&follow_up_dispatch_id, &follow_up_task_id, &host_id, now_ms],
        )?;

        Ok(Some((
            FollowUpReopenOutcome {
                original_dispatch_id: orig_dispatch_id.to_string(),
                follow_up_task_id,
                follow_up_dispatch_id,
            },
            1,
        )))
    }
}

impl OrchestrationEngine {
    fn resolve_group_recipients(
        conn: &Connection,
        run_id: &str,
        agent_filter: Option<&str>,
        worktree_filter: Option<&str>,
        idle_only: bool,
    ) -> Result<Vec<(String, Option<u64>)>> {
        let mut stmt = conn.prepare(
            "SELECT w.dispatch_id, w.generation, c.agent_id, c.worktree_id, w.is_idle, w.status
             FROM worker_dispatches w
             JOIN dispatch_contexts c ON c.dispatch_id = w.dispatch_id
             WHERE c.run_id = ?1
               AND (w.status IN ('pending', 'dispatched')
                    OR (w.status = 'completed' AND c.keep_open = 1 AND w.is_idle = 1))
             ORDER BY w.dispatch_id ASC",
        )?;
        let rows = stmt
            .query_map(params![run_id], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, i64>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, Option<String>>(3)?,
                    row.get::<_, i64>(4)? != 0,
                    row.get::<_, String>(5)?,
                ))
            })?
            .collect::<std::result::Result<Vec<_>, _>>()?;

        let mut out = Vec::new();
        for (dispatch_id, gen_i64, agent_id, worktree_id, is_idle, status) in rows {
            if let Some(af) = agent_filter
                && agent_id != af
            {
                continue;
            }
            if let Some(wf) = worktree_filter
                && worktree_id.as_deref() != Some(wf)
            {
                continue;
            }
            if idle_only && !is_idle && status != "pending" {
                continue;
            }
            let gen_u64 = u64::try_from(gen_i64.max(1)).unwrap_or(1);
            out.push((format!("dispatch:{dispatch_id}"), Some(gen_u64)));
        }
        Ok(out)
    }

    /// Checks and claims mailbox deliveries for `recipient_id`, strictly verifying dispatch
    /// generation via `DispatchRegistry::verify_generation` when supplied (`UnknownDispatch` /
    /// `StaleDispatch`), fencing deliveries from older generations, and returning urgent/high
    /// messages first.
    pub fn check_mailbox(&self, req: CheckMailboxRequest) -> Result<Vec<DeliveryWithMessage>> {
        let now_ms = req.now_ms.unwrap_or_else(now_epoch_ms);
        let conn = self.conn.lock();

        let active_gen = if let Some(ref dispatch_id) = req.dispatch_id {
            let registry = Self::load_dispatch_registry(&conn)?;
            if let Some(expected_gen) = req.expected_generation {
                let rec = registry.verify_generation(dispatch_id, expected_gen)?;
                Some(rec.generation)
            } else {
                let rec = registry.records.get(dispatch_id).ok_or_else(|| {
                    OrcaIdeRuntimeError::ide(
                        OrcaIdeErrorCode::UnknownDispatch,
                        format!("unknown dispatch '{dispatch_id}'"),
                    )
                })?;
                Some(rec.generation)
            }
        } else {
            None
        };

        if let Some(curr_gen) = active_gen {
            conn.execute(
                "UPDATE deliveries SET state = 'fenced'
                 WHERE recipient_id = ?1 AND state IN ('pending', 'outstanding')
                   AND generation IS NOT NULL AND generation < ?2",
                params![&req.recipient_id, i64::try_from(curr_gen).unwrap_or(i64::MAX)],
            )?;
        }

        let mut stmt = conn.prepare(
            "SELECT d.delivery_id, d.recipient_id, d.generation, d.state, d.delivered_at_ms, d.acked_at_ms,
                    m.message_id, m.run_id, m.from_id, m.target_kind, m.target_value,
                    m.message_type, m.priority, m.route_kind, m.server_host_id,
                    m.body, m.dispatch_generation, m.delivery_state, m.created_at_ms
             FROM deliveries d
             JOIN messages m ON m.message_id = d.message_id
             WHERE d.recipient_id = ?1 AND d.state IN ('pending', 'outstanding')
             ORDER BY
               CASE m.priority WHEN 'urgent' THEN 0 WHEN 'high' THEN 1 ELSE 2 END ASC,
               m.created_at_ms ASC, d.delivery_id ASC",
        )?;

        let raw_rows = stmt
            .query_map(params![&req.recipient_id], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, Option<i64>>(2)?,
                    row.get::<_, String>(3)?,
                    row.get::<_, Option<i64>>(4)?,
                    row.get::<_, Option<i64>>(5)?,
                    row.get::<_, String>(6)?,
                    row.get::<_, String>(7)?,
                    row.get::<_, String>(8)?,
                    row.get::<_, String>(9)?,
                    row.get::<_, Option<String>>(10)?,
                    row.get::<_, String>(11)?,
                    row.get::<_, String>(12)?,
                    row.get::<_, String>(13)?,
                    row.get::<_, Option<String>>(14)?,
                    row.get::<_, String>(15)?,
                    row.get::<_, Option<i64>>(16)?,
                    row.get::<_, String>(17)?,
                    row.get::<_, i64>(18)?,
                ))
            })?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        drop(stmt);

        let mut out = Vec::with_capacity(raw_rows.len());
        for r in raw_rows {
            let mut state = str_to_delivery_state(&r.3)?;
            let mut delivered_at_ms = r.4;
            if state == MailboxDeliveryState::Pending {
                conn.execute(
                    "UPDATE deliveries SET state = 'outstanding', delivered_at_ms = ?1
                     WHERE delivery_id = ?2",
                    params![now_ms, &r.0],
                )?;
                state = MailboxDeliveryState::Outstanding;
                delivered_at_ms = Some(now_ms);
            }
            let message = MailboxMessage {
                message_id: r.6,
                run_id: r.7,
                from_id: r.8,
                target: cols_to_target(&r.9, r.10)?,
                message_type: str_to_message_type(&r.11)?,
                priority: str_to_priority(&r.12)?,
                route: cols_to_route(&r.13, r.14)?,
                body: r.15,
                dispatch_generation: r.16.and_then(|g| u64::try_from(g).ok()),
                delivery_state: str_to_delivery_state(&r.17)?,
                created_at_ms: r.18,
            };
            out.push(DeliveryWithMessage {
                delivery_id: r.0,
                recipient_id: r.1,
                generation: r.2.and_then(|g| u64::try_from(g).ok()),
                state,
                delivered_at_ms,
                acked_at_ms: r.5,
                message,
            });
        }
        Ok(out)
    }

    /// Acknowledges a mailbox delivery (with optional dispatch generation verification).
    pub fn ack_delivery(
        &self,
        delivery_id: &str,
        expected_generation: Option<u64>,
        now_ms: Option<i64>,
    ) -> Result<()> {
        let now_ms = now_ms.unwrap_or_else(now_epoch_ms);
        let conn = self.conn.lock();
        let row = conn
            .query_row(
                "SELECT message_id, recipient_id FROM deliveries WHERE delivery_id = ?1",
                params![delivery_id],
                |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)),
            )
            .optional()?
            .ok_or_else(|| {
                OrcaIdeRuntimeError::not_found(format!("unknown delivery '{delivery_id}'"))
            })?;

        if let Some(expected_gen) = expected_generation
            && let Some(dispatch_id) = row.1.strip_prefix("dispatch:")
        {
            let registry = Self::load_dispatch_registry(&conn)?;
            registry.verify_generation(dispatch_id, expected_gen)?;
        }

        conn.execute(
            "UPDATE deliveries SET state = 'acknowledged', acked_at_ms = ?1 WHERE delivery_id = ?2",
            params![now_ms, delivery_id],
        )?;

        let remaining: i64 = conn.query_row(
            "SELECT COUNT(*) FROM deliveries
             WHERE message_id = ?1 AND state IN ('pending', 'outstanding')",
            params![&row.0],
            |r| r.get(0),
        )?;
        if remaining == 0 {
            conn.execute(
                "UPDATE messages SET delivery_state = 'acknowledged' WHERE message_id = ?1",
                params![&row.0],
            )?;
        }
        Ok(())
    }

    /// Records a worker question in `questions` and wakes the coordinator.
    pub fn ask_question(
        &self,
        run_id: &str,
        dispatch_id: &str,
        prompt: &str,
        now_ms: Option<i64>,
    ) -> Result<OrchestrationQuestion> {
        if prompt.trim().is_empty() {
            return Err(OrcaIdeRuntimeError::invalid_argument(
                "question prompt cannot be empty",
            ));
        }
        let now_ms = now_ms.unwrap_or_else(now_epoch_ms);
        let question_id = format!("q-{}", Uuid::new_v4());
        let conn = self.conn.lock();
        let _run = Self::get_run_inner(&conn, run_id)?;
        conn.execute(
            "INSERT INTO questions
             (question_id, run_id, dispatch_id, prompt, status, answer, created_at_ms, answered_at_ms)
             VALUES (?1, ?2, ?3, ?4, 'pending', NULL, ?5, NULL)",
            params![&question_id, run_id, dispatch_id, prompt.trim(), now_ms],
        )?;
        Self::touch_coordinator_wake(&conn, run_id, "question_asked", now_ms)?;
        drop(conn);

        self.emit_wake(OrchestrationWakeEvent::QuestionUpdated {
            run_id: run_id.to_string(),
            question_id: question_id.clone(),
            answered: false,
            timestamp_ms: now_ms,
        });
        Ok(OrchestrationQuestion {
            question_id,
            run_id: run_id.to_string(),
            dispatch_id: dispatch_id.to_string(),
            prompt: prompt.trim().to_string(),
            status: "pending".to_string(),
            answer: None,
            created_at_ms: now_ms,
            answered_at_ms: None,
        })
    }

    /// Answers an open worker question and wakes the run.
    pub fn answer_question(
        &self,
        question_id: &str,
        answer: &str,
        now_ms: Option<i64>,
    ) -> Result<OrchestrationQuestion> {
        if answer.trim().is_empty() {
            return Err(OrcaIdeRuntimeError::invalid_argument(
                "question answer cannot be empty",
            ));
        }
        let now_ms = now_ms.unwrap_or_else(now_epoch_ms);
        let conn = self.conn.lock();
        let existing = conn
            .query_row(
                "SELECT run_id, dispatch_id, prompt, status, created_at_ms
                 FROM questions WHERE question_id = ?1",
                params![question_id],
                |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, String>(2)?,
                        row.get::<_, String>(3)?,
                        row.get::<_, i64>(4)?,
                    ))
                },
            )
            .optional()?
            .ok_or_else(|| {
                OrcaIdeRuntimeError::not_found(format!("unknown question '{question_id}'"))
            })?;

        if existing.3 != "pending" {
            return Err(OrcaIdeRuntimeError::conflict(format!(
                "question '{question_id}' is already {}",
                existing.3
            )));
        }

        conn.execute(
            "UPDATE questions SET status = 'answered', answer = ?1, answered_at_ms = ?2
             WHERE question_id = ?3",
            params![answer.trim(), now_ms, question_id],
        )?;
        Self::touch_coordinator_wake(&conn, &existing.0, "question_answered", now_ms)?;
        drop(conn);

        self.emit_wake(OrchestrationWakeEvent::QuestionUpdated {
            run_id: existing.0.clone(),
            question_id: question_id.to_string(),
            answered: true,
            timestamp_ms: now_ms,
        });

        Ok(OrchestrationQuestion {
            question_id: question_id.to_string(),
            run_id: existing.0,
            dispatch_id: existing.1,
            prompt: existing.2,
            status: "answered".to_string(),
            answer: Some(answer.trim().to_string()),
            created_at_ms: existing.4,
            answered_at_ms: Some(now_ms),
        })
    }
}
