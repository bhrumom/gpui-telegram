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
    ComputerUsePolicy, ComputerUseRequest, DiffAnnotation, DiffSide, LineAttribution,
    PluginManifestV2,
};
use orca_ide::accounts_usage::ProviderQuotaSnapshot;
use orca_ide::hibernation::HibernationCandidateState;
use orca_ide::plugin_host::PluginContributionPoint;
use orca_ide::profile_state::DirtyDomainMutation;
use orca_ide::rpc_dispatcher::E2eeHandshakeInitRequest;
use orca_ide::system_services::{ArtifactKind, PublishArtifactRequest};
use orca_ide::vault_search::{VaultMessageInput, VaultSearchRequest};
use orca_ide::worktree_manager::{
    execute_worktree_include_copy, parse_orca_yaml_text, CleanupWorktreeRequest,
    CreateWorktreeRequest, WorkspaceStatus, WorktreeIncludeCopyOptions,
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

/// `POST /api/v1/orca/ide/orca-yaml/parse` (`200 OK`).
/// Accepts JSON `{yaml | content}` OR raw `text/plain` / YAML body.
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
        } else if let Some(s) = val.as_str() {
            s.to_string()
        } else {
            return orca_err(
                StatusCode::BAD_REQUEST,
                "invalid_argument",
                "Expected JSON object with 'yaml' or 'content' string, or raw text/plain YAML.",
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
        Ok(cfg) => Json(cfg).into_response(),
        Err(err) => map_ide_err(err),
    }
}

// ---------------------------------------------------------------------------
// 2. Worktrees, `.worktreeinclude`, Listening Ports & Guarded Cleanup
// ---------------------------------------------------------------------------

/// `GET /api/v1/orca/ide/worktrees` (`200 OK`).
pub async fn list_worktrees(
    _auth: AuthBearer,
    State(state): State<Arc<AppState>>,
) -> Response {
    match state.orca_ide.worktrees().list_worktrees() {
        Ok(list) => Json(list).into_response(),
        Err(err) => map_ide_err(err),
    }
}

#[derive(Debug, Deserialize)]
pub struct CreateWorktreeBody {
    #[serde(default)]
    pub id: Option<String>,
    #[serde(alias = "repoRoot")]
    pub repo_root: String,
    #[serde(alias = "worktreePath")]
    pub worktree_path: String,
    #[serde(default, alias = "branchName")]
    pub branch_name: Option<String>,
    #[serde(default)]
    pub title: Option<String>,
    #[serde(default, alias = "parentWorktreeId")]
    pub parent_worktree_id: Option<String>,
    #[serde(default, alias = "projectGroupId")]
    pub project_group_id: Option<String>,
    #[serde(default = "default_true", alias = "createdByOrca")]
    pub created_by_orca: bool,
    #[serde(default, alias = "existingBranches")]
    pub existing_branches: BTreeSet<String>,
    #[serde(default, alias = "seedHint")]
    pub seed_hint: Option<String>,
}

const fn default_true() -> bool {
    true
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
    let create_req = CreateWorktreeRequest {
        id: req.id,
        repo_root: PathBuf::from(req.repo_root),
        worktree_path: PathBuf::from(req.worktree_path),
        branch_name: req.branch_name,
        title: req.title,
        parent_worktree_id: req.parent_worktree_id,
        project_group_id: req.project_group_id,
        created_by_orca: req.created_by_orca,
        existing_branches: req.existing_branches,
        seed_hint: req.seed_hint,
    };
    match state.orca_ide.worktrees().create_worktree(create_req) {
        Ok(rec) => (StatusCode::CREATED, Json(rec)).into_response(),
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
        Ok(updated) => Json(updated).into_response(),
        Err(err) => map_ide_err(err),
    }
}

#[derive(Debug, Deserialize)]
pub struct IncludeCopyBody {
    #[serde(alias = "sourceRepoRoot")]
    pub source_repo_root: String,
    #[serde(alias = "targetWorktreeRoot")]
    pub target_worktree_root: String,
    #[serde(default)]
    pub rules: String,
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
    match execute_worktree_include_copy(&src, &dst, &req.rules, &req.options) {
        Ok(report) => Json(report).into_response(),
        Err(err) => map_ide_err(err),
    }
}

/// `GET /api/v1/orca/ide/worktrees/ports` (`200 OK`).
pub async fn worktree_ports(
    _auth: AuthBearer,
    State(state): State<Arc<AppState>>,
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
    Json(attributed).into_response()
}

#[derive(Debug, Deserialize)]
pub struct CleanupWorktreeBody {
    #[serde(alias = "worktreeId", alias = "id")]
    pub worktree_id: String,
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
    let cleanup = CleanupWorktreeRequest {
        worktree_id: req.worktree_id,
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
    #[serde(default)]
    pub file_path: Option<String>,
    #[serde(default)]
    pub include_resolved: bool,
    #[serde(default)]
    pub batch: bool,
}

/// `GET /api/v1/orca/ide/diff-annotations` (`200 OK`).
/// Supports `?file_path=&include_resolved=&batch=true`.
pub async fn list_diff_annotations(
    _auth: AuthBearer,
    State(state): State<Arc<AppState>>,
    Query(query): Query<DiffAnnotationsQuery>,
) -> Response {
    let fp = query
        .file_path
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty());
    let diff = state.orca_ide.diff_review();
    if query.batch {
        let prompt = diff.build_agent_batch_prompt(fp);
        return Json(json!({ "prompt": prompt })).into_response();
    }
    Json(diff.list_annotations(fp, query.include_resolved)).into_response()
}

/// `GET /api/v1/orca/ide/diff-annotations/batch` (`200 OK`).
pub async fn diff_annotations_batch(
    _auth: AuthBearer,
    State(state): State<Arc<AppState>>,
    Query(query): Query<DiffAnnotationsQuery>,
) -> Response {
    let fp = query
        .file_path
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty());
    let prompt = state.orca_ide.diff_review().build_agent_batch_prompt(fp);
    Json(json!({ "prompt": prompt })).into_response()
}

#[derive(Debug, Deserialize)]
pub struct CreateDiffAnnotationBody {
    #[serde(default, alias = "annotationId", alias = "id")]
    pub annotation_id: Option<String>,
    #[serde(alias = "filePath")]
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
    match state.orca_ide.diff_review().add_annotation(ann) {
        Ok(saved) => (StatusCode::CREATED, Json(saved)).into_response(),
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
    #[serde(default)]
    pub workspace_id: Option<String>,
    #[serde(default)]
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
        .or_else(|| val.get("agentId"))
        .and_then(Value::as_str)
        .filter(|s| !s.trim().is_empty())
        .unwrap_or("claude");
    let workspace_id = val
        .get("workspace_id")
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
    Json(state.orca_ide.plugin_host().list_plugins()).into_response()
}

fn parse_manifest_and_contributions(
    val: &Value,
) -> Result<(PluginManifestV2, Vec<PluginContributionPoint>), Response> {
    let manifest_val = val.get("manifest").cloned().unwrap_or_else(|| val.clone());
    let manifest: PluginManifestV2 = serde_json::from_value(manifest_val).map_err(|err| {
        orca_err(
            StatusCode::BAD_REQUEST,
            "invalid_argument",
            format!("Invalid PluginManifestV2: {err}"),
        )
    })?;
    let contributions: Vec<PluginContributionPoint> = match val.get("contributions") {
        Some(c) => serde_json::from_value(c.clone()).map_err(|err| {
            orca_err(
                StatusCode::BAD_REQUEST,
                "invalid_argument",
                format!("Invalid plugin contributions: {err}"),
            )
        })?,
        None => Vec::new(),
    };
    Ok((manifest, contributions))
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
            let (manifest, contributions) = match parse_manifest_and_contributions(&val) {
                Ok(parsed) => parsed,
                Err(resp) => return resp,
            };
            match host.validate_manifest(&manifest, &contributions) {
                Ok(summary) => Json(summary).into_response(),
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
            let (manifest, contributions) = match parse_manifest_and_contributions(&val) {
                Ok(parsed) => parsed,
                Err(resp) => return resp,
            };
            let pkg_dir = val
                .get("package_dir")
                .or_else(|| val.get("packageDir"))
                .and_then(Value::as_str)
                .filter(|s| !s.trim().is_empty())
                .map(PathBuf::from);
            let record = match host.install_plugin(
                manifest,
                contributions,
                pkg_dir.as_deref(),
                now_ms,
            ) {
                Ok(rec) => rec,
                Err(err) => return map_ide_err(err),
            };
            let auto_approve = val
                .get("approve")
                .and_then(Value::as_bool)
                .unwrap_or(false);
            if auto_approve
                && let Err(err) =
                    host.approve_plugin(&record.manifest.id, &record.fingerprint, now_ms)
            {
                return map_ide_err(err);
            }
            let enforce_approval = val
                .get("enforce_approval")
                .or_else(|| val.get("enforceApproval"))
                .and_then(Value::as_bool)
                .unwrap_or(true);
            if enforce_approval
                && let Err(err) = host.evaluate_plugin(&record.manifest.id)
            {
                return map_ide_err(err);
            }
            (StatusCode::CREATED, Json(record)).into_response()
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
            Ok(view) => Json(view).into_response(),
            Err(err) => map_ide_err(err),
        };
    }
    let profile_id = query
        .profile_id
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty());
    match store.list_artifacts(profile_id) {
        Ok(views) => Json(views).into_response(),
        Err(err) => map_ide_err(err),
    }
}

#[derive(Debug, Deserialize)]
pub struct PublishArtifactBody {
    #[serde(default, alias = "artifactId")]
    pub artifact_id: Option<String>,
    pub title: String,
    #[serde(default)]
    pub kind: Option<String>,
    #[serde(default)]
    pub content: String,
    #[serde(default, alias = "profileId")]
    pub profile_id: Option<String>,
    #[serde(default, alias = "userConfirmedShare", alias = "confirmed")]
    pub user_confirmed_share: bool,
    #[serde(default, alias = "baseUrl")]
    pub base_url: Option<String>,
    #[serde(default, alias = "nowMs")]
    pub now_ms: Option<i64>,
}

/// `POST /api/v1/orca/ide/artifacts` (`201 Created`, `403 approval_required` if unconfirmed).
/// Returns strictly `ArtifactPublicView` — sealed `edit_token` is NEVER exposed!
pub async fn publish_artifact(
    _auth: AuthBearer,
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
        None => ArtifactKind::Markdown,
    };
    let publish = PublishArtifactRequest {
        artifact_id: req.artifact_id,
        title: req.title,
        kind,
        content: req.content,
        profile_id: req
            .profile_id
            .filter(|s| !s.trim().is_empty())
            .unwrap_or_else(|| "default".to_string()),
        user_confirmed_share: req.user_confirmed_share,
        base_url: req.base_url,
        now_ms: req.now_ms.unwrap_or_else(now_epoch_ms),
    };
    match state.orca_ide.artifacts().publish_artifact(publish) {
        Ok(view) => (StatusCode::CREATED, Json(view)).into_response(),
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
        Ok(decision) => Json(decision).into_response(),
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
    Json(state.orca_ide.usage_roster().sorted_roster(now_ms)).into_response()
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
    #[serde(default, alias = "tabId")]
    pub tab_id: Option<String>,
    #[serde(default, alias = "agentStateIsDone")]
    pub agent_state_is_done: Option<bool>,
    #[serde(default, alias = "tabVisible")]
    pub tab_visible: Option<bool>,
    #[serde(default, alias = "keystrokesSinceDone")]
    pub keystrokes_since_done: Option<u64>,
    #[serde(default, alias = "agentId")]
    pub agent_id: Option<String>,
    #[serde(default, alias = "idleDurationMs")]
    pub idle_duration_ms: Option<u64>,
    #[serde(default, alias = "mobileDriverAttached")]
    pub mobile_driver_attached: Option<bool>,
    #[serde(default, alias = "unsettledOrchestrationDispatch")]
    pub unsettled_orchestration_dispatch: Option<bool>,
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
    fn into_candidate(self) -> HibernationCandidateState {
        HibernationCandidateState {
            tab_id: self.tab_id.unwrap_or_else(|| "tab-check".to_string()),
            agent_state_is_done: self.agent_state_is_done.unwrap_or(true),
            tab_visible: self.tab_visible.unwrap_or(false),
            keystrokes_since_done: self.keystrokes_since_done.unwrap_or(0),
            agent_id: self.agent_id.unwrap_or_else(|| "claude".to_string()),
            idle_duration_ms: self.idle_duration_ms.unwrap_or(1_800_000),
            mobile_driver_attached: self.mobile_driver_attached.unwrap_or(false),
            unsettled_orchestration_dispatch: self
                .unsettled_orchestration_dispatch
                .unwrap_or(false),
            live_subagent_count: self.live_subagent_count.unwrap_or(0),
            git_operation_in_progress: self.git_operation_in_progress.unwrap_or(false),
            active_port_forward: self.active_port_forward.unwrap_or(false),
            pending_bot_approval: self.pending_bot_approval.unwrap_or(false),
            unsynced_local_snapshot: self.unsynced_local_snapshot.unwrap_or(false),
            pinned_no_hibernate: self.pinned_no_hibernate.unwrap_or(false),
            session_handle: self.session_handle,
            original_cwd: PathBuf::from(self.original_cwd.unwrap_or_else(|| ".".to_string())),
            original_args: self.original_args.unwrap_or_default(),
            original_env: self.original_env.unwrap_or_default(),
        }
    }
}

/// `GET /api/v1/orca/ide/hibernation/check` (`200 OK` / `409 hibernation_blocked`).
pub async fn hibernation_check_get(
    _auth: AuthBearer,
    State(state): State<Arc<AppState>>,
    Query(params): Query<HibernationCheckParams>,
) -> Response {
    let candidate = params.into_candidate();
    match state.orca_ide.hibernation().evaluate(&candidate) {
        Ok(decision) => Json(decision).into_response(),
        Err(err) => map_ide_err(err),
    }
}

/// `POST /api/v1/orca/ide/hibernation/check` (`200 OK` / `409 hibernation_blocked`).
pub async fn hibernation_check_post(
    _auth: AuthBearer,
    State(state): State<Arc<AppState>>,
    body: Bytes,
) -> Response {
    let candidate: HibernationCandidateState = match serde_json::from_slice(&body) {
        Ok(c) => c,
        Err(_) => match parse_optional_json_body::<HibernationCheckParams>(&body) {
            Ok(p) => p.into_candidate(),
            Err(resp) => return resp,
        },
    };
    match state.orca_ide.hibernation().evaluate(&candidate) {
        Ok(decision) => Json(decision).into_response(),
        Err(err) => map_ide_err(err),
    }
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
}

/// `GET /api/v1/orca/ide/profile-state` (`200 OK`).
pub async fn profile_state_get(
    _auth: AuthBearer,
    State(state): State<Arc<AppState>>,
    Query(query): Query<ProfileStateQuery>,
) -> Response {
    let profile = state.orca_ide.profile_state();
    if let Some(domain) = query
        .domain
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
    {
        return match profile.get_domain(domain) {
            Ok(doc) => Json(doc).into_response(),
            Err(err) => map_ide_err(err),
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
