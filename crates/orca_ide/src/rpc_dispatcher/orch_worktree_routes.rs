//! Unified RPC routing for `orchestration.*`, `agentSession.*`, `terminal.*`,
//! `worktree.*`, and `layout.*`.

use std::collections::BTreeSet;
use std::path::PathBuf;

use orca_core::orchestration_protocol::{
    ExecutionHostRecord, HostSelectionCriteria, OrchestrationTaskStatus,
};
use orca_core::workspace_layout::{
    FolderWorkspace, QuickSwitcherItem, SplitDirection, SplitPaneNode, TabContentType,
    TabLeafState, TabViewMode, WorkspaceLayoutModel, WorkspaceTab, rank_quick_switcher_items,
};
use orca_core::RUNTIME_PROTOCOL_VERSION;
use serde_json::{Value, json};

use super::{MuxStreamState, RegisteredWorktreePort, RpcSubsystemHandles};
use crate::agent_catalog::{AgentCatalog, SlashCommandSurface};
use crate::agent_status_hooks::AgentRunState;
use crate::chat_journal::{
    AgentSessionSnapshot, AppendJournalItemRequest, CreateAgentSessionRequest,
    EnqueueSubmissionRequest,
};
use crate::orchestration_engine::{
    CheckMailboxRequest, CompleteWorkerRequest, CreateGateRequest, CreateRunRequest,
    CreateTaskRequest, ResolveGateAction, SendMessageRequest, StartWorkerRequest,
};
use crate::pty_multiplex::{
    DaemonTransientFact, KillSessionRequest, PtyIncarnationId, PtyMultiplexController,
};
use crate::worktree_manager::{
    CleanupWorktreeRequest, CreateWorktreeRequest, DEFAULT_FIRST_WORK_RENAME_BACKOFF_MS,
    FirstWorkRenameContext, FirstWorkRenameOutcome, ProjectGroup, SparseCheckoutPreset,
    WorktreeIncludeCopyOptions, WorkspaceStatus, execute_worktree_include_copy,
    parse_orca_yaml_text,
};
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

pub(super) fn dispatch_orchestration_namespace(
    handles: &RpcSubsystemHandles,
    op: &str,
    params: &Value,
) -> Result<Value> {
    let eng = &handles.orchestration;
    match op {
        "run_create" => {
            let req: CreateRunRequest = serde_json::from_value(params.clone())?;
            Ok(serde_json::to_value(eng.create_run(req)?)?)
        }
        "run_list" => {
            let status = opt_str(params, "status")
                .map(crate::orchestration_engine::OrchestrationRunStatus::parse)
                .transpose()?;
            Ok(serde_json::to_value(eng.list_runs(status)?)?)
        }
        "run_get" => {
            let run_id = req_str(params, "run_id")?;
            Ok(serde_json::to_value(eng.get_run(run_id)?)?)
        }
        "task_create" => {
            let req: CreateTaskRequest = serde_json::from_value(params.clone())?;
            Ok(serde_json::to_value(eng.create_task(req)?)?)
        }
        "task_transition" => {
            let run_id = req_str(params, "run_id")?;
            let task_id = req_str(params, "task_id")?;
            let next: OrchestrationTaskStatus = serde_json::from_value(
                params
                    .get("status")
                    .or_else(|| params.get("next"))
                    .cloned()
                    .ok_or_else(|| OrcaIdeRuntimeError::invalid_argument("missing task status"))?,
            )?;
            let now_ms = params.get("now_ms").and_then(Value::as_i64);
            let (task, newly_ready) = eng.transition_task(run_id, task_id, next, now_ms)?;
            Ok(json!({ "task": task, "newly_ready": newly_ready }))
        }
        "worker_start" => {
            let req: StartWorkerRequest = serde_json::from_value(params.clone())?;
            Ok(serde_json::to_value(eng.start_worker(req)?)?)
        }
        "worker_done" => {
            let req: CompleteWorkerRequest = serde_json::from_value(params.clone())?;
            Ok(serde_json::to_value(eng.complete_worker(req)?)?)
        }
        "worker_read" => {
            let dispatch_id = req_str(params, "dispatch_id")?;
            Ok(serde_json::to_value(eng.read_worker_archive(dispatch_id)?)?)
        }
        "worker_release" => {
            let dispatch_id = req_str(params, "dispatch_id")?;
            let now_ms = params.get("now_ms").and_then(Value::as_i64);
            let released = eng.release_worker_resources(dispatch_id, now_ms)?;
            Ok(json!({ "dispatch_id": dispatch_id, "released": released }))
        }
        "send" => {
            let req: SendMessageRequest = serde_json::from_value(params.clone())?;
            Ok(serde_json::to_value(eng.send_message(req)?)?)
        }
        "check" => {
            let req: CheckMailboxRequest = serde_json::from_value(params.clone())?;
            Ok(serde_json::to_value(eng.check_mailbox(req)?)?)
        }
        "ask" => {
            let run_id = req_str(params, "run_id")?;
            let dispatch_id = req_str(params, "dispatch_id")?;
            let prompt = req_str(params, "prompt")?;
            let now_ms = params.get("now_ms").and_then(Value::as_i64);
            Ok(serde_json::to_value(
                eng.ask_question(run_id, dispatch_id, prompt, now_ms)?,
            )?)
        }
        "gate_create" => {
            let req: CreateGateRequest = serde_json::from_value(params.clone())?;
            Ok(serde_json::to_value(eng.create_gate(req)?)?)
        }
        "gate_resolve" => {
            let gate_id = req_str(params, "gate_id")?;
            let action: ResolveGateAction = serde_json::from_value(
                params
                    .get("action")
                    .cloned()
                    .unwrap_or_else(|| params.clone()),
            )?;
            let now_ms = params.get("now_ms").and_then(Value::as_i64);
            Ok(serde_json::to_value(
                eng.resolve_gate(gate_id, action, now_ms)?,
            )?)
        }
        "host_register" => {
            let host: ExecutionHostRecord = if let Some(h) = params.get("host") {
                serde_json::from_value(h.clone())?
            } else {
                serde_json::from_value(params.clone())?
            };
            let supports_mail = params
                .get("supports_mail")
                .and_then(Value::as_bool)
                .unwrap_or(true);
            Ok(serde_json::to_value(
                eng.register_host(host, supports_mail)?,
            )?)
        }
        "task.list" | "task_list" => {
            let run_id = req_str(params, "runId").or_else(|_| req_str(params, "run_id"))?;
            let items = eng.list_tasks_with_parent(run_id)?;
            let wire: Vec<Value> = items
                .into_iter()
                .map(|(t, parent_id)| {
                    let status_wire = match t.status {
                        OrchestrationTaskStatus::Succeeded => "completed",
                        OrchestrationTaskStatus::Pending => "pending",
                        OrchestrationTaskStatus::Ready => "ready",
                        OrchestrationTaskStatus::Running => "running",
                        OrchestrationTaskStatus::WaitingGate => "waiting_gate",
                        OrchestrationTaskStatus::Failed => "failed",
                        OrchestrationTaskStatus::Canceled => "canceled",
                    };
                    json!({
                        "id": t.task_id,
                        "taskId": t.task_id,
                        "task_id": t.task_id,
                        "runId": t.run_id,
                        "run_id": t.run_id,
                        "title": t.title,
                        "deps": t.depends_on,
                        "depends_on": t.depends_on,
                        "parentId": parent_id,
                        "parent_id": parent_id,
                        "status": status_wire,
                        "taskStatus": t.status,
                        "assignedHostId": t.assigned_host_id,
                        "assignedDispatchId": t.assigned_dispatch_id,
                        "nestingDepth": t.nesting_depth,
                        "createdAtMs": t.created_at_ms,
                        "updatedAtMs": t.updated_at_ms,
                    })
                })
                .collect();
            Ok(json!({ "tasks": wire }))
        }
        "worker.show" | "worker_show" => {
            let dispatch_id = req_str(params, "dispatchId")
                .or_else(|_| req_str(params, "dispatch_id"))
                .or_else(|_| req_str(params, "id"))?;
            eng.get_worker_show(dispatch_id)
        }
        "federation.registerPeer" | "federation.register_peer" => {
            let host_id = req_str(params, "serverId")
                .or_else(|_| req_str(params, "server_id"))
                .or_else(|_| req_str(params, "hostId"))
                .or_else(|_| req_str(params, "host_id"))?
                .to_string();
            let supports_mail = params
                .get("supportsServerMail")
                .or_else(|| params.get("supports_server_mail"))
                .or_else(|| params.get("supports_mail"))
                .and_then(Value::as_bool)
                .unwrap_or(true);
            let display_name = opt_str(params, "displayName")
                .or_else(|| opt_str(params, "display_name"))
                .unwrap_or(&host_id)
                .to_string();
            let now_ms = params
                .get("now_ms")
                .or_else(|| params.get("nowMs"))
                .and_then(Value::as_i64)
                .unwrap_or_else(now_epoch_ms);
            let capabilities: BTreeSet<String> = params
                .get("capabilities")
                .cloned()
                .map(serde_json::from_value)
                .transpose()?
                .unwrap_or_else(|| {
                    ["claude", "codex", "orchestration", "federation"]
                        .into_iter()
                        .map(ToOwned::to_owned)
                        .collect()
                });
            let record = ExecutionHostRecord {
                host_id: host_id.clone(),
                display_name,
                capabilities,
                online: params.get("online").and_then(Value::as_bool).unwrap_or(true),
                protocol_version: params
                    .get("protocolVersion")
                    .or_else(|| params.get("protocol_version"))
                    .and_then(Value::as_u64)
                    .map(|v| v as u32)
                    .unwrap_or(RUNTIME_PROTOCOL_VERSION),
                active_workers: 0,
                max_workers: params
                    .get("maxWorkers")
                    .or_else(|| params.get("max_workers"))
                    .and_then(Value::as_u64)
                    .map(|v| v as u32)
                    .unwrap_or(16),
                last_heartbeat_ms: now_ms,
            };
            let saved = eng.register_host(record, supports_mail)?;
            Ok(json!({
                "serverId": saved.host_id,
                "hostId": saved.host_id,
                "supportsServerMail": supports_mail,
                "online": saved.online,
                "host": saved,
            }))
        }
        "host_select" => {
            let criteria: HostSelectionCriteria = if let Some(c) = params.get("criteria") {
                serde_json::from_value(c.clone())?
            } else {
                serde_json::from_value(params.clone())?
            };
            let now_ms = params.get("now_ms").and_then(Value::as_i64);
            Ok(serde_json::to_value(eng.select_host(&criteria, now_ms)?)?)
        }
        other => Err(OrcaIdeRuntimeError::not_found(format!(
            "unknown orchestration RPC method 'orchestration.{other}'"
        ))),
    }
}

pub(super) fn dispatch_agent_catalog_namespace(op: &str, params: &Value) -> Result<Value> {
    let catalog = AgentCatalog::new();
    match op {
        "catalog.list" | "catalog_list" | "list" => {
            let entries = catalog.entries();
            let agents: Vec<Value> = entries
                .iter()
                .map(|e| {
                    let mut v = serde_json::to_value(e).unwrap_or_else(|_| json!({}));
                    if let Some(obj) = v.as_object_mut() {
                        obj.insert("agent".into(), json!(e.id));
                    }
                    v
                })
                .collect();
            Ok(json!({
                "agents": agents,
                "botAgentCount": catalog.bot_agents().len(),
                "tuiAgentCount": entries.len(),
            }))
        }
        "probeCapabilities" | "probe_capabilities" => {
            let agent = req_str(params, "agent")
                .or_else(|_| req_str(params, "agentId"))
                .or_else(|_| req_str(params, "agent_id"))?;
            let (entry, migrated_from) = catalog.resolve_agent(agent)?;
            let slash_commands: Vec<Value> = catalog
                .resolve_slash_commands(Some(&entry.id), SlashCommandSurface::Desktop, None)
                .iter()
                .map(|c| serde_json::to_value(c).unwrap_or_else(|_| json!({})))
                .collect();
            let models = if entry.models.is_empty() {
                vec![json!({ "id": format!("{}-default", entry.id), "label": entry.display_name })]
            } else {
                entry
                    .models
                    .iter()
                    .map(|m| serde_json::to_value(m).unwrap_or_else(|_| json!({})))
                    .collect()
            };
            let efforts = if entry.efforts.is_empty() {
                vec!["low".to_string(), "medium".to_string(), "high".to_string()]
            } else {
                entry.efforts.clone()
            };
            Ok(json!({
                "agent": entry.id,
                "displayName": entry.display_name,
                "migratedFrom": migrated_from,
                "models": models,
                "reasoningEfforts": efforts,
                "slashCommands": slash_commands,
                "skills": slash_commands,
                "supportsResume": entry.supports_resume,
                "acpDialect": entry.acp_dialect,
            }))
        }
        other => Err(OrcaIdeRuntimeError::not_found(format!(
            "unknown agent RPC method 'agent.{other}'"
        ))),
    }
}

fn session_snapshot_to_wire_json(snap: &AgentSessionSnapshot) -> Value {
    let mut val = serde_json::to_value(snap).unwrap_or_else(|_| json!({}));
    if let Some(obj) = val.as_object_mut() {
        obj.insert("id".into(), json!(snap.session.session_id));
        obj.insert("sessionId".into(), json!(snap.session.session_id));
        obj.insert("workspaceId".into(), json!(snap.session.workspace_id));
        obj.insert("agentId".into(), json!(snap.session.agent_id));
        obj.insert("agent".into(), json!(snap.session.agent_id));
        obj.insert("viewMode".into(), json!(snap.session.view_mode));
        obj.insert("activeCliOwner".into(), json!(snap.session.active_cli_owner));
        obj.insert(
            "providerHandleChain".into(),
            json!(snap.session.provider_handle_chain),
        );
        obj.insert("parentSessionId".into(), json!(snap.session.parent_session_id));
        obj.insert("epoch".into(), json!(snap.chat_state.epoch));
        obj.insert("lastSequence".into(), json!(snap.chat_state.last_sequence));
        obj.insert("highestFence".into(), json!(snap.chat_state.highest_fence));
        obj.insert(
            "contextBoundary".into(),
            json!(snap.chat_state.context_boundary),
        );
        obj.insert("queueRevision".into(), json!(snap.chat_state.queue_revision));
    }
    val
}

fn parse_view_mode_opt(val: Option<&Value>, default: TabViewMode) -> TabViewMode {
    match val.and_then(Value::as_str).map(|s| s.trim().to_ascii_lowercase()) {
        Some(ref s) if s == "terminal" => TabViewMode::Terminal,
        Some(ref s) if s == "chat" => TabViewMode::Chat,
        _ => default,
    }
}

pub(super) fn dispatch_native_chat_namespace(
    handles: &RpcSubsystemHandles,
    op: &str,
    params: &Value,
) -> Result<Value> {
    let journal = &handles.chat_journal;
    match op {
        "toggleViewMode" | "toggle_view_mode" => {
            let session_id = req_str(params, "sessionId").or_else(|_| req_str(params, "session_id"))?;
            let new_handle = opt_str(params, "newProviderHandle")
                .or_else(|| opt_str(params, "new_provider_handle"))
                .map(ToOwned::to_owned);
            let now_ms = params
                .get("nowMs")
                .or_else(|| params.get("now_ms"))
                .and_then(Value::as_i64);
            let snap = if let Some(vm_val) = params.get("viewMode").or_else(|| params.get("view_mode")).or_else(|| params.get("mode")) {
                let target = parse_view_mode_opt(Some(vm_val), TabViewMode::Chat);
                journal.set_view_mode(session_id, target, new_handle, now_ms)?
            } else {
                journal.toggle_view_mode(session_id, new_handle, now_ms)?
            };
            Ok(session_snapshot_to_wire_json(&snap))
        }
        other => Err(OrcaIdeRuntimeError::not_found(format!(
            "unknown nativeChat RPC method 'nativeChat.{other}'"
        ))),
    }
}

pub(super) fn dispatch_agent_session_namespace(
    handles: &RpcSubsystemHandles,
    op: &str,
    params: &Value,
    envelope_operation_id: Option<&str>,
) -> Result<Value> {
    let journal = &handles.chat_journal;
    let now_ms = params
        .get("nowMs")
        .or_else(|| params.get("now_ms"))
        .and_then(Value::as_i64)
        .unwrap_or_else(now_epoch_ms);

    match op {
        "create" => {
            let req = if let Ok(mut parsed) = serde_json::from_value::<CreateAgentSessionRequest>(params.clone()) {
                if parsed.provider_handle_chain.is_empty() {
                    parsed.provider_handle_chain.push(format!(
                        "hdl-{}-{}",
                        parsed.agent_id,
                        uuid::Uuid::new_v4().simple()
                    ));
                }
                parsed
            } else {
                let agent_id = req_str(params, "agent")
                    .or_else(|_| req_str(params, "agentId"))
                    .or_else(|_| req_str(params, "agent_id"))?
                    .to_string();
                let workspace_id = opt_str(params, "workspaceId")
                    .or_else(|| opt_str(params, "workspace_id"))
                    .or_else(|| opt_str(params, "workspacePath"))
                    .unwrap_or("default")
                    .to_string();
                let view_mode = parse_view_mode_opt(
                    params.get("viewMode").or_else(|| params.get("view_mode")),
                    TabViewMode::Chat,
                );
                let mut chain: Vec<String> = params
                    .get("providerHandleChain")
                    .or_else(|| params.get("provider_handle_chain"))
                    .cloned()
                    .map(serde_json::from_value)
                    .transpose()?
                    .unwrap_or_default();
                if chain.is_empty() {
                    chain.push(format!("hdl-{agent_id}-{}", uuid::Uuid::new_v4().simple()));
                }
                CreateAgentSessionRequest {
                    session_id: opt_str(params, "sessionId")
                        .or_else(|| opt_str(params, "session_id"))
                        .map(ToOwned::to_owned),
                    workspace_id,
                    host_id: opt_str(params, "hostId")
                        .or_else(|| opt_str(params, "host_id"))
                        .map(ToOwned::to_owned),
                    agent_id,
                    view_mode,
                    active_cli_owner: params
                        .get("activeCliOwner")
                        .or_else(|| params.get("active_cli_owner"))
                        .and_then(Value::as_bool)
                        .unwrap_or(false),
                    provider_handle_chain: chain,
                    adopted_native_id: opt_str(params, "adoptedNativeId").map(ToOwned::to_owned),
                    now_ms: Some(now_ms),
                }
            };
            let snap = journal.create_session(req)?;
            Ok(session_snapshot_to_wire_json(&snap))
        }
        "send" | "submit" => {
            let session_id = req_str(params, "sessionId").or_else(|_| req_str(params, "session_id"))?;
            let text = opt_str(params, "text")
                .or_else(|| opt_str(params, "promptText"))
                .or_else(|| opt_str(params, "prompt_text"))
                .unwrap_or("")
                .to_string();
            let caller_key = opt_str(params, "callerKey")
                .or_else(|| opt_str(params, "caller_key"))
                .unwrap_or("default-caller")
                .to_string();
            let op_id = opt_str(params, "operationId")
                .or_else(|| opt_str(params, "operation_id"))
                .or(envelope_operation_id)
                .map(ToOwned::to_owned)
                .unwrap_or_else(|| format!("op-{}", uuid::Uuid::new_v4().simple()));
            let mode = opt_str(params, "mode").unwrap_or("normal");
            let simulate_working = params
                .get("simulateWorking")
                .or_else(|| params.get("simulate_working"))
                .and_then(Value::as_bool)
                .unwrap_or(false);
            let attachments: Vec<Value> = params
                .get("attachments")
                .cloned()
                .map(serde_json::from_value)
                .transpose()?
                .unwrap_or_default();

            let is_queue_mode = mode.eq_ignore_ascii_case("queue");
            if is_queue_mode {
                let sub = journal.enqueue_followup_submission(
                    session_id,
                    &op_id,
                    &text,
                    attachments,
                    Some(now_ms),
                )?;
                let snap = journal.get_session(session_id)?;
                return Ok(json!({
                    "sessionId": session_id,
                    "attemptId": sub.attempt_id,
                    "queued": true,
                    "current": sub.current,
                    "sequence": snap.chat_state.last_sequence,
                    "queueRevision": snap.chat_state.queue_revision,
                }));
            }

            let (attempt, replayed) = journal.enqueue_submission(EnqueueSubmissionRequest {
                session_id: session_id.to_string(),
                operation_id: op_id.clone(),
                caller_key,
                prompt_text: text.clone(),
                attachments,
                now_ms: Some(now_ms),
            })?;
            let seq = if !replayed {
                let item = journal.append_item(AppendJournalItemRequest {
                    session_id: session_id.to_string(),
                    item_id: Some(format!("msg-{}", attempt.attempt_id)),
                    item_kind: "user_message".to_string(),
                    provider_item_id: None,
                    payload_json: json!({
                        "role": "user",
                        "text": text,
                        "content": text,
                        "operationId": op_id,
                    }),
                    now_ms: Some(now_ms),
                })?;
                if !simulate_working {
                    let _ = journal.complete_submission(session_id, &attempt.attempt_id, Some(now_ms));
                }
                item.sequence
            } else {
                let snap = journal.get_session(session_id)?;
                let existing_items = journal.list_items(session_id, false).unwrap_or_default();
                existing_items
                    .iter()
                    .find(|i| i.item_id == format!("msg-{}", attempt.attempt_id))
                    .map(|i| i.sequence)
                    .unwrap_or(snap.chat_state.last_sequence.max(1))
            };
            let snap = journal.get_session(session_id)?;
            Ok(json!({
                "sessionId": session_id,
                "attemptId": attempt.attempt_id,
                "id": attempt.attempt_id,
                "replayed": replayed,
                "sequence": seq,
                "epoch": snap.chat_state.epoch,
                "lastSequence": snap.chat_state.last_sequence,
                "queueRevision": snap.chat_state.queue_revision,
            }))
        }
        "getState" | "get_state" | "get" => {
            let session_id = req_str(params, "sessionId").or_else(|_| req_str(params, "session_id"))?;
            let snap = journal.get_session(session_id)?;
            let subs = journal.list_submissions(session_id)?;
            let mut val = session_snapshot_to_wire_json(&snap);
            if let Some(obj) = val.as_object_mut() {
                obj.insert("submissions".into(), json!(subs));
                obj.insert("queue".into(), json!(subs));
            }
            Ok(val)
        }
        "getTranscript" | "get_transcript" => {
            let session_id = req_str(params, "sessionId").or_else(|_| req_str(params, "session_id"))?;
            let snap = journal.get_session(session_id)?;
            let raw_items = journal.list_items(session_id, false)?;
            let items: Vec<Value> = raw_items
                .iter()
                .map(|it| {
                    let mut m = serde_json::Map::new();
                    if let Some(pobj) = it.payload_json.as_object() {
                        for (k, v) in pobj {
                            m.insert(k.clone(), v.clone());
                        }
                    }
                    m.insert("id".into(), json!(it.item_id));
                    m.insert("itemId".into(), json!(it.item_id));
                    m.insert("sequence".into(), json!(it.sequence));
                    m.insert("epoch".into(), json!(it.epoch));
                    m.insert("kind".into(), json!(it.item_kind));
                    m.insert("itemKind".into(), json!(it.item_kind));
                    m.insert("payload".into(), it.payload_json.clone());
                    Value::Object(m)
                })
                .collect();
            Ok(json!({
                "sessionId": session_id,
                "epoch": snap.chat_state.epoch,
                "lastSequence": snap.chat_state.last_sequence,
                "items": items,
                "transcript": items,
            }))
        }
        "contextBoundary" | "context_boundary" => {
            let session_id = req_str(params, "sessionId").or_else(|_| req_str(params, "session_id"))?;
            let reason = opt_str(params, "reason");
            let new_handle = opt_str(params, "newProviderHandle").map(ToOwned::to_owned);
            let snap = journal.record_context_boundary(session_id, reason, new_handle, Some(now_ms))?;
            Ok(session_snapshot_to_wire_json(&snap))
        }
        "continueInTerminal" | "continue_in_terminal" => {
            let session_id = req_str(params, "sessionId").or_else(|_| req_str(params, "session_id"))?;
            let snap = journal.continue_in_new_session(
                session_id,
                TabViewMode::Terminal,
                false,
                true,
                false,
                Some(now_ms),
            )?;
            Ok(session_snapshot_to_wire_json(&snap))
        }
        "stopTerminalOwner" | "stop_terminal_owner" | "setActiveCliOwner" => {
            let session_id = req_str(params, "sessionId").or_else(|_| req_str(params, "session_id"))?;
            let active = params
                .get("active")
                .and_then(Value::as_bool)
                .unwrap_or(false);
            let rec = journal.set_active_cli_owner(session_id, active, Some(now_ms))?;
            Ok(json!({
                "sessionId": rec.session_id,
                "activeCliOwner": rec.active_cli_owner,
            }))
        }
        "continueInChat" | "continue_in_chat" => {
            let session_id = req_str(params, "sessionId").or_else(|_| req_str(params, "session_id"))?;
            let snap = journal.continue_in_new_session(
                session_id,
                TabViewMode::Chat,
                true,
                false,
                true,
                Some(now_ms),
            )?;
            Ok(session_snapshot_to_wire_json(&snap))
        }
        "emitInteractivePrompt" | "emit_interactive_prompt" => {
            let session_id = req_str(params, "sessionId").or_else(|_| req_str(params, "session_id"))?;
            let kind = opt_str(params, "kind").unwrap_or("ask_user_question");
            let questions = params.get("questions").cloned().unwrap_or_else(|| json!([]));
            let q_id = format!("q-{}", uuid::Uuid::new_v4().simple());
            let item = journal.append_item(AppendJournalItemRequest {
                session_id: session_id.to_string(),
                item_id: Some(q_id.clone()),
                item_kind: kind.to_string(),
                provider_item_id: None,
                payload_json: json!({
                    "questionId": q_id,
                    "kind": kind,
                    "status": "pending",
                    "questions": questions,
                }),
                now_ms: Some(now_ms),
            })?;
            Ok(json!({
                "id": item.item_id,
                "questionId": item.item_id,
                "sessionId": session_id,
                "kind": kind,
                "status": "pending",
                "sequence": item.sequence,
            }))
        }
        "answerQuestion" | "answer_question" => {
            let session_id = req_str(params, "sessionId").or_else(|_| req_str(params, "session_id"))?;
            let q_id = req_str(params, "questionId")
                .or_else(|_| req_str(params, "question_id"))
                .or_else(|_| req_str(params, "id"))?;
            let answers = params.get("answers").cloned().unwrap_or_else(|| json!({}));
            let raw_items = journal.list_items(session_id, false)?;
            let mut payload = raw_items
                .iter()
                .find(|i| i.item_id == q_id)
                .map(|i| i.payload_json.clone())
                .unwrap_or_else(|| json!({ "questionId": q_id, "kind": "ask_user_question" }));
            if let Some(obj) = payload.as_object_mut() {
                obj.insert("status".into(), json!("answered"));
                obj.insert("answers".into(), answers.clone());
            }
            let updated = journal.update_item_payload(session_id, q_id, payload, Some(now_ms))?;
            Ok(json!({
                "id": updated.item_id,
                "questionId": updated.item_id,
                "sessionId": session_id,
                "status": "answered",
                "answers": answers,
            }))
        }
        "saveDraft" | "save_draft" => {
            let session_id = req_str(params, "sessionId").or_else(|_| req_str(params, "session_id"))?;
            let text = params.get("text").and_then(Value::as_str).unwrap_or("");
            let attachments: Vec<Value> = params
                .get("attachments")
                .cloned()
                .map(serde_json::from_value)
                .transpose()?
                .unwrap_or_default();
            let (saved_text, saved_att) =
                journal.save_draft_with_attachments(session_id, text, &attachments, Some(now_ms))?;
            Ok(json!({
                "sessionId": session_id,
                "text": saved_text,
                "attachments": saved_att,
            }))
        }
        "getDraft" | "get_draft" => {
            let session_id = req_str(params, "sessionId").or_else(|_| req_str(params, "session_id"))?;
            let (text, attachments) = journal.get_draft_with_attachments(session_id)?;
            Ok(json!({
                "sessionId": session_id,
                "text": text,
                "attachments": attachments,
            }))
        }
        "list" => {
            let ws = opt_str(params, "workspace_id").or_else(|| opt_str(params, "workspaceId"));
            Ok(serde_json::to_value(journal.list_sessions(ws)?)?)
        }
        "append" => {
            let req: AppendJournalItemRequest = serde_json::from_value(params.clone())?;
            Ok(serde_json::to_value(journal.append_item(req)?)?)
        }
        "flip" | "toggleViewMode" | "toggle_view_mode" => {
            let session_id = req_str(params, "session_id").or_else(|_| req_str(params, "sessionId"))?;
            let new_handle = opt_str(params, "new_provider_handle")
                .or_else(|| opt_str(params, "newProviderHandle"))
                .map(ToOwned::to_owned);
            let snap = if let Some(vm) = params.get("viewMode").or_else(|| params.get("mode")) {
                journal.set_view_mode(
                    session_id,
                    parse_view_mode_opt(Some(vm), TabViewMode::Chat),
                    new_handle,
                    Some(now_ms),
                )?
            } else {
                journal.toggle_view_mode(session_id, new_handle, Some(now_ms))?
            };
            Ok(session_snapshot_to_wire_json(&snap))
        }
        "resume" | "resumeInNewSession" => {
            let src = req_str(params, "source_session_id")
                .or_else(|_| req_str(params, "sessionId"))
                .or_else(|_| req_str(params, "session_id"))?;
            let target_mode = parse_view_mode_opt(
                params
                    .get("target_view_mode")
                    .or_else(|| params.get("viewMode")),
                TabViewMode::Chat,
            );
            let new_id = opt_str(params, "new_session_id")
                .or_else(|| opt_str(params, "newSessionId"))
                .map(ToOwned::to_owned);
            let snap = journal.resume_in_new_session(src, target_mode, new_id, Some(now_ms))?;
            Ok(session_snapshot_to_wire_json(&snap))
        }
        "fork" => {
            let src = req_str(params, "source_session_id")
                .or_else(|_| req_str(params, "sessionId"))
                .or_else(|_| req_str(params, "session_id"))?;
            let up_to = params
                .get("up_to_sequence")
                .or_else(|| params.get("upToSequence"))
                .and_then(Value::as_i64);
            let new_id = opt_str(params, "new_session_id")
                .or_else(|| opt_str(params, "newSessionId"))
                .map(ToOwned::to_owned);
            let snap = journal.fork_session(src, up_to, new_id, Some(now_ms))?;
            Ok(session_snapshot_to_wire_json(&snap))
        }
        "catalog" => {
            let catalog = AgentCatalog::new();
            Ok(json!({
                "bot_agent_count": catalog.bot_agents().len(),
                "tui_agent_count": catalog.entries().len(),
                "botAgents": catalog.bot_agents(),
                "tuiAgents": catalog.entries(),
                "entries": catalog.entries(),
            }))
        }
        other => Err(OrcaIdeRuntimeError::not_found(format!(
            "unknown agentSession RPC method 'agentSession.{other}'"
        ))),
    }
}

fn transient_fact_to_wire_json(fact: &DaemonTransientFact) -> Value {
    match fact {
        DaemonTransientFact::Bell => json!({ "kind": "bell", "type": "bell" }),
        DaemonTransientFact::TitleChanged { title } => {
            json!({ "kind": "title-changed", "type": "title-changed", "title": title })
        }
        DaemonTransientFact::CommandFinished { exit_code } => json!({
            "kind": "command-finished",
            "type": "command-finished",
            "exitCode": exit_code,
            "exit_code": exit_code,
        }),
        DaemonTransientFact::PrLink { url } => {
            json!({ "kind": "pr-link", "type": "pr-link", "url": url })
        }
        DaemonTransientFact::Mode2031Subscribe => {
            json!({ "kind": "2031-subscribe", "type": "2031-subscribe" })
        }
        DaemonTransientFact::Mode2031Unsubscribe => {
            json!({ "kind": "2031-unsubscribe", "type": "2031-unsubscribe" })
        }
        DaemonTransientFact::ForegroundChanged { process } => {
            json!({ "kind": "foreground-changed", "type": "foreground-changed", "process": process })
        }
        DaemonTransientFact::Osc52ClipboardWrite { text } => json!({
            "kind": "osc52-clipboard",
            "type": "osc52-clipboard",
            "direction": "write",
            "text": text,
        }),
    }
}

pub(super) fn dispatch_terminal_namespace(
    handles: &RpcSubsystemHandles,
    op: &str,
    params: &Value,
) -> Result<Value> {
    let pty = &handles.pty;
    let now_ms = params
        .get("now_ms")
        .or_else(|| params.get("nowMs"))
        .and_then(Value::as_i64)
        .unwrap_or_else(now_epoch_ms);

    match op {
        "create" => {
            let tab_id = opt_str(params, "tabId")
                .or_else(|| opt_str(params, "tab_id"))
                .or_else(|| opt_str(params, "sessionId"))
                .map(ToOwned::to_owned)
                .unwrap_or_else(|| format!("tab-pty-{}", uuid::Uuid::new_v4().simple()));
            if handles.closed_tab_tombstones.read().contains_key(&tab_id) {
                return Err(OrcaIdeRuntimeError::ide(
                    orca_core::OrcaIdeErrorCode::TabNotFound,
                    format!("tab '{tab_id}' has been closed and tombstoned"),
                ));
            }
            let pane_id = opt_str(params, "paneId")
                .or_else(|| opt_str(params, "pane_id"))
                .unwrap_or("main");
            let title = opt_str(params, "title").unwrap_or("Terminal");
            let cols = params.get("cols").and_then(Value::as_u64).map(|v| v as usize);
            let rows = params.get("rows").and_then(Value::as_u64).map(|v| v as usize);
            let allow_osc52 = params
                .get("allowOsc52Clipboard")
                .or_else(|| params.get("allow_osc52_clipboard"))
                .and_then(Value::as_bool)
                .unwrap_or(false);
            let inc = pty.create_session_with_options(
                tab_id.clone(),
                pane_id,
                title,
                cols,
                rows,
                allow_osc52,
                now_ms,
            )?;
            let _ = PtyMultiplexController::spawn_shell_for_session(pty, &tab_id);
            let (actual_cols, actual_rows) = pty.session_dimensions(&tab_id).unwrap_or((80, 24));
            Ok(json!({
                "id": tab_id,
                "sessionId": tab_id,
                "tab_id": tab_id,
                "tabId": tab_id,
                "incarnationId": inc.0,
                "incarnation_id": inc,
                "cols": actual_cols,
                "rows": actual_rows,
            }))
        }
        "write" => {
            let tab_id = req_str(params, "sessionId")
                .or_else(|_| req_str(params, "tabId"))
                .or_else(|_| req_str(params, "tab_id"))?;
            let data = params
                .get("data")
                .and_then(Value::as_str)
                .unwrap_or_default();
            let raw_output = params
                .get("rawOutput")
                .or_else(|| params.get("raw_output"))
                .and_then(Value::as_bool)
                .unwrap_or(false);
            if raw_output {
                let facts = pty.ingest_pty_output(tab_id, data.as_bytes())?;
                Ok(json!({
                    "sessionId": tab_id,
                    "written": data.len(),
                    "facts": facts.iter().map(transient_fact_to_wire_json).collect::<Vec<_>>(),
                }))
            } else {
                pty.write_stdin(tab_id, data.as_bytes())?;
                Ok(json!({
                    "sessionId": tab_id,
                    "written": data.len(),
                }))
            }
        }
        "resize" => {
            let tab_id = req_str(params, "sessionId")
                .or_else(|_| req_str(params, "tabId"))
                .or_else(|_| req_str(params, "tab_id"))?;
            let cols = params
                .get("cols")
                .and_then(Value::as_u64)
                .ok_or_else(|| OrcaIdeRuntimeError::invalid_argument("missing 'cols'"))?
                as usize;
            let rows = params
                .get("rows")
                .and_then(Value::as_u64)
                .ok_or_else(|| OrcaIdeRuntimeError::invalid_argument("missing 'rows'"))?
                as usize;
            let (c, r) = pty.resize_session(tab_id, cols, rows)?;
            Ok(json!({ "sessionId": tab_id, "cols": c, "rows": r }))
        }
        "takePendingOutput" | "take_pending_output" => {
            let tab_id = req_str(params, "sessionId")
                .or_else(|_| req_str(params, "tabId"))
                .or_else(|_| req_str(params, "tab_id"))?;
            let cursor = params.get("cursor").and_then(Value::as_u64).unwrap_or(0);
            let chunk = pty.take_pending_output(tab_id, cursor)?;
            Ok(json!({
                "sessionId": tab_id,
                "data": String::from_utf8_lossy(&chunk.data),
                "nextCursor": chunk.next_cursor,
                "truncated": chunk.truncated,
            }))
        }
        "getSnapshot" | "get_snapshot" | "snapshot" => {
            let tab_id = req_str(params, "sessionId")
                .or_else(|_| req_str(params, "tabId"))
                .or_else(|_| req_str(params, "tab_id"))?;
            let text = pty.snapshot_screen(tab_id)?;
            let (cols, rows) = pty.session_dimensions(tab_id)?;
            Ok(json!({
                "sessionId": tab_id,
                "snapshot": text,
                "text": text,
                "cols": cols,
                "rows": rows,
            }))
        }
        "takeSideEffects" | "take_side_effects" => {
            let tab_id = req_str(params, "sessionId")
                .or_else(|_| req_str(params, "tabId"))
                .or_else(|_| req_str(params, "tab_id"))?;
            let facts = pty.take_side_effects(tab_id)?;
            let wire: Vec<Value> = facts.iter().map(transient_fact_to_wire_json).collect();
            Ok(json!({
                "sessionId": tab_id,
                "effects": wire,
                "facts": wire,
            }))
        }
        "setSessionBackground" | "set_session_background" | "park" => {
            let tab_id = req_str(params, "sessionId")
                .or_else(|_| req_str(params, "tabId"))
                .or_else(|_| req_str(params, "tab_id"))?;
            let hidden = params
                .get("background")
                .or_else(|| params.get("hidden"))
                .and_then(Value::as_bool)
                .unwrap_or(true);
            pty.set_session_background(tab_id, hidden)?;
            Ok(json!({
                "sessionId": tab_id,
                "tab_id": tab_id,
                "background": hidden,
                "hidden": hidden,
                "deliveryGated": hidden,
            }))
        }
        "getViewState" | "get_view_state" => {
            let tab_id = req_str(params, "sessionId")
                .or_else(|_| req_str(params, "tabId"))
                .or_else(|_| req_str(params, "tab_id"))?;
            let (bg, pushed) = pty.get_view_state(tab_id)?;
            Ok(json!({
                "sessionId": tab_id,
                "background": bg,
                "deliveryGated": bg,
                "pushedRawBytesWhileHidden": pushed,
            }))
        }
        "detach" => {
            let tab_id = req_str(params, "sessionId")
                .or_else(|_| req_str(params, "tabId"))
                .or_else(|_| req_str(params, "tab_id"))?;
            if let Some(cid) = opt_str(params, "clientId").or_else(|| opt_str(params, "client_id")) {
                pty.detach_session(tab_id, cid)?;
            } else {
                pty.detach_all_clients(tab_id)?;
            }
            Ok(json!({ "sessionId": tab_id, "detached": true }))
        }
        "multiplex.open" => {
            let session_ids: Vec<String> = params
                .get("sessionIds")
                .or_else(|| params.get("session_ids"))
                .cloned()
                .map(serde_json::from_value)
                .transpose()?
                .unwrap_or_default();
            let window_bytes = params
                .get("windowBytes")
                .or_else(|| params.get("window_bytes"))
                .and_then(Value::as_u64)
                .unwrap_or(65_536) as usize;
            let stream_id = format!("mux-{}", uuid::Uuid::new_v4().simple());
            handles.mux_streams.write().insert(
                stream_id.clone(),
                MuxStreamState {
                    stream_id: stream_id.clone(),
                    session_ids: session_ids.clone(),
                    window_bytes,
                    available_credits: window_bytes,
                    overflow_count: 0,
                },
            );
            Ok(json!({
                "id": stream_id,
                "streamId": stream_id,
                "sessionIds": session_ids,
                "windowBytes": window_bytes,
                "availableCredits": window_bytes,
            }))
        }
        "multiplex.poll" => {
            let stream_id = req_str(params, "streamId").or_else(|_| req_str(params, "stream_id"))?;
            let max_bytes = params
                .get("maxBytes")
                .or_else(|| params.get("max_bytes"))
                .and_then(Value::as_u64)
                .unwrap_or(65_536) as usize;
            let mut streams = handles.mux_streams.write();
            let st = streams.get_mut(stream_id).ok_or_else(|| {
                OrcaIdeRuntimeError::not_found(format!("multiplex stream '{stream_id}' not found"))
            })?;
            let frames = pty.drain_round_robin(max_bytes);
            let mut chunks = Vec::new();
            for f in frames {
                if !st.session_ids.is_empty() && !st.session_ids.contains(&f.tab_id) {
                    continue;
                }
                if f.dropped_bytes > 0 {
                    st.overflow_count = st.overflow_count.saturating_add(1);
                }
                let data_str = String::from_utf8_lossy(&f.payload).to_string();
                st.available_credits = st.available_credits.saturating_sub(f.payload.len());
                chunks.push(json!({
                    "sessionId": f.tab_id,
                    "tabId": f.tab_id,
                    "incarnationId": f.incarnation_id.0,
                    "kind": f.kind,
                    "data": data_str,
                    "droppedBytes": f.dropped_bytes,
                }));
            }
            Ok(json!({
                "streamId": stream_id,
                "chunks": chunks,
                "frames": chunks,
                "availableCredits": st.available_credits,
                "overflowCount": st.overflow_count,
            }))
        }
        "ackOutput" | "ack_output" => {
            let stream_id = opt_str(params, "streamId").or_else(|| opt_str(params, "stream_id"));
            let acked_bytes = params
                .get("ackedBytes")
                .or_else(|| params.get("acked_bytes"))
                .and_then(Value::as_u64)
                .unwrap_or(0) as usize;
            let rtt_ms = params
                .get("rttMs")
                .or_else(|| params.get("rtt_ms"))
                .and_then(Value::as_u64)
                .unwrap_or(10);
            if let Some(sid) = stream_id {
                let mut streams = handles.mux_streams.write();
                let st = streams.get_mut(sid).ok_or_else(|| {
                    OrcaIdeRuntimeError::not_found(format!("multiplex stream '{sid}' not found"))
                })?;
                st.available_credits = st
                    .available_credits
                    .saturating_add(acked_bytes)
                    .min(st.window_bytes);
                let mut link_est = pty.link_estimate_bps();
                for tab_id in &st.session_ids {
                    if let Ok(stats) = pty.ack(tab_id, acked_bytes, rtt_ms) {
                        link_est = stats.link_estimate_bps;
                    }
                }
                Ok(json!({
                    "streamId": sid,
                    "availableCredits": st.available_credits,
                    "linkEstimate": link_est,
                    "overflowCount": st.overflow_count,
                }))
            } else {
                let tab_id = req_str(params, "sessionId").or_else(|_| req_str(params, "tab_id"))?;
                let stats = pty.ack(tab_id, acked_bytes, rtt_ms)?;
                Ok(json!({
                    "sessionId": stats.tab_id,
                    "availableCredits": stats.credit_remaining,
                    "linkEstimate": stats.link_estimate_bps,
                    "overflowCount": 0,
                }))
            }
        }
        "output" => {
            let tab_id = req_str(params, "tab_id").or_else(|_| req_str(params, "sessionId"))?;
            let data = params
                .get("data")
                .and_then(Value::as_str)
                .unwrap_or_default();
            let facts = pty.ingest_pty_output(tab_id, data.as_bytes())?;
            Ok(json!({ "tab_id": tab_id, "facts": facts }))
        }
        "drain" => {
            let max_bytes = params
                .get("max_bytes")
                .and_then(Value::as_u64)
                .unwrap_or(65_536) as usize;
            let frames = pty.drain_round_robin(max_bytes);
            let encoded: Vec<Value> = frames
                .into_iter()
                .map(|f| {
                    json!({
                        "tab_id": f.tab_id,
                        "incarnation_id": f.incarnation_id,
                        "kind": f.kind,
                        "payload_utf8": String::from_utf8_lossy(&f.payload),
                        "dropped_bytes": f.dropped_bytes,
                    })
                })
                .collect();
            Ok(json!({ "frames": encoded }))
        }
        "ack" => {
            let tab_id = req_str(params, "tab_id").or_else(|_| req_str(params, "sessionId"))?;
            let acked_bytes = params
                .get("acked_bytes")
                .and_then(Value::as_u64)
                .unwrap_or(0) as usize;
            let rtt_ms = params.get("rtt_ms").and_then(Value::as_u64).unwrap_or(10);
            let stats = pty.ack(tab_id, acked_bytes, rtt_ms)?;
            Ok(json!({
                "tab_id": stats.tab_id,
                "credit_remaining": stats.credit_remaining,
                "unacked_bytes": stats.unacked_bytes,
                "link_estimate_bps": stats.link_estimate_bps,
            }))
        }
        "kill" | "close" => {
            let tab_id = req_str(params, "tab_id")
                .or_else(|_| req_str(params, "sessionId"))
                .or_else(|_| req_str(params, "tabId"))?
                .to_string();
            let expected_incarnation_id = opt_str(params, "expected_incarnation_id")
                .or_else(|| opt_str(params, "expectedIncarnationId"))
                .map(|s| PtyIncarnationId(s.to_string()));
            let inc_str = expected_incarnation_id.as_ref().map(|i| i.0.clone());
            let require_no_agent_owner = params
                .get("require_no_agent_owner")
                .or_else(|| params.get("requireNoAgentOwner"))
                .and_then(Value::as_bool)
                .unwrap_or(false);
            let require_unattached = params
                .get("require_unattached")
                .or_else(|| params.get("requireUnattached"))
                .and_then(Value::as_bool)
                .unwrap_or(false);
            let tombstone = pty.kill_session(&KillSessionRequest {
                tab_id,
                expected_incarnation_id,
                require_no_agent_owner,
                require_unattached,
                now_ms,
            })?;
            let mut val = serde_json::to_value(&tombstone)?;
            if let Some(obj) = val.as_object_mut() {
                obj.insert("killed".into(), json!(true));
                if let Some(inc) = inc_str {
                    obj.insert("incarnationId".into(), json!(inc));
                }
            }
            Ok(val)
        }
        "tombstones" => Ok(serde_json::to_value(pty.list_tombstones())?),
        "hook_status" => {
            if let Some(session_id) = opt_str(params, "session_id").or_else(|| opt_str(params, "sessionId")) {
                Ok(serde_json::to_value(
                    handles.agent_status.get_status(session_id),
                )?)
            } else {
                Ok(serde_json::to_value(handles.agent_status.list_statuses())?)
            }
        }
        other => Err(OrcaIdeRuntimeError::not_found(format!(
            "unknown terminal RPC method 'terminal.{other}'"
        ))),
    }
}

pub(super) fn dispatch_worktree_namespace(
    handles: &RpcSubsystemHandles,
    op: &str,
    params: &Value,
) -> Result<Value> {
    let wt = &handles.worktrees;
    let now_ms = params
        .get("now_ms")
        .and_then(Value::as_i64)
        .unwrap_or_else(now_epoch_ms);

    match op {
        "parse_yaml" => {
            let raw = req_str(params, "yaml").or_else(|_| req_str(params, "content"))?;
            let cfg = parse_orca_yaml_text(raw)?;
            Ok(serde_json::to_value(cfg)?)
        }
        "include_copy" | "includeCopy" => {
            let src_raw = req_str(params, "source_repo_root")
                .or_else(|_| req_str(params, "sourcePath"))
                .or_else(|_| req_str(params, "source"))?;
            let dst_raw = req_str(params, "target_worktree_root")
                .or_else(|_| req_str(params, "destPath"))
                .or_else(|_| req_str(params, "dest"))?;
            let src = PathBuf::from(src_raw);
            let dst = PathBuf::from(dst_raw);
            let rules = if let Some(r) = params.get("rules").and_then(Value::as_str) {
                r.to_string()
            } else {
                let inc = src.join(".worktreeinclude");
                if inc.is_file() {
                    std::fs::read_to_string(&inc).unwrap_or_default()
                } else {
                    String::new()
                }
            };
            let options: WorktreeIncludeCopyOptions = params
                .get("options")
                .cloned()
                .map(serde_json::from_value)
                .transpose()?
                .unwrap_or_default();
            let report = execute_worktree_include_copy(&src, &dst, &rules, &options)?;
            let mut val = serde_json::to_value(&report)?;
            if let Some(obj) = val.as_object_mut() {
                obj.insert("copiedCount".into(), json!(report.copied_files.len()));
                obj.insert("copiedFiles".into(), json!(report.copied_files));
                obj.insert("skippedSymlinks".into(), json!(report.skipped_symlinks));
                obj.insert("warnings".into(), json!(report.skipped_symlinks));
            }
            Ok(val)
        }
        "firstWorkRename" | "first_work_rename" => {
            let worktree_id = req_str(params, "worktreeId")
                .or_else(|_| req_str(params, "worktree_id"))
                .or_else(|_| req_str(params, "id"))?
                .to_string();
            let state_raw = opt_str(params, "agentState")
                .or_else(|| opt_str(params, "agent_state"))
                .unwrap_or("working");
            let agent_state = match state_raw.trim().to_ascii_lowercase().as_str() {
                "working" | "running" => AgentRunState::Working,
                "waiting" => AgentRunState::Waiting,
                "blocked" => AgentRunState::Blocked,
                "done" => AgentRunState::Done,
                _ => AgentRunState::Idle,
            };
            let prompt_summary = opt_str(params, "prompt")
                .or_else(|| opt_str(params, "promptSummary"))
                .or_else(|| opt_str(params, "prompt_summary"))
                .unwrap_or("")
                .to_string();
            let existing_branches: BTreeSet<String> = params
                .get("existingBranches")
                .or_else(|| params.get("existing_branches"))
                .cloned()
                .map(serde_json::from_value)
                .transpose()?
                .unwrap_or_default();
            let ctx = FirstWorkRenameContext {
                worktree_id: worktree_id.clone(),
                agent_state,
                prompt_summary,
                checkout_locked: params
                    .get("checkoutLocked")
                    .or_else(|| params.get("checkout_locked"))
                    .and_then(Value::as_bool)
                    .unwrap_or(false),
                upstream_published: params
                    .get("upstreamPublished")
                    .or_else(|| params.get("upstream_published"))
                    .and_then(Value::as_bool)
                    .unwrap_or(false),
                existing_branches,
                rename_directory_on_disk: params
                    .get("renameDirectoryOnDisk")
                    .or_else(|| params.get("rename_directory_on_disk"))
                    .and_then(Value::as_bool)
                    .unwrap_or(false),
                base_backoff_ms: DEFAULT_FIRST_WORK_RENAME_BACKOFF_MS,
                now_ms,
            };
            match wt.evaluate_and_apply_first_work_rename(&ctx)? {
                FirstWorkRenameOutcome::Renamed {
                    old_branch,
                    new_branch,
                    old_path,
                    new_path,
                    new_title,
                } => Ok(json!({
                    "renamed": true,
                    "worktreeId": worktree_id,
                    "name": new_branch,
                    "branch": new_branch,
                    "slug": new_branch,
                    "oldBranch": old_branch,
                    "newBranch": new_branch,
                    "oldPath": old_path,
                    "newPath": new_path,
                    "title": new_title,
                })),
                FirstWorkRenameOutcome::Skipped { reason } => Ok(json!({
                    "renamed": false,
                    "skipped": true,
                    "worktreeId": worktree_id,
                    "reason": reason,
                })),
                FirstWorkRenameOutcome::Failed {
                    error,
                    failure_count,
                    retry_after_ms,
                } => Ok(json!({
                    "renamed": false,
                    "failed": true,
                    "worktreeId": worktree_id,
                    "error": error,
                    "failureCount": failure_count,
                    "retryAfterMs": retry_after_ms,
                })),
            }
        }
        "sparsePreset.set" | "sparse_preset_set" => {
            let name = req_str(params, "name")?;
            let patterns: Vec<String> = params
                .get("paths")
                .or_else(|| params.get("patterns"))
                .cloned()
                .map(serde_json::from_value)
                .transpose()?
                .unwrap_or_default();
            let cone_mode = params
                .get("coneMode")
                .or_else(|| params.get("cone_mode"))
                .and_then(Value::as_bool)
                .unwrap_or(true);
            let preset = wt.save_sparse_preset(SparseCheckoutPreset {
                id: format!("sp-{}", uuid::Uuid::new_v4().simple()),
                name: name.to_string(),
                patterns,
                cone_mode,
                created_at_ms: now_ms,
                updated_at_ms: now_ms,
            })?;
            Ok(json!({
                "id": preset.id,
                "name": preset.name,
                "paths": preset.patterns,
                "patterns": preset.patterns,
                "coneMode": preset.cone_mode,
            }))
        }
        "create" => {
            let repo_root = PathBuf::from(
                req_str(params, "repo_root").or_else(|_| req_str(params, "repoPath"))?,
            );
            let worktree_path = PathBuf::from(
                req_str(params, "worktree_path").or_else(|_| req_str(params, "worktreePath"))?,
            );
            let existing_branches: BTreeSet<String> = params
                .get("existing_branches")
                .cloned()
                .map(serde_json::from_value)
                .transpose()?
                .unwrap_or_default();
            let rec = wt.create_worktree(CreateWorktreeRequest {
                id: opt_str(params, "id").map(ToOwned::to_owned),
                repo_root,
                worktree_path,
                branch_name: opt_str(params, "branch_name")
                    .or_else(|| opt_str(params, "name"))
                    .map(ToOwned::to_owned),
                title: opt_str(params, "title").map(ToOwned::to_owned),
                parent_worktree_id: opt_str(params, "parent_worktree_id")
                    .or_else(|| opt_str(params, "parentWorktreeId"))
                    .map(ToOwned::to_owned),
                project_group_id: opt_str(params, "project_group_id")
                    .or_else(|| opt_str(params, "projectGroupId"))
                    .map(ToOwned::to_owned),
                created_by_orca: params
                    .get("created_by_orca")
                    .and_then(Value::as_bool)
                    .unwrap_or(true),
                existing_branches,
                seed_hint: opt_str(params, "seed_hint").map(ToOwned::to_owned),
            })?;
            Ok(serde_json::to_value(rec)?)
        }
        "list" => Ok(serde_json::to_value(wt.list_worktrees()?)?),
        "set" | "checkpoint_set" => {
            let worktree_id = req_str(params, "worktree_id")
                .or_else(|_| req_str(params, "worktreeId"))
                .or_else(|_| req_str(params, "id"))?;
            let comment = opt_str(params, "checkpoint_comment")
                .or_else(|| opt_str(params, "comment"))
                .map(ToOwned::to_owned);
            let status = opt_str(params, "workspace_status")
                .or_else(|| opt_str(params, "status"))
                .map(WorkspaceStatus::parse)
                .transpose()?;
            let updated = wt.set_worktree_checkpoint(worktree_id, comment, status, now_ms)?;
            Ok(serde_json::to_value(updated)?)
        }
        "remove" | "cleanup" => {
            let worktree_id = req_str(params, "worktree_id")
                .or_else(|_| req_str(params, "worktreeId"))
                .or_else(|_| req_str(params, "id"))?
                .to_string();
            let unsaved_editor_tabs: Vec<String> = params
                .get("unsaved_editor_tabs")
                .cloned()
                .map(serde_json::from_value)
                .transpose()?
                .unwrap_or_default();
            let force_discard_unsaved = params
                .get("force_discard_unsaved")
                .and_then(Value::as_bool)
                .unwrap_or(false);
            let remove_directory_on_disk = params
                .get("remove_directory_on_disk")
                .and_then(Value::as_bool)
                .unwrap_or(false);
            let outcome = wt.cleanup_worktree(&CleanupWorktreeRequest {
                worktree_id,
                unsaved_editor_tabs,
                force_discard_unsaved,
                remove_directory_on_disk,
                now_ms,
            })?;
            Ok(serde_json::to_value(outcome)?)
        }
        "ports" => {
            let worktrees: Vec<(String, PathBuf)> = wt
                .list_worktrees()?
                .into_iter()
                .map(|w| (w.branch_name, PathBuf::from(w.worktree_path)))
                .collect();
            let attributed = handles
                .port_scanner
                .attribute_ports_to_worktrees(&worktrees);
            Ok(serde_json::to_value(attributed)?)
        }
        "folder_create" => {
            let folder: FolderWorkspace = serde_json::from_value(params.clone())?;
            Ok(serde_json::to_value(wt.create_folder_workspace(folder)?)?)
        }
        "folder_list" => Ok(serde_json::to_value(wt.list_folder_workspaces()?)?),
        other => Err(OrcaIdeRuntimeError::not_found(format!(
            "unknown worktree RPC method 'worktree.{other}'"
        ))),
    }
}

pub(super) fn dispatch_project_group_namespace(
    handles: &RpcSubsystemHandles,
    op: &str,
    params: &Value,
) -> Result<Value> {
    let wt = &handles.worktrees;
    let now_ms = params
        .get("now_ms")
        .or_else(|| params.get("nowMs"))
        .and_then(Value::as_i64)
        .unwrap_or_else(now_epoch_ms);
    match op {
        "create" => {
            let name = req_str(params, "name")?;
            let description = opt_str(params, "description");
            let color = opt_str(params, "color");
            let group = wt.create_project_group(ProjectGroup {
                id: format!("pg-{}", uuid::Uuid::new_v4().simple()),
                name: name.to_string(),
                description: description.map(ToOwned::to_owned),
                color: color.map(ToOwned::to_owned),
                sort_order: 0,
                created_at_ms: now_ms,
                updated_at_ms: now_ms,
            })?;
            Ok(json!({
                "id": group.id,
                "projectGroupId": group.id,
                "name": group.name,
                "description": group.description,
                "color": group.color,
                "sortOrder": group.sort_order,
                "createdAtMs": group.created_at_ms,
            }))
        }
        "list" => {
            let groups = wt.list_project_groups()?;
            Ok(json!({ "groups": groups }))
        }
        other => Err(OrcaIdeRuntimeError::not_found(format!(
            "unknown projectGroup RPC method 'projectGroup.{other}'"
        ))),
    }
}

pub(super) fn dispatch_folder_workspace_namespace(
    handles: &RpcSubsystemHandles,
    op: &str,
    params: &Value,
) -> Result<Value> {
    match op {
        "getPathStatus" | "get_path_status" => {
            let raw_path = req_str(params, "folderPath")
                .or_else(|_| req_str(params, "folder_path"))
                .or_else(|_| req_str(params, "path"))?;
            let p = std::path::Path::new(raw_path);
            let exists = p.exists();
            let is_dir = p.is_dir();
            let is_git = p.join(".git").exists();
            Ok(json!({
                "folderPath": raw_path,
                "exists": exists,
                "isDir": is_dir,
                "isGit": is_git,
            }))
        }
        "list" => Ok(serde_json::to_value(
            handles.worktrees.list_folder_workspaces()?,
        )?),
        other => Err(OrcaIdeRuntimeError::not_found(format!(
            "unknown folderWorkspace RPC method 'folderWorkspace.{other}'"
        ))),
    }
}

pub(super) async fn dispatch_workspace_ports_namespace(
    handles: &RpcSubsystemHandles,
    op: &str,
    params: &Value,
) -> Result<Value> {
    match op {
        "register" => {
            let worktree_id = opt_str(params, "worktreeId")
                .or_else(|| opt_str(params, "worktree_id"))
                .unwrap_or("default")
                .to_string();
            let label = req_str(params, "label")?;
            let target_port = params
                .get("targetPort")
                .or_else(|| params.get("target_port"))
                .or_else(|| params.get("port"))
                .and_then(Value::as_u64)
                .and_then(|p| u16::try_from(p).ok())
                .ok_or_else(|| {
                    OrcaIdeRuntimeError::invalid_argument("missing valid u16 'targetPort'")
                })?;
            let proxy = handles.ensure_label_proxy().await?;
            let clean_label = proxy.register_route(label, target_port)?;
            let hostname = format!("{clean_label}.orca.localhost");
            let entry = RegisteredWorktreePort {
                worktree_id: worktree_id.clone(),
                label: clean_label.clone(),
                target_port,
                hostname: hostname.clone(),
            };
            {
                let mut reg = handles.registered_ports.write();
                reg.retain(|e| !(e.worktree_id == worktree_id && e.label == clean_label));
                reg.push(entry);
            }
            let proxy_port = proxy.port();
            Ok(json!({
                "worktreeId": worktree_id,
                "label": clean_label,
                "targetPort": target_port,
                "proxyPort": proxy_port,
                "hostname": hostname,
                "host": hostname,
            }))
        }
        "list" => {
            let proxy_port = handles.label_proxy.get().map(|p| p.port());
            let ports = handles.registered_ports.read().clone();
            Ok(json!({
                "proxyPort": proxy_port,
                "ports": ports,
            }))
        }
        other => Err(OrcaIdeRuntimeError::not_found(format!(
            "unknown workspacePorts RPC method 'workspacePorts.{other}'"
        ))),
    }
}

pub(super) fn dispatch_workspace_cleanup_namespace(
    handles: &RpcSubsystemHandles,
    op: &str,
    params: &Value,
) -> Result<Value> {
    match op {
        "preview" => {
            let repo_filter = opt_str(params, "repoPath").or_else(|| opt_str(params, "repo_root"));
            let all = handles.worktrees.list_worktrees()?;
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
                    handles
                        .worktrees
                        .analyze_disk_usage(&p)
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
            Ok(json!({
                "preview": true,
                "candidates": candidates,
                "totalReclaimableBytes": total_bytes,
                "reclaimableBytes": total_bytes,
            }))
        }
        other => Err(OrcaIdeRuntimeError::not_found(format!(
            "unknown workspaceCleanup RPC method 'workspaceCleanup.{other}'"
        ))),
    }
}

fn parse_layout_tree_node(
    val: &Value,
    pane_counter: &mut usize,
    now_ms: i64,
) -> Result<SplitPaneNode> {
    let Some(obj) = val.as_object() else {
        return Err(OrcaIdeRuntimeError::invalid_argument(
            "layout tree node must be a JSON object",
        ));
    };
    if obj.contains_key("first") && obj.contains_key("second") {
        let orient = obj
            .get("orientation")
            .or_else(|| obj.get("direction"))
            .and_then(Value::as_str)
            .unwrap_or("horizontal")
            .trim()
            .to_ascii_lowercase();
        let direction = match orient.as_str() {
            "horizontal" | "row" | "h" => SplitDirection::Horizontal,
            "vertical" | "column" | "col" | "v" => SplitDirection::Vertical,
            other => {
                return Err(OrcaIdeRuntimeError::invalid_argument(format!(
                    "unsupported split orientation '{other}'"
                )));
            }
        };
        let ratio_bp = if let Some(r_bp) = obj.get("ratio_bp").and_then(Value::as_u64) {
            if r_bp == 0 || r_bp >= 10_000 {
                return Err(OrcaIdeRuntimeError::invalid_argument(format!(
                    "split ratio_bp {r_bp} must be strictly between 0 and 10000"
                )));
            }
            r_bp as u16
        } else {
            let ratio = obj.get("ratio").and_then(Value::as_f64).unwrap_or(0.5);
            if !ratio.is_finite() || ratio <= 0.0 || ratio >= 1.0 {
                return Err(OrcaIdeRuntimeError::invalid_argument(format!(
                    "split ratio {ratio} must be strictly between 0.0 and 1.0"
                )));
            }
            (ratio * 10_000.0).round().clamp(1.0, 9_999.0) as u16
        };
        let pinned_boundary = obj
            .get("pinned")
            .or_else(|| obj.get("pinned_boundary"))
            .and_then(Value::as_bool)
            .unwrap_or(false);
        let first = parse_layout_tree_node(&obj["first"], pane_counter, now_ms)?;
        let second = parse_layout_tree_node(&obj["second"], pane_counter, now_ms)?;
        Ok(SplitPaneNode::Split {
            direction,
            ratio_bp,
            pinned_boundary,
            first: Box::new(first),
            second: Box::new(second),
        })
    } else {
        *pane_counter += 1;
        let pane_id = obj
            .get("pane_id")
            .or_else(|| obj.get("paneId"))
            .and_then(Value::as_str)
            .filter(|s| !s.trim().is_empty())
            .map(ToOwned::to_owned)
            .unwrap_or_else(|| format!("pane-{}", *pane_counter));
        let mut tabs = Vec::new();
        if let Some(arr) = obj.get("tabs").and_then(Value::as_array) {
            for (idx, t) in arr.iter().enumerate() {
                let tab_id = t
                    .get("id")
                    .or_else(|| t.get("tab_id"))
                    .or_else(|| t.get("tabId"))
                    .and_then(Value::as_str)
                    .filter(|s| !s.trim().is_empty())
                    .map(ToOwned::to_owned)
                    .unwrap_or_else(|| format!("tab-{}-{idx}", *pane_counter));
                let kind_raw = t
                    .get("kind")
                    .or_else(|| t.get("content_type"))
                    .or_else(|| t.get("contentType"))
                    .and_then(Value::as_str)
                    .unwrap_or("terminal");
                let content_type = TabContentType::parse(kind_raw).ok_or_else(|| {
                    OrcaIdeRuntimeError::invalid_argument(format!(
                        "unsupported tab content kind '{kind_raw}'"
                    ))
                })?;
                let title = t
                    .get("title")
                    .and_then(Value::as_str)
                    .filter(|s| !s.trim().is_empty())
                    .unwrap_or(content_type.traits().label)
                    .to_string();
                let pinned = t.get("pinned").and_then(Value::as_bool).unwrap_or(false);
                let preview = t.get("preview").and_then(Value::as_bool).unwrap_or(false);
                tabs.push(WorkspaceTab {
                    tab_id,
                    content_type,
                    title,
                    pinned,
                    preview,
                    view_mode: TabViewMode::Terminal,
                    leaf_state: TabLeafState::Live,
                    pty_incarnation_id: None,
                    dirty: false,
                    created_at_ms: now_ms,
                    last_active_at_ms: now_ms,
                });
            }
        }
        let active_tab_id = tabs.first().map(|t| t.tab_id.clone());
        Ok(SplitPaneNode::Leaf {
            pane_id,
            tabs,
            active_tab_id,
        })
    }
}

fn layout_node_to_wire_json(node: &SplitPaneNode) -> Value {
    match node {
        SplitPaneNode::Split {
            direction,
            ratio_bp,
            pinned_boundary,
            first,
            second,
        } => {
            let orient = match direction {
                SplitDirection::Horizontal => "horizontal",
                SplitDirection::Vertical => "vertical",
            };
            let ratio = f64::from(*ratio_bp) / 10_000.0;
            json!({
                "kind": "split",
                "direction": orient,
                "orientation": orient,
                "ratio_bp": ratio_bp,
                "ratio": ratio,
                "pinned_boundary": pinned_boundary,
                "pinned": pinned_boundary,
                "first": layout_node_to_wire_json(first),
                "second": layout_node_to_wire_json(second),
            })
        }
        SplitPaneNode::Leaf {
            pane_id,
            tabs,
            active_tab_id,
        } => {
            let wire_tabs: Vec<Value> = tabs
                .iter()
                .map(|t| {
                    let mut v = serde_json::to_value(t).unwrap_or_else(|_| json!({}));
                    if let Some(obj) = v.as_object_mut() {
                        obj.insert("id".into(), json!(t.tab_id));
                        obj.insert("kind".into(), json!(t.content_type.as_str()));
                    }
                    v
                })
                .collect();
            json!({
                "kind": "leaf",
                "pane_id": pane_id,
                "tabs": wire_tabs,
                "active_tab_id": active_tab_id,
            })
        }
    }
}

fn layout_model_to_wire_json(model: &WorkspaceLayoutModel) -> Value {
    let mut val = serde_json::to_value(model).unwrap_or_else(|_| json!({}));
    if let Some(obj) = val.as_object_mut() {
        obj.insert("root".into(), layout_node_to_wire_json(&model.root));
    }
    val
}

pub(super) fn dispatch_layout_namespace(
    handles: &RpcSubsystemHandles,
    op: &str,
    params: &Value,
) -> Result<Value> {
    let now_ms = params
        .get("now_ms")
        .and_then(Value::as_i64)
        .unwrap_or_else(now_epoch_ms);
    let workspace_id = opt_str(params, "workspace_id")
        .or_else(|| opt_str(params, "workspaceId"))
        .unwrap_or("default");

    match op {
        "get" => {
            let mut layouts = handles.layouts.write();
            let model = layouts
                .entry(workspace_id.to_string())
                .or_insert_with(|| WorkspaceLayoutModel::new(workspace_id, &handles.host_id, "main"));
            Ok(layout_model_to_wire_json(model))
        }
        "apply" => {
            let root_val = params
                .get("root")
                .ok_or_else(|| OrcaIdeRuntimeError::invalid_argument("missing 'root' layout tree"))?;
            let mut pane_counter = 0usize;
            let root = parse_layout_tree_node(root_val, &mut pane_counter, now_ms)?;
            let mut layouts = handles.layouts.write();
            let model = layouts
                .entry(workspace_id.to_string())
                .or_insert_with(|| WorkspaceLayoutModel::new(workspace_id, &handles.host_id, "main"));
            model.root = root;
            Ok(layout_model_to_wire_json(model))
        }
        "open_tab" | "openTab" => {
            let pane_id = opt_str(params, "pane_id").unwrap_or("main");
            let tab_id = req_str(params, "tab_id")
                .or_else(|_| req_str(params, "tabId"))?
                .to_string();
            let title = opt_str(params, "title").unwrap_or("Tab").to_string();
            let content_type: TabContentType = params
                .get("content_type")
                .or_else(|| params.get("kind"))
                .cloned()
                .map(serde_json::from_value)
                .transpose()?
                .unwrap_or(TabContentType::Terminal);
            let tab = WorkspaceTab {
                tab_id,
                content_type,
                title,
                pinned: params.get("pinned").and_then(Value::as_bool).unwrap_or(false),
                preview: params.get("preview").and_then(Value::as_bool).unwrap_or(false),
                view_mode: TabViewMode::Terminal,
                leaf_state: TabLeafState::Live,
                pty_incarnation_id: None,
                dirty: false,
                created_at_ms: now_ms,
                last_active_at_ms: now_ms,
            };
            let mut layouts = handles.layouts.write();
            let model = layouts
                .entry(workspace_id.to_string())
                .or_insert_with(|| WorkspaceLayoutModel::new(workspace_id, &handles.host_id, "main"));
            model.open_tab(pane_id, tab, now_ms)?;
            Ok(layout_model_to_wire_json(model))
        }
        "close_tab" | "closeTab" => {
            let tab_id = req_str(params, "tab_id")
                .or_else(|_| req_str(params, "tabId"))
                .or_else(|_| req_str(params, "id"))?;
            let confirm_pinned = params
                .get("confirm_pinned")
                .or_else(|| params.get("confirmPinned"))
                .or_else(|| params.get("force"))
                .and_then(Value::as_bool)
                .unwrap_or(false);
            let mut layouts = handles.layouts.write();
            let model = layouts
                .entry(workspace_id.to_string())
                .or_insert_with(|| WorkspaceLayoutModel::new(workspace_id, &handles.host_id, "main"));
            let is_pinned = model.find_tab(tab_id).is_some_and(|t| t.pinned);
            if is_pinned && !confirm_pinned {
                return Err(OrcaIdeRuntimeError::invalid_argument(format!(
                    "pinned_tab_protected: tab '{tab_id}' is pinned and requires confirm_pinned=true"
                )));
            }
            let tomb = model.close_tab(tab_id, now_ms)?;
            handles
                .closed_tab_tombstones
                .write()
                .insert(tab_id.to_string(), now_ms);
            let mut val = serde_json::to_value(&tomb)?;
            if let Some(obj) = val.as_object_mut() {
                obj.insert(
                    "tombstone_ttl_days".into(),
                    json!(orca_core::CLOSED_TAB_TOMBSTONE_DAYS),
                );
            }
            Ok(val)
        }
        "reopen_tab" | "reopenTab" => {
            let tab_id = opt_str(params, "tab_id").or_else(|| opt_str(params, "tabId"));
            let mut layouts = handles.layouts.write();
            let model = layouts
                .entry(workspace_id.to_string())
                .or_insert_with(|| WorkspaceLayoutModel::new(workspace_id, &handles.host_id, "main"));
            let tab = model.reopen_closed_tab(tab_id, now_ms)?;
            handles.closed_tab_tombstones.write().remove(&tab.tab_id);
            Ok(serde_json::to_value(tab)?)
        }
        "split" => {
            let pane_id = opt_str(params, "pane_id").unwrap_or("main");
            let new_pane_id = req_str(params, "new_pane_id")?.to_string();
            let direction: SplitDirection = params
                .get("direction")
                .cloned()
                .map(serde_json::from_value)
                .transpose()?
                .unwrap_or(SplitDirection::Horizontal);
            let ratio_bp = params
                .get("ratio_bp")
                .and_then(Value::as_u64)
                .unwrap_or(5_000) as u16;
            let mut layouts = handles.layouts.write();
            let model = layouts
                .entry(workspace_id.to_string())
                .or_insert_with(|| WorkspaceLayoutModel::new(workspace_id, &handles.host_id, "main"));
            model.split_pane(pane_id, new_pane_id, direction, ratio_bp, None)?;
            Ok(layout_model_to_wire_json(model))
        }
        "toggle_view_mode" | "toggleViewMode" => {
            let tab_id = req_str(params, "tab_id").or_else(|_| req_str(params, "tabId"))?;
            let mut layouts = handles.layouts.write();
            let model = layouts
                .entry(workspace_id.to_string())
                .or_insert_with(|| WorkspaceLayoutModel::new(workspace_id, &handles.host_id, "main"));
            let mode = model.toggle_tab_view_mode(tab_id)?;
            Ok(json!({ "tab_id": tab_id, "view_mode": mode }))
        }
        "quick_switch" | "quickSwitch" => {
            let query = params
                .get("query")
                .and_then(Value::as_str)
                .unwrap_or_default();
            let items: Vec<QuickSwitcherItem> = params
                .get("items")
                .cloned()
                .map(serde_json::from_value)
                .transpose()?
                .unwrap_or_default();
            let limit = params
                .get("limit")
                .and_then(Value::as_u64)
                .unwrap_or(20) as usize;
            let ranked = rank_quick_switcher_items(query, &items, now_ms, limit);
            Ok(serde_json::to_value(ranked)?)
        }
        other => Err(OrcaIdeRuntimeError::not_found(format!(
            "unknown layout RPC method 'layout.{other}'"
        ))),
    }
}

fn jump_candidate_status_rank(status: &str) -> u8 {
    match status.trim().to_ascii_lowercase().as_str() {
        "needs_you" | "needsyou" | "needs-you" => 0,
        "waiting" | "blocked" => 1,
        "done" | "completed" => 2,
        "working" | "running" => 3,
        _ => 4,
    }
}

pub(super) fn dispatch_jump_palette_namespace(
    _handles: &RpcSubsystemHandles,
    op: &str,
    params: &Value,
) -> Result<Value> {
    match op {
        "list" | "rank" => {
            let query = params
                .get("query")
                .or_else(|| params.get("q"))
                .and_then(Value::as_str)
                .unwrap_or("")
                .trim()
                .to_ascii_lowercase();
            let active_id = opt_str(params, "active_id").or_else(|| opt_str(params, "activeId"));
            let limit = params
                .get("limit")
                .and_then(Value::as_u64)
                .unwrap_or(6)
                .clamp(1, 6) as usize;
            let raw_candidates = params
                .get("candidates")
                .or_else(|| params.get("items"))
                .and_then(Value::as_array)
                .cloned()
                .unwrap_or_default();

            let mut scored: Vec<(u8, usize, Value)> = Vec::new();
            for (orig_idx, item) in raw_candidates.into_iter().enumerate() {
                let id = item
                    .get("id")
                    .or_else(|| item.get("sessionId"))
                    .and_then(Value::as_str)
                    .unwrap_or("");
                let status = item
                    .get("status")
                    .and_then(Value::as_str)
                    .unwrap_or("idle");
                let rank = jump_candidate_status_rank(status);
                if query.is_empty()
                    && active_id.is_some_and(|aid| aid == id)
                    && rank == 4
                {
                    continue;
                }
                if !query.is_empty() {
                    let hay = format!(
                        "{} {} {} {}",
                        id,
                        item.get("title").and_then(Value::as_str).unwrap_or(""),
                        item.get("label").and_then(Value::as_str).unwrap_or(""),
                        status
                    )
                    .to_ascii_lowercase();
                    if !hay.contains(&query) {
                        continue;
                    }
                }
                scored.push((rank, orig_idx, item));
            }
            scored.sort_by(|a, b| a.0.cmp(&b.0).then_with(|| a.1.cmp(&b.1)));
            let items: Vec<Value> = scored
                .into_iter()
                .take(limit)
                .map(|(_, _, v)| v)
                .collect();
            Ok(json!({ "items": items, "count": items.len() }))
        }
        other => Err(OrcaIdeRuntimeError::not_found(format!(
            "unknown jumpPalette RPC method 'jumpPalette.{other}'"
        ))),
    }
}
