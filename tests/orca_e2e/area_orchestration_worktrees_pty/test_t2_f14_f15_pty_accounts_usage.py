"""Tier 2 opaque-box E2E tests for Area E3: F14 (Detached PTY & Multiplexing) and F15 (Multi-Account & Usage)."""

from __future__ import annotations

import base64
import threading
import time
import uuid
from typing import Any

import pytest

from harness import (
    CLOSED_TAB_TOMBSTONE_DAYS,
    IDE_ERROR_CODES,
    USAGE_PROVIDERS_9,
    OrcaApiClient,
    OrcaCliRunner,
    OrcaServerHandle,
    TempWorkspace,
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


def _list_field(obj: Any, *keys: str) -> list[Any]:
    if isinstance(obj, list):
        return obj
    assert isinstance(obj, dict), f"Expected list/dict payload, got {obj!r}"
    for key in keys:
        val = obj.get(key)
        if isinstance(val, list):
            return val
    raise AssertionError(f"Missing list key in {keys!r}: {obj!r}")


_UNIMPL_CODES = ("not_implemented", "unknown_method", "method_not_found")


def _wait_pty_marker(
    api: OrcaApiClient, sid: str, marker: str, cursor: int = 0, timeout: float = 5.0
) -> tuple[str, int]:
    deadline, gate, chunks, cur = time.monotonic() + timeout, threading.Event(), [], cursor
    while True:
        delta = api.assert_rpc_ok("terminal.takePendingOutput", {"sessionId": sid, "cursor": cur})
        assert isinstance(delta, dict), f"Expected dict delta, got {delta!r}"
        chunks.append(str(delta.get("data", "")))
        if "nextCursor" in delta:
            cur = int(delta["nextCursor"])
        combined = "".join(chunks)
        if marker in combined:
            return combined, cur
        if time.monotonic() >= deadline:
            raise AssertionError(f"Timed out waiting for PTY marker {marker!r}; got {combined[-300:]!r}")
        gate.wait(0.05)


def _wait_pty_snapshot(
    api: OrcaApiClient, sid: str, marker: str, timeout: float = 5.0
) -> dict[str, Any]:
    deadline, gate = time.monotonic() + timeout, threading.Event()
    while True:
        snap = api.assert_rpc_ok("terminal.getSnapshot", {"sessionId": sid})
        assert isinstance(snap, dict), f"Expected dict snapshot, got {snap!r}"
        text = str(snap.get("snapshot", snap.get("text", "")))
        if marker in text:
            return snap
        if time.monotonic() >= deadline:
            raise AssertionError(f"Timed out waiting for PTY snapshot {marker!r}; got {text[-300:]!r}")
        gate.wait(0.05)


# ---------------------------------------------------------------------------
# F14 Tier 2 — Closed-Tab Tombstones, Callback Isolation, Multiplex Flood & Fencing
# ---------------------------------------------------------------------------


@pytest.mark.tier2
@pytest.mark.f14
@pytest.mark.edge_case("SM1-06")
def test_f14_t2_closed_tab_tombstone_refuses_stale_terminal_create_replay(
    fresh_orca_server: OrcaServerHandle,
) -> None:
    assert "tab_not_found" in IDE_ERROR_CODES
    sess = fresh_orca_server.register_user()
    api1 = fresh_orca_server.api_client(sess)
    tab_id = _uid("tab-sm106-closed")
    created = api1.assert_rpc_ok("terminal.create", {"tabId": tab_id, "cols": 80, "rows": 24})
    assert _req_str(created, "sessionId", "id")

    closed = api1.assert_rpc_ok("terminal.closeTab", {"tabId": tab_id})
    assert int(closed["tombstoneTtlDays"]) == CLOSED_TAB_TOMBSTONE_DAYS

    fresh_orca_server.restart()
    api2 = fresh_orca_server.api_client(sess)
    api2.assert_rpc_error("terminal.create", "tab_not_found", {"tabId": tab_id, "cols": 80, "rows": 24})
    listed = _list_field(api2.assert_rpc_ok("terminal.list"), "sessions", "terminals", "items")
    assert all(t.get("tabId") != tab_id for t in listed if isinstance(t, dict))


@pytest.mark.tier2
@pytest.mark.f14
@pytest.mark.edge_case("SM1-07")
def test_f14_t2_native_pty_callback_exception_isolated_per_session(
    orca_api: OrcaApiClient,
) -> None:
    created: list[str] = []
    try:
        a = orca_api.assert_rpc_ok("terminal.create", {"tabId": _uid("tab-sm107-a"), "cols": 80, "rows": 24})
        sid_a = _req_str(a, "sessionId", "id")
        created.append(sid_a)
        b = orca_api.assert_rpc_ok("terminal.create", {"tabId": _uid("tab-sm107-b"), "cols": 80, "rows": 24})
        sid_b, inc_b = _req_str(b, "sessionId", "id"), _req_str(b, "incarnationId")
        created.append(sid_b)

        fault = orca_api.assert_rpc_ok("terminal.injectFault", {"sessionId": sid_a, "fault": "native_pty_exception"})
        assert fault.get("isolated") is True
        orca_api.get("/health", include_auth=False).assert_status(200)

        st_a = orca_api.assert_rpc_ok("terminal.get", {"sessionId": sid_a})
        assert st_a.get("state") in ("error", "faulted", "closed")

        orca_api.assert_rpc_ok("terminal.write", {"sessionId": sid_b, "data": "printf 'SIBLING_ALIVE_%s\\n' 'SM107'\n"})
        out_b, _ = _wait_pty_marker(orca_api, sid_b, "SIBLING_ALIVE_SM107", cursor=0)
        snap_b = _wait_pty_snapshot(orca_api, sid_b, "SIBLING_ALIVE_SM107")
        assert "SIBLING_ALIVE_SM107" in out_b and "SIBLING_ALIVE_SM107" in str(snap_b.get("snapshot", snap_b.get("text", "")))
        assert orca_api.assert_rpc_ok("terminal.get", {"sessionId": sid_b}).get("incarnationId") == inc_b
    finally:
        for sid in created:
            orca_api.rpc("terminal.kill", {"sessionId": sid})


@pytest.mark.tier2
@pytest.mark.f14
@pytest.mark.edge_case("SM1-08")
def test_f14_t2_multiplex_flood_round_robin_drain_and_ack_pending_overflow_recovery(
    orca_api: OrcaApiClient,
) -> None:
    all_sids: list[str] = []
    try:
        noisy: list[str] = []
        for i in range(3):
            nsid = _req_str(orca_api.assert_rpc_ok("terminal.create", {"tabId": _uid(f"tab-flood-{i}"), "cols": 80, "rows": 24}), "sessionId", "id")
            noisy.append(nsid)
            all_sids.append(nsid)
        quiet = _req_str(orca_api.assert_rpc_ok("terminal.create", {"tabId": _uid("tab-quiet"), "cols": 80, "rows": 24}), "sessionId", "id")
        all_sids.append(quiet)

        stream = orca_api.assert_rpc_ok("terminal.multiplex.open", {"sessionIds": all_sids, "windowBytes": 2048, "maxPendingBytes": 8192})
        stream_id = _req_str(stream, "streamId", "id")
        burst = ("N" * 1024 + "\n") * 24
        for nsid in noisy:
            orca_api.assert_rpc_ok("terminal.write", {"sessionId": nsid, "data": burst, "rawOutput": True})
        orca_api.assert_rpc_ok("terminal.write", {"sessionId": quiet, "data": "INTERACTIVE_FAIR_SM108\n", "rawOutput": True})

        first_poll = orca_api.assert_rpc_ok("terminal.multiplex.poll", {"streamId": stream_id})
        assert first_poll.get("paused") is True or int(first_poll.get("availableCredits", 1)) == 0

        saw_quiet = "INTERACTIVE_FAIR_SM108" in str(first_poll)
        saw_overflow = "ack-pending-overflow" in str(first_poll)
        quiet_poll, total_acked = first_poll, 0
        for _ in range(2):
            if saw_quiet:
                break
            orca_api.assert_rpc_ok("terminal.ackOutput", {"streamId": stream_id, "ackedBytes": 2048})
            total_acked += 2048
            polled = orca_api.assert_rpc_ok("terminal.multiplex.poll", {"streamId": stream_id})
            text = str(polled)
            if "INTERACTIVE_FAIR_SM108" in text:
                saw_quiet, quiet_poll = True, polled
            if "ack-pending-overflow" in text:
                saw_overflow = True
        assert saw_quiet and total_acked <= 4096, f"Quiet PTY starved past 2 round-robin ACK rounds (acked={total_acked})"
        assert (
            quiet_poll.get("paused") is True
            or int(quiet_poll.get("pendingBytes", quiet_poll.get("backlogBytes", 0))) > 0
            or int(quiet_poll.get("availableCredits", 1)) == 0
        ), f"Expected noisy backlog remaining after quiet delivery: {quiet_poll!r}"

        for _ in range(10):
            if saw_overflow:
                break
            orca_api.assert_rpc_ok("terminal.ackOutput", {"streamId": stream_id, "ackedBytes": 2048})
            polled = orca_api.assert_rpc_ok("terminal.multiplex.poll", {"streamId": stream_id})
            if "ack-pending-overflow" in str(polled):
                saw_overflow = True
        assert saw_overflow, "Missing ack-pending-overflow budgeted recovery snapshot"
    finally:
        for sid in all_sids:
            orca_api.rpc("terminal.kill", {"sessionId": sid})


@pytest.mark.tier2
@pytest.mark.f14
def test_f14_t2_kill_fencing_refuses_stale_incarnation_owned_or_attached_pty(
    orca_api: OrcaApiClient,
) -> None:
    created = orca_api.assert_rpc_ok(
        "terminal.create",
        {"tabId": _uid("tab-fence"), "cols": 80, "rows": 24, "agentOwner": "claude", "attached": True},
    )
    sid, inc = _req_str(created, "sessionId", "id"), _req_str(created, "incarnationId")
    try:
        for params in (
            {"sessionId": sid, "expectedIncarnationId": "stale-inc-999"},
            {"sessionId": sid, "expectedIncarnationId": inc, "requireNoAgentOwner": True},
            {"sessionId": sid, "expectedIncarnationId": inc, "requireUnattached": True},
        ):
            resp = orca_api.rpc("terminal.kill", params)
            resp.assert_status(200, 400, 409, 422)
            body = resp.json()
            assert isinstance(body, dict)
            if body.get("ok") is True:
                assert isinstance(body.get("result"), dict) and body["result"].get("killed") is False
            else:
                err = body.get("error")
                assert body.get("ok") is False and isinstance(err, dict)
                assert isinstance(err.get("code"), str) and err["code"] and err["code"] not in _UNIMPL_CODES

        orca_api.assert_rpc_ok("terminal.write", {"sessionId": sid, "data": "printf 'STILL_%s\\n' 'LIVE'\n"})
        snap = _wait_pty_snapshot(orca_api, sid, "STILL_LIVE")
        assert "STILL_LIVE" in str(snap.get("snapshot", snap.get("text", "")))
        assert orca_api.assert_rpc_ok("terminal.get", {"sessionId": sid}).get("incarnationId") == inc
    finally:
        orca_api.rpc("terminal.kill", {"sessionId": sid, "force": True})


@pytest.mark.tier2
@pytest.mark.f14
def test_f14_t2_osc52_blocks_clipboard_read_and_drops_oversized_payload(
    orca_api: OrcaApiClient,
) -> None:
    created: list[str] = []
    try:
        s1 = _req_str(orca_api.assert_rpc_ok("terminal.create", {"tabId": _uid("tab-o52-on"), "cols": 80, "rows": 24, "allowOsc52Clipboard": True}), "sessionId", "id")
        created.append(s1)
        s2 = _req_str(orca_api.assert_rpc_ok("terminal.create", {"tabId": _uid("tab-o52-off"), "cols": 80, "rows": 24, "allowOsc52Clipboard": False}), "sessionId", "id")
        created.append(s2)

        orca_api.assert_rpc_ok("terminal.write", {"sessionId": s1, "data": "\x1b]52;c;?\x07", "rawOutput": True})
        fx1 = _list_field(orca_api.assert_rpc_ok("terminal.takeSideEffects", {"sessionId": s1}), "effects", "facts", "items")
        assert all(e.get("direction") != "read" for e in fx1 if isinstance(e, dict))

        huge_b64 = base64.b64encode(b"X" * 200_000).decode("ascii")
        orca_api.assert_rpc_ok("terminal.write", {"sessionId": s1, "data": f"\x1b]52;c;{huge_b64}\x07AFTER_HUGE\n", "rawOutput": True})
        fx2 = _list_field(orca_api.assert_rpc_ok("terminal.takeSideEffects", {"sessionId": s1}), "effects", "facts", "items")
        assert all(e.get("direction") != "write" for e in fx2 if isinstance(e, dict))
        snap = orca_api.assert_rpc_ok("terminal.getSnapshot", {"sessionId": s1})
        assert "AFTER_HUGE" in str(snap.get("snapshot", snap.get("text", "")))

        small_b64 = base64.b64encode(b"blocked").decode("ascii")
        orca_api.assert_rpc_ok("terminal.write", {"sessionId": s2, "data": f"\x1b]52;c;{small_b64}\x07", "rawOutput": True})
        fx3 = _list_field(orca_api.assert_rpc_ok("terminal.takeSideEffects", {"sessionId": s2}), "effects", "facts", "items")
        assert all(e.get("direction") != "write" for e in fx3 if isinstance(e, dict))
    finally:
        for sid in created:
            orca_api.rpc("terminal.kill", {"sessionId": sid})


@pytest.mark.tier2
@pytest.mark.f14
def test_f14_t2_hidden_worktree_retention_budget_evicts_lru_without_losing_daemon_pty(
    orca_api: OrcaApiClient,
) -> None:
    sids: list[str] = []
    try:
        for idx in range(4):
            c = orca_api.assert_rpc_ok("terminal.create", {"tabId": _uid(f"tab-lru-{idx}"), "cols": 80, "rows": 24, "worktreeId": _uid("wt")})
            sid = _req_str(c, "sessionId", "id")
            sids.append(sid)
            orca_api.assert_rpc_ok("terminal.write", {"sessionId": sid, "data": f"LRU_MARKER_{idx}\n", "rawOutput": True})
            orca_api.assert_rpc_ok("terminal.setSessionBackground", {"sessionId": sid, "background": True, "hiddenWorktreeRetentionBudget": 2})
        states = [orca_api.assert_rpc_ok("terminal.getViewState", {"sessionId": s}) for s in sids]
        assert sum(1 for st in states if st.get("coldParked") is True) >= 2
        assert sum(1 for st in states if st.get("retained") is True) <= 2

        orca_api.assert_rpc_ok("terminal.setSessionBackground", {"sessionId": sids[0], "background": False})
        snap0 = orca_api.assert_rpc_ok("terminal.getSnapshot", {"sessionId": sids[0]})
        assert "LRU_MARKER_0" in str(snap0.get("snapshot", snap0.get("text", "")))
    finally:
        for sid in sids:
            orca_api.rpc("terminal.kill", {"sessionId": sid})


# ---------------------------------------------------------------------------
# F15 Tier 2 — Corrupted Config Fallback, Bounded Restarts, Credits & Thresholds
# ---------------------------------------------------------------------------


@pytest.mark.tier2
@pytest.mark.f15
def test_f15_t2_corrupted_codex_config_toml_falls_back_to_last_known_good(
    orca_api: OrcaApiClient, temp_workspace: TempWorkspace
) -> None:
    acct_id = _uid("codex-toml")
    orca_api.assert_rpc_ok("accounts.add", {"provider": "codex", "accountId": acct_id, "homeDir": str(temp_workspace.mkdir(acct_id))})
    cfg_path = temp_workspace.write("src_codex_config.toml", 'model = "o3"\napproval_policy = "on-request"\n')
    assert orca_api.assert_rpc_ok("accounts.syncCodexConfig", {"accountId": acct_id, "sourceConfigPath": str(cfg_path)}).get("fallbackUsed") is False

    temp_workspace.write("src_codex_config.toml", 'model = "unterminated\n[[[corrupt\n')
    bad = orca_api.assert_rpc_ok("accounts.syncCodexConfig", {"accountId": acct_id, "sourceConfigPath": str(cfg_path)})
    assert bad.get("fallbackUsed") is True and _req_str(bad, "warning", "parseWarning", "error")

    active_cfg = orca_api.assert_rpc_ok("accounts.getCodexConfig", {"accountId": acct_id})
    assert active_cfg.get("model") == "o3" and active_cfg.get("approval_policy") == "on-request"


@pytest.mark.tier2
@pytest.mark.f15
def test_f15_t2_turn_end_restart_bounded_concurrency_max_4_and_90s_reask(
    orca_api: OrcaApiClient,
) -> None:
    terms = [_uid(f"term-r{i}") for i in range(7)]
    orca_api.assert_rpc_ok("accounts.scheduleTurnEndRestarts", {"terminalIds": terms, "targetAccountId": _uid("acct")})

    q1 = orca_api.assert_rpc_ok("accounts.getRestartQueue")
    in_flight1, queued1 = _list_field(q1, "inFlight"), _list_field(q1, "queued")
    assert len(in_flight1) == 4 and len(queued1) == 3

    done_id = in_flight1[0] if isinstance(in_flight1[0], str) else _req_str(in_flight1[0], "terminalId", "id")
    orca_api.assert_rpc_ok("accounts.completeRestart", {"terminalId": done_id})
    q2 = orca_api.assert_rpc_ok("accounts.getRestartQueue")
    assert len(_list_field(q2, "inFlight")) == 4 and len(_list_field(q2, "queued")) == 2

    reask = orca_api.assert_rpc_ok("accounts.scheduleTurnEndRestarts", {"terminalIds": [done_id], "targetAccountId": _uid("acct"), "elapsedSec": 30})
    assert reask.get("reaskSuppressed") is True and int(reask["reaskCooldownSec"]) == 90


@pytest.mark.tier2
@pytest.mark.f15
def test_f15_t2_codex_reset_credit_idempotent_duplicate_attempt_no_double_spend(
    orca_api: OrcaApiClient, temp_workspace: TempWorkspace
) -> None:
    acct_id, op_dup = _uid("codex-dup"), _uid("op-dup")
    orca_api.assert_rpc_ok("accounts.add", {"provider": "codex", "accountId": acct_id, "homeDir": str(temp_workspace.mkdir(acct_id)), "resetCredits": 2, "rateLimited": True})
    r1 = orca_api.assert_rpc_ok("accounts.spendResetCredit", {"accountId": acct_id, "attemptId": "dup-att-1"}, operation_id=op_dup)
    r2 = orca_api.assert_rpc_ok("accounts.spendResetCredit", {"accountId": acct_id, "attemptId": "dup-att-1"}, operation_id=op_dup)
    assert r1.get("status") == "applied" and r2.get("idempotentReplay") is True
    assert int(orca_api.assert_rpc_ok("accounts.getResetCreditStatus", {"accountId": acct_id})["remainingCredits"]) == 1

    orca_api.assert_rpc_ok("accounts.spendResetCredit", {"accountId": acct_id, "attemptId": "att-2"}, operation_id=_uid("op-2"))
    assert int(orca_api.assert_rpc_ok("accounts.getResetCreditStatus", {"accountId": acct_id})["remainingCredits"]) == 0
    orca_api.assert_rpc_error("accounts.spendResetCredit", "invalid_argument", {"accountId": acct_id, "attemptId": "att-3"}, operation_id=_uid("op-3"))
    assert int(orca_api.assert_rpc_ok("accounts.getResetCreditStatus", {"accountId": acct_id})["remainingCredits"]) == 0


@pytest.mark.tier2
@pytest.mark.f15
def test_f15_t2_usage_80_percent_warning_chip_exact_threshold_and_tie_break(
    orca_api: OrcaApiClient, orca_cli: OrcaCliRunner
) -> None:
    samples = [("claude", 79.9, 600), ("codex", 80.0, 3600), ("grok", 80.0, 300), ("cursor", 80.1, 1200), ("minimax", 100.0, 900), ("kimi-code", 0.0, 7200)]
    for prov, pct, resets in samples:
        orca_api.assert_rpc_ok("usage.ingestSnapshot", {"provider": prov, "percentUsed": pct, "resetsInSec": resets})

    for bad_pct in (-1.0, 101.0):
        orca_api.assert_rpc_error("usage.ingestSnapshot", "invalid_argument", {"provider": "claude", "percentUsed": bad_pct})

    rest_rows = _list_field(orca_api.assert_ok(orca_api.get("/api/v1/orca/ide/usage")), "providers", "usage", "items")
    cli_rows = _list_field(orca_cli.run("ide", "usage").assert_ok(), "providers", "usage", "items")
    expected_order = ["minimax", "cursor", "grok", "codex", "claude", "kimi-code"]
    for rows in (rest_rows, cli_rows):
        filtered = [r for r in rows if isinstance(r, dict) and r.get("provider") in expected_order]
        assert [r["provider"] for r in filtered] == expected_order
        by_p = {r["provider"]: r for r in filtered}
        assert by_p["claude"].get("warningChip") is False and by_p["kimi-code"].get("warningChip") is False
        assert by_p["grok"].get("warningChip") is True and by_p["codex"].get("warningChip") is True
        assert by_p["cursor"].get("warningChip") is True and by_p["minimax"].get("warningChip") is True


@pytest.mark.tier2
@pytest.mark.f15
def test_f15_t2_usage_limit_stop_idle_or_past_grace_shows_hit_limit_resets(
    orca_api: OrcaApiClient, orca_cli: OrcaCliRunner
) -> None:
    cases = [
        ("claude", {"provider": "claude", "autoContinue": False, "resetsAt": "15:00"}),
        ("codex", {"provider": "codex", "autoContinue": True, "graceMinutes": 10, "elapsedPastResetSec": 601, "resetsAt": "16:15"}),
    ]
    for prov, body in cases:
        orca_api.assert_rpc_ok("usage.reportLimitStop", body)
        st = orca_api.assert_rpc_ok("usage.getLimitStatus", {"provider": prov})
        expected_text = f"Hit limit · resets {body['resetsAt']}"
        assert st.get("statusText") == expected_text
        assert st.get("autoResumeScheduled") is False

        rest_rows = _list_field(orca_api.assert_ok(orca_api.get("/api/v1/orca/ide/usage")), "providers", "usage", "items")
        cli_rows = _list_field(orca_cli.run("ide", "usage").assert_ok(), "providers", "usage", "items")
        for rows in (rest_rows, cli_rows):
            matched = [r for r in rows if isinstance(r, dict) and r.get("provider") == prov]
            assert len(matched) == 1
            text = matched[0].get("limitStatusText", matched[0].get("statusText"))
            assert text == expected_text and not str(text).startswith("Continues at")


@pytest.mark.tier2
@pytest.mark.f15
def test_f15_t2_unknown_provider_or_account_and_duplicate_account_rejection(
    orca_api: OrcaApiClient, orca_cli: OrcaCliRunner, temp_workspace: TempWorkspace
) -> None:
    assert "bogus-provider" not in USAGE_PROVIDERS_9
    dup_id = _uid("dup-acct")
    home = str(temp_workspace.mkdir(dup_id))
    orca_api.assert_rpc_ok("accounts.add", {"provider": "codex", "accountId": dup_id, "homeDir": home})
    orca_api.assert_rpc_error("accounts.add", "invalid_argument", {"provider": "codex", "accountId": dup_id, "homeDir": home})
    orca_api.assert_rpc_error("accounts.setActive", "not_found", {"provider": "codex", "accountId": _uid("missing-acct")})
    orca_api.assert_rpc_error("usage.ingestSnapshot", "invalid_argument", {"provider": "bogus-provider", "percentUsed": 50})
    orca_cli.run("ide", "usage", "--provider", "bogus-provider").assert_error("invalid_argument")
