//! Persistent Diff Annotations, Local Per-Line AI-vs-Human Attribution,
//! Conventional Commit / Stacked PR Draft Composer, and Multi-Forge Tasks Hub (`F17`).

use std::collections::{BTreeMap, BTreeSet};
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
    updated_at_ms INTEGER NOT NULL,
    repo_path TEXT NOT NULL DEFAULT '',
    anchor_line_text TEXT
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

/// Serializes a [`DiffAnnotation`] with canonical REST/RPC/CLI wire aliases
/// (`id`, `line`, `file`, optional `repo_path`, `orphaned`, `anchor_status`).
pub fn diff_annotation_to_wire_json(
    ann: &DiffAnnotation,
    repo_path: Option<&str>,
) -> serde_json::Value {
    diff_annotation_to_wire_json_with_orphan(ann, repo_path, false)
}

/// Serializes a [`DiffAnnotation`] including explicit `orphaned` / `anchor_status` state.
pub fn diff_annotation_to_wire_json_with_orphan(
    ann: &DiffAnnotation,
    repo_path: Option<&str>,
    orphaned: bool,
) -> serde_json::Value {
    let mut val = serde_json::to_value(ann).unwrap_or_else(|_| serde_json::json!({}));
    if let Some(obj) = val.as_object_mut() {
        obj.insert("id".to_string(), serde_json::json!(ann.annotation_id));
        obj.insert("line".to_string(), serde_json::json!(ann.line_start));
        obj.insert("file".to_string(), serde_json::json!(ann.file_path));
        obj.insert("orphaned".to_string(), serde_json::json!(orphaned));
        obj.insert(
            "anchor_status".to_string(),
            serde_json::json!(if orphaned { "orphaned" } else { "anchored" }),
        );
        if let Some(rp) = repo_path
            && !rp.trim().is_empty()
        {
            obj.insert("repo_path".to_string(), serde_json::json!(rp));
        }
    }
    val
}

struct DiffReviewInner {
    conn: Connection,
    annotations: DiffAnnotationStore,
    annotation_meta: BTreeMap<String, (String, Option<String>)>,
    orphaned_ids: BTreeSet<String>,
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
    let _ = conn.execute(
        "ALTER TABLE diff_annotations ADD COLUMN repo_path TEXT NOT NULL DEFAULT ''",
        [],
    );
    let _ = conn.execute(
        "ALTER TABLE diff_annotations ADD COLUMN anchor_line_text TEXT",
        [],
    );
    let _ = conn.execute(
        "ALTER TABLE diff_annotations ADD COLUMN orphaned INTEGER NOT NULL DEFAULT 0",
        [],
    );

    let mut annotations = DiffAnnotationStore::default();
    let mut annotation_meta = BTreeMap::new();
    let mut orphaned_ids = BTreeSet::new();
    {
        let mut stmt = conn.prepare(
            "SELECT annotation_id, file_path, side, line_start, line_end, body,
                    author_json, resolved, created_at_ms, updated_at_ms,
                    COALESCE(repo_path, ''), anchor_line_text, COALESCE(orphaned, 0)
             FROM diff_annotations",
        )?;
        let rows = stmt.query_map([], |r| {
            let side_raw: String = r.get(2)?;
            let author_raw: String = r.get(6)?;
            let author = serde_json::from_str(&author_raw).unwrap_or(LineAttribution::Unknown);
            let ann = DiffAnnotation {
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
            };
            let repo_path: String = r.get(10)?;
            let anchor_line_text: Option<String> = r.get(11)?;
            let is_orph: bool = r.get::<_, i64>(12)? != 0;
            Ok((ann, repo_path, anchor_line_text, is_orph))
        })?;
        for row in rows {
            let (ann, repo_path, anchor_line_text, is_orph) = row?;
            annotation_meta.insert(ann.annotation_id.clone(), (repo_path, anchor_line_text));
            if is_orph {
                orphaned_ids.insert(ann.annotation_id.clone());
            }
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
        annotation_meta,
        orphaned_ids,
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

    /// Persists a new line-anchored diff annotation validated by `DiffAnnotationStore`,
    /// capturing optional `repo_path` and `anchor_line_text` (auto-read from disk if absent).
    pub fn add_annotation_with_context(
        &self,
        annotation: DiffAnnotation,
        repo_path: Option<&str>,
        anchor_line_text: Option<&str>,
    ) -> Result<DiffAnnotation> {
        let fp_trimmed = annotation.file_path.trim();
        if fp_trimmed.is_empty()
            || fp_trimmed.starts_with('/')
            || fp_trimmed.starts_with('\\')
            || fp_trimmed.contains('\0')
            || Path::new(fp_trimmed)
                .components()
                .any(|c| matches!(c, std::path::Component::ParentDir | std::path::Component::RootDir | std::path::Component::Prefix(_)))
        {
            return Err(OrcaIdeRuntimeError::invalid_argument(format!(
                "invalid relative file_path '{}'",
                annotation.file_path
            )));
        }
        if annotation.line_start == 0 || annotation.line_end < annotation.line_start {
            return Err(OrcaIdeRuntimeError::invalid_argument(
                "line_start must be >= 1 and line_end >= line_start",
            ));
        }
        if annotation.body.trim().is_empty() {
            return Err(OrcaIdeRuntimeError::invalid_argument(
                "annotation body must be non-empty",
            ));
        }
        let rp = repo_path.map(str::trim).unwrap_or("").to_string();
        let resolved_anchor = match anchor_line_text {
            Some(txt) => Some(txt.to_string()),
            None if !rp.is_empty() && annotation.line_start >= 1 => {
                let full_path = Path::new(&rp).join(&annotation.file_path);
                fs::read_to_string(&full_path).ok().and_then(|content| {
                    content
                        .lines()
                        .nth((annotation.line_start - 1) as usize)
                        .map(ToOwned::to_owned)
                })
            }
            None => None,
        };

        let mut guard = self.inner.lock();
        guard.annotations.add(annotation.clone())?;
        let author_json = serde_json::to_string(&annotation.author)?;
        if let Err(err) = guard.conn.execute(
            "INSERT INTO diff_annotations
                (annotation_id, file_path, side, line_start, line_end, body, author_json, resolved, created_at_ms, updated_at_ms, repo_path, anchor_line_text)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)",
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
                &rp,
                &resolved_anchor,
            ],
        ) {
            guard.annotations.annotations.remove(&annotation.annotation_id);
            return Err(OrcaIdeRuntimeError::Sqlite(err));
        }
        guard
            .annotation_meta
            .insert(annotation.annotation_id.clone(), (rp, resolved_anchor));
        Ok(annotation)
    }

    /// Persists a new line-anchored diff annotation validated by `DiffAnnotationStore`.
    pub fn add_annotation(&self, annotation: DiffAnnotation) -> Result<DiffAnnotation> {
        self.add_annotation_with_context(annotation, None, None)
    }

    /// Lists diff annotations (optionally scoped to one file, optionally including resolved).
    pub fn list_annotations(
        &self,
        file_path: Option<&str>,
        include_resolved: bool,
    ) -> Vec<DiffAnnotation> {
        self.list_annotations_scoped(None, file_path, include_resolved)
            .into_iter()
            .map(|(a, _)| a)
            .collect()
    }

    /// Lists diff annotations optionally filtered by `repo_path` and/or `file_path`.
    pub fn list_annotations_scoped(
        &self,
        repo_path: Option<&str>,
        file_path: Option<&str>,
        include_resolved: bool,
    ) -> Vec<(DiffAnnotation, String)> {
        let rp_filter = repo_path.map(str::trim).filter(|s| !s.is_empty());
        let fp_filter = file_path.map(str::trim).filter(|s| !s.is_empty());
        let guard = self.inner.lock();
        let mut out: Vec<(DiffAnnotation, String)> = guard
            .annotations
            .annotations
            .values()
            .filter(|a| include_resolved || !a.resolved)
            .filter(|a| fp_filter.is_none_or(|fp| a.file_path == fp))
            .filter_map(|a| {
                let rp = guard
                    .annotation_meta
                    .get(&a.annotation_id)
                    .map(|(r, _)| r.clone())
                    .unwrap_or_default();
                if let Some(target_rp) = rp_filter
                    && rp != target_rp
                {
                    return None;
                }
                Some((a.clone(), rp))
            })
            .collect();
        out.sort_by(|(a, _), (b, _)| {
            a.file_path
                .cmp(&b.file_path)
                .then_with(|| a.line_start.cmp(&b.line_start))
                .then_with(|| a.annotation_id.cmp(&b.annotation_id))
        });
        out
    }

    pub fn annotation_repo_path(&self, annotation_id: &str) -> Option<String> {
        self.inner
            .lock()
            .annotation_meta
            .get(annotation_id)
            .map(|(r, _)| r.clone())
            .filter(|r| !r.is_empty())
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

    /// Re-anchors persisted annotations for `(repo_path, file_path)` against live edits
    /// using `git diff -U0 HEAD -- <file_path>` hunks and stored `anchor_line_text`.
    pub fn reanchor_file(
        &self,
        repo_path: &Path,
        file_path: &str,
        now_ms: i64,
    ) -> Result<Vec<DiffAnnotation>> {
        let rp_str = repo_path.to_string_lossy().to_string();
        let disk_lines: Vec<String> = fs::read_to_string(repo_path.join(file_path))
            .map(|s| s.lines().map(ToOwned::to_owned).collect())
            .unwrap_or_default();

        // Parse unified diff hunks (`@@ -old_s[,old_c] +new_s[,new_c] @@`).
        let mut hunks: Vec<(u32, u32, u32, u32)> = Vec::new();
        if let Ok(out) = std::process::Command::new("git")
            .arg("-C")
            .arg(repo_path)
            .args(["diff", "-U0", "HEAD", "--", file_path])
            .output()
            && out.status.success()
        {
            let diff_txt = String::from_utf8_lossy(&out.stdout);
            for line in diff_txt.lines() {
                if let Some(rest) = line.strip_prefix("@@ -")
                    && let Some((hdr, _)) = rest.split_once(" @@")
                    && let Some((old_part, new_part)) = hdr.split_once(" +")
                {
                    let parse_pair = |p: &str| -> Option<(u32, u32)> {
                        if let Some((s, c)) = p.split_once(',') {
                            Some((s.parse().ok()?, c.parse().ok()?))
                        } else {
                            Some((p.parse().ok()?, 1))
                        }
                    };
                    if let (Some((os, oc)), Some((ns, nc))) =
                        (parse_pair(old_part), parse_pair(new_part))
                    {
                        hunks.push((os, oc, ns, nc));
                    }
                }
            }
        }

        let shift_line = |orig: u32| -> u32 {
            let mut delta: i64 = 0;
            for &(os, oc, ns, nc) in &hunks {
                if oc == 0 {
                    // Pure insertion immediately after line `os` (`os == 0` = top of file).
                    if orig > os {
                        delta += i64::from(nc);
                    }
                } else if orig >= os.saturating_add(oc) {
                    delta += i64::from(nc) - i64::from(oc);
                } else if orig >= os {
                    let mapped = ns.saturating_add((orig - os).min(nc.saturating_sub(1)));
                    return mapped.max(1);
                }
            }
            (i64::from(orig) + delta).max(1) as u32
        };

        let mut guard = self.inner.lock();
        let target_ids: Vec<String> = guard
            .annotations
            .annotations
            .values()
            .filter(|a| {
                a.file_path == file_path
                    && guard
                        .annotation_meta
                        .get(&a.annotation_id)
                        .is_none_or(|(r, _)| r.is_empty() || r == &rp_str)
            })
            .map(|a| a.annotation_id.clone())
            .collect();

        let mut updated = Vec::new();
        let mut newly_orphaned: Vec<String> = Vec::new();
        for aid in target_ids {
            let anchor_text = guard
                .annotation_meta
                .get(&aid)
                .and_then(|(_, txt)| txt.clone());
            let Some(ann) = guard.annotations.annotations.get_mut(&aid) else {
                continue;
            };
            let span = ann.line_end.saturating_sub(ann.line_start);

            // If anchor text already matches at the current line, avoid double-shifting.
            let already_anchored = anchor_text.as_deref().is_some_and(|at| {
                disk_lines
                    .get(ann.line_start.saturating_sub(1) as usize)
                    .is_some_and(|cur| cur == at)
            });

            if !already_anchored {
                let hunk_deleted_anchor = hunks
                    .iter()
                    .any(|&(os, oc, _, nc)| oc > 0 && nc == 0 && ann.line_start >= os && ann.line_start < os.saturating_add(oc));
                let text_exists_anywhere = anchor_text
                    .as_deref()
                    .is_some_and(|at| disk_lines.iter().any(|l| l == at));
                if (anchor_text.is_some() && !text_exists_anywhere) || (hunk_deleted_anchor && !text_exists_anywhere) {
                    newly_orphaned.push(aid.clone());
                    ann.updated_at_ms = now_ms;
                    updated.push(ann.clone());
                    continue;
                }

                let hunk_start = shift_line(ann.line_start);
                let hunk_matches_anchor = anchor_text.as_deref().is_some_and(|at| {
                    disk_lines
                        .get(hunk_start.saturating_sub(1) as usize)
                        .is_some_and(|cur| cur == at)
                });
                let new_start = if !hunks.is_empty() && (anchor_text.is_none() || hunk_matches_anchor) {
                    hunk_start
                } else if let Some(ref at) = anchor_text {
                    disk_lines
                        .iter()
                        .enumerate()
                        .filter(|(_, line)| line.as_str() == at.as_str())
                        .map(|(idx, _)| (idx + 1) as u32)
                        .min_by_key(|&ln| ln.abs_diff(ann.line_start))
                        .unwrap_or(hunk_start)
                } else {
                    hunk_start
                };
                ann.line_start = new_start.max(1);
                ann.line_end = ann.line_start.saturating_add(span);
                ann.updated_at_ms = now_ms;
            }
            updated.push(ann.clone());
        }

        for oid in &newly_orphaned {
            guard.orphaned_ids.insert(oid.clone());
        }
        let orphaned_snapshot = guard.orphaned_ids.clone();

        let tx = guard.conn.transaction()?;
        for ann in &updated {
            let is_orph = i64::from(orphaned_snapshot.contains(&ann.annotation_id));
            tx.execute(
                "UPDATE diff_annotations
                 SET line_start = ?1, line_end = ?2, updated_at_ms = ?3, orphaned = ?4
                 WHERE annotation_id = ?5",
                params![
                    ann.line_start as i64,
                    ann.line_end as i64,
                    ann.updated_at_ms,
                    is_orph,
                    &ann.annotation_id,
                ],
            )?;
        }
        tx.commit()?;
        Ok(updated)
    }

    /// Returns whether the annotation with `annotation_id` has been marked orphaned.
    pub fn is_orphaned(&self, annotation_id: &str) -> bool {
        self.inner.lock().orphaned_ids.contains(annotation_id)
    }

    /// Formats all unresolved annotations into a single coherent `"Send to agent"` batch prompt.
    pub fn build_agent_batch_prompt(&self, file_path: Option<&str>) -> String {
        self.build_agent_batch_prompt_scoped(None, file_path)
    }

    /// Formats unresolved annotations scoped by optional `(repo_path, file_path)` into a
    /// single coherent `"Send to agent"` batch prompt, keeping annotations pinned/unresolved.
    pub fn build_agent_batch_prompt_scoped(
        &self,
        repo_path: Option<&str>,
        file_path: Option<&str>,
    ) -> String {
        let items = self.list_annotations_scoped(repo_path, file_path, false);
        if items.is_empty() {
            return String::new();
        }
        let mut lines = vec![format!(
            "Address these {} diff review annotation(s):",
            items.len()
        )];
        for (ann, _) in items {
            let loc = if ann.line_start == ann.line_end {
                format!("L{}", ann.line_start)
            } else {
                format!("L{}-L{}", ann.line_start, ann.line_end)
            };
            lines.push(format!("- {}:{}: {}", ann.file_path, loc, ann.body));
        }
        lines.join("\n")
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

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
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
    hidden_providers: Mutex<BTreeSet<TaskForge>>,
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
            hidden_providers: Mutex::new(BTreeSet::new()),
        })
    }

    pub fn in_memory() -> Result<Self> {
        let conn = Connection::open_in_memory()?;
        init_tasks_hub_conn(&conn)?;
        Ok(Self {
            conn: Mutex::new(conn),
            hidden_providers: Mutex::new(BTreeSet::new()),
        })
    }

    /// Toggles whether tasks from `forge` appear in `list_tasks` results.
    pub fn set_provider_visibility(&self, forge: TaskForge, visible: bool) {
        let mut hidden = self.hidden_providers.lock();
        if visible {
            hidden.remove(&forge);
        } else {
            hidden.insert(forge);
        }
    }

    /// Upserts a forge task, normalizing Jira/Linear key casing automatically and deduplicating
    /// equivalent keys (`eng-42` == `ENG-42`) on the same forge.
    pub fn upsert_task(&self, mut record: HubTaskRecord) -> Result<HubTaskRecord> {
        if record.task_id.trim().is_empty() || record.title.trim().is_empty() {
            return Err(OrcaIdeRuntimeError::invalid_argument(
                "task_id and title must be non-empty",
            ));
        }
        record.external_key = normalize_task_key(record.forge, &record.external_key)?;
        let canonical_prefix = format!("{}:", record.forge.as_str());
        if record.task_id.starts_with(&canonical_prefix)
            || record.task_id.eq_ignore_ascii_case(&record.external_key)
        {
            record.task_id = format!("{}:{}", record.forge.as_str(), record.external_key);
        }
        let labels_json = serde_json::to_string(&record.labels)?;
        let conn = self.conn.lock();
        // Remove any legacy cased duplicate for the same (forge, normalized external_key).
        let _ = conn.execute(
            "DELETE FROM hub_tasks
             WHERE forge = ?1 AND UPPER(external_key) = UPPER(?2) AND task_id != ?3",
            params![record.forge.as_str(), &record.external_key, &record.task_id],
        );
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

    /// Looks up a task by `task_id` or case-insensitive `external_key` (e.g., `"GH-101"`).
    pub fn get_task_by_key(&self, key: &str) -> Result<Option<HubTaskRecord>> {
        let trimmed = key.trim();
        let conn = self.conn.lock();
        let row = conn
            .query_row(
                "SELECT task_id, forge, external_key, title, description, status, assignee,
                        labels_json, artifact_url, repo_or_project, updated_at_ms
                 FROM hub_tasks
                 WHERE task_id = ?1 OR external_key = ?1 OR UPPER(external_key) = UPPER(?1)
                 ORDER BY updated_at_ms DESC
                 LIMIT 1",
                params![trimmed],
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

        let hidden_snapshot = self.hidden_providers.lock().clone();
        let mut out = Vec::new();
        let rows = stmt.query_map([], row_to_hub_task)?;
        for item in rows {
            let task = item?;
            if filter.forge.is_none() && hidden_snapshot.contains(&task.forge) {
                continue;
            }
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
