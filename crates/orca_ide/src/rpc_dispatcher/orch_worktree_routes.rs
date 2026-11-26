//! Unified RPC routing for `orchestration.*`, `agentSession.*`, `terminal.*`,
//! `worktree.*`, and `layout.*`.

use std::collections::BTreeSet;
use std::path::PathBuf;

use orca_core::orchestration_protocol::{
    ExecutionHostRecord, HostSelectionCriteria, OrchestrationTaskStatus,
};
use orca_core::workspace_layout::{
    FolderWorkspace, QuickSwitcherItem, SplitDirection, TabContentType, TabLeafState, TabViewMode,
    WorkspaceLayoutModel, WorkspaceTab, rank_quick_switcher_items,
};
use serde_json::{Value, json};

use super::RpcSubsystemHandles;
use crate::agent_catalog::AgentCatalog;
use crate::chat_journal::{AppendJournalItemRequest, CreateAgentSessionRequest};
use crate::orchestration_engine::{
    CheckMailboxRequest, CompleteWorkerRequest, CreateGateRequest, CreateRunRequest,
    CreateTaskRequest, ResolveGateAction, SendMessageRequest, StartWorkerRequest,
};
use crate::pty_multiplex::{KillSessionRequest, PtyIncarnationId};
use crate::worktree_manager::{
    CleanupWorktreeRequest, CreateWorktreeRequest, WorktreeIncludeCopyOptions, WorkspaceStatus,
    execute_worktree_include_copy, parse_orca_yaml_text,
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

pub(super) fn dispatch_agent_session_namespace(
    handles: &RpcSubsystemHandles,
    op: &str,
    params: &Value,
) -> Result<Value> {
    let journal = &handles.chat_journal;
    match op {
        "create" => {
            let req: CreateAgentSessionRequest = serde_json::from_value(params.clone())?;
            Ok(serde_json::to_value(journal.create_session(req)?)?)
        }
        "get" => {
            let session_id = req_str(params, "session_id")?;
            Ok(serde_json::to_value(journal.get_session(session_id)?)?)
        }
        "list" => {
            let ws = opt_str(params, "workspace_id");
            Ok(serde_json::to_value(journal.list_sessions(ws)?)?)
        }
        "append" => {
            let req: AppendJournalItemRequest = serde_json::from_value(params.clone())?;
            Ok(serde_json::to_value(journal.append_item(req)?)?)
        }
        "flip" => {
            let session_id = req_str(params, "session_id")?;
            let new_handle = opt_str(params, "new_provider_handle").map(ToOwned::to_owned);
            let now_ms = params.get("now_ms").and_then(Value::as_i64);
            Ok(serde_json::to_value(
                journal.toggle_view_mode(session_id, new_handle, now_ms)?,
            )?)
        }
        "resume" => {
            let src = req_str(params, "source_session_id").or_else(|_| req_str(params, "session_id"))?;
            let target_mode: TabViewMode = params
                .get("target_view_mode")
                .cloned()
                .map(serde_json::from_value)
                .transpose()?
                .unwrap_or(TabViewMode::Chat);
            let new_id = opt_str(params, "new_session_id").map(ToOwned::to_owned);
            let now_ms = params.get("now_ms").and_then(Value::as_i64);
            Ok(serde_json::to_value(
                journal.resume_in_new_session(src, target_mode, new_id, now_ms)?,
            )?)
        }
        "fork" => {
            let src = req_str(params, "source_session_id").or_else(|_| req_str(params, "session_id"))?;
            let up_to = params.get("up_to_sequence").and_then(Value::as_i64);
            let new_id = opt_str(params, "new_session_id").map(ToOwned::to_owned);
            let now_ms = params.get("now_ms").and_then(Value::as_i64);
            Ok(serde_json::to_value(
                journal.fork_session(src, up_to, new_id, now_ms)?,
            )?)
        }
        "catalog" => {
            let catalog = AgentCatalog::new();
            Ok(json!({
                "bot_agent_count": catalog.bot_agents().len(),
                "tui_agent_count": catalog.entries().len(),
                "entries": catalog.entries(),
            }))
        }
        other => Err(OrcaIdeRuntimeError::not_found(format!(
            "unknown agentSession RPC method 'agentSession.{other}'"
        ))),
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
        .and_then(Value::as_i64)
        .unwrap_or_else(now_epoch_ms);

    match op {
        "create" => {
            let tab_id = req_str(params, "tab_id")?.to_string();
            let pane_id = opt_str(params, "pane_id").unwrap_or("main");
            let title = opt_str(params, "title").unwrap_or("Terminal");
            let inc = pty.create_session(tab_id.clone(), pane_id, title, now_ms)?;
            Ok(json!({
                "tab_id": tab_id,
                "incarnation_id": inc,
            }))
        }
        "output" => {
            let tab_id = req_str(params, "tab_id")?;
            let data = params
                .get("data")
                .and_then(Value::as_str)
                .unwrap_or_default();
            let facts = pty.ingest_pty_output(tab_id, data.as_bytes())?;
            Ok(json!({ "tab_id": tab_id, "facts": facts }))
        }
        "park" => {
            let tab_id = req_str(params, "tab_id")?;
            let hidden = params
                .get("hidden")
                .and_then(Value::as_bool)
                .unwrap_or(true);
            pty.set_session_background(tab_id, hidden)?;
            Ok(json!({ "tab_id": tab_id, "hidden": hidden }))
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
            let tab_id = req_str(params, "tab_id")?;
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
            let tab_id = req_str(params, "tab_id")?.to_string();
            let expected_incarnation_id = opt_str(params, "expected_incarnation_id")
                .map(|s| PtyIncarnationId(s.to_string()));
            let require_no_agent_owner = params
                .get("require_no_agent_owner")
                .and_then(Value::as_bool)
                .unwrap_or(false);
            let require_unattached = params
                .get("require_unattached")
                .and_then(Value::as_bool)
                .unwrap_or(false);
            let tombstone = pty.kill_session(&KillSessionRequest {
                tab_id,
                expected_incarnation_id,
                require_no_agent_owner,
                require_unattached,
                now_ms,
            })?;
            Ok(serde_json::to_value(tombstone)?)
        }
        "tombstones" => Ok(serde_json::to_value(pty.list_tombstones())?),
        "hook_status" => {
            if let Some(session_id) = opt_str(params, "session_id") {
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
        "include_copy" => {
            let src = PathBuf::from(req_str(params, "source_repo_root")?);
            let dst = PathBuf::from(req_str(params, "target_worktree_root")?);
            let rules = params
                .get("rules")
                .and_then(Value::as_str)
                .unwrap_or_default();
            let options: WorktreeIncludeCopyOptions = params
                .get("options")
                .cloned()
                .map(serde_json::from_value)
                .transpose()?
                .unwrap_or_default();
            let report = execute_worktree_include_copy(&src, &dst, rules, &options)?;
            Ok(serde_json::to_value(report)?)
        }
        "create" => {
            let repo_root = PathBuf::from(req_str(params, "repo_root")?);
            let worktree_path = PathBuf::from(req_str(params, "worktree_path")?);
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
                branch_name: opt_str(params, "branch_name").map(ToOwned::to_owned),
                title: opt_str(params, "title").map(ToOwned::to_owned),
                parent_worktree_id: opt_str(params, "parent_worktree_id").map(ToOwned::to_owned),
                project_group_id: opt_str(params, "project_group_id").map(ToOwned::to_owned),
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
            let worktree_id = req_str(params, "worktree_id").or_else(|_| req_str(params, "id"))?;
            let comment = opt_str(params, "checkpoint_comment").map(ToOwned::to_owned);
            let status = opt_str(params, "workspace_status")
                .map(WorkspaceStatus::parse)
                .transpose()?;
            let updated = wt.set_worktree_checkpoint(worktree_id, comment, status, now_ms)?;
            Ok(serde_json::to_value(updated)?)
        }
        "remove" | "cleanup" => {
            let worktree_id = req_str(params, "worktree_id")
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

pub(super) fn dispatch_layout_namespace(
    handles: &RpcSubsystemHandles,
    op: &str,
    params: &Value,
) -> Result<Value> {
    let now_ms = params
        .get("now_ms")
        .and_then(Value::as_i64)
        .unwrap_or_else(now_epoch_ms);
    let workspace_id = opt_str(params, "workspace_id").unwrap_or("default");

    match op {
        "get" => {
            let mut layouts = handles.layouts.write();
            let model = layouts
                .entry(workspace_id.to_string())
                .or_insert_with(|| WorkspaceLayoutModel::new(workspace_id, &handles.host_id, "main"));
            Ok(serde_json::to_value(model)?)
        }
        "open_tab" => {
            let pane_id = opt_str(params, "pane_id").unwrap_or("main");
            let tab_id = req_str(params, "tab_id")?.to_string();
            let title = opt_str(params, "title").unwrap_or("Tab").to_string();
            let content_type: TabContentType = params
                .get("content_type")
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
            Ok(serde_json::to_value(model)?)
        }
        "close_tab" => {
            let tab_id = req_str(params, "tab_id")?;
            let mut layouts = handles.layouts.write();
            let model = layouts
                .entry(workspace_id.to_string())
                .or_insert_with(|| WorkspaceLayoutModel::new(workspace_id, &handles.host_id, "main"));
            let tomb = model.close_tab(tab_id, now_ms)?;
            Ok(serde_json::to_value(tomb)?)
        }
        "reopen_tab" => {
            let tab_id = opt_str(params, "tab_id");
            let mut layouts = handles.layouts.write();
            let model = layouts
                .entry(workspace_id.to_string())
                .or_insert_with(|| WorkspaceLayoutModel::new(workspace_id, &handles.host_id, "main"));
            let tab = model.reopen_closed_tab(tab_id, now_ms)?;
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
            Ok(serde_json::to_value(model)?)
        }
        "toggle_view_mode" => {
            let tab_id = req_str(params, "tab_id")?;
            let mut layouts = handles.layouts.write();
            let model = layouts
                .entry(workspace_id.to_string())
                .or_insert_with(|| WorkspaceLayoutModel::new(workspace_id, &handles.host_id, "main"));
            let mode = model.toggle_tab_view_mode(tab_id)?;
            Ok(json!({ "tab_id": tab_id, "view_mode": mode }))
        }
        "quick_switch" => {
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
