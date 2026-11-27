"""Tier 1 opaque-box E2E tests for Area E3: F14 (Detached PTY & Multiplexing) and F15 (Multi-Account & Usage)."""

from __future__ import annotations

import base64
import threading
import time
import uuid
from typing import Any

import pytest

from harness import (
    USAGE_PROVIDERS_9,
    OrcaApiClient,
    OrcaCliRunner,
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
# F14 — Detached PTY Persistence, Cold-Park, Side Effects & Multiplex Flow Control
# ---------------------------------------------------------------------------


@pytest.mark.tier1
@pytest.mark.f14
def test_f14_t1_pty_create_write_resize_snapshot_and_take_pending_output(
    orca_api: OrcaApiClient,
) -> None:
    created = orca_api.assert_rpc_ok(
        "terminal.create", {"tabId": _uid("tab-pty"), "cols": 80, "rows": 24}
    )
    sid = _req_str(created, "sessionId", "id")
    inc_id = _req_str(created, "incarnationId")
    assert int(created["cols"]) == 80 and int(created["rows"]) == 24
    try:
        orca_api.assert_rpc_ok("terminal.write", {"sessionId": sid, "data": "printf 'ORCA_PTY_%s\\n' 'ALPHA'\n"})
        resized = orca_api.assert_rpc_ok("terminal.resize", {"sessionId": sid, "cols": 100, "rows": 32})
        assert int(resized["cols"]) == 100 and int(resized["rows"]) == 32

        out1, next_cursor = _wait_pty_marker(orca_api, sid, "ORCA_PTY_ALPHA", cursor=0)
        assert "ORCA_PTY_ALPHA" in out1 and next_cursor > 0

        delta2 = orca_api.assert_rpc_ok("terminal.takePendingOutput", {"sessionId": sid, "cursor": next_cursor})
        assert "ORCA_PTY_ALPHA" not in str(delta2.get("data", ""))
        assert int(delta2["nextCursor"]) >= next_cursor

        orca_api.assert_rpc_ok("terminal.write", {"sessionId": sid, "data": "printf 'ORCA_PTY_%s\\n' 'BETA'\n"})
        snap = _wait_pty_snapshot(orca_api, sid, "ORCA_PTY_BETA")
        snap_text = str(snap.get("snapshot", snap.get("text", "")))
        assert "ORCA_PTY_ALPHA" in snap_text and "ORCA_PTY_BETA" in snap_text
        assert int(snap["cols"]) == 100 and int(snap["rows"]) == 32
    finally:
        orca_api.rpc("terminal.kill", {"sessionId": sid, "expectedIncarnationId": inc_id})


@pytest.mark.tier1
@pytest.mark.f14
def test_f14_t1_daemon_transient_fact_side_effect_extraction(orca_api: OrcaApiClient) -> None:
    created = orca_api.assert_rpc_ok("terminal.create", {"tabId": _uid("tab-fx"), "cols": 80, "rows": 24})
    sid = _req_str(created, "sessionId", "id")
    pr_url = "https://github.com/stablyai/orca/pull/48844"
    try:
        stream = f"\x07\x1b]133;D;0\x07Created PR: {pr_url}\r\n\x1b[?2031h\x1b[?2031l"
        orca_api.assert_rpc_ok("terminal.write", {"sessionId": sid, "data": stream, "rawOutput": True})
        effects = _list_field(orca_api.assert_rpc_ok("terminal.takeSideEffects", {"sessionId": sid}), "effects", "facts", "items")
        kinds = {e.get("kind") or e.get("type") for e in effects if isinstance(e, dict)}
        assert {"bell", "command-finished", "pr-link", "2031-subscribe", "2031-unsubscribe"}.issubset(kinds)
        cmd_fx = [e for e in effects if (e.get("kind") or e.get("type")) == "command-finished"]
        assert any(int(e.get("exitCode", -1)) == 0 for e in cmd_fx)
        pr_fx = [e for e in effects if (e.get("kind") or e.get("type")) == "pr-link"]
        assert any(pr_url in str(e.get("url", e)) for e in pr_fx)
    finally:
        orca_api.rpc("terminal.kill", {"sessionId": sid})


@pytest.mark.tier1
@pytest.mark.f14
def test_f14_t1_hidden_view_cold_park_and_lossless_snapshot_restore(orca_api: OrcaApiClient) -> None:
    created = orca_api.assert_rpc_ok("terminal.create", {"tabId": _uid("tab-park"), "cols": 80, "rows": 24})
    sid = _req_str(created, "sessionId", "id")
    try:
        orca_api.assert_rpc_ok(
            "terminal.setSessionBackground",
            {"sessionId": sid, "background": True, "hiddenDeliveryGate": True},
        )
        orca_api.assert_rpc_ok("terminal.write", {"sessionId": sid, "data": "printf 'WHILE_PARKED_%s\\n' 'LINE_42'\n"})
        _wait_pty_snapshot(orca_api, sid, "WHILE_PARKED_LINE_42")
        vs = orca_api.assert_rpc_ok("terminal.getViewState", {"sessionId": sid})
        assert vs.get("background") is True and vs.get("deliveryGated") is True
        assert "pushedRawBytesWhileHidden" in vs and not isinstance(vs["pushedRawBytesWhileHidden"], bool) and int(vs["pushedRawBytesWhileHidden"]) == 0

        orca_api.assert_rpc_ok("terminal.setSessionBackground", {"sessionId": sid, "background": False})
        snap = _wait_pty_snapshot(orca_api, sid, "WHILE_PARKED_LINE_42")
        assert "WHILE_PARKED_LINE_42" in str(snap.get("snapshot", snap.get("text", "")))
    finally:
        orca_api.rpc("terminal.kill", {"sessionId": sid})


@pytest.mark.tier1
@pytest.mark.f14
def test_f14_t1_pty_incarnation_id_kill_fencing_happy_path(orca_api: OrcaApiClient) -> None:
    c1 = orca_api.assert_rpc_ok("terminal.create", {"tabId": _uid("tab-inc1"), "cols": 80, "rows": 24})
    sid1 = _req_str(c1, "sessionId", "id")
    inc1 = _req_str(c1, "incarnationId")
    orca_api.assert_rpc_ok("terminal.detach", {"sessionId": sid1})
    killed = orca_api.assert_rpc_ok(
        "terminal.kill",
        {"sessionId": sid1, "expectedIncarnationId": inc1, "requireNoAgentOwner": True, "requireUnattached": True},
    )
    assert killed.get("killed") is True and killed.get("incarnationId") == inc1

    c2 = orca_api.assert_rpc_ok("terminal.create", {"tabId": _uid("tab-inc2"), "cols": 80, "rows": 24})
    sid2 = _req_str(c2, "sessionId", "id")
    inc2 = _req_str(c2, "incarnationId")
    try:
        assert inc2 != inc1
    finally:
        orca_api.rpc("terminal.kill", {"sessionId": sid2})


@pytest.mark.tier1
@pytest.mark.f14
def test_f14_t1_osc52_write_clipboard_allowed_when_enabled(orca_api: OrcaApiClient) -> None:
    created = orca_api.assert_rpc_ok(
        "terminal.create",
        {"tabId": _uid("tab-osc52"), "cols": 80, "rows": 24, "allowOsc52Clipboard": True},
    )
    sid = _req_str(created, "sessionId", "id")
    try:
        encoded = base64.b64encode(b"orca-clip-ok").decode("ascii")
        orca_api.assert_rpc_ok(
            "terminal.write",
            {"sessionId": sid, "data": f"\x1b]52;c;{encoded}\x07", "rawOutput": True},
        )
        effects = _list_field(orca_api.assert_rpc_ok("terminal.takeSideEffects", {"sessionId": sid}), "effects", "facts", "items")
        clips = [e for e in effects if isinstance(e, dict) and e.get("direction") == "write" and e.get("text") == "orca-clip-ok"]
        assert len(clips) >= 1
    finally:
        orca_api.rpc("terminal.kill", {"sessionId": sid})


@pytest.mark.tier1
@pytest.mark.f14
def test_f14_t1_multiplex_credit_ack_round_robin_steady_state(orca_api: OrcaApiClient) -> None:
    created: list[str] = []
    try:
        s1 = _req_str(orca_api.assert_rpc_ok("terminal.create", {"tabId": _uid("tab-mx1"), "cols": 80, "rows": 24}), "sessionId", "id")
        created.append(s1)
        s2 = _req_str(orca_api.assert_rpc_ok("terminal.create", {"tabId": _uid("tab-mx2"), "cols": 80, "rows": 24}), "sessionId", "id")
        created.append(s2)
        stream = orca_api.assert_rpc_ok("terminal.multiplex.open", {"sessionIds": [s1, s2], "windowBytes": 16384})
        stream_id = _req_str(stream, "streamId", "id")
        orca_api.assert_rpc_ok("terminal.write", {"sessionId": s1, "data": "printf 'MX_%s\\n' 'ONE'\n"})
        orca_api.assert_rpc_ok("terminal.write", {"sessionId": s2, "data": "printf 'MX_%s\\n' 'TWO'\n"})

        deadline, gate, combined, acked_bytes = time.monotonic() + 5.0, threading.Event(), "", 0
        while not ("MX_ONE" in combined and "MX_TWO" in combined):
            polled = orca_api.assert_rpc_ok("terminal.multiplex.poll", {"streamId": stream_id})
            for c in _list_field(polled, "chunks", "frames", "items"):
                if isinstance(c, dict):
                    piece = str(c.get("data", ""))
                    combined += piece
                    acked_bytes += len(piece.encode("utf-8"))
            if "MX_ONE" in combined and "MX_TWO" in combined:
                break
            if time.monotonic() >= deadline:
                raise AssertionError(f"Timed out waiting for multiplex markers; got {combined[-300:]!r}")
            gate.wait(0.05)
        assert acked_bytes > 0

        ack = orca_api.assert_rpc_ok("terminal.ackOutput", {"streamId": stream_id, "ackedBytes": acked_bytes})
        assert int(ack["availableCredits"]) > 0
        assert ack.get("linkEstimate") is not None
        assert "overflowCount" in ack and not isinstance(ack["overflowCount"], bool) and int(ack["overflowCount"]) == 0
    finally:
        for sid in created:
            orca_api.rpc("terminal.kill", {"sessionId": sid})


# ---------------------------------------------------------------------------
# F15 — Multi-Account Hot-Swap & 9-Provider Usage / Rate-Limit Roster
# ---------------------------------------------------------------------------


@pytest.mark.tier1
@pytest.mark.f15
def test_f15_t1_multi_account_isolated_homes_and_active_selection(
    orca_api: OrcaApiClient, temp_workspace: TempWorkspace
) -> None:
    id_a, id_b = _uid("work-a"), _uid("personal-b")
    home_a = str(temp_workspace.mkdir(id_a))
    home_b = str(temp_workspace.mkdir(id_b))
    orca_api.assert_rpc_ok("accounts.add", {"provider": "codex", "accountId": id_a, "label": "Work", "homeDir": home_a})
    orca_api.assert_rpc_ok("accounts.add", {"provider": "codex", "accountId": id_b, "label": "Personal", "homeDir": home_b})

    listed = _list_field(orca_api.assert_rpc_ok("accounts.list", {"provider": "codex"}), "accounts", "items")
    by_id = {(a.get("accountId") or a.get("id")): a for a in listed if isinstance(a, dict)}
    assert id_a in by_id and id_b in by_id
    assert by_id[id_a].get("homeDir") != by_id[id_b].get("homeDir")

    sw = orca_api.assert_rpc_ok("accounts.setActive", {"provider": "codex", "accountId": id_b})
    assert sw.get("activeAccountId") == id_b
    after = _list_field(orca_api.assert_rpc_ok("accounts.list", {"provider": "codex"}), "accounts", "items")
    assert {a.get("accountId") or a.get("id") for a in after if isinstance(a, dict)} >= {id_a, id_b}

    env_a = orca_api.assert_rpc_ok("accounts.getRuntimeEnv", {"provider": "codex", "accountId": id_a})
    env_b = orca_api.assert_rpc_ok("accounts.getRuntimeEnv", {"provider": "codex", "accountId": id_b})
    assert home_a in str(env_a) and home_b in str(env_b) and env_a != env_b


@pytest.mark.tier1
@pytest.mark.f15
def test_f15_t1_live_hotswap_terminal_pinned_vs_chat_next_turn_switch(
    orca_api: OrcaApiClient, temp_workspace: TempWorkspace
) -> None:
    acct_a, acct_b = _uid("claude-a"), _uid("claude-b")
    orca_api.assert_rpc_ok("accounts.add", {"provider": "claude", "accountId": acct_a, "homeDir": str(temp_workspace.mkdir(acct_a))})
    orca_api.assert_rpc_ok("accounts.add", {"provider": "claude", "accountId": acct_b, "homeDir": str(temp_workspace.mkdir(acct_b))})
    orca_api.assert_rpc_ok("accounts.setActive", {"provider": "claude", "accountId": acct_a})

    term_id, chat_id = _uid("term-pin"), _uid("chat-swap")
    orca_api.assert_rpc_ok("accounts.launchBinding", {"provider": "claude", "surface": "terminal", "surfaceId": term_id})
    orca_api.assert_rpc_ok("accounts.launchBinding", {"provider": "claude", "surface": "chat", "surfaceId": chat_id})
    orca_api.assert_rpc_ok("accounts.setActive", {"provider": "claude", "accountId": acct_b})

    term_res = orca_api.assert_rpc_ok("accounts.resolveTurnAccount", {"provider": "claude", "surface": "terminal", "surfaceId": term_id})
    chat_res = orca_api.assert_rpc_ok("accounts.resolveTurnAccount", {"provider": "claude", "surface": "chat", "surfaceId": chat_id})
    assert term_res.get("launchAccountId") == acct_a
    assert chat_res.get("nextTurnAccountId") == acct_b


@pytest.mark.tier1
@pytest.mark.f15
def test_f15_t1_usage_roster_9_providers_sorted_tightest_first_via_rest_cli_rpc(
    orca_api: OrcaApiClient, orca_cli: OrcaCliRunner
) -> None:
    utils = [40, 92, 10, 85, 60, 20, 79, 50, 30]
    windows = ["5h", "daily", "weekly", "fable"]
    for idx, (provider, pct) in enumerate(zip(USAGE_PROVIDERS_9, utils)):
        orca_api.assert_rpc_ok(
            "usage.ingestSnapshot",
            {
                "provider": provider,
                "percentUsed": pct,
                "window": windows[idx % len(windows)],
                "resetsInSec": 1800 + idx * 60,
                "inferredCostUsd": round(1.25 * (idx + 1), 2),
            },
        )

    rest_rows = _list_field(orca_api.assert_ok(orca_api.get("/api/v1/orca/ide/usage")), "providers", "usage", "items")
    cli_rows = _list_field(orca_cli.run("ide", "usage").assert_ok(), "providers", "usage", "items")
    rpc_rows = _list_field(orca_api.assert_rpc_ok("usage.list"), "providers", "usage", "items")

    for rows in (rest_rows, cli_rows, rpc_rows):
        by_prov = {r.get("provider"): r for r in rows if isinstance(r, dict)}
        assert set(USAGE_PROVIDERS_9).issubset(by_prov.keys())
        used_seq = [float(r["percentUsed"]) for r in rows if isinstance(r, dict) and r.get("provider") in USAGE_PROVIDERS_9]
        assert used_seq == sorted(used_seq, reverse=True)
        for prov in USAGE_PROVIDERS_9:
            row = by_prov[prov]
            used = float(row["percentUsed"])
            rem = float(row["percentRemaining"])
            assert abs((used + rem) - 100.0) < 0.01
            assert row.get("warningChip") is (used >= 80.0)
            assert row.get("resetsInSec") is not None
            assert row.get("inferredCostUsd") is not None


@pytest.mark.tier1
@pytest.mark.f15
def test_f15_t1_claude_zero_network_status_line_feed_and_cost_stats(
    orca_api: OrcaApiClient,
) -> None:
    orca_api.assert_rpc_ok(
        "usage.ingestStatusLine",
        {
            "provider": "claude",
            "percentUsed": 64.5,
            "inputTokens": 42000,
            "outputTokens": 8500,
            "window": "5h",
            "resetsInSec": 5400,
            "inferredCostUsd": 2.34,
        },
    )
    rows = _list_field(orca_api.assert_ok(orca_api.get("/api/v1/orca/ide/usage")), "providers", "usage", "items")
    claude = [r for r in rows if isinstance(r, dict) and r.get("provider") == "claude"]
    assert len(claude) == 1
    row = claude[0]
    assert row.get("source") == "status_line"
    assert abs(float(row["percentUsed"]) - 64.5) < 0.01
    assert abs(float(row["percentRemaining"]) - 35.5) < 0.01
    assert int(row["resetsInSec"]) > 0
    assert float(row["inferredCostUsd"]) >= 2.34


@pytest.mark.tier1
@pytest.mark.f15
def test_f15_t1_codex_rate_limit_reset_credit_spend_happy_path(
    orca_api: OrcaApiClient, temp_workspace: TempWorkspace
) -> None:
    acct_id = _uid("codex-cred")
    orca_api.assert_rpc_ok(
        "accounts.add",
        {
            "provider": "codex",
            "accountId": acct_id,
            "homeDir": str(temp_workspace.mkdir(acct_id)),
            "resetCredits": 2,
            "rateLimited": True,
        },
    )
    spent = orca_api.assert_rpc_ok(
        "accounts.spendResetCredit",
        {"accountId": acct_id, "attemptId": _uid("att")},
        operation_id=_uid("op-credit"),
    )
    assert isinstance(spent, dict) and spent.get("status") == "applied"
    st = orca_api.assert_rpc_ok("accounts.getResetCreditStatus", {"accountId": acct_id})
    assert int(st["remainingCredits"]) == 1
    assert st.get("status") == "applied"
    assert st.get("rateLimited") is False


@pytest.mark.tier1
@pytest.mark.f15
def test_f15_t1_usage_limit_auto_continue_within_10m_grace_status(
    orca_api: OrcaApiClient, orca_cli: OrcaCliRunner
) -> None:
    orca_api.assert_rpc_ok(
        "usage.reportLimitStop",
        {"provider": "codex", "autoContinue": True, "graceMinutes": 10, "resumesAt": "14:30"},
    )
    rpc_st = orca_api.assert_rpc_ok("usage.getLimitStatus", {"provider": "codex"})
    assert rpc_st.get("statusText") == "Continues at 14:30"
    assert int(rpc_st["graceMinutes"]) == 10 and rpc_st.get("autoContinue") is True

    rest_rows = _list_field(orca_api.assert_ok(orca_api.get("/api/v1/orca/ide/usage")), "providers", "usage", "items")
    cli_rows = _list_field(orca_cli.run("ide", "usage").assert_ok(), "providers", "usage", "items")
    for rows in (rest_rows, cli_rows):
        codex = [r for r in rows if isinstance(r, dict) and r.get("provider") == "codex"]
        assert len(codex) == 1
        assert codex[0].get("limitStatusText", codex[0].get("statusText")) == "Continues at 14:30"
