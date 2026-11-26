//! `orca.yaml` setup/teardown/archive runner, `sharedDirectories` symlink builder,
//! and `.worktreeinclude` copy execution engine (`F10`).

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::process::Command;

use orca_core::workspace_layout::{
    ORCA_YAML_MAX_SHARED_DIRS, OrcaYamlConfig, SetupAgentStartupPolicy, WorktreeCopyCandidate,
    WorktreeCopyPlan, is_posix_shebang_script, parse_orca_yaml_bytes, parse_worktree_include,
    plan_worktree_include_copy, setup_trust_hash, validate_worktree_relative_path,
};
use orca_core::OrcaIdeErrorCode;
use serde::{Deserialize, Serialize};
use walkdir::WalkDir;

use crate::{OrcaIdeRuntimeError, Result};

/// Precedence mode for merging shared `orca.yaml` vs local `orca.local.yaml` scripts.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ScriptOverrideMode {
    #[default]
    SharedOnly,
    LocalOnly,
    RunBoth,
}

/// Target execution platform for script-safety guards (notably Windows POSIX `#!` refusal).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ScriptPlatform {
    Unix,
    Windows,
}

impl ScriptPlatform {
    pub const fn current() -> Self {
        if cfg!(windows) {
            Self::Windows
        } else {
            Self::Unix
        }
    }
}

/// Lifecycle phase in `orca.yaml`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OrcaYamlScriptPhase {
    Setup,
    Teardown,
    Archive,
}

/// Request to plan or execute `orca.yaml` scripts for a worktree.
#[derive(Debug, Clone)]
pub struct OrcaYamlRunRequest {
    pub shared_yaml_bytes: Option<Vec<u8>>,
    pub local_yaml_bytes: Option<Vec<u8>>,
    pub override_mode: ScriptOverrideMode,
    pub phase: OrcaYamlScriptPhase,
    pub platform: ScriptPlatform,
    pub git_bash_available: bool,
    pub approved_trust_hash: Option<String>,
    pub require_trust_approval: bool,
}

/// Planned script step resolved after override merging and security validation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PlannedScriptStep {
    pub source: String,
    pub command: String,
    pub uses_posix_shebang: bool,
}

/// Deterministic plan produced before executing `orca.yaml` lifecycle scripts.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OrcaYamlRunnerPlan {
    pub trust_hash: String,
    pub startup_policy: SetupAgentStartupPolicy,
    pub auto_archive_days: Option<u32>,
    pub env: BTreeMap<String, String>,
    pub steps: Vec<PlannedScriptStep>,
    pub shared_directories: Vec<String>,
}

/// Result of executing a single planned script step.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExecutedScriptStep {
    pub source: String,
    pub command: String,
    pub exit_code: i32,
    pub stdout: String,
    pub stderr: String,
}

/// Outcome of running an `OrcaYamlRunnerPlan` inside a worktree directory.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OrcaYamlExecutionReport {
    pub trust_hash: String,
    pub startup_policy: SetupAgentStartupPolicy,
    pub succeeded: bool,
    pub executed_steps: Vec<ExecutedScriptStep>,
}

/// Result of creating one `sharedDirectories` symlink inside a worktree.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SharedDirectoryLinkResult {
    pub relative_path: String,
    pub source_path: PathBuf,
    pub target_path: PathBuf,
    pub created: bool,
}

/// Options governing `.worktreeinclude` scanning and physical copying.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorktreeIncludeCopyOptions {
    /// When `true` (default), symlinks encountered during `walkdir` scan are skipped and
    /// reported in `skipped_symlinks` rather than aborting the copy plan.
    /// When `false`, symlinks are forwarded to `plan_worktree_include_copy`, which rejects
    /// matched symlinks with `OrcaIdeErrorCode::CopyBudgetExceeded`.
    pub skip_symlinks: bool,
    /// Optional explicit set of repo-relative paths known to be git-ignored.
    /// When `None`, `.gitignore` / `git ls-files --others --ignored --exclude-standard`
    /// is probed automatically.
    pub git_ignored_paths: Option<BTreeSet<String>>,
}

impl Default for WorktreeIncludeCopyOptions {
    fn default() -> Self {
        Self {
            skip_symlinks: true,
            git_ignored_paths: None,
        }
    }
}

/// Outcome of scanning and physically copying `.worktreeinclude` files.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorktreeIncludeCopyReport {
    pub plan: WorktreeCopyPlan,
    pub copied_files: Vec<String>,
    pub created_dirs: Vec<String>,
    pub skipped_symlinks: Vec<String>,
    pub skipped_git_entries: usize,
}

fn extract_phase_scripts(config: &OrcaYamlConfig, phase: OrcaYamlScriptPhase) -> Vec<String> {
    match phase {
        OrcaYamlScriptPhase::Setup => config.setup.clone(),
        OrcaYamlScriptPhase::Teardown => config.teardown.clone(),
        OrcaYamlScriptPhase::Archive => config
            .scripts
            .get("archive")
            .cloned()
            .into_iter()
            .collect(),
    }
}

fn validate_script_platform(
    script: &str,
    platform: ScriptPlatform,
    git_bash_available: bool,
) -> Result<bool> {
    let uses_shebang = is_posix_shebang_script(script);
    if platform == ScriptPlatform::Windows && !git_bash_available && uses_shebang {
        return Err(OrcaIdeRuntimeError::permission_denied(
            "POSIX '#!' shebang script refused on Windows without Git Bash",
        ));
    }
    Ok(uses_shebang)
}

/// Parses shared/local `orca.yaml` documents, computes deterministic `setup_trust_hash`,
/// enforces trust approval, merges scripts per `ScriptOverrideMode`, and enforces the
/// Windows POSIX `#!` shebang refusal guard.
pub fn plan_orca_yaml_scripts(req: &OrcaYamlRunRequest) -> Result<OrcaYamlRunnerPlan> {
    let shared_cfg = match &req.shared_yaml_bytes {
        Some(bytes) => Some(parse_orca_yaml_bytes(bytes)?),
        None => None,
    };
    let local_cfg = match &req.local_yaml_bytes {
        Some(bytes) => Some(parse_orca_yaml_bytes(bytes)?),
        None => None,
    };

    let primary_cfg = match req.override_mode {
        ScriptOverrideMode::SharedOnly => shared_cfg.as_ref(),
        ScriptOverrideMode::LocalOnly => local_cfg.as_ref().or(shared_cfg.as_ref()),
        ScriptOverrideMode::RunBoth => shared_cfg.as_ref().or(local_cfg.as_ref()),
    };

    let mut effective_cfg = primary_cfg.cloned().unwrap_or_default();
    if let (Some(shared), Some(local)) = (&shared_cfg, &local_cfg)
        && req.override_mode == ScriptOverrideMode::RunBoth
    {
        effective_cfg.setup = shared
            .setup
            .iter()
            .chain(local.setup.iter())
            .cloned()
            .collect();
        effective_cfg.teardown = shared
            .teardown
            .iter()
            .chain(local.teardown.iter())
            .cloned()
            .collect();
        for (k, v) in &local.env {
            effective_cfg.env.insert(k.clone(), v.clone());
        }
        for (k, v) in &local.scripts {
            effective_cfg.scripts.insert(k.clone(), v.clone());
        }
        if local.setup_agent_startup_policy == SetupAgentStartupPolicy::WaitForSetup {
            effective_cfg.setup_agent_startup_policy = SetupAgentStartupPolicy::WaitForSetup;
        }
    }

    let trust_hash = setup_trust_hash(&effective_cfg);
    if req.require_trust_approval
        && req.approved_trust_hash.as_deref() != Some(trust_hash.as_str())
    {
        return Err(OrcaIdeRuntimeError::ide(
            OrcaIdeErrorCode::ApprovalRequired,
            format!("orca.yaml setup trust hash '{trust_hash}' has not been approved"),
        ));
    }

    let mut steps = Vec::new();
    match req.override_mode {
        ScriptOverrideMode::SharedOnly => {
            if let Some(cfg) = &shared_cfg {
                for cmd in extract_phase_scripts(cfg, req.phase) {
                    let uses_posix_shebang =
                        validate_script_platform(&cmd, req.platform, req.git_bash_available)?;
                    steps.push(PlannedScriptStep {
                        source: "orca.yaml".to_string(),
                        command: cmd,
                        uses_posix_shebang,
                    });
                }
            }
        }
        ScriptOverrideMode::LocalOnly => {
            if let Some(cfg) = &local_cfg {
                for cmd in extract_phase_scripts(cfg, req.phase) {
                    let uses_posix_shebang =
                        validate_script_platform(&cmd, req.platform, req.git_bash_available)?;
                    steps.push(PlannedScriptStep {
                        source: "orca.local.yaml".to_string(),
                        command: cmd,
                        uses_posix_shebang,
                    });
                }
            }
        }
        ScriptOverrideMode::RunBoth => {
            if let Some(cfg) = &shared_cfg {
                for cmd in extract_phase_scripts(cfg, req.phase) {
                    let uses_posix_shebang =
                        validate_script_platform(&cmd, req.platform, req.git_bash_available)?;
                    steps.push(PlannedScriptStep {
                        source: "orca.yaml".to_string(),
                        command: cmd,
                        uses_posix_shebang,
                    });
                }
            }
            if let Some(cfg) = &local_cfg {
                for cmd in extract_phase_scripts(cfg, req.phase) {
                    let uses_posix_shebang =
                        validate_script_platform(&cmd, req.platform, req.git_bash_available)?;
                    steps.push(PlannedScriptStep {
                        source: "orca.local.yaml".to_string(),
                        command: cmd,
                        uses_posix_shebang,
                    });
                }
            }
        }
    }

    if effective_cfg.shared_directories.len() > ORCA_YAML_MAX_SHARED_DIRS {
        return Err(OrcaIdeRuntimeError::invalid_argument(format!(
            "sharedDirectories count {} exceeds limit {ORCA_YAML_MAX_SHARED_DIRS}",
            effective_cfg.shared_directories.len()
        )));
    }
    for rel in &effective_cfg.shared_directories {
        validate_worktree_relative_path(rel).map_err(|e| {
            OrcaIdeRuntimeError::invalid_argument(format!(
                "invalid sharedDirectories entry '{rel}': {e}"
            ))
        })?;
    }

    Ok(OrcaYamlRunnerPlan {
        trust_hash,
        startup_policy: effective_cfg.setup_agent_startup_policy,
        auto_archive_days: effective_cfg.auto_archive_days,
        env: effective_cfg.env,
        steps,
        shared_directories: effective_cfg.shared_directories,
    })
}

/// Executes a validated `OrcaYamlRunnerPlan` inside `worktree_dir`, stopping on first non-zero exit.
pub fn execute_orca_yaml_scripts(
    worktree_dir: &Path,
    plan: &OrcaYamlRunnerPlan,
    platform: ScriptPlatform,
    git_bash_available: bool,
) -> Result<OrcaYamlExecutionReport> {
    if !worktree_dir.is_dir() {
        return Err(OrcaIdeRuntimeError::not_found(format!(
            "worktree directory '{}' does not exist",
            worktree_dir.display()
        )));
    }

    let mut executed_steps = Vec::with_capacity(plan.steps.len());
    let mut succeeded = true;

    for step in &plan.steps {
        validate_script_platform(&step.command, platform, git_bash_available)?;
        let output = if platform == ScriptPlatform::Windows {
            Command::new("cmd")
                .args(["/C", &step.command])
                .current_dir(worktree_dir)
                .envs(&plan.env)
                .output()?
        } else {
            Command::new("sh")
                .args(["-c", &step.command])
                .current_dir(worktree_dir)
                .envs(&plan.env)
                .output()?
        };
        let exit_code = output.status.code().unwrap_or(-1);
        executed_steps.push(ExecutedScriptStep {
            source: step.source.clone(),
            command: step.command.clone(),
            exit_code,
            stdout: String::from_utf8_lossy(&output.stdout).into_owned(),
            stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
        });
        if exit_code != 0 {
            succeeded = false;
            break;
        }
    }

    Ok(OrcaYamlExecutionReport {
        trust_hash: plan.trust_hash.clone(),
        startup_policy: plan.startup_policy,
        succeeded,
        executed_steps,
    })
}

/// Convenience wrapper parsing UTF-8 `orca.yaml` text via `orca_core::workspace_layout::parse_orca_yaml`.
pub fn parse_orca_yaml_text(input: &str) -> Result<OrcaYamlConfig> {
    Ok(orca_core::workspace_layout::parse_orca_yaml(input)?)
}

/// Creates symlinks in `target_worktree_root` pointing to `sharedDirectories` inside
/// `source_repo_root`. Enforces `ORCA_YAML_MAX_SHARED_DIRS` (100) and rejects path traversal,
/// absolute paths, control chars, and `.git` segments.
pub fn link_shared_directories(
    source_repo_root: &Path,
    target_worktree_root: &Path,
    shared_dirs: &[String],
) -> Result<Vec<SharedDirectoryLinkResult>> {
    if shared_dirs.len() > ORCA_YAML_MAX_SHARED_DIRS {
        return Err(OrcaIdeRuntimeError::invalid_argument(format!(
            "sharedDirectories count {} exceeds cap {ORCA_YAML_MAX_SHARED_DIRS}",
            shared_dirs.len()
        )));
    }
    if !source_repo_root.is_dir() {
        return Err(OrcaIdeRuntimeError::not_found(format!(
            "source repository root '{}' does not exist",
            source_repo_root.display()
        )));
    }
    std::fs::create_dir_all(target_worktree_root)?;

    let mut results = Vec::with_capacity(shared_dirs.len());
    for raw_rel in shared_dirs {
        let norm_rel = validate_worktree_relative_path(raw_rel).map_err(|e| {
            OrcaIdeRuntimeError::invalid_argument(format!(
                "unsafe sharedDirectories path '{raw_rel}': {e}"
            ))
        })?;
        let source_path = source_repo_root.join(&norm_rel);
        let target_path = target_worktree_root.join(&norm_rel);

        if let Some(parent) = target_path.parent() {
            std::fs::create_dir_all(parent)?;
        }

        let created = if target_path.symlink_metadata().is_ok() {
            false
        } else {
            create_dir_symlink(&source_path, &target_path)?;
            true
        };

        results.push(SharedDirectoryLinkResult {
            relative_path: norm_rel,
            source_path,
            target_path,
            created,
        });
    }
    Ok(results)
}

#[cfg(unix)]
fn create_dir_symlink(source: &Path, target: &Path) -> Result<()> {
    std::os::unix::fs::symlink(source, target)?;
    Ok(())
}

#[cfg(not(unix))]
fn create_dir_symlink(_source: &Path, _target: &Path) -> Result<()> {
    Err(OrcaIdeRuntimeError::permission_denied(
        "sharedDirectories symlinks require Unix symlink support on this host",
    ))
}

fn discover_git_ignored_paths(source_repo_root: &Path) -> Option<BTreeSet<String>> {
    if !source_repo_root.join(".git").exists() {
        return None;
    }
    let output = Command::new("git")
        .args([
            "ls-files",
            "--others",
            "--ignored",
            "--exclude-standard",
            "-z",
        ])
        .current_dir(source_repo_root)
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let mut ignored = BTreeSet::new();
    for raw in output.stdout.split(|b| *b == 0) {
        if raw.is_empty() {
            continue;
        }
        let path_str = String::from_utf8_lossy(raw).replace('\\', "/");
        let trimmed = path_str.trim_matches('/').to_string();
        if !trimmed.is_empty() {
            ignored.insert(trimmed);
        }
    }
    Some(ignored)
}

fn is_path_git_ignored(
    rel_path: &str,
    is_dir: bool,
    explicit_ignored: Option<&BTreeSet<String>>,
    gitignore_rules: &[orca_core::workspace_layout::WorktreeIncludeRule],
) -> bool {
    if let Some(set) = explicit_ignored {
        if set.contains(rel_path) {
            return true;
        }
        if is_dir {
            let prefix = format!("{rel_path}/");
            if set.iter().any(|p| p.starts_with(&prefix)) {
                return true;
            }
        }
        // Check ancestor directory entries in set (e.g., `node_modules` listed as ignored dir).
        let mut cursor = rel_path;
        while let Some((parent, _)) = cursor.rsplit_once('/') {
            if set.contains(parent) {
                return true;
            }
            cursor = parent;
        }
        return false;
    }

    if gitignore_rules.is_empty() {
        return false;
    }
    let mut ignored = false;
    for rule in gitignore_rules {
        if rule.matches(rel_path, is_dir) {
            ignored = !rule.negated;
        }
    }
    ignored
}

/// Scans `source_repo_root` with `walkdir`, evaluates `.worktreeinclude` rules via
/// `parse_worktree_include` + `plan_worktree_include_copy` (enforcing 2 GiB / 50,000 entries /
/// 1,000 paths -> `OrcaIdeErrorCode::CopyBudgetExceeded`), skips `.git`, tracked files,
/// negated rules, and (by default) symlinks, and physically copies matched entries into
/// `target_worktree_root`.
pub fn execute_worktree_include_copy(
    source_repo_root: &Path,
    target_worktree_root: &Path,
    include_rules_raw: &str,
    options: &WorktreeIncludeCopyOptions,
) -> Result<WorktreeIncludeCopyReport> {
    if !source_repo_root.is_dir() {
        return Err(OrcaIdeRuntimeError::not_found(format!(
            "source repository root '{}' does not exist",
            source_repo_root.display()
        )));
    }
    std::fs::create_dir_all(target_worktree_root)?;

    let rules = parse_worktree_include(include_rules_raw);
    let discovered_ignored = if options.git_ignored_paths.is_none() {
        discover_git_ignored_paths(source_repo_root)
    } else {
        None
    };
    let effective_ignored = options
        .git_ignored_paths
        .as_ref()
        .or(discovered_ignored.as_ref());

    let gitignore_rules = if effective_ignored.is_none() {
        let gitignore_path = source_repo_root.join(".gitignore");
        std::fs::read_to_string(&gitignore_path)
            .map(|raw| parse_worktree_include(&raw))
            .unwrap_or_default()
    } else {
        Vec::new()
    };

    let mut candidates = Vec::new();
    let mut skipped_symlinks = Vec::new();
    let mut skipped_git_entries = 0usize;

    let walker = WalkDir::new(source_repo_root)
        .follow_links(false)
        .into_iter()
        .filter_entry(|entry| {
            if entry.depth() == 0 {
                return true;
            }
            if entry.file_name().to_str() == Some(".git") {
                return false;
            }
            true
        });

    for entry_res in walker {
        let entry = entry_res.map_err(|e| {
            OrcaIdeRuntimeError::Io(std::io::Error::other(format!("walkdir scan failed: {e}")))
        })?;
        if entry.depth() == 0 {
            continue;
        }
        let Ok(rel) = entry.path().strip_prefix(source_repo_root) else {
            continue;
        };
        let rel_str = rel.to_string_lossy().replace('\\', "/");
        if rel_str.split('/').any(|seg| seg == ".git") {
            skipped_git_entries = skipped_git_entries.saturating_add(1);
            continue;
        }

        let is_symlink = entry.path_is_symlink();
        if is_symlink && options.skip_symlinks {
            skipped_symlinks.push(rel_str);
            continue;
        }

        let is_dir = entry.file_type().is_dir();
        let size_bytes = if is_dir || is_symlink {
            0
        } else {
            entry.metadata().map(|m| m.len()).unwrap_or(0)
        };
        let is_git_ignored =
            is_path_git_ignored(&rel_str, is_dir, effective_ignored, &gitignore_rules);

        candidates.push(WorktreeCopyCandidate {
            relative_path: rel_str,
            size_bytes,
            is_dir,
            is_symlink,
            is_git_ignored,
            can_clone_cow: false,
        });
    }

    // Count `.git` directory presence cleanly if excluded by filter_entry.
    if source_repo_root.join(".git").exists() && skipped_git_entries == 0 {
        skipped_git_entries = 1;
    }

    let plan = plan_worktree_include_copy(&rules, &candidates)?;
    let mut copied_files = Vec::new();
    let mut created_dirs = Vec::new();

    for action in &plan.actions {
        let safe_rel = validate_worktree_relative_path(&action.relative_path).map_err(|e| {
            OrcaIdeRuntimeError::ide(
                OrcaIdeErrorCode::CopyBudgetExceeded,
                format!("unsafe copy action '{}': {e}", action.relative_path),
            )
        })?;
        let src = source_repo_root.join(&safe_rel);
        let dst = target_worktree_root.join(&safe_rel);

        if action.is_dir {
            std::fs::create_dir_all(&dst)?;
            created_dirs.push(safe_rel);
        } else {
            if let Some(parent) = dst.parent() {
                std::fs::create_dir_all(parent)?;
            }
            std::fs::copy(&src, &dst)?;
            copied_files.push(safe_rel);
        }
    }

    Ok(WorktreeIncludeCopyReport {
        plan,
        copied_files,
        created_dirs,
        skipped_symlinks,
        skipped_git_entries,
    })
}
