//! SQLite WAL schema (upstream v51, 20 tables + execution_hosts) and lossless enum codecs
//! for `OrchestrationEngine`.

use orca_core::orchestration_protocol::{
    DispatchStatus, GateStatus, MailboxDeliveryState, MailboxMessageType, MailboxPriority,
    MailboxRoute, MailboxTarget, OrchestrationTaskStatus, WorkerLivenessState,
};
use rusqlite::Connection;

use crate::{OrcaIdeRuntimeError, Result};

/// Schema version matching upstream Orca orchestration store v51.
pub const ORCHESTRATION_SCHEMA_VERSION: u32 = 51;

const SCHEMA_SQL: &str = r#"
CREATE TABLE IF NOT EXISTS runs (
    run_id TEXT PRIMARY KEY,
    title TEXT NOT NULL,
    consumer_generation INTEGER NOT NULL DEFAULT 1,
    coordinator_orca_session_id TEXT,
    status TEXT NOT NULL,
    mailbox_is_local_only INTEGER NOT NULL DEFAULT 1,
    created_at_ms INTEGER NOT NULL,
    updated_at_ms INTEGER NOT NULL
);

CREATE TABLE IF NOT EXISTS coordinator_runs (
    run_id TEXT PRIMARY KEY REFERENCES runs(run_id) ON DELETE CASCADE,
    coordinator_session_id TEXT NOT NULL,
    wake_state TEXT NOT NULL,
    last_wake_at_ms INTEGER
);

CREATE TABLE IF NOT EXISTS tasks (
    task_id TEXT PRIMARY KEY,
    run_id TEXT NOT NULL REFERENCES runs(run_id) ON DELETE CASCADE,
    title TEXT NOT NULL,
    deps TEXT NOT NULL DEFAULT '[]',
    parent_id TEXT,
    status TEXT NOT NULL,
    assigned_host_id TEXT,
    assigned_dispatch_id TEXT,
    nesting_depth INTEGER NOT NULL DEFAULT 1,
    created_at_ms INTEGER NOT NULL,
    updated_at_ms INTEGER NOT NULL
);

CREATE INDEX IF NOT EXISTS idx_tasks_run_status ON tasks(run_id, status);

CREATE TABLE IF NOT EXISTS dispatch_contexts (
    dispatch_id TEXT PRIMARY KEY,
    task_id TEXT NOT NULL REFERENCES tasks(task_id) ON DELETE CASCADE,
    run_id TEXT NOT NULL REFERENCES runs(run_id) ON DELETE CASCADE,
    agent_id TEXT NOT NULL,
    worktree_id TEXT,
    depth INTEGER NOT NULL DEFAULT 1,
    consumer_generation INTEGER NOT NULL,
    capability_hash TEXT NOT NULL,
    launch_token_hash TEXT NOT NULL,
    follow_up_of_dispatch_id TEXT,
    keep_open INTEGER NOT NULL DEFAULT 0,
    setup_failed INTEGER NOT NULL DEFAULT 0,
    created_at_ms INTEGER NOT NULL
);

CREATE INDEX IF NOT EXISTS idx_dispatch_contexts_run ON dispatch_contexts(run_id);

CREATE TABLE IF NOT EXISTS worker_dispatches (
    dispatch_id TEXT PRIMARY KEY REFERENCES dispatch_contexts(dispatch_id) ON DELETE CASCADE,
    task_id TEXT NOT NULL REFERENCES tasks(task_id) ON DELETE CASCADE,
    host_id TEXT NOT NULL,
    generation INTEGER NOT NULL,
    status TEXT NOT NULL,
    liveness TEXT NOT NULL,
    exit_code INTEGER,
    is_idle INTEGER NOT NULL DEFAULT 0,
    created_at_ms INTEGER NOT NULL,
    updated_at_ms INTEGER NOT NULL
);

CREATE TABLE IF NOT EXISTS worker_terminal_resources (
    resource_id TEXT PRIMARY KEY,
    dispatch_id TEXT NOT NULL REFERENCES dispatch_contexts(dispatch_id) ON DELETE CASCADE,
    tab_id TEXT NOT NULL,
    pty_incarnation_id TEXT NOT NULL,
    owner_role TEXT NOT NULL,
    keep_open INTEGER NOT NULL DEFAULT 0,
    auto_release_at_ms INTEGER,
    released_at_ms INTEGER
);

CREATE INDEX IF NOT EXISTS idx_worker_term_res_dispatch ON worker_terminal_resources(dispatch_id);

CREATE TABLE IF NOT EXISTS worker_terminal_archives (
    archive_id TEXT PRIMARY KEY,
    dispatch_id TEXT NOT NULL REFERENCES dispatch_contexts(dispatch_id) ON DELETE CASCADE,
    transcript_text TEXT NOT NULL,
    archived_at_ms INTEGER NOT NULL
);

CREATE TABLE IF NOT EXISTS messages (
    message_id TEXT PRIMARY KEY,
    run_id TEXT NOT NULL REFERENCES runs(run_id) ON DELETE CASCADE,
    from_id TEXT NOT NULL,
    target_kind TEXT NOT NULL,
    target_value TEXT,
    message_type TEXT NOT NULL,
    priority TEXT NOT NULL,
    route_kind TEXT NOT NULL,
    server_host_id TEXT,
    body TEXT NOT NULL,
    dispatch_generation INTEGER,
    delivery_state TEXT NOT NULL,
    created_at_ms INTEGER NOT NULL
);

CREATE TABLE IF NOT EXISTS deliveries (
    delivery_id TEXT PRIMARY KEY,
    message_id TEXT NOT NULL REFERENCES messages(message_id) ON DELETE CASCADE,
    recipient_id TEXT NOT NULL,
    generation INTEGER,
    state TEXT NOT NULL,
    delivered_at_ms INTEGER,
    acked_at_ms INTEGER
);

CREATE INDEX IF NOT EXISTS idx_deliveries_recipient_state ON deliveries(recipient_id, state);

CREATE TABLE IF NOT EXISTS questions (
    question_id TEXT PRIMARY KEY,
    run_id TEXT NOT NULL REFERENCES runs(run_id) ON DELETE CASCADE,
    dispatch_id TEXT NOT NULL,
    prompt TEXT NOT NULL,
    status TEXT NOT NULL,
    answer TEXT,
    created_at_ms INTEGER NOT NULL,
    answered_at_ms INTEGER
);

CREATE TABLE IF NOT EXISTS decision_gates (
    gate_id TEXT PRIMARY KEY,
    run_id TEXT NOT NULL REFERENCES runs(run_id) ON DELETE CASCADE,
    task_id TEXT NOT NULL REFERENCES tasks(task_id) ON DELETE CASCADE,
    gate_kind TEXT NOT NULL,
    payload_json TEXT NOT NULL,
    status TEXT NOT NULL,
    deadline_ms INTEGER,
    resolved_at_ms INTEGER
);

CREATE TABLE IF NOT EXISTS mutation_receipts (
    caller_fingerprint TEXT NOT NULL,
    request_id TEXT NOT NULL,
    payload_hash TEXT NOT NULL,
    response_json TEXT NOT NULL,
    created_at_ms INTEGER NOT NULL,
    PRIMARY KEY (caller_fingerprint, request_id)
);

CREATE TABLE IF NOT EXISTS attempt_observation_facts (
    fact_id TEXT PRIMARY KEY,
    dispatch_id TEXT NOT NULL REFERENCES dispatch_contexts(dispatch_id) ON DELETE CASCADE,
    kind TEXT NOT NULL,
    payload_json TEXT NOT NULL,
    observed_at_ms INTEGER NOT NULL
);

CREATE TABLE IF NOT EXISTS execution_hosts (
    host_id TEXT PRIMARY KEY,
    display_name TEXT NOT NULL,
    capabilities_json TEXT NOT NULL DEFAULT '[]',
    online INTEGER NOT NULL DEFAULT 1,
    protocol_version INTEGER NOT NULL,
    active_workers INTEGER NOT NULL DEFAULT 0,
    max_workers INTEGER NOT NULL DEFAULT 4,
    last_heartbeat_ms INTEGER NOT NULL,
    supports_mail INTEGER NOT NULL DEFAULT 0
);
"#;

const FEDERATION_SCHEMA_SQL: &str = r#"
CREATE TABLE IF NOT EXISTS federated_dispatches (
    dispatch_id TEXT PRIMARY KEY,
    run_id TEXT NOT NULL,
    task_id TEXT NOT NULL,
    home_host_id TEXT NOT NULL,
    worker_host_id TEXT NOT NULL,
    depth INTEGER NOT NULL DEFAULT 1,
    home_auto_release_at_ms INTEGER,
    status TEXT NOT NULL,
    created_at_ms INTEGER NOT NULL,
    updated_at_ms INTEGER NOT NULL
);

CREATE TABLE IF NOT EXISTS remote_dispatch_attachments (
    attachment_id TEXT PRIMARY KEY,
    dispatch_id TEXT NOT NULL REFERENCES federated_dispatches(dispatch_id) ON DELETE CASCADE,
    kind TEXT NOT NULL,
    uri TEXT NOT NULL,
    sha256 TEXT,
    created_at_ms INTEGER NOT NULL
);

CREATE TABLE IF NOT EXISTS federation_relay_items (
    seq INTEGER PRIMARY KEY AUTOINCREMENT,
    direction TEXT NOT NULL,
    dispatch_id TEXT NOT NULL REFERENCES federated_dispatches(dispatch_id) ON DELETE CASCADE,
    payload_json TEXT NOT NULL,
    acked INTEGER NOT NULL DEFAULT 0,
    created_at_ms INTEGER NOT NULL,
    acked_at_ms INTEGER
);

CREATE INDEX IF NOT EXISTS idx_fed_relay_dispatch_acked
    ON federation_relay_items(dispatch_id, acked, seq);

CREATE TABLE IF NOT EXISTS remote_questions (
    question_id TEXT PRIMARY KEY,
    dispatch_id TEXT NOT NULL REFERENCES federated_dispatches(dispatch_id) ON DELETE CASCADE,
    home_host_id TEXT NOT NULL,
    worker_host_id TEXT NOT NULL,
    prompt TEXT NOT NULL,
    status TEXT NOT NULL,
    answer TEXT,
    created_at_ms INTEGER NOT NULL,
    answered_at_ms INTEGER
);

CREATE TABLE IF NOT EXISTS peer_mail_conversations (
    conversation_id TEXT PRIMARY KEY,
    local_host_id TEXT NOT NULL,
    remote_host_id TEXT NOT NULL,
    subject TEXT NOT NULL,
    created_at_ms INTEGER NOT NULL,
    updated_at_ms INTEGER NOT NULL
);

CREATE TABLE IF NOT EXISTS peer_mail_messages (
    message_id TEXT PRIMARY KEY,
    conversation_id TEXT NOT NULL REFERENCES peer_mail_conversations(conversation_id) ON DELETE CASCADE,
    from_host_id TEXT NOT NULL,
    to_host_id TEXT NOT NULL,
    body TEXT NOT NULL,
    created_at_ms INTEGER NOT NULL
);

CREATE TABLE IF NOT EXISTS peer_runs (
    peer_run_id TEXT PRIMARY KEY,
    home_host_id TEXT NOT NULL,
    remote_host_id TEXT NOT NULL,
    remote_run_id TEXT NOT NULL,
    status TEXT NOT NULL,
    updated_at_ms INTEGER NOT NULL
);
"#;

pub(crate) fn initialize_schema(conn: &Connection) -> Result<()> {
    conn.execute_batch(
        "PRAGMA journal_mode = WAL;
         PRAGMA foreign_keys = ON;
         PRAGMA busy_timeout = 5000;",
    )?;
    conn.execute_batch(SCHEMA_SQL)?;
    conn.execute_batch(FEDERATION_SCHEMA_SQL)?;
    conn.pragma_update(None, "user_version", ORCHESTRATION_SCHEMA_VERSION)?;
    Ok(())
}

pub(crate) fn task_status_to_str(status: OrchestrationTaskStatus) -> &'static str {
    match status {
        OrchestrationTaskStatus::Pending => "pending",
        OrchestrationTaskStatus::Ready => "ready",
        OrchestrationTaskStatus::Running => "running",
        OrchestrationTaskStatus::WaitingGate => "waiting_gate",
        OrchestrationTaskStatus::Succeeded => "succeeded",
        OrchestrationTaskStatus::Failed => "failed",
        OrchestrationTaskStatus::Canceled => "canceled",
    }
}

pub(crate) fn str_to_task_status(raw: &str) -> Result<OrchestrationTaskStatus> {
    match raw {
        "pending" => Ok(OrchestrationTaskStatus::Pending),
        "ready" => Ok(OrchestrationTaskStatus::Ready),
        "running" | "dispatched" => Ok(OrchestrationTaskStatus::Running),
        "waiting_gate" | "blocked" => Ok(OrchestrationTaskStatus::WaitingGate),
        "succeeded" | "completed" => Ok(OrchestrationTaskStatus::Succeeded),
        "failed" => Ok(OrchestrationTaskStatus::Failed),
        "canceled" => Ok(OrchestrationTaskStatus::Canceled),
        other => Err(OrcaIdeRuntimeError::invalid_argument(format!(
            "unknown task status '{other}'"
        ))),
    }
}

pub(crate) fn dispatch_status_to_str(status: DispatchStatus) -> &'static str {
    match status {
        DispatchStatus::Pending => "pending",
        DispatchStatus::Dispatched => "dispatched",
        DispatchStatus::Completed => "completed",
        DispatchStatus::Failed => "failed",
        DispatchStatus::CircuitBroken => "circuit_broken",
    }
}

pub(crate) fn str_to_dispatch_status(raw: &str) -> Result<DispatchStatus> {
    match raw {
        "pending" => Ok(DispatchStatus::Pending),
        "dispatched" => Ok(DispatchStatus::Dispatched),
        "completed" => Ok(DispatchStatus::Completed),
        "failed" => Ok(DispatchStatus::Failed),
        "circuit_broken" => Ok(DispatchStatus::CircuitBroken),
        other => Err(OrcaIdeRuntimeError::invalid_argument(format!(
            "unknown dispatch status '{other}'"
        ))),
    }
}

pub(crate) fn liveness_to_cols(liveness: WorkerLivenessState) -> (&'static str, Option<i32>) {
    match liveness {
        WorkerLivenessState::Alive => ("alive", None),
        WorkerLivenessState::Exited { exit_code } => ("exited", exit_code),
        WorkerLivenessState::Unverifiable => ("unverifiable", None),
    }
}

pub(crate) fn cols_to_liveness(raw: &str, exit_code: Option<i32>) -> Result<WorkerLivenessState> {
    match raw {
        "alive" => Ok(WorkerLivenessState::Alive),
        "exited" => Ok(WorkerLivenessState::Exited { exit_code }),
        "unverifiable" => Ok(WorkerLivenessState::Unverifiable),
        other => Err(OrcaIdeRuntimeError::invalid_argument(format!(
            "unknown worker liveness '{other}'"
        ))),
    }
}

pub(crate) fn priority_to_str(priority: MailboxPriority) -> &'static str {
    match priority {
        MailboxPriority::Normal => "normal",
        MailboxPriority::High => "high",
        MailboxPriority::Urgent => "urgent",
    }
}

pub(crate) fn str_to_priority(raw: &str) -> Result<MailboxPriority> {
    match raw {
        "normal" => Ok(MailboxPriority::Normal),
        "high" => Ok(MailboxPriority::High),
        "urgent" => Ok(MailboxPriority::Urgent),
        other => Err(OrcaIdeRuntimeError::invalid_argument(format!(
            "unknown mailbox priority '{other}'"
        ))),
    }
}

pub(crate) fn delivery_state_to_str(state: MailboxDeliveryState) -> &'static str {
    match state {
        MailboxDeliveryState::Pending => "pending",
        MailboxDeliveryState::Outstanding => "outstanding",
        MailboxDeliveryState::Acknowledged => "acknowledged",
        MailboxDeliveryState::Fenced => "fenced",
    }
}

pub(crate) fn str_to_delivery_state(raw: &str) -> Result<MailboxDeliveryState> {
    match raw {
        "pending" => Ok(MailboxDeliveryState::Pending),
        "outstanding" => Ok(MailboxDeliveryState::Outstanding),
        "acknowledged" => Ok(MailboxDeliveryState::Acknowledged),
        "fenced" => Ok(MailboxDeliveryState::Fenced),
        other => Err(OrcaIdeRuntimeError::invalid_argument(format!(
            "unknown mailbox delivery state '{other}'"
        ))),
    }
}

pub(crate) fn gate_status_to_str(status: GateStatus) -> &'static str {
    match status {
        GateStatus::Pending => "pending",
        GateStatus::Approved => "approved",
        GateStatus::Rejected => "rejected",
        GateStatus::TimedOut => "timed_out",
    }
}

pub(crate) fn str_to_gate_status(raw: &str) -> Result<GateStatus> {
    match raw {
        "pending" => Ok(GateStatus::Pending),
        "approved" | "resolved" => Ok(GateStatus::Approved),
        "rejected" => Ok(GateStatus::Rejected),
        "timed_out" => Ok(GateStatus::TimedOut),
        other => Err(OrcaIdeRuntimeError::invalid_argument(format!(
            "unknown gate status '{other}'"
        ))),
    }
}

pub(crate) fn target_to_cols(target: &MailboxTarget) -> (&'static str, Option<String>) {
    match target {
        MailboxTarget::Direct { recipient_id } => ("direct", Some(recipient_id.clone())),
        MailboxTarget::GroupAll => ("group_all", None),
        MailboxTarget::GroupIdle => ("group_idle", None),
        MailboxTarget::GroupAgent { agent } => ("group_agent", Some(agent.clone())),
        MailboxTarget::GroupWorktree { worktree_id } => {
            ("group_worktree", Some(worktree_id.clone()))
        }
    }
}

pub(crate) fn cols_to_target(kind: &str, value: Option<String>) -> Result<MailboxTarget> {
    match kind {
        "direct" => Ok(MailboxTarget::Direct {
            recipient_id: value.unwrap_or_default(),
        }),
        "group_all" => Ok(MailboxTarget::GroupAll),
        "group_idle" => Ok(MailboxTarget::GroupIdle),
        "group_agent" => Ok(MailboxTarget::GroupAgent {
            agent: value.unwrap_or_default(),
        }),
        "group_worktree" => Ok(MailboxTarget::GroupWorktree {
            worktree_id: value.unwrap_or_default(),
        }),
        other => Err(OrcaIdeRuntimeError::invalid_argument(format!(
            "unknown mailbox target kind '{other}'"
        ))),
    }
}

pub(crate) fn route_to_cols(route: &MailboxRoute) -> (&'static str, Option<String>) {
    match route {
        MailboxRoute::LocalOnly => ("local_only", None),
        MailboxRoute::ServerRelay { server_host_id } => {
            ("server_relay", Some(server_host_id.clone()))
        }
    }
}

pub(crate) fn cols_to_route(kind: &str, server_host_id: Option<String>) -> Result<MailboxRoute> {
    match kind {
        "local_only" => Ok(MailboxRoute::LocalOnly),
        "server_relay" => Ok(MailboxRoute::ServerRelay {
            server_host_id: server_host_id.unwrap_or_default(),
        }),
        other => Err(OrcaIdeRuntimeError::invalid_argument(format!(
            "unknown mailbox route kind '{other}'"
        ))),
    }
}

pub(crate) fn str_to_message_type(raw: &str) -> Result<MailboxMessageType> {
    MailboxMessageType::parse(raw).ok_or_else(|| {
        OrcaIdeRuntimeError::invalid_argument(format!("unknown mailbox message type '{raw}'"))
    })
}
