"""Tier 1 opaque-box E2E tests for Area E3: F12 (CLI Agents & Journal) and F13 (Status & Hibernation)."""

from __future__ import annotations

import time
import uuid
from pathlib import Path
from typing import Any
from urllib.parse import urlparse

import pytest

from harness import (
    BOT_AGENTS,
    OrcaApiClient,
    OrcaCliRunner,
    OrcaServerHandle,
    RawHttpClient,
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


# ---------------------------------------------------------------------------
# F12 — 22+ CLI Agents, Event-Sourced Chat Journal & Chat<->Terminal Continuation
# ---------------------------------------------------------------------------


@pytest.mark.tier1
@pytest.mark.f12
def test_t1_f12_agent_catalog_22_plus_and_dynamic_model_effort_probing(
    orca_api: OrcaApiClient,
) -> None:
    catalog = _list_field(orca_api.assert_rpc_ok("agent.catalog.list"), "agents", "items")
    assert len(catalog) >= 22
    ids = {a.get("id") or a.get("agent") if isinstance(a, dict) else a for a in catalog}
    assert set(BOT_AGENTS).issubset(ids), f"Missing BOT_AGENTS: {set(BOT_AGENTS) - ids}"

    for agent in ("claude", "codex", "grok"):
        caps = orca_api.assert_rpc_ok("agent.probeCapabilities", {"agent": agent})
        assert isinstance(caps, dict)
        models = caps.get("models")
        assert isinstance(models, list) and len(models) >= 1
        efforts = caps.get("reasoningEfforts")
        assert isinstance(efforts, (list, dict)) and len(efforts) >= 1
        slash = caps.get("slashCommands", caps.get("skills"))
        assert isinstance(slash, list)


@pytest.mark.tier1
@pytest.mark.f12
def test_t1_f12_session_journal_event_sourced_state_and_idempotent_receipts(
    orca_api: OrcaApiClient, temp_workspace: TempWorkspace
) -> None:
    sess = orca_api.assert_rpc_ok(
        "agentSession.create",
        {"agent": "claude", "workspacePath": str(temp_workspace.path), "viewMode": "chat"},
    )
    sess_id = _req_str(sess, "sessionId", "id")
    op_id = _uid("op-journal")
    params = {"sessionId": sess_id, "text": "Summarize crate layout", "callerKey": "desktop-pane-1"}
    r1 = orca_api.assert_rpc_ok("agentSession.send", params, operation_id=op_id)
    r2 = orca_api.assert_rpc_ok("agentSession.send", params, operation_id=op_id)
    assert isinstance(r1, dict) and isinstance(r2, dict)

    state = orca_api.assert_rpc_ok("agentSession.getState", {"sessionId": sess_id})
    assert isinstance(state, dict)
    assert int(state["epoch"]) >= 1
    assert int(state["lastSequence"]) >= 1
    assert int(state["highestFence"]) >= 0
    assert "sequence" in r1 and not isinstance(r1["sequence"], bool) and r1["sequence"] == r2.get("sequence") and int(r1["sequence"]) >= 1

    items = _list_field(
        orca_api.assert_rpc_ok("agentSession.getTranscript", {"sessionId": sess_id}),
        "items",
        "transcript",
    )
    user_items = [i for i in items if isinstance(i, dict) and i.get("role") == "user" and "Summarize crate layout" in str(i.get("text", i.get("content", "")))]
    assert len(user_items) == 1


@pytest.mark.tier1
@pytest.mark.f12
def test_t1_f12_queued_followups_while_working_and_provider_handle_chain(
    orca_api: OrcaApiClient, temp_workspace: TempWorkspace
) -> None:
    sess = orca_api.assert_rpc_ok(
        "agentSession.create",
        {"agent": "claude", "workspacePath": str(temp_workspace.path), "viewMode": "chat"},
    )
    sess_id = _req_str(sess, "sessionId", "id")
    orca_api.assert_rpc_ok("agentSession.send", {"sessionId": sess_id, "text": "Start heavy refactor", "simulateWorking": True})
    orca_api.assert_rpc_ok("agentSession.send", {"sessionId": sess_id, "text": "Queued follow-up check", "mode": "queue"})

    st = orca_api.assert_rpc_ok("agentSession.getState", {"sessionId": sess_id})
    assert int(st["queueRevision"]) >= 1
    subs = _list_field(st, "submissions", "queue")
    assert any(isinstance(s, dict) and s.get("current") is True for s in subs)
    assert any("Queued follow-up check" in str(s) for s in subs)

    orca_api.assert_rpc_ok("agentSession.contextBoundary", {"sessionId": sess_id, "reason": "/clear"})
    after = orca_api.assert_rpc_ok("agentSession.getState", {"sessionId": sess_id})
    assert int(after.get("contextBoundary", 0)) >= 1
    chain = after.get("providerHandleChain")
    assert isinstance(chain, list) and len(chain) >= 2


@pytest.mark.tier1
@pytest.mark.f12
def test_t1_f12_bidirectional_chat_terminal_switch_and_history_fork(
    orca_api: OrcaApiClient, temp_workspace: TempWorkspace
) -> None:
    sess = orca_api.assert_rpc_ok(
        "agentSession.create",
        {"agent": "codex", "workspacePath": str(temp_workspace.path), "viewMode": "chat"},
    )
    sess_id = _req_str(sess, "sessionId", "id")
    orca_api.assert_rpc_ok("agentSession.send", {"sessionId": sess_id, "text": "Inspect RPC routes"})

    to_term = orca_api.assert_rpc_ok("nativeChat.toggleViewMode", {"sessionId": sess_id, "viewMode": "terminal"})
    assert _req_str(to_term, "sessionId", "id") == sess_id and to_term.get("viewMode") == "terminal"
    to_chat = orca_api.assert_rpc_ok("nativeChat.toggleViewMode", {"sessionId": sess_id, "viewMode": "chat"})
    assert _req_str(to_chat, "sessionId", "id") == sess_id and to_chat.get("viewMode") == "chat"

    fork_term = orca_api.assert_rpc_ok("agentSession.continueInTerminal", {"sessionId": sess_id})
    term_id = _req_str(fork_term, "sessionId", "id")
    assert term_id != sess_id and isinstance(fork_term.get("providerHandleChain"), list) and len(fork_term["providerHandleChain"]) >= 1

    orca_api.assert_rpc_ok("agentSession.stopTerminalOwner", {"sessionId": term_id})
    fork_chat = orca_api.assert_rpc_ok("agentSession.continueInChat", {"sessionId": term_id})
    chat2_id = _req_str(fork_chat, "sessionId", "id")
    assert chat2_id not in (sess_id, term_id)
    assert isinstance(fork_chat.get("providerHandleChain"), list) and len(fork_chat["providerHandleChain"]) >= 2
    items = _list_field(orca_api.assert_rpc_ok("agentSession.getTranscript", {"sessionId": chat2_id}), "items", "transcript")
    assert any("Inspect RPC routes" in str(i) for i in items)


@pytest.mark.tier1
@pytest.mark.f12
def test_t1_f12_ask_user_question_interactive_card_roundtrip(
    orca_api: OrcaApiClient, temp_workspace: TempWorkspace
) -> None:
    sess_id = _req_str(
        orca_api.assert_rpc_ok("agentSession.create", {"agent": "claude", "workspacePath": str(temp_workspace.path), "viewMode": "chat"}),
        "sessionId",
        "id",
    )
    emitted = orca_api.assert_rpc_ok(
        "agentSession.emitInteractivePrompt",
        {
            "sessionId": sess_id,
            "kind": "ask_user_question",
            "questions": [{"prompt": "Deploy canary?", "options": ["approve", "hold"], "recommended": 0}],
        },
    )
    q_id = _req_str(emitted, "questionId", "id")
    before = _list_field(orca_api.assert_rpc_ok("agentSession.getTranscript", {"sessionId": sess_id}), "items", "transcript")
    assert any(isinstance(i, dict) and i.get("kind") == "ask_user_question" and i.get("status") == "pending" for i in before)

    answered = orca_api.assert_rpc_ok(
        "agentSession.answerQuestion",
        {"sessionId": sess_id, "questionId": q_id, "answers": {"0": ["approve"]}},
        operation_id=_uid("op-ans"),
    )
    assert isinstance(answered, dict) and answered.get("status") == "answered"
    after = _list_field(orca_api.assert_rpc_ok("agentSession.getTranscript", {"sessionId": sess_id}), "items", "transcript")
    assert any(isinstance(i, dict) and i.get("status") == "answered" and "approve" in str(i) for i in after)


@pytest.mark.tier1
@pytest.mark.f12
def test_t1_f12_reconnect_retains_settled_transcript_and_unsent_drafts(
    fresh_orca_server: OrcaServerHandle, temp_workspace: TempWorkspace
) -> None:
    sess = fresh_orca_server.register_user()
    api = fresh_orca_server.api_client(sess)
    sid = _req_str(
        api.assert_rpc_ok("agentSession.create", {"agent": "claude", "workspacePath": str(temp_workspace.path), "viewMode": "chat"}),
        "sessionId",
        "id",
    )
    api.assert_rpc_ok("agentSession.send", {"sessionId": sid, "text": "Turn one settled"})
    api.assert_rpc_ok("agentSession.send", {"sessionId": sid, "text": "Turn two settled"})
    api.assert_rpc_ok(
        "agentSession.saveDraft",
        {"sessionId": sid, "text": "Unsent follow-up draft", "attachments": [{"path": "notes.md"}]},
    )
    pre_state = api.assert_rpc_ok("agentSession.getState", {"sessionId": sid})

    fresh_orca_server.restart()
    api2 = fresh_orca_server.api_client(sess)
    post_state = api2.assert_rpc_ok("agentSession.getState", {"sessionId": sid})
    post_tx = _list_field(api2.assert_rpc_ok("agentSession.getTranscript", {"sessionId": sid}), "items", "transcript")
    post_draft = api2.assert_rpc_ok("agentSession.getDraft", {"sessionId": sid})

    assert post_state["epoch"] == pre_state["epoch"]
    assert post_state["lastSequence"] == pre_state["lastSequence"]
    assert post_state.get("providerHandleChain") == pre_state.get("providerHandleChain")
    assert any("Turn one settled" in str(i) for i in post_tx)
    assert any("Turn two settled" in str(i) for i in post_tx)
    assert post_draft.get("text") == "Unsent follow-up draft"
    assert post_draft.get("attachments") == [{"path": "notes.md"}]


# ---------------------------------------------------------------------------
# F13 — 4-State Agent Status Engine, Hook Server, Screen-Anchor Rules & Hibernation
# ---------------------------------------------------------------------------


def _f13_hook_client(endpoint: dict[str, Any]) -> tuple[RawHttpClient, str, str]:
    url = _req_str(endpoint, "url")
    token = _req_str(endpoint, "token")
    parsed = urlparse(url)
    assert parsed.hostname in ("127.0.0.1", "localhost") and parsed.port
    path = parsed.path or "/hook"
    return RawHttpClient(parsed.hostname, parsed.port), path, token


@pytest.mark.tier1
@pytest.mark.f13
def test_f13_t1_four_state_hook_server_lifecycle_and_endpoint_env(
    orca_api: OrcaApiClient,
) -> None:
    ep = orca_api.assert_rpc_ok("agentStatus.getHookEndpoint")
    raw, hook_path, token = _f13_hook_client(ep)
    env_path = Path(_req_str(ep, "endpointEnvPath"))
    assert env_path.is_file()
    env_text = env_path.read_text(encoding="utf-8")
    assert ep["url"] in env_text and token in env_text

    sid = _uid("f13-hook")
    hdrs = {"Authorization": f"Bearer {token}"}
    prompt_payload = {"questionId": "q1", "prompt": "Proceed?", "options": ["yes", "no"]}
    transitions = [
        ("working", {}),
        ("blocked", {"reason": "permission_prompt"}),
        ("waiting", {"interactivePrompt": prompt_payload}),
        ("done", {"sessionBoundary": False}),
    ]
    for state, extra in transitions:
        body = {"sessionId": sid, "state": state, **extra}
        raw.request("POST", hook_path, headers=hdrs, json_body=body).assert_status(200, 201, 202, 204)
        cur = orca_api.assert_rpc_ok("agentStatus.get", {"sessionId": sid})
        assert cur.get("state") == state
        if state == "waiting":
            assert cur.get("interactivePrompt") == prompt_payload
        if state == "done":
            assert cur.get("turnCompletedAt") is not None

    off = orca_api.assert_rpc_ok("agentStatus.toggleHooks", {"enabled": False})
    assert off.get("enabled") is False
    on = orca_api.assert_rpc_ok("agentStatus.toggleHooks", {"enabled": True})
    assert on.get("enabled") is True


@pytest.mark.tier1
@pytest.mark.f13
def test_f13_t1_hook_over_screen_anchor_precedence(orca_api: OrcaApiClient) -> None:
    sid = _uid("f13-prec")
    s1 = orca_api.assert_rpc_ok(
        "agentStatus.evaluateScreen",
        {"sessionId": sid, "agent": "claude", "screen": "⠋ Thinking... (esc to interrupt)\n"},
    )
    assert s1.get("state") == "working" and s1.get("source") == "screen"

    orca_api.assert_rpc_ok(
        "agentStatus.ingestHook",
        {"sessionId": sid, "state": "blocked", "reason": "approval_needed"},
    )
    g1 = orca_api.assert_rpc_ok("agentStatus.get", {"sessionId": sid})
    assert g1.get("state") == "blocked" and g1.get("source") == "hook"

    orca_api.assert_rpc_ok(
        "agentStatus.evaluateScreen",
        {"sessionId": sid, "agent": "claude", "screen": "⠙ Reading files...\n"},
    )
    g2 = orca_api.assert_rpc_ok("agentStatus.get", {"sessionId": sid})
    assert g2.get("state") == "blocked" and g2.get("source") == "hook"

    orca_api.assert_rpc_ok("agentStatus.ingestHook", {"sessionId": sid, "clearOverride": True})
    s2 = orca_api.assert_rpc_ok(
        "agentStatus.evaluateScreen",
        {"sessionId": sid, "agent": "claude", "screen": "⠙ Reading files...\n"},
    )
    assert s2.get("state") == "working" and s2.get("source") == "screen"


@pytest.mark.tier1
@pytest.mark.f13
def test_f13_t1_screen_anchor_json_rules_classification(orca_api: OrcaApiClient) -> None:
    packs = _list_field(orca_api.assert_rpc_ok("agentStatus.listRulePacks"), "packs", "items")
    assert len(packs) >= 13
    pack_ids = {p.get("id") or p.get("agent") if isinstance(p, dict) else p for p in packs}
    assert {"claude", "codex", "opencode", "cursor"}.issubset(pack_ids)

    samples = [
        ("working", "⠋ Thinking... (esc to interrupt)\n"),
        ("blocked", "Allow bash command? [y/N]\n"),
        ("waiting", "AskUserQuestion: Choose an option:\n> 1. Proceed\n"),
        ("done", "Task complete.\n> \n"),
    ]
    for expected, screen in samples:
        t0 = time.monotonic()
        res = orca_api.assert_rpc_ok(
            "agentStatus.evaluateScreen",
            {"sessionId": _uid("f13-rule"), "agent": "claude", "screen": screen},
        )
        assert (time.monotonic() - t0) < 0.5
        assert res.get("state") == expected
        assert _req_str(res, "matchedRuleId")


@pytest.mark.tier1
@pytest.mark.f13
def test_f13_t1_osc_7501_facets_subagents_and_prompt_cache_ttl(orca_api: OrcaApiClient) -> None:
    sid = _uid("f13-osc")
    orca_api.assert_rpc_ok(
        "agentStatus.ingestOsc7501",
        {
            "sessionId": sid,
            "workingMode": "monitoring",
            "programStatus": "working",
            "interactivePrompt": {"prompt": "Keep tailing logs?"},
            "promptCacheTtlMs": 300_000,
        },
    )
    orca_api.assert_rpc_ok(
        "agentStatus.updateSubagents",
        {"sessionId": sid, "subagents": [{"id": "sub-1", "status": "running"}, {"id": "sub-2", "status": "running"}]},
    )
    st = orca_api.assert_rpc_ok("agentStatus.get", {"sessionId": sid})
    assert st.get("workingMode") == "monitoring"
    subs = _list_field(st, "subagents")
    assert len(subs) == 2 and {s.get("id") for s in subs} == {"sub-1", "sub-2"}
    assert int(st["promptCacheTtlMs"]) == 300_000
    assert int(st["promptCacheExpiresAt"]) > int(st.get("lastTurnActivityAt", 0))


def _f13_valid_hib_payload(session_id: str) -> dict[str, Any]:
    return {
        "sessionId": session_id, "state": "done", "tabVisible": False, "keystrokesSinceDone": 0,
        "agent": "claude", "idleMs": 1_800_000, "windowMs": 1_800_000,
        "mobileDriverActive": False, "unsettledDispatchCount": 0, "liveSubagentCount": 0,
    }


@pytest.mark.tier1
@pytest.mark.f13
def test_f13_t1_hibernation_8_check_pass_via_rest_cli_and_rpc(
    orca_api: OrcaApiClient, orca_cli: OrcaCliRunner
) -> None:
    payload = _f13_valid_hib_payload(_uid("f13-hib-ok"))
    rest = orca_api.assert_ok(orca_api.post("/api/v1/orca/ide/hibernation/check", json_body=payload))
    rpc = orca_api.assert_rpc_ok("hibernation.check", payload)
    cli = orca_cli.run(
        "ide", "hibernation-check", "--state", "done", "--agent", "claude",
        "--idle-ms", "1800000", "--window-ms", "1800000", "--tab-visible", "false",
        "--keystrokes-since-done", "0", "--mobile-driver", "false",
        "--unsettled-dispatches", "0", "--live-subagents", "0",
    ).assert_ok()
    for res in (rest, rpc, cli):
        assert isinstance(res, dict) and res.get("eligible") is True and res.get("failedChecks") == []
        assert isinstance(res.get("passedChecks"), list) and len(res["passedChecks"]) == 8


@pytest.mark.tier1
@pytest.mark.f13
def test_f13_t1_hibernation_sleep_and_transparent_resume_rehydration(
    orca_api: OrcaApiClient,
) -> None:
    sid, env, args = _uid("f13-sleep"), {"ORCA_KEEP_VAR": "preserved-13"}, ["--model", "sonnet"]
    slept = orca_api.assert_rpc_ok("hibernation.sleep", {**_f13_valid_hib_payload(sid), "env": env, "args": args})
    assert slept.get("sleeping") is True and slept.get("ptyRunning") is False
    resume = slept.get("resume")
    assert isinstance(resume, dict) and resume.get("agent") == "claude"
    resume_sid = _req_str(resume, "sessionId")
    assert resume.get("env") == env and resume.get("args") == args

    woke = orca_api.assert_rpc_ok("hibernation.wake", {"sessionId": sid})
    assert woke.get("sleeping") is False
    launch_cmd = str(woke.get("launchCommand") or woke.get("command") or "")
    assert "--resume" in launch_cmd and resume_sid in launch_cmd
    assert woke.get("env") == env and woke.get("args") == args
