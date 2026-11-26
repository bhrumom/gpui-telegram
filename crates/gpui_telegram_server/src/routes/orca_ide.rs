//! Orca Agent IDE REST routes (`F10`, `F13`, `F15`–`F20`):
//! Strict `orca.yaml` parser, Worktrees / `.worktreeinclude` / Ports / Cleanup,
//! Diff Annotations & Batch Prompt, AI Vault Search & Indexing, Plugin System V2,
//! Shareable Artifacts (zero `edit_token` leak), Computer Use Guard, 9-Provider Usage Roster,
//! 8-Condition Hibernation Check, E2EE v2 Handshake, and ProfileState Authority.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use axum::{
    body::Bytes,
    extract::{Path as AxumPath, Query, State},
    http::{header, HeaderMap, StatusCode},
    response::{IntoResponse, Json, Response},
    routing::{get, patch, post},
    Router,
};
use orca_core::orchestration_protocol::{
    ComputerUseAction, ComputerUsePolicy, ComputerUseRequest, ComputerUseTargetKind,
    DiffAnnotation, DiffSide, LineAttribution, PluginManifestV2,
};
use orca_core::workspace_layout::{
    FolderWorkspace, KeyChord, OrcaYamlConfig, is_system_shortcut_blocked, setup_trust_hash,
};
use orca_ide::accounts_usage::ProviderQuotaSnapshot;
use orca_ide::diff_review::diff_annotation_to_wire_json;
use orca_ide::hibernation::HibernationCandidateState;
use orca_ide::plugin_host::{
    PluginContributionPoint, PluginHostV2, installed_plugin_to_wire_json,
    plugin_validation_to_wire_json, scan_plugin_package_dir,
};
use orca_ide::profile_state::DirtyDomainMutation;
use orca_ide::rpc_dispatcher::E2eeHandshakeInitRequest;
use orca_ide::system_services::{ArtifactKind, PublishArtifactRequest};
use orca_ide::vault_search::{VaultMessageInput, VaultSearchRequest};
use orca_ide::worktree_manager::{
    execute_worktree_include_copy, parse_orca_yaml_text, CleanupWorktreeRequest,
    CreateWorktreeRequest, WorkspaceStatus, WorktreeIncludeCopyOptions, WorktreeRecord,
};
use serde::Deserialize;
use serde_json::{json, Value};

use crate::routes::orca_error::{map_ide_err, orca_err, parse_json_body, parse_optional_json_body};
use crate::server::{AppState, AuthBearer};

fn now_epoch_ms() -> i64 {
    chrono::Utc::now().timestamp_millis()
}

// ---------------------------------------------------------------------------
// 1. Strict `orca.yaml` Parser
// ---------------------------------------------------------------------------

fn orca_yaml_to_wire_json(cfg: &OrcaYamlConfig) -> Value {
    let trust_hash = setup_trust_hash(cfg);
    let mut val = serde_json::to_value(cfg).unwrap_or_else(|_| json!({}));
    if let Some(obj) = val.as_object_mut() {
        obj.insert(
            "setupAgentStartupPolicy".into(),
            json!(cfg.setup_agent_startup_policy),
        );
        obj.insert("issueCommand".into(), json!(cfg.issue_command));
        obj.insert("sharedDirectories".into(), json!(cfg.shared_directories));
        obj.insert(
            "worktree".into(),
            json!({ "sharedDirectories": cfg.shared_directories }),
        );
        obj.insert("defaultTabs".into(), json!(cfg.default_tabs));
        obj.insert("environmentRecipes".into(), json!(cfg.environment_recipes));
        obj.insert("setupTrustHash".into(), json!(trust_hash));
        obj.insert("setup_trust_hash".into(), json!(trust_hash));
    }
    val
}

/// `POST /api/v1/orca/ide/orca-yaml/parse` (`200 OK`).
/// Accepts JSON `{yaml | content | path}` OR raw `text/plain` / YAML body.
pub async fn parse_orca_yaml(
    _auth: AuthBearer,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    if body.is_empty() {
        return orca_err(
            StatusCode::BAD_REQUEST,
            "invalid_argument",
            "orca.yaml body cannot be empty.",
        );
    }

    let content_type = headers
        .get(header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("")
        .to_ascii_lowercase();

    let raw_yaml: String = if content_type.contains("text/plain")
        || content_type.contains("yaml")
        || content_type.contains("yml")
    {
        match std::str::from_utf8(&body) {
            Ok(s) => s.to_string(),
            Err(err) => {
                return orca_err(
                    StatusCode::BAD_REQUEST,
                    "invalid_argument",
                    format!("orca.yaml body must be valid UTF-8: {err}"),
                );
            }
        }
    } else if let Ok(val) = serde_json::from_slice::<Value>(&body) {
        if let Some(s) = val
            .get("yaml")
            .or_else(|| val.get("content"))
            .and_then(Value::as_str)
        {
            s.to_string()
        } else if let Some(fp) = val.get("path").and_then(Value::as_str) {
            match std::fs::read_to_string(fp) {
                Ok(s) => s,
                Err(err) => {
                    return orca_err(
                        StatusCode::BAD_REQUEST,
                        "invalid_argument",
                        format!("Failed reading orca.yaml at '{fp}': {err}"),
                    );
                }
            }
        } else if let Some(s) = val.as_str() {
            s.to_string()
        } else {
            return orca_err(
                StatusCode::BAD_REQUEST,
                "invalid_argument",
                "Expected JSON object with 'yaml', 'content', or 'path' string, or raw text/plain YAML.",
            );
        }
    } else {
        match std::str::from_utf8(&body) {
            Ok(s) => s.to_string(),
            Err(err) => {
                return orca_err(
                    StatusCode::BAD_REQUEST,
                    "invalid_argument",
                    format!("orca.yaml body must be valid UTF-8: {err}"),
                );
            }
        }
    };

    match parse_orca_yaml_text(&raw_yaml) {
        Ok(cfg) => Json(orca_yaml_to_wire_json(&cfg)).into_response(),
        Err(err) => map_ide_err(err),
    }
}

// ---------------------------------------------------------------------------
// 2. Worktrees, `.worktreeinclude`, Listening Ports & Guarded Cleanup
// ---------------------------------------------------------------------------

fn worktree_record_to_wire_json(rec: &WorktreeRecord) -> Value {
    let mut val = serde_json::to_value(rec).unwrap_or_else(|_| json!({}));
    if let Some(obj) = val.as_object_mut() {
        obj.insert("worktreeId".into(), json!(rec.id));
        obj.insert("kind".into(), json!("git"));
        obj.insert("isGit".into(), json!(true));
        obj.insert("name".into(), json!(rec.branch_name));
        obj.insert("branch".into(), json!(rec.branch_name));
        obj.insert("branchName".into(), json!(rec.branch_name));
        obj.insert("path".into(), json!(rec.worktree_path));
        obj.insert("worktreePath".into(), json!(rec.worktree_path));
        obj.insert("repoPath".into(), json!(rec.repo_root));
        obj.insert("parentWorktreeId".into(), json!(rec.parent_worktree_id));
        obj.insert("projectGroupId".into(), json!(rec.project_group_id));
        obj.insert("comment".into(), json!(rec.checkpoint_comment));
        obj.insert("checkpointComment".into(), json!(rec.checkpoint_comment));
        obj.insert("workspaceStatus".into(), json!(rec.workspace_status));
        obj.insert("status".into(), json!(rec.workspace_status));
        obj.insert("autoGeneratedName".into(), json!(rec.is_creature_named));
    }
    val
}

fn folder_workspace_to_wire_json(fw: &FolderWorkspace) -> Value {
    let mut val = serde_json::to_value(fw).unwrap_or_else(|_| json!({}));
    if let Some(obj) = val.as_object_mut() {
        obj.insert("worktreeId".into(), json!(fw.id));
        obj.insert("kind".into(), json!("folder"));
        obj.insert("isGit".into(), json!(false));
        obj.insert("path".into(), json!(fw.root_path));
        obj.insert("folderPath".into(), json!(fw.root_path));
        obj.insert("worktreePath".into(), json!(fw.root_path));
        obj.insert("projectGroupId".into(), json!(fw.project_group_id));
        obj.insert("comment".into(), json!(fw.checkpoint_comment));
        obj.insert("workspaceStatus".into(), json!(fw.workspace_status));
    }
    val
}

/// `GET /api/v1/orca/ide/worktrees` (`200 OK`).
pub async fn list_worktrees(
    _auth: AuthBearer,
    State(state): State<Arc<AppState>>,
) -> Response {
    let mgr = state.orca_ide.worktrees();
    let git_list = match mgr.list_worktrees() {
        Ok(list) => list,
        Err(err) => return map_ide_err(err),
    };
    let folder_list = mgr.list_folder_workspaces().unwrap_or_default();
    let mut out: Vec<Value> = git_list.iter().map(worktree_record_to_wire_json).collect();
    out.extend(folder_list.iter().map(folder_workspace_to_wire_json));
    Json(out).into_response()
}

#[derive(Debug, Deserialize)]
pub struct CreateWorktreeBody {
    #[serde(default)]
    pub id: Option<String>,
    #[serde(default)]
    pub kind: Option<String>,
    #[serde(default, alias = "repoRoot", alias = "repoPath")]
    pub repo_root: Option<String>,
    #[serde(default, alias = "folderPath", alias = "folder_path")]
    pub folder_path: Option<String>,
    #[serde(default, alias = "worktreePath", alias = "path")]
    pub worktree_path: Option<String>,
    #[serde(default, alias = "branchName", alias = "name")]
    pub branch_name: Option<String>,
    #[serde(default)]
    pub label: Option<String>,
    #[serde(default, alias = "baseRef")]
    pub base_ref: Option<String>,
    #[serde(default)]
    pub title: Option<String>,
    #[serde(default, alias = "parentWorktreeId")]
    pub parent_worktree_id: Option<String>,
    #[serde(default, alias = "projectGroupId")]
    pub project_group_id: Option<String>,
    #[serde(default = "default_true", alias = "createdByOrca")]
    pub created_by_orca: bool,
    #[serde(default, alias = "autoGeneratedName")]
    pub auto_generated_name: Option<bool>,
    #[serde(default, alias = "sparsePreset")]
    pub sparse_preset: Option<String>,
    #[serde(default, alias = "existingBranches")]
    pub existing_branches: BTreeSet<String>,
    #[serde(default, alias = "seedHint")]
    pub seed_hint: Option<String>,
}

const fn default_true() -> bool {
    true
}

fn materialize_git_worktree(
    repo_root: &Path,
    wt_path: &Path,
    branch_name: &str,
    base_ref: Option<&str>,
    sparse_patterns: Option<(&[String], bool)>,
) -> Result<(), String> {
    if !repo_root.join(".git").exists() || wt_path.exists() {
        return Ok(());
    }
    if let Some(parent) = wt_path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let is_sparse = sparse_patterns.is_some_and(|(p, _)| !p.is_empty());
    let mut cmd = std::process::Command::new("git");
    cmd.arg("-C").arg(repo_root).arg("worktree").arg("add");
    if is_sparse {
        cmd.arg("--no-checkout");
    }
    cmd.arg("-b").arg(branch_name).arg(wt_path);
    if let Some(base) = base_ref.filter(|s| !s.trim().is_empty()) {
        cmd.arg(base);
    }
    let status = cmd.output().map_err(|e| format!("git worktree add failed: {e}"))?;
    if !status.status.success() {
        // Retry attaching existing branch if `-b` collided with an existing branch name.
        let mut retry = std::process::Command::new("git");
        retry.arg("-C").arg(repo_root).arg("worktree").arg("add");
        if is_sparse {
            retry.arg("--no-checkout");
        }
        retry.arg(wt_path).arg(branch_name);
        let retry_out = retry
            .output()
            .map_err(|e| format!("git worktree add retry failed: {e}"))?;
        if !retry_out.status.success() {
            return Err(format!(
                "git worktree add failed: {}",
                String::from_utf8_lossy(&status.stderr).trim()
            ));
        }
    }

    if let Some((patterns, cone_mode)) = sparse_patterns
        && !patterns.is_empty()
    {
        let mut init_cmd = std::process::Command::new("git");
        init_cmd
            .arg("-C")
            .arg(wt_path)
            .arg("sparse-checkout")
            .arg("init");
        if cone_mode {
            init_cmd.arg("--cone");
        } else {
            init_cmd.arg("--no-cone");
        }
        let init_out = init_cmd
            .output()
            .map_err(|e| format!("git sparse-checkout init failed: {e}"))?;
        if !init_out.status.success() {
            return Err(format!(
                "git sparse-checkout init failed: {}",
                String::from_utf8_lossy(&init_out.stderr).trim()
            ));
        }

        let mut set_cmd = std::process::Command::new("git");
        set_cmd
            .arg("-C")
            .arg(wt_path)
            .arg("sparse-checkout")
            .arg("set");
        for p in patterns {
            set_cmd.arg(p);
        }
        let set_out = set_cmd
            .output()
            .map_err(|e| format!("git sparse-checkout set failed: {e}"))?;
        if !set_out.status.success() {
            return Err(format!(
                "git sparse-checkout set failed: {}",
                String::from_utf8_lossy(&set_out.stderr).trim()
            ));
        }

        let co_out = std::process::Command::new("git")
            .arg("-C")
            .arg(wt_path)
            .arg("checkout")
            .output()
            .map_err(|e| format!("git checkout after sparse-checkout failed: {e}"))?;
        if !co_out.status.success() {
            return Err(format!(
                "git checkout failed: {}",
                String::from_utf8_lossy(&co_out.stderr).trim()
            ));
        }
    }
    Ok(())
}

/// `POST /api/v1/orca/ide/worktrees` (`201 Created`).
pub async fn create_worktree(
    _auth: AuthBearer,
    State(state): State<Arc<AppState>>,
    body: Bytes,
) -> Response {
    let req: CreateWorktreeBody = match parse_json_body(&body) {
        Ok(r) => r,
        Err(resp) => return resp,
    };
    let is_folder = req
        .kind
        .as_deref()
        .is_some_and(|k| k.eq_ignore_ascii_case("folder"))
        || (req.folder_path.is_some() && req.repo_root.is_none());

    if is_folder {
        let Some(root_path) = req
            .folder_path
            .or(req.worktree_path)
            .filter(|s| !s.trim().is_empty())
        else {
            return orca_err(
                StatusCode::BAD_REQUEST,
                "invalid_argument",
                "Missing required 'folderPath' for folder workspace.",
            );
        };
        let now_ms = now_epoch_ms();
        let id = req
            .id
            .filter(|s| !s.trim().is_empty())
            .unwrap_or_else(|| format!("fw-{}", uuid::Uuid::new_v4().simple()));
        let name = req
            .branch_name
            .or(req.label)
            .or(req.title)
            .filter(|s| !s.trim().is_empty())
            .unwrap_or_else(|| {
                Path::new(&root_path)
                    .file_name()
                    .and_then(|n| n.to_str())
                    .unwrap_or("folder")
                    .to_string()
            });
        let folder = FolderWorkspace {
            id,
            name,
            root_path,
            project_group_id: req.project_group_id,
            default_agent: None,
            created_at_ms: now_ms,
            updated_at_ms: now_ms,
            pinned: false,
            checkpoint_comment: None,
            workspace_status: Some(WorkspaceStatus::Todo.as_str().to_string()),
        };
        return match state.orca_ide.worktrees().create_folder_workspace(folder) {
            Ok(saved) => (StatusCode::CREATED, Json(folder_workspace_to_wire_json(&saved)))
                .into_response(),
            Err(err) => map_ide_err(err),
        };
    }

    let Some(repo_root_str) = req.repo_root.filter(|s| !s.trim().is_empty()) else {
        return orca_err(
            StatusCode::BAD_REQUEST,
            "invalid_argument",
            "Missing required 'repo_root' / 'repoPath' for git worktree.",
        );
    };
    let repo_root = PathBuf::from(&repo_root_str);
    let mgr = state.orca_ide.worktrees();

    let resolved_branch = match req
        .branch_name
        .or(req.label)
        .filter(|s| !s.trim().is_empty())
    {
        Some(b) => b,
        None => match mgr.generate_creature_branch_name(&req.existing_branches, req.seed_hint.as_deref()) {
            Ok(b) => b,
            Err(err) => return map_ide_err(err),
        },
    };
    let worktree_path = match req.worktree_path.filter(|s| !s.trim().is_empty()) {
        Some(p) => PathBuf::from(p),
        None => {
            let safe_slug: String = resolved_branch
                .chars()
                .map(|c| if c.is_ascii_alphanumeric() || c == '-' { c } else { '_' })
                .collect();
            let short = &uuid::Uuid::new_v4().simple().to_string()[..8];
            state
                .orca_ide
                .config()
                .workspaces_root
                .join(format!("{safe_slug}-{short}"))
        }
    };

    let sparse_lookup = if let Some(ref preset_key) = req.sparse_preset {
        match mgr.list_sparse_presets() {
            Ok(presets) => presets
                .into_iter()
                .find(|p| p.name == *preset_key || p.id == *preset_key)
                .map(|p| (p.patterns, p.cone_mode)),
            Err(_) => None,
        }
    } else {
        None
    };

    if let Err(err_msg) = materialize_git_worktree(
        &repo_root,
        &worktree_path,
        &resolved_branch,
        req.base_ref.as_deref(),
        sparse_lookup
            .as_ref()
            .map(|(pats, cone)| (pats.as_slice(), *cone)),
    ) {
        return orca_err(StatusCode::INTERNAL_SERVER_ERROR, "git_worktree_error", err_msg);
    }

    let create_req = CreateWorktreeRequest {
        id: req.id,
        repo_root,
        worktree_path,
        branch_name: Some(resolved_branch),
        title: req.title,
        parent_worktree_id: req.parent_worktree_id,
        project_group_id: req.project_group_id,
        created_by_orca: req.created_by_orca,
        existing_branches: req.existing_branches,
        seed_hint: req.seed_hint,
    };
    match mgr.create_worktree_with_auto_flag(create_req, req.auto_generated_name) {
        Ok(rec) => (StatusCode::CREATED, Json(worktree_record_to_wire_json(&rec))).into_response(),
        Err(err) => map_ide_err(err),
    }
}

#[derive(Debug, Default, Deserialize)]
pub struct PatchWorktreeBody {
    #[serde(default, alias = "checkpointComment", alias = "comment")]
    pub checkpoint_comment: Option<String>,
    #[serde(default, alias = "workspaceStatus", alias = "status")]
    pub workspace_status: Option<String>,
    #[serde(default, alias = "nowMs")]
    pub now_ms: Option<i64>,
}

/// `PATCH /api/v1/orca/ide/worktrees/{id}` (`200 OK`).
pub async fn patch_worktree(
    _auth: AuthBearer,
    State(state): State<Arc<AppState>>,
    AxumPath(id): AxumPath<String>,
    body: Bytes,
) -> Response {
    let req: PatchWorktreeBody = match parse_json_body(&body) {
        Ok(r) => r,
        Err(resp) => return resp,
    };
    let status = match req.workspace_status.as_deref() {
        Some(raw) => match WorkspaceStatus::parse(raw) {
            Ok(st) => Some(st),
            Err(err) => return map_ide_err(err),
        },
        None => None,
    };
    let now_ms = req.now_ms.unwrap_or_else(now_epoch_ms);
    match state
        .orca_ide
        .worktrees()
        .set_worktree_checkpoint(&id, req.checkpoint_comment, status, now_ms)
    {
        Ok(updated) => Json(worktree_record_to_wire_json(&updated)).into_response(),
        Err(err) => map_ide_err(err),
    }
}

#[derive(Debug, Deserialize)]
pub struct IncludeCopyBody {
    #[serde(alias = "sourceRepoRoot", alias = "sourcePath", alias = "source")]
    pub source_repo_root: String,
    #[serde(alias = "targetWorktreeRoot", alias = "destPath", alias = "dest")]
    pub target_worktree_root: String,
    #[serde(default)]
    pub rules: Option<String>,
    #[serde(default)]
    pub options: WorktreeIncludeCopyOptions,
}

/// `POST /api/v1/orca/ide/worktrees/include-copy` (`200 OK`, `413 copy_budget_exceeded`).
pub async fn worktree_include_copy(
    _auth: AuthBearer,
    body: Bytes,
) -> Response {
    let req: IncludeCopyBody = match parse_json_body(&body) {
        Ok(r) => r,
        Err(resp) => return resp,
    };
    let src = PathBuf::from(&req.source_repo_root);
    let dst = PathBuf::from(&req.target_worktree_root);
    let rules = match req.rules {
        Some(r) if !r.is_empty() => r,
        _ => {
            let default_file = src.join(".worktreeinclude");
            if default_file.is_file() {
                std::fs::read_to_string(&default_file).unwrap_or_default()
            } else {
                String::new()
            }
        }
    };
    match execute_worktree_include_copy(&src, &dst, &rules, &req.options) {
        Ok(report) => {
            let mut val = serde_json::to_value(&report).unwrap_or_else(|_| json!({}));
            if let Some(obj) = val.as_object_mut() {
                obj.insert("copiedCount".into(), json!(report.copied_files.len()));
                obj.insert("copiedFiles".into(), json!(report.copied_files));
                obj.insert("skippedSymlinks".into(), json!(report.skipped_symlinks));
                obj.insert("warnings".into(), json!(report.skipped_symlinks));
            }
            Json(val).into_response()
        }
        Err(err) => map_ide_err(err),
    }
}

#[derive(Debug, Default, Deserialize)]
pub struct WorktreePortsQuery {
    #[serde(default, alias = "worktreeId", alias = "worktree")]
    pub worktree_id: Option<String>,
}

/// `GET /api/v1/orca/ide/worktrees/ports` (`200 OK`).
pub async fn worktree_ports(
    _auth: AuthBearer,
    State(state): State<Arc<AppState>>,
    Query(query): Query<WorktreePortsQuery>,
) -> Response {
    let worktrees = match state.orca_ide.worktrees().list_worktrees() {
        Ok(list) => list
            .into_iter()
            .map(|w| (w.branch_name, PathBuf::from(w.worktree_path)))
            .collect::<Vec<_>>(),
        Err(err) => return map_ide_err(err),
    };
    let attributed = state
        .orca_ide
        .port_scanner()
        .attribute_ports_to_worktrees(&worktrees);
    let proxy_port = state.orca_ide.label_proxy_port();
    let mut registered = state.orca_ide.registered_worktree_ports();
    if let Some(ref wid) = query.worktree_id
        && !wid.trim().is_empty()
    {
        registered.retain(|p| p.worktree_id == wid.trim());
    }
    let ports: Vec<Value> = registered
        .iter()
        .map(|p| {
            json!({
                "worktreeId": p.worktree_id,
                "label": p.label,
                "targetPort": p.target_port,
                "port": p.target_port,
                "hostname": p.hostname,
                "host": p.hostname,
                "proxyPort": proxy_port,
            })
        })
        .collect();
    Json(json!({
        "proxyPort": proxy_port,
        "ports": ports,
        "attributed": attributed,
    }))
    .into_response()
}

#[derive(Debug, Default, Deserialize)]
pub struct CleanupWorktreeBody {
    #[serde(default, alias = "worktreeId", alias = "id")]
    pub worktree_id: Option<String>,
    #[serde(default)]
    pub preview: bool,
    #[serde(default, alias = "repoPath", alias = "repo")]
    pub repo_root: Option<String>,
    #[serde(default, alias = "unsavedEditorTabs")]
    pub unsaved_editor_tabs: Vec<String>,
    #[serde(default, alias = "forceDiscardUnsaved")]
    pub force_discard_unsaved: bool,
    #[serde(default, alias = "removeDirectoryOnDisk")]
    pub remove_directory_on_disk: bool,
    #[serde(default, alias = "nowMs")]
    pub now_ms: Option<i64>,
}

/// `POST /api/v1/orca/ide/worktrees/cleanup` (`200 OK`).
pub async fn cleanup_worktree(
    _auth: AuthBearer,
    State(state): State<Arc<AppState>>,
    body: Bytes,
) -> Response {
    let req: CleanupWorktreeBody = match parse_json_body(&body) {
        Ok(r) => r,
        Err(resp) => return resp,
    };
    if req.preview || req.worktree_id.as_deref().is_none_or(|s| s.trim().is_empty()) {
        let mgr = state.orca_ide.worktrees();
        let all = match mgr.list_worktrees() {
            Ok(list) => list,
            Err(err) => return map_ide_err(err),
        };
        let repo_filter = req.repo_root.as_deref().map(str::trim).filter(|s| !s.is_empty());
        let mut candidates = Vec::new();
        let mut total_bytes: u64 = 0;
        for wt in all {
            if let Some(rf) = repo_filter
                && wt.repo_root != rf
            {
                continue;
            }
            let p = PathBuf::from(&wt.worktree_path);
            let disk_bytes = if p.exists() {
                mgr.analyze_disk_usage(&p)
                    .map(|rep| rep.total_bytes)
                    .unwrap_or(0)
            } else {
                0
            };
            total_bytes = total_bytes.saturating_add(disk_bytes);
            candidates.push(json!({
                "id": wt.id,
                "worktreeId": wt.id,
                "name": wt.branch_name,
                "path": wt.worktree_path,
                "reclaimableBytes": disk_bytes,
                "diskBytes": disk_bytes,
            }));
        }
        return Json(json!({
            "preview": true,
            "candidates": candidates,
            "totalReclaimableBytes": total_bytes,
            "reclaimableBytes": total_bytes,
        }))
        .into_response();
    }

    let cleanup = CleanupWorktreeRequest {
        worktree_id: req.worktree_id.unwrap_or_default(),
        unsaved_editor_tabs: req.unsaved_editor_tabs,
        force_discard_unsaved: req.force_discard_unsaved,
        remove_directory_on_disk: req.remove_directory_on_disk,
        now_ms: req.now_ms.unwrap_or_else(now_epoch_ms),
    };
    match state.orca_ide.worktrees().cleanup_worktree(&cleanup) {
        Ok(outcome) => Json(outcome).into_response(),
        Err(err) => map_ide_err(err),
    }
}

// ---------------------------------------------------------------------------
// 3. Diff Annotations & Single-Batch Agent Prompt
// ---------------------------------------------------------------------------

#[derive(Debug, Default, Deserialize)]
pub struct DiffAnnotationsQuery {
    #[serde(default, alias = "repoPath")]
    pub repo_path: Option<String>,
    #[serde(default, alias = "file", alias = "filePath")]
    pub file_path: Option<String>,
    #[serde(default)]
    pub include_resolved: Option<bool>,
    #[serde(default)]
    pub batch: bool,
}

/// `GET /api/v1/orca/ide/diff-annotations` (`200 OK`).
/// Supports `?repo_path=&file_path=&include_resolved=&batch=true`.
pub async fn list_diff_annotations(
    _auth: AuthBearer,
    State(state): State<Arc<AppState>>,
    Query(query): Query<DiffAnnotationsQuery>,
) -> Response {
    let rp = query
        .repo_path
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty());
    let fp = query
        .file_path
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty());
    let diff = state.orca_ide.diff_review();
    if query.batch {
        let prompt = diff.build_agent_batch_prompt_scoped(rp, fp);
        return Json(json!({ "prompt": prompt })).into_response();
    }
    let include_resolved = query.include_resolved.unwrap_or(true);
    let items: Vec<Value> = diff
        .list_annotations_scoped(rp, fp, include_resolved)
        .into_iter()
        .map(|(a, r)| {
            orca_ide::diff_review::diff_annotation_to_wire_json_with_orphan(
                &a,
                Some(&r),
                diff.is_orphaned(&a.annotation_id),
            )
        })
        .collect();
    Json(items).into_response()
}

/// `GET /api/v1/orca/ide/diff-annotations/batch` (`200 OK`, or `400 invalid_argument` if empty).
pub async fn diff_annotations_batch(
    _auth: AuthBearer,
    State(state): State<Arc<AppState>>,
    Query(query): Query<DiffAnnotationsQuery>,
) -> Response {
    let rp = query
        .repo_path
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty());
    let fp = query
        .file_path
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty());
    let diff = state.orca_ide.diff_review();
    let unresolved = diff.list_annotations_scoped(rp, fp, false);
    if unresolved.is_empty() {
        return orca_err(
            StatusCode::BAD_REQUEST,
            "invalid_argument",
            "No unresolved diff annotations to batch-send",
        );
    }
    let prompt = diff.build_agent_batch_prompt_scoped(rp, fp);
    Json(json!({ "prompt": prompt, "count": unresolved.len() })).into_response()
}

#[derive(Debug, Deserialize)]
pub struct CreateDiffAnnotationBody {
    #[serde(default, alias = "annotationId", alias = "id")]
    pub annotation_id: Option<String>,
    #[serde(default, alias = "repoPath")]
    pub repo_path: Option<String>,
    #[serde(alias = "filePath", alias = "file")]
    pub file_path: String,
    #[serde(default)]
    pub side: Option<DiffSide>,
    #[serde(default, alias = "lineStart", alias = "line")]
    pub line_start: Option<u32>,
    #[serde(default, alias = "lineEnd")]
    pub line_end: Option<u32>,
    pub body: String,
    #[serde(default)]
    pub author: Option<LineAttribution>,
    #[serde(default)]
    pub resolved: bool,
    #[serde(default, alias = "createdAtMs")]
    pub created_at_ms: Option<i64>,
    #[serde(default, alias = "updatedAtMs")]
    pub updated_at_ms: Option<i64>,
    #[serde(default, alias = "nowMs")]
    pub now_ms: Option<i64>,
}

/// `POST /api/v1/orca/ide/diff-annotations` (`201 Created`).
pub async fn create_diff_annotation(
    _auth: AuthBearer,
    State(state): State<Arc<AppState>>,
    body: Bytes,
) -> Response {
    let req: CreateDiffAnnotationBody = match parse_json_body(&body) {
        Ok(r) => r,
        Err(resp) => return resp,
    };
    let now_ms = req.now_ms.unwrap_or_else(now_epoch_ms);
    let line_start = req.line_start.unwrap_or(1);
    let line_end = req.line_end.unwrap_or(line_start);
    let repo_path = req
        .repo_path
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(ToOwned::to_owned);
    let ann = DiffAnnotation {
        annotation_id: req
            .annotation_id
            .filter(|s| !s.trim().is_empty())
            .unwrap_or_else(|| format!("ann-{}", uuid::Uuid::new_v4())),
        file_path: req.file_path,
        side: req.side.unwrap_or(DiffSide::Right),
        line_start,
        line_end,
        body: req.body,
        author: req.author.unwrap_or(LineAttribution::Human),
        resolved: req.resolved,
        created_at_ms: req.created_at_ms.unwrap_or(now_ms),
        updated_at_ms: req.updated_at_ms.unwrap_or(now_ms),
    };
    match state
        .orca_ide
        .diff_review()
        .add_annotation_with_context(ann, repo_path.as_deref(), None)
    {
        Ok(saved) => (
            StatusCode::CREATED,
            Json(diff_annotation_to_wire_json(&saved, repo_path.as_deref())),
        )
            .into_response(),
        Err(err) => map_ide_err(err),
    }
}

// ---------------------------------------------------------------------------
// 4. AI Vault Cross-Agent Search & Incremental Indexing
// ---------------------------------------------------------------------------

#[derive(Debug, Default, Deserialize)]
pub struct VaultSearchQuery {
    #[serde(default)]
    pub q: Option<String>,
    #[serde(default)]
    pub query: Option<String>,
    #[serde(default, alias = "workspace", alias = "workspacePath")]
    pub workspace_id: Option<String>,
    #[serde(default, alias = "agent", alias = "agentId")]
    pub agent_id: Option<String>,
    #[serde(default)]
    pub limit: Option<usize>,
}

fn run_vault_index(state: &AppState, val: &Value) -> Response {
    let Some(session_id) = val
        .get("session_id")
        .or_else(|| val.get("sessionId"))
        .and_then(Value::as_str)
        .filter(|s| !s.trim().is_empty())
    else {
        return orca_err(
            StatusCode::BAD_REQUEST,
            "invalid_argument",
            "Missing required string 'session_id'.",
        );
    };
    let agent_id = val
        .get("agent_id")
        .or_else(|| val.get("agent"))
        .or_else(|| val.get("agentId"))
        .and_then(Value::as_str)
        .filter(|s| !s.trim().is_empty())
        .unwrap_or("claude");
    let workspace_id = val
        .get("workspace_id")
        .or_else(|| val.get("workspace_path"))
        .or_else(|| val.get("workspacePath"))
        .or_else(|| val.get("workspaceId"))
        .and_then(Value::as_str)
        .filter(|s| !s.trim().is_empty())
        .unwrap_or("default");
    let now_ms = val
        .get("now_ms")
        .or_else(|| val.get("nowMs"))
        .and_then(Value::as_i64)
        .unwrap_or_else(now_epoch_ms);

    let vault = state.orca_ide.vault_search();
    if let Some(fp) = val
        .get("file_path")
        .or_else(|| val.get("transcript_path"))
        .or_else(|| val.get("transcriptPath"))
        .or_else(|| val.get("filePath"))
        .and_then(Value::as_str)
        .filter(|s| !s.trim().is_empty())
    {
        return match vault.index_transcript_file(
            Path::new(fp),
            session_id,
            agent_id,
            workspace_id,
            now_ms,
        ) {
            Ok(rep) => Json(rep).into_response(),
            Err(err) => map_ide_err(err),
        };
    }

    let messages: Vec<VaultMessageInput> = match val.get("messages") {
        Some(m) => match serde_json::from_value(m.clone()) {
            Ok(parsed) => parsed,
            Err(err) => {
                return orca_err(
                    StatusCode::BAD_REQUEST,
                    "invalid_argument",
                    format!("Malformed 'messages' array: {err}"),
                );
            }
        },
        None => Vec::new(),
    };
    match vault.index_messages(session_id, agent_id, workspace_id, &messages, now_ms) {
        Ok((generation, incarnation, messages_added)) => Json(json!({
            "session_id": session_id,
            "messages_added": messages_added,
            "generation": generation,
            "incarnation": incarnation,
        }))
        .into_response(),
        Err(err) => map_ide_err(err),
    }
}

/// `GET /api/v1/orca/ide/vault/search` (`200 OK`).
pub async fn vault_search_get(
    _auth: AuthBearer,
    State(state): State<Arc<AppState>>,
    Query(query): Query<VaultSearchQuery>,
) -> Response {
    let q = query
        .q
        .or(query.query)
        .unwrap_or_default();
    let req = VaultSearchRequest {
        query: q,
        workspace_id: query.workspace_id.filter(|s| !s.trim().is_empty()),
        agent_id: query.agent_id.filter(|s| !s.trim().is_empty()),
        limit: query.limit.unwrap_or(20),
        cursor: None,
    };
    match state.orca_ide.vault_search().search(&req) {
        Ok(resp) => Json(resp).into_response(),
        Err(err) => map_ide_err(err),
    }
}

/// `POST /api/v1/orca/ide/vault/search` (`200 OK`).
/// Supports `VaultSearchRequest` AND `{action: "index", ...}`.
pub async fn vault_search_post(
    _auth: AuthBearer,
    State(state): State<Arc<AppState>>,
    body: Bytes,
) -> Response {
    let mut val: Value = match parse_json_body(&body) {
        Ok(v) => v,
        Err(resp) => return resp,
    };
    if val
        .get("action")
        .and_then(Value::as_str)
        .is_some_and(|a| a.eq_ignore_ascii_case("index"))
    {
        return run_vault_index(&state, &val);
    }
    if val.get("query").is_none()
        && let Some(q) = val.get("q").cloned()
        && let Some(obj) = val.as_object_mut()
    {
        obj.insert("query".to_string(), q);
    }
    let req: VaultSearchRequest = match serde_json::from_value(val) {
        Ok(r) => r,
        Err(err) => {
            return orca_err(
                StatusCode::BAD_REQUEST,
                "invalid_argument",
                format!("Invalid VaultSearchRequest: {err}"),
            );
        }
    };
    match state.orca_ide.vault_search().search(&req) {
        Ok(resp) => Json(resp).into_response(),
        Err(err) => map_ide_err(err),
    }
}

/// `POST /api/v1/orca/ide/vault/index` (`200 OK`).
pub async fn vault_index_post(
    _auth: AuthBearer,
    State(state): State<Arc<AppState>>,
    body: Bytes,
) -> Response {
    let val: Value = match parse_json_body(&body) {
        Ok(v) => v,
        Err(resp) => return resp,
    };
    run_vault_index(&state, &val)
}

// ---------------------------------------------------------------------------
// 5. Plugin System V2 (`validate | install | approve | evaluate`)
// ---------------------------------------------------------------------------

/// `GET /api/v1/orca/ide/plugins` (`200 OK`).
pub async fn list_plugins(
    _auth: AuthBearer,
    State(state): State<Arc<AppState>>,
) -> Response {
    let items: Vec<Value> = state
        .orca_ide
        .plugin_host()
        .list_plugins()
        .iter()
        .map(installed_plugin_to_wire_json)
        .collect();
    Json(items).into_response()
}

fn parse_manifest_and_contributions(
    val: &Value,
) -> Result<(PluginManifestV2, Vec<PluginContributionPoint>, Value, Option<PathBuf>), Response> {
    if let Some(p) = val
        .get("path")
        .and_then(Value::as_str)
        .filter(|s| !s.trim().is_empty())
    {
        return PluginHostV2::load_manifest_from_path(Path::new(p)).map_err(map_ide_err);
    }
    let mut manifest_val = val.get("manifest").cloned().unwrap_or_else(|| val.clone());
    let pkg_dir = val
        .get("package_dir")
        .or_else(|| val.get("packageDir"))
        .and_then(Value::as_str)
        .filter(|s| !s.trim().is_empty())
        .map(PathBuf::from);
    let fallback_sha = if let Some(ref dir) = pkg_dir {
        match scan_plugin_package_dir(dir) {
            Ok(rep) => Some(rep.package_sha256),
            Err(err) => return Err(map_ide_err(err)),
        }
    } else {
        None
    };
    let (manifest, mut contributions, contributes_raw) =
        PluginHostV2::parse_manifest_value(&mut manifest_val, fallback_sha.as_deref())
            .map_err(map_ide_err)?;
    if let Some(c) = val.get("contributions") {
        let extra: Vec<PluginContributionPoint> =
            serde_json::from_value(c.clone()).map_err(|err| {
                orca_err(
                    StatusCode::BAD_REQUEST,
                    "invalid_argument",
                    format!("Invalid plugin contributions: {err}"),
                )
            })?;
        for pt in extra {
            if !contributions.contains(&pt) {
                contributions.push(pt);
            }
        }
    }
    Ok((manifest, contributions, contributes_raw, pkg_dir))
}

/// `POST /api/v1/orca/ide/plugins` (`200`/`201`, `403 approval_required`).
/// Supports `action: "validate" | "install" | "approve" | "evaluate"` (default `"install"`).
pub async fn manage_plugins(
    _auth: AuthBearer,
    State(state): State<Arc<AppState>>,
    body: Bytes,
) -> Response {
    let val: Value = match parse_json_body(&body) {
        Ok(v) => v,
        Err(resp) => return resp,
    };
    let action = val
        .get("action")
        .and_then(Value::as_str)
        .unwrap_or("install")
        .trim()
        .to_ascii_lowercase();
    let now_ms = val
        .get("now_ms")
        .or_else(|| val.get("nowMs"))
        .and_then(Value::as_i64)
        .unwrap_or_else(now_epoch_ms);
    let host = state.orca_ide.plugin_host();

    match action.as_str() {
        "validate" => {
            let (manifest, contributions, _, _) = match parse_manifest_and_contributions(&val) {
                Ok(parsed) => parsed,
                Err(resp) => return resp,
            };
            match host.validate_manifest(&manifest, &contributions) {
                Ok(summary) => Json(plugin_validation_to_wire_json(
                    &summary,
                    manifest.publisher.as_deref(),
                ))
                .into_response(),
                Err(err) => map_ide_err(err),
            }
        }
        "approve" => {
            let Some(plugin_id) = val
                .get("plugin_id")
                .or_else(|| val.get("pluginId"))
                .or_else(|| val.get("id"))
                .and_then(Value::as_str)
                .filter(|s| !s.trim().is_empty())
            else {
                return orca_err(
                    StatusCode::BAD_REQUEST,
                    "invalid_argument",
                    "Missing required string 'plugin_id'.",
                );
            };
            let Some(fp) = val
                .get("fingerprint")
                .and_then(Value::as_str)
                .filter(|s| !s.trim().is_empty())
            else {
                return orca_err(
                    StatusCode::BAD_REQUEST,
                    "invalid_argument",
                    "Missing required string 'fingerprint'.",
                );
            };
            match host.approve_plugin(plugin_id, fp, now_ms) {
                Ok(decision) => Json(decision).into_response(),
                Err(err) => map_ide_err(err),
            }
        }
        "evaluate" => {
            let Some(plugin_id) = val
                .get("plugin_id")
                .or_else(|| val.get("pluginId"))
                .or_else(|| val.get("id"))
                .and_then(Value::as_str)
                .filter(|s| !s.trim().is_empty())
            else {
                return orca_err(
                    StatusCode::BAD_REQUEST,
                    "invalid_argument",
                    "Missing required string 'plugin_id'.",
                );
            };
            match host.evaluate_plugin(plugin_id) {
                Ok(decision) => Json(decision).into_response(),
                Err(err) => map_ide_err(err),
            }
        }
        "install" => {
            let (manifest, contributions, contributes_raw, pkg_dir) =
                match parse_manifest_and_contributions(&val) {
                    Ok(parsed) => parsed,
                    Err(resp) => return resp,
                };
            let approve_fp = val
                .get("approve_fingerprint")
                .or_else(|| val.get("approveFingerprint"))
                .and_then(Value::as_str)
                .filter(|s| !s.trim().is_empty())
                .or_else(|| {
                    val.get("approve")
                        .and_then(Value::as_str)
                        .filter(|s| !s.trim().is_empty())
                });
            let auto_approve = val
                .get("approve")
                .and_then(Value::as_bool)
                .unwrap_or(false);
            let enforce_approval = val
                .get("enforce_approval")
                .or_else(|| val.get("enforceApproval"))
                .and_then(Value::as_bool)
                .unwrap_or(true);
            match host.install_plugin_checked(
                manifest,
                contributions,
                contributes_raw,
                pkg_dir.as_deref(),
                approve_fp,
                auto_approve,
                enforce_approval,
                now_ms,
            ) {
                Ok(rec) => (StatusCode::CREATED, Json(installed_plugin_to_wire_json(&rec)))
                    .into_response(),
                Err(err) => map_ide_err(err),
            }
        }
        other => orca_err(
            StatusCode::BAD_REQUEST,
            "invalid_argument",
            format!("Unsupported plugin action '{other}' (expected validate|install|approve|evaluate)."),
        ),
    }
}

// ---------------------------------------------------------------------------
// 6. Shareable Artifacts (ZERO `edit_token` Leak) & Computer Use Guard
// ---------------------------------------------------------------------------

#[derive(Debug, Default, Deserialize)]
pub struct ListArtifactsQuery {
    #[serde(default)]
    pub profile_id: Option<String>,
    #[serde(default)]
    pub artifact_id: Option<String>,
}

fn artifact_to_wire_json(view: &orca_ide::system_services::ArtifactPublicView) -> Value {
    let mut val = serde_json::to_value(view).unwrap_or_else(|_| json!({}));
    if let Some(obj) = val.as_object_mut() {
        obj.insert("id".into(), json!(view.artifact_id));
        obj.insert("filename".into(), json!(view.title));
        obj.insert("url".into(), json!(view.share_url));
    }
    val
}

/// `GET /api/v1/orca/ide/artifacts` (`200 OK`) — returns strictly `ArtifactPublicView`
/// (NEVER contains `edit_token`).
pub async fn list_artifacts(
    _auth: AuthBearer,
    State(state): State<Arc<AppState>>,
    Query(query): Query<ListArtifactsQuery>,
) -> Response {
    let store = state.orca_ide.artifacts();
    if let Some(id) = query
        .artifact_id
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
    {
        return match store.get_artifact(id) {
            Ok(Some(view)) => Json(artifact_to_wire_json(&view)).into_response(),
            Ok(None) => Json(Value::Null).into_response(),
            Err(err) => map_ide_err(err),
        };
    }
    let profile_id = query
        .profile_id
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty());
    let wire_list = match store.list_artifacts(profile_id) {
        Ok(views) => views.iter().map(artifact_to_wire_json).collect::<Vec<_>>(),
        Err(err) => return map_ide_err(err),
    };
    Json(wire_list).into_response()
}

#[derive(Debug, Deserialize)]
pub struct PublishArtifactBody {
    #[serde(default, alias = "artifactId")]
    pub artifact_id: Option<String>,
    #[serde(alias = "filename")]
    pub title: String,
    #[serde(default, alias = "content_type", alias = "contentType")]
    pub kind: Option<String>,
    #[serde(default)]
    pub content: String,
    #[serde(default, alias = "profileId")]
    pub profile_id: Option<String>,
    #[serde(
        default,
        alias = "userConfirmedShare",
        alias = "confirmed",
        alias = "explicit_user_intent",
        alias = "explicitUserIntent"
    )]
    pub user_confirmed_share: bool,
    #[serde(default, alias = "baseUrl")]
    pub base_url: Option<String>,
    #[serde(default, alias = "nowMs")]
    pub now_ms: Option<i64>,
}

/// `POST /api/v1/orca/ide/artifacts` (`201 Created`, `403 approval_required` if unconfirmed).
/// Returns strictly `ArtifactPublicView` — sealed `edit_token` is NEVER exposed!
pub async fn publish_artifact(
    auth: AuthBearer,
    State(state): State<Arc<AppState>>,
    body: Bytes,
) -> Response {
    let req: PublishArtifactBody = match parse_json_body(&body) {
        Ok(r) => r,
        Err(resp) => return resp,
    };
    let kind = match req
        .kind
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
    {
        Some(raw) => match ArtifactKind::parse(raw) {
            Ok(k) => k,
            Err(err) => return map_ide_err(err),
        },
        None => {
            if req.title.to_ascii_lowercase().ends_with(".html") {
                ArtifactKind::Html
            } else {
                ArtifactKind::Markdown
            }
        }
    };
    let caller_profile = format!("user:{}", auth.user_id.0);
    if let Some(ref requested_profile) = req.profile_id {
        let trimmed = requested_profile.trim();
        if !trimmed.is_empty() && trimmed != caller_profile {
            return orca_err(
                StatusCode::FORBIDDEN,
                "permission_denied",
                format!(
                    "Authenticated caller '{caller_profile}' cannot publish artifact for profile '{trimmed}'."
                ),
            );
        }
    }
    let publish = PublishArtifactRequest {
        artifact_id: req.artifact_id,
        title: req.title,
        kind,
        content: req.content,
        profile_id: caller_profile,
        user_confirmed_share: req.user_confirmed_share,
        base_url: req.base_url,
        now_ms: req.now_ms.unwrap_or_else(now_epoch_ms),
    };
    match state.orca_ide.artifacts().publish_artifact(publish) {
        Ok(view) => (StatusCode::CREATED, Json(artifact_to_wire_json(&view))).into_response(),
        Err(err) => map_ide_err(err),
    }
}

/// `GET /api/v1/orca/ide/artifacts/{artifact_id}` (`200 OK`, `404 not_found`).
/// NEVER exposes sealed `edit_token`.
pub async fn get_artifact_by_id(
    _auth: AuthBearer,
    State(state): State<Arc<AppState>>,
    AxumPath(artifact_id): AxumPath<String>,
) -> Response {
    match state.orca_ide.artifacts().get_artifact(&artifact_id) {
        Ok(Some(view)) => Json(artifact_to_wire_json(&view)).into_response(),
        Ok(None) => orca_err(
            StatusCode::NOT_FOUND,
            "not_found",
            format!("Artifact '{artifact_id}' not found."),
        ),
        Err(err) => map_ide_err(err),
    }
}

#[derive(Debug, Default, Deserialize)]
pub struct UpdateArtifactBody {
    #[serde(default, alias = "filename")]
    pub title: Option<String>,
    #[serde(default)]
    pub content: Option<String>,
    #[serde(default, alias = "profileId")]
    pub profile_id: Option<String>,
    #[serde(default, alias = "nowMs")]
    pub now_ms: Option<i64>,
}

/// `PATCH /api/v1/orca/ide/artifacts/{artifact_id}` (`200 OK`, `403 permission_denied`, `404 not_found`).
pub async fn update_artifact_by_id(
    auth: AuthBearer,
    State(state): State<Arc<AppState>>,
    AxumPath(artifact_id): AxumPath<String>,
    body: Bytes,
) -> Response {
    let req: UpdateArtifactBody = match parse_json_body(&body) {
        Ok(r) => r,
        Err(resp) => return resp,
    };
    let store = state.orca_ide.artifacts();
    let existing = match store.get_artifact(&artifact_id) {
        Ok(Some(a)) => a,
        Ok(None) => {
            return orca_err(
                StatusCode::NOT_FOUND,
                "not_found",
                format!("Artifact '{artifact_id}' not found."),
            );
        }
        Err(err) => return map_ide_err(err),
    };
    let caller_profile = format!("user:{}", auth.user_id.0);
    if let Some(ref requested_profile) = req.profile_id {
        let trimmed = requested_profile.trim();
        if !trimmed.is_empty() && trimmed != caller_profile {
            return orca_err(
                StatusCode::FORBIDDEN,
                "permission_denied",
                format!("Authenticated caller '{caller_profile}' cannot impersonate profile '{trimmed}'."),
            );
        }
    }
    if existing.profile_id != caller_profile {
        return orca_err(
            StatusCode::FORBIDDEN,
            "permission_denied",
            format!("Caller does not own artifact '{artifact_id}'."),
        );
    }
    let content = req.content.unwrap_or(existing.content);
    let now_ms = req.now_ms.unwrap_or_else(now_epoch_ms);
    match store.update_artifact(
        &artifact_id,
        &caller_profile,
        req.title.as_deref(),
        &content,
        now_ms,
    ) {
        Ok(view) => Json(artifact_to_wire_json(&view)).into_response(),
        Err(err) => map_ide_err(err),
    }
}

/// `DELETE /api/v1/orca/ide/artifacts/{artifact_id}` (`200 OK`, `403 permission_denied`, `404 not_found`).
pub async fn delete_artifact_by_id(
    auth: AuthBearer,
    State(state): State<Arc<AppState>>,
    AxumPath(artifact_id): AxumPath<String>,
) -> Response {
    let store = state.orca_ide.artifacts();
    let existing = match store.get_artifact(&artifact_id) {
        Ok(Some(a)) => a,
        Ok(None) => {
            return orca_err(
                StatusCode::NOT_FOUND,
                "not_found",
                format!("Artifact '{artifact_id}' not found."),
            );
        }
        Err(err) => return map_ide_err(err),
    };
    let caller_profile = format!("user:{}", auth.user_id.0);
    if existing.profile_id != caller_profile {
        return orca_err(
            StatusCode::FORBIDDEN,
            "permission_denied",
            format!("Caller does not own artifact '{artifact_id}'."),
        );
    }
    match store.delete_artifact(&artifact_id, now_epoch_ms()) {
        Ok(deleted) => Json(json!({ "deleted": deleted, "id": artifact_id, "artifact_id": artifact_id }))
            .into_response(),
        Err(err) => map_ide_err(err),
    }
}

/// `POST /api/v1/orca/ide/computer/guard` (`200 OK`, `403 system_shortcut_blocked` / `emulator_input_blocked`).
pub async fn computer_guard(
    _auth: AuthBearer,
    State(state): State<Arc<AppState>>,
    body: Bytes,
) -> Response {
    let val: Value = match parse_json_body(&body) {
        Ok(v) => v,
        Err(resp) => return resp,
    };
    if let Some(key_raw) = val
        .get("key")
        .and_then(Value::as_str)
        .filter(|s| !s.trim().is_empty())
        && val.get("request").is_none()
        && val.get("action").is_none()
    {
        let Some(chord) = KeyChord::parse(key_raw) else {
            return orca_err(
                StatusCode::BAD_REQUEST,
                "invalid_argument",
                format!("Invalid key chord '{key_raw}'."),
            );
        };
        let blocked_sys = is_system_shortcut_blocked(&chord);
        let allow_sys = val
            .get("allow_system_shortcut")
            .or_else(|| val.get("allowSystemShortcut"))
            .and_then(Value::as_bool)
            .unwrap_or(false);
        let target_id = val
            .get("target_id")
            .or_else(|| val.get("app"))
            .and_then(Value::as_str)
            .filter(|s| !s.trim().is_empty())
            .unwrap_or("default-window")
            .to_string();
        let target_kind: ComputerUseTargetKind = match val.get("target_kind") {
            Some(tk) if !tk.is_null() => match serde_json::from_value(tk.clone()) {
                Ok(parsed) => parsed,
                Err(err) => {
                    return orca_err(
                        StatusCode::BAD_REQUEST,
                        "invalid_argument",
                        format!("Invalid target_kind: {err}"),
                    );
                }
            },
            _ => ComputerUseTargetKind::NormalWindow,
        };
        let mut policy = state.orca_ide.computer_use().policy();
        policy.allow_system_shortcut = allow_sys;
        policy.approved_targets.insert(target_id.clone());
        let req = ComputerUseRequest {
            target_id,
            target_kind,
            action: ComputerUseAction::Hotkey { chord: chord.clone() },
        };
        return match state
            .orca_ide
            .computer_use()
            .evaluate_guard(&req, Some(&policy))
        {
            Ok(decision) => Json(json!({
                "allowed": true,
                "requires_override": blocked_sys,
                "key": key_raw,
                "canonical_chord": chord.canonical_string(),
                "background_delivery": decision.background_delivery,
                "steals_focus": !decision.background_delivery,
            }))
            .into_response(),
            Err(err) => map_ide_err(err),
        };
    }

    let req_val = val.get("request").cloned().unwrap_or_else(|| val.clone());
    let req: ComputerUseRequest = match serde_json::from_value(req_val) {
        Ok(r) => r,
        Err(err) => {
            return orca_err(
                StatusCode::BAD_REQUEST,
                "invalid_argument",
                format!("Invalid ComputerUseRequest: {err}"),
            );
        }
    };
    let policy_override: Option<ComputerUsePolicy> = match val.get("policy") {
        Some(p) if !p.is_null() => match serde_json::from_value(p.clone()) {
            Ok(pol) => Some(pol),
            Err(err) => {
                return orca_err(
                    StatusCode::BAD_REQUEST,
                    "invalid_argument",
                    format!("Invalid ComputerUsePolicy: {err}"),
                );
            }
        },
        _ => None,
    };
    match state
        .orca_ide
        .computer_use()
        .evaluate_guard(&req, policy_override.as_ref())
    {
        Ok(decision) => Json(json!({
            "allowed": true,
            "requires_override": false,
            "background_delivery": decision.background_delivery,
            "steals_focus": !decision.background_delivery,
        }))
        .into_response(),
        Err(err) => map_ide_err(err),
    }
}

// ---------------------------------------------------------------------------
// 7. 9-Provider Usage Roster & 8-Condition Hibernation Check
// ---------------------------------------------------------------------------

#[derive(Debug, Default, Deserialize)]
pub struct UsageQuery {
    #[serde(default)]
    pub now_ms: Option<i64>,
}

/// `GET /api/v1/orca/ide/usage` (`200 OK` — tightest-first 9-provider roster).
pub async fn usage_roster_get(
    _auth: AuthBearer,
    State(state): State<Arc<AppState>>,
    Query(query): Query<UsageQuery>,
) -> Response {
    let now_ms = query.now_ms.unwrap_or_else(now_epoch_ms);
    let rows = state.orca_ide.usage_roster().sorted_roster_wire_json(now_ms);
    Json(json!({
        "providers": rows,
        "usage": rows,
        "items": rows,
    }))
    .into_response()
}

/// `POST /api/v1/orca/ide/usage` (`200 OK` — ingests a provider quota snapshot or Claude status line).
pub async fn usage_roster_post(
    _auth: AuthBearer,
    State(state): State<Arc<AppState>>,
    body: Bytes,
) -> Response {
    let val: Value = match parse_json_body(&body) {
        Ok(v) => v,
        Err(resp) => return resp,
    };
    if let Some(status_line) = val
        .get("status_line")
        .or_else(|| val.get("statusLine"))
        .and_then(Value::as_str)
    {
        let acct = val
            .get("account_id")
            .or_else(|| val.get("accountId"))
            .and_then(Value::as_str)
            .unwrap_or("default");
        let now_ms = val
            .get("now_ms")
            .or_else(|| val.get("nowMs"))
            .and_then(Value::as_i64)
            .unwrap_or_else(now_epoch_ms);
        return match state
            .orca_ide
            .usage_roster()
            .ingest_claude_status_line(acct, status_line, now_ms)
        {
            Ok(snap) => Json(snap).into_response(),
            Err(err) => map_ide_err(err),
        };
    }
    let snap: ProviderQuotaSnapshot = match serde_json::from_value(val) {
        Ok(s) => s,
        Err(err) => {
            return orca_err(
                StatusCode::BAD_REQUEST,
                "invalid_argument",
                format!("Invalid ProviderQuotaSnapshot: {err}"),
            );
        }
    };
    match state.orca_ide.usage_roster().ingest_snapshot(snap) {
        Ok(saved) => Json(saved).into_response(),
        Err(err) => map_ide_err(err),
    }
}

#[derive(Debug, Default, Deserialize)]
pub struct HibernationCheckParams {
    #[serde(default, alias = "tabId", alias = "sessionId", alias = "session_id")]
    pub tab_id: Option<String>,
    #[serde(default, alias = "dispatchId")]
    pub dispatch_id: Option<String>,
    #[serde(default, alias = "agentStateIsDone")]
    pub agent_state_is_done: Option<bool>,
    #[serde(default)]
    pub state: Option<String>,
    #[serde(default, alias = "tabVisible")]
    pub tab_visible: Option<bool>,
    #[serde(default, alias = "keystrokesSinceDone")]
    pub keystrokes_since_done: Option<u64>,
    #[serde(default, alias = "agentId", alias = "agent")]
    pub agent_id: Option<String>,
    #[serde(default, alias = "idleDurationMs", alias = "idleMs")]
    pub idle_duration_ms: Option<u64>,
    #[serde(default, alias = "windowMs")]
    pub window_ms: Option<u64>,
    #[serde(default, alias = "mobileDriverAttached", alias = "mobileDriverActive")]
    pub mobile_driver_attached: Option<bool>,
    #[serde(default, alias = "unsettledOrchestrationDispatch")]
    pub unsettled_orchestration_dispatch: Option<bool>,
    #[serde(default, alias = "unsettledDispatchCount")]
    pub unsettled_dispatch_count: Option<u64>,
    #[serde(default, alias = "liveSubagentCount")]
    pub live_subagent_count: Option<usize>,
    #[serde(default, alias = "gitOperationInProgress")]
    pub git_operation_in_progress: Option<bool>,
    #[serde(default, alias = "activePortForward")]
    pub active_port_forward: Option<bool>,
    #[serde(default, alias = "pendingBotApproval")]
    pub pending_bot_approval: Option<bool>,
    #[serde(default, alias = "unsyncedLocalSnapshot")]
    pub unsynced_local_snapshot: Option<bool>,
    #[serde(default, alias = "pinnedNoHibernate")]
    pub pinned_no_hibernate: Option<bool>,
    #[serde(default, alias = "sessionHandle")]
    pub session_handle: Option<String>,
    #[serde(default, alias = "originalCwd")]
    pub original_cwd: Option<String>,
    #[serde(default, alias = "originalArgs")]
    pub original_args: Option<Vec<String>>,
    #[serde(default, alias = "originalEnv")]
    pub original_env: Option<BTreeMap<String, String>>,
}

impl HibernationCheckParams {
    fn into_candidate_window_and_dispatch(
        self,
    ) -> (HibernationCandidateState, Option<u64>, Option<String>) {
        let tab_id = self.tab_id.unwrap_or_else(|| "tab-check".to_string());
        let dispatch_id = self
            .dispatch_id
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty());
        let is_done = self.agent_state_is_done.unwrap_or_else(|| {
            self.state
                .as_deref()
                .map(|s| s.eq_ignore_ascii_case("done") || s.eq_ignore_ascii_case("completed"))
                .unwrap_or(true)
        });
        let unsettled = self
            .unsettled_orchestration_dispatch
            .unwrap_or_else(|| self.unsettled_dispatch_count.unwrap_or(0) > 0);
        let session_handle = self
            .session_handle
            .or_else(|| Some(format!("resume-{tab_id}")));
        (
            HibernationCandidateState {
                tab_id,
                agent_state_is_done: is_done,
                tab_visible: self.tab_visible.unwrap_or(false),
                keystrokes_since_done: self.keystrokes_since_done.unwrap_or(0),
                agent_id: self.agent_id.unwrap_or_else(|| "claude".to_string()),
                idle_duration_ms: self.idle_duration_ms.unwrap_or(1_800_000),
                mobile_driver_attached: self.mobile_driver_attached.unwrap_or(false),
                unsettled_orchestration_dispatch: unsettled,
                live_subagent_count: self.live_subagent_count.unwrap_or(0),
                git_operation_in_progress: self.git_operation_in_progress.unwrap_or(false),
                active_port_forward: self.active_port_forward.unwrap_or(false),
                pending_bot_approval: self.pending_bot_approval.unwrap_or(false),
                unsynced_local_snapshot: self.unsynced_local_snapshot.unwrap_or(false),
                pinned_no_hibernate: self.pinned_no_hibernate.unwrap_or(false),
                session_handle,
                original_cwd: PathBuf::from(self.original_cwd.unwrap_or_else(|| ".".to_string())),
                original_args: self.original_args.unwrap_or_default(),
                original_env: self.original_env.unwrap_or_default(),
            },
            self.window_ms,
            dispatch_id,
        )
    }
}

fn hibernation_check_response(
    state: &AppState,
    mut candidate: HibernationCandidateState,
    win_override: Option<u64>,
    dispatch_id: Option<&str>,
) -> Response {
    if let Some(did) = dispatch_id {
        match state.orca_ide.orchestration().is_dispatch_unsettled(did) {
            Ok(true) => candidate.unsettled_orchestration_dispatch = true,
            Ok(false) => {}
            Err(err) => return map_ide_err(err),
        }
    }
    let (eligible, passed, failed, decision) = state
        .orca_ide
        .hibernation()
        .evaluate_8_checks(&candidate, win_override);
    if eligible {
        Json(json!({
            "decision": decision,
            "eligible": true,
            "passedChecks": passed,
            "failedChecks": failed,
        }))
        .into_response()
    } else {
        let message = format!("hibernation blocked by checks: {}", failed.join(", "));
        (
            StatusCode::CONFLICT,
            Json(json!({
                "error": "hibernation_blocked",
                "code": "hibernation_blocked",
                "message": message,
                "decision": "blocked",
                "eligible": false,
                "passedChecks": passed,
                "failedChecks": failed,
            })),
        )
            .into_response()
    }
}

/// `GET /api/v1/orca/ide/hibernation/check` (`200 OK` if eligible, `409 Conflict` with `hibernation_blocked` if blocked).
pub async fn hibernation_check_get(
    _auth: AuthBearer,
    State(state): State<Arc<AppState>>,
    Query(params): Query<HibernationCheckParams>,
) -> Response {
    let (candidate, win_override, dispatch_id) = params.into_candidate_window_and_dispatch();
    hibernation_check_response(&state, candidate, win_override, dispatch_id.as_deref())
}

/// `POST /api/v1/orca/ide/hibernation/check` (`200 OK` if eligible, `409 Conflict` with `hibernation_blocked` if blocked).
pub async fn hibernation_check_post(
    _auth: AuthBearer,
    State(state): State<Arc<AppState>>,
    body: Bytes,
) -> Response {
    let body_dispatch_id = serde_json::from_slice::<Value>(&body).ok().and_then(|v| {
        v.get("dispatchId")
            .or_else(|| v.get("dispatch_id"))
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(ToOwned::to_owned)
    });
    let (candidate, win_override, dispatch_id) =
        match serde_json::from_slice::<HibernationCandidateState>(&body) {
            Ok(c) => (c, None, body_dispatch_id),
            Err(_) => match parse_optional_json_body::<HibernationCheckParams>(&body) {
                Ok(p) => {
                    let (c, w, d) = p.into_candidate_window_and_dispatch();
                    (c, w, d.or(body_dispatch_id))
                }
                Err(resp) => return resp,
            },
        };
    hibernation_check_response(&state, candidate, win_override, dispatch_id.as_deref())
}

// ---------------------------------------------------------------------------
// 8. E2EE v2 Handshake & ProfileState Authority
// ---------------------------------------------------------------------------

/// `POST /api/v1/orca/ide/e2ee/handshake` (`200 OK`).
pub async fn e2ee_handshake(
    _auth: AuthBearer,
    State(state): State<Arc<AppState>>,
    body: Bytes,
) -> Response {
    let req: E2eeHandshakeInitRequest = match parse_json_body(&body) {
        Ok(r) => r,
        Err(resp) => return resp,
    };
    match state.orca_ide.e2ee().server_handshake(&req) {
        Ok(resp) => Json(resp).into_response(),
        Err(err) => map_ide_err(err),
    }
}

#[derive(Debug, Default, Deserialize)]
pub struct ProfileStateQuery {
    #[serde(default)]
    pub domain: Option<String>,
    #[serde(default)]
    pub export: bool,
    #[serde(default, alias = "recoverStatus")]
    pub recover_status: bool,
}

/// `GET /api/v1/orca/ide/profile-state` (`200 OK`).
pub async fn profile_state_get(
    _auth: AuthBearer,
    State(state): State<Arc<AppState>>,
    Query(query): Query<ProfileStateQuery>,
) -> Response {
    let profile = state.orca_ide.profile_state();
    if query.recover_status {
        return Json(profile.inspect_recovery(None, None)).into_response();
    }
    if query.export {
        return match profile.export_all() {
            Ok(bundle) => Json(bundle).into_response(),
            Err(err) => map_ide_err(err),
        };
    }
    if let Some(domain) = query
        .domain
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
    {
        return match (profile.get_domain(domain), profile.list_document_parts(domain)) {
            (Ok(doc), Ok(parts)) if !parts.is_empty() => Json(json!({
                "domain": domain,
                "parts": parts,
                "document": doc,
            }))
            .into_response(),
            (Ok(doc), Ok(_)) => Json(doc).into_response(),
            (Err(err), _) | (_, Err(err)) => map_ide_err(err),
        };
    }
    match profile.list_domains() {
        Ok(docs) => Json(docs).into_response(),
        Err(err) => map_ide_err(err),
    }
}

/// `PUT /api/v1/orca/ide/profile-state` & `POST /api/v1/orca/ide/profile-state`
/// (`200 OK`, `409 conflict` on optimistic revision mismatch).
pub async fn profile_state_put(
    _auth: AuthBearer,
    State(state): State<Arc<AppState>>,
    body: Bytes,
) -> Response {
    let val: Value = match parse_json_body(&body) {
        Ok(v) => v,
        Err(resp) => return resp,
    };
    let now_ms = val
        .get("now_ms")
        .or_else(|| val.get("nowMs"))
        .and_then(Value::as_i64)
        .unwrap_or_else(now_epoch_ms);
    let profile = state.orca_ide.profile_state();

    if let Some(mutations_val) = val.get("mutations") {
        let mutations: Vec<DirtyDomainMutation> = match serde_json::from_value(mutations_val.clone())
        {
            Ok(m) => m,
            Err(err) => {
                return orca_err(
                    StatusCode::BAD_REQUEST,
                    "invalid_argument",
                    format!("Invalid ProfileState mutations array: {err}"),
                );
            }
        };
        return match profile.write_dirty_domains(&mutations, now_ms) {
            Ok(docs) => Json(docs).into_response(),
            Err(err) => map_ide_err(err),
        };
    }

    let Some(domain) = val
        .get("domain")
        .and_then(Value::as_str)
        .filter(|s| !s.trim().is_empty())
        .map(ToOwned::to_owned)
    else {
        return orca_err(
            StatusCode::BAD_REQUEST,
            "invalid_argument",
            "Missing required string 'domain' or 'mutations' array.",
        );
    };
    let payload = val.get("payload").cloned().unwrap_or(Value::Null);
    let expected_revision = val
        .get("expected_revision")
        .or_else(|| val.get("expectedRevision"))
        .and_then(Value::as_u64);
    let domain_version = val
        .get("domain_version")
        .or_else(|| val.get("domainVersion"))
        .and_then(Value::as_u64)
        .unwrap_or(1) as u32;

    match profile.write_dirty_domains(
        &[DirtyDomainMutation {
            domain,
            expected_revision,
            domain_version,
            payload,
        }],
        now_ms,
    ) {
        Ok(mut docs) => Json(docs.pop()).into_response(),
        Err(err) => map_ide_err(err),
    }
}

/// Router exposing all Orca Agent IDE REST routes.
pub fn router() -> Router<Arc<AppState>> {
    Router::new()
        .route("/api/v1/orca/ide/orca-yaml/parse", post(parse_orca_yaml))
        .route(
            "/api/v1/orca/ide/worktrees",
            get(list_worktrees).post(create_worktree),
        )
        .route("/api/v1/orca/ide/worktrees/{id}", patch(patch_worktree))
        .route(
            "/api/v1/orca/ide/worktrees/include-copy",
            post(worktree_include_copy),
        )
        .route("/api/v1/orca/ide/worktrees/ports", get(worktree_ports))
        .route("/api/v1/orca/ide/worktrees/cleanup", post(cleanup_worktree))
        .route(
            "/api/v1/orca/ide/diff-annotations",
            get(list_diff_annotations).post(create_diff_annotation),
        )
        .route(
            "/api/v1/orca/ide/diff-annotations/batch",
            get(diff_annotations_batch),
        )
        .route(
            "/api/v1/orca/ide/vault/search",
            get(vault_search_get).post(vault_search_post),
        )
        .route("/api/v1/orca/ide/vault/index", post(vault_index_post))
        .route(
            "/api/v1/orca/ide/plugins",
            get(list_plugins).post(manage_plugins),
        )
        .route(
            "/api/v1/orca/ide/artifacts",
            get(list_artifacts).post(publish_artifact),
        )
        .route(
            "/api/v1/orca/ide/artifacts/{artifact_id}",
            get(get_artifact_by_id)
                .patch(update_artifact_by_id)
                .delete(delete_artifact_by_id),
        )
        .route("/api/v1/orca/ide/computer/guard", post(computer_guard))
        .route(
            "/api/v1/orca/ide/usage",
            get(usage_roster_get).post(usage_roster_post),
        )
        .route(
            "/api/v1/orca/ide/hibernation/check",
            get(hibernation_check_get).post(hibernation_check_post),
        )
        .route("/api/v1/orca/ide/e2ee/handshake", post(e2ee_handshake))
        .route(
            "/api/v1/orca/ide/profile-state",
            get(profile_state_get)
                .put(profile_state_put)
                .post(profile_state_put),
        )
}
