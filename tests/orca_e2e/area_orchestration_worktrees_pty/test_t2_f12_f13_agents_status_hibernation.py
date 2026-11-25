"""Tier 2 opaque-box E2E tests for Area E3: F12 (CLI Agents & Journal) and F13 (Status & Hibernation)."""

from __future__ import annotations

import time
import uuid
from pathlib import Path
from typing import Any
from urllib.parse import urlparse

import pytest

from harness import (
    BOT_AGENTS,
    IDE_ERROR_CODES,
    MIN_COMPATIBLE_PROTOCOL_VERSION,
    RUNTIME_PROTOCOL_VERSION,
    HttpResponse,
    OrcaApiClient,
    OrcaCliRunner,
    OrcaServerHandle,
    RawHttpClient,
    TempWorkspace,
)

_UNIMPL_CODES = ("not_implemented", "unknown_method", "method_not_found")


def _uid(prefix: str) -> str:
    return f"{prefix}-{uuid.uuid4().hex[:8]}"


def _req_str(obj: Any, *keys: str) -> str:
    assert isinstance(obj, dict), f"Expected dict payload, got {obj!r}"
    for key in keys:
        val = obj.get(key)
        if isinstance(val, str) and val:
            return val
    raise AssertionError(f"Missing non-empty string key in {keys!r}: {obj!r}")


def _list_field(obj: Any, *keys: str) -> list[Any]:
    if isinstance(obj, list):
        return obj
    assert isinstance(obj, dict), f"Expected list/dict payload, got {obj!r}"
    for key in keys:
        val = obj.get(key)
        if isinstance(val, list):
            return val
    raise AssertionError(f"Missing list key in {keys!r}: {obj!r}")


def _assert_rpc_rejected(
    resp: HttpResponse,
    allowed_codes: tuple[str, ...] | None = None,
    statuses: tuple[int, ...] = (200, 400, 403, 409, 422),
) -> dict[str, Any]:
    resp.assert_status(*statuses)
    body = resp.json()
    assert isinstance(body, dict) and body.get("ok") is False, f"Expected ok=false: {body!r}"
    err = body.get("error")
    assert isinstance(err, dict), f"Expected error dict: {body!r}"
    code = err.get("code")
    assert isinstance(code, str) and code and code not in _UNIMPL_CODES, f"Bad RPC error code {code!r}: {body!r}"
    if allowed_codes is not None:
        assert code in allowed_codes, f"Unexpected RPC error code {code!r} not in {allowed_codes!r}: {body!r}"
    return err


# ---------------------------------------------------------------------------
# F12 Tier 2 — Continuation Refusal, Receipts/Fences, Tombstones & Protocol Floor
# ---------------------------------------------------------------------------


@pytest.mark.tier2
@pytest.mark.f12
def test_t2_f12_active_cli_continuation_refusal_until_terminal_stopped(
    orca_api: OrcaApiClient, temp_workspace: TempWorkspace
) -> None:
    sess = orca_api.assert_rpc_ok(
        "agentSession.create",
        {"agent": "claude", "workspacePath": str(temp_workspace.path), "viewMode": "terminal", "terminalActive": True},
    )
    sid = _req_str(sess, "sessionId", "id")

    refused = orca_api.rpc("agentSession.continueInChat", {"sessionId": sid})
    err = _assert_rpc_rejected(refused)
    assert any(tok in str(err).lower() for tok in ("active", "terminal", "stop", "owner"))

    orca_api.assert_rpc_ok("agentSession.stopTerminalOwner", {"sessionId": sid})
    forked = orca_api.assert_rpc_ok("agentSession.continueInChat", {"sessionId": sid})
    assert _req_str(forked, "sessionId", "id") != sid
    assert isinstance(forked.get("providerHandleChain"), list) and len(forked["providerHandleChain"]) >= 1


@pytest.mark.tier2
@pytest.mark.f12
def test_t2_f12_command_receipt_caller_key_isolation_and_fence_regression(
    orca_api: OrcaApiClient, temp_workspace: TempWorkspace
) -> None:
    sid = _req_str(orca_api.assert_rpc_ok("agentSession.create", {"agent": "claude", "workspacePath": str(temp_workspace.path), "viewMode": "chat"}), "sessionId", "id")
    shared_op = _uid("op-shared")
    orca_api.assert_rpc_ok("agentSession.send", {"sessionId": sid, "text": "Payload A", "callerKey": "caller-A"}, operation_id=shared_op)

    # Same (operation_id, caller_key) with mutated payload rejected
    conflict = orca_api.rpc("agentSession.send", {"sessionId": sid, "text": "Mutated A", "callerKey": "caller-A"}, operation_id=shared_op)
    _assert_rpc_rejected(conflict)

    # Different callerKey with same operation_id is isolated and accepted
    orca_api.assert_rpc_ok("agentSession.send", {"sessionId": sid, "text": "Payload B", "callerKey": "caller-B"}, operation_id=shared_op)

    st = orca_api.assert_rpc_ok("agentSession.getState", {"sessionId": sid})
    seq_before, epoch, fence = int(st["lastSequence"]), int(st["epoch"]), int(st["highestFence"])
    stale_fence = orca_api.rpc("agentSession.send", {"sessionId": sid, "text": "Regressed", "ownerFence": fence - 1, "expectedEpoch": epoch - 1})
    _assert_rpc_rejected(stale_fence)
    st_after = orca_api.assert_rpc_ok("agentSession.getState", {"sessionId": sid})
    assert int(st_after["lastSequence"]) == seq_before


@pytest.mark.tier2
@pytest.mark.f12
def test_t2_f12_submission_single_current_invariant_and_item_tombstone_replay(
    orca_api: OrcaApiClient, temp_workspace: TempWorkspace
) -> None:
    sid = _req_str(orca_api.assert_rpc_ok("agentSession.create", {"agent": "claude", "workspacePath": str(temp_workspace.path), "viewMode": "chat"}), "sessionId", "id")
    first = orca_api.assert_rpc_ok("agentSession.send", {"sessionId": sid, "text": "Active submission", "simulateWorking": True})
    item_id = _req_str(first, "itemId", "id")
    qrev = int(orca_api.assert_rpc_ok("agentSession.getState", {"sessionId": sid})["queueRevision"])

    stale_q = orca_api.rpc("agentSession.send", {"sessionId": sid, "text": "Competing write", "expectedQueueRevision": qrev - 1})
    _assert_rpc_rejected(stale_q)
    subs = _list_field(orca_api.assert_rpc_ok("agentSession.getState", {"sessionId": sid}), "submissions", "queue")
    assert sum(1 for s in subs if isinstance(s, dict) and s.get("current") is True) <= 1

    # Positive control: upsertItem succeeds for a fresh non-tombstoned item
    fresh_id = _uid("live-item")
    orca_api.assert_rpc_ok("agentSession.upsertItem", {"sessionId": sid, "itemId": fresh_id, "role": "user", "text": "live-item"})
    tx_live = _list_field(orca_api.assert_rpc_ok("agentSession.getTranscript", {"sessionId": sid}), "items", "transcript")
    assert any((i.get("itemId") or i.get("id")) == fresh_id for i in tx_live if isinstance(i, dict))

    # Tombstone replay must either explicitly ignore/tombstone or return structured domain RPC rejection
    orca_api.assert_rpc_ok("agentSession.deleteItem", {"sessionId": sid, "itemId": item_id})
    replay = orca_api.rpc("agentSession.upsertItem", {"sessionId": sid, "itemId": item_id, "role": "user", "text": "Resurrected"})
    replay.assert_status(200, 400, 409)
    rbody = replay.json()
    if isinstance(rbody, dict) and rbody.get("ok") is True:
        rres = rbody.get("result")
        assert isinstance(rres, dict) and (rres.get("tombstoned") is True or rres.get("ignored") is True)
    else:
        _assert_rpc_rejected(replay, statuses=(200, 400, 409))
    tx = _list_field(orca_api.assert_rpc_ok("agentSession.getTranscript", {"sessionId": sid}), "items", "transcript")
    ids_after = {(i.get("itemId") or i.get("id")) for i in tx if isinstance(i, dict)}
    assert item_id not in ids_after and fresh_id in ids_after


@pytest.mark.tier2
@pytest.mark.f12
def test_t2_f12_ask_user_question_invalid_option_and_closed_double_answer(
    orca_api: OrcaApiClient, temp_workspace: TempWorkspace
) -> None:
    sid = _req_str(
        orca_api.assert_rpc_ok("agentSession.create", {"agent": "claude", "workspacePath": str(temp_workspace.path), "viewMode": "chat"}),
        "sessionId",
        "id",
    )
    q = orca_api.assert_rpc_ok(
        "agentSession.emitInteractivePrompt",
        {"sessionId": sid, "kind": "ask_user_question", "questions": [{"prompt": "Confirm?", "options": ["yes", "no"]}]},
    )
    q_id = _req_str(q, "questionId", "id")
    orca_api.assert_rpc_error(
        "agentSession.answerQuestion",
        "invalid_argument",
        {"sessionId": sid, "questionId": q_id, "answers": {"0": ["nonexistent-choice"]}},
    )
    answered = orca_api.assert_rpc_ok(
        "agentSession.answerQuestion",
        {"sessionId": sid, "questionId": q_id, "answers": {"0": ["yes"]}},
        operation_id=_uid("op-q1"),
    )
    assert answered.get("status") == "answered"
    orca_api.assert_rpc_error(
        "agentSession.answerQuestion",
        "question_closed",
        {"sessionId": sid, "questionId": q_id, "answers": {"0": ["no"]}},
        operation_id=_uid("op-q2"),
    )
    tx = _list_field(orca_api.assert_rpc_ok("agentSession.getTranscript", {"sessionId": sid}), "items", "transcript")
    assert any("yes" in str(i) and i.get("status") == "answered" for i in tx if isinstance(i, dict))


@pytest.mark.tier2
@pytest.mark.f12
def test_t2_f12_unknown_agent_rejected_retired_migrated_and_missing_cli_preflight(
    orca_api: OrcaApiClient, orca_cli: OrcaCliRunner, temp_workspace: TempWorkspace
) -> None:
    orca_api.assert_rpc_error(
        "agentSession.create",
        "invalid_argument",
        {"agent": "unknown-agent-xyz", "workspacePath": str(temp_workspace.path)},
    )
    orca_cli.run("bot", "create", "--name", _uid("BadBot"), "--agent", "unknown-agent-xyz").assert_error("invalid_argument")

    migrated = orca_api.assert_rpc_ok("agent.catalog.resolve", {"agent": "claude-code"})
    resolved = _req_str(migrated, "agent", "id")
    assert resolved in BOT_AGENTS

    pre = orca_api.assert_rpc_ok(
        "preflight.check", {"agent": "claude", "cmdOverride": str(temp_workspace.resolve("missing-cli-bin"))}
    )
    assert pre.get("available") is False
    assert _req_str(pre, "diagnostic", "message", "error")


@pytest.mark.tier2
@pytest.mark.f12
def test_t2_f12_capability_terminal_fallback_and_protocol_version_floor(
    orca_api: OrcaApiClient,
) -> None:
    fb = orca_api.assert_rpc_ok("nativeChat.resolveMode", {"agent": "claude", "hostCapabilities": []})
    assert fb.get("viewMode") == "terminal"

    too_old = orca_api.rpc("agent.catalog.list", protocol_version=MIN_COMPATIBLE_PROTOCOL_VERSION - 1)
    _assert_rpc_rejected(too_old, statuses=(200, 400, 422, 426))

    v_min = orca_api.assert_rpc_ok("agent.catalog.list", protocol_version=MIN_COMPATIBLE_PROTOCOL_VERSION)
    v_cur = orca_api.assert_rpc_ok("agent.catalog.list", protocol_version=RUNTIME_PROTOCOL_VERSION)
    assert len(_list_field(v_min, "agents", "items")) >= 22
    assert len(_list_field(v_cur, "agents", "items")) >= 22


# ---------------------------------------------------------------------------
# F13 Tier 2 — Session Boundary Suppression, 8-Invariant Hibernation, ReDoS & Restart
# ---------------------------------------------------------------------------


def _f13_parse_endpoint(ep: dict[str, Any]) -> tuple[RawHttpClient, str, str]:
    url, token = _req_str(ep, "url"), _req_str(ep, "token")
    parsed = urlparse(url)
    assert parsed.hostname in ("127.0.0.1", "localhost") and parsed.port
    return RawHttpClient(parsed.hostname, parsed.port), (parsed.path or "/hook"), token


@pytest.mark.tier2
@pytest.mark.f13
@pytest.mark.edge_case("SM1-09")
def test_f13_t2_session_boundary_suppresses_notifications_triggers_badges_and_clock(
    orca_api: OrcaApiClient,
) -> None:
    sid = _uid("f13-sm109")
    auto = orca_api.assert_ok(
        orca_api.post(
            "/api/v1/orca/automations",
            json_body={
                "name": _uid("sm109-trigger"),
                "trigger": {"source": "agent_status", "kind": "turn_completed", "sessionId": sid},
                "action": {"type": "log", "message": "turn_done"},
            },
        )
    )
    auto_id = _req_str(auto, "id", "automationId")
    try:
        for reason in ("connect", "resume", "/clear"):
            orca_api.assert_rpc_ok("agentStatus.ingestHook", {"sessionId": sid, "state": "done", "sessionBoundary": True, "boundaryReason": reason})
            st = orca_api.assert_rpc_ok("agentStatus.get", {"sessionId": sid})
            assert st.get("state") == "done" and st.get("sessionBoundary") is True
            assert int(st["notificationsEmitted"]) == 0 and int(st["automationTriggersFired"]) == 0
            assert st.get("badgeUnread") is False and st.get("turnCompletedAt") is None
            assert _list_field(orca_api.assert_rpc_ok("agentStatus.getNotifications", {"sessionId": sid}), "notifications", "items") == []

        # Non-boundary control: real working -> done MUST fire notification, automation trigger, badge, and clock
        orca_api.assert_rpc_ok("agentStatus.ingestHook", {"sessionId": sid, "state": "working"})
        orca_api.assert_rpc_ok("agentStatus.ingestHook", {"sessionId": sid, "state": "done", "sessionBoundary": False})
        after = orca_api.assert_rpc_ok("agentStatus.get", {"sessionId": sid})
        assert after.get("turnCompletedAt") is not None and after.get("badgeUnread") is True
        assert int(after["notificationsEmitted"]) >= 1 and int(after["automationTriggersFired"]) >= 1
        assert len(_list_field(orca_api.assert_rpc_ok("agentStatus.getNotifications", {"sessionId": sid}), "notifications", "items")) >= 1
    finally:
        orca_api.delete(f"/api/v1/orca/automations/{auto_id}")


@pytest.mark.tier2
@pytest.mark.f13
@pytest.mark.edge_case("SM2-23")
def test_f13_t2_hibernation_blocked_on_each_of_8_invariants_rest_cli_rpc(
    orca_api: OrcaApiClient, orca_cli: OrcaCliRunner
) -> None:
    assert "hibernation_blocked" in IDE_ERROR_CODES
    run_id = _req_str(orca_api.assert_ok(orca_api.post("/api/v1/orca/orchestration/runs", json_body={"title": _uid("sm223")})), "id", "runId")
    task_id = _req_str(orca_api.assert_ok(orca_api.post("/api/v1/orca/orchestration/tasks", json_body={"runId": run_id, "title": "Unsettled"})), "id", "taskId")
    disp_id = _req_str(orca_api.assert_ok(orca_api.post("/api/v1/orca/orchestration/workers/start", json_body={"runId": run_id, "taskId": task_id, "agent": "claude"})), "id", "dispatchId")

    valid_base: dict[str, Any] = {
        "sessionId": _uid("f13-sm223"), "state": "done", "tabVisible": False, "keystrokesSinceDone": 0,
        "agent": "claude", "idleMs": 1_800_000, "windowMs": 1_800_000,
        "mobileDriverActive": False, "unsettledDispatchCount": 0, "liveSubagentCount": 0,
    }
    counterexamples: list[dict[str, Any]] = [
        {"state": "working"}, {"state": "blocked"}, {"state": "waiting"},
        {"tabVisible": True}, {"keystrokesSinceDone": 1}, {"agent": "raw-shell"},
        {"idleMs": 1_799_999}, {"mobileDriverActive": True},
        {"dispatchId": disp_id, "unsettledDispatchCount": 0}, {"liveSubagentCount": 1},
    ]
    for fault in counterexamples:
        body = {**valid_base, **fault}
        rest = orca_api.post("/api/v1/orca/ide/hibernation/check", json_body=body)
        rest.assert_error_code("hibernation_blocked", 200, 400, 409, 422)
        rj = rest.json()
        assert rj.get("eligible") is False
        assert isinstance(rj.get("failedChecks"), list) and len(rj["failedChecks"]) >= 1

        orca_api.assert_rpc_error("hibernation.sleep", "hibernation_blocked", body)

        cli_args = [
            "ide", "hibernation-check",
            "--state", str(body["state"]),
            "--agent", str(body["agent"]),
            "--idle-ms", str(body["idleMs"]),
            "--window-ms", str(body["windowMs"]),
            "--tab-visible", str(body["tabVisible"]).lower(),
            "--keystrokes-since-done", str(body["keystrokesSinceDone"]),
            "--mobile-driver", str(body["mobileDriverActive"]).lower(),
            "--unsettled-dispatches", str(body["unsettledDispatchCount"]),
            "--live-subagents", str(body["liveSubagentCount"]),
        ]
        if "dispatchId" in body:
            cli_args.extend(["--dispatch-id", str(body["dispatchId"])])
        orca_cli.run(*cli_args).assert_error("hibernation_blocked")


@pytest.mark.tier2
@pytest.mark.f13
def test_f13_t2_screen_anchor_redos_adversarial_input_linear_time(orca_api: OrcaApiClient) -> None:
    adversarial = [
        ("a" * 50_000) + "!",
        ("\x1b[31m" * 10_000) + "Thinking...",
        ("(" * 20_000) + "waiting?",
    ]
    for screen in adversarial:
        t0 = time.monotonic()
        res = orca_api.assert_rpc_ok(
            "agentStatus.evaluateScreen",
            {"sessionId": _uid("f13-redos"), "agent": "claude", "screen": screen},
        )
        assert (time.monotonic() - t0) < 2.0
        assert res.get("state") in ("working", "blocked", "waiting", "done", "unknown")

    orca_api.assert_rpc_error(
        "agentStatus.validateRule",
        "invalid_argument",
        {"pattern": r"(a+)+(?=\1)bad", "state": "working"},
    )


@pytest.mark.tier2
@pytest.mark.f13
def test_f13_t2_endpoint_env_survives_server_restart_for_long_lived_pty(
    fresh_orca_server: OrcaServerHandle,
) -> None:
    sess = fresh_orca_server.register_user()
    api1 = fresh_orca_server.api_client(sess)
    ep1 = api1.assert_rpc_ok("agentStatus.getHookEndpoint")
    env_path = Path(_req_str(ep1, "endpointEnvPath"))
    assert env_path.is_file()
    sid = _uid("f13-restart")
    api1.assert_rpc_ok("agentStatus.ingestHook", {"sessionId": sid, "state": "working"})

    fresh_orca_server.restart()
    api2 = fresh_orca_server.api_client(sess)
    ep2 = api2.assert_rpc_ok("agentStatus.getHookEndpoint")
    assert Path(_req_str(ep2, "endpointEnvPath")) == env_path
    assert _req_str(ep2, "url") in env_path.read_text(encoding="utf-8")

    hydrated = api2.assert_rpc_ok("agentStatus.get", {"sessionId": sid})
    assert hydrated.get("restoredUnconfirmed") is True

    raw2, hook_path2, tok2 = _f13_parse_endpoint(ep2)
    raw2.request(
        "POST",
        hook_path2,
        headers={"Authorization": f"Bearer {tok2}"},
        json_body={"sessionId": sid, "state": "done", "sessionBoundary": False},
    ).assert_status(200, 201, 202, 204)
    recovered = api2.assert_rpc_ok("agentStatus.get", {"sessionId": sid})
    assert recovered.get("restoredUnconfirmed") is False and recovered.get("state") == "done"


@pytest.mark.tier2
@pytest.mark.f13
def test_f13_t2_hibernation_idle_window_bounds_and_invalid_config_rejected(
    orca_api: OrcaApiClient,
) -> None:
    for valid_ms in (60_000, 86_400_000):
        cfg = orca_api.assert_rpc_ok("hibernation.configure", {"idleWindowMs": valid_ms})
        assert int(cfg["idleWindowMs"]) == valid_ms
    for invalid_ms in (59_999, 0, -1, 86_400_001):
        orca_api.assert_rpc_error("hibernation.configure", "invalid_argument", {"idleWindowMs": invalid_ms})


@pytest.mark.tier2
@pytest.mark.f13
def test_f13_t2_hook_auth_token_rejection_and_guarded_interrupt_fallback(
    orca_api: OrcaApiClient,
) -> None:
    ep = orca_api.assert_rpc_ok("agentStatus.getHookEndpoint")
    raw, hook_path, _ = _f13_parse_endpoint(ep)
    sid = _uid("f13-auth")
    orca_api.assert_rpc_ok("agentStatus.ingestHook", {"sessionId": sid, "state": "working", "nativeCancelHook": False})

    raw.request("POST", hook_path, json_body={"sessionId": sid, "state": "done"}).assert_status(401, 403)
    raw.request(
        "POST",
        hook_path,
        headers={"Authorization": "Bearer forged-invalid-token"},
        json_body={"sessionId": sid, "state": "done"},
    ).assert_status(401, 403)
    assert orca_api.assert_rpc_ok("agentStatus.get", {"sessionId": sid}).get("state") == "working"

    orca_api.assert_rpc_error("agentStatus.ingestHook", "invalid_argument", {"sessionId": sid, "state": "exploding"})
    interrupted = orca_api.assert_rpc_ok("agentStatus.interrupt", {"sessionId": sid})
    assert interrupted.get("fallbackUsed") is True
    assert orca_api.assert_rpc_ok("agentStatus.get", {"sessionId": sid}).get("state") != "working"
