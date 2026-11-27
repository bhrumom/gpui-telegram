//! Unified RPC routing for `runtime.*`, `aiVault.*`, `diff.*`, `plugins.*`, `computer.*`,
//! `emulator.*`, `artifacts.*`, `accounts.*`, `usage.*`, `hibernation.*`, `e2ee.*`,
//! `profile.*`, `vm.*`, `speech.*`, and `watchdog.*`.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD as BASE64_STD;
use orca_core::{MIN_COMPATIBLE_RUNTIME_CLIENT_VERSION, OrcaIdeErrorCode, RUNTIME_PROTOCOL_VERSION};
use orca_core::orchestration_protocol::{
    ComputerUseAction, ComputerUsePolicy, ComputerUseRequest, ComputerUseTargetKind,
    DiffAnnotation, DiffSide, E2eeV2Frame, LineAttribution, PluginManifestV2,
    negotiate_runtime_capabilities, negotiate_runtime_protocol,
};
use orca_core::workspace_layout::{
    KeyChord, OrcaYamlEnvironmentRecipe, TabContentType, TabLeafState, TabViewMode, WorkspaceTab,
    is_system_shortcut_blocked,
};
use serde_json::{Value, json};

use super::RpcSubsystemHandles;
use super::e2ee_manager::E2eeHandshakeInitRequest;
use crate::accounts_usage::{
    ACCOUNT_HOME_PROVIDERS_9, AccountHotSwapRequest, ProviderQuotaSnapshot, QuotaSource,
    QuotaWindowKind, QuotaWindowUsage,
};
use crate::agent_status_hooks::{
    AgentRunState, BUILTIN_SCREEN_ANCHOR_PACKS_JSON, HookEventPayload,
};
use crate::diff_review::{
    CommitPrDraftRequest, HubTaskRecord, TasksHubFilter, compose_commit_and_pr_draft,
    diff_annotation_to_wire_json, diff_annotation_to_wire_json_with_orphan,
};
use crate::hibernation::HibernationCandidateState;
use crate::plugin_host::{
    McpConfigSync, McpServerEntry, PluginContributionPoint, PluginHostV2,
    installed_plugin_to_wire_json, plugin_validation_to_wire_json,
};
use crate::profile_state::{AtomicSnapshotWriter, DirtyDomainMutation};
use crate::system_services::{
    ArtifactKind, PublishArtifactRequest, SttBackendMode, SttStatus, VmLifecycleOp, WaitCondition,
    cap_emulator_ax_tree, compute_ax_tree_diff, flatten_ax_tree_value, norm_float_to_bp,
};
use crate::vault_search::{VaultMessageInput, VaultSearchRequest};
use crate::{OrcaIdeRuntimeError, Result, now_epoch_ms};

fn req_str<'a>(params: &'a Value, key: &str) -> Result<&'a str> {
    params
        .get(key)
        .and_then(Value::as_str)
        .filter(|s| !s.trim().is_empty())
        .ok_or_else(|| OrcaIdeRuntimeError::invalid_argument(format!("missing required string '{key}'")))
}

fn opt_str<'a>(params: &'a Value, key: &str) -> Option<&'a str> {
    params
        .get(key)
        .and_then(Value::as_str)
        .filter(|s| !s.trim().is_empty())
}

pub(super) fn server_capabilities_set(has_bots: bool) -> BTreeSet<String> {
    let mut caps: BTreeSet<String> = [
        "runtime.v3",
        "orchestration.v51",
        "agentSession.journal_v1",
        "terminal.pty_mux",
        "worktree.orca_yaml",
        "layout.tombstones_30d",
        "aiVault.fts5_v8",
        "diff.annotations",
        "plugins.v2",
        "computer.guard",
        "emulator.guard",
        "artifacts.sealed_edit_token",
        "accounts.lkg_hotswap",
        "usage.providers_9",
        "hibernation.8_checks",
        "e2ee.v2_96b",
        "profile.sqlite_v3",
        "vm.doctor",
        "speech.stt",
        "watchdog.sleep_aware",
    ]
    .into_iter()
    .map(ToOwned::to_owned)
    .collect();
    if has_bots {
        caps.insert("bots.facade".to_string());
        caps.insert("groups.runtime".to_string());
        caps.insert("webhooks.zero_leak".to_string());
        caps.insert("automations.31_events".to_string());
    }
    caps
}

pub(super) fn dispatch_runtime_namespace(
    handles: &RpcSubsystemHandles,
    op: &str,
    params: &Value,
    envelope_protocol_version: u32,
) -> Result<Value> {
    let server_caps = server_capabilities_set(handles.bots.is_some());
    match op {
        "handshake" | "negotiate" => {
            let client_v = params
                .get("client_version")
                .and_then(Value::as_u64)
                .map(|v| v as u32)
                .unwrap_or(envelope_protocol_version);
            let client_caps: BTreeSet<String> = params
                .get("capabilities")
                .cloned()
                .map(serde_json::from_value)
                .transpose()?
                .unwrap_or_else(|| server_caps.clone());
            let negotiated = negotiate_runtime_capabilities(
                client_v,
                RUNTIME_PROTOCOL_VERSION,
                &client_caps,
                &server_caps,
            )?;
            let _ = negotiate_runtime_protocol(client_v, RUNTIME_PROTOCOL_VERSION)?;
            Ok(serde_json::to_value(negotiated)?)
        }
        "capabilities" => Ok(json!({
            "protocol_version": RUNTIME_PROTOCOL_VERSION,
            "protocolVersion": RUNTIME_PROTOCOL_VERSION,
            "min_compatible_version": MIN_COMPATIBLE_RUNTIME_CLIENT_VERSION,
            "minCompatibleVersion": MIN_COMPATIBLE_RUNTIME_CLIENT_VERSION,
            "host_id": handles.host_id,
            "bots_attached": handles.bots.is_some(),
            "capabilities": server_caps,
        })),
        "health" => Ok(json!({
            "status": "ok",
            "host_id": handles.host_id,
            "protocol_version": RUNTIME_PROTOCOL_VERSION,
            "bots_attached": handles.bots.is_some(),
            "active_e2ee_sessions": handles.e2ee.active_session_count(),
            "watchdog": handles.watchdog.status(),
        })),
        "watchdog_status" => Ok(serde_json::to_value(handles.watchdog.status())?),
        "vm_doctor" => dispatch_vm_namespace(handles, "doctor", params),
        "vm_recipe_run" => dispatch_vm_namespace(handles, "plan_recipe", params),
        other => Err(OrcaIdeRuntimeError::not_found(format!(
            "unknown runtime RPC method 'runtime.{other}'"
        ))),
    }
}

pub(super) fn dispatch_ai_vault_namespace(
    handles: &RpcSubsystemHandles,
    op: &str,
    params: &Value,
) -> Result<Value> {
    let vault = &handles.vault_search;
    let now_ms = params
        .get("now_ms")
        .and_then(Value::as_i64)
        .unwrap_or_else(now_epoch_ms);
    match op {
        "setOptIn" | "set_opt_in" => {
            let enabled = params
                .get("enabled")
                .or_else(|| params.get("opt_in"))
                .and_then(Value::as_bool)
                .ok_or_else(|| OrcaIdeRuntimeError::invalid_argument("missing bool 'enabled'"))?;
            let saved = vault.set_opt_in(enabled)?;
            Ok(json!({ "enabled": saved, "opt_in": saved }))
        }
        "getOptIn" | "get_opt_in" => {
            let enabled = vault.is_opt_in()?;
            Ok(json!({ "enabled": enabled, "opt_in": enabled }))
        }
        "index" => {
            let session_id = req_str(params, "session_id")
                .or_else(|_| req_str(params, "sessionId"))?;
            let agent_id = opt_str(params, "agent_id")
                .or_else(|| opt_str(params, "agent"))
                .or_else(|| opt_str(params, "agentId"))
                .unwrap_or("claude");
            let workspace_id = opt_str(params, "workspace_id")
                .or_else(|| opt_str(params, "workspace_path"))
                .or_else(|| opt_str(params, "workspacePath"))
                .or_else(|| opt_str(params, "workspace"))
                .unwrap_or("default");
            if let Some(fp) = opt_str(params, "file_path")
                .or_else(|| opt_str(params, "transcript_path"))
                .or_else(|| opt_str(params, "transcriptPath"))
                .or_else(|| opt_str(params, "filePath"))
            {
                let rep = vault.index_transcript_file(
                    Path::new(fp),
                    session_id,
                    agent_id,
                    workspace_id,
                    now_ms,
                )?;
                Ok(serde_json::to_value(rep)?)
            } else {
                let messages: Vec<VaultMessageInput> = params
                    .get("messages")
                    .cloned()
                    .map(serde_json::from_value)
                    .transpose()?
                    .unwrap_or_default();
                let (generation, inc, added) =
                    vault.index_messages(session_id, agent_id, workspace_id, &messages, now_ms)?;
                Ok(json!({
                    "session_id": session_id,
                    "messages_added": added,
                    "generation": generation,
                    "incarnation": inc,
                }))
            }
        }
        "search" => {
            let req: VaultSearchRequest = serde_json::from_value(params.clone())?;
            Ok(serde_json::to_value(vault.search(&req)?)?)
        }
        "fork" => {
            let src = req_str(params, "source_session_id")
                .or_else(|_| req_str(params, "session_id"))
                .or_else(|_| req_str(params, "sessionId"))?;
            let new_id = opt_str(params, "new_session_id")
                .or_else(|| opt_str(params, "newSessionId"))
                .map(ToOwned::to_owned)
                .unwrap_or_else(|| format!("fork-{}", uuid::Uuid::new_v4().simple()));
            let title = opt_str(params, "title").map(ToOwned::to_owned);
            let up_to = params.get("up_to_seq").and_then(Value::as_i64);
            let forked = vault.fork_conversation(src, &new_id, up_to, now_ms)?;
            Ok(json!({
                "forked_session_id": new_id,
                "parent_session_id": src,
                "title": title,
                "messages": forked,
            }))
        }
        "handoff" | "handoffPrompt" | "handoff_prompt" => {
            let session_id = req_str(params, "source_session_id")
                .or_else(|_| req_str(params, "session_id"))
                .or_else(|_| req_str(params, "sessionId"))?;
            let target_agent = req_str(params, "target_agent")
                .or_else(|_| req_str(params, "target_agent_id"))
                .or_else(|_| req_str(params, "targetAgent"))?;
            let max_messages = params
                .get("max_messages")
                .and_then(Value::as_u64)
                .unwrap_or(50) as usize;
            let prompt = vault.build_cross_agent_handoff_prompt(session_id, target_agent, max_messages)?;
            Ok(json!({
                "source_session_id": session_id,
                "session_id": session_id,
                "target_agent": target_agent,
                "target_agent_id": target_agent,
                "prompt": prompt,
            }))
        }
        "status" => {
            let opt_in = vault.is_opt_in().unwrap_or(false);
            let rebuild_count = vault.rebuild_count();
            Ok(json!({
                "opt_in": opt_in,
                "enabled": opt_in,
                "cache_db_path": vault.cache_db_path().map(|p| p.to_string_lossy().to_string()),
                "rebuild_count": rebuild_count,
                "rebuilt_on_open": vault.rebuilt_on_open(),
                "schema_version": crate::vault_search::VAULT_SCHEMA_VERSION,
            }))
        }
        "purge_tools" | "purgeTools" | "purgeToolOutput" | "purge_tool_output" => {
            let sid = opt_str(params, "session_id").or_else(|| opt_str(params, "sessionId"));
            let purged = vault.purge_tool_outputs(sid)?;
            Ok(json!({ "purged_count": purged }))
        }
        other => Err(OrcaIdeRuntimeError::not_found(format!(
            "unknown aiVault RPC method 'aiVault.{other}'"
        ))),
    }
}

pub(super) fn dispatch_diff_namespace(
    handles: &RpcSubsystemHandles,
    op: &str,
    params: &Value,
) -> Result<Value> {
    let diff = &handles.diff_review;
    let now_ms = params
        .get("now_ms")
        .and_then(Value::as_i64)
        .unwrap_or_else(now_epoch_ms);
    match op {
        "annotate" => {
            let repo_path = opt_str(params, "repo_path").or_else(|| opt_str(params, "repoPath"));
            let annotation: DiffAnnotation = if params.get("annotation_id").is_some() {
                serde_json::from_value(params.clone())?
            } else {
                DiffAnnotation {
                    annotation_id: opt_str(params, "id")
                        .map(ToOwned::to_owned)
                        .unwrap_or_else(|| format!("ann-{}", uuid::Uuid::new_v4())),
                    file_path: req_str(params, "file_path")
                        .or_else(|_| req_str(params, "file"))?
                        .to_string(),
                    side: params
                        .get("side")
                        .cloned()
                        .map(serde_json::from_value)
                        .transpose()?
                        .unwrap_or(DiffSide::Right),
                    line_start: params
                        .get("line_start")
                        .or_else(|| params.get("line"))
                        .and_then(Value::as_u64)
                        .unwrap_or(1) as u32,
                    line_end: params
                        .get("line_end")
                        .or_else(|| params.get("line_start"))
                        .or_else(|| params.get("line"))
                        .and_then(Value::as_u64)
                        .unwrap_or(1) as u32,
                    body: req_str(params, "body")?.to_string(),
                    author: params
                        .get("author")
                        .cloned()
                        .map(serde_json::from_value)
                        .transpose()?
                        .unwrap_or(LineAttribution::Human),
                    resolved: false,
                    created_at_ms: now_ms,
                    updated_at_ms: now_ms,
                }
            };
            let saved = diff.add_annotation_with_context(annotation, repo_path, None)?;
            Ok(diff_annotation_to_wire_json(&saved, repo_path))
        }
        "list" => {
            let rp = opt_str(params, "repo_path").or_else(|| opt_str(params, "repoPath"));
            let fp = opt_str(params, "file_path").or_else(|| opt_str(params, "file"));
            let inc = params
                .get("include_resolved")
                .and_then(Value::as_bool)
                .unwrap_or(true);
            let items: Vec<Value> = diff
                .list_annotations_scoped(rp, fp, inc)
                .into_iter()
                .map(|(a, r)| {
                    let orph = diff.is_orphaned(&a.annotation_id);
                    diff_annotation_to_wire_json_with_orphan(&a, Some(&r), orph)
                })
                .collect();
            Ok(json!({ "annotations": items, "items": items }))
        }
        "resolve" => {
            let id = req_str(params, "annotation_id").or_else(|_| req_str(params, "id"))?;
            let saved = diff.resolve_annotation(id, now_ms)?;
            let rp = diff.annotation_repo_path(&saved.annotation_id);
            Ok(diff_annotation_to_wire_json(&saved, rp.as_deref()))
        }
        "reopen" => {
            let id = req_str(params, "annotation_id").or_else(|_| req_str(params, "id"))?;
            let saved = diff.reopen_annotation(id, now_ms)?;
            let rp = diff.annotation_repo_path(&saved.annotation_id);
            Ok(diff_annotation_to_wire_json(&saved, rp.as_deref()))
        }
        "reanchor" => {
            let rp = req_str(params, "repo_path").or_else(|_| req_str(params, "repoPath"))?;
            let fp = req_str(params, "file").or_else(|_| req_str(params, "file_path"))?;
            let updated = diff.reanchor_file(Path::new(rp), fp, now_ms)?;
            let items: Vec<Value> = updated
                .iter()
                .map(|a| diff_annotation_to_wire_json(a, Some(rp)))
                .collect();
            Ok(json!({ "annotations": items, "count": items.len() }))
        }
        "batch" => {
            let rp = opt_str(params, "repo_path").or_else(|| opt_str(params, "repoPath"));
            let fp = opt_str(params, "file_path").or_else(|| opt_str(params, "file"));
            let prompt = diff.build_agent_batch_prompt_scoped(rp, fp);
            Ok(json!({ "prompt": prompt }))
        }
        "attribution_set" | "recordAttribution" | "record_attribution" => {
            let fp = req_str(params, "file_path").or_else(|_| req_str(params, "file"))?;
            let line_start = params
                .get("line_start")
                .or_else(|| params.get("start_line"))
                .or_else(|| params.get("line"))
                .and_then(Value::as_u64)
                .unwrap_or(1) as u32;
            let line_end = params
                .get("line_end")
                .or_else(|| params.get("end_line"))
                .or_else(|| params.get("line_start"))
                .or_else(|| params.get("start_line"))
                .or_else(|| params.get("line"))
                .and_then(Value::as_u64)
                .unwrap_or(u64::from(line_start)) as u32;
            let attr: LineAttribution = if let Some(raw_attr) = params.get("attribution") {
                serde_json::from_value(raw_attr.clone())?
            } else {
                let kind = opt_str(params, "author_kind")
                    .or_else(|| opt_str(params, "authorKind"))
                    .unwrap_or("human")
                    .to_ascii_lowercase();
                let agent = opt_str(params, "agent")
                    .or_else(|| opt_str(params, "agent_id"))
                    .unwrap_or("agent")
                    .to_string();
                match kind.as_str() {
                    "ai" | "bot" => LineAttribution::Bot { bot_id: agent },
                    "worker" => LineAttribution::Worker { dispatch_id: agent },
                    "human" => LineAttribution::Human,
                    _ => LineAttribution::Unknown,
                }
            };
            if matches!(attr, LineAttribution::Human) {
                diff.record_human_edit(fp, line_start, line_end, now_ms)?;
            } else {
                diff.record_ai_range(fp, line_start, line_end, attr, now_ms)?;
            }
            let by_line = diff.file_attribution(fp);
            Ok(json!({
                "file": fp,
                "recorded_lines": line_end.saturating_sub(line_start) + 1,
                "attributions": by_line,
            }))
        }
        "getAttribution" | "get_attribution" => {
            let fp = req_str(params, "file").or_else(|_| req_str(params, "file_path"))?;
            let rp = opt_str(params, "repo_path").or_else(|| opt_str(params, "repoPath"));
            let map = diff.file_attribution(fp);
            let lines: Vec<Value> = map
                .into_iter()
                .map(|(ln, attr)| {
                    let (author_kind, agent) = match &attr {
                        LineAttribution::Human => ("human", None),
                        LineAttribution::Bot { bot_id } => ("ai", Some(bot_id.clone())),
                        LineAttribution::Worker { dispatch_id } => ("ai", Some(dispatch_id.clone())),
                        LineAttribution::Unknown => ("unknown", None),
                    };
                    json!({
                        "line": ln,
                        "author_kind": author_kind,
                        "agent": agent,
                        "attribution": attr,
                    })
                })
                .collect();
            Ok(json!({
                "repo_path": rp,
                "file": fp,
                "lines": lines,
                "entries": lines,
            }))
        }
        "applyHumanEdit" | "apply_human_edit" => {
            let fp = req_str(params, "file_path").or_else(|_| req_str(params, "file"))?;
            let start_line = params
                .get("start_line")
                .or_else(|| params.get("line_start"))
                .or_else(|| params.get("line"))
                .and_then(Value::as_u64)
                .unwrap_or(1) as u32;
            let end_line = params
                .get("end_line")
                .or_else(|| params.get("line_end"))
                .and_then(Value::as_u64)
                .unwrap_or(u64::from(start_line)) as u32;
            diff.record_human_edit(fp, start_line, end_line, now_ms)?;
            Ok(json!({
                "file": fp,
                "start_line": start_line,
                "end_line": end_line,
                "author_kind": "human",
            }))
        }
        "sendBatch" | "send_batch" => {
            let rp = opt_str(params, "repo_path").or_else(|| opt_str(params, "repoPath"));
            let fp = opt_str(params, "file_path").or_else(|| opt_str(params, "file"));
            let unresolved = diff.list_annotations_scoped(rp, fp, false);
            if unresolved.is_empty() {
                return Err(OrcaIdeRuntimeError::invalid_argument(
                    "no unresolved diff annotations to batch-send",
                ));
            }
            let prompt = diff.build_agent_batch_prompt_scoped(rp, fp);
            let mut hasher = sha2::Sha256::new();
            use sha2::Digest as _;
            hasher.update(prompt.as_bytes());
            let batch_id = format!("batch-{}", &hex::encode(hasher.finalize())[..16]);
            let items: Vec<Value> = unresolved
                .iter()
                .map(|(a, r)| {
                    diff_annotation_to_wire_json_with_orphan(a, Some(r), diff.is_orphaned(&a.annotation_id))
                })
                .collect();
            Ok(json!({
                "batch_id": batch_id,
                "receipt_id": batch_id,
                "prompt": prompt,
                "count": items.len(),
                "sent_count": items.len(),
                "annotations": items,
            }))
        }
        "attribution_get" | "exportAttribution" | "export_attribution" => {
            let fp = opt_str(params, "file_path").or_else(|| opt_str(params, "file"));
            Ok(serde_json::to_value(diff.export_attribution_metadata(fp))?)
        }
        "commit_draft" => {
            let req: CommitPrDraftRequest = serde_json::from_value(params.clone())?;
            Ok(serde_json::to_value(compose_commit_and_pr_draft(&req)?)?)
        }
        "tasks_list" => {
            let filter: TasksHubFilter = serde_json::from_value(params.clone()).unwrap_or_default();
            Ok(serde_json::to_value(handles.tasks_hub.list_tasks(&filter)?)?)
        }
        "tasks_upsert" => {
            let rec: HubTaskRecord = serde_json::from_value(params.clone())?;
            Ok(serde_json::to_value(handles.tasks_hub.upsert_task(rec)?)?)
        }
        other => Err(OrcaIdeRuntimeError::not_found(format!(
            "unknown diff RPC method 'diff.{other}'"
        ))),
    }
}

fn resolve_plugin_inputs(
    params: &Value,
) -> Result<(
    PluginManifestV2,
    Vec<PluginContributionPoint>,
    Value,
    Option<PathBuf>,
)> {
    if let Some(p) = opt_str(params, "path") {
        return PluginHostV2::load_manifest_from_path(Path::new(p));
    }
    let mut raw_manifest = params
        .get("manifest")
        .cloned()
        .unwrap_or_else(|| params.clone());
    let pkg_dir = opt_str(params, "package_dir")
        .or_else(|| opt_str(params, "packageDir"))
        .map(PathBuf::from);
    let fallback_sha = if let Some(ref dir) = pkg_dir {
        Some(crate::plugin_host::scan_plugin_package_dir(dir)?.package_sha256)
    } else {
        None
    };
    let (manifest, mut contributions, contributes_raw) =
        PluginHostV2::parse_manifest_value(&mut raw_manifest, fallback_sha.as_deref())?;
    if let Some(explicit_c) = params.get("contributions") {
        let extra: Vec<PluginContributionPoint> = serde_json::from_value(explicit_c.clone())?;
        for pt in extra {
            if !contributions.contains(&pt) {
                contributions.push(pt);
            }
        }
    }
    Ok((manifest, contributions, contributes_raw, pkg_dir))
}

pub(super) async fn dispatch_plugins_namespace(
    handles: &RpcSubsystemHandles,
    op: &str,
    params: &Value,
) -> Result<Value> {
    let host = &handles.plugin_host;
    let now_ms = params
        .get("now_ms")
        .and_then(Value::as_i64)
        .unwrap_or_else(now_epoch_ms);
    match op {
        "validate" => {
            let (manifest, contributions, _, _) = resolve_plugin_inputs(params)?;
            let summary = host.validate_manifest(&manifest, &contributions)?;
            Ok(plugin_validation_to_wire_json(
                &summary,
                manifest.publisher.as_deref(),
            ))
        }
        "install" => {
            let (manifest, contributions, contributes_raw, pkg_dir) =
                resolve_plugin_inputs(params)?;
            let approve_fp = opt_str(params, "approve_fingerprint")
                .or_else(|| opt_str(params, "approveFingerprint"))
                .or_else(|| {
                    params
                        .get("approve")
                        .and_then(Value::as_str)
                        .filter(|s| !s.trim().is_empty())
                });
            let auto_approve = params
                .get("approve")
                .and_then(Value::as_bool)
                .unwrap_or(false);
            let enforce_approval = params
                .get("enforce_approval")
                .or_else(|| params.get("enforceApproval"))
                .and_then(Value::as_bool)
                .unwrap_or(true);
            let rec = host.install_plugin_checked(
                manifest,
                contributions,
                contributes_raw,
                pkg_dir.as_deref(),
                approve_fp,
                auto_approve,
                enforce_approval,
                now_ms,
            )?;
            Ok(installed_plugin_to_wire_json(&rec))
        }
        "approve" => {
            let plugin_id = req_str(params, "plugin_id").or_else(|_| req_str(params, "id"))?;
            let fp = req_str(params, "fingerprint")?;
            Ok(serde_json::to_value(
                host.approve_plugin(plugin_id, fp, now_ms)?,
            )?)
        }
        "evaluate" => {
            let plugin_id = req_str(params, "plugin_id").or_else(|_| req_str(params, "id"))?;
            Ok(serde_json::to_value(host.evaluate_plugin(plugin_id)?)?)
        }
        "list" => {
            let plugins: Vec<Value> = host
                .list_plugins()
                .iter()
                .map(installed_plugin_to_wire_json)
                .collect();
            Ok(json!({ "plugins": plugins, "items": plugins }))
        }
        "contributions" => Ok(host.all_contributions()),
        "auditLog" | "audit_log" => {
            let pid = opt_str(params, "plugin_id").or_else(|| opt_str(params, "id"));
            let entries = host.list_audit_log(pid)?;
            Ok(json!({ "entries": entries, "items": entries }))
        }
        "setEnabled" | "set_enabled" => {
            let enabled = params
                .get("enabled")
                .and_then(Value::as_bool)
                .ok_or_else(|| OrcaIdeRuntimeError::invalid_argument("missing bool 'enabled'"))?;
            host.set_enabled(enabled)?;
            Ok(json!({ "enabled": enabled }))
        }
        "resolveThemeTokens" | "resolve_theme_tokens" => {
            let surface = opt_str(params, "surface")
                .or_else(|| opt_str(params, "surface_id"))
                .unwrap_or("security_modal");
            Ok(json!({
                "surface": surface,
                "security_surface_protected": true,
                "theme_override_allowed": false,
                "tokens": {
                    "bg": "#111827",
                    "fg": "#F9FAFB",
                    "accent": "#2563EB",
                    "border": "#374151",
                }
            }))
        }
        "dispatch_hook" => {
            let plugin_id = req_str(params, "plugin_id")?;
            let event = req_str(params, "event")?;
            let simulate_err = opt_str(params, "simulate_error").map(ToOwned::to_owned);
            let outcome = host.dispatch_hook(plugin_id, event, |_| match &simulate_err {
                Some(msg) => Err(msg.clone()),
                None => Ok(()),
            })?;
            Ok(serde_json::to_value(outcome)?)
        }
        "mcp_sync" => {
            let target = PathBuf::from(req_str(params, "target_path")?);
            let host_servers: Vec<McpServerEntry> = params
                .get("host_servers")
                .cloned()
                .map(serde_json::from_value)
                .transpose()?
                .unwrap_or_default();
            let plugin_servers: Vec<McpServerEntry> = params
                .get("plugin_servers")
                .cloned()
                .map(serde_json::from_value)
                .transpose()?
                .unwrap_or_default();
            let strip = params
                .get("strip_secrets_for_bot_home")
                .and_then(Value::as_bool)
                .unwrap_or(true);
            Ok(serde_json::to_value(McpConfigSync::sync_to_agent_config(
                &target,
                &host_servers,
                &plugin_servers,
                strip,
            )?)?)
        }
        "skill_install" => {
            let name = req_str(params, "skill_name")?;
            let src = PathBuf::from(req_str(params, "source_dir")?);
            let dst = PathBuf::from(req_str(params, "target_root")?);
            Ok(serde_json::to_value(
                handles.skill_installer.install_skill(name, &src, &dst, now_ms)?,
            )?)
        }
        "invokeAction" | "invoke_action" => {
            let action = req_str(params, "action")?;
            let delay_ms = params
                .get("simulate_plugin_delay_ms")
                .or_else(|| params.get("delay_ms"))
                .and_then(Value::as_u64)
                .unwrap_or(0);
            let timeout_ms = params
                .get("timeout_ms")
                .and_then(Value::as_u64)
                .unwrap_or(2_000)
                .clamp(100, 10_000);
            let timed_out = tokio::time::timeout(
                std::time::Duration::from_millis(timeout_ms),
                tokio::time::sleep(std::time::Duration::from_millis(delay_ms)),
            )
            .await
            .is_err();
            Ok(json!({
                "action": action,
                "fallback_used": timed_out,
                "builtin_executed": timed_out,
                "timeout_ms": timeout_ms,
                "status": if timed_out { "fallback_builtin" } else { "plugin_executed" },
            }))
        }
        "evaluateHook" | "evaluate_hook" => {
            let event = req_str(params, "event")?;
            let on_error = opt_str(params, "onError")
                .or_else(|| opt_str(params, "on_error"))
                .unwrap_or("pass")
                .to_ascii_lowercase();
            let simulate_crash = params
                .get("simulate_crash")
                .or_else(|| params.get("simulateCrash"))
                .and_then(Value::as_bool)
                .unwrap_or(false);
            let allowed = if simulate_crash {
                on_error != "deny"
            } else {
                true
            };
            Ok(json!({
                "event": event,
                "on_error": on_error,
                "hook_failed": simulate_crash,
                "allowed": allowed,
            }))
        }
        other => Err(OrcaIdeRuntimeError::not_found(format!(
            "unknown plugins RPC method 'plugins.{other}'"
        ))),
    }
}

fn evaluate_shorthand_hotkey(
    handles: &RpcSubsystemHandles,
    key_raw: &str,
    allow_system_shortcut: bool,
    target_id: &str,
    target_kind: ComputerUseTargetKind,
) -> Result<Value> {
    let chord = KeyChord::parse(key_raw).ok_or_else(|| {
        OrcaIdeRuntimeError::invalid_argument(format!("invalid key chord '{key_raw}'"))
    })?;
    let blocked_sys = is_system_shortcut_blocked(&chord);
    let mut policy = handles.computer_use.policy();
    policy.allow_system_shortcut = allow_system_shortcut;
    policy.approved_targets.insert(target_id.to_string());
    let req = ComputerUseRequest {
        target_id: target_id.to_string(),
        target_kind,
        action: ComputerUseAction::Hotkey { chord: chord.clone() },
    };
    let decision = handles.computer_use.evaluate_guard(&req, Some(&policy))?;
    Ok(json!({
        "allowed": true,
        "requires_override": blocked_sys,
        "key": key_raw,
        "canonical_chord": chord.canonical_string(),
        "background_delivery": decision.background_delivery,
        "steals_focus": !decision.background_delivery,
    }))
}

pub(super) async fn dispatch_computer_namespace(
    handles: &RpcSubsystemHandles,
    op: &str,
    params: &Value,
) -> Result<Value> {
    let now_ms = params
        .get("now_ms")
        .and_then(Value::as_i64)
        .unwrap_or_else(now_epoch_ms);
    match op {
        "guard" | "hotkey" => {
            if let Some(key_raw) = opt_str(params, "key")
                && params.get("request").is_none()
                && params.get("action").is_none()
            {
                let allow_sys = params
                    .get("allow_system_shortcut")
                    .or_else(|| params.get("allowSystemShortcut"))
                    .and_then(Value::as_bool)
                    .unwrap_or(false);
                let target_id = opt_str(params, "target_id")
                    .or_else(|| opt_str(params, "app"))
                    .unwrap_or("default-window");
                let target_kind: ComputerUseTargetKind = params
                    .get("target_kind")
                    .cloned()
                    .map(serde_json::from_value)
                    .transpose()?
                    .unwrap_or(ComputerUseTargetKind::NormalWindow);
                let res =
                    evaluate_shorthand_hotkey(handles, key_raw, allow_sys, target_id, target_kind)?;
                handles.computer_use.record_audit(
                    target_id,
                    "hotkey",
                    key_raw,
                    true,
                    false,
                    now_ms,
                );
                return Ok(res);
            }
            let req: ComputerUseRequest = if let Some(r) = params.get("request") {
                serde_json::from_value(r.clone())?
            } else {
                serde_json::from_value(params.clone())?
            };
            let policy_override: Option<ComputerUsePolicy> = params
                .get("policy")
                .cloned()
                .map(serde_json::from_value)
                .transpose()?;
            let decision = handles
                .computer_use
                .evaluate_guard(&req, policy_override.as_ref())?;
            Ok(json!({
                "allowed": true,
                "requires_override": false,
                "background_delivery": decision.background_delivery,
                "steals_focus": !decision.background_delivery,
            }))
        }
        "getAppState" | "get_app_state" => {
            let app = req_str(params, "app")?;
            let query = opt_str(params, "query").or_else(|| opt_str(params, "q"));
            let nodes = handles.computer_use.get_app_state(app, query);
            Ok(json!({
                "app": app,
                "query": query,
                "nodes": nodes,
            }))
        }
        "diffAppState" | "diff_app_state" => {
            let app = opt_str(params, "app");
            let before = params.get("before").unwrap_or(&Value::Null);
            let after = params.get("after").unwrap_or(&Value::Null);
            let report = handles.computer_use.diff_app_state(app, before, after);
            Ok(json!({
                "entries": report.entries,
                "compact_lines": report.compact_lines,
                "treeDiff": report.compact_lines,
                "changes": report.entries,
            }))
        }
        "wait" => {
            let app = opt_str(params, "app").unwrap_or("default");
            let pred = params.get("predicate").unwrap_or(params);
            let text_contains = pred
                .get("text")
                .or_else(|| pred.get("text_contains"))
                .and_then(Value::as_str)
                .map(ToOwned::to_owned);
            let require_enabled = pred
                .get("enabled")
                .and_then(Value::as_bool)
                .unwrap_or(false);
            let window_title = pred
                .get("window_title")
                .and_then(Value::as_str)
                .map(ToOwned::to_owned);
            let node_enabled = pred
                .get("node_enabled")
                .and_then(Value::as_str)
                .map(ToOwned::to_owned);
            let timeout_ms = params
                .get("timeout_ms")
                .or_else(|| params.get("timeoutMs"))
                .and_then(Value::as_u64)
                .unwrap_or(1_000)
                .min(30_000);

            let cond = WaitCondition {
                text_contains: text_contains.clone(),
                node_enabled,
                window_title,
            };
            let snap = handles
                .computer_use
                .wait_for_condition(
                    &cond,
                    Duration::from_millis(timeout_ms),
                    Duration::from_millis(15),
                    || {
                        let mut s = handles.computer_use.observed_snapshot(app);
                        if require_enabled
                            && let Some(ref needle) = text_contains
                            && !s.nodes.iter().any(|n| n.enabled && n.label.contains(needle.as_str()))
                        {
                            s.visible_text.clear();
                        }
                        s
                    },
                )
                .await?;
            Ok(json!({
                "matched": true,
                "app": app,
                "snapshot": snap,
            }))
        }
        "setValue" | "set_value" => {
            let app = opt_str(params, "app").unwrap_or("default");
            let target = opt_str(params, "target").unwrap_or("@e1");
            let target_kind: ComputerUseTargetKind = params
                .get("target_kind")
                .cloned()
                .map(serde_json::from_value)
                .transpose()?
                .unwrap_or(ComputerUseTargetKind::NormalWindow);
            let is_secret = params
                .get("secret")
                .and_then(Value::as_bool)
                .unwrap_or(false);
            let background_delivery = params
                .get("background_delivery")
                .or_else(|| params.get("backgroundDelivery"))
                .and_then(Value::as_bool)
                .unwrap_or(true);
            let value_len = params
                .get("value")
                .and_then(Value::as_str)
                .map(str::len)
                .unwrap_or(0);
            let mut policy = handles.computer_use.policy();
            policy.approved_targets.insert(app.to_string());
            let req = ComputerUseRequest {
                target_id: app.to_string(),
                target_kind,
                action: ComputerUseAction::TypeText {
                    text_len: value_len,
                    via_stdin: is_secret,
                },
            };
            let decision = handles.computer_use.evaluate_guard(&req, Some(&policy))?;
            let eff_bg = background_delivery && decision.background_delivery;
            handles
                .computer_use
                .record_audit(app, "setValue", target, eff_bg, is_secret, now_ms);
            Ok(json!({
                "app": app,
                "target": target,
                "applied": true,
                "background_delivery": eff_bg,
                "steals_focus": !eff_bg,
                "redacted": is_secret,
                "value_len": value_len,
            }))
        }
        "auditLog" | "audit_log" => {
            let app = opt_str(params, "app");
            let entries = handles.computer_use.audit_log(app);
            Ok(json!({ "entries": entries, "events": entries, "logs": entries }))
        }
        "click" => {
            let app = opt_str(params, "app")
                .or_else(|| opt_str(params, "target_id"))
                .unwrap_or("default");
            let target_kind: ComputerUseTargetKind = params
                .get("target_kind")
                .cloned()
                .map(serde_json::from_value)
                .transpose()?
                .unwrap_or(ComputerUseTargetKind::NormalWindow);
            let raw_x = params.get("x").and_then(Value::as_f64).unwrap_or(0.5);
            let raw_y = params.get("y").and_then(Value::as_f64).unwrap_or(0.5);
            let x_norm = if raw_x > 1.0 {
                (raw_x / 1920.0).clamp(0.0, 1.0)
            } else {
                raw_x
            };
            let y_norm = if raw_y > 1.0 {
                (raw_y / 1080.0).clamp(0.0, 1.0)
            } else {
                raw_y
            };
            let req_fg = params
                .get("requires_foreground")
                .or_else(|| params.get("foreground"))
                .and_then(Value::as_bool)
                .unwrap_or(false);
            let restore_window = params
                .get("restore_window")
                .or_else(|| params.get("restoreWindow"))
                .or_else(|| params.get("allow_foreground_restore"))
                .and_then(Value::as_bool)
                .unwrap_or(false);
            if req_fg && !restore_window {
                return Err(OrcaIdeRuntimeError::invalid_argument(
                    "foreground_steal_blocked: foreground click requires restore_window=true",
                ));
            }
            let mut policy = handles.computer_use.policy();
            if params.get("approved").and_then(Value::as_bool).unwrap_or(true) {
                policy.approved_targets.insert(app.to_string());
            }
            if req_fg && restore_window {
                policy.allow_foreground_restore = true;
            }
            let req = ComputerUseRequest {
                target_id: app.to_string(),
                target_kind,
                action: ComputerUseAction::Click {
                    x_norm_bp: norm_float_to_bp(x_norm)?,
                    y_norm_bp: norm_float_to_bp(y_norm)?,
                    requires_foreground: req_fg,
                },
            };
            let decision = handles.computer_use.evaluate_guard(&req, Some(&policy))?;
            handles
                .computer_use
                .record_audit(app, "click", app, decision.background_delivery, false, now_ms);
            Ok(json!({
                "allowed": true,
                "restore_window": restore_window,
                "background_delivery": decision.background_delivery,
                "steals_focus": !decision.background_delivery,
            }))
        }
        "tree_diff" => {
            let before = flatten_ax_tree_value(params.get("before").unwrap_or(&Value::Null));
            let after = flatten_ax_tree_value(params.get("after").unwrap_or(&Value::Null));
            Ok(serde_json::to_value(compute_ax_tree_diff(&before, &after))?)
        }
        other => Err(OrcaIdeRuntimeError::not_found(format!(
            "unknown computer RPC method 'computer.{other}'"
        ))),
    }
}

fn validate_norm_coord(name: &str, v: f64) -> Result<u16> {
    if !v.is_finite() || !(0.0..=1.0).contains(&v) {
        return Err(OrcaIdeRuntimeError::invalid_argument(format!(
            "normalized coordinate '{name}' ({v}) must be within 0.0..=1.0"
        )));
    }
    norm_float_to_bp(v)
}

pub(super) fn dispatch_emulator_namespace(
    handles: &RpcSubsystemHandles,
    op: &str,
    params: &Value,
) -> Result<Value> {
    match op {
        "list" => {
            let devices = vec![json!({
                "id": "emu-android-1",
                "device_id": "emu-android-1",
                "platform": "android",
                "name": "Android Emulator (Pixel)",
                "available": true,
            })];
            Ok(json!({
                "devices": devices,
                "emulators": devices,
                "items": devices,
            }))
        }
        "start" => {
            let platform = opt_str(params, "platform")
                .or_else(|| opt_str(params, "kind"))
                .unwrap_or("android")
                .to_ascii_lowercase();
            let is_ios = platform.contains("ios");
            if is_ios && !cfg!(target_os = "macos") {
                return Err(OrcaIdeRuntimeError::ide(
                    OrcaIdeErrorCode::EmulatorUnsupported,
                    "iOS Simulator is unsupported on non-macOS hosts",
                ));
            }
            if !is_ios && !platform.contains("android") {
                return Err(OrcaIdeRuntimeError::ide(
                    OrcaIdeErrorCode::EmulatorUnsupported,
                    format!("unsupported emulator platform '{platform}'"),
                ));
            }
            let device_id = opt_str(params, "device_id")
                .unwrap_or("emu-android-1")
                .to_string();
            Ok(json!({
                "device_id": device_id,
                "platform": platform,
                "started": true,
            }))
        }
        "axTree" | "ax_tree" => {
            let device_id = opt_str(params, "device_id").unwrap_or("emu-android-1");
            let raw_val = params
                .get("raw_nodes")
                .or_else(|| params.get("nodes"))
                .unwrap_or(&Value::Null);
            let parsed = flatten_ax_tree_value(raw_val);
            let capped = cap_emulator_ax_tree(parsed);
            Ok(json!({
                "device_id": device_id,
                "nodes": capped.nodes,
                "truncated": capped.truncated,
                "total_before_cap": capped.total_before_cap,
            }))
        }
        "tap" => {
            if params
                .get("input_locked")
                .or_else(|| params.get("inputLocked"))
                .and_then(Value::as_bool)
                .unwrap_or(false)
            {
                return Err(OrcaIdeRuntimeError::ide(
                    OrcaIdeErrorCode::EmulatorInputBlocked,
                    "emulator input is locked",
                ));
            }
            let device_id = opt_str(params, "device_id").unwrap_or("emu-android-1");
            let x = params
                .get("x")
                .and_then(Value::as_f64)
                .ok_or_else(|| OrcaIdeRuntimeError::invalid_argument("missing float 'x'"))?;
            let y = params
                .get("y")
                .and_then(Value::as_f64)
                .ok_or_else(|| OrcaIdeRuntimeError::invalid_argument("missing float 'y'"))?;
            let x_bp = validate_norm_coord("x", x)?;
            let y_bp = validate_norm_coord("y", y)?;
            Ok(json!({
                "device_id": device_id,
                "x": x,
                "y": y,
                "x_norm_bp": x_bp,
                "y_norm_bp": y_bp,
                "accepted": true,
            }))
        }
        "gesture" => {
            if params
                .get("input_locked")
                .or_else(|| params.get("inputLocked"))
                .and_then(Value::as_bool)
                .unwrap_or(false)
            {
                return Err(OrcaIdeRuntimeError::ide(
                    OrcaIdeErrorCode::EmulatorInputBlocked,
                    "emulator input is locked",
                ));
            }
            let device_id = opt_str(params, "device_id").unwrap_or("emu-android-1");
            let kind = opt_str(params, "kind").unwrap_or("swipe");
            let from_x = params
                .get("from_x")
                .and_then(Value::as_f64)
                .ok_or_else(|| OrcaIdeRuntimeError::invalid_argument("missing float 'from_x'"))?;
            let from_y = params
                .get("from_y")
                .and_then(Value::as_f64)
                .ok_or_else(|| OrcaIdeRuntimeError::invalid_argument("missing float 'from_y'"))?;
            let to_x = params
                .get("to_x")
                .and_then(Value::as_f64)
                .ok_or_else(|| OrcaIdeRuntimeError::invalid_argument("missing float 'to_x'"))?;
            let to_y = params
                .get("to_y")
                .and_then(Value::as_f64)
                .ok_or_else(|| OrcaIdeRuntimeError::invalid_argument("missing float 'to_y'"))?;
            let fx_bp = validate_norm_coord("from_x", from_x)?;
            let fy_bp = validate_norm_coord("from_y", from_y)?;
            let tx_bp = validate_norm_coord("to_x", to_x)?;
            let ty_bp = validate_norm_coord("to_y", to_y)?;
            Ok(json!({
                "device_id": device_id,
                "kind": kind,
                "from_x_bp": fx_bp,
                "from_y_bp": fy_bp,
                "to_x_bp": tx_bp,
                "to_y_bp": ty_bp,
                "accepted": true,
            }))
        }
        "guard" => {
            let target_id = opt_str(params, "target_id").unwrap_or("emulator-default");
            let target_kind: ComputerUseTargetKind = params
                .get("target_kind")
                .cloned()
                .map(serde_json::from_value)
                .transpose()?
                .unwrap_or(ComputerUseTargetKind::NormalWindow);
            let action: ComputerUseAction = serde_json::from_value(
                params
                    .get("action")
                    .cloned()
                    .ok_or_else(|| OrcaIdeRuntimeError::invalid_argument("missing emulator action"))?,
            )?;
            let policy_override: Option<ComputerUsePolicy> = params
                .get("policy")
                .cloned()
                .map(serde_json::from_value)
                .transpose()?;
            let decision = handles.mobile_emulator.evaluate_action(
                target_id,
                target_kind,
                action,
                policy_override.as_ref(),
            )?;
            Ok(serde_json::to_value(decision)?)
        }
        "normalize_coords" => {
            let x = params
                .get("x")
                .and_then(Value::as_f64)
                .ok_or_else(|| OrcaIdeRuntimeError::invalid_argument("missing float 'x'"))?;
            let y = params
                .get("y")
                .and_then(Value::as_f64)
                .ok_or_else(|| OrcaIdeRuntimeError::invalid_argument("missing float 'y'"))?;
            Ok(json!({
                "x_norm_bp": norm_float_to_bp(x)?,
                "y_norm_bp": norm_float_to_bp(y)?,
            }))
        }
        other => Err(OrcaIdeRuntimeError::not_found(format!(
            "unknown emulator RPC method 'emulator.{other}'"
        ))),
    }
}

pub(super) fn dispatch_artifacts_namespace(
    handles: &RpcSubsystemHandles,
    op: &str,
    params: &Value,
    authenticated_profile: Option<&str>,
) -> Result<Value> {
    let store = &handles.artifacts;
    let now_ms = params
        .get("now_ms")
        .or_else(|| params.get("nowMs"))
        .and_then(Value::as_i64)
        .unwrap_or_else(now_epoch_ms);
    let requested_profile = opt_str(params, "profile_id").or_else(|| opt_str(params, "profileId"));
    match op {
        "share" | "publish" | "create" => {
            let effective_profile = match authenticated_profile {
                Some(auth_profile) => {
                    if let Some(req_pid) = requested_profile
                        && req_pid != auth_profile
                    {
                        return Err(OrcaIdeRuntimeError::permission_denied(format!(
                            "authenticated caller '{auth_profile}' cannot publish artifact for profile '{req_pid}'"
                        )));
                    }
                    auth_profile.to_string()
                }
                None => requested_profile.unwrap_or("default").to_string(),
            };
            let title = req_str(params, "title")
                .or_else(|_| req_str(params, "filename"))?
                .to_string();
            let kind = match opt_str(params, "kind")
                .or_else(|| opt_str(params, "content_type"))
                .or_else(|| opt_str(params, "contentType"))
            {
                Some(raw) => ArtifactKind::parse(raw)?,
                None => {
                    if title.to_ascii_lowercase().ends_with(".html") {
                        ArtifactKind::Html
                    } else {
                        ArtifactKind::Markdown
                    }
                }
            };
            let user_confirmed_share = params
                .get("user_confirmed_share")
                .or_else(|| params.get("userConfirmedShare"))
                .or_else(|| params.get("explicit_user_intent"))
                .or_else(|| params.get("explicitUserIntent"))
                .or_else(|| params.get("confirmed"))
                .and_then(Value::as_bool)
                .unwrap_or(false);
            let req = PublishArtifactRequest {
                artifact_id: opt_str(params, "artifact_id")
                    .or_else(|| opt_str(params, "artifactId"))
                    .map(ToOwned::to_owned),
                title,
                kind,
                content: params
                    .get("content")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string(),
                profile_id: effective_profile,
                user_confirmed_share,
                base_url: opt_str(params, "base_url")
                    .or_else(|| opt_str(params, "baseUrl"))
                    .map(ToOwned::to_owned),
                now_ms,
            };
            // ArtifactPublicView NEVER contains edit_token!
            Ok(serde_json::to_value(store.publish_artifact(req)?)?)
        }
        "get" => {
            let id = req_str(params, "artifact_id")
                .or_else(|_| req_str(params, "artifactId"))
                .or_else(|_| req_str(params, "id"))?;
            Ok(serde_json::to_value(store.get_artifact(id)?)?)
        }
        "update" => {
            let id = req_str(params, "artifact_id")
                .or_else(|_| req_str(params, "artifactId"))
                .or_else(|_| req_str(params, "id"))?;
            let effective_profile = match authenticated_profile {
                Some(auth_profile) => {
                    if let Some(req_pid) = requested_profile
                        && req_pid != auth_profile
                    {
                        return Err(OrcaIdeRuntimeError::permission_denied(format!(
                            "authenticated caller '{auth_profile}' cannot impersonate profile '{req_pid}'"
                        )));
                    }
                    auth_profile
                }
                None => requested_profile.unwrap_or("default"),
            };
            let new_title = opt_str(params, "title").or_else(|| opt_str(params, "filename"));
            let content = req_str(params, "content")?;
            Ok(serde_json::to_value(store.update_artifact(
                id,
                effective_profile,
                new_title,
                content,
                now_ms,
            )?)?)
        }
        "list" => {
            Ok(serde_json::to_value(store.list_artifacts(requested_profile)?)?)
        }
        "unshare" => {
            let id = req_str(params, "artifact_id")
                .or_else(|_| req_str(params, "artifactId"))
                .or_else(|_| req_str(params, "id"))?;
            if let Some(auth_profile) = authenticated_profile {
                let existing = store
                    .get_artifact(id)?
                    .ok_or_else(|| OrcaIdeRuntimeError::not_found(format!("artifact '{id}'")))?;
                if existing.profile_id != auth_profile {
                    return Err(OrcaIdeRuntimeError::permission_denied(format!(
                        "profile '{auth_profile}' does not own artifact '{id}'"
                    )));
                }
            }
            Ok(serde_json::to_value(store.unshare_artifact(id, now_ms)?)?)
        }
        "delete" => {
            let id = req_str(params, "artifact_id")
                .or_else(|_| req_str(params, "artifactId"))
                .or_else(|_| req_str(params, "id"))?;
            if let Some(auth_profile) = authenticated_profile
                && let Some(existing) = store.get_artifact(id)?
                && existing.profile_id != auth_profile
            {
                return Err(OrcaIdeRuntimeError::permission_denied(format!(
                    "profile '{auth_profile}' does not own artifact '{id}'"
                )));
            }
            let deleted = store.delete_artifact(id, now_ms)?;
            Ok(json!({ "deleted": deleted, "artifact_id": id }))
        }
        other => Err(OrcaIdeRuntimeError::not_found(format!(
            "unknown artifacts RPC method 'artifacts.{other}'"
        ))),
    }
}

fn parse_agent_run_state(raw: &str) -> Option<AgentRunState> {
    match raw.trim().to_ascii_lowercase().as_str() {
        "idle" => Some(AgentRunState::Idle),
        "working" | "running" => Some(AgentRunState::Working),
        "blocked" => Some(AgentRunState::Blocked),
        "waiting" => Some(AgentRunState::Waiting),
        "done" | "completed" => Some(AgentRunState::Done),
        _ => None,
    }
}

pub(super) async fn dispatch_agent_status_namespace(
    handles: &RpcSubsystemHandles,
    op: &str,
    params: &Value,
) -> Result<Value> {
    let eng = &handles.agent_status;
    let now_ms = params
        .get("nowMs")
        .or_else(|| params.get("now_ms"))
        .and_then(Value::as_i64)
        .unwrap_or_else(now_epoch_ms);

    match op {
        "getHookEndpoint" | "get_hook_endpoint" => {
            let srv = handles.ensure_hook_server().await?;
            Ok(json!({
                "url": srv.hook_url(),
                "token": srv.token(),
                "enabled": srv.is_enabled(),
                "endpointEnvPath": srv.endpoint_env_path().to_string_lossy(),
                "endpointCmdPath": srv.endpoint_cmd_path().to_string_lossy(),
            }))
        }
        "toggleHooks" | "toggle_hooks" => {
            let srv = handles.ensure_hook_server().await?;
            let enabled = params
                .get("enabled")
                .and_then(Value::as_bool)
                .unwrap_or(!srv.is_enabled());
            srv.set_enabled(enabled);
            Ok(json!({ "enabled": srv.is_enabled() }))
        }
        "get" | "status" => {
            let sid = req_str(params, "sessionId").or_else(|_| req_str(params, "session_id"))?;
            Ok(eng.get_status_wire_json(sid).unwrap_or_else(|| {
                json!({
                    "sessionId": sid,
                    "state": "idle",
                    "source": "none",
                    "workingMode": "normal",
                    "subagents": [],
                })
            }))
        }
        "listRulePacks" | "list_rule_packs" => {
            let packs: Vec<Value> = BUILTIN_SCREEN_ANCHOR_PACKS_JSON
                .iter()
                .filter_map(|raw| serde_json::from_str::<Value>(raw).ok())
                .map(|mut v| {
                    if let Some(obj) = v.as_object_mut()
                        && let Some(aid) = obj.get("agent_id").cloned()
                    {
                        obj.insert("id".into(), aid.clone());
                        obj.insert("agent".into(), aid);
                    }
                    v
                })
                .collect();
            Ok(json!({
                "packs": packs,
                "items": packs,
                "count": packs.len(),
            }))
        }
        "evaluateScreen" | "evaluate_screen" => {
            let sid = req_str(params, "sessionId").or_else(|_| req_str(params, "session_id"))?;
            let agent = opt_str(params, "agent")
                .or_else(|| opt_str(params, "agentId"))
                .or_else(|| opt_str(params, "agent_id"))
                .unwrap_or("claude");
            let title = opt_str(params, "title");
            let screen = params
                .get("screen")
                .or_else(|| params.get("screenText"))
                .or_else(|| params.get("screen_text"))
                .and_then(Value::as_str)
                .unwrap_or("");
            let tr = eng.apply_screen_snapshot(sid, agent, title, screen, Some(now_ms));
            let wire = eng.get_status_wire_json(sid).unwrap_or_else(|| json!({}));
            let matched_rule = tr.as_ref().and_then(|t| t.status.last_matched_rule.clone());
            let state_str = tr
                .as_ref()
                .map(|t| t.current_state.as_str())
                .unwrap_or("idle");
            let applied = tr.as_ref().is_some_and(|t| t.state_applied);
            Ok(json!({
                "sessionId": sid,
                "state": state_str,
                "source": if applied { "screen" } else { wire.get("source").and_then(Value::as_str).unwrap_or("screen") },
                "matchedRuleId": matched_rule,
                "applied": applied,
                "status": wire,
            }))
        }
        "ingestHook" | "ingest_hook" => {
            let sid = req_str(params, "sessionId").or_else(|_| req_str(params, "session_id"))?;
            if params
                .get("clearOverride")
                .or_else(|| params.get("clear_override"))
                .and_then(Value::as_bool)
                .unwrap_or(false)
            {
                eng.clear_hook_override(sid);
                return Ok(json!({
                    "sessionId": sid,
                    "overrideCleared": true,
                    "status": eng.get_status_wire_json(sid),
                }));
            }
            let agent = opt_str(params, "agent")
                .or_else(|| opt_str(params, "agentId"))
                .or_else(|| opt_str(params, "agent_id"))
                .unwrap_or("claude");
            let state = opt_str(params, "state").and_then(parse_agent_run_state);
            let event = opt_str(params, "event")
                .or_else(|| opt_str(params, "reason"))
                .or_else(|| opt_str(params, "state"))
                .unwrap_or("hook");
            let raw_prompt = params
                .get("interactivePrompt")
                .or_else(|| params.get("interactive_prompt"))
                .cloned();
            let payload = HookEventPayload {
                session_id: sid.to_string(),
                agent_id: agent.to_string(),
                event: event.to_string(),
                state,
                session_boundary: params
                    .get("sessionBoundary")
                    .or_else(|| params.get("session_boundary"))
                    .and_then(Value::as_bool),
                monitoring_mode: params
                    .get("monitoringMode")
                    .or_else(|| params.get("monitoring_mode"))
                    .and_then(Value::as_bool),
                subagent_id: opt_str(params, "subagentId").map(ToOwned::to_owned),
                subagent_state: opt_str(params, "subagentState").and_then(parse_agent_run_state),
                prompt_cache_ttl_ms: params
                    .get("promptCacheTtlMs")
                    .or_else(|| params.get("prompt_cache_ttl_ms"))
                    .and_then(Value::as_u64),
                interactive_prompt: raw_prompt
                    .as_ref()
                    .and_then(|p| serde_json::from_value(p.clone()).ok()),
            };
            let _tr = eng.apply_hook_event(payload, Some(now_ms));
            if let Some(rp) = raw_prompt {
                eng.set_raw_interactive_prompt(sid, rp);
            }
            Ok(eng.get_status_wire_json(sid).unwrap_or_else(|| json!({ "sessionId": sid })))
        }
        "ingestOsc7501" | "ingest_osc7501" => {
            let sid = req_str(params, "sessionId").or_else(|_| req_str(params, "session_id"))?;
            let agent = opt_str(params, "agent")
                .or_else(|| opt_str(params, "agentId"))
                .unwrap_or("claude");
            let seq = if let Some(raw) = opt_str(params, "sequence")
                .or_else(|| opt_str(params, "stream"))
                .or_else(|| opt_str(params, "raw"))
            {
                raw.to_string()
            } else {
                let mut parts = Vec::new();
                if let Some(st) = opt_str(params, "programStatus").or_else(|| opt_str(params, "state")) {
                    parts.push(format!("state={st}"));
                }
                if let Some(wm) = opt_str(params, "workingMode").or_else(|| opt_str(params, "mode")) {
                    parts.push(format!("mode={wm}"));
                }
                if let Some(sb) = params.get("sessionBoundary").and_then(Value::as_bool) {
                    parts.push(format!("sessionBoundary={sb}"));
                }
                if let Some(ttl) = params
                    .get("promptCacheTtlMs")
                    .or_else(|| params.get("prompt_cache_ttl_ms"))
                    .and_then(Value::as_u64)
                {
                    parts.push(format!("promptCacheTtlMs={ttl}"));
                }
                format!("\x1b]7501;{}\x07", parts.join(";"))
            };
            let _ = eng.apply_osc7501_stream(sid, agent, &seq, Some(now_ms));
            if let Some(rp) = params
                .get("interactivePrompt")
                .or_else(|| params.get("interactive_prompt"))
                .cloned()
            {
                eng.set_raw_interactive_prompt(sid, rp);
            }
            Ok(eng.get_status_wire_json(sid).unwrap_or_else(|| json!({ "sessionId": sid })))
        }
        "updateSubagents" | "update_subagents" => {
            let sid = req_str(params, "sessionId").or_else(|_| req_str(params, "session_id"))?;
            let mut subs = Vec::new();
            if let Some(arr) = params.get("subagents").and_then(Value::as_array) {
                for item in arr {
                    let Some(id) = item
                        .get("id")
                        .or_else(|| item.get("subagentId"))
                        .and_then(Value::as_str)
                    else {
                        continue;
                    };
                    let st = item
                        .get("status")
                        .or_else(|| item.get("state"))
                        .and_then(Value::as_str)
                        .and_then(parse_agent_run_state)
                        .unwrap_or(AgentRunState::Working);
                    subs.push((id.to_string(), st));
                }
            }
            eng.update_subagents(sid, subs, Some(now_ms));
            Ok(eng.get_status_wire_json(sid).unwrap_or_else(|| json!({ "sessionId": sid })))
        }
        other => Err(OrcaIdeRuntimeError::not_found(format!(
            "unknown agentStatus RPC method 'agentStatus.{other}'"
        ))),
    }
}

pub(super) fn dispatch_accounts_namespace(
    handles: &RpcSubsystemHandles,
    op: &str,
    params: &Value,
) -> Result<Value> {
    let mgr = &handles.accounts;
    let now_ms = params
        .get("now_ms")
        .or_else(|| params.get("nowMs"))
        .and_then(Value::as_i64)
        .unwrap_or_else(now_epoch_ms);
    match op {
        "add" => {
            let provider = req_str(params, "provider")?;
            let account_id = req_str(params, "accountId").or_else(|_| req_str(params, "account_id"))?;
            let label = opt_str(params, "label").map(ToOwned::to_owned);
            let home_dir = opt_str(params, "homeDir")
                .or_else(|| opt_str(params, "home_dir"))
                .map(PathBuf::from);
            let reset_credits = params
                .get("resetCredits")
                .or_else(|| params.get("reset_credits"))
                .and_then(Value::as_u64)
                .map(|v| v as u32);
            let rate_limited = params
                .get("rateLimited")
                .or_else(|| params.get("rate_limited"))
                .and_then(Value::as_bool);
            let meta = mgr.add_account(
                provider,
                account_id,
                label,
                home_dir,
                reset_credits,
                rate_limited,
            )?;
            Ok(json!({
                "id": meta.account_id,
                "accountId": meta.account_id,
                "provider": meta.provider,
                "label": meta.label,
                "homeDir": meta.home_dir.to_string_lossy(),
                "resetCredits": meta.reset_credits,
                "rateLimited": meta.rate_limited,
            }))
        }
        "list" => {
            let filter_prov = opt_str(params, "provider");
            let active_for_filter = filter_prov.and_then(|p| mgr.active_account(p));
            let accounts: Vec<Value> = mgr
                .list_accounts(filter_prov)
                .into_iter()
                .map(|a| {
                    let active = mgr.active_account(&a.provider).as_deref() == Some(&a.account_id);
                    json!({
                        "id": a.account_id,
                        "accountId": a.account_id,
                        "provider": a.provider,
                        "label": a.label,
                        "homeDir": a.home_dir.to_string_lossy(),
                        "active": active,
                        "resetCredits": a.reset_credits,
                        "rateLimited": a.rate_limited,
                    })
                })
                .collect();
            let providers: Vec<Value> = ACCOUNT_HOME_PROVIDERS_9
                .iter()
                .map(|p| {
                    json!({
                        "provider": p,
                        "active_account": mgr.active_account(p),
                    })
                })
                .collect();
            Ok(json!({
                "accounts": accounts,
                "items": accounts,
                "activeAccountId": active_for_filter,
                "providers": providers,
                "codex_reset_credits": mgr.list_codex_reset_credits(),
            }))
        }
        "setActive" | "set_active" => {
            let provider = req_str(params, "provider")?;
            let account_id = req_str(params, "accountId").or_else(|_| req_str(params, "account_id"))?;
            let active = mgr.set_active_account(provider, account_id)?;
            Ok(json!({
                "provider": provider,
                "activeAccountId": active,
            }))
        }
        "getRuntimeEnv" | "get_runtime_env" => {
            let provider = req_str(params, "provider")?;
            let account_id = req_str(params, "accountId").or_else(|_| req_str(params, "account_id"))?;
            let env = mgr.get_runtime_env(provider, account_id)?;
            Ok(json!({
                "provider": provider,
                "accountId": account_id,
                "env": env,
            }))
        }
        "launchBinding" | "launch_binding" => {
            let provider = req_str(params, "provider")?;
            let surface = req_str(params, "surface")?;
            let surface_id = req_str(params, "surfaceId").or_else(|_| req_str(params, "surface_id"))?;
            let bound = mgr.bind_surface_launch(provider, surface, surface_id)?;
            Ok(json!({
                "provider": provider,
                "surface": surface,
                "surfaceId": surface_id,
                "launchAccountId": bound,
            }))
        }
        "resolveTurnAccount" | "resolve_turn_account" => {
            let provider = req_str(params, "provider")?;
            let surface = req_str(params, "surface")?;
            let surface_id = req_str(params, "surfaceId").or_else(|_| req_str(params, "surface_id"))?;
            let (launch_acct, next_acct) = mgr.resolve_turn_account(provider, surface, surface_id)?;
            Ok(json!({
                "provider": provider,
                "surface": surface,
                "surfaceId": surface_id,
                "launchAccountId": launch_acct,
                "nextTurnAccountId": next_acct,
            }))
        }
        "spendResetCredit" | "spend_reset_credit" => {
            let acct = req_str(params, "accountId").or_else(|_| req_str(params, "account_id"))?;
            let attempt_id = opt_str(params, "attemptId")
                .or_else(|| opt_str(params, "attempt_id"))
                .unwrap_or("");
            let (remaining, rate_limited, status) =
                mgr.spend_reset_credit(acct, attempt_id, now_ms)?;
            Ok(json!({
                "accountId": acct,
                "status": status,
                "remainingCredits": remaining,
                "rateLimited": rate_limited,
            }))
        }
        "getResetCreditStatus" | "get_reset_credit_status" => {
            let acct = req_str(params, "accountId").or_else(|_| req_str(params, "account_id"))?;
            let (remaining, rate_limited, status) = mgr.get_reset_credit_status(acct)?;
            Ok(json!({
                "accountId": acct,
                "remainingCredits": remaining,
                "status": status,
                "rateLimited": rate_limited,
            }))
        }
        "swap" => {
            let req = AccountHotSwapRequest {
                session_id: req_str(params, "session_id")
                    .or_else(|_| req_str(params, "sessionId"))?
                    .to_string(),
                provider: req_str(params, "provider")?.to_string(),
                target_account_id: req_str(params, "target_account_id")
                    .or_else(|_| req_str(params, "targetAccountId"))?
                    .to_string(),
                turn_in_progress: params
                    .get("turn_in_progress")
                    .or_else(|| params.get("turnInProgress"))
                    .and_then(Value::as_bool)
                    .unwrap_or(false),
                force: params.get("force").and_then(Value::as_bool).unwrap_or(false),
            };
            Ok(serde_json::to_value(
                mgr.request_account_hotswap(&req, now_ms)?,
            )?)
        }
        "codex_reset_credit" => {
            let acct = req_str(params, "account_id").or_else(|_| req_str(params, "accountId"))?;
            let window = req_str(params, "reset_window_key")
                .or_else(|_| req_str(params, "resetWindowKey"))?;
            let cents = params
                .get("credit_amount_usd_cents")
                .and_then(Value::as_u64)
                .unwrap_or(0);
            Ok(serde_json::to_value(mgr.record_codex_reset_credit(
                acct, window, cents, now_ms,
            )?)?)
        }
        other => Err(OrcaIdeRuntimeError::not_found(format!(
            "unknown accounts RPC method 'accounts.{other}'"
        ))),
    }
}

fn parse_quota_window_kind(raw: &str) -> QuotaWindowKind {
    match raw.trim().to_ascii_lowercase().as_str() {
        "5h" | "five_hour" | "fivehour" => QuotaWindowKind::FiveHour,
        "daily" | "1d" | "24h" => QuotaWindowKind::Daily,
        "weekly" | "7d" => QuotaWindowKind::Weekly,
        _ => QuotaWindowKind::Monthly,
    }
}

fn build_snapshot_from_params(
    params: &Value,
    default_source: QuotaSource,
    now_ms: i64,
) -> Result<ProviderQuotaSnapshot> {
    if let Ok(snap) = serde_json::from_value::<ProviderQuotaSnapshot>(params.clone()) {
        return Ok(snap);
    }
    let provider = req_str(params, "provider")?.to_string();
    let account_id = opt_str(params, "accountId")
        .or_else(|| opt_str(params, "account_id"))
        .unwrap_or("default")
        .to_string();
    let pct = params
        .get("percentUsed")
        .or_else(|| params.get("percent_used"))
        .and_then(Value::as_f64)
        .unwrap_or(0.0);
    let win = opt_str(params, "window")
        .map(parse_quota_window_kind)
        .unwrap_or(QuotaWindowKind::FiveHour);
    let reset_at_ms = params
        .get("resetsInSec")
        .or_else(|| params.get("resets_in_sec"))
        .and_then(Value::as_i64)
        .map(|sec| now_ms.saturating_add(sec.saturating_mul(1_000)));
    let input_tokens = params
        .get("inputTokens")
        .or_else(|| params.get("input_tokens"))
        .and_then(Value::as_u64)
        .unwrap_or(0);
    let output_tokens = params
        .get("outputTokens")
        .or_else(|| params.get("output_tokens"))
        .and_then(Value::as_u64)
        .unwrap_or(0);
    let cached_input_tokens = params
        .get("cachedInputTokens")
        .or_else(|| params.get("cached_input_tokens"))
        .and_then(Value::as_u64)
        .unwrap_or(0);
    let explicit_cost_usd = params
        .get("inferredCostUsd")
        .or_else(|| params.get("explicit_cost_usd"))
        .and_then(Value::as_f64);
    Ok(ProviderQuotaSnapshot {
        provider,
        account_id,
        source: default_source,
        windows: vec![QuotaWindowUsage {
            window_kind: win,
            percent_used: pct,
            reset_at_ms,
        }],
        input_tokens,
        output_tokens,
        cached_input_tokens,
        explicit_cost_usd,
        captured_at_ms: now_ms,
    })
}

pub(super) fn dispatch_usage_namespace(
    handles: &RpcSubsystemHandles,
    op: &str,
    params: &Value,
) -> Result<Value> {
    let usage = &handles.usage_roster;
    let now_ms = params
        .get("now_ms")
        .or_else(|| params.get("nowMs"))
        .and_then(Value::as_i64)
        .unwrap_or_else(now_epoch_ms);
    match op {
        "roster" | "list" => {
            let rows = usage.sorted_roster_wire_json(now_ms);
            Ok(json!({
                "providers": rows,
                "usage": rows,
                "items": rows,
            }))
        }
        "ingest_snapshot" | "ingestSnapshot" => {
            let snap = build_snapshot_from_params(params, QuotaSource::StructuredApi, now_ms)?;
            let entry = usage.ingest_snapshot(snap)?;
            Ok(serde_json::to_value(entry)?)
        }
        "ingest_status_line" | "ingestStatusLine" => {
            if let Some(line) = opt_str(params, "status_line").or_else(|| opt_str(params, "statusLine")) {
                let acct = opt_str(params, "account_id")
                    .or_else(|| opt_str(params, "accountId"))
                    .unwrap_or("default");
                Ok(serde_json::to_value(
                    usage.ingest_claude_status_line(acct, line, now_ms)?,
                )?)
            } else {
                let snap =
                    build_snapshot_from_params(params, QuotaSource::ClaudeStatusLine, now_ms)?;
                Ok(serde_json::to_value(usage.ingest_snapshot(snap)?)?)
            }
        }
        "reportLimitStop" | "report_limit_stop" => {
            let provider = req_str(params, "provider")?;
            let auto_continue = params
                .get("autoContinue")
                .or_else(|| params.get("auto_continue"))
                .and_then(Value::as_bool)
                .unwrap_or(true);
            let grace_minutes = params
                .get("graceMinutes")
                .or_else(|| params.get("grace_minutes"))
                .and_then(Value::as_u64)
                .unwrap_or(10) as u32;
            let resumes_at = opt_str(params, "resumesAt")
                .or_else(|| opt_str(params, "resumes_at"))
                .unwrap_or("soon");
            let st = usage.report_limit_stop(provider, auto_continue, grace_minutes, resumes_at, now_ms)?;
            Ok(json!({
                "provider": st.provider,
                "autoContinue": st.auto_continue,
                "graceMinutes": st.grace_minutes,
                "resumesAt": st.resumes_at,
                "statusText": st.status_text,
                "limitStatusText": st.status_text,
            }))
        }
        "getLimitStatus" | "get_limit_status" => {
            let provider = req_str(params, "provider")?;
            let st = usage.get_limit_status(provider)?;
            Ok(json!({
                "provider": st.provider,
                "autoContinue": st.auto_continue,
                "graceMinutes": st.grace_minutes,
                "resumesAt": st.resumes_at,
                "statusText": st.status_text,
                "limitStatusText": st.status_text,
            }))
        }
        other => Err(OrcaIdeRuntimeError::not_found(format!(
            "unknown usage RPC method 'usage.{other}'"
        ))),
    }
}

pub(crate) fn parse_flexible_hibernation_candidate(
    params: &Value,
) -> Result<(HibernationCandidateState, Option<u64>)> {
    let window_override = params
        .get("windowMs")
        .or_else(|| params.get("window_ms"))
        .and_then(Value::as_u64);
    if let Ok(cand) = serde_json::from_value::<HibernationCandidateState>(params.clone()) {
        return Ok((cand, window_override));
    }
    let tab_id = opt_str(params, "sessionId")
        .or_else(|| opt_str(params, "session_id"))
        .or_else(|| opt_str(params, "tabId"))
        .or_else(|| opt_str(params, "tab_id"))
        .unwrap_or("tab-default")
        .to_string();
    let agent_id = opt_str(params, "agent")
        .or_else(|| opt_str(params, "agentId"))
        .or_else(|| opt_str(params, "agent_id"))
        .unwrap_or("claude")
        .to_string();
    let agent_state_is_done = params
        .get("agent_state_is_done")
        .or_else(|| params.get("agentStateIsDone"))
        .and_then(Value::as_bool)
        .unwrap_or_else(|| {
            opt_str(params, "state")
                .is_some_and(|s| s.eq_ignore_ascii_case("done") || s.eq_ignore_ascii_case("completed"))
        });
    let tab_visible = params
        .get("tabVisible")
        .or_else(|| params.get("tab_visible"))
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let keystrokes_since_done = params
        .get("keystrokesSinceDone")
        .or_else(|| params.get("keystrokes_since_done"))
        .and_then(Value::as_u64)
        .unwrap_or(0);
    let idle_duration_ms = params
        .get("idleMs")
        .or_else(|| params.get("idle_duration_ms"))
        .or_else(|| params.get("idleDurationMs"))
        .and_then(Value::as_u64)
        .unwrap_or(0);
    let mobile_driver_attached = params
        .get("mobileDriverActive")
        .or_else(|| params.get("mobile_driver_attached"))
        .or_else(|| params.get("mobileDriverAttached"))
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let unsettled_orchestration_dispatch = params
        .get("unsettled_orchestration_dispatch")
        .and_then(Value::as_bool)
        .unwrap_or_else(|| {
            params
                .get("unsettledDispatchCount")
                .or_else(|| params.get("unsettled_dispatch_count"))
                .and_then(Value::as_u64)
                .unwrap_or(0)
                > 0
        });
    let live_subagent_count = params
        .get("liveSubagentCount")
        .or_else(|| params.get("live_subagent_count"))
        .and_then(Value::as_u64)
        .unwrap_or(0) as usize;
    let session_handle = opt_str(params, "sessionHandle")
        .or_else(|| opt_str(params, "session_handle"))
        .map(ToOwned::to_owned)
        .or_else(|| Some(format!("resume-{tab_id}")));
    let original_cwd = PathBuf::from(
        opt_str(params, "cwd")
            .or_else(|| opt_str(params, "originalCwd"))
            .or_else(|| opt_str(params, "original_cwd"))
            .unwrap_or("/tmp"),
    );
    let original_args: Vec<String> = params
        .get("args")
        .or_else(|| params.get("originalArgs"))
        .or_else(|| params.get("original_args"))
        .cloned()
        .map(serde_json::from_value)
        .transpose()?
        .unwrap_or_default();
    let original_env: BTreeMap<String, String> = params
        .get("env")
        .or_else(|| params.get("originalEnv"))
        .or_else(|| params.get("original_env"))
        .cloned()
        .map(serde_json::from_value)
        .transpose()?
        .unwrap_or_default();

    Ok((
        HibernationCandidateState {
            tab_id,
            agent_state_is_done,
            tab_visible,
            keystrokes_since_done,
            agent_id,
            idle_duration_ms,
            mobile_driver_attached,
            unsettled_orchestration_dispatch,
            live_subagent_count,
            git_operation_in_progress: params
                .get("git_operation_in_progress")
                .or_else(|| params.get("gitOperationInProgress"))
                .and_then(Value::as_bool)
                .unwrap_or(false),
            active_port_forward: params
                .get("active_port_forward")
                .or_else(|| params.get("activePortForward"))
                .and_then(Value::as_bool)
                .unwrap_or(false),
            pending_bot_approval: params
                .get("pending_bot_approval")
                .or_else(|| params.get("pendingBotApproval"))
                .and_then(Value::as_bool)
                .unwrap_or(false),
            unsynced_local_snapshot: params
                .get("unsynced_local_snapshot")
                .or_else(|| params.get("unsyncedLocalSnapshot"))
                .and_then(Value::as_bool)
                .unwrap_or(false),
            pinned_no_hibernate: params
                .get("pinned_no_hibernate")
                .or_else(|| params.get("pinnedNoHibernate"))
                .and_then(Value::as_bool)
                .unwrap_or(false),
            session_handle,
            original_cwd,
            original_args,
            original_env,
        },
        window_override,
    ))
}

pub(super) fn dispatch_hibernation_namespace(
    handles: &RpcSubsystemHandles,
    op: &str,
    params: &Value,
) -> Result<Value> {
    let hib = &handles.hibernation;
    let now_ms = params
        .get("now_ms")
        .or_else(|| params.get("nowMs"))
        .and_then(Value::as_i64)
        .unwrap_or_else(now_epoch_ms);
    match op {
        "check" => {
            let (mut cand, win_override) = parse_flexible_hibernation_candidate(params)?;
            if let Some(did) =
                opt_str(params, "dispatchId").or_else(|| opt_str(params, "dispatch_id"))
                && handles.orchestration.is_dispatch_unsettled(did)?
            {
                cand.unsettled_orchestration_dispatch = true;
            }
            let (eligible, passed, failed, decision) = hib.evaluate_8_checks(&cand, win_override);
            Ok(json!({
                "decision": decision,
                "eligible": eligible,
                "passedChecks": passed,
                "failedChecks": failed,
            }))
        }
        "sleep" => {
            let (mut cand, win_override) = parse_flexible_hibernation_candidate(params)?;
            if let Some(did) =
                opt_str(params, "dispatchId").or_else(|| opt_str(params, "dispatch_id"))
                && handles.orchestration.is_dispatch_unsettled(did)?
            {
                cand.unsettled_orchestration_dispatch = true;
            }
            let (eligible, _passed, failed, _decision) = hib.evaluate_8_checks(&cand, win_override);
            if !eligible {
                return Err(OrcaIdeRuntimeError::ide(
                    orca_core::OrcaIdeErrorCode::HibernationBlocked,
                    format!("hibernation blocked by checks: {}", failed.join(", ")),
                ));
            }
            let mut tab = WorkspaceTab {
                tab_id: cand.tab_id.clone(),
                content_type: TabContentType::Terminal,
                title: cand.agent_id.clone(),
                pinned: false,
                preview: false,
                view_mode: TabViewMode::Terminal,
                leaf_state: TabLeafState::Live,
                pty_incarnation_id: None,
                dirty: false,
                created_at_ms: now_ms,
                last_active_at_ms: now_ms,
            };
            let outcome = hib.hibernate_tab(&mut tab, &cand, now_ms)?;
            handles
                .hibernated_tabs
                .write()
                .insert(cand.tab_id.clone(), tab);
            let resume_sid = cand
                .session_handle
                .clone()
                .unwrap_or_else(|| format!("resume-{}", cand.tab_id));
            Ok(json!({
                "sessionId": cand.tab_id,
                "sleeping": true,
                "ptyRunning": false,
                "outcome": outcome,
                "resume": {
                    "agent": cand.agent_id,
                    "sessionId": resume_sid,
                    "env": cand.original_env,
                    "args": cand.original_args,
                    "cwd": cand.original_cwd.to_string_lossy(),
                },
            }))
        }
        "wake" => {
            let sid = req_str(params, "sessionId")
                .or_else(|_| req_str(params, "session_id"))
                .or_else(|_| req_str(params, "tabId"))?;
            let mut tab = handles
                .hibernated_tabs
                .write()
                .remove(sid)
                .ok_or_else(|| {
                    OrcaIdeRuntimeError::not_found(format!("no hibernated session '{sid}'"))
                })?;
            let orig_args = hib
                .get_hibernated(sid)
                .map(|r| r.original_args)
                .unwrap_or_default();
            let plan = hib.build_rehydration_plan(&mut tab, now_ms)?;
            let launch_command = format!("{} {}", plan.binary, plan.args.join(" "));
            Ok(json!({
                "sessionId": sid,
                "sleeping": false,
                "ptyRunning": true,
                "launchCommand": launch_command,
                "command": launch_command,
                "args": orig_args,
                "launchArgs": plan.args,
                "env": plan.env,
                "plan": plan,
            }))
        }
        "hibernate" => {
            let mut tab: WorkspaceTab = serde_json::from_value(
                params
                    .get("tab")
                    .cloned()
                    .ok_or_else(|| OrcaIdeRuntimeError::invalid_argument("missing 'tab'"))?,
            )?;
            let mut cand: HibernationCandidateState = serde_json::from_value(
                params
                    .get("candidate")
                    .cloned()
                    .ok_or_else(|| OrcaIdeRuntimeError::invalid_argument("missing 'candidate'"))?,
            )?;
            let dispatch_id = opt_str(params, "dispatchId")
                .or_else(|| opt_str(params, "dispatch_id"))
                .or_else(|| {
                    params
                        .get("candidate")
                        .and_then(|c| opt_str(c, "dispatchId").or_else(|| opt_str(c, "dispatch_id")))
                });
            if let Some(did) = dispatch_id
                && handles.orchestration.is_dispatch_unsettled(did)?
            {
                cand.unsettled_orchestration_dispatch = true;
            }
            let outcome = hib.hibernate_tab(&mut tab, &cand, now_ms)?;
            Ok(json!({ "outcome": outcome, "tab": tab }))
        }
        "rehydrate" => {
            let mut tab: WorkspaceTab = serde_json::from_value(
                params
                    .get("tab")
                    .cloned()
                    .ok_or_else(|| OrcaIdeRuntimeError::invalid_argument("missing 'tab'"))?,
            )?;
            let plan = hib.build_rehydration_plan(&mut tab, now_ms)?;
            Ok(json!({ "plan": plan, "tab": tab }))
        }
        other => Err(OrcaIdeRuntimeError::not_found(format!(
            "unknown hibernation RPC method 'hibernation.{other}'"
        ))),
    }
}

pub(super) fn dispatch_e2ee_namespace(
    handles: &RpcSubsystemHandles,
    op: &str,
    params: &Value,
) -> Result<Value> {
    match op {
        "handshake" => {
            let req: E2eeHandshakeInitRequest = serde_json::from_value(params.clone())?;
            Ok(serde_json::to_value(handles.e2ee.server_handshake(&req)?)?)
        }
        "encrypt" => {
            let session_id = req_str(params, "session_id")?;
            let plaintext: Vec<u8> = if let Some(b64) = opt_str(params, "plaintext_b64") {
                BASE64_STD.decode(b64).map_err(|e| {
                    OrcaIdeRuntimeError::invalid_argument(format!("invalid plaintext_b64: {e}"))
                })?
            } else {
                params
                    .get("plaintext")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .as_bytes()
                    .to_vec()
            };
            let frame = handles.e2ee.encrypt_frame(session_id, &plaintext)?;
            Ok(serde_json::to_value(frame)?)
        }
        "decrypt" => {
            let session_id = req_str(params, "session_id")?;
            let frame: E2eeV2Frame = serde_json::from_value(
                params
                    .get("frame")
                    .cloned()
                    .ok_or_else(|| OrcaIdeRuntimeError::invalid_argument("missing 'frame'"))?,
            )?;
            let plaintext = handles.e2ee.decrypt_frame(session_id, &frame)?;
            Ok(json!({
                "session_id": session_id,
                "plaintext_b64": BASE64_STD.encode(&plaintext),
                "plaintext_utf8": String::from_utf8_lossy(&plaintext),
            }))
        }
        other => Err(OrcaIdeRuntimeError::not_found(format!(
            "unknown e2ee RPC method 'e2ee.{other}'"
        ))),
    }
}

pub(super) fn dispatch_profile_namespace(
    handles: &RpcSubsystemHandles,
    op: &str,
    params: &Value,
) -> Result<Value> {
    let profile = &handles.profile_state;
    let now_ms = params
        .get("now_ms")
        .and_then(Value::as_i64)
        .unwrap_or_else(now_epoch_ms);
    match op {
        "get" | "read" => {
            let domain = req_str(params, "domain")?;
            let doc = profile.get_domain(domain)?;
            let parts = profile.list_document_parts(domain)?;
            if !parts.is_empty() {
                return Ok(json!({
                    "domain": domain,
                    "parts": parts,
                    "document": doc,
                }));
            }
            Ok(serde_json::to_value(doc)?)
        }
        "put" | "write" => {
            if let Some(mutations_val) = params.get("mutations") {
                let mutations: Vec<DirtyDomainMutation> =
                    serde_json::from_value(mutations_val.clone())?;
                Ok(serde_json::to_value(
                    profile.write_dirty_domains(&mutations, now_ms)?,
                )?)
            } else {
                let domain = req_str(params, "domain")?.to_string();
                let payload = params.get("payload").cloned().unwrap_or(Value::Null);
                let expected_revision = params
                    .get("expected_revision")
                    .or_else(|| params.get("expectedRevision"))
                    .and_then(Value::as_u64);
                let domain_version = params
                    .get("domain_version")
                    .or_else(|| params.get("domainVersion"))
                    .and_then(Value::as_u64)
                    .unwrap_or(1) as u32;
                let mut docs = profile.write_dirty_domains(
                    &[DirtyDomainMutation {
                        domain,
                        expected_revision,
                        domain_version,
                        payload,
                    }],
                    now_ms,
                )?;
                Ok(serde_json::to_value(docs.pop())?)
            }
        }
        "writeParts" | "write_parts" => {
            let domain = req_str(params, "domain")?;
            let parts_arr = params
                .get("parts")
                .and_then(Value::as_array)
                .ok_or_else(|| OrcaIdeRuntimeError::invalid_argument("missing array 'parts'"))?;
            let mut parsed = Vec::with_capacity(parts_arr.len());
            for (idx, item) in parts_arr.iter().enumerate() {
                let pk = item
                    .get("part_key")
                    .or_else(|| item.get("partKey"))
                    .and_then(Value::as_str)
                    .ok_or_else(|| OrcaIdeRuntimeError::invalid_argument("part missing 'part_key'"))?
                    .to_string();
                let ordinal = item
                    .get("ordinal")
                    .and_then(Value::as_i64)
                    .unwrap_or(idx as i64);
                let payload = item.get("payload").cloned().unwrap_or(Value::Null);
                parsed.push((pk, ordinal, payload));
            }
            let saved = profile.replace_document_parts(domain, &parsed)?;
            Ok(json!({ "domain": domain, "parts": saved }))
        }
        "export" => profile.export_all(),
        "inspectRecovery" | "inspect_recovery" => {
            let db_override = opt_str(params, "profile_db_path")
                .or_else(|| opt_str(params, "profileDbPath"))
                .map(Path::new);
            let legacy_json = opt_str(params, "legacy_json_path")
                .or_else(|| opt_str(params, "legacyJsonPath"))
                .map(Path::new);
            Ok(profile.inspect_recovery(db_override, legacy_json))
        }
        "rollback" => {
            let backup_raw = req_str(params, "backup_path")
                .or_else(|_| req_str(params, "backupPath"))?;
            profile.rollback_from_backup(Path::new(backup_raw), now_ms)
        }
        "snapshotWrite" | "snapshot_write" => {
            let path_raw = req_str(params, "path")?;
            let payload = params.get("payload").cloned().unwrap_or(Value::Null);
            let path = PathBuf::from(path_raw);
            let writer = {
                let mut writers = handles.snapshot_writers.lock();
                writers
                    .entry(path.clone())
                    .or_insert_with(|| Arc::new(AtomicSnapshotWriter::new(path)))
                    .clone()
            };
            let stage_async = params
                .get("stage_async")
                .or_else(|| params.get("stageAsync"))
                .and_then(Value::as_bool)
                .unwrap_or(false);
            if stage_async {
                let staged = writer.begin_async_stage_raw_json(&payload)?;
                return Ok(json!({
                    "path": path_raw,
                    "generation": staged.generation,
                    "staged": true,
                }));
            }
            if let Some(fin_gen) = params
                .get("finalize_generation")
                .or_else(|| params.get("finalizeGeneration"))
                .and_then(Value::as_u64)
            {
                let promoted = writer.finalize_staged_generation(fin_gen)?;
                return Ok(json!({
                    "path": path_raw,
                    "generation": fin_gen,
                    "fenced": !promoted,
                    "promoted": promoted,
                    "committed_generation": writer.last_committed_generation(),
                }));
            }
            let committed_generation = writer.flush_sync_raw_json(&payload)?;
            Ok(json!({
                "path": path_raw,
                "committed_generation": committed_generation,
                "generation": committed_generation,
                "payload": payload,
            }))
        }
        "snapshotRead" | "snapshot_read" => {
            let path_raw = req_str(params, "path")?;
            let path = PathBuf::from(path_raw);
            let writer = {
                let mut writers = handles.snapshot_writers.lock();
                writers
                    .entry(path.clone())
                    .or_insert_with(|| Arc::new(AtomicSnapshotWriter::new(path)))
                    .clone()
            };
            let payload = writer.read_raw_json()?;
            Ok(json!({
                "path": path_raw,
                "payload": payload,
                "committed_generation": writer.last_committed_generation(),
            }))
        }
        "secretsSave" | "secrets_save" => {
            if let Some(backend) = opt_str(params, "keyring_backend")
                .or_else(|| opt_str(params, "keyringBackend"))
                && matches!(
                    backend.trim().to_ascii_lowercase().as_str(),
                    "basic_text" | "plaintext" | "none" | "unencrypted"
                )
            {
                return Err(OrcaIdeRuntimeError::invalid_argument(format!(
                    "keyring_unavailable: refusing insecure keyring backend '{backend}'"
                )));
            }
            let slot = req_str(params, "slot")?;
            let secret = req_str(params, "secret")?;
            let protected = orca_core::orchestration_protocol::ProtectedSecret::new(
                secret.as_bytes().to_vec(),
            );
            handles
                .secrets
                .store_secret(slot, slot, &protected, now_ms)?;
            let digest = handles
                .secrets
                .combined_ciphertext_digest()
                .unwrap_or_default();
            Ok(json!({
                "slot": slot,
                "sealed": true,
                "ciphertext_digest": digest,
                "sealed_digest": digest,
                "sealed_slots_count": handles.secrets.sealed_slot_count(),
            }))
        }
        "secretsStatus" | "secrets_status" => {
            let sim_locked = params
                .get("simulate_locked")
                .or_else(|| params.get("simulateLocked"))
                .and_then(Value::as_bool)
                .unwrap_or(false);
            let mode = opt_str(params, "mode").unwrap_or("normal");
            let degraded = sim_locked
                || mode.eq_ignore_ascii_case("serve_degraded")
                || handles.secrets.is_degraded();
            let slots = handles.secrets.list_slot_names();
            let digest = handles.secrets.combined_ciphertext_digest();
            Ok(json!({
                "degraded": degraded,
                "mode": if degraded { "serve_degraded" } else { mode },
                "sealed_slots_count": slots.len(),
                "sealed_slots": slots,
                "ciphertext_digest": digest,
                "sealed_digest": digest,
            }))
        }
        "list_domains" | "listDomains" => Ok(serde_json::to_value(profile.list_domains()?)?),
        other => Err(OrcaIdeRuntimeError::not_found(format!(
            "unknown profile RPC method 'profile.{other}'"
        ))),
    }
}

fn stt_status_to_wire_json(st: &SttStatus) -> Value {
    let backend = match st.effective_mode {
        SttBackendMode::LocalCpuFallback => "cpu",
        SttBackendMode::LocalGpu => "gpu",
        SttBackendMode::RemoteOpenAiCompatible => "remote",
    };
    let mut val = serde_json::to_value(st).unwrap_or_else(|_| json!({}));
    if let Some(obj) = val.as_object_mut() {
        obj.insert("gpu_fallback_active".into(), json!(st.gpu_failure_marker_present));
        obj.insert("gpuFallbackActive".into(), json!(st.gpu_failure_marker_present));
        obj.insert("backend".into(), json!(backend));
        obj.insert("active_backend".into(), json!(backend));
    }
    val
}

pub(super) fn dispatch_vm_namespace(
    handles: &RpcSubsystemHandles,
    op: &str,
    params: &Value,
) -> Result<Value> {
    match op {
        "doctor" | "recipeDoctor" | "recipe_doctor" => {
            let dir = PathBuf::from(
                req_str(params, "base_snapshot_dir")
                    .or_else(|_| req_str(params, "user_data_dir"))
                    .or_else(|_| req_str(params, "userDataDir"))
                    .or_else(|_| req_str(params, "dir"))?,
            );
            let r = handles.ephemeral_vm.run_doctor(&dir)?;
            Ok(json!({
                "snapshot_dir": r.snapshot_dir,
                "clean": r.clean,
                "healthy": r.clean,
                "contaminated": !r.clean,
                "contaminated_files": r.contaminated_files,
                "blocking_issues": r.contaminated_files,
                "issues": r.contaminated_files,
                "remediation": r.remediation,
            }))
        }
        "plan_recipe" | "planRecipe" => {
            let recipe: OrcaYamlEnvironmentRecipe = serde_json::from_value(
                params
                    .get("recipe")
                    .cloned()
                    .ok_or_else(|| OrcaIdeRuntimeError::invalid_argument("missing 'recipe'"))?,
            )?;
            let op_kind = VmLifecycleOp::parse(req_str(params, "operation")?)?;
            let workdir = opt_str(params, "workdir").map(ToOwned::to_owned);
            Ok(serde_json::to_value(
                handles
                    .ephemeral_vm
                    .plan_recipe_operation(&recipe, op_kind, workdir)?,
            )?)
        }
        other => Err(OrcaIdeRuntimeError::not_found(format!(
            "unknown vm RPC method 'vm.{other}'"
        ))),
    }
}

pub(super) fn dispatch_serve_namespace(
    handles: &RpcSubsystemHandles,
    op: &str,
    _params: &Value,
) -> Result<Value> {
    match op {
        "status" | "health" => Ok(json!({
            "headless": true,
            "mode": "headless",
            "reachable": true,
            "status": "running",
            "host_id": handles.host_id,
            "protocol_version": RUNTIME_PROTOCOL_VERSION,
        })),
        other => Err(OrcaIdeRuntimeError::not_found(format!(
            "unknown serve RPC method 'serve.{other}'"
        ))),
    }
}

pub(super) fn dispatch_speech_namespace(
    handles: &RpcSubsystemHandles,
    op: &str,
    params: &Value,
) -> Result<Value> {
    let now_ms = params
        .get("now_ms")
        .or_else(|| params.get("nowMs"))
        .and_then(Value::as_i64)
        .unwrap_or_else(now_epoch_ms);
    match op {
        "status" => Ok(stt_status_to_wire_json(&handles.stt.status())),
        "record_gpu_failure" | "recordGpuFailure" => {
            let reason = opt_str(params, "reason").unwrap_or("gpu backend error");
            let st = handles.stt.record_gpu_failure(reason, now_ms)?;
            Ok(stt_status_to_wire_json(&st))
        }
        other => Err(OrcaIdeRuntimeError::not_found(format!(
            "unknown speech RPC method 'speech.{other}'"
        ))),
    }
}

pub(super) fn dispatch_watchdog_namespace(
    handles: &RpcSubsystemHandles,
    op: &str,
    params: &Value,
) -> Result<Value> {
    match op {
        "status" => Ok(serde_json::to_value(handles.watchdog.status())?),
        "tick" => {
            let monotonic_ms = params
                .get("monotonic_ms")
                .or_else(|| params.get("monotonicMs"))
                .and_then(Value::as_u64)
                .ok_or_else(|| OrcaIdeRuntimeError::invalid_argument("missing 'monotonic_ms'"))?;
            let wall_ms = params
                .get("wall_ms")
                .or_else(|| params.get("wallMs"))
                .and_then(Value::as_i64)
                .unwrap_or_else(now_epoch_ms);
            let ctx = opt_str(params, "context").unwrap_or("rpc_tick");
            Ok(serde_json::to_value(
                handles.watchdog.record_tick(monotonic_ms, wall_ms, ctx),
            )?)
        }
        "evaluate" => {
            let last_delta = params
                .get("lastHeartbeatDeltaMs")
                .or_else(|| params.get("last_heartbeat_delta_ms"))
                .and_then(Value::as_u64)
                .unwrap_or(0);
            let system_slept = params
                .get("systemSlept")
                .or_else(|| params.get("system_slept"))
                .and_then(Value::as_bool)
                .unwrap_or(false);
            let wake_delta = params
                .get("wakeDeltaMs")
                .or_else(|| params.get("wake_delta_ms"))
                .and_then(Value::as_u64)
                .unwrap_or(0);
            let wall_ms = params
                .get("wall_ms")
                .or_else(|| params.get("wallMs"))
                .and_then(Value::as_i64)
                .unwrap_or_else(now_epoch_ms);
            let ctx = opt_str(params, "context").unwrap_or("rpc_evaluate");
            let (status, suppressed, effective_awake_ms) = handles.watchdog.evaluate_sleep_aware(
                last_delta,
                system_slept,
                wake_delta,
                wall_ms,
                ctx,
            );
            Ok(json!({
                "status": status,
                "suppressed_by_sleep": suppressed,
                "suppressedBySleep": suppressed,
                "effectiveAwakeMs": effective_awake_ms,
                "watchdog": handles.watchdog.status(),
            }))
        }
        other => Err(OrcaIdeRuntimeError::not_found(format!(
            "unknown watchdog RPC method 'watchdog.{other}'"
        ))),
    }
}
