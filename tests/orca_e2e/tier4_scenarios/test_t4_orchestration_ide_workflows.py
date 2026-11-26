"""Tier 4 realistic end-to-end workflows (T4-06..T4-10): Orchestration, IDE, Vault & Plugins."""

from __future__ import annotations

import base64
import json
import stat
import sys
from pathlib import Path
from typing import Any

import pytest

from harness import (
    CLOSED_TAB_TOMBSTONE_DAYS, ORCHESTRATION_MESSAGE_TYPES_9, PLUGIN_ACTION_FALLBACK_TIMEOUT_SEC,
    PLUGIN_APPROVAL_REQUIRED_EXIT_CODE, PLUGIN_TIERS_4, REPO_ROOT,
    SHARED_MCP_FAILED_START_DEBOUNCE_MS, USAGE_PROVIDERS_9, GitRepoFixture, OrcaApiClient,
    OrcaCliRunner, OrcaServerHandle, RawHttpClient, SqliteFixtureHelper, TempWorkspace,
)

TINY_PNG_B64 = "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mP8/x8AAwMCAO+ip1sAAAAASUVORK5CYII="


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
    assert resp.status in (200, 400, 403, 409, 422, 426) and resp.status not in (404, 500, 501), f"Bad RPC status ({resp.status}): {resp.text[:300]}"
    body = resp.json()
    assert isinstance(body, dict) and body.get("ok") is False and str(_req(_req(body, "error"), "code")) in codes, f"Unexpected RPC error: {body!r}"


def _attr_map(attr: Any) -> dict[int, str]:
    rows = _req(attr, "lines", "entries")
    assert isinstance(rows, list), f"Expected attribution list, got {rows!r}"
    return {int(e["line"]): str(e["author_kind"]) for e in rows}


def _extract_relay_grant(api: OrcaApiClient, bot_id: str) -> tuple[int, str, str]:
    info = api.assert_rpc_ok("bots.tools.relayInfo", {"botId": bot_id})
    grant = api.assert_rpc_ok("bots.tools.grant", {"botId": bot_id})
    port = int(_req(info if isinstance(info, dict) and "port" in info else grant, "port", "relayPort"))
    token = str(_req(grant, "token", "bearer", "bearerToken"))
    base = str((grant.get("path") or grant.get("basePath") or info.get("basePath") or "") if isinstance(grant, dict) and isinstance(info, dict) else "")
    assert port > 0 and len(token) >= 8
    return port, token, base


@pytest.mark.tier4
@pytest.mark.f10
@pytest.mark.f11
@pytest.mark.f21
@pytest.mark.edge_case("SM1-01")
@pytest.mark.edge_case("SM1-02")
@pytest.mark.edge_case("SM1-10")
@pytest.mark.edge_case("SM2-22")
def test_t4_06_multi_agent_feature_delivery_dag_and_reopen(orca_api: OrcaApiClient, orca_cli: OrcaCliRunner, git_repo: GitRepoFixture) -> None:
    """Workflow #6: orca.yaml/.worktreeinclude -> 2-worktree DAG -> gate/mailbox -> reopen."""
    git_repo.commit_file("README.md", "# Feature Delivery\n", msg="init")
    git_repo.create_ignored_file(".env.local", "DB=local\n")
    git_repo.create_ignored_file("secret.glob_skip", "SKIP=1\n", ignore_pattern="*.glob_skip")
    git_repo.write_worktreeinclude([".env.local", "*.glob_skip", "../escape"], commit=True)
    orca_yaml = (
        "scripts:\n  setup: echo ok\n  archive: echo bye\n"
        "setupAgentStartupPolicy: wait-for-setup\n"
        "worktree:\n  sharedDirectories:\n    - .cache\n"
    )
    yaml_path = git_repo.write_orca_yaml(orca_yaml, commit=True)

    parsed = _post(orca_api, "/api/v1/orca/ide/orca-yaml/parse", {"path": str(yaml_path), "content": orca_yaml})
    assert len(str(_req(parsed, "setupTrustHash", "setup_trust_hash"))) >= 16
    assert str(_req(_unwrap(orca_cli.run("worktree", "parse-yaml", "--path", str(yaml_path)).assert_ok()), "setupTrustHash", "setup_trust_hash"))
    orca_api.post("/api/v1/orca/ide/orca-yaml/parse", json_body={"content": "scripts:\n  setup: 1\n  setup: 2\n"}).assert_error_code("yaml_duplicate_key", 400, 422)

    wt_api = _unwrap(_post(orca_api, "/api/v1/orca/ide/worktrees", {"repoPath": str(git_repo.path), "name": "feat-api"}), "worktree")
    wt_ui = _unwrap(_post(orca_api, "/api/v1/orca/ide/worktrees", {"repoPath": str(git_repo.path), "name": "feat-ui"}), "worktree")
    wt_api_id, wt_ui_id = str(_req(wt_api, "id", "worktreeId", "worktree_id")), str(_req(wt_ui, "id", "worktreeId", "worktree_id"))
    wt_api_path = Path(str(_req(wt_api, "path", "worktreePath", "worktree_path")))
    env_copy = wt_api_path / ".env.local"
    assert env_copy.is_file() and not env_copy.is_symlink() and env_copy.read_text(encoding="utf-8") == "DB=local\n"
    assert not (wt_api_path / "secret.glob_skip").exists() and _req(wt_api, "warnings")

    run_id = str(_req(_unwrap(_post(orca_api, "/api/v1/orca/orchestration/runs", {"title": "feature-delivery"}), "run"), "id", "runId", "run_id"))
    task_api = _unwrap(_post(orca_api, "/api/v1/orca/orchestration/tasks", {"runId": run_id, "title": "Build API", "deps": []}), "task")
    task_api_id = str(_req(task_api, "id", "taskId", "task_id"))
    assert _req(task_api, "status") == "ready"
    task_ui = _unwrap(_post(orca_api, "/api/v1/orca/orchestration/tasks", {"runId": run_id, "title": "Build UI", "deps": [task_api_id]}), "task")
    task_ui_id = str(_req(task_ui, "id", "taskId", "task_id"))
    assert _req(task_ui, "status") in ("pending", "blocked")

    start_body = {"runId": run_id, "taskId": task_api_id, "worktree": wt_api_id, "agent": "claude", "keepOpen": False, "operationId": "op-t4-06-w1"}
    w1 = _unwrap(_post(orca_api, "/api/v1/orca/orchestration/workers/start", start_body), "worker", "dispatch")
    w1_dup = _unwrap(_post(orca_api, "/api/v1/orca/orchestration/workers/start", start_body), "worker", "dispatch")
    d1_id = str(_req(w1, "dispatchId", "dispatch_id", "id"))
    assert d1_id == str(_req(w1_dup, "dispatchId", "dispatch_id", "id"))

    orca_api.assert_rpc_ok("orchestration.worker.reportLiveness", {"dispatchId": d1_id, "liveness": "unverifiable", "reason": "heartbeat_timeout"})
    shown = orca_api.assert_rpc_ok("orchestration.worker.show", {"dispatchId": d1_id})
    assert _req(shown, "liveness") == "unverifiable" and _req(shown, "taskStatus", "status") == "dispatched"
    assert int(_req(shown, "activeDispatchCount", "active_dispatch_count")) == 1

    assert "status" in ORCHESTRATION_MESSAGE_TYPES_9 and "question" in ORCHESTRATION_MESSAGE_TYPES_9
    _post(orca_api, "/api/v1/orca/orchestration/messages", {"runId": run_id, "dispatchId": d1_id, "to": "coordinator", "type": "status", "body": "Migration drafted"})
    _post(orca_api, "/api/v1/orca/orchestration/messages", {"runId": run_id, "dispatchId": d1_id, "to": "coordinator", "type": "question", "body": "Approve column rename?"})
    gate = _unwrap(_post(orca_api, "/api/v1/orca/orchestration/gates", {"runId": run_id, "dispatchId": d1_id, "prompt": "Approve schema migration", "options": ["approved", "deferred"]}), "gate")
    gate_id = str(_req(gate, "id", "gateId", "gate_id"))
    mb_json = json.dumps(_get(orca_api, "/api/v1/orca/orchestration/mailbox", {"runId": run_id}))
    assert "Approve column rename?" in mb_json and gate_id in mb_json
    resolved = _unwrap(_post(orca_api, f"/api/v1/orca/orchestration/gates/{gate_id}/resolve", {"decision": "approved", "rationale": "Verified"}), "gate")
    assert _req(resolved, "decision", "status") == "approved"

    orca_cli.run("orchestration", "worker-done", "--dispatch", d1_id, "--summary", "done").assert_ok()
    assert _req(orca_api.assert_rpc_ok("orchestration.worker.show", {"dispatchId": d1_id}), "terminalClosed", "terminal_closed", "terminalReleased") is True
    chk = _unwrap(orca_cli.run("orchestration", "check", "--run", run_id).assert_ok())
    chk_msgs = chk if isinstance(chk, list) else _req(chk, "messages", "items")
    assert isinstance(chk_msgs, list) and chk_msgs and "Approve column rename?" in json.dumps(chk_msgs)
    tasks_body = orca_api.assert_rpc_ok("orchestration.task.list", {"runId": run_id})
    by_tid = {str(_req(t, "id", "taskId", "task_id")): t for t in (tasks_body if isinstance(tasks_body, list) else _req(tasks_body, "tasks", "items"))}
    assert _req(by_tid[task_api_id], "status") == "completed" and _req(by_tid[task_ui_id], "status") == "ready"
    assert str(_req(_unwrap(_post(orca_api, "/api/v1/orca/orchestration/workers/start", {"runId": run_id, "taskId": task_ui_id, "worktree": wt_ui_id, "agent": "codex"}), "worker", "dispatch"), "dispatchId", "dispatch_id", "id"))

    reopen = _unwrap(_post(orca_api, "/api/v1/orca/orchestration/messages", {"runId": run_id, "to": f"dispatch:{d1_id}", "type": "dispatch", "body": "Address review nit"}), "message", "dispatch")
    assert str(_req(reopen, "followUpOfDispatchId", "follow_up_of_dispatch_id")) == d1_id


@pytest.mark.tier4
@pytest.mark.f12
@pytest.mark.f13
@pytest.mark.f14
@pytest.mark.f16
@pytest.mark.edge_case("SM1-06")
@pytest.mark.edge_case("SM1-09")
@pytest.mark.edge_case("SM2-23")
def test_t4_07_chat_terminal_continuation_hibernation_and_ai_vault(orca_api: OrcaApiClient, git_repo: GitRepoFixture, temp_workspace: TempWorkspace) -> None:
    """Workflow #7: Chat journal idempotency -> Chat<->Terminal -> cold-park -> hibernation -> tombstone -> Vault fork."""
    git_repo.commit_file("src/retry.rs", "pub fn camelCaseRetryHandler() {}\n", msg="seed retry")
    sess = orca_api.assert_rpc_ok("agentSession.create", {"agent": "claude", "workspacePath": str(git_repo.path), "viewMode": "chat"})
    session_id = str(_req(sess, "sessionId", "id"))

    turn_txt = "Audit camelCaseRetryHandler and snake_case_backoff edge cases"
    t1 = orca_api.assert_rpc_ok("agentSession.send", {"sessionId": session_id, "text": turn_txt}, operation_id="op-t4-07-turn-1")
    t2 = orca_api.assert_rpc_ok("agentSession.send", {"sessionId": session_id, "text": turn_txt}, operation_id="op-t4-07-turn-1")
    assert _req(t1, "sequence", "itemId") == _req(t2, "sequence", "itemId")
    st_before = orca_api.assert_rpc_ok("agentSession.getState", {"sessionId": session_id})
    chain_before = _req(st_before, "providerHandleChain", "provider_handle_chain")
    assert isinstance(chain_before, list) and len(chain_before) >= 1

    orca_api.assert_rpc_ok("nativeChat.toggleViewMode", {"sessionId": session_id, "viewMode": "terminal"})
    orca_api.assert_rpc_ok("nativeChat.toggleViewMode", {"sessionId": session_id, "viewMode": "chat"})
    assert _req(orca_api.assert_rpc_ok("agentSession.getState", {"sessionId": session_id}), "providerHandleChain", "provider_handle_chain") == chain_before
    tx_items = _req(orca_api.assert_rpc_ok("agentSession.getTranscript", {"sessionId": session_id}), "items", "messages", "transcript")
    assert "camelCaseRetryHandler" in json.dumps(tx_items)

    tab_id = f"tab-t4-07-{session_id[:8]}"
    term = orca_api.assert_rpc_ok("terminal.create", {"tabId": tab_id, "cols": 80, "rows": 24, "cwd": str(git_repo.path), "agent": "claude"})
    pty_sid = str(_req(term, "sessionId", "id"))
    try:
        orca_api.assert_rpc_ok("terminal.setSessionBackground", {"sessionId": pty_sid, "background": True, "hiddenDeliveryGate": True})
        orca_api.assert_rpc_ok("terminal.write", {"sessionId": pty_sid, "data": "\x07\x1b]133;D;0\x07https://github.com/acme/repo/pull/77\r\n", "rawOutput": True})
        side = orca_api.assert_rpc_ok("terminal.takeSideEffects", {"sessionId": pty_sid})
        kinds = {str(_req(f, "kind", "type")) for f in _req(side, "effects", "facts", "items")}
        assert {"bell", "command-finished", "pr-link"}.issubset(kinds)
        assert _req(orca_api.assert_rpc_ok("terminal.getViewState", {"sessionId": pty_sid}), "deliveryGated", "background") is True

        orca_api.assert_rpc_ok("agentStatus.ingestHook", {"sessionId": pty_sid, "state": "done", "sessionBoundary": True, "boundaryReason": "/clear"})
        st_b = orca_api.assert_rpc_ok("agentStatus.get", {"sessionId": pty_sid})
        assert int(_req(st_b, "notificationsEmitted")) == 0 and int(_req(st_b, "automationTriggersFired")) == 0 and st_b.get("badgeUnread") is False

        h_body: dict[str, Any] = {
            "sessionId": pty_sid, "state": "done", "tabVisible": False, "keystrokesSinceDone": 0, "agent": "claude",
            "idleMs": 2_400_000, "windowMs": 1_800_000, "mobileDriverActive": False, "unsettledDispatchCount": 1, "liveSubagentCount": 0,
        }
        hb = orca_api.post("/api/v1/orca/ide/hibernation/check", json_body=h_body)
        hb.assert_error_code("hibernation_blocked", 200, 400, 409, 422)
        assert hb.json().get("eligible") is False
        h_body["unsettledDispatchCount"] = 0
        assert _req(_post(orca_api, "/api/v1/orca/ide/hibernation/check", h_body), "eligible") is True
        hib = orca_api.assert_rpc_ok("hibernation.sleep", h_body)
        assert _req(hib, "sleeping") is True and _req(hib, "ptyRunning") is False
        woke = orca_api.assert_rpc_ok("hibernation.wake", {"sessionId": pty_sid})
        assert _req(woke, "sleeping") is False and "--resume" in str(_req(woke, "launchCommand", "command"))
    finally:
        orca_api.rpc("terminal.kill", {"sessionId": pty_sid})
    closed = orca_api.assert_rpc_ok("terminal.closeTab", {"tabId": tab_id})
    assert int(_req(closed, "tombstoneTtlDays", "ttl_days")) == CLOSED_TAB_TOMBSTONE_DAYS
    orca_api.assert_rpc_error("terminal.create", "tab_not_found", {"tabId": tab_id, "cols": 80, "rows": 24})

    t_path = temp_workspace.write(f"transcripts/t4_07_{session_id}.jsonl", "\n".join(json.dumps(m) for m in tx_items) + "\n")
    orca_api.assert_rpc_ok("aiVault.setOptIn", {"enabled": True})
    orca_api.assert_rpc_ok("aiVault.index", {"agent": "claude", "session_id": session_id, "transcript_path": str(t_path), "workspace_path": str(git_repo.path)})
    hits = _get(orca_api, "/api/v1/orca/ide/vault/search", {"q": "camelCaseRetryHandlr"})
    assert session_id in json.dumps(hits)
    forked = orca_api.assert_rpc_ok("aiVault.fork", {"session_id": session_id, "title": "Codex retry fork"})
    assert str(_req(forked, "forked_session_id", "session_id", "id")) != session_id
    handoff = orca_api.assert_rpc_ok("aiVault.handoffPrompt", {"source_session_id": session_id, "target_agent": "codex"})
    assert "camelCaseRetryHandler" in str(_req(handoff, "prompt")) or "retry" in str(_req(handoff, "prompt")).lower()


@pytest.mark.tier4
@pytest.mark.f17
@pytest.mark.f18
@pytest.mark.f22
def test_t4_08_code_review_diff_annotations_browser_design_and_pr_draft(orca_api: OrcaApiClient, orca_cli: OrcaCliRunner, git_repo: GitRepoFixture) -> None:
    """Workflow #8: AI line attribution -> human flip -> shift annotations -> Design Mode -> PR draft."""
    assert base64.b64decode(TINY_PNG_B64).startswith(b"\x89PNG\r\n\x1a\n")
    git_repo.commit_file("src/checkout.rs", "\n".join(f"// checkout {i:02d}" for i in range(1, 21)) + "\n", msg="seed checkout")
    wt = _unwrap(_post(orca_api, "/api/v1/orca/ide/worktrees", {"repoPath": str(git_repo.path), "name": "wt-checkout-rev"}), "worktree")
    wt_path = Path(str(_req(wt, "path", "worktreePath", "worktree_path")))
    chat_id = str(_req(orca_api.assert_rpc_ok("agentSession.create", {"workspacePath": str(wt_path), "agent": "claude", "viewMode": "chat"}), "sessionId", "id"))

    orca_api.assert_rpc_ok("diffReview.recordAttribution", {"repo_path": str(wt_path), "file": "src/checkout.rs", "author_kind": "ai", "agent": "claude", "start_line": 5, "end_line": 14})
    attr1 = _attr_map(orca_api.assert_rpc_ok("diffReview.getAttribution", {"repo_path": str(wt_path), "file": "src/checkout.rs"}))
    assert all(attr1[ln] == "ai" for ln in range(5, 15)) and git_repo.git("status", "--porcelain").stdout.strip() == ""

    orca_api.assert_rpc_ok("diffReview.applyHumanEdit", {"repo_path": str(wt_path), "file": "src/checkout.rs", "start_line": 10, "end_line": 11})
    attr2 = _attr_map(orca_api.assert_rpc_ok("diffReview.getAttribution", {"repo_path": str(wt_path), "file": "src/checkout.rs"}))
    assert attr2[10] == "human" and attr2[11] == "human" and attr2[12] == "ai"

    a1_id = str(_req(_unwrap(_post(orca_api, "/api/v1/orca/ide/diff-annotations", {"repo_path": str(wt_path), "file": "src/checkout.rs", "line": 10, "body": "Validate tax rounding"}), "annotation"), "id", "annotationId", "annotation_id"))
    a2_id = str(_req(_unwrap(_post(orca_api, "/api/v1/orca/ide/diff-annotations", {"repo_path": str(wt_path), "file": "src/checkout.rs", "line": 16, "body": "Guard negative qty"}), "annotation"), "id", "annotationId", "annotation_id"))
    co_file = wt_path / "src/checkout.rs"
    co_file.write_text("// hdr 1\n// hdr 2\n// hdr 3\n// hdr 4\n" + co_file.read_text(encoding="utf-8"), encoding="utf-8")
    orca_api.assert_rpc_ok("diffReview.reanchor", {"repo_path": str(wt_path), "file": "src/checkout.rs"})

    batch = _unwrap(orca_cli.run("ide", "diff-batch", "--repo", str(wt_path)).assert_ok())
    batch_json = json.dumps(batch)
    assert "Validate tax rounding" in batch_json and "Guard negative qty" in batch_json and "14" in batch_json and "20" in batch_json

    post_body = _get(orca_api, "/api/v1/orca/ide/diff-annotations", {"repo_path": str(wt_path)})
    post_anns = post_body if isinstance(post_body, list) else _req(post_body, "annotations", "items")
    post_by_id = {str(_req(a, "id", "annotationId", "annotation_id")): a for a in post_anns}
    assert int(_req(post_by_id[a1_id], "line")) == 14 and _req(post_by_id[a1_id], "resolved") is False
    assert int(_req(post_by_id[a2_id], "line")) == 20 and _req(post_by_id[a2_id], "resolved") is False

    dm = orca_api.assert_rpc_ok(
        "browser.designMode.capture",
        {
            "tab_id": "tab-dm-t4-08", "selector": "button.pay",
            "outer_html": '<button class="pay">Pay</button>',
            "dom_neighborhood": '<form><button class="pay">Pay</button></form>',
            "computed_styles": {"color": "#111", "padding": "8px 16px"},
            "crop": {"x": 0, "y": 0, "width": 64, "height": 32},
            "screenshot_png_b64": TINY_PNG_B64, "source_map": {"file": "src/checkout.rs", "line": 14},
        },
    )
    assert "Pay" in json.dumps(dm) and "src/checkout.rs" in json.dumps(dm)
    assert base64.b64decode(str(_req(dm, "screenshot_png_b64", "screenshotPngB64"))).startswith(b"\x89PNG\r\n\x1a\n")
    att = _req(dm, "chat_attachment", "attachment")
    orca_api.assert_rpc_ok("agentSession.send", {"sessionId": chat_id, "text": "Fix Pay button styling", "attachments": [att]})
    chat_tx = json.dumps(orca_api.assert_rpc_ok("agentSession.getTranscript", {"sessionId": chat_id}))
    assert "Pay" in chat_tx and "src/checkout.rs" in chat_tx and (TINY_PNG_B64[:16] not in json.dumps(att) or TINY_PNG_B64[:16] in chat_tx)
    dm_rs = REPO_ROOT / "crates/bot-browser/src/design_mode.rs"
    assert dm_rs.is_file() and "DesignModeCapture" in dm_rs.read_text(encoding="utf-8")

    commit_d = orca_api.assert_rpc_ok("git.draftCommit", {"repo_path": str(wt_path)})
    pr_d = orca_api.assert_rpc_ok("git.draftPr", {"repo_path": str(wt_path), "base_branch": "main"})
    assert str(_req(commit_d, "message", "subject")).strip() and str(_req(pr_d, "body", "title")).strip()
    assert git_repo.git("status", "--porcelain").stdout.strip() == ""


@pytest.mark.tier4
@pytest.mark.f15
@pytest.mark.f16
@pytest.mark.f20
@pytest.mark.edge_case("SM1-04")
@pytest.mark.edge_case("SM1-05")
@pytest.mark.edge_case("SM1-13")
def test_t4_09_multi_account_quota_resilience_and_crash_recovery(
    fresh_orca_server: OrcaServerHandle,
    temp_workspace: TempWorkspace,
) -> None:
    """Workflow #9: 9-provider usage -> hot-swap -> ProfileState CAS -> degraded keyring -> corrupt Vault rebuild."""
    session = fresh_orca_server.register_user()
    api = fresh_orca_server.api_client(session)

    for idx, prov in enumerate(USAGE_PROVIDERS_9):
        pct = 85 if prov == "claude" else (30 if prov == "codex" else 10 + idx * 5)
        api.assert_rpc_ok("usage.ingestSnapshot", {"provider": prov, "percentUsed": pct, "window": "5h", "resetsInSec": 1200, "inferredCostUsd": 2.0})
    usage_body = _get(api, "/api/v1/orca/ide/usage")
    rows = usage_body if isinstance(usage_body, list) else _req(usage_body, "providers", "usage", "items")
    ordered = [str(_req(r, "provider", "id", "name")) for r in rows]
    assert set(USAGE_PROVIDERS_9).issubset(set(ordered)) and ordered[0] == "claude"
    assert _req(rows[0], "warningChip", "warning_chip") is True

    home_1, home_2 = temp_workspace.mkdir("claude_home_1"), temp_workspace.mkdir("claude_home_2")
    api.assert_rpc_ok("accounts.add", {"provider": "claude", "accountId": "claude-1", "homeDir": str(home_1)})
    api.assert_rpc_ok("accounts.add", {"provider": "claude", "accountId": "claude-2", "homeDir": str(home_2)})
    api.assert_rpc_ok("accounts.launchBinding", {"provider": "claude", "surface": "chat", "surfaceId": "chat-t4-09"})
    sw = api.assert_rpc_ok("accounts.setActive", {"provider": "claude", "accountId": "claude-2"})
    assert str(_req(sw, "activeAccountId", "active_account_id", "accountId")) == "claude-2"

    st0 = api.assert_rpc_ok("profileState.write", {"domain": "ui", "payload": {"density": "default"}})
    rev0 = int(_req(st0, "revision"))
    st1 = api.assert_rpc_ok("profileState.write", {"domain": "ui", "expected_revision": rev0, "payload": {"density": "compact"}})
    assert int(_req(st1, "revision")) == rev0 + 1
    _assert_rpc_err_in(api, "profileState.write", ("revision_conflict", "item_changed", "invalid_argument"), {"domain": "ui", "expected_revision": rev0, "payload": {"density": "stale"}})

    raw_tok = "sealed_api_secret_t4_09"
    sec_set = api.assert_rpc_ok("profileState.secretsSave", {"slot": "oauth_tok", "secret": raw_tok})
    orig_digest = str(_req(sec_set, "ciphertext_digest", "sealed_digest"))
    assert orig_digest and raw_tok not in orig_digest
    pre_deg_rev = int(_req(api.assert_rpc_ok("profileState.read", {"domain": "ui"}), "revision"))
    deg = api.assert_rpc_ok("profileState.write", {"domain": "ui", "expected_revision": pre_deg_rev, "keyring_mode": "serve_degraded", "payload": {"density": "compact", "theme": "dark"}})
    deg_rev = int(_req(deg, "revision"))
    assert deg_rev == pre_deg_rev + 1
    sec_st = api.assert_rpc_ok("profileState.secretsStatus", {"simulate_locked": True, "mode": "serve_degraded"})
    assert _req(sec_st, "degraded") is True and str(_req(sec_st, "ciphertext_digest", "sealed_digest")) == orig_digest

    snap_path = temp_workspace.resolve("t4_09_quit.json")
    g_stale = int(_req(api.assert_rpc_ok("profileState.snapshotWrite", {"path": str(snap_path), "stage_async": True, "payload": {"state": "stale"}}), "generation"))
    g_flush = api.assert_rpc_ok("profileState.snapshotWrite", {"path": str(snap_path), "write_sync": True, "payload": {"state": "quit-flush"}})
    assert int(_req(g_flush, "committed_generation")) >= g_stale + 1
    fin = api.rpc("profileState.snapshotWrite", {"path": str(snap_path), "finalize_generation": g_stale, "payload": {"state": "stale"}})
    assert fin.status not in (404, 500, 501)
    fj = fin.json()
    assert isinstance(fj, dict) and (
        (fj.get("ok") is True and isinstance(fj.get("result"), dict) and fj["result"].get("fenced") is True)
        or (fj.get("ok") is False and str(_req(_req(fj, "error"), "code")) in ("stale_generation", "revision_conflict", "invalid_argument"))
    )
    assert json.loads(snap_path.read_text(encoding="utf-8"))["state"] == "quit-flush" and list(snap_path.parent.glob("*.tmp")) == []
    assert "quit-flush" in json.dumps(api.assert_rpc_ok("profileState.snapshotRead", {"path": str(snap_path)}))

    t_path = temp_workspace.write("transcripts/t4_09_rec.jsonl", json.dumps({"role": "assistant", "text": "VaultRecoverySentinel49"}) + "\n")
    api.assert_rpc_ok("aiVault.setOptIn", {"enabled": True})
    api.assert_rpc_ok("aiVault.index", {"agent": "claude", "session_id": "vault-t4-09", "transcript_path": str(t_path), "workspace_path": str(temp_workspace.path)})
    assert "vault-t4-09" in json.dumps(_get(api, "/api/v1/orca/ide/vault/search", {"q": "VaultRecoverySentinel49"}))
    vault_db = Path(str(_req(api.assert_rpc_ok("aiVault.status", {}), "cache_db_path", "dbPath", "db_path")))
    assert vault_db.is_file(), f"Missing AI Vault cache DB: {vault_db}"

    fresh_orca_server.stop(cleanup=False)
    SqliteFixtureHelper.corrupt_header(vault_db, length=100)
    fresh_orca_server.start()
    api = fresh_orca_server.api_client(session)

    assert "vault-t4-09" in json.dumps(_get(api, "/api/v1/orca/ide/vault/search", {"q": "VaultRecoverySentinel49"}))
    vault_after = api.assert_rpc_ok("aiVault.status", {})
    assert int(_req(vault_after, "rebuild_count", "rebuildCount")) == 1
    assert int(_req(api.assert_rpc_ok("profileState.read", {"domain": "ui"}), "revision")) >= deg_rev
    assert str(_req(api.assert_rpc_ok("profileState.secretsStatus", {"simulate_locked": True, "mode": "serve_degraded"}), "ciphertext_digest", "sealed_digest")) == orig_digest
    acct_after = api.assert_rpc_ok("accounts.resolveTurnAccount", {"provider": "claude", "surface": "chat", "surfaceId": "chat-t4-09"})
    assert str(_req(acct_after, "nextTurnAccountId", "activeAccountId", "accountId")) == "claude-2"


@pytest.mark.tier4
@pytest.mark.f08
@pytest.mark.f19
@pytest.mark.f21
@pytest.mark.f22
@pytest.mark.edge_case("SM1-12")
@pytest.mark.edge_case("SM2-14")
def test_t4_10_plugin_v2_4tier_security_mcp_pool_and_skills(orca_api: OrcaApiClient, orca_cli: OrcaCliRunner, temp_workspace: TempWorkspace) -> None:
    """Workflow #10: 4-tier Plugin V2 approval/fallback/SecuritySurface -> shared stdio MCP pool -> skills."""
    assert SHARED_MCP_FAILED_START_DEBOUNCE_MS == 10_000
    plug_dirs: dict[str, Path] = {}
    for tier in PLUGIN_TIERS_4:
        pdir = temp_workspace.mkdir(f"plugin_{tier}")
        manifest: dict[str, Any] = {"schema_version": 2, "publisher": "acme", "id": f"plug-{tier}", "version": "1.0.0", "tier": tier, "contributes": {}}
        if tier == "data":
            manifest["contributes"]["themes"] = [{"id": "override", "path": "theme.json"}]
            (pdir / "theme.json").write_text(json.dumps({"securitySurface.bg": "#ff0000"}), encoding="utf-8")
        elif tier == "trusted-worker":
            manifest["capabilities"] = ["storage", "hooks.agentLaunch"]
            manifest["contributes"]["actions"] = [{"id": "git.stage", "timeoutMs": 3000}]
            manifest["contributes"]["hooks"] = [{"event": "agentLaunch", "onError": "deny"}, {"event": "agentLaunch", "onError": "pass"}]
            (pdir / "worker.js").write_text("export function main() {}", encoding="utf-8")
        (pdir / "orca-plugin.json").write_text(json.dumps(manifest), encoding="utf-8")
        plug_dirs[tier] = pdir

    for free_tier in ("data", "declarative", "sandboxed-ui"):
        orca_cli.run("ide", "plugin-install", "--path", str(plug_dirs[free_tier])).assert_ok()
    listed_json = json.dumps(_get(orca_api, "/api/v1/orca/ide/plugins"))
    for free_tier in ("data", "declarative", "sandboxed-ui"):
        assert f"plug-{free_tier}" in listed_json

    worker_res = orca_cli.run("ide", "plugin-install", "--path", str(plug_dirs["trusted-worker"]))
    worker_res.assert_error("approval_required", expected_exit=PLUGIN_APPROVAL_REQUIRED_EXIT_CODE)
    err_payload = worker_res.json()
    assert "fingerprint" in json.dumps(err_payload).lower()
    err_dict = err_payload.get("error") if isinstance(err_payload.get("error"), dict) else err_payload
    fp_raw = next((d.get(k) for d in (err_dict, err_payload) if isinstance(d, dict) for k in ("capability_fingerprint", "fingerprint") if d.get(k)), None)
    if fp_raw is None:
        val = _unwrap(orca_cli.run("ide", "plugin-validate", "--path", str(plug_dirs["trusted-worker"])).assert_ok())
        fp_raw = _req(val, "capability_fingerprint", "fingerprint")
    fingerprint = str(fp_raw)
    assert len(fingerprint) >= 16
    approved = _unwrap(orca_cli.run("ide", "plugin-install", "--path", str(plug_dirs["trusted-worker"]), "--approve", fingerprint).assert_ok(), "plugin")
    assert "acme.plug-trusted-worker" in json.dumps(approved)

    act = orca_api.assert_rpc_ok("plugins.invokeAction", {"action": "git.stage", "simulate_plugin_delay_ms": (PLUGIN_ACTION_FALLBACK_TIMEOUT_SEC + 2) * 1000})
    assert _req(act, "fallback_used", "fallbackUsed") is True and _req(act, "builtin_executed", "builtinExecuted") is True
    assert _req(orca_api.assert_rpc_ok("plugins.evaluateHook", {"event": "agentLaunch", "onError": "deny", "simulate_crash": True}), "allowed") is False
    assert _req(orca_api.assert_rpc_ok("plugins.evaluateHook", {"event": "agentLaunch", "onError": "pass", "simulate_crash": True}), "allowed") is True
    sec_surface = orca_api.assert_rpc_ok("plugins.resolveThemeTokens", {"surface": "security"})
    assert _req(sec_surface, "security_surface_protected", "themeOverrideBlocked") is True and "#ff0000" not in json.dumps(sec_surface).lower()

    probe_py = temp_workspace.write(
        "t4_10_mcp_probe.py",
        f"#!{sys.executable}\nimport json, os, sys\nfor line in sys.stdin:\n    line = line.strip()\n    if not line: continue\n"
        "    req = json.loads(line); rid = req.get('id')\n"
        "    res = {'content': [{'type': 'text', 'text': json.dumps({'pid': os.getpid(), 'env': sorted(os.environ.keys())})}]}\n"
        "    if rid is not None: sys.stdout.write(json.dumps({'jsonrpc': '2.0', 'id': rid, 'result': res}) + '\\n'); sys.stdout.flush()\n",
    )
    probe_py.chmod(probe_py.stat().st_mode | stat.S_IXUSR)
    orca_api.assert_rpc_ok("mcpServers.add", {"name": "shared_t4_10", "command": sys.executable, "args": [str(probe_py)], "env": {"MCP_MODE": "shared"}})
    bot_a = str(_req(_unwrap(_post(orca_api, "/api/v1/orca/bots", {"name": "Pool Bot A", "agent": "claude"}), "bot"), "id", "botId", "bot_id"))
    bot_b = str(_req(_unwrap(_post(orca_api, "/api/v1/orca/bots", {"name": "Pool Bot B", "agent": "claude"}), "bot"), "id", "botId", "bot_id"))
    for bid in (bot_a, bot_b):
        orca_api.assert_rpc_ok("bots.tools.setHostMcpServers", {"botId": bid, "enabled": True})

    def _probe(bid: str) -> dict[str, Any]:
        port, tok, base = _extract_relay_grant(orca_api, bid)
        body = RawHttpClient("127.0.0.1", port).request("POST", f"{base.rstrip('/')}/shared_t4_10", headers={"Authorization": f"Bearer {tok}"}, json_body={"jsonrpc": "2.0", "id": 1, "method": "tools/call", "params": {"name": "probe", "arguments": {}}}).assert_status(200).json()
        return json.loads(_req(body["result"]["content"][0], "text"))

    pa, pb = _probe(bot_a), _probe(bot_b)
    env_keys = _req(pa, "env")
    assert pa.get("pid") and pa["pid"] == pb.get("pid") and isinstance(env_keys, list)
    assert not (set(env_keys) & {"FABUSHI_TOKEN", "GPUI_TELEGRAM_TOKEN", "ORCA_AUTH_TOKEN", "ORCA_RELAY_SECRET"})

    spawns_log = temp_workspace.resolve("t4_10_spawns.log")
    crash_py = temp_workspace.write("t4_10_crash.py", f"#!{sys.executable}\nimport os, sys\nwith open({str(spawns_log)!r}, 'a', encoding='utf-8') as f: f.write(f'{{os.getpid()}}\\n')\nsys.exit(1)\n")
    crash_py.chmod(crash_py.stat().st_mode | stat.S_IXUSR)
    orca_api.assert_rpc_ok("mcpServers.add", {"name": "crash_t4_10", "command": sys.executable, "args": [str(crash_py)], "env": {}})
    port_a, tok_a, base_a = _extract_relay_grant(orca_api, bot_a)
    for idx in range(3):
        r = RawHttpClient("127.0.0.1", port_a).request("POST", f"{base_a.rstrip('/')}/crash_t4_10", headers={"Authorization": f"Bearer {tok_a}"}, json_body={"jsonrpc": "2.0", "id": idx + 1, "method": "tools/list"})
        assert r.status in (500, 502, 503)
    assert spawns_log.is_file() and len(spawns_log.read_text(encoding="utf-8").splitlines()) == 1

    src_skill = temp_workspace.mkdir("skills_src_t4_10/release-readiness")
    (src_skill / "SKILL.md").write_text("# release-readiness\nVerify release.\n", encoding="utf-8")
    target_root = temp_workspace.mkdir("skills_t4_10")
    txn_marker = target_root / ".txn-broken-t4-10"
    txn_marker.write_text('{"skill":"release-readiness","state":"staging"}', encoding="utf-8")
    stage_dir = target_root / "release-readiness.staging"
    stage_dir.mkdir(parents=True, exist_ok=True)
    (stage_dir / "SKILL.md").write_text("# partial\n", encoding="utf-8")
    orca_api.assert_rpc_ok("skills.recover", {"target_root": str(target_root)})
    assert not txn_marker.exists() and not stage_dir.exists()
    orca_api.assert_rpc_ok("skills.install", {"source_path": str(src_skill), "target_root": str(target_root)})
    assert "release-readiness" in json.dumps(orca_api.assert_rpc_ok("skills.list", {"target_root": str(target_root)}))
    assert any(p.is_file() and p.name == "SKILL.md" for p in target_root.rglob("*"))
