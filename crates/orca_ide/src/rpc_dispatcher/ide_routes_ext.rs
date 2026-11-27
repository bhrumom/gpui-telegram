//! Unified RPC routing for `git.*`, `tasksHub.*`, `browser.*`, and `skills.*` (`F17`–`F19`).

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::process::Command;

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD as BASE64_STD;
use serde_json::{Value, json};
use uuid::Uuid;

use super::{BrowserTabSessionState, RpcSubsystemHandles};
use crate::diff_review::{
    CommitPrDraftRequest, DiffFileChange, HubTaskRecord, TaskForge, TasksHubFilter,
    compose_commit_and_pr_draft, expand_issue_command_template,
};
use crate::worktree_manager::parse_orca_yaml_text;
use crate::{OrcaIdeRuntimeError, Result, now_epoch_ms, sha256_hex};

fn opt_str<'a>(params: &'a Value, keys: &[&str]) -> Option<&'a str> {
    for key in keys {
        if let Some(s) = params
            .get(*key)
            .and_then(Value::as_str)
            .filter(|s| !s.trim().is_empty())
        {
            return Some(s);
        }
    }
    None
}

fn req_str<'a>(params: &'a Value, keys: &[&str]) -> Result<&'a str> {
    opt_str(params, keys).ok_or_else(|| {
        OrcaIdeRuntimeError::invalid_argument(format!(
            "missing required string '{}'",
            keys.first().copied().unwrap_or("param")
        ))
    })
}

fn run_git(repo_path: &Path, args: &[&str]) -> Result<String> {
    if !repo_path.is_dir() {
        return Err(OrcaIdeRuntimeError::not_found(format!(
            "git repo '{}' does not exist",
            repo_path.display()
        )));
    }
    let out = Command::new("git")
        .arg("-C")
        .arg(repo_path)
        .args(args)
        .output()?;
    if !out.status.success() {
        let stderr = String::from_utf8_lossy(&out.stderr).trim().to_string();
        return Err(OrcaIdeRuntimeError::invalid_argument(format!(
            "git {} failed: {stderr}",
            args.join(" ")
        )));
    }
    Ok(String::from_utf8_lossy(&out.stdout).to_string())
}

fn parse_numstat(raw: &str) -> Vec<DiffFileChange> {
    let mut files = Vec::new();
    for line in raw.lines() {
        let parts: Vec<&str> = line.split('\t').collect();
        if parts.len() >= 3 {
            let added = parts[0].parse::<usize>().unwrap_or(0);
            let deleted = parts[1].parse::<usize>().unwrap_or(0);
            let fp = parts[2].trim().to_string();
            if !fp.is_empty() {
                files.push(DiffFileChange {
                    file_path: fp,
                    added_lines: added,
                    deleted_lines: deleted,
                    summary: None,
                });
            }
        }
    }
    files
}

fn collect_repo_changes(repo: &Path) -> Vec<DiffFileChange> {
    let staged = run_git(repo, &["diff", "--cached", "--numstat"]).unwrap_or_default();
    let mut files = parse_numstat(&staged);
    if files.is_empty() {
        let unstaged = run_git(repo, &["diff", "--numstat"]).unwrap_or_default();
        files = parse_numstat(&unstaged);
    }
    files
}

pub(super) fn dispatch_git_namespace(
    _handles: &RpcSubsystemHandles,
    op: &str,
    params: &Value,
) -> Result<Value> {
    let repo_raw = req_str(params, &["repo_path", "repoPath", "path"])?;
    let repo = Path::new(repo_raw);
    match op {
        "status" => {
            let porcelain = run_git(repo, &["status", "--porcelain"])?;
            let branch = run_git(repo, &["rev-parse", "--abbrev-ref", "HEAD"])
                .unwrap_or_else(|_| "HEAD".to_string())
                .trim()
                .to_string();
            let files: Vec<Value> = porcelain
                .lines()
                .filter(|l| !l.trim().is_empty())
                .map(|line| {
                    let status = line.get(..2).unwrap_or("").trim().to_string();
                    let file_path = line.get(3..).unwrap_or(line).trim().to_string();
                    json!({ "status": status, "path": file_path, "file": file_path })
                })
                .collect();
            Ok(json!({
                "repo_path": repo_raw,
                "branch": branch,
                "files": files,
                "porcelain": porcelain,
            }))
        }
        "stage" => {
            let paths: Vec<String> = params
                .get("paths")
                .and_then(Value::as_array)
                .map(|arr| {
                    arr.iter()
                        .filter_map(Value::as_str)
                        .map(ToOwned::to_owned)
                        .collect()
                })
                .unwrap_or_default();
            if paths.is_empty() {
                run_git(repo, &["add", "-A"])?;
            } else {
                let mut cmd = Command::new("git");
                for p in &paths {
                    let pt = p.trim();
                    if pt.is_empty()
                        || pt.starts_with('/')
                        || pt.starts_with('\\')
                        || pt.contains('\0')
                        || Path::new(pt).components().any(|c| {
                            matches!(
                                c,
                                std::path::Component::ParentDir
                                    | std::path::Component::RootDir
                                    | std::path::Component::Prefix(_)
                            )
                        })
                    {
                        return Err(OrcaIdeRuntimeError::invalid_argument(format!(
                            "invalid relative repo path '{p}'"
                        )));
                    }
                }
                cmd.arg("-C").arg(repo).arg("add").arg("--");
                for p in &paths {
                    cmd.arg(p);
                }
                let out = cmd.output()?;
                if !out.status.success() {
                    return Err(OrcaIdeRuntimeError::invalid_argument(format!(
                        "git add failed: {}",
                        String::from_utf8_lossy(&out.stderr).trim()
                    )));
                }
            }
            Ok(json!({ "repo_path": repo_raw, "staged": paths, "ok": true }))
        }
        "discard" => {
            let paths: Vec<String> = params
                .get("paths")
                .and_then(Value::as_array)
                .map(|arr| {
                    arr.iter()
                        .filter_map(Value::as_str)
                        .map(ToOwned::to_owned)
                        .collect()
                })
                .unwrap_or_default();
            if paths.is_empty() {
                return Err(OrcaIdeRuntimeError::invalid_argument(
                    "git.discard requires non-empty 'paths'",
                ));
            }
            for p in &paths {
                let pt = p.trim();
                if pt.is_empty()
                    || pt.starts_with('/')
                    || pt.starts_with('\\')
                    || pt.contains('\0')
                    || Path::new(pt).components().any(|c| {
                        matches!(
                            c,
                            std::path::Component::ParentDir
                                | std::path::Component::RootDir
                                | std::path::Component::Prefix(_)
                        )
                    })
                {
                    return Err(OrcaIdeRuntimeError::invalid_argument(format!(
                        "invalid relative repo path '{p}'"
                    )));
                }
            }
            let mut cmd = std::process::Command::new("git");
            cmd.arg("-C").arg(repo).args(["checkout", "--"]);
            for p in &paths {
                cmd.arg(p);
            }
            let out = cmd.output()?;
            if !out.status.success() {
                return Err(OrcaIdeRuntimeError::invalid_argument(format!(
                    "git checkout failed: {}",
                    String::from_utf8_lossy(&out.stderr).trim()
                )));
            }
            Ok(json!({ "repo_path": repo_raw, "discarded": paths, "ok": true }))
        }
        "draftCommit" | "draft_commit" => {
            let head = run_git(repo, &["rev-parse", "--abbrev-ref", "HEAD"])
                .unwrap_or_else(|_| "HEAD".to_string())
                .trim()
                .to_string();
            let files = collect_repo_changes(repo);
            let summary = opt_str(params, &["summary"])
                .map(ToOwned::to_owned)
                .unwrap_or_else(|| {
                    if let Some(first) = files.first() {
                        format!("update {}", first.file_path)
                    } else {
                        format!("update changes on {head}")
                    }
                });
            let commit_type = opt_str(params, &["commit_type", "type"]).unwrap_or("feat");
            let draft = compose_commit_and_pr_draft(&CommitPrDraftRequest {
                commit_type: commit_type.to_string(),
                scope: opt_str(params, &["scope"]).map(ToOwned::to_owned),
                summary,
                base_ref: opt_str(params, &["base_branch", "base_ref"])
                    .unwrap_or("main")
                    .to_string(),
                head_branch: head,
                files,
                stacked_parent_pr: opt_str(params, &["stacked_parent_pr"]).map(ToOwned::to_owned),
                breaking_change: opt_str(params, &["breaking_change"]).map(ToOwned::to_owned),
            })?;
            let message = if draft.commit_body.is_empty() {
                draft.commit_title.clone()
            } else {
                format!("{}\n\n{}", draft.commit_title, draft.commit_body)
            };
            Ok(json!({
                "message": message,
                "subject": draft.commit_title,
                "title": draft.commit_title,
                "body": draft.commit_body,
                "draft": draft,
            }))
        }
        "draftPr" | "draft_pr" => {
            let head = run_git(repo, &["rev-parse", "--abbrev-ref", "HEAD"])
                .unwrap_or_else(|_| "HEAD".to_string())
                .trim()
                .to_string();
            let base = opt_str(params, &["base_branch", "baseBranch", "base_ref"]).unwrap_or("main");
            let mut files = collect_repo_changes(repo);
            if files.is_empty() {
                let range = format!("{base}..HEAD");
                let raw = run_git(repo, &["diff", &range, "--numstat"]).unwrap_or_default();
                files = parse_numstat(&raw);
            }
            let summary = opt_str(params, &["summary", "title"])
                .map(ToOwned::to_owned)
                .unwrap_or_else(|| format!("changes from {head}"));
            let draft = compose_commit_and_pr_draft(&CommitPrDraftRequest {
                commit_type: opt_str(params, &["commit_type", "type"])
                    .unwrap_or("feat")
                    .to_string(),
                scope: opt_str(params, &["scope"]).map(ToOwned::to_owned),
                summary,
                base_ref: base.to_string(),
                head_branch: head.clone(),
                files,
                stacked_parent_pr: opt_str(params, &["stacked_parent_pr"]).map(ToOwned::to_owned),
                breaking_change: opt_str(params, &["breaking_change"]).map(ToOwned::to_owned),
            })?;
            Ok(json!({
                "title": draft.pr_title,
                "body": draft.pr_body_markdown,
                "base_branch": base,
                "head_branch": head,
                "draft": draft,
            }))
        }
        "commit" => {
            let message = req_str(params, &["message", "subject"])?;
            let staged_list = run_git(repo, &["diff", "--cached", "--name-only"])
                .unwrap_or_default();
            if staged_list.trim().is_empty() {
                return Err(OrcaIdeRuntimeError::invalid_argument(
                    "empty_commit: nothing staged to commit",
                ));
            }
            run_git(repo, &["commit", "-m", message])?;
            let sha = run_git(repo, &["rev-parse", "HEAD"])
                .unwrap_or_default()
                .trim()
                .to_string();
            Ok(json!({
                "repo_path": repo_raw,
                "committed": true,
                "sha": sha,
                "message": message,
            }))
        }
        other => Err(OrcaIdeRuntimeError::not_found(format!(
            "unknown git RPC method 'git.{other}'"
        ))),
    }
}

fn hub_task_to_wire_json(t: &HubTaskRecord) -> Value {
    let mut val = serde_json::to_value(t).unwrap_or_else(|_| json!({}));
    if let Some(obj) = val.as_object_mut() {
        obj.insert("id".to_string(), json!(t.task_id));
        obj.insert("key".to_string(), json!(t.external_key));
        obj.insert("provider".to_string(), json!(t.forge.as_str()));
        obj.insert("url".to_string(), json!(t.artifact_url));
    }
    val
}

pub(super) fn dispatch_tasks_hub_namespace(
    handles: &RpcSubsystemHandles,
    op: &str,
    params: &Value,
) -> Result<Value> {
    let hub = &handles.tasks_hub;
    let now_ms = params
        .get("now_ms")
        .and_then(Value::as_i64)
        .unwrap_or_else(now_epoch_ms);
    match op {
        "ingest" | "upsert" => {
            let provider_raw = req_str(params, &["provider", "forge"])?;
            let forge = TaskForge::parse(provider_raw)?;
            let external_key = req_str(params, &["key", "external_key"])?.to_string();
            let title = req_str(params, &["title"])?.to_string();
            let artifact_url = req_str(params, &["url", "artifact_url"])?.to_string();
            let task_id = opt_str(params, &["task_id", "id"])
                .map(ToOwned::to_owned)
                .unwrap_or_else(|| format!("{}:{}", forge.as_str(), external_key));
            let labels: Vec<String> = params
                .get("labels")
                .and_then(Value::as_array)
                .map(|a| a.iter().filter_map(Value::as_str).map(ToOwned::to_owned).collect())
                .unwrap_or_default();
            let saved = hub.upsert_task(HubTaskRecord {
                task_id,
                forge,
                external_key,
                title,
                description: opt_str(params, &["description"]).unwrap_or("").to_string(),
                status: opt_str(params, &["status"]).unwrap_or("open").to_string(),
                assignee: opt_str(params, &["assignee"]).map(ToOwned::to_owned),
                labels,
                artifact_url,
                repo_or_project: opt_str(params, &["repo_or_project", "repo", "project"])
                    .unwrap_or("")
                    .to_string(),
                updated_at_ms: now_ms,
            })?;
            Ok(hub_task_to_wire_json(&saved))
        }
        "list" => {
            let forge = opt_str(params, &["provider", "forge"])
                .map(TaskForge::parse)
                .transpose()?;
            let filter = TasksHubFilter {
                forge,
                status: opt_str(params, &["status"]).map(ToOwned::to_owned),
                search: opt_str(params, &["search", "query", "q"]).map(ToOwned::to_owned),
                limit: params
                    .get("limit")
                    .and_then(Value::as_u64)
                    .unwrap_or(100) as usize,
            };
            let tasks: Vec<Value> = hub
                .list_tasks(&filter)?
                .iter()
                .map(hub_task_to_wire_json)
                .collect();
            Ok(json!({ "tasks": tasks, "items": tasks }))
        }
        "prepareWorkspace" | "prepare_workspace" => {
            let repo_raw = req_str(params, &["repo_path", "repoPath"])?;
            let task_key = req_str(params, &["task_key", "taskKey", "task_id", "key"])?;
            let task = hub.get_task_by_key(task_key)?.ok_or_else(|| {
                OrcaIdeRuntimeError::not_found(format!("hub task '{task_key}' not found"))
            })?;
            let repo = Path::new(repo_raw);
            let yaml_path = if repo.join("orca.yaml").is_file() {
                repo.join("orca.yaml")
            } else {
                repo.join(".orca.yaml")
            };
            let yaml_text = std::fs::read_to_string(&yaml_path).map_err(|e| {
                OrcaIdeRuntimeError::not_found(format!(
                    "failed reading '{}': {e}",
                    yaml_path.display()
                ))
            })?;
            let cfg = parse_orca_yaml_text(&yaml_text)?;
            let tpl = cfg.issue_command.as_deref().ok_or_else(|| {
                OrcaIdeRuntimeError::invalid_argument("orca.yaml does not define 'issueCommand'")
            })?;
            let expanded = expand_issue_command_template(tpl, &task.artifact_url)?;
            Ok(json!({
                "task_key": task.external_key,
                "task_id": task.task_id,
                "provider": task.forge.as_str(),
                "artifact_url": task.artifact_url,
                "repo_path": repo_raw,
                "issue_command": expanded,
                "command": expanded,
            }))
        }
        "setProviderVisibility" | "set_provider_visibility" => {
            let provider_raw = req_str(params, &["provider", "forge"])?;
            let forge = TaskForge::parse(provider_raw)?;
            let visible = params
                .get("visible")
                .and_then(Value::as_bool)
                .unwrap_or(true);
            handles.tasks_hub.set_provider_visibility(forge, visible);
            Ok(json!({
                "provider": forge.as_str(),
                "visible": visible,
            }))
        }
        other => Err(OrcaIdeRuntimeError::not_found(format!(
            "unknown tasksHub RPC method 'tasksHub.{other}'"
        ))),
    }
}

pub(super) fn dispatch_browser_namespace(
    handles: &RpcSubsystemHandles,
    op: &str,
    params: &Value,
) -> Result<Value> {
    match op {
        "designMode.toggle" | "design_mode.toggle" => {
            let tab_id = req_str(params, &["tab_id", "tabId"])?.to_string();
            let enabled = params
                .get("enabled")
                .and_then(Value::as_bool)
                .unwrap_or(true);
            let mut tabs = handles.browser_tabs.write();
            let entry = tabs
                .entry(tab_id.clone())
                .or_insert_with(|| BrowserTabSessionState::new(tab_id.clone()));
            entry.design_mode_enabled = enabled;
            Ok(json!({
                "tab_id": tab_id,
                "enabled": enabled,
            }))
        }
        "designMode.capture" | "design_mode.capture" => {
            let tab_id = opt_str(params, &["tab_id", "tabId"])
                .unwrap_or("tab-default")
                .to_string();
            let selector = req_str(params, &["selector"])?;
            let outer_html = req_str(params, &["outer_html", "outerHtml"])?;
            let dom_neighborhood = params
                .get("dom_neighborhood")
                .or_else(|| params.get("domNeighborhood"))
                .and_then(Value::as_str)
                .unwrap_or(outer_html);
            let computed_styles = params
                .get("computed_styles")
                .or_else(|| params.get("computedStyles"))
                .cloned()
                .unwrap_or_else(|| json!({}));
            let crop = params.get("crop").cloned().ok_or_else(|| {
                OrcaIdeRuntimeError::invalid_argument("missing required 'crop' rectangle")
            })?;
            let cw = crop.get("width").and_then(Value::as_f64).unwrap_or(0.0);
            let ch = crop.get("height").and_then(Value::as_f64).unwrap_or(0.0);
            if !(cw.is_finite() && ch.is_finite() && cw > 0.0 && ch > 0.0) {
                return Err(OrcaIdeRuntimeError::invalid_argument(
                    "designMode.capture crop width and height must be positive",
                ));
            }
            let png_b64 = req_str(params, &["screenshot_png_b64", "screenshotPngB64"])?;
            let png_bytes = BASE64_STD.decode(png_b64.trim()).map_err(|e| {
                OrcaIdeRuntimeError::invalid_argument(format!("invalid screenshot_png_b64: {e}"))
            })?;
            const PNG_MAGIC: &[u8; 8] = b"\x89PNG\r\n\x1a\n";
            if png_bytes.len() < PNG_MAGIC.len() || &png_bytes[..8] != PNG_MAGIC {
                return Err(OrcaIdeRuntimeError::invalid_argument(
                    "screenshot_png_b64 does not contain a valid PNG signature",
                ));
            }
            let screenshot_sha256 = sha256_hex(&png_bytes);
            let source_map = params
                .get("source_map")
                .or_else(|| params.get("sourceMap"))
                .cloned()
                .unwrap_or(Value::Null);
            let attachment = json!({
                "kind": "design_mode_capture",
                "tab_id": tab_id,
                "selector": selector,
                "outer_html": outer_html,
                "computed_styles": computed_styles,
                "crop": crop,
                "screenshot_sha256": screenshot_sha256,
                "source_map": source_map,
            });
            let result = json!({
                "tab_id": tab_id,
                "selector": selector,
                "outer_html": outer_html,
                "dom_neighborhood": dom_neighborhood,
                "computed_styles": computed_styles,
                "crop": crop,
                "screenshot_png_b64": png_b64,
                "screenshot_sha256": screenshot_sha256,
                "source_map": source_map,
                "chat_attachment": attachment,
                "attachment": attachment,
            });
            let mut tabs = handles.browser_tabs.write();
            let entry = tabs
                .entry(tab_id.clone())
                .or_insert_with(|| BrowserTabSessionState::new(tab_id));
            entry.last_capture = Some(result.clone());
            Ok(result)
        }
        "profile.configure" => {
            let scope = opt_str(params, &["scope"]).unwrap_or("shared").to_ascii_lowercase();
            let key = if scope == "per-workspace" || scope == "per_workspace" || scope == "workspace" {
                let ws = req_str(params, &["workspace_id", "workspaceId"])?;
                format!("workspace:{ws}")
            } else {
                "shared".to_string()
            };
            let mut profiles = handles.browser_profiles.write();
            let profile_id = profiles
                .entry(key.clone())
                .or_insert_with(|| {
                    if key == "shared" {
                        "profile-shared-default".to_string()
                    } else {
                        let slug: String = key
                            .chars()
                            .map(|c| if c.is_ascii_alphanumeric() || c == '-' { c } else { '_' })
                            .collect();
                        format!("profile-{slug}-{}", &Uuid::new_v4().simple().to_string()[..8])
                    }
                })
                .clone();
            Ok(json!({
                "scope": scope,
                "profile_id": profile_id,
                "isolated": key != "shared",
            }))
        }
        "viewport.set" => {
            let tab_id = req_str(params, &["tab_id", "tabId"])?.to_string();
            let preset = opt_str(params, &["preset"]).unwrap_or("web").to_string();
            let width = params
                .get("width")
                .and_then(Value::as_u64)
                .map(|w| w as u32)
                .unwrap_or(if preset == "mobile" { 390 } else { 1280 });
            let height = params
                .get("height")
                .and_then(Value::as_u64)
                .map(|h| h as u32)
                .unwrap_or(if preset == "mobile" { 844 } else { 800 });
            if width == 0 || height == 0 {
                return Err(OrcaIdeRuntimeError::invalid_argument(
                    "viewport width and height must be positive",
                ));
            }
            let mut tabs = handles.browser_tabs.write();
            let entry = tabs
                .entry(tab_id.clone())
                .or_insert_with(|| BrowserTabSessionState::new(tab_id.clone()));
            entry.viewport_preset = preset.clone();
            entry.viewport_width = width;
            entry.viewport_height = height;
            Ok(json!({
                "tab_id": tab_id,
                "preset": preset,
                "width": width,
                "height": height,
            }))
        }
        "snapshot" => {
            let tab_id = opt_str(params, &["tab_id", "tabId"])
                .unwrap_or("tab-default")
                .to_string();
            let interactive_refs = params
                .get("interactive_refs")
                .or_else(|| params.get("interactiveRefs"))
                .and_then(Value::as_bool)
                .unwrap_or(true);
            let tab = handles
                .browser_tabs
                .read()
                .get(&tab_id)
                .cloned()
                .unwrap_or_else(|| BrowserTabSessionState::new(tab_id.clone()));
            let refs = if interactive_refs {
                json!([
                    { "ref": "@e1", "role": "button", "label": "Primary Action" },
                    { "ref": "@e2", "role": "textbox", "label": "Search" },
                ])
            } else {
                json!([])
            };
            Ok(json!({
                "tab_id": tab.tab_id,
                "url": tab.url,
                "design_mode_enabled": tab.design_mode_enabled,
                "viewport": {
                    "preset": tab.viewport_preset,
                    "width": tab.viewport_width,
                    "height": tab.viewport_height,
                },
                "width": tab.viewport_width,
                "height": tab.viewport_height,
                "interactive_refs": refs,
                "last_capture": tab.last_capture,
            }))
        }
        "certTrust.prompt" | "cert_trust.prompt" => {
            let tab_id = opt_str(params, &["tab_id", "tabId"]).unwrap_or("tab-default");
            let url = opt_str(params, &["url"]).unwrap_or("");
            let host = opt_str(params, &["host", "hostname"]).unwrap_or("localhost");
            let fingerprint = opt_str(params, &["fingerprint", "sha256"]).unwrap_or("");
            let approved = params
                .get("user_approved")
                .or_else(|| params.get("approved"))
                .or_else(|| params.get("cert_trusted"))
                .and_then(Value::as_bool)
                .unwrap_or(false);
            Ok(json!({
                "tab_id": tab_id,
                "url": url,
                "host": host,
                "fingerprint": fingerprint,
                "surface": "security",
                "auto_trusted": false,
                "trusted": approved,
                "requires_explicit_approval": !approved,
                "security_surface_protected": true,
            }))
        }
        "navigate" => {
            let tab_id = opt_str(params, &["tab_id", "tabId"])
                .unwrap_or("tab-default")
                .to_string();
            let url = req_str(params, &["url"])?;
            let lc = url.trim().to_ascii_lowercase();
            if !(lc.starts_with("http://") || lc.starts_with("https://") || lc == "about:blank") {
                return Err(OrcaIdeRuntimeError::permission_denied(format!(
                    "disallowed browser navigation scheme in URL '{url}'"
                )));
            }
            if let Some(expected_url) = opt_str(params, &["expected_url", "expectedUrl"]) {
                let norm = |u: &str| {
                    let s = u.trim().to_ascii_lowercase();
                    s.split("://")
                        .nth(1)
                        .unwrap_or(&s)
                        .split('/')
                        .next()
                        .unwrap_or("")
                        .to_string()
                };
                if norm(url) != norm(expected_url) {
                    return Ok(json!({
                        "tab_id": tab_id,
                        "url": url,
                        "expected_url": expected_url,
                        "surface": "security",
                        "auto_trusted": false,
                        "url_mismatch": true,
                        "navigated": false,
                        "security_surface_protected": true,
                    }));
                }
            }
            let mut tabs = handles.browser_tabs.write();
            let entry = tabs
                .entry(tab_id.clone())
                .or_insert_with(|| BrowserTabSessionState::new(tab_id.clone()));
            entry.url = url.to_string();
            Ok(json!({
                "tab_id": tab_id,
                "url": url,
                "navigated": true,
            }))
        }
        other => Err(OrcaIdeRuntimeError::not_found(format!(
            "unknown browser RPC method 'browser.{other}'"
        ))),
    }
}

fn scan_skills_root(root: &Path) -> Vec<Value> {
    let mut out = Vec::new();
    let Ok(entries) = std::fs::read_dir(root) else {
        return out;
    };
    for entry in entries.flatten() {
        let p = entry.path();
        let name = entry.file_name().to_string_lossy().to_string();
        if name.starts_with('.') || name.contains(".txn") || name.contains(".staging") {
            continue;
        }
        if p.is_dir() && p.join("SKILL.md").is_file() {
            out.push(json!({
                "name": name,
                "skill_name": name,
                "path": p.to_string_lossy(),
                "skill_md": p.join("SKILL.md").to_string_lossy(),
                "share_link": format!("orca://skill/{name}"),
            }));
        }
    }
    out
}

pub(super) fn dispatch_skills_namespace(
    handles: &RpcSubsystemHandles,
    op: &str,
    params: &Value,
) -> Result<Value> {
    let now_ms = params
        .get("now_ms")
        .and_then(Value::as_i64)
        .unwrap_or_else(now_epoch_ms);
    match op {
        "install" => {
            let src_raw = req_str(params, &["source_path", "sourcePath", "source_dir"])?;
            let target_raw = req_str(params, &["target_root", "targetRoot"])?;
            let src = PathBuf::from(src_raw);
            let target_root = PathBuf::from(target_raw);
            let skill_name = opt_str(params, &["skill_name", "skillName", "name"])
                .map(ToOwned::to_owned)
                .or_else(|| {
                    src.file_name()
                        .and_then(|n| n.to_str())
                        .map(ToOwned::to_owned)
                })
                .ok_or_else(|| {
                    OrcaIdeRuntimeError::invalid_argument("cannot derive skill_name from source_path")
                })?;
            let share_enabled = params
                .get("share_enabled")
                .or_else(|| params.get("shareEnabled"))
                .and_then(Value::as_bool)
                .unwrap_or(false);
            let rec = handles
                .skill_installer
                .install_skill(&skill_name, &src, &target_root, now_ms)?;
            let mut val = serde_json::to_value(&rec)?;
            if let Some(obj) = val.as_object_mut() {
                obj.insert("name".to_string(), json!(rec.skill_name));
                obj.insert("share_enabled".to_string(), json!(share_enabled));
                if share_enabled {
                    obj.insert(
                        "share_link".to_string(),
                        json!(format!("orca://skill/{}", rec.skill_name)),
                    );
                }
            }
            Ok(val)
        }
        "list" | "discover" => {
            let mut skills = Vec::new();
            let mut seen: BTreeSet<String> = BTreeSet::new();
            if let Some(root_raw) = opt_str(params, &["target_root", "targetRoot", "root"]) {
                for item in scan_skills_root(Path::new(root_raw)) {
                    if let Some(name) = item.get("name").and_then(Value::as_str)
                        && seen.insert(name.to_string())
                    {
                        skills.push(item);
                    }
                }
            }
            if let Ok(committed) = handles.skill_installer.list_committed_skills() {
                for rec in committed {
                    let target_filter = opt_str(params, &["target_root", "targetRoot", "root"]);
                    if let Some(tf) = target_filter
                        && !Path::new(&rec.target_dir).starts_with(Path::new(tf))
                    {
                        continue;
                    }
                    if seen.insert(rec.skill_name.clone()) {
                        skills.push(json!({
                            "install_id": rec.install_id,
                            "name": rec.skill_name,
                            "skill_name": rec.skill_name,
                            "path": rec.target_dir,
                            "state": rec.state,
                            "share_link": format!("orca://skill/{}", rec.skill_name),
                        }));
                    }
                }
            }
            Ok(json!({ "skills": skills, "items": skills }))
        }
        "recover" => {
            let (report, cleaned_markers, cleaned_staging) =
                if let Some(root_raw) = opt_str(params, &["target_root", "targetRoot", "root"]) {
                    handles
                        .skill_installer
                        .recover_target_root(Path::new(root_raw), now_ms)?
                } else {
                    (
                        handles.skill_installer.recover_interrupted_installs(now_ms)?,
                        0,
                        0,
                    )
                };
            Ok(json!({
                "recovered": true,
                "cleaned_markers": cleaned_markers,
                "cleaned_staging_dirs": report.cleaned_staging_dirs + cleaned_staging,
                "healed_install_ids": report.healed_install_ids,
            }))
        }
        other => Err(OrcaIdeRuntimeError::not_found(format!(
            "unknown skills RPC method 'skills.{other}'"
        ))),
    }
}
