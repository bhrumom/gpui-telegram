"""Tier 1 opaque-box E2E tests for Area E3: F10 (Worktrees) and F11 (Orchestration)."""

from __future__ import annotations

import threading
import uuid
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from pathlib import Path
from typing import Any

import pytest

from harness import (
    ORCHESTRATION_MESSAGE_TYPES_9,
    GitRepoFixture,
    OrcaApiClient,
    OrcaCliRunner,
    OrcaServerHandle,
    RawHttpClient,
    TempWorkspace,
    allocate_free_port,
)


def _uid(prefix: str) -> str:
    return f"{prefix}-{uuid.uuid4().hex[:8]}"


def _req_str(obj: Any, *keys: str) -> str:
    assert isinstance(obj, dict), f"Expected dict payload, got {obj!r}"
    for key in keys:
        val = obj.get(key)
        if isinstance(val, str) and val:
            return val
    raise AssertionError(f"Missing non-empty string key in {keys!r}: {obj!r}")


def _items(payload: Any, *keys: str) -> list[dict[str, Any]]:
    if isinstance(payload, list):
        return payload
    assert isinstance(payload, dict), f"Expected list/dict payload, got {payload!r}"
    for key in keys:
        val = payload.get(key)
        if isinstance(val, list):
            return val
    raise AssertionError(f"Missing list key in {keys!r}: {payload!r}")


def _start_echo_server(body: bytes) -> tuple[ThreadingHTTPServer, int, threading.Thread]:
    port = allocate_free_port()

    class _Handler(BaseHTTPRequestHandler):
        def do_GET(self) -> None:
            self.send_response(200)
            self.send_header("Content-Type", "text/plain; charset=utf-8")
            self.send_header("Content-Length", str(len(body)))
            self.end_headers()
            self.wfile.write(body)

        def log_message(self, fmt: str, *args: Any) -> None:
            return

    server = ThreadingHTTPServer(("127.0.0.1", port), _Handler)
    thread = threading.Thread(target=server.serve_forever, daemon=True)
    thread.start()
    return server, port, thread


# ---------------------------------------------------------------------------
# F10 — Strict orca.yaml, .worktreeinclude, Folder Workspaces & Parallel Worktrees
# ---------------------------------------------------------------------------


@pytest.mark.tier1
@pytest.mark.f10
def test_t1_f10_orca_yaml_parse_full_schema_rest_and_cli(
    git_repo: GitRepoFixture, orca_api: OrcaApiClient, orca_cli: OrcaCliRunner
) -> None:
    yaml_text = (
        "scripts:\n  setup: echo setup\n  archive: echo archive\n"
        "setupAgentStartupPolicy: wait-for-setup\n"
        'issueCommand: "claude {{artifact_url}}"\n'
        "defaultTabs:\n  - title: Shell\n    type: terminal\n  - title: Diff\n    type: diff\n"
        "worktree:\n  sharedDirectories:\n    - .cache/deps\n"
        "environmentRecipes:\n  - name: rust-dev\n    command: cargo check\n"
    )
    yaml_path = git_repo.write_orca_yaml(yaml_text, commit=True)
    rest = orca_api.assert_ok(orca_api.post("/api/v1/orca/ide/orca-yaml/parse", json_body={"path": str(yaml_path), "content": yaml_text}))
    cli = orca_cli.run("worktree", "parse-yaml", "--path", str(yaml_path)).assert_ok()
    for data in (rest, cli):
        assert isinstance(data, dict) and isinstance(data.get("scripts"), dict)
        assert data["scripts"].get("setup") == "echo setup" and data["scripts"].get("archive") == "echo archive"
        assert data.get("setupAgentStartupPolicy") == "wait-for-setup"
        assert "{{artifact_url}}" in _req_str(data, "issueCommand")
        assert isinstance(data.get("defaultTabs"), list) and len(data["defaultTabs"]) == 2
        wt_cfg = data.get("worktree") if isinstance(data.get("worktree"), dict) else data
        assert wt_cfg.get("sharedDirectories") == [".cache/deps"]
        assert isinstance(data.get("environmentRecipes"), list) and len(data["environmentRecipes"]) == 1
    rest_hash, cli_hash = _req_str(rest, "setupTrustHash"), _req_str(cli, "setupTrustHash")
    assert len(rest_hash) == 64 and all(c in "0123456789abcdef" for c in rest_hash.lower())
    assert rest_hash == cli_hash


@pytest.mark.tier1
@pytest.mark.f10
def test_t1_f10_worktreeinclude_private_copy_gitignored_files(
    git_repo: GitRepoFixture, temp_workspace: TempWorkspace, orca_api: OrcaApiClient, orca_cli: OrcaCliRunner
) -> None:
    git_repo.commit_file("README.md", "base\n", msg="init")
    git_repo.create_ignored_file(".env.local", "APP_ENV=dev\n")
    git_repo.create_ignored_file("secrets/local.json", '{"k":1}\n')
    git_repo.write_worktreeinclude([".env.local", "secrets/local.json"], commit=True)

    dest_cli = temp_workspace.mkdir("wt_dest_cli")
    cli_res = orca_cli.run("worktree", "include-copy", "--source", str(git_repo.path), "--dest", str(dest_cli)).assert_ok()
    assert isinstance(cli_res, dict) and cli_res.get("copiedCount") == 2 and cli_res.get("warnings") == []

    copied_env, copied_secret = dest_cli / ".env.local", dest_cli / "secrets/local.json"
    assert copied_env.is_file() and not copied_env.is_symlink() and copied_env.read_text(encoding="utf-8") == "APP_ENV=dev\n"
    assert copied_secret.is_file() and not copied_secret.is_symlink() and copied_secret.read_text(encoding="utf-8") == '{"k":1}\n'
    copied_env.write_text("APP_ENV=mutated\n", encoding="utf-8")
    assert (git_repo.path / ".env.local").read_text(encoding="utf-8") == "APP_ENV=dev\n"

    dest_rpc = temp_workspace.mkdir("wt_dest_rpc")
    rpc_res = orca_api.assert_rpc_ok("worktree.includeCopy", {"sourcePath": str(git_repo.path), "destPath": str(dest_rpc)})
    assert isinstance(rpc_res, dict) and rpc_res.get("copiedCount") == 2
    assert (dest_rpc / ".env.local").read_text(encoding="utf-8") == "APP_ENV=dev\n" and not (dest_rpc / ".env.local").is_symlink()


@pytest.mark.tier1
@pytest.mark.f10
def test_t1_f10_folder_workspace_first_class_without_git(
    temp_workspace: TempWorkspace, orca_api: OrcaApiClient, orca_cli: OrcaCliRunner
) -> None:
    folder = temp_workspace.mkdir(_uid("non_git_notes"))
    (folder / "notes.md").write_text("# Scratch\n", encoding="utf-8")
    assert not (folder / ".git").exists()

    group_id = _req_str(orca_api.assert_rpc_ok("projectGroup.create", {"name": _uid("notes-group")}), "id", "projectGroupId")
    created = orca_api.assert_ok(
        orca_api.post("/api/v1/orca/ide/worktrees", json_body={"kind": "folder", "name": _uid("scratch-notes"), "folderPath": str(folder), "projectGroupId": group_id})
    )
    ws_id = _req_str(created, "id", "worktreeId")
    assert (created.get("kind") == "folder" or created.get("isGit") is False) and created.get("projectGroupId") == group_id
    assert orca_api.assert_rpc_ok("folderWorkspace.getPathStatus", {"folderPath": str(folder)}).get("exists") is True

    rest_list = _items(orca_api.assert_ok(orca_api.get("/api/v1/orca/ide/worktrees")), "worktrees", "items")
    cli_list = _items(orca_cli.run("worktree", "list").assert_ok(), "worktrees", "items")
    for listing in (rest_list, cli_list):
        matched = [w for w in listing if (w.get("id") or w.get("worktreeId")) == ws_id]
        assert len(matched) == 1 and (matched[0].get("kind") == "folder" or matched[0].get("isGit") is False)
        assert matched[0].get("projectGroupId") == group_id


@pytest.mark.tier1
@pytest.mark.f10
def test_t1_f10_git_worktree_lineage_checkpoints_and_first_work_rename(
    git_repo: GitRepoFixture, orca_api: OrcaApiClient, orca_cli: OrcaCliRunner
) -> None:
    git_repo.commit_file("lib.rs", "pub fn ok() {}\n", msg="init")
    parent_id = _req_str(
        orca_api.assert_ok(orca_api.post("/api/v1/orca/ide/worktrees", json_body={"repoPath": str(git_repo.path), "name": _uid("parent-wt"), "baseRef": "main"})),
        "id",
        "worktreeId",
    )
    child = orca_api.assert_ok(
        orca_api.post(
            "/api/v1/orca/ide/worktrees",
            json_body={"repoPath": str(git_repo.path), "name": "gentle-otter", "baseRef": "main", "parentWorktreeId": parent_id, "autoGeneratedName": True},
        )
    )
    child_id = _req_str(child, "id", "worktreeId")
    assert child.get("parentWorktreeId") == parent_id

    cp = orca_cli.run("worktree", "set", "--worktree", child_id, "--comment", "Wiring parser tests", "--status", "in-progress").assert_ok()
    assert isinstance(cp, dict)
    renamed = orca_api.assert_rpc_ok("worktree.firstWorkRename", {"worktreeId": child_id, "agentState": "working", "prompt": "Fix websocket reconnect backoff"})
    assert isinstance(renamed, dict) and renamed.get("renamed") is True and _req_str(renamed, "name", "branch", "slug") != "gentle-otter"

    listing = _items(orca_api.assert_ok(orca_api.get("/api/v1/orca/ide/worktrees")), "worktrees", "items")
    found = [w for w in listing if (w.get("id") or w.get("worktreeId")) == child_id]
    assert len(found) == 1 and found[0].get("parentWorktreeId") == parent_id
    assert found[0].get("comment") == "Wiring parser tests" and found[0].get("workspaceStatus") == "in-progress"
    assert (found[0].get("name") or found[0].get("branch")) != "gentle-otter"


@pytest.mark.tier1
@pytest.mark.f10
def test_t1_f10_localhost_label_proxy_routes_isolated_origins(
    git_repo: GitRepoFixture, orca_api: OrcaApiClient, orca_cli: OrcaCliRunner
) -> None:
    git_repo.commit_file("README.md", "proxy\n", msg="init")
    started: list[tuple[ThreadingHTTPServer, threading.Thread]] = []
    try:
        srv_a, port_a, th_a = _start_echo_server(b"origin-alpha")
        started.append((srv_a, th_a))
        srv_b, port_b, th_b = _start_echo_server(b"origin-beta")
        started.append((srv_b, th_b))
        lbl_a, lbl_b = _uid("alpha"), _uid("beta")
        wt_a_id = _req_str(orca_api.assert_ok(orca_api.post("/api/v1/orca/ide/worktrees", json_body={"repoPath": str(git_repo.path), "name": lbl_a, "label": lbl_a, "baseRef": "main"})), "id", "worktreeId")
        wt_b_id = _req_str(orca_api.assert_ok(orca_api.post("/api/v1/orca/ide/worktrees", json_body={"repoPath": str(git_repo.path), "name": lbl_b, "label": lbl_b, "baseRef": "main"})), "id", "worktreeId")

        reg_a = orca_api.assert_rpc_ok("workspacePorts.register", {"worktreeId": wt_a_id, "label": lbl_a, "targetPort": port_a})
        reg_b = orca_api.assert_rpc_ok("workspacePorts.register", {"worktreeId": wt_b_id, "label": lbl_b, "targetPort": port_b})
        ports_cli = orca_cli.run("worktree", "ports", "--worktree", wt_a_id).assert_ok()
        assert isinstance(ports_cli, dict)
        proxy_port = int(reg_a.get("proxyPort") or ports_cli.get("proxyPort"))
        host_a, host_b = _req_str(reg_a, "hostname", "host"), _req_str(reg_b, "hostname", "host")
        assert host_a.endswith(".orca.localhost") and host_b.endswith(".orca.localhost") and host_a != host_b

        raw = RawHttpClient("127.0.0.1", proxy_port)
        assert raw.request("GET", "/", headers={"Host": f"{host_a}:{proxy_port}"}).assert_status(200).text == "origin-alpha"
        assert raw.request("GET", "/", headers={"Host": f"{host_b}:{proxy_port}"}).assert_status(200).text == "origin-beta"
    finally:
        for srv, th in started:
            srv.shutdown()
            srv.server_close()
            th.join(timeout=3.0)


@pytest.mark.tier1
@pytest.mark.f10
def test_t1_f10_sparse_checkout_preset_and_cleanup_disk_preview(
    git_repo: GitRepoFixture, orca_api: OrcaApiClient, orca_cli: OrcaCliRunner
) -> None:
    git_repo.commit_file("services/api/main.rs", "fn main() {}\n", msg="api")
    git_repo.commit_file("services/web/index.ts", "export const v = 1;\n", msg="web")
    preset = orca_api.assert_rpc_ok("worktree.sparsePreset.set", {"repoPath": str(git_repo.path), "name": "api-only", "paths": ["services/api"]})
    assert isinstance(preset, dict) and preset.get("name") == "api-only"

    wt = orca_api.assert_ok(
        orca_api.post("/api/v1/orca/ide/worktrees", json_body={"repoPath": str(git_repo.path), "name": _uid("wt-api"), "baseRef": "main", "sparsePreset": "api-only"})
    )
    wt_path = Path(_req_str(wt, "path", "worktreePath"))
    assert (wt_path / "services/api/main.rs").is_file() and not (wt_path / "services/web/index.ts").exists()

    rpc_prev = orca_api.assert_rpc_ok("workspaceCleanup.preview", {"repoPath": str(git_repo.path)})
    cli_prev = orca_cli.run("worktree", "cleanup", "--preview", "--repo", str(git_repo.path)).assert_ok()
    for prev in (rpc_prev, cli_prev):
        total_bytes = prev.get("totalReclaimableBytes", prev.get("reclaimableBytes"))
        assert isinstance(prev, dict) and isinstance(total_bytes, int) and total_bytes >= 0 and isinstance(prev.get("candidates"), list)
    assert wt_path.is_dir()


# ---------------------------------------------------------------------------
# F11 — Durable SQLite Multi-Agent Orchestration & Cross-Host Federation
# ---------------------------------------------------------------------------


@pytest.mark.tier1
@pytest.mark.f11
def test_t1_f11_durable_run_and_task_dag_progression_across_restart(
    fresh_orca_server: OrcaServerHandle, cli_bin_path: Path, temp_workspace: TempWorkspace
) -> None:
    sess = fresh_orca_server.register_user()
    api = fresh_orca_server.api_client(sess)
    cli = OrcaCliRunner(cli_bin_path, fresh_orca_server.base_url, sess.token, temp_workspace.mkdir("cli"))
    run = cli.run("orchestration", "run-create", "--title", _uid("dag"), "--coordinator-session", _uid("coord")).assert_ok()
    run_id = _req_str(run, "id", "runId")
    assert int(run.get("consumerGeneration", 0)) >= 1
    task_a = api.assert_ok(api.post("/api/v1/orca/orchestration/tasks", json_body={"runId": run_id, "title": "Task A", "deps": []}))
    task_a_id = _req_str(task_a, "id", "taskId")
    assert task_a.get("status") == "ready"
    task_b = api.assert_ok(api.post("/api/v1/orca/orchestration/tasks", json_body={"runId": run_id, "title": "Task B", "deps": [task_a_id]}))
    task_b_id = _req_str(task_b, "id", "taskId")
    assert task_b.get("status") == "pending"
    sub = cli.run("orchestration", "task-create", "--run", run_id, "--title", "Subtask", "--parent", task_a_id).assert_ok()
    sub_id = _req_str(sub, "id", "taskId")
    assert sub.get("parentId") == task_a_id
    disp = api.assert_ok(api.post("/api/v1/orca/orchestration/workers/start", json_body={"runId": run_id, "taskId": task_a_id, "agent": "claude"}))
    disp_id = _req_str(disp, "id", "dispatchId")
    cli.run("orchestration", "worker-done", "--dispatch", disp_id, "--summary", "done").assert_ok()

    fresh_orca_server.restart()
    api2 = fresh_orca_server.api_client(sess)
    runs = _items(api2.assert_ok(api2.get("/api/v1/orca/orchestration/runs")), "runs", "items")
    found = [r for r in runs if (r.get("id") or r.get("runId")) == run_id]
    assert len(found) == 1
    tasks = _items(api2.assert_rpc_ok("orchestration.task.list", {"runId": run_id}), "tasks", "items")
    by_id = {(t.get("id") or t.get("taskId")): t for t in tasks}
    assert by_id[task_a_id].get("status") == "completed"
    assert by_id[task_b_id].get("status") == "ready"
    assert sub_id in by_id and by_id[sub_id].get("parentId") == task_a_id


@pytest.mark.tier1
@pytest.mark.f11
def test_t1_f11_mailbox_all_nine_message_types_and_priorities(
    orca_api: OrcaApiClient, orca_cli: OrcaCliRunner
) -> None:
    run_id = _req_str(orca_api.assert_ok(orca_api.post("/api/v1/orca/orchestration/runs", json_body={"title": _uid("mb")})), "id", "runId")
    task_id = _req_str(orca_api.assert_ok(orca_api.post("/api/v1/orca/orchestration/tasks", json_body={"runId": run_id, "title": "T"})), "id", "taskId")
    disp_id = _req_str(orca_api.assert_ok(orca_api.post("/api/v1/orca/orchestration/workers/start", json_body={"runId": run_id, "taskId": task_id, "agent": "claude"})), "id", "dispatchId")
    priorities = ("normal", "high", "urgent")
    for idx, msg_type in enumerate(ORCHESTRATION_MESSAGE_TYPES_9):
        prio = priorities[idx % 3]
        if idx % 2 == 0:
            orca_api.assert_ok(orca_api.post("/api/v1/orca/orchestration/messages", json_body={"runId": run_id, "to": f"dispatch:{disp_id}", "type": msg_type, "priority": prio, "body": f"msg-{msg_type}"}))
        else:
            orca_cli.run("orchestration", "send", "--run", run_id, "--to", f"dispatch:{disp_id}", "--type", msg_type, "--priority", prio, "--body", f"msg-{msg_type}").assert_ok()
    rest_msgs = _items(orca_api.assert_ok(orca_api.get("/api/v1/orca/orchestration/mailbox", params={"runId": run_id})), "messages", "items")
    cli_msgs = _items(orca_cli.run("orchestration", "check", "--run", run_id).assert_ok(), "messages", "items")
    assert {m.get("type") for m in rest_msgs} >= set(ORCHESTRATION_MESSAGE_TYPES_9)
    assert {m.get("type") for m in cli_msgs} >= set(ORCHESTRATION_MESSAGE_TYPES_9)
    assert {m.get("priority") for m in rest_msgs} >= set(priorities)
    seqs = [int(m["seq"]) for m in rest_msgs if "seq" in m]
    assert len(seqs) >= 9 and seqs == sorted(seqs)
    wake_types = {"question", "decision_gate", "escalation", "worker_done"}
    for msg in rest_msgs:
        if msg.get("type") in wake_types:
            assert msg.get("wakeCoordinator") is True or msg.get("wake") is True


@pytest.mark.tier1
@pytest.mark.f11
def test_t1_f11_group_addressing_all_idle_agent_and_worktree(
    orca_api: OrcaApiClient, orca_cli: OrcaCliRunner
) -> None:
    run_id = _req_str(orca_api.assert_ok(orca_api.post("/api/v1/orca/orchestration/runs", json_body={"title": _uid("grp")})), "id", "runId")
    specs = [("claude", "wt-a"), ("codex", "wt-a"), ("claude", "wt-b")]
    dispatches: list[str] = []
    for idx, (agent, wt) in enumerate(specs):
        tid = _req_str(orca_api.assert_ok(orca_api.post("/api/v1/orca/orchestration/tasks", json_body={"runId": run_id, "title": f"W{idx}"})), "id", "taskId")
        d = orca_api.assert_ok(orca_api.post("/api/v1/orca/orchestration/workers/start", json_body={"runId": run_id, "taskId": tid, "agent": agent, "worktree": wt, "keepOpen": True}))
        dispatches.append(_req_str(d, "id", "dispatchId"))
    w1, w2, w3 = dispatches
    orca_cli.run("orchestration", "worker-done", "--dispatch", w1, "--summary", "idle").assert_ok()
    for target, tag in (("@all", "m-all"), ("@idle", "m-idle"), ("@agent:codex", "m-codex"), ("@worktree:wt-a", "m-wta")):
        orca_api.assert_ok(orca_api.post("/api/v1/orca/orchestration/messages", json_body={"runId": run_id, "to": target, "type": "status", "body": tag}))
    delivered: dict[str, set[str]] = {}
    for wid in dispatches:
        msgs = _items(orca_api.assert_ok(orca_api.get("/api/v1/orca/orchestration/mailbox", params={"runId": run_id, "dispatchId": wid})), "messages", "items")
        delivered[wid] = {str(m.get("body")) for m in msgs}
    assert all("m-all" in delivered[w] for w in (w1, w2, w3))
    assert "m-idle" in delivered[w1] and "m-idle" not in delivered[w2] and "m-idle" not in delivered[w3]
    assert "m-codex" in delivered[w2] and "m-codex" not in delivered[w1] and "m-codex" not in delivered[w3]
    assert "m-wta" in delivered[w1] and "m-wta" in delivered[w2] and "m-wta" not in delivered[w3]


@pytest.mark.tier1
@pytest.mark.f11
def test_t1_f11_decision_gates_and_questions_push_wake_roundtrip(
    orca_api: OrcaApiClient, orca_cli: OrcaCliRunner
) -> None:
    run_id = _req_str(orca_api.assert_ok(orca_api.post("/api/v1/orca/orchestration/runs", json_body={"title": _uid("gate")})), "id", "runId")
    task_id = _req_str(orca_api.assert_ok(orca_api.post("/api/v1/orca/orchestration/tasks", json_body={"runId": run_id, "title": "GateTask"})), "id", "taskId")
    disp_id = _req_str(orca_api.assert_ok(orca_api.post("/api/v1/orca/orchestration/workers/start", json_body={"runId": run_id, "taskId": task_id, "agent": "claude"})), "id", "dispatchId")
    q = orca_cli.run("orchestration", "ask", "--dispatch", disp_id, "--question", "Use WAL mode?").assert_ok()
    assert isinstance(q, dict) and q.get("status") == "pending"
    gate = orca_api.assert_ok(orca_api.post("/api/v1/orca/orchestration/gates", json_body={"runId": run_id, "dispatchId": disp_id, "prompt": "Approve schema migration?", "options": ["approve", "defer"]}))
    gate_id = _req_str(gate, "id", "gateId")
    assert gate.get("status") == "pending" and (gate.get("wakeQueued") is True or gate.get("wakeCoordinator") is True)
    resolved = orca_api.assert_ok(orca_api.post(f"/api/v1/orca/orchestration/gates/{gate_id}/resolve", json_body={"decision": "approve", "rationale": "Verified"}))
    assert resolved.get("status") == "resolved" and resolved.get("decision") == "approve"
    cli_gate = orca_cli.run("orchestration", "gate-create", "--run", run_id, "--dispatch", disp_id, "--prompt", "Ship?", "--options", "yes,no").assert_ok()
    cli_gate_id = _req_str(cli_gate, "id", "gateId")
    cli_res = orca_cli.run("orchestration", "gate-resolve", "--gate", cli_gate_id, "--decision", "yes").assert_ok()
    assert isinstance(cli_res, dict) and cli_res.get("status") == "resolved"
    inbox = orca_cli.run("orchestration", "worker-read", "--dispatch", disp_id).assert_ok()
    assert gate_id in str(inbox) and "approve" in str(inbox)


@pytest.mark.tier1
@pytest.mark.f11
def test_t1_f11_worker_auto_teardown_keep_open_and_followup_reopen(
    orca_api: OrcaApiClient, orca_cli: OrcaCliRunner
) -> None:
    run_id = _req_str(orca_api.assert_ok(orca_api.post("/api/v1/orca/orchestration/runs", json_body={"title": _uid("custody")})), "id", "runId")
    t1 = _req_str(orca_api.assert_ok(orca_api.post("/api/v1/orca/orchestration/tasks", json_body={"runId": run_id, "title": "AutoClose"})), "id", "taskId")
    t2 = _req_str(orca_api.assert_ok(orca_api.post("/api/v1/orca/orchestration/tasks", json_body={"runId": run_id, "title": "KeepOpen"})), "id", "taskId")
    w1 = _req_str(orca_api.assert_ok(orca_api.post("/api/v1/orca/orchestration/workers/start", json_body={"runId": run_id, "taskId": t1, "agent": "claude", "keepOpen": False})), "id", "dispatchId")
    w2 = _req_str(orca_api.assert_ok(orca_api.post("/api/v1/orca/orchestration/workers/start", json_body={"runId": run_id, "taskId": t2, "agent": "codex", "keepOpen": True})), "id", "dispatchId")
    orca_cli.run("orchestration", "worker-done", "--dispatch", w1, "--summary", "ok").assert_ok()
    orca_cli.run("orchestration", "worker-done", "--dispatch", w2, "--summary", "ok").assert_ok()
    s1 = orca_api.assert_rpc_ok("orchestration.worker.show", {"dispatchId": w1})
    s2 = orca_api.assert_rpc_ok("orchestration.worker.show", {"dispatchId": w2})
    assert s1.get("terminalClosed") is True and s2.get("terminalClosed") is False
    orca_cli.run("orchestration", "worker-release", "--dispatch", w2).assert_ok()
    assert orca_api.assert_rpc_ok("orchestration.worker.show", {"dispatchId": w2}).get("terminalClosed") is True
    followup = orca_cli.run("orchestration", "send", "--run", run_id, "--to", f"dispatch:{w1}", "--body", "Add regression test").assert_ok()
    assert isinstance(followup, dict) and followup.get("followUpOfDispatchId") == w1


@pytest.mark.tier1
@pytest.mark.f11
def test_t1_f11_idempotent_mutation_receipts_and_cross_host_federation(
    orca_api: OrcaApiClient, orca_cli: OrcaCliRunner
) -> None:
    run_id = _req_str(orca_api.assert_ok(orca_api.post("/api/v1/orca/orchestration/runs", json_body={"title": _uid("fed")})), "id", "runId")
    t1 = _req_str(orca_api.assert_ok(orca_api.post("/api/v1/orca/orchestration/tasks", json_body={"runId": run_id, "title": "Idem"})), "id", "taskId")
    t2 = _req_str(orca_api.assert_ok(orca_api.post("/api/v1/orca/orchestration/tasks", json_body={"runId": run_id, "title": "Remote"})), "id", "taskId")
    op_id = _uid("req-idem")
    body = {"runId": run_id, "taskId": t1, "agent": "claude", "operationId": op_id, "requestId": op_id, "callerFingerprint": "fp-desktop-1"}
    d1 = orca_api.assert_ok(orca_api.post("/api/v1/orca/orchestration/workers/start", json_body=body))
    d2 = orca_api.assert_ok(orca_api.post("/api/v1/orca/orchestration/workers/start", json_body=body))
    assert _req_str(d1, "id", "dispatchId") == _req_str(d2, "id", "dispatchId")

    peer_id = _uid("peer-west")
    orca_api.assert_rpc_ok("orchestration.federation.registerPeer", {"serverId": peer_id, "supportsServerMail": True})
    fed = orca_cli.run("orchestration", "worker-start", "--run", run_id, "--task", t2, "--on", peer_id, "--agent", "claude").assert_ok()
    assert isinstance(fed, dict) and fed.get("targetServer") == peer_id
    assert fed.get("homeAutoReleaseAt") is not None
    relays = fed.get("federationRelayItems")
    assert isinstance(relays, list) and len(relays) >= 1
    assert relays[0].get("direction") == "to_worker" and int(relays[0].get("seq", 0)) >= 1
