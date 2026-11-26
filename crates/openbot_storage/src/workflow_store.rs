//! Workflow persistence stores (`WorkflowStore`, `WorkflowRunStore`, `WorkflowGitHubStore`, `WebhookRouteStore`).
//! Faithful 1:1 port of `src/backend/workflows/{workflow-store,workflow-run-store,workflow-github-store}.ts`
//! and `src/backend/webhook-route-store.ts` over SQLite `openbot.db` schema `v69`.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::sync::Arc;

use chrono::{DateTime, TimeZone, Utc};
use rusqlite::{Connection, OptionalExtension, params, params_from_iter};
use serde::ser::SerializeMap;
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use uuid::Uuid;

use crate::database::OpenBotDatabase;
use crate::error::{StorageError, StorageResult};
use openbot_contracts::limits::{INPUT_LIMITS, is_bounded_string, is_identifier, utf16_len};
use openbot_contracts::routines::{
    EventRoutineOwner, RoutineSchedule, next_routine_occurrence, routine_once_instant,
};
use openbot_contracts::workflows::{
    WORKFLOW_AUTO_VERSION_INTERVAL_MS, WORKFLOW_FORMAT, WORKFLOW_GITHUB_ACTIONS,
    WORKFLOW_GITHUB_EVENTS, WORKFLOW_MAX_VERSIONS, WorkflowBranch, WorkflowDetail, WorkflowEdge,
    WorkflowFieldValue, WorkflowFields, WorkflowGraph, WorkflowLastRun, WorkflowNode,
    WorkflowNodeKind, WorkflowNodeSize, WorkflowPoint, WorkflowPortType, WorkflowRunSource,
    WorkflowRunStatus, WorkflowStepStatus, WorkflowStroke, WorkflowSummary, WorkflowTriggerState,
    WorkflowTriggerType, WorkflowVersionKind, known_workflow_field_value, workflow_graph_problem,
    workflow_node_registry,
};

pub const WORKFLOW_GITHUB_DELIVERY_LIMIT: usize = 50;
pub const WORKFLOW_GITHUB_OFFLINE_MERGE_MS: i64 = 2 * 60_000;
pub const WEBHOOK_RECEIPT_RETENTION_MS: i64 = 7 * 24 * 60 * 60 * 1_000;

pub(crate) fn now_millis() -> i64 {
    Utc::now().timestamp_millis()
}

pub(crate) fn ms_to_iso(ms: i64) -> String {
    Utc.timestamp_millis_opt(ms)
        .single()
        .unwrap_or_else(Utc::now)
        .to_rfc3339_opts(chrono::SecondsFormat::Millis, true)
}

pub(crate) fn parse_iso_ms(iso: &str) -> Option<i64> {
    DateTime::parse_from_rfc3339(iso)
        .ok()
        .map(|dt| dt.timestamp_millis())
        .or_else(|| {
            DateTime::parse_from_str(iso, "%Y-%m-%dT%H:%M:%S%.f%z")
                .ok()
                .map(|dt| dt.timestamp_millis())
        })
}

pub fn webhook_receipt_cutoff(received_at: &str) -> StorageResult<String> {
    let parsed = parse_iso_ms(received_at).ok_or_else(|| {
        StorageError::Validation(format!("invalid webhook receipt timestamp: {received_at}"))
    })?;
    Ok(ms_to_iso(parsed - WEBHOOK_RECEIPT_RETENTION_MS))
}

#[derive(Debug, Clone, PartialEq)]
pub enum WorkflowSaveResult {
    Saved { revision: u64 },
    Conflict { conflict: Box<WorkflowDetail> },
}

impl WorkflowSaveResult {
    pub fn into_result(self) -> StorageResult<u64> {
        match self {
            Self::Saved { revision } => Ok(revision),
            Self::Conflict { conflict } => Err(StorageError::Conflict(format!(
                "workflow {} revision conflict (current revision {})",
                conflict.summary.id, conflict.revision
            ))),
        }
    }
}

impl Serialize for WorkflowSaveResult {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let mut map = serializer.serialize_map(Some(2))?;
        match self {
            Self::Saved { revision } => {
                map.serialize_entry("ok", &true)?;
                map.serialize_entry("revision", revision)?;
            }
            Self::Conflict { conflict } => {
                map.serialize_entry("ok", &false)?;
                map.serialize_entry("conflict", conflict)?;
            }
        }
        map.end()
    }
}

impl<'de> Deserialize<'de> for WorkflowSaveResult {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let val = serde_json::Value::deserialize(deserializer)?;
        let Some(obj) = val.as_object() else {
            return Err(serde::de::Error::custom("WorkflowSaveResult must be an object"));
        };
        match obj.get("ok").and_then(serde_json::Value::as_bool) {
            Some(true) => {
                let rev = obj
                    .get("revision")
                    .and_then(serde_json::Value::as_u64)
                    .ok_or_else(|| serde::de::Error::custom("missing revision"))?;
                Ok(Self::Saved { revision: rev })
            }
            Some(false) => {
                let conflict_val = obj
                    .get("conflict")
                    .cloned()
                    .ok_or_else(|| serde::de::Error::custom("missing conflict detail"))?;
                let detail: WorkflowDetail =
                    serde_json::from_value(conflict_val).map_err(serde::de::Error::custom)?;
                Ok(Self::Conflict {
                    conflict: Box::new(detail),
                })
            }
            None => Err(serde::de::Error::custom("WorkflowSaveResult.ok must be bool")),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkflowListGraphEntry {
    pub summary: WorkflowSummary,
    pub graph: WorkflowGraph,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub needs_newer_app: Option<bool>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DueWorkflowTrigger {
    pub workflow_id: String,
    pub node_id: String,
    pub schedule: RoutineSchedule,
    pub timezone: Option<String>,
    pub due_at: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkflowVersion {
    pub id: String,
    pub at: String,
    pub kind: WorkflowVersionKind,
    pub revision: u64,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CreateWorkflowInput {
    pub title: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub graph: Option<WorkflowGraph>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub paused: Option<bool>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WorkflowValuesMode {
    New,
    Saved,
}

type IdGenerator = Arc<dyn Fn() -> String + Send + Sync>;
type NextDueFn = Arc<dyn Fn(&RoutineSchedule, Option<&str>, i64) -> Option<String> + Send + Sync>;

const NO_OPEN_RUN_SQL: &str = "NOT EXISTS (SELECT 1 FROM workflow_runs AS r \
    WHERE r.workflow_id = t.workflow_id AND r.status IN ('running', 'approval', 'stopping'))";

const SUMMARY_SELECT_SQL: &str = "\
  SELECT w.workflow_id, w.title, w.graph_revision, w.strokes_json, w.updated_at, w.paused,
    (SELECT COUNT(*) FROM workflow_nodes AS n
      WHERE n.workflow_id = w.workflow_id AND n.kind NOT IN ('text', 'note', 'shape')) AS steps,
    (SELECT r.status FROM workflow_runs AS r WHERE r.workflow_id = w.workflow_id
      ORDER BY r.started_at DESC, r.run_id DESC LIMIT 1) AS last_run_status,
    (SELECT COALESCE(r.ended_at, r.started_at) FROM workflow_runs AS r WHERE r.workflow_id = w.workflow_id
      ORDER BY r.started_at DESC, r.run_id DESC LIMIT 1) AS last_run_at
  FROM workflows AS w";

pub fn default_workflow_next_due(
    schedule: &RoutineSchedule,
    timezone: Option<&str>,
    now_ms: i64,
) -> Option<String> {
    let tz = timezone.filter(|s| !s.is_empty()).unwrap_or("UTC");
    match schedule {
        RoutineSchedule::Once { date, time } => {
            let due = routine_once_instant(date, time, tz).ok()?;
            (due > now_ms).then(|| ms_to_iso(due))
        }
        _ => next_routine_occurrence(schedule, tz, now_ms)
            .ok()
            .map(ms_to_iso),
    }
}

#[derive(Clone)]
pub struct WorkflowStore {
    db: OpenBotDatabase,
    create_id: IdGenerator,
    next_due_at: NextDueFn,
}

impl WorkflowStore {
    pub fn new(db: OpenBotDatabase) -> Self {
        Self {
            db,
            create_id: Arc::new(|| Uuid::new_v4().to_string()),
            next_due_at: Arc::new(default_workflow_next_due),
        }
    }

    pub fn with_options(
        db: OpenBotDatabase,
        create_id: IdGenerator,
        next_due_at: NextDueFn,
    ) -> Self {
        Self {
            db,
            create_id,
            next_due_at,
        }
    }

    pub fn list(&self) -> StorageResult<Vec<WorkflowSummary>> {
        self.db.with_conn(|conn| {
            let mut stmt = conn.prepare(&format!(
                "{SUMMARY_SELECT_SQL} ORDER BY w.updated_at DESC, w.workflow_id"
            ))?;
            let rows = stmt.query_map([], read_summary_row)?;
            let mut out = Vec::new();
            for row in rows {
                out.push(row?);
            }
            Ok(out)
        })
    }

    pub fn list_with_graphs(&self) -> StorageResult<Vec<WorkflowListGraphEntry>> {
        self.db.with_conn(|conn| {
            let mut sum_stmt = conn.prepare(&format!(
                "{SUMMARY_SELECT_SQL} ORDER BY w.updated_at DESC, w.workflow_id"
            ))?;
            let summaries: Vec<(WorkflowSummary, String)> = sum_stmt
                .query_map([], |row| {
                    let s = read_summary_row(row)?;
                    let strokes: String = row.get(3)?;
                    Ok((s, strokes))
                })?
                .collect::<rusqlite::Result<Vec<_>>>()?;

            let mut nodes_by_wf: HashMap<String, Vec<RawNodeRow>> = HashMap::new();
            let mut n_stmt = conn.prepare(
                "SELECT workflow_id, node_id, kind, x, y, width, height, font, locked, fields_json, schedule_json \
                 FROM workflow_nodes ORDER BY workflow_id, position, node_id",
            )?;
            let n_rows = n_stmt.query_map([], read_raw_node_row)?;
            for r in n_rows {
                let raw = r?;
                nodes_by_wf
                    .entry(raw.workflow_id.clone())
                    .or_default()
                    .push(raw);
            }

            let mut edges_by_wf: HashMap<String, Vec<RawEdgeRow>> = HashMap::new();
            let mut e_stmt = conn.prepare(
                "SELECT workflow_id, edge_id, from_node_id, to_node_id, port_type, branch \
                 FROM workflow_edges ORDER BY workflow_id, position, edge_id",
            )?;
            let e_rows = e_stmt.query_map([], read_raw_edge_row)?;
            for r in e_rows {
                let raw = r?;
                edges_by_wf
                    .entry(raw.workflow_id.clone())
                    .or_default()
                    .push(raw);
            }

            let mut out = Vec::with_capacity(summaries.len());
            for (summary, strokes_json) in summaries {
                let stored = build_stored_graph(
                    nodes_by_wf
                        .get(&summary.id)
                        .map_or(&[], Vec::as_slice),
                    edges_by_wf
                        .get(&summary.id)
                        .map_or(&[], Vec::as_slice),
                    &strokes_json,
                );
                out.push(WorkflowListGraphEntry {
                    summary,
                    graph: stored.graph,
                    needs_newer_app: stored.unreadable.then_some(true),
                });
            }
            Ok(out)
        })
    }

    pub fn workflows_of_agent(&self, agent_id: &str) -> StorageResult<Vec<String>> {
        self.db.with_conn(|conn| {
            let mut stmt = conn.prepare(
                "SELECT DISTINCT workflow_id FROM workflow_nodes WHERE agent_id = ?1 ORDER BY workflow_id",
            )?;
            let rows = stmt.query_map(params![agent_id], |r| r.get::<_, String>(0))?;
            Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
        })
    }

    pub fn detail(&self, workflow_id: &str) -> StorageResult<Option<WorkflowDetail>> {
        self.db.with_conn(|conn| self.detail_on_conn(conn, workflow_id))
    }

    fn detail_on_conn(
        &self,
        conn: &Connection,
        workflow_id: &str,
    ) -> StorageResult<Option<WorkflowDetail>> {
        let row = conn
            .query_row(
                &format!("{SUMMARY_SELECT_SQL} WHERE w.workflow_id = ?1"),
                params![workflow_id],
                |row| {
                    let summary = read_summary_row(row)?;
                    let revision: i64 = row.get(2)?;
                    let strokes_json: String = row.get(3)?;
                    Ok((summary, revision as u64, strokes_json))
                },
            )
            .optional()?;
        let Some((summary, revision, strokes_json)) = row else {
            return Ok(None);
        };
        let stored = read_graph_on_conn(conn, workflow_id, &strokes_json)?;
        let triggers = read_triggers_on_conn(conn, workflow_id)?;
        let format = workflow_graph_format(&stored.graph);
        Ok(Some(WorkflowDetail {
            summary,
            graph: stored.graph,
            revision,
            triggers,
            needs_newer_app: stored.unreadable.then_some(true),
            format: Some(format),
            host_format: Some(WORKFLOW_FORMAT),
        }))
    }

    pub fn create(
        &self,
        input: CreateWorkflowInput,
        now_ms: Option<i64>,
        values_mode: WorkflowValuesMode,
    ) -> StorageResult<WorkflowDetail> {
        let trimmed = input.title.trim();
        if trimmed.is_empty() || utf16_len(trimmed) > INPUT_LIMITS.workflow_title {
            return Err(StorageError::Validation("invalid workflow title".to_string()));
        }
        let graph = input.graph.unwrap_or_default();
        assert_runnable(&graph)?;
        let now = now_ms.unwrap_or_else(now_millis);
        self.db.with_tx(|conn| {
            if values_mode == WorkflowValuesMode::New {
                let saved = saved_nodes_by_ids(conn, &graph.nodes)?;
                assert_known_values(&graph, &saved)?;
            }
            let wf_id = (self.create_id)();
            let at = ms_to_iso(now);
            let paused_i = if input.paused.unwrap_or(false) { 1 } else { 0 };
            conn.execute(
                "INSERT INTO workflows (workflow_id, title, paused, created_at, updated_at) VALUES (?1, ?2, ?3, ?4, ?5)",
                params![wf_id, trimmed, paused_i, at, at],
            )?;
            self.write_graph_on_conn(conn, &wf_id, &graph, now)?;
            self.require_detail_on_conn(conn, &wf_id)
        })
    }

    pub fn rename(
        &self,
        workflow_id: &str,
        title: &str,
        now_ms: Option<i64>,
    ) -> StorageResult<WorkflowSummary> {
        let trimmed = title.trim();
        if trimmed.is_empty() || utf16_len(trimmed) > INPUT_LIMITS.workflow_title {
            return Err(StorageError::Validation("invalid workflow title".to_string()));
        }
        let at = ms_to_iso(now_ms.unwrap_or_else(now_millis));
        self.db.with_tx(|conn| {
            let changes = conn.execute(
                "UPDATE workflows SET title = ?1, updated_at = ?2 WHERE workflow_id = ?3",
                params![trimmed, at, workflow_id],
            )?;
            if changes == 0 {
                return Err(StorageError::NotFound(format!("workflow {workflow_id} missing")));
            }
            Ok(self.require_detail_on_conn(conn, workflow_id)?.summary)
        })
    }

    pub fn set_paused(
        &self,
        workflow_id: &str,
        paused: bool,
        now_ms: Option<i64>,
    ) -> StorageResult<WorkflowSummary> {
        let at = ms_to_iso(now_ms.unwrap_or_else(now_millis));
        self.db.with_tx(|conn| {
            let changes = conn.execute(
                "UPDATE workflows SET paused = ?1, updated_at = ?2 WHERE workflow_id = ?3",
                params![i64::from(paused), at, workflow_id],
            )?;
            if changes == 0 {
                return Err(StorageError::NotFound(format!("workflow {workflow_id} missing")));
            }
            Ok(self.require_detail_on_conn(conn, workflow_id)?.summary)
        })
    }

    pub fn remove(&self, workflow_id: &str, now_ms: Option<i64>) -> StorageResult<bool> {
        let at = ms_to_iso(now_ms.unwrap_or_else(now_millis));
        self.db.with_tx(|conn| {
            conn.execute(
                "INSERT OR IGNORE INTO projection_webhook_route_revocations (route_id, created_at) \
                 SELECT route_id, ?1 FROM workflow_trigger_webhooks WHERE workflow_id = ?2",
                params![at, workflow_id],
            )?;
            let changes = conn.execute(
                "DELETE FROM workflows WHERE workflow_id = ?1",
                params![workflow_id],
            )?;
            Ok(changes > 0)
        })
    }

    pub fn save_graph(
        &self,
        workflow_id: &str,
        base_revision: u64,
        graph: &WorkflowGraph,
        now_ms: Option<i64>,
    ) -> StorageResult<WorkflowSaveResult> {
        assert_runnable(graph)?;
        let now = now_ms.unwrap_or_else(now_millis);
        self.db.with_tx(|conn| {
            let current_rev = read_revision_on_conn(conn, workflow_id)?;
            if current_rev != base_revision {
                let detail = self.require_detail_on_conn(conn, workflow_id)?;
                return Ok(WorkflowSaveResult::Conflict {
                    conflict: Box::new(detail),
                });
            }
            let saved = self.require_detail_on_conn(conn, workflow_id)?;
            refuse_placeholders_on_conn(conn, workflow_id, &saved, graph)?;
            assert_known_values(graph, &saved.graph.nodes)?;
            self.write_graph_on_conn(conn, workflow_id, graph, now)?;
            let next_rev = bump_revision_on_conn(conn, workflow_id, now)?;
            let last_at = last_version_ms_on_conn(conn, workflow_id)?;
            if last_at.is_none_or(|prev| now - prev >= WORKFLOW_AUTO_VERSION_INTERVAL_MS as i64) {
                self.add_version_on_conn(
                    conn,
                    workflow_id,
                    next_rev,
                    WorkflowVersionKind::Auto,
                    graph,
                    now,
                )?;
            }
            Ok(WorkflowSaveResult::Saved { revision: next_rev })
        })
    }

    pub fn save_graph_strict(
        &self,
        workflow_id: &str,
        base_revision: u64,
        graph: &WorkflowGraph,
        now_ms: Option<i64>,
    ) -> StorageResult<u64> {
        self.save_graph(workflow_id, base_revision, graph, now_ms)?
            .into_result()
    }

    pub fn next_trigger_due_at(&self) -> StorageResult<Option<String>> {
        self.db.with_conn(|conn| {
            let due: Option<Option<String>> = conn
                .query_row(
                    &format!(
                        "SELECT MIN(t.next_due_at) AS due FROM workflow_triggers AS t \
                         WHERE t.kind = 'schedule' AND t.next_due_at IS NOT NULL AND {NO_OPEN_RUN_SQL}"
                    ),
                    [],
                    |r| r.get(0),
                )
                .optional()?;
            Ok(due.flatten())
        })
    }

    pub fn due_triggers(&self, now_ms: i64) -> StorageResult<Vec<DueWorkflowTrigger>> {
        let now_iso = ms_to_iso(now_ms);
        self.db.with_conn(|conn| {
            let mut stmt = conn.prepare(&format!(
                "SELECT t.workflow_id, t.node_id, t.schedule_json, t.timezone, t.next_due_at \
                 FROM workflow_triggers AS t \
                 WHERE t.kind = 'schedule' AND t.next_due_at IS NOT NULL AND t.next_due_at <= ?1 AND {NO_OPEN_RUN_SQL} \
                 ORDER BY t.next_due_at, t.workflow_id, t.node_id"
            ))?;
            let rows = stmt.query_map(params![now_iso], |row| {
                let wf_id: String = row.get(0)?;
                let node_id: String = row.get(1)?;
                let sched_json: Option<String> = row.get(2)?;
                let timezone: Option<String> = row.get(3)?;
                let due_at: String = row.get(4)?;
                Ok((wf_id, node_id, sched_json, timezone, due_at))
            })?;
            let mut out = Vec::new();
            for r in rows {
                let (workflow_id, node_id, sched_json, timezone, due_at) = r?;
                if let Some(raw) = sched_json
                    && let Ok(schedule) = serde_json::from_str::<RoutineSchedule>(&raw)
                    && schedule.is_valid()
                {
                    out.push(DueWorkflowTrigger {
                        workflow_id,
                        node_id,
                        schedule,
                        timezone,
                        due_at,
                    });
                }
            }
            Ok(out)
        })
    }

    pub fn advance_trigger(
        &self,
        due: &DueWorkflowTrigger,
        next_due_at: Option<&str>,
        now_ms: Option<i64>,
    ) -> StorageResult<bool> {
        let at = ms_to_iso(now_ms.unwrap_or_else(now_millis));
        self.db.with_conn(|conn| {
            let changes = conn.execute(
                "UPDATE workflow_triggers SET next_due_at = ?1, updated_at = ?2 \
                 WHERE workflow_id = ?3 AND node_id = ?4 AND next_due_at = ?5",
                params![next_due_at, at, due.workflow_id, due.node_id, due.due_at],
            )?;
            Ok(changes > 0)
        })
    }

    pub fn versions(&self, workflow_id: &str) -> StorageResult<Vec<WorkflowVersion>> {
        self.db.with_conn(|conn| {
            let mut stmt = conn.prepare(
                "SELECT version_id, graph_revision, kind, created_at FROM workflow_versions \
                 WHERE workflow_id = ?1 ORDER BY created_at DESC, version_id DESC",
            )?;
            let rows = stmt.query_map(params![workflow_id], |row| {
                let id: String = row.get(0)?;
                let rev: i64 = row.get(1)?;
                let kind_str: String = row.get(2)?;
                let at: String = row.get(3)?;
                Ok((id, rev as u64, kind_str, at))
            })?;
            let mut out = Vec::new();
            for r in rows {
                let (id, revision, kind_str, at) = r?;
                let kind = parse_version_kind(&kind_str)?;
                out.push(WorkflowVersion {
                    id,
                    at,
                    kind,
                    revision,
                });
            }
            Ok(out)
        })
    }

    pub fn version_graph(
        &self,
        workflow_id: &str,
        version_id: &str,
    ) -> StorageResult<Option<WorkflowGraph>> {
        self.db.with_conn(|conn| {
            let raw: Option<String> = conn
                .query_row(
                    "SELECT graph_json FROM workflow_versions WHERE workflow_id = ?1 AND version_id = ?2",
                    params![workflow_id, version_id],
                    |r| r.get(0),
                )
                .optional()?;
            raw.map(|json| parse_stored_graph_strict(&json)).transpose()
        })
    }

    pub fn restore_version(
        &self,
        workflow_id: &str,
        version_id: &str,
        now_ms: Option<i64>,
    ) -> StorageResult<WorkflowDetail> {
        let now = now_ms.unwrap_or_else(now_millis);
        self.db.with_tx(|conn| {
            let raw: Option<String> = conn
                .query_row(
                    "SELECT graph_json FROM workflow_versions WHERE workflow_id = ?1 AND version_id = ?2",
                    params![workflow_id, version_id],
                    |r| r.get(0),
                )
                .optional()?;
            let Some(json) = raw else {
                return Err(StorageError::NotFound(format!(
                    "workflow version {version_id} missing"
                )));
            };
            let graph = parse_stored_graph_strict(&json)?;
            assert_runnable(&graph)?;
            let current = self.require_detail_on_conn(conn, workflow_id)?;
            if current.needs_newer_app == Some(true) {
                return Err(StorageError::Validation(
                    "workflow uses steps from a newer app version".to_string(),
                ));
            }
            self.add_version_on_conn(
                conn,
                workflow_id,
                current.revision,
                WorkflowVersionKind::Restore,
                &current.graph,
                now,
            )?;
            self.write_graph_on_conn(conn, workflow_id, &graph, now)?;
            bump_revision_on_conn(conn, workflow_id, now)?;
            self.require_detail_on_conn(conn, workflow_id)
        })
    }

    pub fn add_run_version(
        &self,
        workflow_id: &str,
        revision: u64,
        graph: &WorkflowGraph,
        now_ms: Option<i64>,
    ) -> StorageResult<()> {
        let now = now_ms.unwrap_or_else(now_millis);
        self.db.with_tx(|conn| {
            let newest: Option<i64> = conn
                .query_row(
                    "SELECT graph_revision FROM workflow_versions WHERE workflow_id = ?1 \
                     ORDER BY created_at DESC, version_id DESC LIMIT 1",
                    params![workflow_id],
                    |r| r.get(0),
                )
                .optional()?;
            if newest == Some(revision as i64) {
                return Ok(());
            }
            self.add_version_on_conn(
                conn,
                workflow_id,
                revision,
                WorkflowVersionKind::Run,
                graph,
                now,
            )
        })
    }

    fn require_detail_on_conn(
        &self,
        conn: &Connection,
        workflow_id: &str,
    ) -> StorageResult<WorkflowDetail> {
        self.detail_on_conn(conn, workflow_id)?
            .ok_or_else(|| StorageError::NotFound(format!("workflow {workflow_id} missing")))
    }

    fn add_version_on_conn(
        &self,
        conn: &Connection,
        workflow_id: &str,
        revision: u64,
        kind: WorkflowVersionKind,
        graph: &WorkflowGraph,
        now_ms: i64,
    ) -> StorageResult<()> {
        let vid = (self.create_id)();
        let at = ms_to_iso(now_ms);
        let graph_json = serde_json::to_string(graph)?;
        conn.execute(
            "INSERT INTO workflow_versions (version_id, workflow_id, graph_revision, kind, graph_json, created_at) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![vid, workflow_id, revision as i64, version_kind_str(kind), graph_json, at],
        )?;
        conn.execute(
            "DELETE FROM workflow_versions WHERE workflow_id = ?1 AND version_id NOT IN ( \
               SELECT version_id FROM workflow_versions WHERE workflow_id = ?2 \
               ORDER BY created_at DESC, version_id DESC LIMIT ?3)",
            params![workflow_id, workflow_id, WORKFLOW_MAX_VERSIONS as i64],
        )?;
        Ok(())
    }

    fn write_graph_on_conn(
        &self,
        conn: &Connection,
        workflow_id: &str,
        graph: &WorkflowGraph,
        now_ms: i64,
    ) -> StorageResult<()> {
        let at = ms_to_iso(now_ms);
        let kept: HashSet<&str> = graph.nodes.iter().map(|n| n.id.as_str()).collect();
        let desired: Vec<(&WorkflowNode, &'static str)> = graph
            .nodes
            .iter()
            .filter_map(|n| stored_trigger_kind(n).map(|k| (n, k)))
            .collect();
        let desired_map: HashMap<&str, (&WorkflowNode, &'static str)> = desired
            .iter()
            .map(|&(n, k)| (n.id.as_str(), (n, k)))
            .collect();

        let existing_triggers = {
            let mut stmt = conn.prepare(
                "SELECT node_id, kind, schedule_json, timezone FROM workflow_triggers WHERE workflow_id = ?1",
            )?;
            stmt.query_map(params![workflow_id], |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    r.get::<_, String>(1)?,
                    r.get::<_, Option<String>>(2)?,
                    r.get::<_, Option<String>>(3)?,
                ))
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?
        };

        for (node_id, old_kind, _, _) in &existing_triggers {
            let next_kind = desired_map.get(node_id.as_str()).map(|&(_, k)| k);
            if old_kind == "webhook" && next_kind != Some("webhook") {
                conn.execute(
                    "INSERT OR IGNORE INTO projection_webhook_route_revocations (route_id, created_at) \
                     SELECT route_id, ?1 FROM workflow_trigger_webhooks WHERE workflow_id = ?2 AND node_id = ?3",
                    params![at, workflow_id, node_id],
                )?;
                conn.execute(
                    "DELETE FROM workflow_trigger_webhooks WHERE workflow_id = ?1 AND node_id = ?2",
                    params![workflow_id, node_id],
                )?;
            }
            if next_kind.is_none() {
                conn.execute(
                    "DELETE FROM workflow_triggers WHERE workflow_id = ?1 AND node_id = ?2",
                    params![workflow_id, node_id],
                )?;
            }
        }

        let existing_nodes: Vec<String> = {
            let mut stmt =
                conn.prepare("SELECT node_id FROM workflow_nodes WHERE workflow_id = ?1")?;
            stmt.query_map(params![workflow_id], |r| r.get(0))?
                .collect::<rusqlite::Result<Vec<_>>>()?
        };
        for node_id in existing_nodes {
            if !kept.contains(node_id.as_str()) {
                conn.execute(
                    "DELETE FROM workflow_nodes WHERE workflow_id = ?1 AND node_id = ?2",
                    params![workflow_id, node_id],
                )?;
            }
        }
        conn.execute(
            "DELETE FROM workflow_edges WHERE workflow_id = ?1",
            params![workflow_id],
        )?;
        self.upsert_nodes_edges_triggers(conn, workflow_id, graph, &desired, &existing_triggers, now_ms, &at)
    }

    #[allow(clippy::too_many_arguments)]
    fn upsert_nodes_edges_triggers(
        &self,
        conn: &Connection,
        workflow_id: &str,
        graph: &WorkflowGraph,
        desired: &[(&WorkflowNode, &'static str)],
        existing_triggers: &[(String, String, Option<String>, Option<String>)],
        now_ms: i64,
        at: &str,
    ) -> StorageResult<()> {
        let mut upsert_node = conn.prepare(
            "INSERT INTO workflow_nodes (workflow_id, node_id, position, kind, agent_id, x, y, width, height, font, locked, \
               fields_json, schedule_json) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13) \
             ON CONFLICT(workflow_id, node_id) DO UPDATE SET position = excluded.position, kind = excluded.kind, \
               agent_id = excluded.agent_id, x = excluded.x, y = excluded.y, width = excluded.width, \
               height = excluded.height, font = excluded.font, locked = excluded.locked, \
               fields_json = excluded.fields_json, schedule_json = excluded.schedule_json",
        )?;
        for (pos, node) in graph.nodes.iter().enumerate() {
            let fields_json = serde_json::to_string(&node.fields)?;
            let sched_json = node
                .schedule
                .as_ref()
                .map(serde_json::to_string)
                .transpose()?;
            upsert_node.execute(params![
                workflow_id,
                node.id,
                pos as i64,
                node.kind.as_str(),
                node.agent_id(),
                node.at.x,
                node.at.y,
                node.size.and_then(|s| s.width),
                node.size.and_then(|s| s.height),
                node.size.and_then(|s| s.font),
                i64::from(node.locked.unwrap_or(false)),
                fields_json,
                sched_json,
            ])?;
        }

        let mut insert_edge = conn.prepare(
            "INSERT INTO workflow_edges (workflow_id, edge_id, position, from_node_id, to_node_id, port_type, branch) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
        )?;
        for (pos, edge) in graph.edges.iter().enumerate() {
            insert_edge.execute(params![
                workflow_id,
                edge.id,
                pos as i64,
                edge.from,
                edge.to,
                edge.port_type.as_str(),
                edge.branch.map(WorkflowBranch::as_str),
            ])?;
        }

        let prev_map: HashMap<&str, (&str, Option<&str>, Option<&str>)> = existing_triggers
            .iter()
            .map(|(nid, k, sj, tz)| (nid.as_str(), (k.as_str(), sj.as_deref(), tz.as_deref())))
            .collect();

        let mut upsert_trig = conn.prepare(
            "INSERT INTO workflow_triggers (workflow_id, node_id, kind, timezone, schedule_json, next_due_at, updated_at) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7) \
             ON CONFLICT(workflow_id, node_id) DO UPDATE SET kind = excluded.kind, timezone = excluded.timezone, \
               schedule_json = excluded.schedule_json, next_due_at = excluded.next_due_at, updated_at = excluded.updated_at",
        )?;
        for &(node, kind) in desired {
            let tz = node_timezone(node);
            let sched_json = if kind == "schedule" {
                node.schedule
                    .as_ref()
                    .map(serde_json::to_string)
                    .transpose()?
            } else {
                None
            };
            if let Some(&(old_k, old_sj, old_tz)) = prev_map.get(node.id.as_str())
                && old_k == kind
                && old_sj == sched_json.as_deref()
                && old_tz == tz
            {
                continue;
            }
            let next_due = if kind == "schedule" {
                node.schedule
                    .as_ref()
                    .and_then(|s| (self.next_due_at)(s, tz, now_ms))
            } else {
                None
            };
            upsert_trig.execute(params![
                workflow_id,
                node.id,
                kind,
                tz,
                sched_json,
                next_due,
                at
            ])?;
        }

        let strokes_json = serde_json::to_string(&graph.strokes)?;
        conn.execute(
            "UPDATE workflows SET strokes_json = ?1, updated_at = ?2 WHERE workflow_id = ?3",
            params![strokes_json, at, workflow_id],
        )?;
        Ok(())
    }
}

pub fn node_timezone(node: &WorkflowNode) -> Option<&str> {
    match node.fields.get("timezone")? {
        WorkflowFieldValue::Text(s) if !s.is_empty() => Some(s.as_str()),
        _ => None,
    }
}

pub fn runtime_fields_of_node(node: &WorkflowNode) -> WorkflowFields {
    let mut out = BTreeMap::new();
    for spec in workflow_node_registry(node.kind) {
        if !spec.runtime {
            continue;
        }
        if let Some(val) = node.fields.get(spec.name) {
            if matches!(val, WorkflowFieldValue::Text(s) if s.is_empty()) {
                continue;
            }
            out.insert(spec.name.to_string(), val.clone());
        }
    }
    out
}

fn stored_trigger_kind(node: &WorkflowNode) -> Option<&'static str> {
    if node.kind != WorkflowNodeKind::Trigger {
        return None;
    }
    match node.fields.get("trigger") {
        None => Some("manual"),
        Some(WorkflowFieldValue::Text(s)) => match s.as_str() {
            "manual" => Some("manual"),
            "schedule" => Some("schedule"),
            "webhook" => Some("webhook"),
            _ => None,
        },
        _ => None,
    }
}

fn assert_runnable(graph: &WorkflowGraph) -> StorageResult<()> {
    graph
        .validate()
        .map_err(|e| StorageError::Validation(e.to_string()))?;
    if let Some(prob) = workflow_graph_problem(graph) {
        return Err(StorageError::Validation(format!(
            "workflow graph validation failed: {prob:?}"
        )));
    }
    Ok(())
}

fn assert_known_values(graph: &WorkflowGraph, saved_nodes: &[WorkflowNode]) -> StorageResult<()> {
    for node in &graph.nodes {
        for spec in workflow_node_registry(node.kind) {
            if let Some(WorkflowFieldValue::Text(s)) = node.fields.get(spec.name)
                && !known_workflow_field_value(spec, s)
            {
                let preserved = saved_nodes.iter().any(|old| {
                    old.id == node.id
                        && old.kind == node.kind
                        && old.fields.get(spec.name) == node.fields.get(spec.name)
                });
                if !preserved {
                    return Err(StorageError::Validation(format!(
                        "workflow node {} has unknown value for {}",
                        node.id, spec.name
                    )));
                }
            }
        }
    }
    Ok(())
}

fn refuse_placeholders_on_conn(
    conn: &Connection,
    workflow_id: &str,
    saved: &WorkflowDetail,
    graph: &WorkflowGraph,
) -> StorageResult<()> {
    if saved.needs_newer_app == Some(true) {
        return Err(StorageError::Validation(
            "workflow uses steps from a newer app version".to_string(),
        ));
    }
    let mut stmt =
        conn.prepare("SELECT node_id, kind FROM workflow_nodes WHERE workflow_id = ?1")?;
    let stored: HashMap<String, String> = stmt
        .query_map(params![workflow_id], |r| {
            Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?))
        })?
        .collect::<rusqlite::Result<HashMap<_, _>>>()?;

    let has_placeholder = graph.nodes.iter().any(|node| {
        node.kind == WorkflowNodeKind::Note
            && node.locked == Some(true)
            && stored
                .get(&node.id)
                .map_or("note", String::as_str)
                != "note"
    });
    if has_placeholder {
        return Err(StorageError::Validation(
            "workflow uses steps from a newer app version".to_string(),
        ));
    }
    Ok(())
}

fn saved_nodes_by_ids(
    conn: &Connection,
    nodes: &[WorkflowNode],
) -> StorageResult<Vec<WorkflowNode>> {
    let mut unique: Vec<&str> = Vec::new();
    let mut seen = HashSet::new();
    for n in nodes {
        if seen.insert(n.id.as_str()) {
            unique.push(n.id.as_str());
        }
    }
    if unique.is_empty() {
        return Ok(Vec::new());
    }
    let placeholders = std::iter::repeat_n("?", unique.len())
        .collect::<Vec<_>>()
        .join(", ");
    let mut stmt = conn.prepare(&format!(
        "SELECT workflow_id, node_id, kind, x, y, width, height, font, locked, fields_json, schedule_json \
         FROM workflow_nodes WHERE node_id IN ({placeholders})"
    ))?;
    let rows = stmt.query_map(params_from_iter(unique), read_raw_node_row)?;
    let mut out = Vec::new();
    for r in rows {
        if let Some(node) = decode_raw_node(&r?) {
            out.push(node);
        }
    }
    Ok(out)
}

fn read_revision_on_conn(conn: &Connection, workflow_id: &str) -> StorageResult<u64> {
    let rev: Option<i64> = conn
        .query_row(
            "SELECT graph_revision FROM workflows WHERE workflow_id = ?1",
            params![workflow_id],
            |r| r.get(0),
        )
        .optional()?;
    rev.map(|v| v as u64)
        .ok_or_else(|| StorageError::NotFound(format!("workflow {workflow_id} missing")))
}

fn bump_revision_on_conn(conn: &Connection, workflow_id: &str, now_ms: i64) -> StorageResult<u64> {
    let at = ms_to_iso(now_ms);
    conn.execute(
        "UPDATE workflows SET graph_revision = graph_revision + 1, updated_at = ?1 WHERE workflow_id = ?2",
        params![at, workflow_id],
    )?;
    read_revision_on_conn(conn, workflow_id)
}

fn last_version_ms_on_conn(conn: &Connection, workflow_id: &str) -> StorageResult<Option<i64>> {
    let created_at: Option<Option<String>> = conn
        .query_row(
            "SELECT MAX(created_at) FROM workflow_versions WHERE workflow_id = ?1",
            params![workflow_id],
            |r| r.get(0),
        )
        .optional()?;
    Ok(created_at.flatten().and_then(|s| parse_iso_ms(&s)))
}

fn parse_stored_graph_strict(json: &str) -> StorageResult<WorkflowGraph> {
    let graph: WorkflowGraph = serde_json::from_str(json)?;
    graph
        .validate()
        .map_err(|e| StorageError::Validation(format!("stored workflow graph invalid: {e}")))?;
    Ok(graph)
}

fn version_kind_str(kind: WorkflowVersionKind) -> &'static str {
    match kind {
        WorkflowVersionKind::Auto => "auto",
        WorkflowVersionKind::Restore => "restore",
        WorkflowVersionKind::Run => "run",
    }
}

fn parse_version_kind(raw: &str) -> StorageResult<WorkflowVersionKind> {
    match raw {
        "auto" => Ok(WorkflowVersionKind::Auto),
        "restore" => Ok(WorkflowVersionKind::Restore),
        "run" => Ok(WorkflowVersionKind::Run),
        _ => Err(StorageError::Validation(format!(
            "invalid workflow version kind: {raw}"
        ))),
    }
}

struct RawNodeRow {
    workflow_id: String,
    node_id: String,
    kind: String,
    x: f64,
    y: f64,
    width: Option<f64>,
    height: Option<f64>,
    font: Option<f64>,
    locked: i64,
    fields_json: String,
    schedule_json: Option<String>,
}

struct RawEdgeRow {
    workflow_id: String,
    edge_id: String,
    from_node_id: String,
    to_node_id: String,
    port_type: String,
    branch: Option<String>,
}

struct StoredGraph {
    graph: WorkflowGraph,
    unreadable: bool,
}

fn read_raw_node_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<RawNodeRow> {
    Ok(RawNodeRow {
        workflow_id: row.get(0)?,
        node_id: row.get(1)?,
        kind: row.get(2)?,
        x: row.get(3)?,
        y: row.get(4)?,
        width: row.get(5)?,
        height: row.get(6)?,
        font: row.get(7)?,
        locked: row.get(8)?,
        fields_json: row.get(9)?,
        schedule_json: row.get(10)?,
    })
}

fn read_raw_edge_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<RawEdgeRow> {
    Ok(RawEdgeRow {
        workflow_id: row.get(0)?,
        edge_id: row.get(1)?,
        from_node_id: row.get(2)?,
        to_node_id: row.get(3)?,
        port_type: row.get(4)?,
        branch: row.get(5)?,
    })
}

fn decode_raw_node(raw: &RawNodeRow) -> Option<WorkflowNode> {
    let kind = WorkflowNodeKind::ALL
        .into_iter()
        .find(|k| k.as_str() == raw.kind)?;
    let fields: WorkflowFields = serde_json::from_str(&raw.fields_json).ok()?;
    let schedule = match &raw.schedule_json {
        None => None,
        Some(s) => {
            let parsed: RoutineSchedule = serde_json::from_str(s).ok()?;
            if !parsed.is_valid() {
                return None;
            }
            Some(parsed)
        }
    };
    let size = if raw.width.is_none() && raw.height.is_none() && raw.font.is_none() {
        None
    } else {
        Some(WorkflowNodeSize {
            width: raw.width,
            height: raw.height,
            font: raw.font,
        })
    };
    let node = WorkflowNode {
        id: raw.node_id.clone(),
        kind,
        at: WorkflowPoint { x: raw.x, y: raw.y },
        size,
        locked: (raw.locked == 1).then_some(true),
        fields,
        schedule,
    };
    node.is_valid().then_some(node)
}

fn placeholder_node(raw: &RawNodeRow) -> Option<WorkflowNode> {
    let at = if raw.x.is_finite() && raw.y.is_finite() {
        WorkflowPoint { x: raw.x, y: raw.y }
    } else {
        WorkflowPoint { x: 0.0, y: 0.0 }
    };
    let node = WorkflowNode {
        id: raw.node_id.clone(),
        kind: WorkflowNodeKind::Note,
        at,
        size: None,
        locked: Some(true),
        fields: BTreeMap::new(),
        schedule: None,
    };
    node.is_valid().then_some(node)
}

fn decode_raw_edge(raw: &RawEdgeRow) -> Option<WorkflowEdge> {
    let port_type = WorkflowPortType::ALL
        .into_iter()
        .find(|p| p.as_str() == raw.port_type)?;
    let branch = match raw.branch.as_deref() {
        None => None,
        Some("yes") => Some(WorkflowBranch::Yes),
        Some("no") => Some(WorkflowBranch::No),
        Some(_) => return None,
    };
    let edge = WorkflowEdge {
        id: raw.edge_id.clone(),
        from: raw.from_node_id.clone(),
        to: raw.to_node_id.clone(),
        port_type,
        branch,
    };
    edge.is_valid().then_some(edge)
}

fn build_stored_graph(
    node_rows: &[RawNodeRow],
    edge_rows: &[RawEdgeRow],
    strokes_json: &str,
) -> StoredGraph {
    let mut nodes = Vec::with_capacity(node_rows.len());
    let mut readable = HashSet::new();
    let mut unreadable = false;
    for row in node_rows {
        if let Some(node) = decode_raw_node(row) {
            readable.insert(node.id.clone());
            nodes.push(node);
        } else {
            unreadable = true;
            if let Some(ph) = placeholder_node(row) {
                nodes.push(ph);
            }
        }
    }
    let mut edges = Vec::new();
    for row in edge_rows {
        if let Some(edge) = decode_raw_edge(row)
            && readable.contains(&edge.from)
            && readable.contains(&edge.to)
        {
            edges.push(edge);
        } else {
            unreadable = true;
        }
    }
    if let Ok(strokes) = serde_json::from_str::<Vec<WorkflowStroke>>(strokes_json) {
        let candidate = WorkflowGraph {
            nodes: nodes.clone(),
            edges: edges.clone(),
            strokes,
        };
        if candidate.validate().is_ok() {
            return StoredGraph {
                graph: candidate,
                unreadable,
            };
        }
    }
    let bare = WorkflowGraph {
        nodes,
        edges,
        strokes: Vec::new(),
    };
    if bare.validate().is_ok() {
        StoredGraph {
            graph: bare,
            unreadable: true,
        }
    } else {
        StoredGraph {
            graph: WorkflowGraph::default(),
            unreadable: true,
        }
    }
}

fn read_graph_on_conn(
    conn: &Connection,
    workflow_id: &str,
    strokes_json: &str,
) -> StorageResult<StoredGraph> {
    let mut n_stmt = conn.prepare(
        "SELECT workflow_id, node_id, kind, x, y, width, height, font, locked, fields_json, schedule_json \
         FROM workflow_nodes WHERE workflow_id = ?1 ORDER BY position, node_id",
    )?;
    let node_rows = n_stmt
        .query_map(params![workflow_id], read_raw_node_row)?
        .collect::<rusqlite::Result<Vec<_>>>()?;

    let mut e_stmt = conn.prepare(
        "SELECT workflow_id, edge_id, from_node_id, to_node_id, port_type, branch \
         FROM workflow_edges WHERE workflow_id = ?1 ORDER BY position, edge_id",
    )?;
    let edge_rows = e_stmt
        .query_map(params![workflow_id], read_raw_edge_row)?
        .collect::<rusqlite::Result<Vec<_>>>()?;

    Ok(build_stored_graph(&node_rows, &edge_rows, strokes_json))
}

fn read_triggers_on_conn(
    conn: &Connection,
    workflow_id: &str,
) -> StorageResult<Vec<WorkflowTriggerState>> {
    let mut stmt = conn.prepare(
        "SELECT t.node_id, t.kind, t.next_due_at, h.url FROM workflow_triggers AS t \
         LEFT JOIN workflow_trigger_webhooks AS h ON h.workflow_id = t.workflow_id AND h.node_id = t.node_id \
         WHERE t.workflow_id = ?1 ORDER BY t.node_id",
    )?;
    let rows = stmt.query_map(params![workflow_id], |row| {
        let node_id: String = row.get(0)?;
        let kind: String = row.get(1)?;
        let next_due_at: Option<String> = row.get(2)?;
        let webhook_url: Option<String> = row.get(3)?;
        Ok((node_id, kind, next_due_at, webhook_url))
    })?;
    let mut out = Vec::new();
    for r in rows {
        let (node_id, kind, next_due_at, webhook_url) = r?;
        let trigger_type = match kind.as_str() {
            "manual" => WorkflowTriggerType::Manual,
            "schedule" => WorkflowTriggerType::Schedule,
            "webhook" => WorkflowTriggerType::Webhook,
            other => {
                return Err(StorageError::Validation(format!(
                    "invalid stored workflow trigger kind: {other}"
                )));
            }
        };
        out.push(WorkflowTriggerState {
            node_id,
            trigger_type,
            next_due_at,
            webhook_url,
        });
    }
    Ok(out)
}

fn parse_run_status(raw: &str) -> Option<WorkflowRunStatus> {
    match raw {
        "running" => Some(WorkflowRunStatus::Running),
        "approval" => Some(WorkflowRunStatus::Approval),
        "stopping" => Some(WorkflowRunStatus::Stopping),
        "done" => Some(WorkflowRunStatus::Done),
        "failed" => Some(WorkflowRunStatus::Failed),
        "stopped" => Some(WorkflowRunStatus::Stopped),
        _ => None,
    }
}

fn run_status_str(status: WorkflowRunStatus) -> &'static str {
    match status {
        WorkflowRunStatus::Running => "running",
        WorkflowRunStatus::Approval => "approval",
        WorkflowRunStatus::Stopping => "stopping",
        WorkflowRunStatus::Done => "done",
        WorkflowRunStatus::Failed => "failed",
        WorkflowRunStatus::Stopped => "stopped",
    }
}

fn read_summary_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<WorkflowSummary> {
    let id: String = row.get(0)?;
    let title: String = row.get(1)?;
    let updated_at: String = row.get(4)?;
    let paused: i64 = row.get(5)?;
    let steps: i64 = row.get(6)?;
    let last_status: Option<String> = row.get(7)?;
    let last_at: Option<String> = row.get(8)?;
    let last_run = match (last_status.as_deref().and_then(parse_run_status), last_at) {
        (Some(status), Some(at)) => Some(WorkflowLastRun { status, at }),
        _ => None,
    };
    Ok(WorkflowSummary {
        id,
        title,
        steps: steps.max(0) as u32,
        updated_at,
        last_run,
        paused: (paused == 1).then_some(true),
    })
}

fn extract_template_names(text: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut rest = text;
    while let Some(open) = rest.find("{{") {
        let after_open = &rest[open + 2..];
        let Some(close) = after_open.find("}}") else {
            break;
        };
        let ident = after_open[..close].trim();
        let valid = !ident.is_empty()
            && ident.len() <= 40
            && ident
                .bytes()
                .next()
                .is_some_and(|b| b.is_ascii_alphabetic() || b == b'_')
            && ident
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'_');
        if valid {
            out.push(ident.to_string());
        }
        rest = &after_open[close + 2..];
    }
    out
}

pub fn workflow_graph_format(graph: &WorkflowGraph) -> u32 {
    let mut fmt = 1u32;
    let uses_v2 = graph.nodes.iter().any(|node| {
        if node.kind != WorkflowNodeKind::Output {
            return false;
        }
        if matches!(node.fields.get("message"), Some(WorkflowFieldValue::Text(s)) if !s.trim().is_empty()) {
            return true;
        }
        ["title", "labels", "person"].iter().any(|k| {
            matches!(node.fields.get(*k), Some(WorkflowFieldValue::Text(s)) if !extract_template_names(s).is_empty())
        })
    });
    if uses_v2 {
        fmt = 2;
    }
    let trigger_builtins: HashSet<&str> = graph
        .nodes
        .iter()
        .filter(|n| n.kind == WorkflowNodeKind::Trigger)
        .flat_map(|n| match n.fields.get("trigger") {
            Some(WorkflowFieldValue::Text(t)) => match t.as_str() {
                "schedule" => &["run_time", "local_date", "local_time", "weekday"][..],
                "github" => &[
                    "repository", "actor", "action", "number", "pull_request", "url", "title",
                    "body", "author", "state", "labels", "label", "assignee", "reviewer",
                    "comment_author", "comment_body", "comment_url", "review_author",
                    "review_state", "review_body", "review_url", "check_conclusion", "check_app",
                    "head_branch",
                ][..],
                "discord" => &["text", "author", "author_id", "message_id", "channel", "channel_id", "server"][..],
                "telegram" => &["text", "author", "author_id", "message_id", "chat", "chat_id"][..],
                "slack" => &["text", "author", "author_id", "message_id", "channel", "channel_id", "workspace", "thread_id"][..],
                _ => &[][..],
            },
            _ => &[][..],
        })
        .copied()
        .collect();
    if !trigger_builtins.is_empty() {
        let declared_answers: HashSet<String> = graph
            .nodes
            .iter()
            .filter_map(|n| match n.fields.get("answer") {
                Some(WorkflowFieldValue::Text(s)) => Some(s.as_str()),
                _ => None,
            })
            .flat_map(|s| s.split(['\n', ';']))
            .filter_map(|line| {
                let raw = line.split(':').next()?.trim().trim_end_matches('?').trim();
                (!raw.is_empty()).then(|| raw.to_string())
            })
            .collect();
        let uses_v3 = graph
            .nodes
            .iter()
            .filter(|n| n.kind != WorkflowNodeKind::Trigger)
            .any(|node| {
                workflow_node_registry(node.kind)
                    .iter()
                    .filter(|sp| sp.template)
                    .any(|sp| {
                        let Some(WorkflowFieldValue::Text(val)) = node.fields.get(sp.name) else {
                            return false;
                        };
                        extract_template_names(val).iter().any(|name| {
                            trigger_builtins.contains(name.as_str())
                                && !declared_answers.contains(name.as_str())
                        })
                    })
            });
        if uses_v3 {
            fmt = 3;
        }
    }
    fmt
}

// ---------------------------------------------------------------------------
// WorkflowRunStore (`workflow_runs`, `workflow_steps`, `workflow_output_deliveries`, `workflow_webhook_receipts`)
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkflowStepSource {
    pub node_id: String,
    #[serde(rename = "type")]
    pub port_type: WorkflowPortType,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub output: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkflowRun {
    pub id: String,
    pub workflow_id: String,
    pub number: u64,
    pub trigger_node_id: Option<String>,
    pub source: WorkflowRunSource,
    pub status: WorkflowRunStatus,
    pub error: Option<String>,
    pub started_at: String,
    pub ended_at: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StoredWorkflowRun {
    pub id: String,
    pub workflow_id: String,
    pub number: u64,
    pub trigger_node_id: Option<String>,
    pub from_node_id: Option<String>,
    pub source: WorkflowRunSource,
    pub scheduled_for: Option<String>,
    pub payload: Option<String>,
    pub graph: WorkflowGraph,
    pub status: WorkflowRunStatus,
    pub error: Option<String>,
    pub started_at: String,
    pub ended_at: Option<String>,
}

impl StoredWorkflowRun {
    pub fn to_public(&self) -> WorkflowRun {
        WorkflowRun {
            id: self.id.clone(),
            workflow_id: self.workflow_id.clone(),
            number: self.number,
            trigger_node_id: self.trigger_node_id.clone(),
            source: self.source,
            status: self.status,
            error: self.error.clone(),
            started_at: self.started_at.clone(),
            ended_at: self.ended_at.clone(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkflowStep {
    pub id: String,
    pub workflow_id: String,
    pub run_id: Option<String>,
    pub node_id: String,
    pub attempt: u32,
    pub agent_id: Option<String>,
    pub status: WorkflowStepStatus,
    pub output: Option<String>,
    pub branch: Option<WorkflowBranch>,
    pub error: Option<String>,
    pub started_at: Option<String>,
    pub ended_at: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StoredWorkflowStep {
    pub id: String,
    pub workflow_id: String,
    pub run_id: Option<String>,
    pub node_id: String,
    pub attempt: u32,
    pub kind: WorkflowNodeKind,
    pub agent_id: Option<String>,
    pub status: WorkflowStepStatus,
    pub delivery_id: Option<String>,
    pub turn_id: Option<String>,
    pub input: String,
    pub settings: Option<WorkflowFields>,
    pub sources: Option<Vec<WorkflowStepSource>>,
    pub output: Option<String>,
    pub branch: Option<WorkflowBranch>,
    pub error: Option<String>,
    pub started_at: Option<String>,
    pub ended_at: Option<String>,
    pub created_at: String,
    pub sequence: i64,
}

impl StoredWorkflowStep {
    pub fn to_public(&self) -> WorkflowStep {
        WorkflowStep {
            id: self.id.clone(),
            workflow_id: self.workflow_id.clone(),
            run_id: self.run_id.clone(),
            node_id: self.node_id.clone(),
            attempt: self.attempt,
            agent_id: self.agent_id.clone(),
            status: self.status,
            output: self.output.clone(),
            branch: self.branch,
            error: self.error.clone(),
            started_at: self.started_at.clone(),
            ended_at: self.ended_at.clone(),
        }
    }

    pub fn is_open(&self) -> bool {
        matches!(
            self.status,
            WorkflowStepStatus::Running | WorkflowStepStatus::Approval
        )
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkflowMediaOutput {
    pub attachment_id: String,
    pub workflow_id: String,
    pub ended_at: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkflowWebhookReceipt {
    pub workflow_id: String,
    pub node_id: String,
    pub delivery_id: String,
    pub status: String,
    pub reason: Option<String>,
    pub run_id: Option<String>,
    pub received_at: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum WorkflowOutputStatus {
    Pending,
    Sending,
    Sent,
    Failed,
    Uncertain,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkflowOutputDelivery {
    pub step_id: String,
    pub connection_id: String,
    pub place_id: String,
    pub text: String,
    pub status: WorkflowOutputStatus,
    pub reason: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NewWorkflowRun {
    pub workflow_id: String,
    pub trigger_node_id: Option<String>,
    pub from_node_id: Option<String>,
    pub source: WorkflowRunSource,
    pub scheduled_for: Option<String>,
    pub idempotency_key: String,
    pub revision: u64,
    pub graph: WorkflowGraph,
    pub payload: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NewWorkflowStep {
    pub workflow_id: String,
    pub run_id: Option<String>,
    pub node_id: String,
    pub kind: WorkflowNodeKind,
    pub request_id: String,
    pub agent_id: Option<String>,
    pub status: WorkflowStepStatus,
    pub input: String,
    pub settings: Option<WorkflowFields>,
    pub sources: Option<Vec<WorkflowStepSource>>,
    pub output: Option<String>,
    pub branch: Option<WorkflowBranch>,
    pub error: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkflowStepEnd {
    pub status: WorkflowStepStatus,
    pub output: Option<String>,
    pub branch: Option<WorkflowBranch>,
    pub error: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkflowRunWithSteps {
    pub run: WorkflowRun,
    pub steps: Vec<WorkflowStep>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkflowRunPage {
    pub runs: Vec<WorkflowRunWithSteps>,
    pub solo_steps: Vec<WorkflowStep>,
    pub next_before: Option<String>,
}

const RUN_SELECT_SQL: &str = "\
  SELECT run_id, workflow_id, number, trigger_node_id, from_node_id, source, scheduled_for, graph_json, payload, \
    status, error, started_at, ended_at \
  FROM workflow_runs";

const STEP_SELECT_SQL: &str = "\
  SELECT step_id, workflow_id, run_id, node_id, attempt, kind, agent_id, status, delivery_id, turn_id, input, \
    settings_json, sources_json, output, branch, error, started_at, ended_at, created_at, rowid AS sequence \
  FROM workflow_steps";

#[derive(Clone)]
pub struct WorkflowRunStore {
    db: OpenBotDatabase,
    create_id: IdGenerator,
}

impl WorkflowRunStore {
    pub fn new(db: OpenBotDatabase) -> Self {
        Self {
            db,
            create_id: Arc::new(|| Uuid::new_v4().to_string()),
        }
    }

    pub fn with_id_generator(db: OpenBotDatabase, create_id: IdGenerator) -> Self {
        Self { db, create_id }
    }

    pub fn start_run(
        &self,
        input: NewWorkflowRun,
        now_ms: Option<i64>,
    ) -> StorageResult<(StoredWorkflowRun, bool)> {
        let now = now_ms.unwrap_or_else(now_millis);
        let at = ms_to_iso(now);
        self.db.with_tx(|conn| {
            if let Some(existing_id) = conn
                .query_row(
                    "SELECT run_id FROM workflow_runs WHERE workflow_id = ?1 AND idempotency_key = ?2",
                    params![input.workflow_id, input.idempotency_key],
                    |r| r.get::<_, String>(0),
                )
                .optional()?
                && let Some(run) = run_on_conn(conn, &existing_id)?
            {
                return Ok((run, false));
            }

            let number: Option<i64> = conn
                .query_row(
                    "UPDATE workflows SET next_run_number = next_run_number + 1 \
                     WHERE workflow_id = ?1 RETURNING next_run_number - 1",
                    params![input.workflow_id],
                    |r| r.get(0),
                )
                .optional()?;
            let Some(number) = number else {
                return Err(StorageError::NotFound(format!(
                    "workflow {} does not exist",
                    input.workflow_id
                )));
            };

            let run_id = (self.create_id)();
            let graph_json = serde_json::to_string(&input.graph)?;
            conn.execute(
                "INSERT INTO workflow_runs (run_id, workflow_id, number, trigger_node_id, from_node_id, source, \
                   scheduled_for, idempotency_key, graph_revision, graph_json, payload, status, started_at, updated_at) \
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, 'running', ?12, ?13)",
                params![
                    run_id,
                    input.workflow_id,
                    number,
                    input.trigger_node_id,
                    input.from_node_id,
                    run_source_str(input.source),
                    input.scheduled_for,
                    input.idempotency_key,
                    input.revision as i64,
                    graph_json,
                    input.payload,
                    at,
                    at,
                ],
            )?;
            let run = run_on_conn(conn, &run_id)?
                .ok_or_else(|| StorageError::Validation("new workflow run missing".to_string()))?;
            Ok((run, true))
        })
    }

    pub fn has_webhook_receipt(
        &self,
        workflow_id: &str,
        node_id: &str,
        delivery_id: &str,
        received_at: &str,
    ) -> StorageResult<bool> {
        let cutoff = webhook_receipt_cutoff(received_at)?;
        self.db.with_conn(|conn| {
            let found: Option<i64> = conn
                .query_row(
                    "SELECT 1 FROM workflow_webhook_receipts \
                     WHERE workflow_id = ?1 AND node_id = ?2 AND delivery_id = ?3 AND received_at >= ?4",
                    params![workflow_id, node_id, delivery_id, cutoff],
                    |r| r.get(0),
                )
                .optional()?;
            Ok(found.is_some())
        })
    }

    pub fn add_webhook_receipt(&self, receipt: &WorkflowWebhookReceipt) -> StorageResult<()> {
        let cutoff = webhook_receipt_cutoff(&receipt.received_at)?;
        self.db.with_tx(|conn| {
            conn.execute(
                "DELETE FROM workflow_webhook_receipts WHERE received_at < ?1",
                params![cutoff],
            )?;
            let id = (self.create_id)();
            conn.execute(
                "INSERT INTO workflow_webhook_receipts \
                   (receipt_id, workflow_id, node_id, delivery_id, status, reason, run_id, received_at) \
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8) \
                 ON CONFLICT(workflow_id, node_id, delivery_id) DO UPDATE SET status = excluded.status, \
                   reason = excluded.reason, run_id = excluded.run_id, received_at = excluded.received_at",
                params![
                    id,
                    receipt.workflow_id,
                    receipt.node_id,
                    receipt.delivery_id,
                    receipt.status,
                    receipt.reason,
                    receipt.run_id,
                    receipt.received_at,
                ],
            )?;
            Ok(())
        })
    }

    pub fn run(&self, run_id: &str) -> StorageResult<Option<StoredWorkflowRun>> {
        self.db.with_conn(|conn| run_on_conn(conn, run_id))
    }

    pub fn handoff_source(&self, run_or_step_id: &str) -> StorageResult<Option<WorkflowRunSource>> {
        self.db.with_conn(|conn| {
            if let Some(src) = conn
                .query_row(
                    "SELECT source FROM workflow_runs WHERE run_id = ?1",
                    params![run_or_step_id],
                    |r| r.get::<_, String>(0),
                )
                .optional()?
            {
                return Ok(parse_run_source(&src));
            }
            let solo: Option<i64> = conn
                .query_row(
                    "SELECT 1 FROM workflow_steps WHERE step_id = ?1 AND run_id IS NULL",
                    params![run_or_step_id],
                    |r| r.get(0),
                )
                .optional()?;
            Ok(solo.map(|_| WorkflowRunSource::Manual))
        })
    }

    pub fn open_runs(&self) -> StorageResult<Vec<StoredWorkflowRun>> {
        self.db.with_conn(|conn| {
            let mut stmt = conn.prepare(&format!(
                "{RUN_SELECT_SQL} WHERE status IN ('running', 'approval', 'stopping') ORDER BY started_at, run_id"
            ))?;
            let rows = stmt.query_map([], read_raw_run_row)?;
            let mut out = Vec::new();
            for r in rows {
                if let Some(run) = decode_readable_run(r?) {
                    out.push(run);
                }
            }
            Ok(out)
        })
    }

    pub fn unreadable_open_run_ids(&self) -> StorageResult<HashSet<String>> {
        self.db.with_conn(|conn| {
            let mut stmt = conn.prepare(
                "SELECT run_id, graph_json FROM workflow_runs WHERE status IN ('running', 'approval', 'stopping')",
            )?;
            let rows = stmt.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))?;
            let mut out = HashSet::new();
            for r in rows {
                let (run_id, graph_json) = r?;
                if parse_stored_graph_strict(&graph_json).is_err() {
                    out.insert(run_id);
                }
            }
            Ok(out)
        })
    }

    pub fn set_run_status(
        &self,
        run_id: &str,
        status: WorkflowRunStatus,
        error: Option<&str>,
        now_ms: Option<i64>,
    ) -> StorageResult<bool> {
        let at = ms_to_iso(now_ms.unwrap_or_else(now_millis));
        let is_open = matches!(
            status,
            WorkflowRunStatus::Running | WorkflowRunStatus::Approval | WorkflowRunStatus::Stopping
        );
        let ended = (!is_open).then_some(at.as_str());
        let st_str = run_status_str(status);
        self.db.with_conn(|conn| {
            let changes = conn.execute(
                "UPDATE workflow_runs SET status = ?1, error = ?2, ended_at = ?3, updated_at = ?4 \
                 WHERE run_id = ?5 AND status IN ('running', 'approval', 'stopping') \
                   AND (status <> ?6 OR error IS NOT ?7)",
                params![st_str, error, ended, at, run_id, st_str, error],
            )?;
            Ok(changes > 0)
        })
    }

    pub fn reopen_run(&self, run_id: &str, now_ms: Option<i64>) -> StorageResult<bool> {
        let at = ms_to_iso(now_ms.unwrap_or_else(now_millis));
        self.db.with_conn(|conn| {
            let changes = conn.execute(
                "UPDATE workflow_runs SET status = 'running', error = NULL, ended_at = NULL, updated_at = ?1 \
                 WHERE run_id = ?2 AND status IN ('done', 'failed', 'stopped')",
                params![at, run_id],
            )?;
            Ok(changes > 0)
        })
    }

    pub fn steps(&self, run_id: &str) -> StorageResult<Vec<StoredWorkflowStep>> {
        self.db.with_conn(|conn| steps_for_run_on_conn(conn, run_id))
    }

    pub fn step(&self, step_id: &str) -> StorageResult<Option<StoredWorkflowStep>> {
        self.db.with_conn(|conn| step_on_conn(conn, step_id))
    }

    pub fn earlier_attempts(
        &self,
        step: &StoredWorkflowStep,
        limit: usize,
    ) -> StorageResult<Vec<StoredWorkflowStep>> {
        self.db.with_conn(|conn| {
            if let Some(run_id) = &step.run_id {
                let mut stmt = conn.prepare(&format!(
                    "{STEP_SELECT_SQL} WHERE run_id = ?1 AND node_id = ?2 AND attempt < ?3 \
                     ORDER BY attempt DESC LIMIT ?4"
                ))?;
                query_steps(&mut stmt, params![run_id, step.node_id, step.attempt, limit as i64])
            } else {
                let mut stmt = conn.prepare(&format!(
                    "{STEP_SELECT_SQL} WHERE run_id IS NULL AND workflow_id = ?1 AND node_id = ?2 AND rowid < ?3 \
                     ORDER BY rowid DESC LIMIT ?4"
                ))?;
                query_steps(
                    &mut stmt,
                    params![step.workflow_id, step.node_id, step.sequence, limit as i64],
                )
            }
        })
    }

    pub fn running_agent_step(&self, delivery_id: &str) -> StorageResult<Option<StoredWorkflowStep>> {
        self.db.with_conn(|conn| {
            let raw = conn
                .query_row(
                    &format!(
                        "{STEP_SELECT_SQL} WHERE delivery_id = ?1 AND status = 'running' AND kind = 'agent'"
                    ),
                    params![delivery_id],
                    read_raw_step_row,
                )
                .optional()?;
            raw.map(decode_step_row).transpose()
        })
    }

    pub fn keep_agent_answer(
        &self,
        step_id: &str,
        delivery_id: &str,
        output: &str,
        now_ms: Option<i64>,
    ) -> StorageResult<bool> {
        let at = ms_to_iso(now_ms.unwrap_or_else(now_millis));
        self.db.with_conn(|conn| {
            let changes = conn.execute(
                "UPDATE workflow_steps SET output = ?1, updated_at = ?2 \
                 WHERE step_id = ?3 AND delivery_id = ?4 AND status = 'running'",
                params![output, at, step_id, delivery_id],
            )?;
            Ok(changes > 0)
        })
    }

    pub fn open_steps(&self) -> StorageResult<Vec<StoredWorkflowStep>> {
        self.db.with_conn(|conn| {
            let mut stmt = conn.prepare(&format!(
                "{STEP_SELECT_SQL} WHERE status IN ('running', 'approval') ORDER BY created_at, step_id"
            ))?;
            query_steps(&mut stmt, [])
        })
    }

    pub fn latest_done(&self, workflow_id: &str) -> StorageResult<HashMap<String, StoredWorkflowStep>> {
        self.db.with_conn(|conn| {
            let mut stmt = conn.prepare(&format!(
                "{STEP_SELECT_SQL} WHERE workflow_id = ?1 AND status = 'done' \
                   AND (run_id IS NULL OR request_id <> run_id || ':' || node_id || ':reused') \
                 ORDER BY created_at, attempt, step_id"
            ))?;
            let list = query_steps(&mut stmt, params![workflow_id])?;
            Ok(list.into_iter().map(|s| (s.node_id.clone(), s)).collect())
        })
    }

    pub fn latest_settled(
        &self,
        workflow_id: &str,
    ) -> StorageResult<HashMap<String, StoredWorkflowStep>> {
        self.db.with_conn(|conn| {
            let mut stmt = conn.prepare(&format!(
                "{STEP_SELECT_SQL} WHERE workflow_id = ?1 AND status IN ('done', 'failed', 'stopped') \
                   AND (run_id IS NULL OR request_id <> run_id || ':' || node_id || ':reused') \
                 ORDER BY created_at, attempt, step_id"
            ))?;
            let list = query_steps(&mut stmt, params![workflow_id])?;
            Ok(list.into_iter().map(|s| (s.node_id.clone(), s)).collect())
        })
    }

    pub fn media_outputs(&self) -> StorageResult<Vec<WorkflowMediaOutput>> {
        self.db.with_conn(|conn| {
            let mut stmt = conn.prepare(
                "SELECT output, workflow_id, ended_at FROM workflow_steps \
                 WHERE kind = 'image' AND status = 'done' AND output IS NOT NULL \
                 ORDER BY created_at, step_id",
            )?;
            let rows = stmt.query_map([], |r| {
                Ok(WorkflowMediaOutput {
                    attachment_id: r.get(0)?,
                    workflow_id: r.get(1)?,
                    ended_at: r.get(2)?,
                })
            })?;
            Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
        })
    }

    pub fn add_step(
        &self,
        input: NewWorkflowStep,
        now_ms: Option<i64>,
    ) -> StorageResult<StoredWorkflowStep> {
        if !input.kind.is_step() {
            return Err(StorageError::Validation(format!(
                "annotation node kind {} cannot execute as workflow step",
                input.kind.as_str()
            )));
        }
        let at = ms_to_iso(now_ms.unwrap_or_else(now_millis));
        self.db.with_tx(|conn| {
            if let Some(existing_id) = conn
                .query_row(
                    "SELECT step_id FROM workflow_steps WHERE workflow_id = ?1 AND request_id = ?2",
                    params![input.workflow_id, input.request_id],
                    |r| r.get::<_, String>(0),
                )
                .optional()?
                && let Some(step) = step_on_conn(conn, &existing_id)?
            {
                return Ok(step);
            }
            let attempt: i64 = match &input.run_id {
                None => 1,
                Some(rid) => conn.query_row(
                    "SELECT COALESCE(MAX(attempt), 0) + 1 FROM workflow_steps WHERE run_id = ?1 AND node_id = ?2",
                    params![rid, input.node_id],
                    |r| r.get(0),
                )?,
            };
            let step_id = (self.create_id)();
            let is_open = matches!(
                input.status,
                WorkflowStepStatus::Running | WorkflowStepStatus::Approval
            );
            let ended = (!is_open).then_some(at.as_str());
            let settings_json = input
                .settings
                .as_ref()
                .map(serde_json::to_string)
                .transpose()?;
            let sources_json = input
                .sources
                .as_ref()
                .map(serde_json::to_string)
                .transpose()?;
            conn.execute(
                "INSERT INTO workflow_steps (step_id, workflow_id, run_id, node_id, attempt, request_id, kind, \
                   agent_id, status, input, settings_json, sources_json, output, branch, error, started_at, ended_at, \
                   created_at, updated_at) \
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17, ?18, ?19)",
                params![
                    step_id,
                    input.workflow_id,
                    input.run_id,
                    input.node_id,
                    attempt,
                    input.request_id,
                    input.kind.as_str(),
                    input.agent_id,
                    step_status_str(input.status),
                    input.input,
                    settings_json,
                    sources_json,
                    input.output,
                    input.branch.map(WorkflowBranch::as_str),
                    input.error,
                    at,
                    ended,
                    at,
                    at,
                ],
            )?;
            step_on_conn(conn, &step_id)?
                .ok_or_else(|| StorageError::Validation("new workflow step missing".to_string()))
        })
    }

    pub fn record_used_settings(&self, step_id: &str, used: &WorkflowFields) -> StorageResult<()> {
        self.db.with_tx(|conn| {
            let Some(mut settings) = step_on_conn(conn, step_id)?.and_then(|s| s.settings) else {
                return Ok(());
            };
            for (k, v) in used {
                settings.insert(k.clone(), v.clone());
            }
            let json = serde_json::to_string(&settings)?;
            conn.execute(
                "UPDATE workflow_steps SET settings_json = ?1 WHERE step_id = ?2",
                params![json, step_id],
            )?;
            Ok(())
        })
    }

    pub fn attach_delivery(
        &self,
        step_id: &str,
        delivery_id: &str,
        now_ms: Option<i64>,
    ) -> StorageResult<()> {
        let at = ms_to_iso(now_ms.unwrap_or_else(now_millis));
        self.db.with_conn(|conn| {
            conn.execute(
                "UPDATE workflow_steps SET delivery_id = ?1, updated_at = ?2 \
                 WHERE step_id = ?3 AND delivery_id IS NULL",
                params![delivery_id, at, step_id],
            )?;
            Ok(())
        })
    }

    pub fn replace_delivery(
        &self,
        step_id: &str,
        delivery_id: &str,
        next_delivery_id: &str,
        now_ms: Option<i64>,
    ) -> StorageResult<bool> {
        let at = ms_to_iso(now_ms.unwrap_or_else(now_millis));
        self.db.with_conn(|conn| {
            let changes = conn.execute(
                "UPDATE workflow_steps SET delivery_id = ?1, turn_id = NULL, updated_at = ?2 \
                 WHERE step_id = ?3 AND delivery_id = ?4 AND status = 'running'",
                params![next_delivery_id, at, step_id, delivery_id],
            )?;
            Ok(changes > 0)
        })
    }

    pub fn attach_turn(
        &self,
        step_id: &str,
        turn_id: &str,
        now_ms: Option<i64>,
    ) -> StorageResult<()> {
        let at = ms_to_iso(now_ms.unwrap_or_else(now_millis));
        self.db.with_conn(|conn| {
            conn.execute(
                "UPDATE workflow_steps SET turn_id = ?1, updated_at = ?2 \
                 WHERE step_id = ?3 AND turn_id IS NOT ?4",
                params![turn_id, at, step_id, turn_id],
            )?;
            Ok(())
        })
    }

    pub fn await_approval(&self, step_id: &str, now_ms: Option<i64>) -> StorageResult<bool> {
        let at = ms_to_iso(now_ms.unwrap_or_else(now_millis));
        self.db.with_conn(|conn| {
            let changes = conn.execute(
                "UPDATE workflow_steps SET status = 'approval', updated_at = ?1 \
                 WHERE step_id = ?2 AND status = 'running'",
                params![at, step_id],
            )?;
            Ok(changes > 0)
        })
    }

    pub fn settle_step(
        &self,
        step_id: &str,
        end: &WorkflowStepEnd,
        now_ms: Option<i64>,
    ) -> StorageResult<bool> {
        if matches!(
            end.status,
            WorkflowStepStatus::Running | WorkflowStepStatus::Approval
        ) {
            return Err(StorageError::Validation(
                "settle_step requires terminal WorkflowStepStatus".to_string(),
            ));
        }
        let at = ms_to_iso(now_ms.unwrap_or_else(now_millis));
        self.db.with_conn(|conn| {
            let changes = conn.execute(
                "UPDATE workflow_steps SET status = ?1, output = ?2, branch = ?3, error = ?4, \
                   ended_at = ?5, updated_at = ?6, started_at = COALESCE(started_at, ?7) \
                 WHERE step_id = ?8 AND status IN ('running', 'approval')",
                params![
                    step_status_str(end.status),
                    end.output,
                    end.branch.map(WorkflowBranch::as_str),
                    end.error,
                    at,
                    at,
                    at,
                    step_id,
                ],
            )?;
            Ok(changes > 0)
        })
    }

    pub fn add_output(
        &self,
        step_id: &str,
        connection_id: &str,
        place_id: &str,
        text: &str,
        now_ms: Option<i64>,
    ) -> StorageResult<()> {
        let at = ms_to_iso(now_ms.unwrap_or_else(now_millis));
        self.db.with_conn(|conn| {
            conn.execute(
                "INSERT OR IGNORE INTO workflow_output_deliveries \
                   (step_id, connection_id, place_id, text, status, created_at, updated_at) \
                 VALUES (?1, ?2, ?3, ?4, 'pending', ?5, ?6)",
                params![step_id, connection_id, place_id, text, at, at],
            )?;
            Ok(())
        })
    }

    pub fn output(&self, step_id: &str) -> StorageResult<Option<WorkflowOutputDelivery>> {
        self.db.with_conn(|conn| {
            let raw = conn
                .query_row(
                    "SELECT step_id, connection_id, place_id, text, status, reason \
                     FROM workflow_output_deliveries WHERE step_id = ?1",
                    params![step_id],
                    |r| {
                        Ok((
                            r.get::<_, String>(0)?,
                            r.get::<_, String>(1)?,
                            r.get::<_, String>(2)?,
                            r.get::<_, String>(3)?,
                            r.get::<_, String>(4)?,
                            r.get::<_, Option<String>>(5)?,
                        ))
                    },
                )
                .optional()?;
            let Some((step_id, connection_id, place_id, text, status_s, reason)) = raw else {
                return Ok(None);
            };
            let status = parse_output_status(&status_s)?;
            Ok(Some(WorkflowOutputDelivery {
                step_id,
                connection_id,
                place_id,
                text,
                status,
                reason,
            }))
        })
    }

    pub fn claim_output(&self, step_id: &str, now_ms: Option<i64>) -> StorageResult<bool> {
        let at = ms_to_iso(now_ms.unwrap_or_else(now_millis));
        self.db.with_conn(|conn| {
            let changes = conn.execute(
                "UPDATE workflow_output_deliveries SET status = 'sending', attempts = attempts + 1, updated_at = ?1 \
                 WHERE step_id = ?2 AND status = 'pending'",
                params![at, step_id],
            )?;
            Ok(changes > 0)
        })
    }

    pub fn end_output(
        &self,
        step_id: &str,
        status: WorkflowOutputStatus,
        reason: Option<&str>,
        now_ms: Option<i64>,
    ) -> StorageResult<()> {
        if matches!(
            status,
            WorkflowOutputStatus::Pending | WorkflowOutputStatus::Sending
        ) {
            return Err(StorageError::Validation(
                "end_output requires terminal WorkflowOutputStatus".to_string(),
            ));
        }
        let at = ms_to_iso(now_ms.unwrap_or_else(now_millis));
        let sent_at = (status == WorkflowOutputStatus::Sent).then_some(at.as_str());
        self.db.with_conn(|conn| {
            conn.execute(
                "UPDATE workflow_output_deliveries SET status = ?1, reason = ?2, sent_at = ?3, updated_at = ?4 \
                 WHERE step_id = ?5 AND status IN ('pending', 'sending')",
                params![output_status_str(status), reason, sent_at, at, step_id],
            )?;
            Ok(())
        })
    }

    pub fn page(
        &self,
        workflow_id: &str,
        before: Option<&str>,
        limit: usize,
    ) -> StorageResult<WorkflowRunPage> {
        let safe_limit = limit.clamp(1, INPUT_LIMITS.workflow_runs_page);
        self.db.with_conn(|conn| {
            let raw_runs: Vec<RawRunRow> = if let Some(cursor) = before {
                let (started_at, run_id) = match cursor.rfind('|') {
                    Some(idx) => (&cursor[..idx], &cursor[idx + 1..]),
                    None => (cursor, ""),
                };
                let mut stmt = conn.prepare(&format!(
                    "{RUN_SELECT_SQL} WHERE workflow_id = ?1 \
                       AND (started_at < ?2 OR (started_at = ?3 AND run_id < ?4)) \
                     ORDER BY started_at DESC, run_id DESC LIMIT ?5"
                ))?;
                stmt.query_map(
                    params![workflow_id, started_at, started_at, run_id, (safe_limit + 1) as i64],
                    read_raw_run_row,
                )?
                .collect::<rusqlite::Result<Vec<_>>>()?
            } else {
                let mut stmt = conn.prepare(&format!(
                    "{RUN_SELECT_SQL} WHERE workflow_id = ?1 \
                     ORDER BY started_at DESC, run_id DESC LIMIT ?2"
                ))?;
                stmt.query_map(params![workflow_id, (safe_limit + 1) as i64], read_raw_run_row)?
                    .collect::<rusqlite::Result<Vec<_>>>()?
            };

            let has_more = raw_runs.len() > safe_limit;
            let mut runs = Vec::with_capacity(raw_runs.len().min(safe_limit));
            let mut last_cursor: Option<(String, String)> = None;
            for raw in raw_runs.into_iter().take(safe_limit) {
                let decoded = decode_run_with_fallback(raw)?;
                last_cursor = Some((decoded.started_at.clone(), decoded.id.clone()));
                let steps = steps_for_run_on_conn(conn, &decoded.id)?
                    .into_iter()
                    .map(|s| s.to_public())
                    .collect();
                runs.push(WorkflowRunWithSteps {
                    run: decoded.to_public(),
                    steps,
                });
            }

            let solo_steps = if before.is_none() {
                let mut stmt = conn.prepare(&format!(
                    "{STEP_SELECT_SQL} WHERE workflow_id = ?1 AND run_id IS NULL \
                     ORDER BY created_at DESC, step_id DESC LIMIT ?2"
                ))?;
                query_steps(&mut stmt, params![workflow_id, safe_limit as i64])?
                    .into_iter()
                    .map(|s| s.to_public())
                    .collect()
            } else {
                Vec::new()
            };

            let next_before = if has_more {
                last_cursor.map(|(st, id)| format!("{st}|{id}"))
            } else {
                None
            };
            Ok(WorkflowRunPage {
                runs,
                solo_steps,
                next_before,
            })
        })
    }
}

struct RawRunRow {
    run_id: String,
    workflow_id: String,
    number: i64,
    trigger_node_id: Option<String>,
    from_node_id: Option<String>,
    source: String,
    scheduled_for: Option<String>,
    graph_json: String,
    payload: Option<String>,
    status: String,
    error: Option<String>,
    started_at: String,
    ended_at: Option<String>,
}

fn read_raw_run_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<RawRunRow> {
    Ok(RawRunRow {
        run_id: row.get(0)?,
        workflow_id: row.get(1)?,
        number: row.get(2)?,
        trigger_node_id: row.get(3)?,
        from_node_id: row.get(4)?,
        source: row.get(5)?,
        scheduled_for: row.get(6)?,
        graph_json: row.get(7)?,
        payload: row.get(8)?,
        status: row.get(9)?,
        error: row.get(10)?,
        started_at: row.get(11)?,
        ended_at: row.get(12)?,
    })
}

fn decode_readable_run(raw: RawRunRow) -> Option<StoredWorkflowRun> {
    let graph = parse_stored_graph_strict(&raw.graph_json).ok()?;
    let source = parse_run_source(&raw.source)?;
    let status = parse_run_status(&raw.status)?;
    Some(StoredWorkflowRun {
        id: raw.run_id,
        workflow_id: raw.workflow_id,
        number: raw.number.max(1) as u64,
        trigger_node_id: raw.trigger_node_id,
        from_node_id: raw.from_node_id,
        source,
        scheduled_for: raw.scheduled_for,
        payload: raw.payload,
        graph,
        status,
        error: raw.error,
        started_at: raw.started_at,
        ended_at: raw.ended_at,
    })
}

fn decode_run_with_fallback(raw: RawRunRow) -> StorageResult<StoredWorkflowRun> {
    let graph = parse_stored_graph_strict(&raw.graph_json).unwrap_or_default();
    let source = parse_run_source(&raw.source)
        .ok_or_else(|| StorageError::Validation(format!("invalid run source: {}", raw.source)))?;
    let status = parse_run_status(&raw.status)
        .ok_or_else(|| StorageError::Validation(format!("invalid run status: {}", raw.status)))?;
    Ok(StoredWorkflowRun {
        id: raw.run_id,
        workflow_id: raw.workflow_id,
        number: raw.number.max(1) as u64,
        trigger_node_id: raw.trigger_node_id,
        from_node_id: raw.from_node_id,
        source,
        scheduled_for: raw.scheduled_for,
        payload: raw.payload,
        graph,
        status,
        error: raw.error,
        started_at: raw.started_at,
        ended_at: raw.ended_at,
    })
}

fn run_on_conn(conn: &Connection, run_id: &str) -> StorageResult<Option<StoredWorkflowRun>> {
    let raw = conn
        .query_row(
            &format!("{RUN_SELECT_SQL} WHERE run_id = ?1"),
            params![run_id],
            read_raw_run_row,
        )
        .optional()?;
    let Some(raw) = raw else {
        return Ok(None);
    };
    decode_readable_run(raw)
        .ok_or_else(|| StorageError::Validation("invalid workflow_runs.graph_json".to_string()))
        .map(Some)
}

struct RawStepRow {
    step_id: String,
    workflow_id: String,
    run_id: Option<String>,
    node_id: String,
    attempt: i64,
    kind: String,
    agent_id: Option<String>,
    status: String,
    delivery_id: Option<String>,
    turn_id: Option<String>,
    input: String,
    settings_json: Option<String>,
    sources_json: Option<String>,
    output: Option<String>,
    branch: Option<String>,
    error: Option<String>,
    started_at: Option<String>,
    ended_at: Option<String>,
    created_at: String,
    sequence: i64,
}

fn read_raw_step_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<RawStepRow> {
    Ok(RawStepRow {
        step_id: row.get(0)?,
        workflow_id: row.get(1)?,
        run_id: row.get(2)?,
        node_id: row.get(3)?,
        attempt: row.get(4)?,
        kind: row.get(5)?,
        agent_id: row.get(6)?,
        status: row.get(7)?,
        delivery_id: row.get(8)?,
        turn_id: row.get(9)?,
        input: row.get(10)?,
        settings_json: row.get(11)?,
        sources_json: row.get(12)?,
        output: row.get(13)?,
        branch: row.get(14)?,
        error: row.get(15)?,
        started_at: row.get(16)?,
        ended_at: row.get(17)?,
        created_at: row.get(18)?,
        sequence: row.get(19)?,
    })
}

fn decode_step_row(raw: RawStepRow) -> StorageResult<StoredWorkflowStep> {
    let kind = WorkflowNodeKind::ALL
        .into_iter()
        .find(|k| k.as_str() == raw.kind)
        .ok_or_else(|| StorageError::Validation(format!("invalid step kind: {}", raw.kind)))?;
    let status = parse_step_status(&raw.status)?;
    let settings = raw
        .settings_json
        .as_deref()
        .map(serde_json::from_str::<WorkflowFields>)
        .transpose()?;
    let sources = raw
        .sources_json
        .as_deref()
        .map(serde_json::from_str::<Vec<WorkflowStepSource>>)
        .transpose()?;
    let branch = match raw.branch.as_deref() {
        Some("yes") => Some(WorkflowBranch::Yes),
        Some("no") => Some(WorkflowBranch::No),
        _ => None,
    };
    Ok(StoredWorkflowStep {
        id: raw.step_id,
        workflow_id: raw.workflow_id,
        run_id: raw.run_id,
        node_id: raw.node_id,
        attempt: raw.attempt.max(1) as u32,
        kind,
        agent_id: raw.agent_id,
        status,
        delivery_id: raw.delivery_id,
        turn_id: raw.turn_id,
        input: raw.input,
        settings,
        sources,
        output: raw.output,
        branch,
        error: raw.error,
        started_at: raw.started_at,
        ended_at: raw.ended_at,
        created_at: raw.created_at,
        sequence: raw.sequence,
    })
}

fn query_steps<P: rusqlite::Params>(
    stmt: &mut rusqlite::Statement<'_>,
    params: P,
) -> StorageResult<Vec<StoredWorkflowStep>> {
    let rows = stmt.query_map(params, read_raw_step_row)?;
    let mut out = Vec::new();
    for r in rows {
        out.push(decode_step_row(r?)?);
    }
    Ok(out)
}

fn step_on_conn(conn: &Connection, step_id: &str) -> StorageResult<Option<StoredWorkflowStep>> {
    let raw = conn
        .query_row(
            &format!("{STEP_SELECT_SQL} WHERE step_id = ?1"),
            params![step_id],
            read_raw_step_row,
        )
        .optional()?;
    raw.map(decode_step_row).transpose()
}

fn steps_for_run_on_conn(conn: &Connection, run_id: &str) -> StorageResult<Vec<StoredWorkflowStep>> {
    let mut stmt = conn.prepare(&format!(
        "{STEP_SELECT_SQL} WHERE run_id = ?1 ORDER BY created_at, attempt, step_id"
    ))?;
    query_steps(&mut stmt, params![run_id])
}

fn run_source_str(source: WorkflowRunSource) -> &'static str {
    match source {
        WorkflowRunSource::Manual => "manual",
        WorkflowRunSource::Schedule => "schedule",
        WorkflowRunSource::Webhook => "webhook",
        WorkflowRunSource::GitHub => "github",
        WorkflowRunSource::Discord => "discord",
        WorkflowRunSource::Telegram => "telegram",
        WorkflowRunSource::Slack => "slack",
    }
}

fn parse_run_source(raw: &str) -> Option<WorkflowRunSource> {
    match raw {
        "manual" => Some(WorkflowRunSource::Manual),
        "schedule" => Some(WorkflowRunSource::Schedule),
        "webhook" => Some(WorkflowRunSource::Webhook),
        "github" => Some(WorkflowRunSource::GitHub),
        "discord" => Some(WorkflowRunSource::Discord),
        "telegram" => Some(WorkflowRunSource::Telegram),
        "slack" => Some(WorkflowRunSource::Slack),
        _ => None,
    }
}

fn step_status_str(status: WorkflowStepStatus) -> &'static str {
    match status {
        WorkflowStepStatus::Running => "running",
        WorkflowStepStatus::Approval => "approval",
        WorkflowStepStatus::Done => "done",
        WorkflowStepStatus::Failed => "failed",
        WorkflowStepStatus::Stopped => "stopped",
        WorkflowStepStatus::Skipped => "skipped",
    }
}

fn parse_step_status(raw: &str) -> StorageResult<WorkflowStepStatus> {
    match raw {
        "running" => Ok(WorkflowStepStatus::Running),
        "approval" => Ok(WorkflowStepStatus::Approval),
        "done" => Ok(WorkflowStepStatus::Done),
        "failed" => Ok(WorkflowStepStatus::Failed),
        "stopped" => Ok(WorkflowStepStatus::Stopped),
        "skipped" => Ok(WorkflowStepStatus::Skipped),
        other => Err(StorageError::Validation(format!(
            "invalid step status: {other}"
        ))),
    }
}

fn output_status_str(status: WorkflowOutputStatus) -> &'static str {
    match status {
        WorkflowOutputStatus::Pending => "pending",
        WorkflowOutputStatus::Sending => "sending",
        WorkflowOutputStatus::Sent => "sent",
        WorkflowOutputStatus::Failed => "failed",
        WorkflowOutputStatus::Uncertain => "uncertain",
    }
}

fn parse_output_status(raw: &str) -> StorageResult<WorkflowOutputStatus> {
    match raw {
        "pending" => Ok(WorkflowOutputStatus::Pending),
        "sending" => Ok(WorkflowOutputStatus::Sending),
        "sent" => Ok(WorkflowOutputStatus::Sent),
        "failed" => Ok(WorkflowOutputStatus::Failed),
        "uncertain" => Ok(WorkflowOutputStatus::Uncertain),
        other => Err(StorageError::Validation(format!(
            "invalid workflow output status: {other}"
        ))),
    }
}

// ---------------------------------------------------------------------------
// WorkflowGitHubStore (`workflow_github_deliveries`, `workflow_github_runs`, `workflow_github_actions`)
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkflowGitHubItem {
    pub repository: String,
    pub number: u64,
    pub pull_request: bool,
    pub title: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum WorkflowGitHubDeliveryOutcome {
    Started,
    Waiting,
    Skipped,
}

impl WorkflowGitHubDeliveryOutcome {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Started => "started",
            Self::Waiting => "waiting",
            Self::Skipped => "skipped",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum WorkflowGitHubSkipReason {
    Filter,
    #[serde(rename = "self")]
    SelfCaused,
    Chain,
    Offline,
    Refused,
}

impl WorkflowGitHubSkipReason {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Filter => "filter",
            Self::SelfCaused => "self",
            Self::Chain => "chain",
            Self::Offline => "offline",
            Self::Refused => "refused",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkflowGitHubDelivery {
    pub id: String,
    pub node_id: String,
    pub event: String,
    pub item: Option<WorkflowGitHubItem>,
    pub received_at: String,
    pub outcome: WorkflowGitHubDeliveryOutcome,
    pub reason: Option<WorkflowGitHubSkipReason>,
    pub run_id: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkflowGitHubRunItem {
    pub run_id: String,
    pub item: WorkflowGitHubItem,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkflowGitHubHistory {
    pub deliveries: Vec<WorkflowGitHubDelivery>,
    pub runs: Vec<WorkflowGitHubRunItem>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NewGitHubDelivery {
    pub workflow_id: String,
    pub node_id: String,
    pub delivery_id: String,
    pub event: String,
    pub item: Option<WorkflowGitHubItem>,
    pub received_at: String,
    pub outcome: WorkflowGitHubDeliveryOutcome,
    pub reason: Option<WorkflowGitHubSkipReason>,
    pub run_id: Option<String>,
    pub payload: Option<String>,
    pub caused_by_bot: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WaitingGitHubDelivery {
    pub workflow_id: String,
    pub node_id: String,
    pub delivery_id: String,
    pub item: WorkflowGitHubItem,
    pub payload: String,
    pub caused_by_bot: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NewGitHubRun {
    pub run_id: String,
    pub workflow_id: String,
    pub node_id: String,
    pub delivery_id: Option<String>,
    pub item: WorkflowGitHubItem,
    pub caused_by_bot: bool,
    pub started_at: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum GitHubActionStatus {
    Sending,
    Done,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NewGitHubAction {
    pub action_key: String,
    pub workflow_id: String,
    pub run_id: Option<String>,
    pub step_id: String,
    pub repository: String,
    pub number: Option<u64>,
    pub action: String,
    pub object_keys: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GitHubActionRecord {
    pub step_id: String,
    pub number: Option<u64>,
    pub status: GitHubActionStatus,
    pub object_keys: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WaitingGitHubEnd {
    Started { run_id: String },
    Skipped { reason: WorkflowGitHubSkipReason },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OfflineGitHubTrigger {
    pub workflow_id: String,
    pub node_id: String,
    pub event: String,
}

const WAITING_SELECT_SQL: &str = "\
  SELECT d.workflow_id, d.node_id, d.delivery_id, d.repository, d.number, d.pull_request, d.title, d.payload, \
    d.caused_by_bot, d.received_at FROM workflow_github_deliveries d \
  WHERE d.workflow_id = ?1 AND d.outcome = 'waiting'";

#[derive(Clone)]
pub struct WorkflowGitHubStore {
    db: OpenBotDatabase,
}

impl WorkflowGitHubStore {
    pub fn new(db: OpenBotDatabase) -> Self {
        Self { db }
    }

    pub fn has_delivery(
        &self,
        workflow_id: &str,
        node_id: &str,
        delivery_id: &str,
    ) -> StorageResult<bool> {
        self.db.with_conn(|conn| {
            let found: Option<i64> = conn
                .query_row(
                    "SELECT 1 FROM workflow_github_deliveries \
                     WHERE workflow_id = ?1 AND node_id = ?2 AND delivery_id = ?3",
                    params![workflow_id, node_id, delivery_id],
                    |r| r.get(0),
                )
                .optional()?;
            Ok(found.is_some())
        })
    }

    pub fn add_delivery(&self, delivery: &NewGitHubDelivery) -> StorageResult<()> {
        if !WORKFLOW_GITHUB_EVENTS.contains(&delivery.event.as_str()) {
            return Err(StorageError::Validation(format!(
                "unknown github event: {}",
                delivery.event
            )));
        }
        self.db.with_tx(|conn| {
            add_github_delivery_on_conn(conn, delivery)?;
            prune_github_deliveries_on_conn(conn, &delivery.workflow_id, &delivery.node_id)
        })
    }

    pub fn add_offline(
        &self,
        triggers: &[OfflineGitHubTrigger],
        from_ms: i64,
        to_ms: i64,
    ) -> StorageResult<()> {
        let from_iso = ms_to_iso(from_ms);
        let to_iso = ms_to_iso(to_ms);
        self.db.with_tx(|conn| {
            for trig in triggers {
                if !WORKFLOW_GITHUB_EVENTS.contains(&trig.event.as_str()) {
                    return Err(StorageError::Validation(format!(
                        "unknown github event: {}",
                        trig.event
                    )));
                }
                let newest: Option<(String, String, Option<String>)> = conn
                    .query_row(
                        "SELECT delivery_id, received_at, reason FROM workflow_github_deliveries \
                         WHERE workflow_id = ?1 AND node_id = ?2 \
                         ORDER BY received_at DESC, delivery_id DESC LIMIT 1",
                        params![trig.workflow_id, trig.node_id],
                        |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
                    )
                    .optional()?;
                if let Some((del_id, rec_at, Some(reason))) = newest
                    && reason == "offline"
                    && let Some(rec_ms) = parse_iso_ms(&rec_at)
                    && from_ms - rec_ms <= WORKFLOW_GITHUB_OFFLINE_MERGE_MS
                {
                    conn.execute(
                        "UPDATE workflow_github_deliveries SET received_at = ?1 \
                         WHERE workflow_id = ?2 AND node_id = ?3 AND delivery_id = ?4",
                        params![to_iso, trig.workflow_id, trig.node_id, del_id],
                    )?;
                    continue;
                }
                let entry = NewGitHubDelivery {
                    workflow_id: trig.workflow_id.clone(),
                    node_id: trig.node_id.clone(),
                    delivery_id: format!("offline:{from_iso}"),
                    event: trig.event.clone(),
                    item: None,
                    received_at: to_iso.clone(),
                    outcome: WorkflowGitHubDeliveryOutcome::Skipped,
                    reason: Some(WorkflowGitHubSkipReason::Offline),
                    run_id: None,
                    payload: None,
                    caused_by_bot: false,
                };
                add_github_delivery_on_conn(conn, &entry)?;
                prune_github_deliveries_on_conn(conn, &trig.workflow_id, &trig.node_id)?;
            }
            Ok(())
        })
    }

    pub fn item_busy(&self, workflow_id: &str, repository: &str, number: u64) -> StorageResult<bool> {
        Ok(self.run_open(workflow_id, repository, number)?
            || self.next_waiting(workflow_id, repository, number)?.is_some())
    }

    pub fn run_open(&self, workflow_id: &str, repository: &str, number: u64) -> StorageResult<bool> {
        self.db.with_conn(|conn| {
            let found: Option<i64> = conn
                .query_row(
                    "SELECT 1 FROM workflow_github_runs g JOIN workflow_runs r ON r.run_id = g.run_id \
                     WHERE g.workflow_id = ?1 AND g.repository = ?2 AND g.number = ?3 \
                       AND r.status IN ('running', 'approval', 'stopping') LIMIT 1",
                    params![workflow_id, repository, number as i64],
                    |r| r.get(0),
                )
                .optional()?;
            Ok(found.is_some())
        })
    }

    pub fn add_run(&self, run: &NewGitHubRun) -> StorageResult<()> {
        self.db.with_conn(|conn| {
            conn.execute(
                "INSERT INTO workflow_github_runs (run_id, workflow_id, node_id, delivery_id, repository, number, \
                   pull_request, title, caused_by_bot, started_at) \
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
                params![
                    run.run_id,
                    run.workflow_id,
                    run.node_id,
                    run.delivery_id,
                    run.item.repository,
                    run.item.number as i64,
                    i64::from(run.item.pull_request),
                    run.item.title,
                    i64::from(run.caused_by_bot),
                    run.started_at,
                ],
            )?;
            Ok(())
        })
    }

    pub fn bot_runs_since(
        &self,
        repository: &str,
        number: u64,
        since_ms: i64,
    ) -> StorageResult<u64> {
        let since_iso = ms_to_iso(since_ms);
        self.db.with_conn(|conn| {
            let count: i64 = conn.query_row(
                "SELECT COUNT(*) FROM workflow_github_runs \
                 WHERE repository = ?1 AND number = ?2 AND caused_by_bot = 1 AND started_at >= ?3",
                params![repository, number as i64, since_iso],
                |r| r.get(0),
            )?;
            Ok(count.max(0) as u64)
        })
    }

    pub fn self_caused(
        &self,
        workflow_id: &str,
        repository: &str,
        number: u64,
        object_key: Option<&str>,
        since_ms: i64,
    ) -> StorageResult<bool> {
        let since_iso = ms_to_iso(since_ms);
        self.db.with_conn(|conn| {
            let sending: Option<i64> = conn
                .query_row(
                    "SELECT 1 FROM workflow_github_actions \
                     WHERE workflow_id = ?1 AND repository = ?2 AND number = ?3 \
                       AND status = 'sending' AND updated_at >= ?4 LIMIT 1",
                    params![workflow_id, repository, number as i64, since_iso],
                    |r| r.get(0),
                )
                .optional()?;
            if sending.is_some() {
                return Ok(true);
            }
            let Some(obj_key) = object_key.filter(|s| !s.is_empty()) else {
                return Ok(false);
            };
            let matched: Option<i64> = conn
                .query_row(
                    "SELECT 1 FROM workflow_github_actions \
                     WHERE workflow_id = ?1 AND repository = ?2 AND (number IS NULL OR number = ?3) \
                       AND updated_at >= ?4 \
                       AND instr(char(10) || object_id || char(10), char(10) || ?5 || char(10)) > 0 LIMIT 1",
                    params![workflow_id, repository, number as i64, since_iso, obj_key],
                    |r| r.get(0),
                )
                .optional()?;
            Ok(matched.is_some())
        })
    }

    pub fn run_item(&self, run_id: &str) -> StorageResult<Option<WorkflowGitHubItem>> {
        self.db.with_conn(|conn| {
            conn.query_row(
                "SELECT repository, number, pull_request, title FROM workflow_github_runs WHERE run_id = ?1",
                params![run_id],
                |r| {
                    Ok(WorkflowGitHubItem {
                        repository: r.get(0)?,
                        number: r.get::<_, i64>(1)?.max(1) as u64,
                        pull_request: r.get::<_, i64>(2)? == 1,
                        title: r.get(3)?,
                    })
                },
            )
            .optional()
            .map_err(StorageError::from)
        })
    }

    pub fn action(&self, action_key: &str) -> StorageResult<Option<GitHubActionRecord>> {
        self.db.with_conn(|conn| {
            let raw = conn
                .query_row(
                    "SELECT step_id, number, status, object_id FROM workflow_github_actions WHERE action_key = ?1",
                    params![action_key],
                    |r| {
                        Ok((
                            r.get::<_, String>(0)?,
                            r.get::<_, Option<i64>>(1)?,
                            r.get::<_, String>(2)?,
                            r.get::<_, Option<String>>(3)?,
                        ))
                    },
                )
                .optional()?;
            let Some((step_id, number, status_s, object_id)) = raw else {
                return Ok(None);
            };
            let status = match status_s.as_str() {
                "sending" => GitHubActionStatus::Sending,
                "done" => GitHubActionStatus::Done,
                other => {
                    return Err(StorageError::Validation(format!(
                        "invalid github action status: {other}"
                    )));
                }
            };
            let object_keys = object_id
                .as_deref()
                .filter(|s| !s.is_empty())
                .map_or_else(Vec::new, |s| s.split('\n').map(str::to_string).collect());
            Ok(Some(GitHubActionRecord {
                step_id,
                number: number.map(|n| n.max(1) as u64),
                status,
                object_keys,
            }))
        })
    }

    pub fn start_action(&self, action: &NewGitHubAction, at_ms: i64) -> StorageResult<()> {
        if !WORKFLOW_GITHUB_ACTIONS.contains(&action.action.as_str()) {
            return Err(StorageError::Validation(format!(
                "unknown github action: {}",
                action.action
            )));
        }
        let at = ms_to_iso(at_ms);
        let obj_id = (!action.object_keys.is_empty()).then(|| action.object_keys.join("\n"));
        self.db.with_conn(|conn| {
            conn.execute(
                "INSERT INTO workflow_github_actions (action_key, workflow_id, run_id, step_id, repository, number, \
                   action, status, object_id, created_at, updated_at) \
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, 'sending', ?8, ?9, ?10) \
                 ON CONFLICT(action_key) DO UPDATE SET step_id = excluded.step_id, number = excluded.number, \
                   status = 'sending', object_id = excluded.object_id, updated_at = excluded.updated_at",
                params![
                    action.action_key,
                    action.workflow_id,
                    action.run_id,
                    action.step_id,
                    action.repository,
                    action.number.map(|n| n as i64),
                    action.action,
                    obj_id,
                    at,
                    at,
                ],
            )?;
            Ok(())
        })
    }

    pub fn finish_action(
        &self,
        action_key: &str,
        number: Option<u64>,
        object_keys: &[String],
        at_ms: i64,
    ) -> StorageResult<()> {
        let at = ms_to_iso(at_ms);
        let obj_id = (!object_keys.is_empty()).then(|| object_keys.join("\n"));
        self.db.with_conn(|conn| {
            conn.execute(
                "UPDATE workflow_github_actions \
                 SET status = 'done', number = coalesce(?1, number), object_id = ?2, updated_at = ?3 \
                 WHERE action_key = ?4",
                params![number.map(|n| n as i64), obj_id, at, action_key],
            )?;
            Ok(())
        })
    }

    pub fn drop_action(&self, action_key: &str) -> StorageResult<()> {
        self.db.with_conn(|conn| {
            conn.execute(
                "DELETE FROM workflow_github_actions WHERE action_key = ?1",
                params![action_key],
            )?;
            Ok(())
        })
    }

    pub fn next_waiting(
        &self,
        workflow_id: &str,
        repository: &str,
        number: u64,
    ) -> StorageResult<Option<WaitingGitHubDelivery>> {
        self.db.with_conn(|conn| {
            conn.query_row(
                &format!(
                    "{WAITING_SELECT_SQL} AND d.repository = ?2 AND d.number = ?3 \
                     ORDER BY d.received_at, d.delivery_id LIMIT 1"
                ),
                params![workflow_id, repository, number as i64],
                read_waiting_row,
            )
            .optional()
            .map_err(StorageError::from)
        })
    }

    pub fn waiting(&self, workflow_id: &str) -> StorageResult<Vec<WaitingGitHubDelivery>> {
        self.db.with_conn(|conn| {
            let mut stmt = conn.prepare(&format!(
                "{WAITING_SELECT_SQL} ORDER BY d.received_at, d.delivery_id"
            ))?;
            let rows = stmt.query_map(params![workflow_id], read_waiting_row)?;
            let mut seen = HashSet::new();
            let mut out = Vec::new();
            for r in rows {
                let d = r?;
                let key = format!("{}#{}", d.item.repository, d.item.number);
                if seen.insert(key) {
                    out.push(d);
                }
            }
            Ok(out)
        })
    }

    pub fn waiting_workflows(&self) -> StorageResult<Vec<String>> {
        self.db.with_conn(|conn| {
            let mut stmt = conn.prepare(
                "SELECT DISTINCT workflow_id FROM workflow_github_deliveries WHERE outcome = 'waiting'",
            )?;
            let rows = stmt.query_map([], |r| r.get::<_, String>(0))?;
            Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
        })
    }

    pub fn end_waiting(
        &self,
        workflow_id: &str,
        node_id: &str,
        delivery_id: &str,
        end: &WaitingGitHubEnd,
    ) -> StorageResult<()> {
        let (outcome, reason, run_id) = match end {
            WaitingGitHubEnd::Started { run_id } => ("started", None, Some(run_id.as_str())),
            WaitingGitHubEnd::Skipped { reason } => ("skipped", Some(reason.as_str()), None),
        };
        self.db.with_tx(|conn| {
            conn.execute(
                "UPDATE workflow_github_deliveries SET outcome = ?1, reason = ?2, run_id = ?3, payload = NULL \
                 WHERE workflow_id = ?4 AND node_id = ?5 AND delivery_id = ?6 AND outcome = 'waiting'",
                params![outcome, reason, run_id, workflow_id, node_id, delivery_id],
            )?;
            prune_github_deliveries_on_conn(conn, workflow_id, node_id)
        })
    }

    pub fn history(&self, workflow_id: &str) -> StorageResult<WorkflowGitHubHistory> {
        self.db.with_conn(|conn| {
            let mut d_stmt = conn.prepare(
                "SELECT node_id, delivery_id, event, repository, number, pull_request, title, received_at, \
                   outcome, reason, run_id FROM workflow_github_deliveries WHERE workflow_id = ?1 \
                 ORDER BY received_at DESC, delivery_id DESC",
            )?;
            let d_rows = d_stmt.query_map(params![workflow_id], |r| {
                let node_id: String = r.get(0)?;
                let id: String = r.get(1)?;
                let event: String = r.get(2)?;
                let repo: Option<String> = r.get(3)?;
                let num: Option<i64> = r.get(4)?;
                let pr: Option<i64> = r.get(5)?;
                let title: Option<String> = r.get(6)?;
                let received_at: String = r.get(7)?;
                let outcome_s: String = r.get(8)?;
                let reason_s: Option<String> = r.get(9)?;
                let run_id: Option<String> = r.get(10)?;
                Ok((node_id, id, event, repo, num, pr, title, received_at, outcome_s, reason_s, run_id))
            })?;
            let mut deliveries = Vec::new();
            for r in d_rows {
                let (node_id, id, event, repo, num, pr, title, received_at, outcome_s, reason_s, run_id) = r?;
                let outcome = match outcome_s.as_str() {
                    "started" => WorkflowGitHubDeliveryOutcome::Started,
                    "waiting" => WorkflowGitHubDeliveryOutcome::Waiting,
                    "skipped" => WorkflowGitHubDeliveryOutcome::Skipped,
                    other => {
                        return Err(StorageError::Validation(format!(
                            "invalid github delivery outcome: {other}"
                        )));
                    }
                };
                let reason = match reason_s.as_deref() {
                    None => None,
                    Some("filter") => Some(WorkflowGitHubSkipReason::Filter),
                    Some("self") => Some(WorkflowGitHubSkipReason::SelfCaused),
                    Some("chain") => Some(WorkflowGitHubSkipReason::Chain),
                    Some("offline") => Some(WorkflowGitHubSkipReason::Offline),
                    Some("refused") => Some(WorkflowGitHubSkipReason::Refused),
                    Some(other) => {
                        return Err(StorageError::Validation(format!(
                            "invalid github skip reason: {other}"
                        )));
                    }
                };
                let item = match (repo, num, pr, title) {
                    (Some(repository), Some(number), Some(pull_request), Some(title)) => {
                        Some(WorkflowGitHubItem {
                            repository,
                            number: number.max(1) as u64,
                            pull_request: pull_request == 1,
                            title,
                        })
                    }
                    _ => None,
                };
                deliveries.push(WorkflowGitHubDelivery {
                    id,
                    node_id,
                    event,
                    item,
                    received_at,
                    outcome,
                    reason,
                    run_id,
                });
            }

            let mut r_stmt = conn.prepare(
                "SELECT g.run_id, g.repository, g.number, g.pull_request, g.title \
                 FROM workflow_github_runs g JOIN workflow_runs r ON r.run_id = g.run_id \
                 WHERE g.workflow_id = ?1 AND (r.status IN ('running', 'approval', 'stopping') OR g.run_id IN ( \
                   SELECT run_id FROM workflow_github_deliveries WHERE workflow_id = ?2 AND run_id IS NOT NULL)) \
                 ORDER BY g.started_at DESC, g.run_id",
            )?;
            let runs = r_stmt
                .query_map(params![workflow_id, workflow_id], |r| {
                    Ok(WorkflowGitHubRunItem {
                        run_id: r.get(0)?,
                        item: WorkflowGitHubItem {
                            repository: r.get(1)?,
                            number: r.get::<_, i64>(2)?.max(1) as u64,
                            pull_request: r.get::<_, i64>(3)? == 1,
                            title: r.get(4)?,
                        },
                    })
                })?
                .collect::<rusqlite::Result<Vec<_>>>()?;

            Ok(WorkflowGitHubHistory { deliveries, runs })
        })
    }
}

fn add_github_delivery_on_conn(conn: &Connection, delivery: &NewGitHubDelivery) -> StorageResult<()> {
    conn.execute(
        "INSERT INTO workflow_github_deliveries (workflow_id, node_id, delivery_id, event, repository, number, \
           pull_request, title, received_at, outcome, reason, run_id, payload, caused_by_bot) \
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14) \
         ON CONFLICT(workflow_id, node_id, delivery_id) DO NOTHING",
        params![
            delivery.workflow_id,
            delivery.node_id,
            delivery.delivery_id,
            delivery.event,
            delivery.item.as_ref().map(|i| i.repository.as_str()),
            delivery.item.as_ref().map(|i| i.number as i64),
            delivery.item.as_ref().map(|i| i64::from(i.pull_request)),
            delivery.item.as_ref().map(|i| i.title.as_str()),
            delivery.received_at,
            delivery.outcome.as_str(),
            delivery.reason.map(WorkflowGitHubSkipReason::as_str),
            delivery.run_id,
            delivery.payload,
            i64::from(delivery.caused_by_bot),
        ],
    )?;
    Ok(())
}

fn prune_github_deliveries_on_conn(
    conn: &Connection,
    workflow_id: &str,
    node_id: &str,
) -> StorageResult<()> {
    conn.execute(
        "DELETE FROM workflow_github_deliveries \
         WHERE workflow_id = ?1 AND node_id = ?2 AND outcome <> 'waiting' AND delivery_id NOT IN ( \
           SELECT delivery_id FROM workflow_github_deliveries WHERE workflow_id = ?3 AND node_id = ?4 \
           ORDER BY received_at DESC, delivery_id DESC LIMIT ?5)",
        params![
            workflow_id,
            node_id,
            workflow_id,
            node_id,
            WORKFLOW_GITHUB_DELIVERY_LIMIT as i64
        ],
    )?;
    Ok(())
}

fn read_waiting_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<WaitingGitHubDelivery> {
    Ok(WaitingGitHubDelivery {
        workflow_id: row.get(0)?,
        node_id: row.get(1)?,
        delivery_id: row.get(2)?,
        item: WorkflowGitHubItem {
            repository: row.get(3)?,
            number: row.get::<_, i64>(4)?.max(1) as u64,
            pull_request: row.get::<_, i64>(5)? == 1,
            title: row.get(6)?,
        },
        payload: row.get(7)?,
        caused_by_bot: row.get::<_, i64>(8)? == 1,
    })
}

// ---------------------------------------------------------------------------
// WebhookRouteStore (`projection_webhook_route_revocations`, `projection_webhook_receipts`,
// `projection_routine_webhooks`, `projection_channel_routine_webhooks`, `workflow_trigger_webhooks`)
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "lowercase")]
pub enum WebhookRouteOwner {
    Agent { id: String },
    Channel { id: String },
    Workflow { id: String },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WebhookRoute {
    pub route_id: String,
    pub owner: WebhookRouteOwner,
    pub routine_id: String,
    pub url: Option<String>,
    pub secret_ciphertext: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EventActivity {
    pub kind: String,
    pub id: String,
    pub delivery_id: String,
    pub event_type: String,
    pub status: String,
    pub reason: Option<String>,
    pub run_id: Option<String>,
    pub occurred_at: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RoutineWebhookReceiptInput<'a> {
    pub owner_kind: &'a str,
    pub routine_id: &'a str,
    pub delivery_id: &'a str,
    pub event_type: &'a str,
    pub status: &'a str,
    pub reason: Option<&'a str>,
    pub run_id: Option<&'a str>,
    pub received_at: &'a str,
}

const ROUTE_SELECT_SQL: &str = "\
  SELECT webhook.route_id, 'agent' AS owner_kind, routine.agent_id AS owner_id, routine.routine_id, \
         webhook.url, webhook.secret_ciphertext \
  FROM projection_routine_webhooks webhook \
  JOIN projection_agent_routines routine ON routine.routine_id = webhook.routine_id \
  UNION ALL \
  SELECT webhook.route_id, 'channel' AS owner_kind, routine.channel_id AS owner_id, routine.routine_id, \
         webhook.url, webhook.secret_ciphertext \
  FROM projection_channel_routine_webhooks webhook \
  JOIN projection_channel_routines routine ON routine.routine_id = webhook.routine_id \
  UNION ALL \
  SELECT route_id, 'workflow' AS owner_kind, workflow_id AS owner_id, node_id AS routine_id, \
         url, secret_ciphertext \
  FROM workflow_trigger_webhooks";

const ROUTE_TABLES: [&str; 3] = [
    "projection_routine_webhooks",
    "projection_channel_routine_webhooks",
    "workflow_trigger_webhooks",
];

#[derive(Clone)]
pub struct WebhookRouteStore {
    db: OpenBotDatabase,
}

impl WebhookRouteStore {
    pub fn new(db: OpenBotDatabase) -> Self {
        Self { db }
    }

    pub fn find(&self, route_id: &str) -> StorageResult<Option<WebhookRoute>> {
        if !is_identifier(route_id) {
            return Ok(None);
        }
        self.db.with_conn(|conn| {
            conn.query_row(
                &format!("SELECT * FROM ({ROUTE_SELECT_SQL}) WHERE route_id = ?1"),
                params![route_id],
                read_webhook_route_row,
            )
            .optional()?
            .transpose()
        })
    }

    pub fn list(&self) -> StorageResult<Vec<WebhookRoute>> {
        self.db.with_conn(|conn| {
            let mut stmt = conn.prepare(&format!("{ROUTE_SELECT_SQL} ORDER BY route_id"))?;
            let rows = stmt.query_map([], read_webhook_route_row)?;
            let mut out = Vec::new();
            for r in rows {
                out.push(r??);
            }
            Ok(out)
        })
    }

    pub fn set_url(&self, route_id: &str, url: Option<&str>) -> StorageResult<()> {
        if let Some(u) = url
            && !is_bounded_string(u, INPUT_LIMITS.browser_url)
        {
            return Err(StorageError::Validation("webhook url too long".to_string()));
        }
        self.db.with_tx(|conn| {
            for table in ROUTE_TABLES {
                conn.execute(
                    &format!("UPDATE {table} SET url = ?1 WHERE route_id = ?2"),
                    params![url, route_id],
                )?;
            }
            Ok(())
        })
    }

    pub fn replace_route_id(
        &self,
        route_id: &str,
        now_ms: Option<i64>,
    ) -> StorageResult<Option<String>> {
        let replacement = Uuid::new_v4().to_string();
        let at = ms_to_iso(now_ms.unwrap_or_else(now_millis));
        self.db.with_tx(|conn| {
            for table in ROUTE_TABLES {
                let changes = conn.execute(
                    &format!(
                        "UPDATE {table} SET route_id = ?1, url = NULL, updated_at = ?2 WHERE route_id = ?3"
                    ),
                    params![replacement, at, route_id],
                )?;
                if changes > 0 {
                    return Ok(Some(replacement));
                }
            }
            Ok(None)
        })
    }

    pub fn rotate_secret(
        &self,
        owner: &EventRoutineOwner,
        routine_id: &str,
        secret_ciphertext: &str,
        now_ms: Option<i64>,
    ) -> StorageResult<()> {
        let at = ms_to_iso(now_ms.unwrap_or_else(now_millis));
        let (routine_table, owner_col, webhook_table, owner_id) = match owner {
            EventRoutineOwner::Agent { id } => (
                "projection_agent_routines",
                "agent_id",
                "projection_routine_webhooks",
                id.as_str(),
            ),
            EventRoutineOwner::Channel { id } => (
                "projection_channel_routines",
                "channel_id",
                "projection_channel_routine_webhooks",
                id.as_str(),
            ),
        };
        self.db.with_conn(|conn| {
            let changes = conn.execute(
                &format!(
                    "UPDATE {webhook_table} SET secret_ciphertext = ?1, updated_at = ?2 \
                     WHERE routine_id = ?3 AND routine_id IN \
                       (SELECT routine_id FROM {routine_table} WHERE {owner_col} = ?4)"
                ),
                params![secret_ciphertext, at, routine_id, owner_id],
            )?;
            if changes == 0 {
                return Err(StorageError::NotFound(format!("routine {routine_id} gone")));
            }
            Ok(())
        })
    }

    pub fn has_workflow_route(&self, workflow_id: &str, node_id: &str) -> StorageResult<bool> {
        self.db.with_conn(|conn| {
            let found: Option<i64> = conn
                .query_row(
                    "SELECT 1 FROM workflow_trigger_webhooks WHERE workflow_id = ?1 AND node_id = ?2",
                    params![workflow_id, node_id],
                    |r| r.get(0),
                )
                .optional()?;
            Ok(found.is_some())
        })
    }

    pub fn rotate_workflow_secret(
        &self,
        workflow_id: &str,
        node_id: &str,
        secret_ciphertext: &str,
        now_ms: Option<i64>,
    ) -> StorageResult<bool> {
        let at = ms_to_iso(now_ms.unwrap_or_else(now_millis));
        let route_id = Uuid::new_v4().to_string();
        self.db.with_conn(|conn| {
            let changes = conn.execute(
                "INSERT INTO workflow_trigger_webhooks \
                   (workflow_id, node_id, route_id, secret_ciphertext, url, created_at, updated_at) \
                 SELECT workflow_id, node_id, ?1, ?2, NULL, ?3, ?4 FROM workflow_triggers \
                 WHERE workflow_id = ?5 AND node_id = ?6 AND kind = 'webhook' \
                 ON CONFLICT(workflow_id, node_id) DO UPDATE SET secret_ciphertext = excluded.secret_ciphertext, \
                   updated_at = excluded.updated_at",
                params![route_id, secret_ciphertext, at, at, workflow_id, node_id],
            )?;
            Ok(changes > 0)
        })
    }

    pub fn pending_revocations(&self) -> StorageResult<Vec<String>> {
        self.db.with_conn(|conn| {
            let mut stmt = conn.prepare(
                "SELECT route_id FROM projection_webhook_route_revocations ORDER BY created_at, route_id",
            )?;
            let rows = stmt.query_map([], |r| r.get::<_, String>(0))?;
            Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
        })
    }

    pub fn clear_revocation(&self, route_id: &str) -> StorageResult<()> {
        self.db.with_conn(|conn| {
            conn.execute(
                "DELETE FROM projection_webhook_route_revocations WHERE route_id = ?1",
                params![route_id],
            )?;
            Ok(())
        })
    }

    pub fn list_receipts(
        &self,
        owner_kind: &str,
        routine_id: &str,
        limit: usize,
    ) -> StorageResult<Vec<EventActivity>> {
        self.db.with_conn(|conn| {
            let mut stmt = conn.prepare(
                "SELECT receipt_id, delivery_id, event_type, status, reason, run_id, received_at \
                 FROM projection_webhook_receipts WHERE owner_kind = ?1 AND routine_id = ?2 \
                 ORDER BY received_at DESC, receipt_id LIMIT ?3",
            )?;
            let rows = stmt.query_map(params![owner_kind, routine_id, limit as i64], |r| {
                Ok(EventActivity {
                    kind: "received".to_string(),
                    id: r.get(0)?,
                    delivery_id: r.get(1)?,
                    event_type: r.get(2)?,
                    status: r.get(3)?,
                    reason: r.get(4)?,
                    run_id: r.get(5)?,
                    occurred_at: r.get(6)?,
                })
            })?;
            Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
        })
    }
}

fn read_webhook_route_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<StorageResult<WebhookRoute>> {
    let route_id: String = row.get(0)?;
    let owner_kind: String = row.get(1)?;
    let owner_id: String = row.get(2)?;
    let routine_id: String = row.get(3)?;
    let url: Option<String> = row.get(4)?;
    let secret_ciphertext: String = row.get(5)?;
    let owner = match owner_kind.as_str() {
        "agent" => WebhookRouteOwner::Agent { id: owner_id },
        "channel" => WebhookRouteOwner::Channel { id: owner_id },
        "workflow" => WebhookRouteOwner::Workflow { id: owner_id },
        other => {
            return Ok(Err(StorageError::Validation(format!(
                "invalid webhook route owner kind: {other}"
            ))));
        }
    };
    Ok(Ok(WebhookRoute {
        route_id,
        owner,
        routine_id,
        url,
        secret_ciphertext,
    }))
}

pub fn has_routine_webhook_receipt(
    conn: &Connection,
    owner_kind: &str,
    routine_id: &str,
    delivery_id: &str,
    received_at: &str,
) -> StorageResult<bool> {
    let cutoff = webhook_receipt_cutoff(received_at)?;
    let found: Option<i64> = conn
        .query_row(
            "SELECT 1 FROM projection_webhook_receipts \
             WHERE owner_kind = ?1 AND routine_id = ?2 AND delivery_id = ?3 AND received_at >= ?4",
            params![owner_kind, routine_id, delivery_id, cutoff],
            |r| r.get(0),
        )
        .optional()?;
    Ok(found.is_some())
}

pub fn insert_routine_webhook_receipt(
    conn: &Connection,
    receipt: &RoutineWebhookReceiptInput<'_>,
) -> StorageResult<()> {
    let cutoff = webhook_receipt_cutoff(receipt.received_at)?;
    conn.execute(
        "DELETE FROM projection_webhook_receipts WHERE received_at < ?1",
        params![cutoff],
    )?;
    conn.execute(
        "INSERT INTO projection_webhook_receipts \
           (receipt_id, owner_kind, routine_id, delivery_id, event_type, status, reason, run_id, received_at) \
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
        params![
            Uuid::new_v4().to_string(),
            receipt.owner_kind,
            receipt.routine_id,
            receipt.delivery_id,
            receipt.event_type,
            receipt.status,
            receipt.reason,
            receipt.run_id,
            receipt.received_at,
        ],
    )?;
    Ok(())
}

pub fn revoke_routine_webhooks(
    conn: &Connection,
    owner_kind: &str,
    routine_ids: &[&str],
    forget_receipts: bool,
    now_ms: Option<i64>,
) -> StorageResult<()> {
    let webhook_table = match owner_kind {
        "agent" => "projection_routine_webhooks",
        "channel" => "projection_channel_routine_webhooks",
        other => {
            return Err(StorageError::Validation(format!(
                "invalid routine webhook owner kind: {other}"
            )));
        }
    };
    let at = ms_to_iso(now_ms.unwrap_or_else(now_millis));
    for &rid in routine_ids {
        conn.execute(
            &format!(
                "INSERT OR IGNORE INTO projection_webhook_route_revocations (route_id, created_at) \
                 SELECT route_id, ?1 FROM {webhook_table} WHERE routine_id = ?2"
            ),
            params![at, rid],
        )?;
        conn.execute(
            &format!("DELETE FROM {webhook_table} WHERE routine_id = ?1"),
            params![rid],
        )?;
        if forget_receipts {
            conn.execute(
                "DELETE FROM projection_webhook_receipts WHERE owner_kind = ?1 AND routine_id = ?2",
                params![owner_kind, rid],
            )?;
        }
    }
    Ok(())
}
