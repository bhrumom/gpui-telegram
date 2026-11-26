//! Persistent Diff Annotations, Local Per-Line AI-vs-Human Attribution,
//! Conventional Commit / Stacked PR Draft Composer, and Multi-Forge Tasks Hub (`F17`).

use std::collections::BTreeMap;
use std::fmt;
use std::fs;
use std::path::Path;

use orca_core::orchestration_protocol::{
    DiffAnnotation, DiffAnnotationStore, DiffSide, FileAttributionMap, LineAttribution,
};
use parking_lot::Mutex;
use rusqlite::{Connection, OptionalExtension, params};
use serde::{Deserialize, Serialize};

use crate::{OrcaIdeRuntimeError, Result};

const DIFF_REVIEW_SCHEMA_SQL: &str = r#"
CREATE TABLE IF NOT EXISTS diff_annotations (
    annotation_id TEXT PRIMARY KEY,
    file_path TEXT NOT NULL,
    side TEXT NOT NULL,
    line_start INTEGER NOT NULL,
    line_end INTEGER NOT NULL,
    body TEXT NOT NULL,
    author_json TEXT NOT NULL,
    resolved INTEGER NOT NULL DEFAULT 0,
    created_at_ms INTEGER NOT NULL,
    updated_at_ms INTEGER NOT NULL
);

CREATE INDEX IF NOT EXISTS idx_diff_annotations_file_line
    ON diff_annotations(file_path, line_start ASC);

CREATE TABLE IF NOT EXISTS line_attributions (
    file_path TEXT NOT NULL,
    line_number INTEGER NOT NULL,
    attribution_json TEXT NOT NULL,
    updated_at_ms INTEGER NOT NULL,
    PRIMARY KEY (file_path, line_number)
);
"#;

const TASKS_HUB_SCHEMA_SQL: &str = r#"
CREATE TABLE IF NOT EXISTS hub_tasks (
    task_id TEXT PRIMARY KEY,
    forge TEXT NOT NULL,
    external_key TEXT NOT NULL,
    title TEXT NOT NULL,
    description TEXT NOT NULL DEFAULT '',
    status TEXT NOT NULL,
    assignee TEXT,
    labels_json TEXT NOT NULL DEFAULT '[]',
    artifact_url TEXT NOT NULL,
    repo_or_project TEXT NOT NULL DEFAULT '',
    updated_at_ms INTEGER NOT NULL
);

CREATE UNIQUE INDEX IF NOT EXISTS idx_hub_tasks_forge_key
    ON hub_tasks(forge, external_key);

CREATE INDEX IF NOT EXISTS idx_hub_tasks_updated
    ON hub_tasks(updated_at_ms DESC);
"#;

fn diff_side_to_str(side: DiffSide) -> &'static str {
    match side {
        DiffSide::Left => "left",
        DiffSide::Right => "right",
        DiffSide::Unified => "unified",
    }
}

fn str_to_diff_side(raw: &str) -> DiffSide {
    match raw {
        "left" => DiffSide::Left,
        "right" => DiffSide::Right,
        _ => DiffSide::Unified,
    }
}

/// Local attribution export payload (strictly local DB metadata; zero git commit pollution).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AttributionExport {
    pub files: BTreeMap<String, BTreeMap<u32, LineAttribution>>,
    pub human_lines: usize,
    pub ai_lines: usize,
    pub unknown_lines: usize,
}

struct DiffReviewInner {
    conn: Connection,
    annotations: DiffAnnotationStore,
    attributions: FileAttributionMap,
}

/// Persistent SQLite WAL store for diff review annotations and local per-line
/// AI-vs-Human provenance (`Send + Sync`).
pub struct PersistentDiffReviewStore {
    inner: Mutex<DiffReviewInner>,
}

impl fmt::Debug for PersistentDiffReviewStore {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let guard = self.inner.lock();
        f.debug_struct("PersistentDiffReviewStore")
            .field("annotations", &guard.annotations.annotations.len())
            .field("attributed_files", &guard.attributions.by_file.len())
            .finish()
    }
}

fn init_diff_review_inner(conn: Connection) -> Result<DiffReviewInner> {
    conn.execute_batch("PRAGMA journal_mode = WAL; PRAGMA foreign_keys = ON;")?;
    conn.execute_batch(DIFF_REVIEW_SCHEMA_SQL)?;

    let mut annotations = DiffAnnotationStore::default();
    {
        let mut stmt = conn.prepare(
            "SELECT annotation_id, file_path, side, line_start, line_end, body,
                    author_json, resolved, created_at_ms, updated_at_ms
             FROM diff_annotations",
        )?;
        let rows = stmt.query_map([], |r| {
            let side_raw: String = r.get(2)?;
            let author_raw: String = r.get(6)?;
            let author = serde_json::from_str(&author_raw).unwrap_or(LineAttribution::Unknown);
            Ok(DiffAnnotation {
                annotation_id: r.get(0)?,
                file_path: r.get(1)?,
                side: str_to_diff_side(&side_raw),
                line_start: r.get::<_, i64>(3)?.max(1) as u32,
                line_end: r.get::<_, i64>(4)?.max(1) as u32,
                body: r.get(5)?,
                author,
                resolved: r.get::<_, i64>(7)? != 0,
                created_at_ms: r.get(8)?,
                updated_at_ms: r.get(9)?,
            })
        })?;
        for row in rows {
            let ann = row?;
            annotations
                .annotations
                .insert(ann.annotation_id.clone(), ann);
        }
    }

    let mut attributions = FileAttributionMap::default();
    {
        let mut stmt = conn.prepare(
            "SELECT file_path, line_number, attribution_json FROM line_attributions",
        )?;
        let rows = stmt.query_map([], |r| {
            let attr_raw: String = r.get(2)?;
            let attr = serde_json::from_str(&attr_raw).unwrap_or(LineAttribution::Unknown);
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, i64>(1)?.max(1) as u32,
                attr,
            ))
        })?;
        for row in rows {
            let (file_path, line_no, attr) = row?;
            attributions
                .by_file
                .entry(file_path)
                .or_default()
                .insert(line_no, attr);
        }
    }

    Ok(DiffReviewInner {
        conn,
        annotations,
        attributions,
    })
}

impl PersistentDiffReviewStore {
    pub fn open(path: impl AsRef<Path>) -> Result<Self> {
        let path = path.as_ref();
        if let Some(parent) = path.parent()
            && !parent.as_os_str().is_empty()
        {
            fs::create_dir_all(parent)?;
        }
        let conn = Connection::open(path)?;
        Ok(Self {
            inner: Mutex::new(init_diff_review_inner(conn)?),
        })
    }

    pub fn in_memory() -> Result<Self> {
        let conn = Connection::open_in_memory()?;
        Ok(Self {
            inner: Mutex::new(init_diff_review_inner(conn)?),
        })
    }

    /// Persists a new line-anchored diff annotation validated by `DiffAnnotationStore`.
    pub fn add_annotation(&self, annotation: DiffAnnotation) -> Result<DiffAnnotation> {
        let mut guard = self.inner.lock();
        guard.annotations.add(annotation.clone())?;
        let author_json = serde_json::to_string(&annotation.author)?;
        if let Err(err) = guard.conn.execute(
            "INSERT INTO diff_annotations
                (annotation_id, file_path, side, line_start, line_end, body, author_json, resolved, created_at_ms, updated_at_ms)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
            params![
                &annotation.annotation_id,
                &annotation.file_path,
                diff_side_to_str(annotation.side),
                annotation.line_start as i64,
                annotation.line_end as i64,
                &annotation.body,
                &author_json,
                i64::from(annotation.resolved),
                annotation.created_at_ms,
                annotation.updated_at_ms,
            ],
        ) {
            guard.annotations.annotations.remove(&annotation.annotation_id);
            return Err(OrcaIdeRuntimeError::Sqlite(err));
        }
        Ok(annotation)
    }

    /// Lists diff annotations (optionally scoped to one file, optionally including resolved).
    pub fn list_annotations(
        &self,
        file_path: Option<&str>,
        include_resolved: bool,
    ) -> Vec<DiffAnnotation> {
        let guard = self.inner.lock();
        if let Some(fp) = file_path {
            return guard
                .annotations
                .filter_by_file(fp, include_resolved)
                .into_iter()
                .cloned()
                .collect();
        }
        let mut out: Vec<DiffAnnotation> = guard
            .annotations
            .annotations
            .values()
            .filter(|a| include_resolved || !a.resolved)
            .cloned()
            .collect();
        out.sort_by(|a, b| {
            a.file_path
                .cmp(&b.file_path)
                .then_with(|| a.line_start.cmp(&b.line_start))
                .then_with(|| a.annotation_id.cmp(&b.annotation_id))
        });
        out
    }

    pub fn resolve_annotation(&self, annotation_id: &str, now_ms: i64) -> Result<DiffAnnotation> {
        let mut guard = self.inner.lock();
        guard.annotations.resolve(annotation_id, now_ms)?;
        guard.conn.execute(
            "UPDATE diff_annotations SET resolved = 1, updated_at_ms = ?1 WHERE annotation_id = ?2",
            params![now_ms, annotation_id],
        )?;
        guard
            .annotations
            .annotations
            .get(annotation_id)
            .cloned()
            .ok_or_else(|| OrcaIdeRuntimeError::not_found(format!("annotation '{annotation_id}'")))
    }

    pub fn reopen_annotation(&self, annotation_id: &str, now_ms: i64) -> Result<DiffAnnotation> {
        let mut guard = self.inner.lock();
        guard.annotations.reopen(annotation_id, now_ms)?;
        guard.conn.execute(
            "UPDATE diff_annotations SET resolved = 0, updated_at_ms = ?1 WHERE annotation_id = ?2",
            params![now_ms, annotation_id],
        )?;
        guard
            .annotations
            .annotations
            .get(annotation_id)
            .cloned()
            .ok_or_else(|| OrcaIdeRuntimeError::not_found(format!("annotation '{annotation_id}'")))
    }

    /// Re-anchors persisted annotations at or below `at_line` when edits insert/delete lines.
    pub fn apply_line_shift(
        &self,
        file_path: &str,
        side: DiffSide,
        at_line: u32,
        delta: i32,
        now_ms: i64,
    ) -> Result<Vec<DiffAnnotation>> {
        let mut guard = self.inner.lock();
        guard
            .annotations
            .apply_line_shift(file_path, side, at_line, delta);
        let updated: Vec<DiffAnnotation> = guard
            .annotations
            .annotations
            .values_mut()
            .filter(|a| a.file_path == file_path && a.side == side)
            .map(|a| {
                a.updated_at_ms = now_ms;
                a.clone()
            })
            .collect();
        let tx = guard.conn.transaction()?;
        for ann in &updated {
            tx.execute(
                "UPDATE diff_annotations
                 SET line_start = ?1, line_end = ?2, updated_at_ms = ?3
                 WHERE annotation_id = ?4",
                params![
                    ann.line_start as i64,
                    ann.line_end as i64,
                    now_ms,
                    &ann.annotation_id
                ],
            )?;
        }
        tx.commit()?;
        Ok(updated)
    }

    /// Formats all unresolved annotations into a single coherent `"Send to agent"` batch prompt.
    pub fn build_agent_batch_prompt(&self, file_path: Option<&str>) -> String {
        self.inner
            .lock()
            .annotations
            .build_agent_batch_prompt(file_path)
    }

    /// Records AI (or worker/bot) provenance for `line_start..=line_end` in the local DB only.
    pub fn record_ai_range(
        &self,
        file_path: &str,
        line_start: u32,
        line_end: u32,
        attribution: LineAttribution,
        now_ms: i64,
    ) -> Result<()> {
        if file_path.trim().is_empty() || line_start == 0 || line_end < line_start {
            return Err(OrcaIdeRuntimeError::invalid_argument(format!(
                "invalid attribution range '{file_path}': {line_start}..={line_end}"
            )));
        }
        let mut guard = self.inner.lock();
        guard
            .attributions
            .record_range(file_path, line_start, line_end, attribution.clone());
        let attr_json = serde_json::to_string(&attribution)?;
        let tx = guard.conn.transaction()?;
        for line in line_start..=line_end {
            tx.execute(
                "INSERT INTO line_attributions (file_path, line_number, attribution_json, updated_at_ms)
                 VALUES (?1, ?2, ?3, ?4)
                 ON CONFLICT(file_path, line_number) DO UPDATE SET
                    attribution_json = excluded.attribution_json,
                    updated_at_ms = excluded.updated_at_ms",
                params![file_path, line as i64, &attr_json, now_ms],
            )?;
        }
        tx.commit()?;
        Ok(())
    }

    /// Flips lines edited by a human back to [`LineAttribution::Human`] in local DB only.
    pub fn record_human_edit(
        &self,
        file_path: &str,
        line_start: u32,
        line_end: u32,
        now_ms: i64,
    ) -> Result<()> {
        if file_path.trim().is_empty() || line_start == 0 || line_end < line_start {
            return Err(OrcaIdeRuntimeError::invalid_argument(format!(
                "invalid human edit range '{file_path}': {line_start}..={line_end}"
            )));
        }
        let mut guard = self.inner.lock();
        guard
            .attributions
            .record_human_edit(file_path, line_start, line_end);
        let attr_json = serde_json::to_string(&LineAttribution::Human)?;
        let tx = guard.conn.transaction()?;
        for line in line_start..=line_end {
            tx.execute(
                "INSERT INTO line_attributions (file_path, line_number, attribution_json, updated_at_ms)
                 VALUES (?1, ?2, ?3, ?4)
                 ON CONFLICT(file_path, line_number) DO UPDATE SET
                    attribution_json = excluded.attribution_json,
                    updated_at_ms = excluded.updated_at_ms",
                params![file_path, line as i64, &attr_json, now_ms],
            )?;
        }
        tx.commit()?;
        Ok(())
    }

    pub fn file_attribution(&self, file_path: &str) -> BTreeMap<u32, LineAttribution> {
        self.inner
            .lock()
            .attributions
            .by_file
            .get(file_path)
            .cloned()
            .unwrap_or_default()
    }

    pub fn line_attribution(&self, file_path: &str, line: u32) -> LineAttribution {
        self.inner
            .lock()
            .attributions
            .line_attribution(file_path, line)
    }

    /// Exports local per-line attribution metadata (never written to git commits).
    pub fn export_attribution_metadata(&self, file_path: Option<&str>) -> AttributionExport {
        let guard = self.inner.lock();
        let mut files = BTreeMap::new();
        let mut human_lines = 0usize;
        let mut ai_lines = 0usize;
        let mut unknown_lines = 0usize;

        for (fp, map) in &guard.attributions.by_file {
            if file_path.is_some_and(|target| target != fp) {
                continue;
            }
            for attr in map.values() {
                match attr {
                    LineAttribution::Human => human_lines += 1,
                    LineAttribution::Bot { .. } | LineAttribution::Worker { .. } => ai_lines += 1,
                    LineAttribution::Unknown => unknown_lines += 1,
                }
            }
            files.insert(fp.clone(), map.clone());
        }

        AttributionExport {
            files,
            human_lines,
            ai_lines,
            unknown_lines,
        }
    }
}

// ---------------------------------------------------------------------------
// AI Commit & Stacked PR Draft Builder
// ---------------------------------------------------------------------------

const CONVENTIONAL_TYPES: [&str; 11] = [
    "feat", "fix", "refactor", "docs", "test", "chore", "perf", "ci", "build", "style", "revert",
];

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DiffFileChange {
    pub file_path: String,
    pub added_lines: usize,
    pub deleted_lines: usize,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub summary: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CommitPrDraftRequest {
    pub commit_type: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scope: Option<String>,
    pub summary: String,
    pub base_ref: String,
    pub head_branch: String,
    #[serde(default)]
    pub files: Vec<DiffFileChange>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stacked_parent_pr: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub breaking_change: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CommitPrDraft {
    pub commit_title: String,
    pub commit_body: String,
    pub pr_title: String,
    pub pr_body_markdown: String,
    pub base_ref: String,
    pub head_branch: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stacked_parent_pr: Option<String>,
}

/// Builds a conventional commit message and stacked-PR-aware pull request draft from
/// file diff digests.
pub fn compose_commit_and_pr_draft(req: &CommitPrDraftRequest) -> Result<CommitPrDraft> {
    let kind = req.commit_type.trim().to_ascii_lowercase();
    if !CONVENTIONAL_TYPES.contains(&kind.as_str()) {
        return Err(OrcaIdeRuntimeError::invalid_argument(format!(
            "unsupported conventional commit_type '{}'",
            req.commit_type
        )));
    }
    let summary = req.summary.trim();
    if summary.is_empty() {
        return Err(OrcaIdeRuntimeError::invalid_argument(
            "commit summary must be non-empty",
        ));
    }
    let base_ref = req.base_ref.trim();
    let head_branch = req.head_branch.trim();
    if base_ref.is_empty() || head_branch.is_empty() {
        return Err(OrcaIdeRuntimeError::invalid_argument(
            "base_ref and head_branch must be non-empty",
        ));
    }

    let bang = if req
        .breaking_change
        .as_deref()
        .is_some_and(|s| !s.trim().is_empty())
    {
        "!"
    } else {
        ""
    };
    let commit_title = match req.scope.as_deref().map(str::trim).filter(|s| !s.is_empty()) {
        Some(scope) => format!("{kind}({scope}){bang}: {summary}"),
        None => format!("{kind}{bang}: {summary}"),
    };

    let mut total_added = 0usize;
    let mut total_deleted = 0usize;
    let mut bullet_lines = Vec::with_capacity(req.files.len());
    for file in &req.files {
        total_added += file.added_lines;
        total_deleted += file.deleted_lines;
        if let Some(desc) = file
            .summary
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
        {
            bullet_lines.push(format!(
                "- `{}` (+{} / -{}): {desc}",
                file.file_path, file.added_lines, file.deleted_lines
            ));
        } else {
            bullet_lines.push(format!(
                "- `{}` (+{} / -{})",
                file.file_path, file.added_lines, file.deleted_lines
            ));
        }
    }

    let mut commit_body_parts = Vec::new();
    if !bullet_lines.is_empty() {
        commit_body_parts.push(bullet_lines.join("\n"));
    }
    if let Some(breaking) = req
        .breaking_change
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
    {
        commit_body_parts.push(format!("BREAKING CHANGE: {breaking}"));
    }
    let commit_body = commit_body_parts.join("\n\n");

    let mut pr_sections = vec![
        format!("## Summary\n\n{summary}"),
        format!(
            "## Stack & Branch Context\n\n- Base ref: `{base_ref}`\n- Head branch: `{head_branch}`"
        ),
    ];
    if let Some(parent_pr) = req
        .stacked_parent_pr
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
    {
        pr_sections.push(format!("- Stacked on parent PR: `{parent_pr}`"));
    }
    pr_sections.push(format!(
        "## Diff Digest ({} file(s), +{total_added} / -{total_deleted})\n\n{}",
        req.files.len(),
        if bullet_lines.is_empty() {
            "- No file diffs attached".to_string()
        } else {
            bullet_lines.join("\n")
        }
    ));
    if let Some(breaking) = req
        .breaking_change
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
    {
        pr_sections.push(format!("## Breaking Changes\n\n{breaking}"));
    }

    Ok(CommitPrDraft {
        commit_title: commit_title.clone(),
        commit_body,
        pr_title: commit_title,
        pr_body_markdown: pr_sections.join("\n\n"),
        base_ref: base_ref.to_string(),
        head_branch: head_branch.to_string(),
        stacked_parent_pr: req.stacked_parent_pr.clone(),
    })
}

// ---------------------------------------------------------------------------
// Multi-Forge Tasks Hub (GitHub, GitLab, Linear, Jira)
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TaskForge {
    GitHub,
    GitLab,
    Linear,
    Jira,
}

impl TaskForge {
    pub const ALL: [Self; 4] = [Self::GitHub, Self::GitLab, Self::Linear, Self::Jira];

    pub const fn as_str(self) -> &'static str {
        match self {
            Self::GitHub => "github",
            Self::GitLab => "gitlab",
            Self::Linear => "linear",
            Self::Jira => "jira",
        }
    }

    pub fn parse(raw: &str) -> Result<Self> {
        match raw.trim().to_ascii_lowercase().as_str() {
            "github" => Ok(Self::GitHub),
            "gitlab" => Ok(Self::GitLab),
            "linear" => Ok(Self::Linear),
            "jira" => Ok(Self::Jira),
            other => Err(OrcaIdeRuntimeError::invalid_argument(format!(
                "unknown task forge '{other}'"
            ))),
        }
    }
}

/// Normalizes forge-specific issue keys (e.g., Jira `"eng-42"` -> `"ENG-42"`,
/// Linear `"plat-7"` -> `"PLAT-7"`).
pub fn normalize_task_key(forge: TaskForge, raw_key: &str) -> Result<String> {
    let trimmed = raw_key.trim();
    if trimmed.is_empty() || trimmed.chars().any(char::is_control) {
        return Err(OrcaIdeRuntimeError::invalid_argument(
            "task external_key must be non-empty without control characters",
        ));
    }
    match forge {
        TaskForge::Jira | TaskForge::Linear => {
            if let Some((prefix, num)) = trimmed.split_once('-') {
                let p = prefix.trim().to_ascii_uppercase();
                let n = num.trim();
                if p.is_empty() || n.is_empty() || !n.chars().all(|c| c.is_ascii_digit()) {
                    return Err(OrcaIdeRuntimeError::invalid_argument(format!(
                        "invalid {:?} issue key '{raw_key}'; expected PREFIX-123",
                        forge
                    )));
                }
                Ok(format!("{p}-{n}"))
            } else {
                Ok(trimmed.to_ascii_uppercase())
            }
        }
        TaskForge::GitHub | TaskForge::GitLab => Ok(trimmed.to_string()),
    }
}

/// Safely expands an `orca.yaml` `issue_command` containing `{{artifact_url}}`,
/// rejecting control characters or shell injection metacharacters inside `artifact_url`.
pub fn expand_issue_command_template(template: &str, artifact_url: &str) -> Result<String> {
    let tpl = template.trim();
    if tpl.is_empty() {
        return Err(OrcaIdeRuntimeError::invalid_argument(
            "issue_command template must be non-empty",
        ));
    }
    let url = artifact_url.trim();
    let valid_scheme = url.starts_with("https://") || url.starts_with("http://");
    let forbidden_char = |c: char| {
        c.is_control()
            || c.is_whitespace()
            || matches!(
                c,
                ';' | '|' | '&' | '`' | '$' | '(' | ')' | '<' | '>' | '\'' | '"' | '\\'
            )
    };
    if !valid_scheme || url.chars().any(forbidden_char) {
        return Err(OrcaIdeRuntimeError::invalid_argument(format!(
            "unsafe artifact_url '{artifact_url}' rejected for issue_command expansion"
        )));
    }
    Ok(tpl.replace("{{artifact_url}}", url))
}

/// Unified multi-forge issue/task item stored in `TasksHubStore`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HubTaskRecord {
    pub task_id: String,
    pub forge: TaskForge,
    pub external_key: String,
    pub title: String,
    #[serde(default)]
    pub description: String,
    pub status: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub assignee: Option<String>,
    #[serde(default)]
    pub labels: Vec<String>,
    pub artifact_url: String,
    #[serde(default)]
    pub repo_or_project: String,
    pub updated_at_ms: i64,
}

/// Filter parameters for [`TasksHubStore::list_tasks`].
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct TasksHubFilter {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub forge: Option<TaskForge>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub status: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub search: Option<String>,
    #[serde(default = "default_tasks_limit")]
    pub limit: usize,
}

const fn default_tasks_limit() -> usize {
    100
}

/// Persistent SQLite WAL multi-forge Tasks Hub unifying GitHub, GitLab, Linear, and Jira (`Send + Sync`).
pub struct TasksHubStore {
    conn: Mutex<Connection>,
}

impl fmt::Debug for TasksHubStore {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("TasksHubStore").finish()
    }
}

fn init_tasks_hub_conn(conn: &Connection) -> Result<()> {
    conn.execute_batch("PRAGMA journal_mode = WAL; PRAGMA foreign_keys = ON;")?;
    conn.execute_batch(TASKS_HUB_SCHEMA_SQL)?;
    Ok(())
}

fn row_to_hub_task(r: &rusqlite::Row<'_>) -> rusqlite::Result<HubTaskRecord> {
    let forge_raw: String = r.get(1)?;
    let forge = TaskForge::parse(&forge_raw).unwrap_or(TaskForge::GitHub);
    let labels_raw: String = r.get(7)?;
    let labels: Vec<String> = serde_json::from_str(&labels_raw).unwrap_or_default();
    Ok(HubTaskRecord {
        task_id: r.get(0)?,
        forge,
        external_key: r.get(2)?,
        title: r.get(3)?,
        description: r.get(4)?,
        status: r.get(5)?,
        assignee: r.get(6)?,
        labels,
        artifact_url: r.get(8)?,
        repo_or_project: r.get(9)?,
        updated_at_ms: r.get(10)?,
    })
}

impl TasksHubStore {
    pub fn open(path: impl AsRef<Path>) -> Result<Self> {
        let path = path.as_ref();
        if let Some(parent) = path.parent()
            && !parent.as_os_str().is_empty()
        {
            fs::create_dir_all(parent)?;
        }
        let conn = Connection::open(path)?;
        init_tasks_hub_conn(&conn)?;
        Ok(Self {
            conn: Mutex::new(conn),
        })
    }

    pub fn in_memory() -> Result<Self> {
        let conn = Connection::open_in_memory()?;
        init_tasks_hub_conn(&conn)?;
        Ok(Self {
            conn: Mutex::new(conn),
        })
    }

    /// Upserts a forge task, normalizing Jira/Linear key casing automatically.
    pub fn upsert_task(&self, mut record: HubTaskRecord) -> Result<HubTaskRecord> {
        if record.task_id.trim().is_empty() || record.title.trim().is_empty() {
            return Err(OrcaIdeRuntimeError::invalid_argument(
                "task_id and title must be non-empty",
            ));
        }
        record.external_key = normalize_task_key(record.forge, &record.external_key)?;
        let labels_json = serde_json::to_string(&record.labels)?;
        let conn = self.conn.lock();
        conn.execute(
            "INSERT INTO hub_tasks
                (task_id, forge, external_key, title, description, status, assignee,
                 labels_json, artifact_url, repo_or_project, updated_at_ms)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)
             ON CONFLICT(task_id) DO UPDATE SET
                forge = excluded.forge,
                external_key = excluded.external_key,
                title = excluded.title,
                description = excluded.description,
                status = excluded.status,
                assignee = excluded.assignee,
                labels_json = excluded.labels_json,
                artifact_url = excluded.artifact_url,
                repo_or_project = excluded.repo_or_project,
                updated_at_ms = excluded.updated_at_ms",
            params![
                &record.task_id,
                record.forge.as_str(),
                &record.external_key,
                &record.title,
                &record.description,
                &record.status,
                &record.assignee,
                &labels_json,
                &record.artifact_url,
                &record.repo_or_project,
                record.updated_at_ms,
            ],
        )?;
        Ok(record)
    }

    pub fn get_task(&self, task_id: &str) -> Result<Option<HubTaskRecord>> {
        let conn = self.conn.lock();
        let row = conn
            .query_row(
                "SELECT task_id, forge, external_key, title, description, status, assignee,
                        labels_json, artifact_url, repo_or_project, updated_at_ms
                 FROM hub_tasks WHERE task_id = ?1",
                params![task_id],
                row_to_hub_task,
            )
            .optional()?;
        Ok(row)
    }

    pub fn list_tasks(&self, filter: &TasksHubFilter) -> Result<Vec<HubTaskRecord>> {
        let conn = self.conn.lock();
        let mut stmt = conn.prepare(
            "SELECT task_id, forge, external_key, title, description, status, assignee,
                    labels_json, artifact_url, repo_or_project, updated_at_ms
             FROM hub_tasks
             ORDER BY updated_at_ms DESC, task_id ASC",
        )?;
        let search_lc = filter
            .search
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::to_ascii_lowercase);
        let status_lc = filter
            .status
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::to_ascii_lowercase);
        let limit = if filter.limit == 0 {
            100
        } else {
            filter.limit.min(500)
        };

        let mut out = Vec::new();
        let rows = stmt.query_map([], row_to_hub_task)?;
        for item in rows {
            let task = item?;
            if let Some(f) = filter.forge
                && task.forge != f
            {
                continue;
            }
            if let Some(ref st) = status_lc
                && task.status.to_ascii_lowercase() != *st
            {
                continue;
            }
            if let Some(ref q) = search_lc {
                let hay = format!(
                    "{} {} {} {}",
                    task.external_key, task.title, task.description, task.repo_or_project
                )
                .to_ascii_lowercase();
                if !hay.contains(q) {
                    continue;
                }
            }
            out.push(task);
            if out.len() >= limit {
                break;
            }
        }
        Ok(out)
    }

    pub fn delete_task(&self, task_id: &str) -> Result<bool> {
        let conn = self.conn.lock();
        let count = conn.execute("DELETE FROM hub_tasks WHERE task_id = ?1", params![task_id])?;
        Ok(count > 0)
    }

    /// Safely expands an `orca.yaml` `issue_command` template with the stored task's `artifact_url`.
    pub fn build_launch_command(&self, task_id: &str, issue_command_template: &str) -> Result<String> {
        let task = self
            .get_task(task_id)?
            .ok_or_else(|| OrcaIdeRuntimeError::not_found(format!("hub task '{task_id}'")))?;
        expand_issue_command_template(issue_command_template, &task.artifact_url)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn assert_send_sync<T: Send + Sync>() {}

    #[test]
    fn diff_review_persists_line_shifts_batch_prompt_and_human_attribution_flip() {
        assert_send_sync::<PersistentDiffReviewStore>();
        assert_send_sync::<TasksHubStore>();

        let dir = TempDir::new().unwrap();
        let db_path = dir.path().join("diff_review.db");
        let store = PersistentDiffReviewStore::open(&db_path).unwrap();

        store
            .add_annotation(DiffAnnotation {
                annotation_id: "ann-1".to_string(),
                file_path: "src/main.rs".to_string(),
                side: DiffSide::Right,
                line_start: 10,
                line_end: 12,
                body: "Avoid unnecessary clone in hot loop".to_string(),
                author: LineAttribution::Human,
                resolved: false,
                created_at_ms: 1000,
                updated_at_ms: 1000,
            })
            .unwrap();
        store
            .add_annotation(DiffAnnotation {
                annotation_id: "ann-2".to_string(),
                file_path: "src/main.rs".to_string(),
                side: DiffSide::Right,
                line_start: 25,
                line_end: 25,
                body: "Add error context".to_string(),
                author: LineAttribution::Human,
                resolved: false,
                created_at_ms: 1100,
                updated_at_ms: 1100,
            })
            .unwrap();

        // Inserting 5 lines at line 15 shifts ann-2 from L25 -> L30, leaving ann-1 at L10..=12.
        store
            .apply_line_shift("src/main.rs", DiffSide::Right, 15, 5, 1500)
            .unwrap();

        // Reopen from disk to prove SQLite WAL persistence.
        drop(store);
        let reopened = PersistentDiffReviewStore::open(&db_path).unwrap();
        let anns = reopened.list_annotations(Some("src/main.rs"), false);
        assert_eq!(anns.len(), 2);
        assert_eq!(anns[0].line_start, 10);
        assert_eq!(anns[1].line_start, 30);

        let batch = reopened.build_agent_batch_prompt(Some("src/main.rs"));
        assert!(batch.contains("- src/main.rs:L10-L12: Avoid unnecessary clone in hot loop"));
        assert!(batch.contains("- src/main.rs:L30: Add error context"));

        reopened.resolve_annotation("ann-1", 2000).unwrap();
        assert_eq!(reopened.list_annotations(Some("src/main.rs"), false).len(), 1);
        reopened.reopen_annotation("ann-1", 2100).unwrap();
        assert_eq!(reopened.list_annotations(Some("src/main.rs"), false).len(), 2);

        // AI-vs-Human provenance: AI writes L1..=10, human edits L4..=6 -> flips to Human!
        reopened
            .record_ai_range(
                "src/main.rs",
                1,
                10,
                LineAttribution::Worker {
                    dispatch_id: "disp-7".to_string(),
                },
                3000,
            )
            .unwrap();
        reopened
            .record_human_edit("src/main.rs", 4, 6, 3100)
            .unwrap();
        assert_eq!(
            reopened.line_attribution("src/main.rs", 2),
            LineAttribution::Worker {
                dispatch_id: "disp-7".to_string()
            }
        );
        assert_eq!(
            reopened.line_attribution("src/main.rs", 5),
            LineAttribution::Human
        );
        let export = reopened.export_attribution_metadata(Some("src/main.rs"));
        assert_eq!(export.ai_lines, 7);
        assert_eq!(export.human_lines, 3);
    }

    #[test]
    fn commit_pr_draft_and_tasks_hub_jira_normalization_and_issue_command() {
        let draft = compose_commit_and_pr_draft(&CommitPrDraftRequest {
            commit_type: "feat".to_string(),
            scope: Some("diff".to_string()),
            summary: "add persistent review annotations".to_string(),
            base_ref: "stack/base-pr-101".to_string(),
            head_branch: "feat/diff-annotations".to_string(),
            files: vec![DiffFileChange {
                file_path: "crates/orca_ide/src/diff_review.rs".to_string(),
                added_lines: 180,
                deleted_lines: 12,
                summary: Some("SQLite WAL review store".to_string()),
            }],
            stacked_parent_pr: Some("#101".to_string()),
            breaking_change: None,
        })
        .unwrap();
        assert_eq!(draft.commit_title, "feat(diff): add persistent review annotations");
        assert!(draft.pr_body_markdown.contains("stack/base-pr-101"));
        assert!(draft.pr_body_markdown.contains("#101"));
        assert!(draft.pr_body_markdown.contains("+180 / -12"));

        let hub = TasksHubStore::in_memory().unwrap();
        let jira = hub
            .upsert_task(HubTaskRecord {
                task_id: "t-jira-1".to_string(),
                forge: TaskForge::Jira,
                external_key: "eng-42".to_string(),
                title: "Fix FTS5 ranking".to_string(),
                description: "Improve bm25 weights".to_string(),
                status: "in_progress".to_string(),
                assignee: Some("alice".to_string()),
                labels: vec!["search".to_string()],
                artifact_url: "https://jira.example.com/browse/ENG-42".to_string(),
                repo_or_project: "ENG".to_string(),
                updated_at_ms: 5000,
            })
            .unwrap();
        assert_eq!(jira.external_key, "ENG-42");

        hub.upsert_task(HubTaskRecord {
            task_id: "t-gh-1".to_string(),
            forge: TaskForge::GitHub,
            external_key: "#128".to_string(),
            title: "Add stacked PR base".to_string(),
            description: String::new(),
            status: "open".to_string(),
            assignee: None,
            labels: vec![],
            artifact_url: "https://github.com/org/repo/issues/128".to_string(),
            repo_or_project: "org/repo".to_string(),
            updated_at_ms: 6000,
        })
        .unwrap();

        let filtered = hub
            .list_tasks(&TasksHubFilter {
                forge: Some(TaskForge::Jira),
                status: None,
                search: Some("eng-42".to_string()),
                limit: 10,
            })
            .unwrap();
        assert_eq!(filtered.len(), 1);
        assert_eq!(filtered[0].external_key, "ENG-42");

        let cmd = hub
            .build_launch_command("t-jira-1", "claude --issue {{artifact_url}}")
            .unwrap();
        assert_eq!(cmd, "claude --issue https://jira.example.com/browse/ENG-42");

        // Shell injection in artifact_url must be rejected!
        assert!(
            expand_issue_command_template("echo {{artifact_url}}", "https://x.io/$(rm -rf /)")
                .is_err()
        );
    }
}
