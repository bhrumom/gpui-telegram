//! Unified RPC routing for `runtime.*`, `aiVault.*`, `diff.*`, `plugins.*`, `computer.*`,
//! `emulator.*`, `artifacts.*`, `accounts.*`, `usage.*`, `hibernation.*`, `e2ee.*`,
//! `profile.*`, `vm.*`, `speech.*`, and `watchdog.*`.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD as BASE64_STD;
use orca_core::RUNTIME_PROTOCOL_VERSION;
use orca_core::orchestration_protocol::{
    ComputerUseAction, ComputerUsePolicy, ComputerUseRequest, ComputerUseTargetKind,
    DiffAnnotation, DiffSide, E2eeV2Frame, LineAttribution, PluginManifestV2,
    negotiate_runtime_capabilities, negotiate_runtime_protocol,
};
use orca_core::workspace_layout::{OrcaYamlEnvironmentRecipe, WorkspaceTab};
use serde_json::{Value, json};

use super::RpcSubsystemHandles;
use super::e2ee_manager::E2eeHandshakeInitRequest;
use crate::accounts_usage::{
    ACCOUNT_HOME_PROVIDERS_9, AccountHotSwapRequest, ProviderQuotaSnapshot,
};
use crate::diff_review::{
    CommitPrDraftRequest, HubTaskRecord, TasksHubFilter, compose_commit_and_pr_draft,
};
use crate::hibernation::HibernationCandidateState;
use crate::plugin_host::{McpConfigSync, McpServerEntry, PluginContributionPoint};
use crate::profile_state::DirtyDomainMutation;
use crate::system_services::{
    ArtifactKind, AxNode, PublishArtifactRequest, VmLifecycleOp, compute_ax_tree_diff,
    norm_float_to_bp,
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
        "index" => {
            let session_id = req_str(params, "session_id")?;
            let agent_id = opt_str(params, "agent_id").unwrap_or("claude");
            let workspace_id = opt_str(params, "workspace_id").unwrap_or("default");
            if let Some(fp) = opt_str(params, "file_path") {
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
            let src = req_str(params, "source_session_id").or_else(|_| req_str(params, "session_id"))?;
            let new_id = req_str(params, "new_session_id")?;
            let up_to = params.get("up_to_seq").and_then(Value::as_i64);
            let forked = vault.fork_conversation(src, new_id, up_to, now_ms)?;
            Ok(serde_json::to_value(forked)?)
        }
        "handoff" => {
            let session_id = req_str(params, "session_id")?;
            let target_agent = req_str(params, "target_agent_id")?;
            let max_messages = params
                .get("max_messages")
                .and_then(Value::as_u64)
                .unwrap_or(50) as usize;
            let prompt = vault.build_cross_agent_handoff_prompt(session_id, target_agent, max_messages)?;
            Ok(json!({ "session_id": session_id, "target_agent_id": target_agent, "prompt": prompt }))
        }
        "purge_tools" => {
            let sid = opt_str(params, "session_id");
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
            let annotation: DiffAnnotation = if params.get("annotation_id").is_some() {
                serde_json::from_value(params.clone())?
            } else {
                DiffAnnotation {
                    annotation_id: opt_str(params, "id")
                        .map(ToOwned::to_owned)
                        .unwrap_or_else(|| format!("ann-{}", uuid::Uuid::new_v4())),
                    file_path: req_str(params, "file_path")?.to_string(),
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
            Ok(serde_json::to_value(diff.add_annotation(annotation)?)?)
        }
        "list" => {
            let fp = opt_str(params, "file_path");
            let inc = params
                .get("include_resolved")
                .and_then(Value::as_bool)
                .unwrap_or(false);
            Ok(serde_json::to_value(diff.list_annotations(fp, inc))?)
        }
        "resolve" => {
            let id = req_str(params, "annotation_id").or_else(|_| req_str(params, "id"))?;
            Ok(serde_json::to_value(diff.resolve_annotation(id, now_ms)?)?)
        }
        "reopen" => {
            let id = req_str(params, "annotation_id").or_else(|_| req_str(params, "id"))?;
            Ok(serde_json::to_value(diff.reopen_annotation(id, now_ms)?)?)
        }
        "batch" => {
            let fp = opt_str(params, "file_path");
            let prompt = diff.build_agent_batch_prompt(fp);
            Ok(json!({ "prompt": prompt }))
        }
        "attribution_set" => {
            let fp = req_str(params, "file_path")?;
            let line_start = params.get("line_start").and_then(Value::as_u64).unwrap_or(1) as u32;
            let line_end = params
                .get("line_end")
                .and_then(Value::as_u64)
                .unwrap_or(u64::from(line_start)) as u32;
            let attr: LineAttribution = params
                .get("attribution")
                .cloned()
                .map(serde_json::from_value)
                .transpose()?
                .unwrap_or(LineAttribution::Human);
            if matches!(attr, LineAttribution::Human) {
                diff.record_human_edit(fp, line_start, line_end, now_ms)?;
            } else {
                diff.record_ai_range(fp, line_start, line_end, attr, now_ms)?;
            }
            Ok(serde_json::to_value(diff.file_attribution(fp))?)
        }
        "attribution_get" => {
            let fp = opt_str(params, "file_path");
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

pub(super) fn dispatch_plugins_namespace(
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
            let manifest: PluginManifestV2 = if let Some(m) = params.get("manifest") {
                serde_json::from_value(m.clone())?
            } else {
                serde_json::from_value(params.clone())?
            };
            let contributions: Vec<PluginContributionPoint> = params
                .get("contributions")
                .cloned()
                .map(serde_json::from_value)
                .transpose()?
                .unwrap_or_default();
            Ok(serde_json::to_value(
                host.validate_manifest(&manifest, &contributions)?,
            )?)
        }
        "install" => {
            let manifest: PluginManifestV2 = if let Some(m) = params.get("manifest") {
                serde_json::from_value(m.clone())?
            } else {
                serde_json::from_value(params.clone())?
            };
            let contributions: Vec<PluginContributionPoint> = params
                .get("contributions")
                .cloned()
                .map(serde_json::from_value)
                .transpose()?
                .unwrap_or_default();
            let pkg_dir = opt_str(params, "package_dir").map(PathBuf::from);
            Ok(serde_json::to_value(host.install_plugin(
                manifest,
                contributions,
                pkg_dir.as_deref(),
                now_ms,
            )?)?)
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
        "list" => Ok(serde_json::to_value(host.list_plugins())?),
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
        other => Err(OrcaIdeRuntimeError::not_found(format!(
            "unknown plugins RPC method 'plugins.{other}'"
        ))),
    }
}

pub(super) fn dispatch_computer_namespace(
    handles: &RpcSubsystemHandles,
    op: &str,
    params: &Value,
) -> Result<Value> {
    match op {
        "guard" => {
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
            Ok(serde_json::to_value(decision)?)
        }
        "tree_diff" => {
            let before: Vec<AxNode> = params
                .get("before")
                .cloned()
                .map(serde_json::from_value)
                .transpose()?
                .unwrap_or_default();
            let after: Vec<AxNode> = params
                .get("after")
                .cloned()
                .map(serde_json::from_value)
                .transpose()?
                .unwrap_or_default();
            Ok(serde_json::to_value(compute_ax_tree_diff(&before, &after))?)
        }
        other => Err(OrcaIdeRuntimeError::not_found(format!(
            "unknown computer RPC method 'computer.{other}'"
        ))),
    }
}

pub(super) fn dispatch_emulator_namespace(
    handles: &RpcSubsystemHandles,
    op: &str,
    params: &Value,
) -> Result<Value> {
    match op {
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
) -> Result<Value> {
    let store = &handles.artifacts;
    let now_ms = params
        .get("now_ms")
        .and_then(Value::as_i64)
        .unwrap_or_else(now_epoch_ms);
    match op {
        "share" => {
            let kind = opt_str(params, "kind")
                .map(ArtifactKind::parse)
                .transpose()?
                .unwrap_or(ArtifactKind::Markdown);
            let req = PublishArtifactRequest {
                artifact_id: opt_str(params, "artifact_id").map(ToOwned::to_owned),
                title: req_str(params, "title")?.to_string(),
                kind,
                content: params
                    .get("content")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string(),
                profile_id: opt_str(params, "profile_id").unwrap_or("default").to_string(),
                user_confirmed_share: params
                    .get("user_confirmed_share")
                    .and_then(Value::as_bool)
                    .unwrap_or(false),
                base_url: opt_str(params, "base_url").map(ToOwned::to_owned),
                now_ms,
            };
            // ArtifactPublicView NEVER contains edit_token!
            Ok(serde_json::to_value(store.publish_artifact(req)?)?)
        }
        "get" => {
            let id = req_str(params, "artifact_id").or_else(|_| req_str(params, "id"))?;
            Ok(serde_json::to_value(store.get_artifact(id)?)?)
        }
        "update" => {
            let id = req_str(params, "artifact_id").or_else(|_| req_str(params, "id"))?;
            let profile_id = opt_str(params, "profile_id").unwrap_or("default");
            let new_title = opt_str(params, "title");
            let content = req_str(params, "content")?;
            Ok(serde_json::to_value(store.update_artifact(
                id, profile_id, new_title, content, now_ms,
            )?)?)
        }
        "list" => {
            let pid = opt_str(params, "profile_id");
            Ok(serde_json::to_value(store.list_artifacts(pid)?)?)
        }
        "unshare" => {
            let id = req_str(params, "artifact_id").or_else(|_| req_str(params, "id"))?;
            Ok(serde_json::to_value(store.unshare_artifact(id, now_ms)?)?)
        }
        "delete" => {
            let id = req_str(params, "artifact_id").or_else(|_| req_str(params, "id"))?;
            let deleted = store.delete_artifact(id, now_ms)?;
            Ok(json!({ "deleted": deleted, "artifact_id": id }))
        }
        other => Err(OrcaIdeRuntimeError::not_found(format!(
            "unknown artifacts RPC method 'artifacts.{other}'"
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
        .and_then(Value::as_i64)
        .unwrap_or_else(now_epoch_ms);
    match op {
        "list" => {
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
                "providers": providers,
                "codex_reset_credits": mgr.list_codex_reset_credits(),
            }))
        }
        "swap" => {
            let req = AccountHotSwapRequest {
                session_id: req_str(params, "session_id")?.to_string(),
                provider: req_str(params, "provider")?.to_string(),
                target_account_id: req_str(params, "target_account_id")?.to_string(),
                turn_in_progress: params
                    .get("turn_in_progress")
                    .and_then(Value::as_bool)
                    .unwrap_or(false),
                force: params.get("force").and_then(Value::as_bool).unwrap_or(false),
            };
            Ok(serde_json::to_value(
                mgr.request_account_hotswap(&req, now_ms)?,
            )?)
        }
        "codex_reset_credit" => {
            let acct = req_str(params, "account_id")?;
            let window = req_str(params, "reset_window_key")?;
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

pub(super) fn dispatch_usage_namespace(
    handles: &RpcSubsystemHandles,
    op: &str,
    params: &Value,
) -> Result<Value> {
    let usage = &handles.usage_roster;
    let now_ms = params
        .get("now_ms")
        .and_then(Value::as_i64)
        .unwrap_or_else(now_epoch_ms);
    match op {
        "roster" => Ok(serde_json::to_value(usage.sorted_roster(now_ms))?),
        "ingest_snapshot" => {
            let snap: ProviderQuotaSnapshot = serde_json::from_value(params.clone())?;
            Ok(serde_json::to_value(usage.ingest_snapshot(snap)?)?)
        }
        "ingest_status_line" => {
            let acct = opt_str(params, "account_id").unwrap_or("default");
            let line = req_str(params, "status_line")?;
            Ok(serde_json::to_value(
                usage.ingest_claude_status_line(acct, line, now_ms)?,
            )?)
        }
        other => Err(OrcaIdeRuntimeError::not_found(format!(
            "unknown usage RPC method 'usage.{other}'"
        ))),
    }
}

pub(super) fn dispatch_hibernation_namespace(
    handles: &RpcSubsystemHandles,
    op: &str,
    params: &Value,
) -> Result<Value> {
    let hib = &handles.hibernation;
    let now_ms = params
        .get("now_ms")
        .and_then(Value::as_i64)
        .unwrap_or_else(now_epoch_ms);
    match op {
        "check" => {
            let cand: HibernationCandidateState = serde_json::from_value(params.clone())?;
            Ok(serde_json::to_value(hib.evaluate(&cand)?)?)
        }
        "hibernate" => {
            let mut tab: WorkspaceTab = serde_json::from_value(
                params
                    .get("tab")
                    .cloned()
                    .ok_or_else(|| OrcaIdeRuntimeError::invalid_argument("missing 'tab'"))?,
            )?;
            let cand: HibernationCandidateState = serde_json::from_value(
                params
                    .get("candidate")
                    .cloned()
                    .ok_or_else(|| OrcaIdeRuntimeError::invalid_argument("missing 'candidate'"))?,
            )?;
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
        "get" => {
            let domain = req_str(params, "domain")?;
            Ok(serde_json::to_value(profile.get_domain(domain)?)?)
        }
        "put" => {
            if let Some(mutations_val) = params.get("mutations") {
                let mutations: Vec<DirtyDomainMutation> =
                    serde_json::from_value(mutations_val.clone())?;
                Ok(serde_json::to_value(
                    profile.write_dirty_domains(&mutations, now_ms)?,
                )?)
            } else {
                let domain = req_str(params, "domain")?.to_string();
                let payload = params.get("payload").cloned().unwrap_or(Value::Null);
                let expected_revision = params.get("expected_revision").and_then(Value::as_u64);
                let domain_version = params
                    .get("domain_version")
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
        "list_domains" => Ok(serde_json::to_value(profile.list_domains()?)?),
        other => Err(OrcaIdeRuntimeError::not_found(format!(
            "unknown profile RPC method 'profile.{other}'"
        ))),
    }
}

pub(super) fn dispatch_vm_namespace(
    handles: &RpcSubsystemHandles,
    op: &str,
    params: &Value,
) -> Result<Value> {
    match op {
        "doctor" => {
            let dir = PathBuf::from(req_str(params, "base_snapshot_dir").or_else(|_| req_str(params, "dir"))?);
            Ok(serde_json::to_value(handles.ephemeral_vm.run_doctor(&dir)?)?)
        }
        "plan_recipe" => {
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

pub(super) fn dispatch_speech_namespace(
    handles: &RpcSubsystemHandles,
    op: &str,
    params: &Value,
) -> Result<Value> {
    let now_ms = params
        .get("now_ms")
        .and_then(Value::as_i64)
        .unwrap_or_else(now_epoch_ms);
    match op {
        "status" => Ok(serde_json::to_value(handles.stt.status())?),
        "record_gpu_failure" => {
            let reason = opt_str(params, "reason").unwrap_or("gpu backend error");
            Ok(serde_json::to_value(
                handles.stt.record_gpu_failure(reason, now_ms)?,
            )?)
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
                .and_then(Value::as_u64)
                .ok_or_else(|| OrcaIdeRuntimeError::invalid_argument("missing 'monotonic_ms'"))?;
            let wall_ms = params
                .get("wall_ms")
                .and_then(Value::as_i64)
                .unwrap_or_else(now_epoch_ms);
            let ctx = opt_str(params, "context").unwrap_or("rpc_tick");
            Ok(serde_json::to_value(
                handles.watchdog.record_tick(monotonic_ms, wall_ms, ctx),
            )?)
        }
        other => Err(OrcaIdeRuntimeError::not_found(format!(
            "unknown watchdog RPC method 'watchdog.{other}'"
        ))),
    }
}
