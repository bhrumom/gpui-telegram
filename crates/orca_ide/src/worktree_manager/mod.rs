//! Worktree, Folder Workspace, Project Group, Floating Workspace, First-Work Auto-Rename,
//! Interrupted Removal Recovery Ledger, Sparse Checkout Presets, `orca.yaml`,
//! `.worktreeinclude`, Checkpoints, Disk Analyzer, and Unsaved-Editor-Guarded Cleanup (`F10`).

pub mod yaml_include;

#[cfg(test)]
mod tests;

use std::collections::{BTreeSet, HashSet};
use std::path::{Path, PathBuf};

use orca_core::workspace_layout::{FolderWorkspace, validate_worktree_relative_path};
use parking_lot::Mutex;
use rusqlite::{Connection, OptionalExtension, params};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use uuid::Uuid;
use walkdir::WalkDir;

use crate::agent_status_hooks::AgentRunState;
use crate::{OrcaIdeRuntimeError, Result, now_epoch_ms};

pub use yaml_include::{
    ExecutedScriptStep, OrcaYamlExecutionReport, OrcaYamlRunRequest, OrcaYamlRunnerPlan,
    OrcaYamlScriptPhase, PlannedScriptStep, ScriptOverrideMode, ScriptPlatform,
    SharedDirectoryLinkResult, WorktreeIncludeCopyOptions, WorktreeIncludeCopyReport,
    execute_orca_yaml_scripts, execute_worktree_include_copy, link_shared_directories,
    parse_orca_yaml_text, plan_orca_yaml_scripts,
};

/// Default base backoff window (5 minutes) after a failed first-work rename attempt.
pub const DEFAULT_FIRST_WORK_RENAME_BACKOFF_MS: i64 = 5 * 60 * 1_000;

/// Maximum exponential backoff window (1 hour) for first-work rename retries.
pub const MAX_FIRST_WORK_RENAME_BACKOFF_MS: i64 = 60 * 60 * 1_000;

const CREATURE_ADJECTIVES: [&str; 32] = [
    "amber", "bright", "calm", "clever", "coral", "crisp", "daring", "ember", "frost", "golden",
    "ivory", "jade", "lunar", "misty", "noble", "opal", "pebble", "quiet", "rapid", "rustic",
    "silver", "solar", "steady", "stormy", "sunny", "swift", "velvet", "vivid", "warm", "wild",
    "wise", "zephyr",
];

const CREATURE_NOUNS: [&str; 32] = [
    "badger", "beaver", "crane", "deer", "dolphin", "eagle", "falcon", "ferret", "finch", "fox",
    "gazelle", "gecko", "hare", "hawk", "heron", "ibis", "jaguar", "kestrel", "kiwi", "koala",
    "lemur", "lynx", "marten", "newt", "osprey", "otter", "owl", "panda", "pelican", "puma",
    "raven", "seal",
];

/// Kanban workspace status (`todo`, `in-progress`, `in-review`, `completed`, or custom).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum WorkspaceStatus {
    Todo,
    InProgress,
    InReview,
    Completed,
    Custom(String),
}

impl WorkspaceStatus {
    pub fn as_str(&self) -> &str {
        match self {
            Self::Todo => "todo",
            Self::InProgress => "in-progress",
            Self::InReview => "in-review",
            Self::Completed => "completed",
            Self::Custom(s) => s.as_str(),
        }
    }

    pub fn parse(raw: &str) -> Result<Self> {
        let trimmed = raw.trim();
        if trimmed.is_empty() || trimmed.chars().any(char::is_control) {
            return Err(OrcaIdeRuntimeError::invalid_argument(
                "workspace_status must be non-empty without control characters",
            ));
        }
        Ok(match trimmed {
            "todo" => Self::Todo,
            "in-progress" => Self::InProgress,
            "in-review" => Self::InReview,
            "completed" => Self::Completed,
            other => Self::Custom(other.to_string()),
        })
    }
}

/// Logical group organizing Git worktrees and non-Git folder workspaces.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProjectGroup {
    pub id: String,
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub color: Option<String>,
    pub sort_order: i32,
    pub created_at_ms: i64,
    pub updated_at_ms: i64,
}

/// Explicit allowlist entry for Floating Workspace working directories (`trusted_cwds`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TrustedFloatingCwd {
    pub cwd_path: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
    pub approved_by: String,
    pub approved_at_ms: i64,
}

/// Persisted Git worktree metadata with parent/child lineage, creature auto-naming,
/// rename backoff tracking, checkpoints, and kanban status.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorktreeRecord {
    pub id: String,
    pub repo_root: String,
    pub worktree_path: String,
    pub branch_name: String,
    pub title: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent_worktree_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub project_group_id: Option<String>,
    pub is_creature_named: bool,
    pub created_by_orca: bool,
    pub first_work_renamed: bool,
    pub rename_failure_count: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_rename_failure_at_ms: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub checkpoint_comment: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub workspace_status: Option<String>,
    pub pinned: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sparse_preset_id: Option<String>,
    pub created_at_ms: i64,
    pub updated_at_ms: i64,
}

/// Request to register a Git worktree under `WorktreeManager`.
#[derive(Debug, Clone)]
pub struct CreateWorktreeRequest {
    pub id: Option<String>,
    pub repo_root: PathBuf,
    pub worktree_path: PathBuf,
    /// When `None`, automatically generates a collision-avoidant creature branch name.
    pub branch_name: Option<String>,
    pub title: Option<String>,
    pub parent_worktree_id: Option<String>,
    pub project_group_id: Option<String>,
    pub created_by_orca: bool,
    pub existing_branches: BTreeSet<String>,
    pub seed_hint: Option<String>,
}

/// Context for evaluating and applying First-Work Auto-Rename on a creature-named worktree.
#[derive(Debug, Clone)]
pub struct FirstWorkRenameContext {
    pub worktree_id: String,
    pub agent_state: AgentRunState,
    pub prompt_summary: String,
    pub checkout_locked: bool,
    pub upstream_published: bool,
    pub existing_branches: BTreeSet<String>,
    pub rename_directory_on_disk: bool,
    pub base_backoff_ms: i64,
    pub now_ms: i64,
}

/// Reason why First-Work Auto-Rename was refused/skipped by safety guards.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "reason", rename_all = "snake_case")]
pub enum FirstWorkRenameSkipReason {
    NotWorking,
    NotCreatedByOrca,
    NotCreatureNamed,
    AlreadyRenamed,
    CheckoutLocked,
    UpstreamPublished,
    InFailureBackoff { retry_after_ms: i64 },
    EmptySemanticSlug,
}

/// Outcome of `WorktreeManager::evaluate_and_apply_first_work_rename`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "outcome", rename_all = "snake_case")]
pub enum FirstWorkRenameOutcome {
    Renamed {
        old_branch: String,
        new_branch: String,
        old_path: String,
        new_path: String,
        new_title: String,
    },
    Skipped {
        reason: FirstWorkRenameSkipReason,
    },
    Failed {
        error: String,
        failure_count: u32,
        retry_after_ms: i64,
    },
}

/// Lifecycle state of an entry in the interrupted worktree-removal recovery ledger.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RemovalLedgerState {
    Pending,
    InProgress,
    Completed,
    Failed,
}

impl RemovalLedgerState {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Pending => "pending",
            Self::InProgress => "in_progress",
            Self::Completed => "completed",
            Self::Failed => "failed",
        }
    }

    fn parse(raw: &str) -> Self {
        match raw {
            "in_progress" => Self::InProgress,
            "completed" => Self::Completed,
            "failed" => Self::Failed,
            _ => Self::Pending,
        }
    }
}

/// Durable ledger record tracking a worktree removal so crashes recover cleanly on restart.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorktreeRemovalEntry {
    pub removal_id: String,
    pub worktree_id: String,
    pub worktree_path: String,
    pub branch_name: String,
    pub repo_root: String,
    pub state: RemovalLedgerState,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error_message: Option<String>,
    pub started_at_ms: i64,
    pub updated_at_ms: i64,
}

/// Report returned by `WorktreeManager::recover_interrupted_removals` on startup.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct RemovalRecoveryReport {
    pub recovered_removal_ids: Vec<String>,
    pub failed_removals: Vec<(String, String)>,
}

/// Named sparse-checkout pattern preset validated against `validate_worktree_relative_path`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SparseCheckoutPreset {
    pub id: String,
    pub name: String,
    pub patterns: Vec<String>,
    pub cone_mode: bool,
    pub created_at_ms: i64,
    pub updated_at_ms: i64,
}

/// Single entry in a disk usage breakdown.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DiskUsageEntry {
    pub relative_path: String,
    pub bytes: u64,
}

/// Disk-space analysis report for a worktree or folder workspace.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct DiskUsageReport {
    pub root_path: String,
    pub total_bytes: u64,
    pub git_bytes: u64,
    pub file_count: usize,
    pub dir_count: usize,
    pub largest_files: Vec<DiskUsageEntry>,
}

/// Request to clean up a worktree with unsaved-editor safety protection.
#[derive(Debug, Clone)]
pub struct CleanupWorktreeRequest {
    pub worktree_id: String,
    pub unsaved_editor_tabs: Vec<String>,
    pub force_discard_unsaved: bool,
    pub remove_directory_on_disk: bool,
    pub now_ms: i64,
}

/// Outcome of a completed worktree cleanup.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CleanupWorktreeOutcome {
    pub worktree_id: String,
    pub removal_id: String,
    pub freed_bytes: u64,
    pub discarded_unsaved_count: usize,
}

/// Durable pure-Rust Worktree, FolderWorkspace, ProjectGroup, FloatingWorkspace,
/// Removal Ledger, Sparse Preset, Checkpoint & Cleanup manager (`Send + Sync`).
pub struct WorktreeManager {
    data_dir: Option<PathBuf>,
    conn: Mutex<Connection>,
}

impl WorktreeManager {
    /// Opens (or creates) durable SQLite WAL state under `<data_dir>/worktree_manager.db`.
    pub fn open(data_dir: impl AsRef<Path>) -> Result<Self> {
        let dir = data_dir.as_ref().to_path_buf();
        std::fs::create_dir_all(&dir)?;
        let db_path = dir.join("worktree_manager.db");
        let conn = Connection::open(&db_path)?;
        init_worktree_schema(&conn)?;
        Ok(Self {
            data_dir: Some(dir),
            conn: Mutex::new(conn),
        })
    }

    /// Opens an in-memory SQLite instance (useful for ephemeral evaluation/testing).
    pub fn open_in_memory() -> Result<Self> {
        let conn = Connection::open_in_memory()?;
        init_worktree_schema(&conn)?;
        Ok(Self {
            data_dir: None,
            conn: Mutex::new(conn),
        })
    }

    pub fn data_dir(&self) -> Option<&Path> {
        self.data_dir.as_deref()
    }
}

fn init_worktree_schema(conn: &Connection) -> Result<()> {
    conn.execute_batch(
        r#"
        PRAGMA journal_mode = WAL;
        PRAGMA foreign_keys = ON;

        CREATE TABLE IF NOT EXISTS project_groups (
            id TEXT PRIMARY KEY,
            name TEXT NOT NULL,
            description TEXT,
            color TEXT,
            sort_order INTEGER NOT NULL DEFAULT 0,
            created_at_ms INTEGER NOT NULL,
            updated_at_ms INTEGER NOT NULL
        );

        CREATE TABLE IF NOT EXISTS folder_workspaces (
            id TEXT PRIMARY KEY,
            payload_json TEXT NOT NULL,
            created_at_ms INTEGER NOT NULL,
            updated_at_ms INTEGER NOT NULL
        );

        CREATE TABLE IF NOT EXISTS floating_trusted_cwds (
            cwd_path TEXT PRIMARY KEY,
            label TEXT,
            approved_by TEXT NOT NULL,
            approved_at_ms INTEGER NOT NULL
        );

        CREATE TABLE IF NOT EXISTS worktrees (
            id TEXT PRIMARY KEY,
            payload_json TEXT NOT NULL,
            branch_name TEXT NOT NULL,
            parent_worktree_id TEXT,
            project_group_id TEXT,
            created_at_ms INTEGER NOT NULL,
            updated_at_ms INTEGER NOT NULL
        );

        CREATE TABLE IF NOT EXISTS removal_ledger (
            removal_id TEXT PRIMARY KEY,
            worktree_id TEXT NOT NULL,
            worktree_path TEXT NOT NULL,
            branch_name TEXT NOT NULL,
            repo_root TEXT NOT NULL,
            state TEXT NOT NULL,
            error_message TEXT,
            started_at_ms INTEGER NOT NULL,
            updated_at_ms INTEGER NOT NULL
        );

        CREATE TABLE IF NOT EXISTS sparse_presets (
            id TEXT PRIMARY KEY,
            name TEXT NOT NULL,
            patterns_json TEXT NOT NULL,
            cone_mode INTEGER NOT NULL,
            created_at_ms INTEGER NOT NULL,
            updated_at_ms INTEGER NOT NULL
        );

        CREATE TABLE IF NOT EXISTS orca_yaml_trust (
            repo_root TEXT PRIMARY KEY,
            approved_trust_hash TEXT NOT NULL,
            approved_at_ms INTEGER NOT NULL
        );
        "#,
    )?;
    Ok(())
}

fn normalize_absolute_or_clean_path(raw: &str) -> Result<String> {
    let trimmed = raw.trim();
    if trimmed.is_empty() || trimmed.chars().any(char::is_control) {
        return Err(OrcaIdeRuntimeError::invalid_argument(
            "path must be non-empty without control characters",
        ));
    }
    let normalized = trimmed.replace('\\', "/");
    if normalized.split('/').any(|seg| seg == "..") {
        return Err(OrcaIdeRuntimeError::invalid_argument(format!(
            "path traversal ('..') forbidden in '{raw}'"
        )));
    }
    let trimmed_end = if normalized.len() > 1 {
        normalized.trim_end_matches('/').to_string()
    } else {
        normalized
    };
    Ok(trimmed_end)
}

impl WorktreeManager {
    // -----------------------------------------------------------------------
    // FolderWorkspace CRUD (reusing `orca_core::workspace_layout::FolderWorkspace`)
    // -----------------------------------------------------------------------

    pub fn create_folder_workspace(&self, workspace: FolderWorkspace) -> Result<FolderWorkspace> {
        workspace
            .validate()
            .map_err(|e| OrcaIdeRuntimeError::invalid_argument(e.to_string()))?;
        let payload = serde_json::to_string(&workspace)?;
        let conn = self.conn.lock();
        let exists: Option<String> = conn
            .query_row(
                "SELECT id FROM folder_workspaces WHERE id = ?1",
                params![&workspace.id],
                |row| row.get(0),
            )
            .optional()?;
        if exists.is_some() {
            return Err(OrcaIdeRuntimeError::conflict(format!(
                "folder workspace '{}' already exists",
                workspace.id
            )));
        }
        conn.execute(
            "INSERT INTO folder_workspaces (id, payload_json, created_at_ms, updated_at_ms)
             VALUES (?1, ?2, ?3, ?4)",
            params![
                &workspace.id,
                &payload,
                workspace.created_at_ms,
                workspace.updated_at_ms
            ],
        )?;
        Ok(workspace)
    }

    pub fn get_folder_workspace(&self, id: &str) -> Result<FolderWorkspace> {
        let conn = self.conn.lock();
        let payload: Option<String> = conn
            .query_row(
                "SELECT payload_json FROM folder_workspaces WHERE id = ?1",
                params![id],
                |row| row.get(0),
            )
            .optional()?;
        let Some(json) = payload else {
            return Err(OrcaIdeRuntimeError::not_found(format!(
                "folder workspace '{id}' not found"
            )));
        };
        Ok(serde_json::from_str(&json)?)
    }

    pub fn list_folder_workspaces(&self) -> Result<Vec<FolderWorkspace>> {
        let conn = self.conn.lock();
        let mut stmt = conn.prepare(
            "SELECT payload_json FROM folder_workspaces ORDER BY created_at_ms ASC, id ASC",
        )?;
        let rows = stmt.query_map([], |row| row.get::<_, String>(0))?;
        let mut out = Vec::new();
        for row in rows {
            out.push(serde_json::from_str(&row?)?);
        }
        Ok(out)
    }

    pub fn update_folder_workspace(&self, workspace: FolderWorkspace) -> Result<FolderWorkspace> {
        workspace
            .validate()
            .map_err(|e| OrcaIdeRuntimeError::invalid_argument(e.to_string()))?;
        let payload = serde_json::to_string(&workspace)?;
        let conn = self.conn.lock();
        let updated = conn.execute(
            "UPDATE folder_workspaces SET payload_json = ?2, updated_at_ms = ?3 WHERE id = ?1",
            params![&workspace.id, &payload, workspace.updated_at_ms],
        )?;
        if updated == 0 {
            return Err(OrcaIdeRuntimeError::not_found(format!(
                "folder workspace '{}' not found",
                workspace.id
            )));
        }
        Ok(workspace)
    }

    pub fn delete_folder_workspace(&self, id: &str) -> Result<bool> {
        let conn = self.conn.lock();
        let deleted = conn.execute("DELETE FROM folder_workspaces WHERE id = ?1", params![id])?;
        Ok(deleted > 0)
    }

    // -----------------------------------------------------------------------
    // Project Group CRUD
    // -----------------------------------------------------------------------

    pub fn create_project_group(&self, group: ProjectGroup) -> Result<ProjectGroup> {
        if group.id.trim().is_empty() || group.name.trim().is_empty() {
            return Err(OrcaIdeRuntimeError::invalid_argument(
                "project group id and name must be non-empty",
            ));
        }
        let conn = self.conn.lock();
        conn.execute(
            "INSERT INTO project_groups (id, name, description, color, sort_order, created_at_ms, updated_at_ms)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            params![
                &group.id,
                &group.name,
                &group.description,
                &group.color,
                group.sort_order,
                group.created_at_ms,
                group.updated_at_ms
            ],
        )?;
        Ok(group)
    }

    pub fn list_project_groups(&self) -> Result<Vec<ProjectGroup>> {
        let conn = self.conn.lock();
        let mut stmt = conn.prepare(
            "SELECT id, name, description, color, sort_order, created_at_ms, updated_at_ms
             FROM project_groups ORDER BY sort_order ASC, created_at_ms ASC, id ASC",
        )?;
        let rows = stmt.query_map([], |row| {
            Ok(ProjectGroup {
                id: row.get(0)?,
                name: row.get(1)?,
                description: row.get(2)?,
                color: row.get(3)?,
                sort_order: row.get(4)?,
                created_at_ms: row.get(5)?,
                updated_at_ms: row.get(6)?,
            })
        })?;
        let mut out = Vec::new();
        for row in rows {
            out.push(row?);
        }
        Ok(out)
    }

    pub fn delete_project_group(&self, id: &str) -> Result<bool> {
        let conn = self.conn.lock();
        let deleted = conn.execute("DELETE FROM project_groups WHERE id = ?1", params![id])?;
        Ok(deleted > 0)
    }

    // -----------------------------------------------------------------------
    // Floating Workspace Explicit `trusted_cwds` Allowlist
    // -----------------------------------------------------------------------

    /// Explicitly adds a directory to the Floating Workspace `trusted_cwds` allowlist.
    /// Never called implicitly when evaluating untrusted inputs.
    pub fn trust_floating_cwd(
        &self,
        cwd_path: impl AsRef<Path>,
        label: Option<String>,
        approved_by: impl Into<String>,
        now_ms: i64,
    ) -> Result<TrustedFloatingCwd> {
        let norm = normalize_absolute_or_clean_path(&cwd_path.as_ref().to_string_lossy())?;
        let approved_by = approved_by.into();
        if approved_by.trim().is_empty() {
            return Err(OrcaIdeRuntimeError::invalid_argument(
                "approved_by must be non-empty",
            ));
        }
        let record = TrustedFloatingCwd {
            cwd_path: norm.clone(),
            label,
            approved_by,
            approved_at_ms: now_ms,
        };
        let conn = self.conn.lock();
        conn.execute(
            "INSERT OR REPLACE INTO floating_trusted_cwds (cwd_path, label, approved_by, approved_at_ms)
             VALUES (?1, ?2, ?3, ?4)",
            params![
                &record.cwd_path,
                &record.label,
                &record.approved_by,
                record.approved_at_ms
            ],
        )?;
        Ok(record)
    }

    pub fn revoke_floating_cwd(&self, cwd_path: impl AsRef<Path>) -> Result<bool> {
        let norm = normalize_absolute_or_clean_path(&cwd_path.as_ref().to_string_lossy())?;
        let conn = self.conn.lock();
        let deleted = conn.execute(
            "DELETE FROM floating_trusted_cwds WHERE cwd_path = ?1",
            params![norm],
        )?;
        Ok(deleted > 0)
    }

    pub fn list_trusted_cwds(&self) -> Result<Vec<TrustedFloatingCwd>> {
        let conn = self.conn.lock();
        let mut stmt = conn.prepare(
            "SELECT cwd_path, label, approved_by, approved_at_ms
             FROM floating_trusted_cwds ORDER BY cwd_path ASC",
        )?;
        let rows = stmt.query_map([], |row| {
            Ok(TrustedFloatingCwd {
                cwd_path: row.get(0)?,
                label: row.get(1)?,
                approved_by: row.get(2)?,
                approved_at_ms: row.get(3)?,
            })
        })?;
        let mut out = Vec::new();
        for row in rows {
            out.push(row?);
        }
        Ok(out)
    }

    /// Strict read-only check verifying `candidate_cwd` is explicitly in `trusted_cwds`.
    /// NEVER mutates or silently expands `trusted_cwds`.
    pub fn validate_floating_workspace_cwd(&self, candidate_cwd: impl AsRef<Path>) -> Result<String> {
        let norm = normalize_absolute_or_clean_path(&candidate_cwd.as_ref().to_string_lossy())?;
        let conn = self.conn.lock();
        let found: Option<String> = conn
            .query_row(
                "SELECT cwd_path FROM floating_trusted_cwds WHERE cwd_path = ?1",
                params![&norm],
                |row| row.get(0),
            )
            .optional()?;
        match found {
            Some(trusted) => Ok(trusted),
            None => Err(OrcaIdeRuntimeError::permission_denied(format!(
                "floating workspace cwd '{norm}' is not in trusted_cwds allowlist"
            ))),
        }
    }
}

/// Returns `true` if `branch` matches a creature auto-generated branch name
/// (`<adj>-<noun>` or `<adj>-<noun>-<collision_index>`).
pub fn is_creature_branch_name(branch: &str) -> bool {
    let parts: Vec<&str> = branch.split('-').collect();
    if parts.len() == 2 {
        CREATURE_ADJECTIVES.contains(&parts[0]) && CREATURE_NOUNS.contains(&parts[1])
    } else if parts.len() == 3 {
        CREATURE_ADJECTIVES.contains(&parts[0])
            && CREATURE_NOUNS.contains(&parts[1])
            && parts[2].parse::<u32>().is_ok()
    } else {
        false
    }
}

/// Converts a user prompt/task summary into a clean kebab-case Git branch slug.
pub fn slugify_semantic_branch(summary: &str) -> String {
    let mut slug = String::new();
    let mut prev_dash = true;
    for ch in summary.chars() {
        if ch.is_ascii_alphanumeric() {
            slug.push(ch.to_ascii_lowercase());
            prev_dash = false;
        } else if !prev_dash && slug.len() < 48 {
            slug.push('-');
            prev_dash = true;
        }
        if slug.len() >= 48 {
            break;
        }
    }
    slug.trim_matches('-').to_string()
}

fn compute_rename_backoff_ms(base_backoff_ms: i64, failure_count: u32) -> i64 {
    if failure_count == 0 {
        return 0;
    }
    let base = base_backoff_ms.max(1_000);
    let shift = failure_count.saturating_sub(1).min(10);
    let multiplier = 1i64.checked_shl(shift).unwrap_or(1_024);
    base.saturating_mul(multiplier)
        .min(MAX_FIRST_WORK_RENAME_BACKOFF_MS)
}

fn has_git_checkout_lock(repo_root: &Path, worktree_path: &Path) -> bool {
    if worktree_path.join(".git").join("index.lock").exists()
        || repo_root.join(".git").join("index.lock").exists()
    {
        return true;
    }
    // In linked Git worktrees `.git` is a file containing `gitdir: <path>`.
    let dot_git = worktree_path.join(".git");
    if dot_git.is_file()
        && let Ok(content) = std::fs::read_to_string(&dot_git)
        && let Some(raw_dir) = content.trim().strip_prefix("gitdir:")
    {
        let gitdir = PathBuf::from(raw_dir.trim());
        let resolved = if gitdir.is_absolute() {
            gitdir
        } else {
            worktree_path.join(gitdir)
        };
        if resolved.join("index.lock").exists() || resolved.join("locked").exists() {
            return true;
        }
    }
    false
}

impl WorktreeManager {
    /// Generates a collision-avoidant creature branch name (e.g., `swift-otter`, `amber-lynx`)
    /// avoiding both `existing_branches` and all branches currently tracked in `WorktreeManager`.
    pub fn generate_creature_branch_name(
        &self,
        existing_branches: &BTreeSet<String>,
        seed_hint: Option<&str>,
    ) -> Result<String> {
        let mut taken: HashSet<String> = existing_branches.iter().cloned().collect();
        {
            let conn = self.conn.lock();
            let mut stmt = conn.prepare("SELECT branch_name FROM worktrees")?;
            let rows = stmt.query_map([], |row| row.get::<_, String>(0))?;
            for row in rows {
                taken.insert(row?);
            }
        }

        let seed_input = seed_hint
            .map(ToString::to_string)
            .unwrap_or_else(|| Uuid::new_v4().to_string());
        let mut hasher = Sha256::new();
        hasher.update(seed_input.as_bytes());
        let digest = hasher.finalize();
        let start_adj = (digest[0] as usize) % CREATURE_ADJECTIVES.len();
        let start_noun = (digest[1] as usize) % CREATURE_NOUNS.len();

        let total_pairs = CREATURE_ADJECTIVES.len() * CREATURE_NOUNS.len();
        for offset in 0..total_pairs {
            let adj_idx = (start_adj + offset / CREATURE_NOUNS.len()) % CREATURE_ADJECTIVES.len();
            let noun_idx = (start_noun + offset) % CREATURE_NOUNS.len();
            let candidate = format!(
                "{}-{}",
                CREATURE_ADJECTIVES[adj_idx], CREATURE_NOUNS[noun_idx]
            );
            if !taken.contains(&candidate) {
                return Ok(candidate);
            }
        }

        let base = format!(
            "{}-{}",
            CREATURE_ADJECTIVES[start_adj], CREATURE_NOUNS[start_noun]
        );
        let mut suffix = 2u32;
        loop {
            let candidate = format!("{base}-{suffix}");
            if !taken.contains(&candidate) {
                return Ok(candidate);
            }
            suffix = suffix.saturating_add(1);
        }
    }

    pub fn create_worktree(&self, req: CreateWorktreeRequest) -> Result<WorktreeRecord> {
        let repo_root = normalize_absolute_or_clean_path(&req.repo_root.to_string_lossy())?;
        let worktree_path = normalize_absolute_or_clean_path(&req.worktree_path.to_string_lossy())?;

        if let Some(parent_id) = &req.parent_worktree_id {
            let _ = self.get_worktree(parent_id)?;
        }

        let (branch_name, is_creature_named) = match req.branch_name {
            Some(explicit) => {
                let trimmed = explicit.trim().to_string();
                if trimmed.is_empty() || trimmed.chars().any(char::is_control) {
                    return Err(OrcaIdeRuntimeError::invalid_argument(
                        "branch_name must be non-empty without control characters",
                    ));
                }
                let creature = is_creature_branch_name(&trimmed);
                (trimmed, creature)
            }
            None => (
                self.generate_creature_branch_name(
                    &req.existing_branches,
                    req.seed_hint.as_deref(),
                )?,
                true,
            ),
        };

        let now = now_epoch_ms();
        let id = req.id.unwrap_or_else(|| Uuid::new_v4().to_string());
        let title = req.title.unwrap_or_else(|| branch_name.clone());
        let record = WorktreeRecord {
            id: id.clone(),
            repo_root,
            worktree_path,
            branch_name: branch_name.clone(),
            title,
            parent_worktree_id: req.parent_worktree_id.clone(),
            project_group_id: req.project_group_id.clone(),
            is_creature_named,
            created_by_orca: req.created_by_orca,
            first_work_renamed: false,
            rename_failure_count: 0,
            last_rename_failure_at_ms: None,
            checkpoint_comment: None,
            workspace_status: Some(WorkspaceStatus::Todo.as_str().to_string()),
            pinned: false,
            sparse_preset_id: None,
            created_at_ms: now,
            updated_at_ms: now,
        };

        let payload = serde_json::to_string(&record)?;
        let conn = self.conn.lock();
        let exists: Option<String> = conn
            .query_row(
                "SELECT id FROM worktrees WHERE id = ?1",
                params![&id],
                |row| row.get(0),
            )
            .optional()?;
        if exists.is_some() {
            return Err(OrcaIdeRuntimeError::conflict(format!(
                "worktree '{id}' already exists"
            )));
        }
        conn.execute(
            "INSERT INTO worktrees
             (id, payload_json, branch_name, parent_worktree_id, project_group_id, created_at_ms, updated_at_ms)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            params![
                &record.id,
                &payload,
                &record.branch_name,
                &record.parent_worktree_id,
                &record.project_group_id,
                record.created_at_ms,
                record.updated_at_ms
            ],
        )?;
        Ok(record)
    }

    pub fn get_worktree(&self, id: &str) -> Result<WorktreeRecord> {
        let conn = self.conn.lock();
        let payload: Option<String> = conn
            .query_row(
                "SELECT payload_json FROM worktrees WHERE id = ?1",
                params![id],
                |row| row.get(0),
            )
            .optional()?;
        let Some(json) = payload else {
            return Err(OrcaIdeRuntimeError::not_found(format!(
                "worktree '{id}' not found"
            )));
        };
        Ok(serde_json::from_str(&json)?)
    }

    pub fn list_worktrees(&self) -> Result<Vec<WorktreeRecord>> {
        let conn = self.conn.lock();
        let mut stmt =
            conn.prepare("SELECT payload_json FROM worktrees ORDER BY created_at_ms ASC, id ASC")?;
        let rows = stmt.query_map([], |row| row.get::<_, String>(0))?;
        let mut out = Vec::new();
        for row in rows {
            out.push(serde_json::from_str(&row?)?);
        }
        Ok(out)
    }

    pub fn list_child_worktrees(&self, parent_worktree_id: &str) -> Result<Vec<WorktreeRecord>> {
        let conn = self.conn.lock();
        let mut stmt = conn.prepare(
            "SELECT payload_json FROM worktrees
             WHERE parent_worktree_id = ?1 ORDER BY created_at_ms ASC, id ASC",
        )?;
        let rows = stmt.query_map(params![parent_worktree_id], |row| row.get::<_, String>(0))?;
        let mut out = Vec::new();
        for row in rows {
            out.push(serde_json::from_str(&row?)?);
        }
        Ok(out)
    }

    /// Returns the worktree lineage chain from root ancestor down to `worktree_id`.
    pub fn get_worktree_lineage(&self, worktree_id: &str) -> Result<Vec<WorktreeRecord>> {
        let mut chain = Vec::new();
        let mut seen = HashSet::new();
        let mut cursor = Some(worktree_id.to_string());
        while let Some(id) = cursor {
            if !seen.insert(id.clone()) {
                break;
            }
            let wt = self.get_worktree(&id)?;
            cursor = wt.parent_worktree_id.clone();
            chain.push(wt);
        }
        chain.reverse();
        Ok(chain)
    }

    fn save_worktree(&self, record: &WorktreeRecord) -> Result<()> {
        let payload = serde_json::to_string(record)?;
        let conn = self.conn.lock();
        conn.execute(
            "UPDATE worktrees
             SET payload_json = ?2, branch_name = ?3, parent_worktree_id = ?4,
                 project_group_id = ?5, updated_at_ms = ?6
             WHERE id = ?1",
            params![
                &record.id,
                &payload,
                &record.branch_name,
                &record.parent_worktree_id,
                &record.project_group_id,
                record.updated_at_ms
            ],
        )?;
        Ok(())
    }
}

impl WorktreeManager {
    // -----------------------------------------------------------------------
    // First-Work Auto-Rename (Creature -> Semantic Branch/Folder/Title)
    // -----------------------------------------------------------------------

    /// Evaluates all 6 safety guards (working state, creation provenance, creature name,
    /// checkout lock, upstream published, failure backoff) and applies a semantic rename.
    pub fn evaluate_and_apply_first_work_rename(
        &self,
        ctx: &FirstWorkRenameContext,
    ) -> Result<FirstWorkRenameOutcome> {
        let mut wt = self.get_worktree(&ctx.worktree_id)?;

        if ctx.agent_state != AgentRunState::Working {
            return Ok(FirstWorkRenameOutcome::Skipped {
                reason: FirstWorkRenameSkipReason::NotWorking,
            });
        }
        if !wt.created_by_orca {
            return Ok(FirstWorkRenameOutcome::Skipped {
                reason: FirstWorkRenameSkipReason::NotCreatedByOrca,
            });
        }
        if wt.first_work_renamed {
            return Ok(FirstWorkRenameOutcome::Skipped {
                reason: FirstWorkRenameSkipReason::AlreadyRenamed,
            });
        }
        if !wt.is_creature_named || !is_creature_branch_name(&wt.branch_name) {
            return Ok(FirstWorkRenameOutcome::Skipped {
                reason: FirstWorkRenameSkipReason::NotCreatureNamed,
            });
        }
        if ctx.checkout_locked
            || has_git_checkout_lock(Path::new(&wt.repo_root), Path::new(&wt.worktree_path))
        {
            return Ok(FirstWorkRenameOutcome::Skipped {
                reason: FirstWorkRenameSkipReason::CheckoutLocked,
            });
        }
        if ctx.upstream_published {
            return Ok(FirstWorkRenameOutcome::Skipped {
                reason: FirstWorkRenameSkipReason::UpstreamPublished,
            });
        }
        if wt.rename_failure_count > 0
            && let Some(last_fail) = wt.last_rename_failure_at_ms
        {
            let backoff = compute_rename_backoff_ms(ctx.base_backoff_ms, wt.rename_failure_count);
            let retry_at = last_fail.saturating_add(backoff);
            if ctx.now_ms < retry_at {
                return Ok(FirstWorkRenameOutcome::Skipped {
                    reason: FirstWorkRenameSkipReason::InFailureBackoff {
                        retry_after_ms: retry_at.saturating_sub(ctx.now_ms),
                    },
                });
            }
        }

        let base_slug = slugify_semantic_branch(&ctx.prompt_summary);
        if base_slug.is_empty() {
            return Ok(FirstWorkRenameOutcome::Skipped {
                reason: FirstWorkRenameSkipReason::EmptySemanticSlug,
            });
        }

        let mut taken: HashSet<String> = ctx.existing_branches.iter().cloned().collect();
        for existing in self.list_worktrees()? {
            if existing.id != wt.id {
                taken.insert(existing.branch_name);
            }
        }
        let mut new_branch = base_slug.clone();
        let mut suffix = 2u32;
        while taken.contains(&new_branch) {
            new_branch = format!("{base_slug}-{suffix}");
            suffix = suffix.saturating_add(1);
        }

        let old_branch = wt.branch_name.clone();
        let old_path = wt.worktree_path.clone();
        let old_path_buf = PathBuf::from(&old_path);
        let new_path_buf = match old_path_buf.parent() {
            Some(parent) => parent.join(&new_branch),
            None => PathBuf::from(&new_branch),
        };
        let new_path = normalize_absolute_or_clean_path(&new_path_buf.to_string_lossy())?;

        if ctx.rename_directory_on_disk
            && old_path_buf.exists()
            && old_path_buf != new_path_buf
            && let Err(err) = std::fs::rename(&old_path_buf, &new_path_buf)
        {
            wt.rename_failure_count = wt.rename_failure_count.saturating_add(1);
            wt.last_rename_failure_at_ms = Some(ctx.now_ms);
            wt.updated_at_ms = ctx.now_ms;
            self.save_worktree(&wt)?;
            let backoff =
                compute_rename_backoff_ms(ctx.base_backoff_ms, wt.rename_failure_count);
            return Ok(FirstWorkRenameOutcome::Failed {
                error: err.to_string(),
                failure_count: wt.rename_failure_count,
                retry_after_ms: backoff,
            });
        }

        let new_title = ctx.prompt_summary.trim().chars().take(72).collect::<String>();
        wt.branch_name = new_branch.clone();
        wt.worktree_path = new_path.clone();
        wt.title = if new_title.is_empty() {
            new_branch.clone()
        } else {
            new_title.clone()
        };
        wt.is_creature_named = false;
        wt.first_work_renamed = true;
        wt.updated_at_ms = ctx.now_ms;
        self.save_worktree(&wt)?;

        Ok(FirstWorkRenameOutcome::Renamed {
            old_branch,
            new_branch,
            old_path,
            new_path,
            new_title: wt.title,
        })
    }

    /// Records a rename failure for `worktree_id` so subsequent attempts respect exponential backoff.
    pub fn record_first_work_rename_failure(
        &self,
        worktree_id: &str,
        now_ms: i64,
    ) -> Result<WorktreeRecord> {
        let mut wt = self.get_worktree(worktree_id)?;
        wt.rename_failure_count = wt.rename_failure_count.saturating_add(1);
        wt.last_rename_failure_at_ms = Some(now_ms);
        wt.updated_at_ms = now_ms;
        self.save_worktree(&wt)?;
        Ok(wt)
    }

    // -----------------------------------------------------------------------
    // Interrupted Worktree-Removal Recovery Ledger
    // -----------------------------------------------------------------------

    /// Records an in-progress removal entry BEFORE destructive worktree removal begins.
    pub fn begin_removal(
        &self,
        worktree_id: &str,
        now_ms: i64,
    ) -> Result<WorktreeRemovalEntry> {
        let wt = self.get_worktree(worktree_id)?;
        let entry = WorktreeRemovalEntry {
            removal_id: Uuid::new_v4().to_string(),
            worktree_id: wt.id,
            worktree_path: wt.worktree_path,
            branch_name: wt.branch_name,
            repo_root: wt.repo_root,
            state: RemovalLedgerState::InProgress,
            error_message: None,
            started_at_ms: now_ms,
            updated_at_ms: now_ms,
        };
        let conn = self.conn.lock();
        conn.execute(
            "INSERT INTO removal_ledger
             (removal_id, worktree_id, worktree_path, branch_name, repo_root, state, error_message, started_at_ms, updated_at_ms)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
            params![
                &entry.removal_id,
                &entry.worktree_id,
                &entry.worktree_path,
                &entry.branch_name,
                &entry.repo_root,
                entry.state.as_str(),
                &entry.error_message,
                entry.started_at_ms,
                entry.updated_at_ms
            ],
        )?;
        Ok(entry)
    }

    /// Marks a ledger removal entry completed and removes the worktree record.
    pub fn complete_removal(
        &self,
        removal_id: &str,
        now_ms: i64,
    ) -> Result<WorktreeRemovalEntry> {
        let conn = self.conn.lock();
        let mut entry = load_removal_entry(&conn, removal_id)?;
        entry.state = RemovalLedgerState::Completed;
        entry.error_message = None;
        entry.updated_at_ms = now_ms;
        conn.execute(
            "UPDATE removal_ledger SET state = ?2, error_message = NULL, updated_at_ms = ?3
             WHERE removal_id = ?1",
            params![&entry.removal_id, entry.state.as_str(), entry.updated_at_ms],
        )?;
        conn.execute(
            "DELETE FROM worktrees WHERE id = ?1",
            params![&entry.worktree_id],
        )?;
        Ok(entry)
    }

    pub fn fail_removal(
        &self,
        removal_id: &str,
        error_message: impl Into<String>,
        now_ms: i64,
    ) -> Result<WorktreeRemovalEntry> {
        let conn = self.conn.lock();
        let mut entry = load_removal_entry(&conn, removal_id)?;
        entry.state = RemovalLedgerState::Failed;
        entry.error_message = Some(error_message.into());
        entry.updated_at_ms = now_ms;
        conn.execute(
            "UPDATE removal_ledger SET state = ?2, error_message = ?3, updated_at_ms = ?4
             WHERE removal_id = ?1",
            params![
                &entry.removal_id,
                entry.state.as_str(),
                &entry.error_message,
                entry.updated_at_ms
            ],
        )?;
        Ok(entry)
    }

    pub fn list_removal_ledger(&self) -> Result<Vec<WorktreeRemovalEntry>> {
        let conn = self.conn.lock();
        let mut stmt = conn.prepare(
            "SELECT removal_id, worktree_id, worktree_path, branch_name, repo_root, state, error_message, started_at_ms, updated_at_ms
             FROM removal_ledger ORDER BY started_at_ms ASC, removal_id ASC",
        )?;
        let rows = stmt.query_map([], map_removal_row)?;
        let mut out = Vec::new();
        for row in rows {
            out.push(row?);
        }
        Ok(out)
    }
}

fn map_removal_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<WorktreeRemovalEntry> {
    let raw_state: String = row.get(5)?;
    Ok(WorktreeRemovalEntry {
        removal_id: row.get(0)?,
        worktree_id: row.get(1)?,
        worktree_path: row.get(2)?,
        branch_name: row.get(3)?,
        repo_root: row.get(4)?,
        state: RemovalLedgerState::parse(&raw_state),
        error_message: row.get(6)?,
        started_at_ms: row.get(7)?,
        updated_at_ms: row.get(8)?,
    })
}

fn load_removal_entry(conn: &Connection, removal_id: &str) -> Result<WorktreeRemovalEntry> {
    let entry = conn
        .query_row(
            "SELECT removal_id, worktree_id, worktree_path, branch_name, repo_root, state, error_message, started_at_ms, updated_at_ms
             FROM removal_ledger WHERE removal_id = ?1",
            params![removal_id],
            map_removal_row,
        )
        .optional()?;
    entry.ok_or_else(|| {
        OrcaIdeRuntimeError::not_found(format!("removal ledger entry '{removal_id}' not found"))
    })
}

fn safe_remove_worktree_dir(path: &Path) -> std::io::Result<()> {
    if !path.exists() {
        return Ok(());
    }
    // Never allow removing filesystem root.
    if path.parent().is_none() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::PermissionDenied,
            "refusing to remove filesystem root",
        ));
    }
    if path.is_dir() {
        std::fs::remove_dir_all(path)
    } else {
        std::fs::remove_file(path)
    }
}

impl WorktreeManager {
    /// Resumes any `Pending` or `InProgress` worktree removals left behind by a crash/restart.
    pub fn recover_interrupted_removals(&self, now_ms: i64) -> Result<RemovalRecoveryReport> {
        let incomplete: Vec<WorktreeRemovalEntry> = self
            .list_removal_ledger()?
            .into_iter()
            .filter(|e| {
                matches!(
                    e.state,
                    RemovalLedgerState::Pending | RemovalLedgerState::InProgress
                )
            })
            .collect();

        let mut report = RemovalRecoveryReport::default();
        for entry in incomplete {
            let path = PathBuf::from(&entry.worktree_path);
            match safe_remove_worktree_dir(&path) {
                Ok(()) => {
                    self.complete_removal(&entry.removal_id, now_ms)?;
                    report.recovered_removal_ids.push(entry.removal_id);
                }
                Err(err) => {
                    let msg = err.to_string();
                    self.fail_removal(&entry.removal_id, &msg, now_ms)?;
                    report.failed_removals.push((entry.removal_id, msg));
                }
            }
        }
        Ok(report)
    }

    // -----------------------------------------------------------------------
    // Sparse Checkout Presets
    // -----------------------------------------------------------------------

    /// Saves a sparse checkout preset after validating EVERY pattern via
    /// `orca_core::workspace_layout::validate_worktree_relative_path`.
    pub fn save_sparse_preset(&self, preset: SparseCheckoutPreset) -> Result<SparseCheckoutPreset> {
        if preset.id.trim().is_empty() || preset.name.trim().is_empty() {
            return Err(OrcaIdeRuntimeError::invalid_argument(
                "sparse checkout preset id and name must be non-empty",
            ));
        }
        if preset.patterns.is_empty() {
            return Err(OrcaIdeRuntimeError::invalid_argument(
                "sparse checkout preset must include at least one pattern",
            ));
        }
        let mut normalized_patterns = Vec::with_capacity(preset.patterns.len());
        for raw in &preset.patterns {
            let clean = validate_worktree_relative_path(raw).map_err(|e| {
                OrcaIdeRuntimeError::invalid_argument(format!(
                    "invalid sparse checkout pattern '{raw}': {e}"
                ))
            })?;
            normalized_patterns.push(clean);
        }
        let sanitized = SparseCheckoutPreset {
            patterns: normalized_patterns,
            ..preset
        };
        let patterns_json = serde_json::to_string(&sanitized.patterns)?;
        let conn = self.conn.lock();
        conn.execute(
            "INSERT OR REPLACE INTO sparse_presets
             (id, name, patterns_json, cone_mode, created_at_ms, updated_at_ms)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![
                &sanitized.id,
                &sanitized.name,
                &patterns_json,
                i32::from(sanitized.cone_mode),
                sanitized.created_at_ms,
                sanitized.updated_at_ms
            ],
        )?;
        Ok(sanitized)
    }

    pub fn get_sparse_preset(&self, id: &str) -> Result<SparseCheckoutPreset> {
        let conn = self.conn.lock();
        let row: Option<(String, String, String, i32, i64, i64)> = conn
            .query_row(
                "SELECT id, name, patterns_json, cone_mode, created_at_ms, updated_at_ms
                 FROM sparse_presets WHERE id = ?1",
                params![id],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?, r.get(5)?)),
            )
            .optional()?;
        let Some((id, name, patterns_json, cone_mode, created_at_ms, updated_at_ms)) = row else {
            return Err(OrcaIdeRuntimeError::not_found(format!(
                "sparse preset '{id}' not found"
            )));
        };
        Ok(SparseCheckoutPreset {
            id,
            name,
            patterns: serde_json::from_str(&patterns_json)?,
            cone_mode: cone_mode != 0,
            created_at_ms,
            updated_at_ms,
        })
    }

    pub fn list_sparse_presets(&self) -> Result<Vec<SparseCheckoutPreset>> {
        let conn = self.conn.lock();
        let mut stmt = conn.prepare(
            "SELECT id, name, patterns_json, cone_mode, created_at_ms, updated_at_ms
             FROM sparse_presets ORDER BY name ASC, id ASC",
        )?;
        let rows = stmt.query_map([], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, String>(2)?,
                r.get::<_, i32>(3)?,
                r.get::<_, i64>(4)?,
                r.get::<_, i64>(5)?,
            ))
        })?;
        let mut out = Vec::new();
        for row in rows {
            let (id, name, patterns_json, cone_mode, created_at_ms, updated_at_ms) = row?;
            out.push(SparseCheckoutPreset {
                id,
                name,
                patterns: serde_json::from_str(&patterns_json)?,
                cone_mode: cone_mode != 0,
                created_at_ms,
                updated_at_ms,
            });
        }
        Ok(out)
    }

    pub fn apply_sparse_preset_to_worktree(
        &self,
        worktree_id: &str,
        preset_id: &str,
        now_ms: i64,
    ) -> Result<WorktreeRecord> {
        let _ = self.get_sparse_preset(preset_id)?;
        let mut wt = self.get_worktree(worktree_id)?;
        wt.sparse_preset_id = Some(preset_id.to_string());
        wt.updated_at_ms = now_ms;
        self.save_worktree(&wt)?;
        Ok(wt)
    }

    // -----------------------------------------------------------------------
    // `orca.yaml` Trust Approval Store
    // -----------------------------------------------------------------------

    pub fn approve_orca_yaml_trust(
        &self,
        repo_root: impl AsRef<Path>,
        trust_hash: impl Into<String>,
        now_ms: i64,
    ) -> Result<()> {
        let norm_repo = normalize_absolute_or_clean_path(&repo_root.as_ref().to_string_lossy())?;
        let hash = trust_hash.into();
        if hash.len() != 64 || !hash.chars().all(|c| c.is_ascii_hexdigit()) {
            return Err(OrcaIdeRuntimeError::invalid_argument(
                "setup trust hash must be a 64-character hex SHA-256 string",
            ));
        }
        let conn = self.conn.lock();
        conn.execute(
            "INSERT OR REPLACE INTO orca_yaml_trust (repo_root, approved_trust_hash, approved_at_ms)
             VALUES (?1, ?2, ?3)",
            params![norm_repo, hash, now_ms],
        )?;
        Ok(())
    }

    pub fn approved_orca_yaml_trust_hash(
        &self,
        repo_root: impl AsRef<Path>,
    ) -> Result<Option<String>> {
        let norm_repo = normalize_absolute_or_clean_path(&repo_root.as_ref().to_string_lossy())?;
        let conn = self.conn.lock();
        let hash: Option<String> = conn
            .query_row(
                "SELECT approved_trust_hash FROM orca_yaml_trust WHERE repo_root = ?1",
                params![norm_repo],
                |row| row.get(0),
            )
            .optional()?;
        Ok(hash)
    }

    // -----------------------------------------------------------------------
    // Checkpoints, Kanban `workspace_status`, Disk Usage & Guarded Cleanup
    // -----------------------------------------------------------------------

    pub fn set_worktree_checkpoint(
        &self,
        worktree_id: &str,
        checkpoint_comment: Option<String>,
        workspace_status: Option<WorkspaceStatus>,
        now_ms: i64,
    ) -> Result<WorktreeRecord> {
        let mut wt = self.get_worktree(worktree_id)?;
        wt.checkpoint_comment = checkpoint_comment;
        if let Some(status) = workspace_status {
            wt.workspace_status = Some(status.as_str().to_string());
        }
        wt.updated_at_ms = now_ms;
        self.save_worktree(&wt)?;
        Ok(wt)
    }

    /// Analyzes disk space consumed by a worktree or folder workspace using `walkdir`.
    pub fn analyze_disk_usage(&self, root_path: impl AsRef<Path>) -> Result<DiskUsageReport> {
        let root = root_path.as_ref();
        if !root.exists() {
            return Err(OrcaIdeRuntimeError::not_found(format!(
                "path '{}' does not exist",
                root.display()
            )));
        }
        let mut report = DiskUsageReport {
            root_path: root.to_string_lossy().into_owned(),
            ..DiskUsageReport::default()
        };
        let mut files: Vec<DiskUsageEntry> = Vec::new();

        for entry_res in WalkDir::new(root).follow_links(false) {
            let entry = entry_res.map_err(|e| {
                OrcaIdeRuntimeError::Io(std::io::Error::other(format!(
                    "disk usage walk failed: {e}"
                )))
            })?;
            if entry.depth() == 0 {
                continue;
            }
            if entry.file_type().is_dir() {
                report.dir_count = report.dir_count.saturating_add(1);
                continue;
            }
            let bytes = entry.metadata().map(|m| m.len()).unwrap_or(0);
            report.file_count = report.file_count.saturating_add(1);
            report.total_bytes = report.total_bytes.saturating_add(bytes);

            let rel_str = entry
                .path()
                .strip_prefix(root)
                .map(|p| p.to_string_lossy().replace('\\', "/"))
                .unwrap_or_default();
            if rel_str == ".git" || rel_str.starts_with(".git/") {
                report.git_bytes = report.git_bytes.saturating_add(bytes);
            }
            files.push(DiskUsageEntry {
                relative_path: rel_str,
                bytes,
            });
        }

        files.sort_by(|a, b| {
            b.bytes
                .cmp(&a.bytes)
                .then_with(|| a.relative_path.cmp(&b.relative_path))
        });
        files.truncate(10);
        report.largest_files = files;
        Ok(report)
    }

    /// Cleans up a worktree while refusing removal if any dirty/unsaved editors exist
    /// unless `force_discard_unsaved == true`. Records durability via the removal ledger.
    pub fn cleanup_worktree(
        &self,
        req: &CleanupWorktreeRequest,
    ) -> Result<CleanupWorktreeOutcome> {
        let wt = self.get_worktree(&req.worktree_id)?;
        if !req.unsaved_editor_tabs.is_empty() && !req.force_discard_unsaved {
            return Err(OrcaIdeRuntimeError::conflict(format!(
                "cleanup refused for worktree '{}': {} unsaved editor tab(s) present ({})",
                wt.id,
                req.unsaved_editor_tabs.len(),
                req.unsaved_editor_tabs.join(", ")
            )));
        }

        let path = PathBuf::from(&wt.worktree_path);
        let freed_bytes = if req.remove_directory_on_disk && path.exists() {
            self.analyze_disk_usage(&path)
                .map(|r| r.total_bytes)
                .unwrap_or(0)
        } else {
            0
        };

        let ledger = self.begin_removal(&wt.id, req.now_ms)?;
        if req.remove_directory_on_disk
            && let Err(err) = safe_remove_worktree_dir(&path)
        {
            let _ = self.fail_removal(&ledger.removal_id, err.to_string(), req.now_ms);
            return Err(OrcaIdeRuntimeError::Io(err));
        }
        self.complete_removal(&ledger.removal_id, req.now_ms)?;

        Ok(CleanupWorktreeOutcome {
            worktree_id: wt.id,
            removal_id: ledger.removal_id,
            freed_bytes,
            discarded_unsaved_count: req.unsaved_editor_tabs.len(),
        })
    }
}
