"""Tier 3 pairwise cross-feature E2E tests (T3-11..T3-20): IDE & Orchestration interactions."""

from __future__ import annotations

import json
import uuid
from pathlib import Path
from typing import Any

import pytest

from harness import (
    BOT_AGENTS, CLOSED_TAB_TOMBSTONE_DAYS, ORCHESTRATION_MESSAGE_TYPES_9,
    USAGE_PROVIDERS_9, GitRepoFixture, OrcaApiClient, OrcaServerHandle, TempWorkspace,
)


def _req(mapping: Any, *keys: str) -> Any:
    assert isinstance(mapping, dict), f"Expected JSON dict, got {type(mapping).__name__}: {mapping!r}"
    for key in keys:
        if key in mapping and mapping[key] is not None:
            return mapping[key]
    raise AssertionError(f"Missing required key(s) {keys!r} in {sorted(mapping.keys())}: {mapping!r}")


def _unwrap(mapping: Any, *keys: str) -> Any:
    cur = mapping["data"] if isinstance(mapping, dict) and mapping.get("success") is True and "data" in mapping else (
        mapping["result"] if isinstance(mapping, dict) and mapping.get("ok") is True and "result" in mapping else mapping
    )
    return next((cur[k] for k in keys if isinstance(cur, dict) and isinstance(cur.get(k), dict)), cur)


def _get(api: OrcaApiClient, path: str, params: dict[str, Any] | None = None) -> Any:
    return api.assert_ok(api.get(path, params=params))


def _post(api: OrcaApiClient, path: str, body: dict[str, Any]) -> Any:
    return api.assert_ok(api.post(path, json_body=body))


def _assert_rpc_err_in(api: OrcaApiClient, method: str, codes: tuple[str, ...], params: dict[str, Any] | None = None) -> None:
    resp = api.rpc(method, params or {})
    assert resp.status in (200, 400, 403, 409, 422, 426) and resp.status not in (404, 500, 501), f"Bad RPC status {resp.status}: {resp.text[:300]}"
    body = resp.json()
    assert isinstance(body, dict) and body.get("ok") is False, f"Expected ok=false: {body!r}"
    assert str(_req(_req(body, "error"), "code")) in codes, f"Unexpected error code in {body!r}"


def _attr_map(attr: Any) -> dict[int, str]:
    rows = _req(attr, "lines", "entries")
    assert isinstance(rows, list), f"Expected attribution list, got {rows!r}"
    return {int(e["line"]): str(e["author_kind"]) for e in rows}


def _provision_worker(api: OrcaApiClient, git_repo: GitRepoFixture, wt_name: str, *, agent: str = "claude", keep_open: bool = False) -> dict[str, Any]:
    git_repo.commit_file("README.md", f"# {wt_name}\n", msg="seed baseline")
    wt = _unwrap(_post(api, "/api/v1/orca/ide/worktrees", {"repoPath": str(git_repo.path), "name": wt_name}), "worktree")
    wt_id = str(_req(wt, "id", "worktreeId", "worktree_id"))
    run_id = str(_req(_unwrap(_post(api, "/api/v1/orca/orchestration/runs", {"title": f"run-{wt_name}"}), "run"), "id", "runId", "run_id"))
    task_id = str(_req(_unwrap(_post(api, "/api/v1/orca/orchestration/tasks", {"runId": run_id, "title": f"Task {wt_name}", "deps": []}), "task"), "id", "taskId", "task_id"))
    worker = _unwrap(_post(api, "/api/v1/orca/orchestration/workers/start", {"runId": run_id, "taskId": task_id, "worktree": wt_id, "agent": agent, "keepOpen": keep_open}), "worker", "dispatch")
    return {"worktree_id": wt_id, "worktree": wt, "run_id": run_id, "task_id": task_id, "dispatch_id": str(_req(worker, "dispatchId", "dispatch_id", "id")), "worker": worker}


@pytest.mark.tier3
@pytest.mark.f10
@pytest.mark.f11
def test_t3_11_worktree_orca_yaml_include_to_orchestration_task_dag(orca_api: OrcaApiClient, git_repo: GitRepoFixture) -> None:
    """F10 + F11: Strict orca.yaml + .worktreeinclude private copy -> 2-task DAG worker dispatch."""
    git_repo.commit_file("src/main.rs", "fn main() {}\n", msg="seed rust bin")
    git_repo.create_ignored_file(".env.local", "API_MODE=staging\n")
    git_repo.write_worktreeinclude([".env.local"], commit=True)
    orca_yaml = "scripts:\n  setup: echo setup\nsetupAgentStartupPolicy: wait-for-setup\nworktree:\n  sharedDirectories:\n    - .cache\n"
    yaml_path = git_repo.write_orca_yaml(orca_yaml, commit=True)

    parsed = _post(orca_api, "/api/v1/orca/ide/orca-yaml/parse", {"path": str(yaml_path), "content": orca_yaml})
    assert len(str(_req(parsed, "setupTrustHash", "setup_trust_hash"))) >= 16
    orca_api.post("/api/v1/orca/ide/orca-yaml/parse", json_body={"content": "scripts:\n  setup: a\n  setup: b\n"}).assert_error_code("yaml_duplicate_key", 400, 422)

    wt = _unwrap(_post(orca_api, "/api/v1/orca/ide/worktrees", {"repoPath": str(git_repo.path), "name": "feat-dag-worker"}), "worktree")
    wt_id, wt_path = str(_req(wt, "id", "worktreeId", "worktree_id")), Path(str(_req(wt, "path", "worktreePath", "worktree_path")))
    copied_env = wt_path / ".env.local"
    assert wt_path.is_dir() and copied_env.is_file() and not copied_env.is_symlink()
    assert copied_env.read_text(encoding="utf-8") == "API_MODE=staging\n"

    run_id = str(_req(_unwrap(_post(orca_api, "/api/v1/orca/orchestration/runs", {"title": "dag-run"}), "run"), "id", "runId", "run_id"))
    task_a = _unwrap(_post(orca_api, "/api/v1/orca/orchestration/tasks", {"runId": run_id, "title": "Task A", "deps": []}), "task")
    task_a_id = str(_req(task_a, "id", "taskId", "task_id"))
    assert _req(task_a, "status") == "ready"
    task_b = _unwrap(_post(orca_api, "/api/v1/orca/orchestration/tasks", {"runId": run_id, "title": "Task B", "deps": [task_a_id]}), "task")
    task_b_id = str(_req(task_b, "id", "taskId", "task_id"))
    assert _req(task_b, "status") in ("pending", "blocked")

    worker = _unwrap(_post(orca_api, "/api/v1/orca/orchestration/workers/start", {"runId": run_id, "taskId": task_a_id, "worktree": wt_id, "agent": "codex"}), "worker", "dispatch")
    dispatch_id = str(_req(worker, "dispatchId", "dispatch_id", "id"))
    assert _req(worker, "status") in ("dispatched", "running") and "worker_done" in ORCHESTRATION_MESSAGE_TYPES_9

    done_1 = orca_api.assert_rpc_ok("orchestration.worker.done", {"dispatchId": dispatch_id, "summary": "done"}, operation_id="op-t3-11-done")
    done_2 = orca_api.assert_rpc_ok("orchestration.worker.done", {"dispatchId": dispatch_id, "summary": "done"}, operation_id="op-t3-11-done")
    assert _req(done_1, "dispatchId", "dispatch_id", "id", "receiptId", "status") == _req(done_2, "dispatchId", "dispatch_id", "id", "receiptId", "status")
    assert run_id in json.dumps(_get(orca_api, "/api/v1/orca/orchestration/runs", {"runId": run_id}))
    tasks_body = orca_api.assert_rpc_ok("orchestration.task.list", {"runId": run_id})
    tasks = tasks_body if isinstance(tasks_body, list) else _req(tasks_body, "tasks", "items")
    by_id = {str(_req(t, "id", "taskId", "task_id")): t for t in tasks}
    assert _req(by_id[task_a_id], "status") == "completed" and _req(by_id[task_b_id], "status") == "ready"


@pytest.mark.tier3
@pytest.mark.f10
@pytest.mark.f12
def test_t3_12_folder_workspace_native_chat_catalog_and_terminal_flip(orca_api: OrcaApiClient, temp_workspace: TempWorkspace) -> None:
    """F10 + F12: Non-Git Folder Workspace -> 22-agent catalog & Chat <-> Terminal continuation."""
    plain_dir = temp_workspace.mkdir("folder_ws_plain")
    assert not (plain_dir / ".git").exists()
    ws = _unwrap(_post(orca_api, "/api/v1/orca/ide/worktrees", {"kind": "folder", "name": "Scratch Folder", "folderPath": str(plain_dir)}), "worktree", "workspace")
    ws_id = str(_req(ws, "id", "worktreeId", "worktree_id"))
    assert ws_id and _req(orca_api.assert_rpc_ok("folderWorkspace.getPathStatus", {"folderPath": str(plain_dir)}), "exists") is True

    catalog_res = orca_api.assert_rpc_ok("agent.catalog.list", {})
    agents = catalog_res if isinstance(catalog_res, list) else _req(catalog_res, "agents", "items")
    catalog_ids = {str(a if isinstance(a, str) else _req(a, "id", "name", "agent")) for a in agents}
    assert not (set(BOT_AGENTS) - catalog_ids), f"Missing agents: {sorted(set(BOT_AGENTS) - catalog_ids)}"

    session_id = str(_req(orca_api.assert_rpc_ok("agentSession.create", {"agent": "claude", "workspacePath": str(plain_dir), "viewMode": "chat"}), "sessionId", "id"))
    orca_api.assert_rpc_ok("agentSession.send", {"sessionId": session_id, "text": "Summarize directory layout"})
    orca_api.assert_rpc_ok("agentSession.send", {"sessionId": session_id, "text": "List next steps"})
    handle_chain = _req(orca_api.assert_rpc_ok("agentSession.getState", {"sessionId": session_id}), "providerHandleChain", "provider_handle_chain")
    assert isinstance(handle_chain, list) and len(handle_chain) >= 1

    to_term = orca_api.assert_rpc_ok("nativeChat.toggleViewMode", {"sessionId": session_id, "viewMode": "terminal"})
    assert str(_req(to_term, "sessionId", "id")) == session_id and _req(to_term, "viewMode", "view_mode") == "terminal"
    to_chat = orca_api.assert_rpc_ok("nativeChat.toggleViewMode", {"sessionId": session_id, "viewMode": "chat"})
    assert str(_req(to_chat, "sessionId", "id")) == session_id and _req(to_chat, "viewMode", "view_mode") == "chat"
    assert _req(orca_api.assert_rpc_ok("agentSession.getState", {"sessionId": session_id}), "providerHandleChain", "provider_handle_chain") == handle_chain
    transcript = _req(orca_api.assert_rpc_ok("agentSession.getTranscript", {"sessionId": session_id}), "items", "transcript", "messages")
    assert len(transcript) >= 2 and "Summarize directory layout" in json.dumps(transcript)


@pytest.mark.tier3
@pytest.mark.f11
@pytest.mark.f13
@pytest.mark.edge_case("SM2-23")
def test_t3_13_unsettled_worker_dispatch_blocks_hibernation_until_done(orca_api: OrcaApiClient, git_repo: GitRepoFixture) -> None:
    """F11 + F13: Active unsettled worker dispatch blocks 8-check hibernation until worker_done."""
    ctx = _provision_worker(orca_api, git_repo, "wt-hiber-guard", agent="claude", keep_open=True)
    session_id = str(_req(ctx["worker"], "sessionId", "session_id", "tabId", "tab_id"))
    dispatch_id = ctx["dispatch_id"]
    hib_payload: dict[str, Any] = {
        "sessionId": session_id, "dispatchId": dispatch_id, "state": "done", "tabVisible": False,
        "keystrokesSinceDone": 0, "agent": "claude", "idleMs": 1_800_000, "windowMs": 1_800_000,
        "mobileDriverActive": False, "unsettledDispatchCount": 1, "liveSubagentCount": 0,
    }
    blocked = orca_api.post("/api/v1/orca/ide/hibernation/check", json_body=hib_payload)
    blocked.assert_error_code("hibernation_blocked", 200, 400, 409, 422)
    bj = blocked.json()
    assert bj.get("eligible") is False and len(_req(bj, "failedChecks")) >= 1

    orca_api.assert_rpc_ok("orchestration.worker.done", {"dispatchId": dispatch_id, "summary": "done"})
    hib_payload["unsettledDispatchCount"] = 0
    ok_check = _post(orca_api, "/api/v1/orca/ide/hibernation/check", hib_payload)
    assert _req(ok_check, "eligible") is True and len(_req(ok_check, "passedChecks")) == 8

    slept = orca_api.assert_rpc_ok("hibernation.sleep", hib_payload)
    assert _req(slept, "sleeping") is True and _req(slept, "ptyRunning") is False
    resume_sid = str(_req(_req(slept, "resume"), "sessionId", "session_id"))
    woke = orca_api.assert_rpc_ok("hibernation.wake", {"sessionId": session_id})
    launch_cmd = str(_req(woke, "launchCommand", "command"))
    assert _req(woke, "sleeping") is False and "--resume" in launch_cmd and resume_sid in launch_cmd


@pytest.mark.tier3
@pytest.mark.f11
@pytest.mark.f14
@pytest.mark.edge_case("SM1-06")
def test_t3_14_worker_auto_release_writes_30d_closed_tab_tombstone(
    orca_api: OrcaApiClient,
    git_repo: GitRepoFixture,
) -> None:
    """F11 + F14: Worker completion auto-releases terminal custody & writes 30-day tab tombstone."""
    ctx = _provision_worker(orca_api, git_repo, "wt-tombstone", agent="claude", keep_open=False)
    worker = ctx["worker"]
    tab_id = str(_req(worker, "tabId", "tab_id"))
    assert str(_req(worker, "ptyIncarnationId", "pty_incarnation_id", "incarnationId"))

    orca_api.assert_rpc_ok("orchestration.worker.done", {"dispatchId": ctx["dispatch_id"], "summary": "done"})
    shown = orca_api.assert_rpc_ok("orchestration.worker.show", {"dispatchId": ctx["dispatch_id"]})
    assert _req(shown, "terminalClosed", "terminal_closed", "terminalReleased", "terminal_released") is True

    ttl = shown.get("tombstoneTtlDays") or shown.get("tombstone_ttl_days")
    if ttl is None:
        closed = orca_api.assert_rpc_ok("terminal.closeTab", {"tabId": tab_id})
        ttl = _req(closed, "tombstoneTtlDays", "ttl_days")
    assert int(ttl) == CLOSED_TAB_TOMBSTONE_DAYS
    orca_api.assert_rpc_error("terminal.create", "tab_not_found", {"tabId": tab_id, "cols": 80, "rows": 24})


@pytest.mark.tier3
@pytest.mark.f12
@pytest.mark.f15
def test_t3_15_multi_account_hotswap_chat_vs_terminal_and_usage_meter(
    orca_api: OrcaApiClient,
    temp_workspace: TempWorkspace,
) -> None:
    """F12 + F15: Hot-swap affects next native chat turn, not running terminal; 9-provider usage."""
    alpha_dir, beta_dir = temp_workspace.mkdir("acct_alpha"), temp_workspace.mkdir("acct_beta")
    orca_api.assert_rpc_ok("accounts.add", {"provider": "codex", "accountId": "acct_alpha", "homeDir": str(alpha_dir)})
    orca_api.assert_rpc_ok("accounts.add", {"provider": "codex", "accountId": "acct_beta", "homeDir": str(beta_dir)})
    orca_api.assert_rpc_ok("accounts.setActive", {"provider": "codex", "accountId": "acct_alpha"})

    chat = orca_api.assert_rpc_ok("agentSession.create", {"agent": "codex", "workspacePath": str(temp_workspace.path), "viewMode": "chat"})
    chat_id = str(_req(chat, "sessionId", "id"))
    term = orca_api.assert_rpc_ok("terminal.create", {"tabId": f"tab-t3-15-{uuid.uuid4().hex[:8]}", "cols": 80, "rows": 24})
    term_sid = str(_req(term, "sessionId", "id"))
    try:
        orca_api.assert_rpc_ok("accounts.launchBinding", {"provider": "codex", "surface": "terminal", "surfaceId": term_sid})
        orca_api.assert_rpc_ok("accounts.launchBinding", {"provider": "codex", "surface": "chat", "surfaceId": chat_id})
        sw = orca_api.assert_rpc_ok("accounts.setActive", {"provider": "codex", "accountId": "acct_beta"})
        assert str(_req(sw, "activeAccountId", "active_account_id")) == "acct_beta"
        orca_api.assert_rpc_ok("agentSession.send", {"sessionId": chat_id, "text": "Verify swapped account"})

        chat_res = orca_api.assert_rpc_ok("accounts.resolveTurnAccount", {"provider": "codex", "surface": "chat", "surfaceId": chat_id})
        term_res = orca_api.assert_rpc_ok("accounts.resolveTurnAccount", {"provider": "codex", "surface": "terminal", "surfaceId": term_sid})
        assert str(_req(chat_res, "nextTurnAccountId", "next_turn_account_id")) == "acct_beta"
        assert str(_req(term_res, "launchAccountId", "launch_account_id")) == "acct_alpha"
    finally:
        orca_api.rpc("terminal.kill", {"sessionId": term_sid})

    for prov in USAGE_PROVIDERS_9:
        pct = 85 if prov == "codex" else (35 if prov == "claude" else 20)
        orca_api.assert_rpc_ok("usage.ingestSnapshot", {"provider": prov, "percentUsed": pct, "window": "5h", "resetsInSec": 900, "inferredCostUsd": 1.5})
    usage_body = _get(orca_api, "/api/v1/orca/ide/usage")
    providers = usage_body if isinstance(usage_body, list) else _req(usage_body, "providers", "usage", "items")
    ordered_names = [str(_req(p, "provider", "id", "name")) for p in providers]
    assert set(USAGE_PROVIDERS_9).issubset(set(ordered_names))
    assert ordered_names.index("codex") < ordered_names.index("claude")
    codex_row = next(p for p in providers if str(_req(p, "provider", "id", "name")) == "codex")
    assert _req(codex_row, "warningChip", "warning_chip") is True
    assert int(_req(codex_row, "resetsInSec", "resets_in_sec")) > 0


@pytest.mark.tier3
@pytest.mark.f12
@pytest.mark.f16
def test_t3_16_chat_journal_idempotent_receipt_to_ai_vault_fts5_search_and_fork(
    orca_api: OrcaApiClient,
    temp_workspace: TempWorkspace,
) -> None:
    """F12 + F16: Idempotent chat journal -> incremental byte_offset/prefix_hash AI Vault FTS5 & fork."""
    sess = orca_api.assert_rpc_ok("agentSession.create", {"agent": "claude", "workspacePath": str(temp_workspace.path), "viewMode": "chat"})
    session_id = str(_req(sess, "sessionId", "id"))
    op_id = f"op-t3-16-{uuid.uuid4().hex[:8]}"
    turn_text = "Inspect handleWebSocketReconnect and pty_incarnation_fence guards. " + ("context_block " * 320)
    assert len(turn_text.encode("utf-8")) > 4096
    r1 = orca_api.assert_rpc_ok("agentSession.send", {"sessionId": session_id, "text": turn_text}, operation_id=op_id)
    r2 = orca_api.assert_rpc_ok("agentSession.send", {"sessionId": session_id, "text": turn_text}, operation_id=op_id)
    assert _req(r1, "sequence", "itemId", "receiptId") == _req(r2, "sequence", "itemId", "receiptId")
    tx1 = _req(orca_api.assert_rpc_ok("agentSession.getTranscript", {"sessionId": session_id}), "items", "transcript", "messages")
    assert len([m for m in tx1 if "handleWebSocketReconnect" in json.dumps(m)]) == 1

    st1 = orca_api.assert_rpc_ok("agentSession.getState", {"sessionId": session_id})
    managed_path = st1.get("transcriptPath") or st1.get("transcript_path")
    t_path = Path(str(managed_path)) if managed_path else temp_workspace.write(f"transcripts/{session_id}.jsonl", "\n".join(json.dumps(m) for m in tx1) + "\n")
    orca_api.assert_rpc_ok("aiVault.setOptIn", {"enabled": True})
    idx_req = {"agent": "claude", "session_id": session_id, "transcript_path": str(t_path), "workspace_path": str(temp_workspace.path)}
    idx1 = orca_api.assert_rpc_ok("aiVault.index", idx_req)
    off1, hash1 = int(_req(idx1, "byte_offset", "byteOffset")), str(_req(idx1, "prefix_hash", "prefixHash"))
    assert off1 > 4096 and len(hash1) == 64

    orca_api.assert_rpc_ok("agentSession.send", {"sessionId": session_id, "text": "Trace fetchRateLimitQuota handler"})
    if not managed_path:
        with t_path.open("a", encoding="utf-8") as fh:
            fh.write(json.dumps({"role": "user", "text": "Trace fetchRateLimitQuota handler"}) + "\n")
    idx2 = orca_api.assert_rpc_ok("aiVault.index", idx_req)
    assert int(_req(idx2, "byte_offset", "byteOffset")) > off1 and str(_req(idx2, "prefix_hash", "prefixHash")) == hash1

    for q in ("WebSocket Reconnect", "incarnation fence"):
        res = _get(orca_api, "/api/v1/orca/ide/vault/search", params={"q": q})
        assert session_id in json.dumps(res), f"Expected session hit for query {q!r}"

    forked = orca_api.assert_rpc_ok("aiVault.fork", {"session_id": session_id, "title": "fork-t3-16"})
    fork_id = str(_req(forked, "forked_session_id", "session_id", "id"))
    assert fork_id != session_id and str(_req(forked, "parent_session_id", "parentSessionId")) == session_id
    handoff = orca_api.assert_rpc_ok("aiVault.handoffPrompt", {"source_session_id": session_id, "target_agent": "codex"})
    assert "websocket" in str(_req(handoff, "prompt")).lower() or "incarnation" in str(_req(handoff, "prompt")).lower()


@pytest.mark.tier3
@pytest.mark.f10
@pytest.mark.f17
def test_t3_17_worktree_ai_attribution_and_line_shifting_diff_annotations(
    orca_api: OrcaApiClient,
    git_repo: GitRepoFixture,
) -> None:
    """F10 + F17: Parallel worktree local AI attribution + human edit line-shift & flip."""
    src_lines = "\n".join(f"// line {i:02d}" for i in range(1, 21)) + "\n"
    git_repo.commit_file("src/lib.rs", src_lines, msg="seed lib.rs")
    wt = _unwrap(_post(orca_api, "/api/v1/orca/ide/worktrees", {"repoPath": str(git_repo.path), "name": "wt-attrib"}), "worktree")
    wt_path = Path(str(_req(wt, "path", "worktreePath", "worktree_path")))

    orca_api.assert_rpc_ok(
        "diffReview.recordAttribution",
        {"repo_path": str(wt_path), "file": "src/lib.rs", "author_kind": "ai", "agent": "claude", "start_line": 10, "end_line": 14},
    )
    attr1 = _attr_map(orca_api.assert_rpc_ok("diffReview.getAttribution", {"repo_path": str(wt_path), "file": "src/lib.rs"}))
    assert all(attr1[ln] == "ai" for ln in range(10, 15))
    assert git_repo.git("status", "--porcelain").stdout.strip() == ""

    ann = _unwrap(_post(orca_api, "/api/v1/orca/ide/diff-annotations", {"repo_path": str(wt_path), "file": "src/lib.rs", "line": 12, "body": "Check integer overflow"}), "annotation")
    ann_id = str(_req(ann, "id", "annotationId", "annotation_id"))
    lib_file = wt_path / "src/lib.rs"
    lib_file.write_text("// hdr 1\n// hdr 2\n// hdr 3\n" + lib_file.read_text(encoding="utf-8"), encoding="utf-8")
    orca_api.assert_rpc_ok("diffReview.reanchor", {"repo_path": str(wt_path), "file": "src/lib.rs"})
    orca_api.assert_rpc_ok("diffReview.applyHumanEdit", {"repo_path": str(wt_path), "file": "src/lib.rs", "start_line": 14, "end_line": 14})

    anns_body = _get(orca_api, "/api/v1/orca/ide/diff-annotations", params={"repo_path": str(wt_path)})
    anns = anns_body if isinstance(anns_body, list) else _req(anns_body, "annotations", "items")
    shifted = next(a for a in anns if str(_req(a, "id", "annotationId", "annotation_id")) == ann_id)
    assert int(_req(shifted, "line")) == 15 and _req(shifted, "resolved") is False
    attr2 = _attr_map(orca_api.assert_rpc_ok("diffReview.getAttribution", {"repo_path": str(wt_path), "file": "src/lib.rs"}))
    assert attr2[14] == "human" and attr2[15] == "ai"


@pytest.mark.tier3
@pytest.mark.f11
@pytest.mark.f17
def test_t3_18_batched_diff_review_annotations_dispatched_to_worker(
    orca_api: OrcaApiClient,
    git_repo: GitRepoFixture,
) -> None:
    """F11 + F17: Multiple unresolved diff annotations batch into 1 worker mailbox dispatch."""
    ctx = _provision_worker(orca_api, git_repo, "wt-batch-review", agent="claude", keep_open=True)
    wt_path = Path(str(_req(ctx["worktree"], "path", "worktreePath", "worktree_path")))
    dispatch_id = ctx["dispatch_id"]
    created_ids: list[str] = []
    for rel_file, line, body in (
        ("src/a.rs", 10, "Avoid panic on empty slice"),
        ("src/a.rs", 28, "Use checked_add here"),
        ("src/b.rs", 5, "Tighten visibility to pub(crate)"),
        ("src/b.rs", 9, "Already fixed typo"),
    ):
        rec = _unwrap(_post(orca_api, "/api/v1/orca/ide/diff-annotations", {"repo_path": str(wt_path), "file": rel_file, "line": line, "body": body}), "annotation")
        created_ids.append(str(_req(rec, "id", "annotationId", "annotation_id")))
    orca_api.assert_rpc_ok("diffReview.resolve", {"annotation_id": created_ids[3]})

    batch = orca_api.assert_rpc_ok("diffReview.sendBatch", {"repo_path": str(wt_path)})
    prompt = str(_req(batch, "prompt", "text", "body"))
    _post(orca_api, "/api/v1/orca/orchestration/messages", {"runId": ctx["run_id"], "to": f"dispatch:{dispatch_id}", "type": "dispatch", "body": prompt})

    mb_body = _get(orca_api, "/api/v1/orca/orchestration/mailbox", params={"runId": ctx["run_id"], "dispatchId": dispatch_id})
    msgs = mb_body if isinstance(mb_body, list) else _req(mb_body, "messages", "items")
    batch_msgs = [m for m in msgs if "Avoid panic on empty slice" in json.dumps(m)]
    assert len(batch_msgs) == 1, f"Expected single batched prompt, got {len(batch_msgs)}"
    batched_text = json.dumps(batch_msgs[0])
    assert "src/a.rs" in batched_text and "Use checked_add here" in batched_text and "src/b.rs" in batched_text
    assert "Already fixed typo" not in batched_text

    anns_body = _get(orca_api, "/api/v1/orca/ide/diff-annotations", params={"repo_path": str(wt_path)})
    anns = anns_body if isinstance(anns_body, list) else _req(anns_body, "annotations", "items")
    unresolved = [a for a in anns if _req(a, "resolved") is False]
    assert len(unresolved) == 3


@pytest.mark.tier3
@pytest.mark.f13
@pytest.mark.f14
@pytest.mark.edge_case("SM1-09")
def test_t3_19_session_boundary_suppression_and_cold_park_transient_facts(
    orca_api: OrcaApiClient,
    git_repo: GitRepoFixture,
) -> None:
    """F13 + F14: Cold-parked hidden PTY suppresses sessionBoundary alerts & extracts transient facts."""
    git_repo.commit_file("README.md", "# cold-park\n", msg="seed")
    tab_id = f"tab-t3-19-{uuid.uuid4().hex[:8]}"
    term = orca_api.assert_rpc_ok("terminal.create", {"tabId": tab_id, "cols": 80, "rows": 24, "cwd": str(git_repo.path), "agent": "claude"})
    sid = str(_req(term, "sessionId", "id"))
    try:
        orca_api.assert_rpc_ok("terminal.setSessionBackground", {"sessionId": sid, "background": True, "hiddenDeliveryGate": True})
        vs1 = orca_api.assert_rpc_ok("terminal.getViewState", {"sessionId": sid})
        assert _req(vs1, "background") is True and _req(vs1, "deliveryGated") is True
        assert int(_req(vs1, "pushedRawBytesWhileHidden")) == 0

        orca_api.assert_rpc_ok("agentStatus.ingestHook", {"sessionId": sid, "state": "done", "sessionBoundary": True, "boundaryReason": "/clear"})
        st = orca_api.assert_rpc_ok("agentStatus.get", {"sessionId": sid})
        assert int(_req(st, "notificationsEmitted")) == 0 and int(_req(st, "automationTriggersFired")) == 0
        assert st.get("badgeUnread") is False and st.get("turnCompletedAt") is None

        stream = "\x07\x1b]133;D;0\x07Created PR: https://github.com/org/repo/pull/99\r\n"
        orca_api.assert_rpc_ok("terminal.write", {"sessionId": sid, "data": stream, "rawOutput": True})
        feed = orca_api.assert_rpc_ok("terminal.takeSideEffects", {"sessionId": sid})
        facts = _req(feed, "effects", "facts", "items")
        kinds = {str(_req(f, "kind", "type")) for f in facts}
        assert {"command-finished", "pr-link", "bell"}.issubset(kinds) and "pull/99" in json.dumps(facts)
        vs2 = orca_api.assert_rpc_ok("terminal.getViewState", {"sessionId": sid})
        assert _req(vs2, "background") is True and _req(vs2, "deliveryGated") is True
    finally:
        orca_api.rpc("terminal.kill", {"sessionId": sid})


@pytest.mark.tier3
@pytest.mark.f15
@pytest.mark.f16
@pytest.mark.edge_case("SM1-05")
def test_t3_20_multi_account_profile_state_revision_conflict_and_sealed_slots(
    orca_api: OrcaApiClient,
    orca_server: OrcaServerHandle,
    temp_workspace: TempWorkspace,
) -> None:
    """F15 + F16: F15 accounts.add/setActive -> F16 ProfileState revision & degraded-keyring sealedSlots."""
    st0 = orca_api.assert_rpc_ok("profileState.read", {"domain": "accounts"})
    rev0 = int(_req(st0, "revision"))

    home_1, home_2 = temp_workspace.mkdir("t3_20_acct_1"), temp_workspace.mkdir("t3_20_acct_2")
    raw_secret = "raw_oauth_secret_xyz_t320"
    orca_api.assert_rpc_ok("accounts.add", {"provider": "codex", "accountId": "acct-1", "label": "Primary", "homeDir": str(home_1)})
    orca_api.assert_rpc_ok("accounts.add", {"provider": "codex", "accountId": "acct-2", "label": "Work", "homeDir": str(home_2)})
    sw = orca_api.assert_rpc_ok("accounts.setActive", {"provider": "codex", "accountId": "acct-2"})
    assert str(_req(sw, "activeAccountId", "active_account_id")) == "acct-2"
    orca_api.assert_rpc_ok("accounts.launchBinding", {"provider": "codex", "surface": "chat", "surfaceId": "chat-t3-20"})
    sec1 = orca_api.assert_rpc_ok("profileState.secretsSave", {"slot": "codex_acct_2", "secret": raw_secret})
    sealed_digest = str(_req(sec1, "ciphertext_digest", "sealed_digest"))
    assert sealed_digest and raw_secret not in sealed_digest

    st1 = orca_api.assert_rpc_ok("profileState.read", {"domain": "accounts"})
    rev1 = int(_req(st1, "revision"))
    assert rev1 > rev0 and "acct-2" in json.dumps(st1) and raw_secret not in json.dumps(st1)
    assert raw_secret not in json.dumps(orca_api.assert_rpc_ok("profileState.export", {}))
    snap_path = temp_workspace.resolve("t3_20_profile_snap.json")
    orca_api.assert_rpc_ok("profileState.snapshotWrite", {"path": str(snap_path), "payload": {"activeCodex": "acct-2"}})
    scan_roots = (home_1, home_2, temp_workspace.path) if orca_server.is_external else (orca_server.data_dir, home_1, home_2, temp_workspace.path)
    checked_files = 0
    for d in scan_roots:
        for p in d.rglob("*"):
            if p.is_file():
                checked_files += 1
                assert raw_secret.encode("utf-8") not in p.read_bytes(), f"Secret leaked to {p}"
    assert checked_files > 0, f"No files scanned across {scan_roots}; secret check would be vacuous"

    _assert_rpc_err_in(orca_api, "profileState.write", ("revision_conflict", "item_changed", "invalid_argument"), {"domain": "accounts", "expected_revision": rev0, "payload": {"activeCodex": "acct-stale"}})
    cur = orca_api.assert_rpc_ok("profileState.read", {"domain": "accounts"})
    assert int(_req(cur, "revision")) == rev1 and "acct-stale" not in json.dumps(cur)

    _assert_rpc_err_in(orca_api, "profileState.secretsSave", ("keyring_unavailable", "invalid_argument"), {"slot": "codex_acct_2", "secret": raw_secret, "keyring_backend": "basic_text"})
    s2 = orca_api.assert_rpc_ok("profileState.write", {"domain": "accounts", "expected_revision": rev1, "payload": {"activeCodex": "acct-2", "tier": "pro"}})
    rev2 = int(_req(s2, "revision"))
    assert rev2 == rev1 + 1
    assert _req(orca_api.assert_rpc_ok("profileState.secretsStatus", {"simulate_locked": True, "mode": "serve_degraded"}), "degraded") is True
    s3 = orca_api.assert_rpc_ok("profileState.write", {"domain": "accounts", "expected_revision": rev2, "keyring_mode": "serve_degraded", "payload": {"activeCodex": "acct-2", "label": "Work"}})
    assert int(_req(s3, "revision")) == rev2 + 1

    after_sec = orca_api.assert_rpc_ok("profileState.secretsStatus", {"simulate_locked": True, "mode": "serve_degraded"})
    raw_cnt = after_sec.get("sealed_slots_count")
    slots_cnt = int(raw_cnt) if raw_cnt is not None else len(_req(after_sec, "sealed_slots"))
    assert _req(after_sec, "degraded") is True and slots_cnt >= 1
    assert str(_req(after_sec, "ciphertext_digest", "sealed_digest")) == sealed_digest and raw_secret not in json.dumps(after_sec)

    listed = orca_api.assert_rpc_ok("accounts.list", {"provider": "codex"})
    assert "acct-1" in json.dumps(listed) and "acct-2" in json.dumps(listed)
    turn_acct = orca_api.assert_rpc_ok("accounts.resolveTurnAccount", {"provider": "codex", "surface": "chat", "surfaceId": "chat-t3-20"})
    assert str(_req(turn_acct, "nextTurnAccountId", "activeAccountId", "accountId")) == "acct-2"
