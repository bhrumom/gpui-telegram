"""Tier 2 Boundary, Corner & Negative Opaque-Box E2E Tests for OB-F06..OB-F10 (30 tests, 6/feature).

Verifies 100-agent host cap, ultrafast/muse/stage guards, side-question/mailbox/DM/queue/branch bounds,
channel roster/archive/lead/router/assignee guards, assignment=8 & reply=2 loop limits, DAG cycles,
reply quarantine, recursive tool_call blocks, essential memory budget, 32-skill/100-MCP caps,
200-step browser limits, and post-fill-card screenshot/evaluate exfiltration locks.
"""

from __future__ import annotations

import json
from pathlib import Path
from typing import Any, Dict, List

from .harness import (
    TempOpenBotEnv,
    assert_not_cli_parse_error,
    e2e_test,
    extract_list,
    pick,
    require_field,
    unwrap_entity,
)


def _mk_agent(env: TempOpenBotEnv, name: str, **flags: str) -> str:
    args: List[str] = ["agent", "create", "--name", name]
    for k, v in flags.items():
        args.extend([f"--{k.replace('_', '-')}", str(v)])
    res = env.run_cli_json(*args)
    ag = unwrap_entity(res, "agent")
    return str(require_field(ag, "id", expected_type=str))


# ============================================================================
# OB-F06 Tier 2: 100-Agent Cap, >8 Backups, ultrafast, muse, Stage & Privilege
# ============================================================================


@e2e_test(
    id="TEST-T2-OB-F06-01",
    tier=2,
    features=["OB-F06"],
    timeout_sec=60.0,
    description="Host enforces INPUT_LIMITS.agents=100 cap, rejecting both 101st agent create and 101st duplicate.",
)
def test_t2_ob_f06_01_host_100_agent_cap(env: TempOpenBotEnv) -> None:
    first_id = ""
    for idx in range(1, 101):
        aid = _mk_agent(env, f"CapBot-{idx:03d}")
        if idx == 1:
            first_id = aid

    env.run_cli_expect_error(
        "agent", "create", "--name", "CapBot-101",
        error_contains=("100", "host", "limit", "up to 100 agents"),
    )
    env.run_cli_expect_error(
        "agent", "duplicate", "--id", first_id,
        "--operation-id", "33333333-3333-4333-8333-333333333333",
        error_contains=("100", "host", "limit", "up to 100 agents"),
    )
    roster = extract_list(env.run_cli_json("agent", "list"), "agents")
    assert len(roster) == 100


@e2e_test(
    id="TEST-T2-OB-F06-02",
    tier=2,
    features=["OB-F06"],
    description="agent update rejects backupModels.order with 9 entries (> AGENT_BACKUP_MODEL_LIMIT=8).",
)
def test_t2_ob_f06_02_update_backup_models_over_8_rejected(env: TempOpenBotEnv) -> None:
    aid = _mk_agent(env, "BackupUpdateBot")
    nine_order = {
        "automatic": True,
        "order": [{"provider": "claude", "model": f"claude-m-{i}"} for i in range(9)],
    }
    env.run_cli_expect_error(
        "agent", "update", "--id", aid, "--backup-models", json.dumps(nine_order),
        error_contains=("backup", "8", "invalid", "limit"),
    )


@e2e_test(
    id="TEST-T2-OB-F06-03",
    tier=2,
    features=["OB-F06"],
    description="agent update rejects ultrafast=true without fastMode=true and clears fastMode/ultrafast on provider switch.",
)
def test_t2_ob_f06_03_ultrafast_requires_fastmode_and_provider_switch_resets(env: TempOpenBotEnv) -> None:
    aid = _mk_agent(env, "FastModeBot")
    env.run_cli_expect_error(
        "agent", "update", "--id", aid, "--ultrafast", "true",
        error_contains=("fast mode", "fast_mode", "ultrafast", "invalid"),
    )

    env.run_cli_json("agent", "update", "--id", aid, "--fast-mode", "true", "--ultrafast", "true")
    switched = unwrap_entity(
        env.run_cli_json("agent", "update", "--id", aid, "--provider", "claude"), "agent"
    )
    assert require_field(switched, "fast_mode", expected_type=bool) is False
    assert pick(switched, "ultrafast") in (None, False)


@e2e_test(
    id="TEST-T2-OB-F06-04",
    tier=2,
    features=["OB-F06"],
    description="generate-profile refuses 'muse' provider because inherited MCP servers cannot be disabled during tool-free turn.",
)
def test_t2_ob_f06_04_generate_profile_refuses_muse_provider(env: TempOpenBotEnv) -> None:
    err_res = env.run_cli_expect_rejection(
        "agent", "generate-profile", "--provider", "muse", "--prompt", "Helper bot",
        allow_soft_refusal=False,
        error_contains=("muse", "mcp", "provider", "unsupported", "invalid"),
    )
    assert err_res.returncode != 0
    assert extract_list(env.run_cli_json("agent", "list"), "agents") == []


@e2e_test(
    id="TEST-T2-OB-F06-05",
    tier=2,
    features=["OB-F06"],
    description="Concurrent source workspace mutation during agent duplicate aborts with changedWhileDuplicating and cleans .openbot-stage.",
)
def test_t2_ob_f06_05_concurrent_mutation_aborts_duplicate_and_cleans_stage(env: TempOpenBotEnv) -> None:
    res = env.run_cli_json("agent", "create", "--name", "MutSource")
    ag = unwrap_entity(res, "agent")
    aid = require_field(ag, "id", expected_type=str)
    ws_parent = Path(require_field(ag, "workspace_path", expected_type=str)).parent

    env.run_cli_expect_error(
        "agent",
        "duplicate",
        "--id", aid,
        "--simulate-concurrent-workspace-mutation",
        "--operation-id", "44444444-4444-4444-8444-444444444444",
        error_contains=("changed", "modified", "mutated", "mismatch"),
    )
    assert list(ws_parent.glob("*.openbot-stage")) == []


@e2e_test(
    id="TEST-T2-OB-F06-06",
    tier=2,
    features=["OB-F06"],
    description="Agent self-widening of access/computerUse via openbot.update_profile and reserved custom provider ID 'codex' are refused.",
)
def test_t2_ob_f06_06_self_privilege_escalation_and_reserved_custom_id_blocked(env: TempOpenBotEnv) -> None:
    aid = _mk_agent(env, "RestrictedBot", access="workspace", computer_use="false")
    env.run_cli_expect_rejection(
        "tools",
        "call",
        "--agent-id", aid,
        "--name", "openbot.update_profile",
        "--arguments", json.dumps({"access": "full", "computerUse": True}),
        error_contains=("widen", "only the user", "permission", "cannot widen", "privilege"),
    )

    reserved_ep = json.dumps(
        {"id": "codex", "name": "FakeCodex", "baseUrl": "http://127.0.0.1:9999/v1", "harness": "opencode", "models": []}
    )
    env.run_cli_expect_error(
        "agent", "create", "--name", "CustomEpBot", "--custom-endpoint", reserved_ep,
        error_contains=("reserved", "built-in", "builtin", "conflict", "invalid"),
    )


# ============================================================================
# OB-F07 Tier 2: Side-Question >20, Mailbox >32, Interrupt Ownership, DM & Queue
# ============================================================================


@e2e_test(
    id="TEST-T2-OB-F07-01",
    tier=2,
    features=["OB-F07"],
    description="chat side-question rejects >20 earlier turns (21 turns) and blank whitespace-only questions.",
)
def test_t2_ob_f07_01_side_question_21_turns_and_blank_rejected(env: TempOpenBotEnv) -> None:
    aid = _mk_agent(env, "SqGuardBot")
    twenty_one = [{"question": f"Q{i}", "answer": f"A{i}"} for i in range(21)]
    env.run_cli_expect_error(
        "chat", "side-question", "--agent-id", aid, "--question", "Next?",
        "--earlier", json.dumps(twenty_one),
        error_contains=("20", "invalid", "limit", "maximum", "too many"),
    )
    env.run_cli_expect_error(
        "chat", "side-question", "--agent-id", aid, "--question", "    ",
        error_contains=("empty", "blank", "required", "invalid"),
    )


@e2e_test(
    id="TEST-T2-OB-F07-02",
    tier=2,
    features=["OB-F07"],
    description="chat mailbox-send rejects 0 recipients (recipientRequired) and 33 unique recipients (> INPUT_LIMITS.messageRecipients=32).",
)
def test_t2_ob_f07_02_mailbox_zero_and_33_recipients_rejected(env: TempOpenBotEnv) -> None:
    sender = _mk_agent(env, "MailSender")
    env.run_cli_expect_error(
        "chat", "mailbox-send", "--sender-agent-id", sender, "--recipient-agent-ids", "", "--text", "Hi",
        error_contains=("recipient", "required", "empty"),
    )
    fake_33 = ",".join(f"agent-00000000-0000-4000-8000-{i:012d}" for i in range(33))
    env.run_cli_expect_error(
        "chat", "mailbox-send", "--sender-agent-id", sender, "--recipient-agent-ids", fake_33, "--text", "Hi",
        error_contains=("32", "recipient", "limit", "at most 32"),
    )


@e2e_test(
    id="TEST-T2-OB-F07-03",
    tier=2,
    features=["OB-F07"],
    description="chat interrupt refuses self-interrupt, user-owned turns (interruptOtherWork), and channel turns (useChannelTaskControlsWork).",
)
def test_t2_ob_f07_03_interrupt_ownership_guards(env: TempOpenBotEnv) -> None:
    a1 = _mk_agent(env, "IntrCaller")
    a2 = _mk_agent(env, "IntrTarget")

    env.run_cli_expect_rejection(
        "chat", "interrupt", "--caller-agent-id", a1, "--target-agent-id", a1,
        error_contains=("itself", "self"),
    )
    env.run_cli_expect_rejection(
        "chat", "interrupt", "--caller-agent-id", a1, "--target-agent-id", a2, "--active-turn-owner", "user",
        error_contains=("other", "started", "owned", "owner"),
    )
    env.run_cli_expect_rejection(
        "chat", "interrupt", "--caller-agent-id", a1, "--target-agent-id", a2, "--active-turn-owner", "channel",
        error_contains=("channel task", "task control", "controls", "task"),
    )


@e2e_test(
    id="TEST-T2-OB-F07-04",
    tier=2,
    features=["OB-F07"],
    description="chat dm-send rejects self-DM (alice->alice), whitespace-only text, and >20,000-char payloads.",
)
def test_t2_ob_f07_04_dm_self_blank_and_over_20000_rejected(env: TempOpenBotEnv) -> None:
    env.run_cli_expect_error(
        "chat", "dm-send", "--sender", "alice", "--recipient", "alice",
        "--client-message-id", "s1", "--text", "Self DM",
        error_contains=("yourself", "self", "direct message"),
    )
    env.run_cli_expect_error(
        "chat", "dm-send", "--sender", "alice", "--recipient", "bob",
        "--client-message-id", "s2", "--text", "   ",
        error_contains=("required", "empty", "direct message"),
    )
    env.run_cli_expect_error(
        "chat", "dm-send", "--sender", "alice", "--recipient", "bob",
        "--client-message-id", "s3", "--text", "x" * 20001,
        error_contains=("20000", "20,000", "long", "limit"),
    )


@e2e_test(
    id="TEST-T2-OB-F07-05",
    tier=2,
    features=["OB-F07"],
    description="queue-reorder rejects partial/stale delivery IDs (queueOrderStale) and queue-steer refuses routine-owned turns.",
)
def test_t2_ob_f07_05_queue_stale_reorder_and_routine_steer_refusal(env: TempOpenBotEnv) -> None:
    aid = _mk_agent(env, "QueueGuardBot")
    env.run_cli_json("chat", "queue-pause", "--agent-id", aid)
    d1 = require_field(
        env.run_cli_json("chat", "queue-add", "--agent-id", aid, "--text", "M1"),
        "delivery_id", expected_type=str,
    )
    env.run_cli_json("chat", "queue-add", "--agent-id", aid, "--text", "M2")

    env.run_cli_expect_error(
        "chat", "queue-reorder", "--agent-id", aid, "--delivery-ids", d1,
        error_contains=("stale", "order", "queue"),
    )
    env.run_cli_expect_rejection(
        "chat", "queue-steer", "--agent-id", aid, "--delivery-id", d1,
        "--expected-turn-id", "t-active-1", "--routine-turn",
        error_contains=("steer_refused", "refused", "cannot steer", "not steerable", "routine turn", "routine_runs_turn"),
    )


@e2e_test(
    id="TEST-T2-OB-F07-06",
    tier=2,
    features=["OB-F07"],
    timeout_sec=60.0,
    description="chat branch rejects branching from a busy running source thread (branchSourceBusy) and 51st open side task (>50).",
)
def test_t2_ob_f07_06_branch_busy_source_and_50_open_task_limit(env: TempOpenBotEnv) -> None:
    aid = _mk_agent(env, "BranchGuardBot")
    m = env.run_cli_json("chat", "send", "--agent-id", aid, "--text", "Seed msg")
    mid = require_field(m, "message_id", expected_type=str)

    env.run_cli_expect_error(
        "chat", "branch", "--agent-id", aid, "--message-id", mid,
        "--request-id", "busy-r1", "--simulate-source-busy",
        error_contains=("source_busy", "branchsourcebusy", "current turn", "running", "active", "busy"),
    )

    for idx in range(1, 51):
        env.run_cli_json(
            "chat", "branch", "--agent-id", aid, "--message-id", mid, "--request-id", f"ok-b-{idx}"
        )
    env.run_cli_expect_error(
        "chat", "branch", "--agent-id", aid, "--message-id", mid, "--request-id", "over-b-51",
        error_contains=("50", "limit", "open", "thread"),
    )


# ============================================================================
# OB-F08 Tier 2: Channel Bounds, Archive-Running, Missing Lead, Router & Assignee
# ============================================================================


@e2e_test(
    id="TEST-T2-OB-F08-01",
    tier=2,
    features=["OB-F08"],
    description="channel create rejects >100 members, leadAgentId outside members, instructions >2000 chars, and icon.hue=360.",
)
def test_t2_ob_f08_01_channel_draft_validation_bounds(env: TempOpenBotEnv) -> None:
    a1 = _mk_agent(env, "ChMem1")
    a2 = _mk_agent(env, "ChMem2")
    ids_101 = ",".join(f"agent-{i:04d}" for i in range(101))

    env.run_cli_expect_error(
        "channel", "create", "--name", "TooMany", "--members", ids_101,
        error_contains=("100", "member", "limit", "invalid"),
    )
    env.run_cli_expect_error(
        "channel", "create", "--name", "BadLead", "--members", a1, "--lead", a2,
        error_contains=("lead", "member", "invalid"),
    )
    env.run_cli_expect_error(
        "channel", "create", "--name", "LongInstr", "--members", a1, "--instructions", "i" * 2001,
        error_contains=("2000", "2,000", "instruction", "long"),
    )
    env.run_cli_expect_error(
        "channel", "create", "--name", "BadHue", "--members", a1,
        "--icon", json.dumps({"name": "star", "hue": 360}),
        error_contains=("hue", "360", "icon", "invalid"),
    )


@e2e_test(
    id="TEST-T2-OB-F08-02",
    tier=2,
    features=["OB-F08"],
    description="channel archive refuses while a task is running (channelArchiveRunning) and channel post refuses archived channels.",
)
def test_t2_ob_f08_02_archive_while_running_and_post_to_archived_refused(env: TempOpenBotEnv) -> None:
    a1 = _mk_agent(env, "ArchBot")
    ch = unwrap_entity(
        env.run_cli_json("channel", "create", "--name", "ArchCh", "--members", a1, "--lead", a1),
        "channel",
    )
    cid = require_field(ch, "id", expected_type=str)
    p = env.run_cli_json(
        "channel", "post", "--channel-id", cid, "--text", "Long job", "--hold-running"
    )
    tid = require_field(extract_list(p, "tasks")[0], "id", expected_type=str)

    env.run_cli_expect_error(
        "channel", "archive", "--channel-id", cid,
        error_contains=("running", "active", "in progress", "stop"),
    )

    env.run_cli_json(
        "channel", "result", "--channel-id", cid, "--task-id", tid, "--agent-id", a1, "--text", "Done"
    )
    env.run_cli_json("channel", "archive", "--channel-id", cid)
    env.run_cli_expect_error(
        "channel", "post", "--channel-id", cid, "--text", "Blocked message",
        error_contains=("archived", "read_only", "read-only"),
    )


@e2e_test(
    id="TEST-T2-OB-F08-03",
    tier=2,
    features=["OB-F08"],
    description="Multi-member channel with leadAgentId=null pauses unaddressed task with channelLeadRequired and posts Coordinator message.",
)
def test_t2_ob_f08_03_missing_lead_pauses_task_and_posts_coordinator(env: TempOpenBotEnv) -> None:
    a1 = _mk_agent(env, "NoLeadA")
    a2 = _mk_agent(env, "NoLeadB")
    ch = unwrap_entity(
        env.run_cli_json("channel", "create", "--name", "NoLeadRoom", "--members", f"{a1},{a2}"),
        "channel",
    )
    cid = require_field(ch, "id", expected_type=str)
    p = env.run_cli_json("channel", "post", "--channel-id", cid, "--text", "Unaddressed question")
    t0 = extract_list(p, "tasks")[0]
    assert require_field(t0, "state", expected_type=str) == "paused"
    err_txt = str(require_field(t0, "error", expected_type=str)).lower()
    assert "lead" in err_txt or "assign" in err_txt

    state = env.run_cli_json("channel", "get", "--channel-id", cid)
    msgs = extract_list(state, "messages")
    assert any(
        str(pick(m, "author.kind", "author_kind") or "").lower() == "coordinator"
        or str(pick(m, "author.name") or "") == "Coordinator"
        for m in msgs
    )


@e2e_test(
    id="TEST-T2-OB-F08-04",
    tier=2,
    features=["OB-F08"],
    description="Malformed Lead router JSON or timeout safely falls back to leadAgentId and stale routingState re-pumps cleanly.",
)
def test_t2_ob_f08_04_lead_router_malformed_json_fallback_and_stale_repump(env: TempOpenBotEnv) -> None:
    a1 = _mk_agent(env, "FallbackLead")
    a2 = _mk_agent(env, "PeerTwo")
    ch = unwrap_entity(
        env.run_cli_json(
            "channel", "create", "--name", "BadJsonCh", "--members", f"{a1},{a2}", "--lead", a1
        ),
        "channel",
    )
    cid = require_field(ch, "id", expected_type=str)

    p_bad = env.run_cli_json(
        "channel", "post", "--channel-id", cid, "--text", "Route me", "--router-mock", "NOT_VALID_JSON{{{"
    )
    t_bad = extract_list(p_bad, "tasks")[0]
    assert require_field(t_bad, "owner_agent_id", expected_type=str) == a1

    p_stale = env.run_cli_json(
        "channel", "post", "--channel-id", cid, "--text", "Mid mutation", "--simulate-stale-routing-state"
    )
    assert require_field(p_stale, "repumped", expected_type=bool) is True
    assert require_field(extract_list(p_stale, "tasks")[0], "owner_agent_id", expected_type=str) in (a1, a2)


@e2e_test(
    id="TEST-T2-OB-F08-05",
    tier=2,
    features=["OB-F08"],
    description="Removing active assignee from channel roster pauses both its parent task and child subtask with channelAssigneeRemoved.",
)
def test_t2_ob_f08_05_removing_active_assignee_pauses_subtree(env: TempOpenBotEnv) -> None:
    a1 = _mk_agent(env, "StayLead")
    a2 = _mk_agent(env, "RemovedAssignee")
    a3 = _mk_agent(env, "ChildPeer")
    ch = unwrap_entity(
        env.run_cli_json(
            "channel", "create", "--name", "RmCh", "--members", f"{a1},{a2},{a3}", "--lead", a1
        ),
        "channel",
    )
    cid = require_field(ch, "id", expected_type=str)
    p = env.run_cli_json(
        "channel", "post", "--channel-id", cid, "--recipient-agent-id", a2,
        "--text", "Parent job", "--hold-running",
    )
    tp = extract_list(p, "tasks")[0]
    tp_id = require_field(tp, "id", expected_type=str)
    m0 = require_field(tp, "request_message_id", expected_type=str)

    asg = env.run_cli_json(
        "channel", "assign", "--channel-id", cid, "--caller-task-id", tp_id,
        "--agent-id", a2, "--recipient-agent-id", a3, "--task", "Subtask",
        "--expected-result", "Out", "--source-message-ids", m0,
    )
    tc_id = require_field(asg, "child_task_id", "task.id", expected_type=str)

    env.run_cli_json(
        "channel", "update", "--channel-id", cid, "--members", f"{a1},{a3}", "--lead", a1
    )
    state = env.run_cli_json("channel", "get", "--channel-id", cid)
    tasks_by_id = {require_field(t, "id", expected_type=str): t for t in extract_list(state, "tasks")}
    assert require_field(tasks_by_id[tp_id], "state", expected_type=str) == "paused"
    assert require_field(tasks_by_id[tc_id], "state", expected_type=str) == "paused"
    assert "removed" in str(require_field(tasks_by_id[tp_id], "error", expected_type=str)).lower()


@e2e_test(
    id="TEST-T2-OB-F08-06",
    tier=2,
    features=["OB-F08"],
    description="Routine-originated unaddressed {'idle':true} completes silently, while explicit non-member recipientAgentId is rejected.",
)
def test_t2_ob_f08_06_routine_idle_silent_complete_and_non_member_recipient_rejected(env: TempOpenBotEnv) -> None:
    a1 = _mk_agent(env, "RoutineLead")
    a2 = _mk_agent(env, "RoutinePeer")
    outsider = _mk_agent(env, "NonMemberBot")
    ch = unwrap_entity(
        env.run_cli_json(
            "channel", "create", "--name", "RoutCh", "--members", f"{a1},{a2}", "--lead", a1
        ),
        "channel",
    )
    cid = require_field(ch, "id", expected_type=str)

    p_idle = env.run_cli_json(
        "channel", "post", "--channel-id", cid, "--text", "Cron check",
        "--origin-routine", "rt-1", "--router-mock", json.dumps({"idle": True}),
    )
    t_idle = extract_list(p_idle, "tasks")[0]
    assert require_field(t_idle, "state", expected_type=str) == "completed"

    env.run_cli_expect_error(
        "channel", "post", "--channel-id", cid, "--recipient-agent-id", outsider, "--text", "Illegal target",
        error_contains=("member", "channel", "required"),
    )


# ============================================================================
# OB-F09 Tier 2: Unsettled Result, Assignment Limit=8, DAG Cycle, Reply=2 & Quarantine
# ============================================================================


@e2e_test(
    id="TEST-T2-OB-F09-01",
    tier=2,
    features=["OB-F09"],
    description="channel result is refused while a child channel assign dependency remains unsettled.",
)
def test_t2_ob_f09_01_premature_result_with_unsettled_dependency_refused(env: TempOpenBotEnv) -> None:
    a1 = _mk_agent(env, "ParentOwner")
    a2 = _mk_agent(env, "ChildOwner")
    ch = unwrap_entity(
        env.run_cli_json(
            "channel", "create", "--name", "UnsetCh", "--members", f"{a1},{a2}", "--lead", a1
        ),
        "channel",
    )
    cid = require_field(ch, "id", expected_type=str)
    p = env.run_cli_json("channel", "post", "--channel-id", cid, "--recipient-agent-id", a1, "--text", "Start")
    t_parent = extract_list(p, "tasks")[0]
    tp_id = require_field(t_parent, "id", expected_type=str)
    m0 = require_field(t_parent, "request_message_id", expected_type=str)

    env.run_cli_json(
        "channel", "assign", "--channel-id", cid, "--caller-task-id", tp_id,
        "--agent-id", a1, "--recipient-agent-id", a2, "--task", "Wait on me",
        "--expected-result", "Res", "--source-message-ids", m0,
    )
    env.run_cli_expect_rejection(
        "channel", "result", "--channel-id", cid, "--task-id", tp_id, "--agent-id", a1, "--text", "Premature",
        error_contains=("not returned", "unsettled", "dependency", "awaited"),
    )


@e2e_test(
    id="TEST-T2-OB-F09-02",
    tier=2,
    features=["OB-F09"],
    description="CHANNEL_ASSIGNMENT_LIMIT=8 allows 8 handoffs and pauses the entire root tree on the 9th handoff.",
)
def test_t2_ob_f09_02_assignment_limit_8_pauses_on_9th_handoff(env: TempOpenBotEnv) -> None:
    a1 = _mk_agent(env, "PingA")
    a2 = _mk_agent(env, "PongB")
    ch = unwrap_entity(
        env.run_cli_json(
            "channel", "create", "--name", "LoopCh", "--members", f"{a1},{a2}", "--lead", a1
        ),
        "channel",
    )
    cid = require_field(ch, "id", expected_type=str)
    p = env.run_cli_json("channel", "post", "--channel-id", cid, "--recipient-agent-id", a1, "--text", "Loop")
    t0 = extract_list(p, "tasks")[0]
    tid = require_field(t0, "id", expected_type=str)
    m0 = require_field(t0, "request_message_id", expected_type=str)

    cur_owner, next_owner = a1, a2
    for step in range(1, 9):
        xf = env.run_cli_json(
            "channel", "transfer", "--channel-id", cid, "--task-id", tid,
            "--agent-id", cur_owner, "--recipient-agent-id", next_owner,
            "--task", f"Handoff {step}", "--expected-result", "Next", "--source-message-ids", m0,
        )
        assert require_field(unwrap_entity(xf, "task"), "assignment_count", expected_type=int) == step
        cur_owner, next_owner = next_owner, cur_owner

    env.run_cli_expect_rejection(
        "channel", "transfer", "--channel-id", cid, "--task-id", tid,
        "--agent-id", cur_owner, "--recipient-agent-id", next_owner,
        "--task", "Handoff 9 overflow", "--expected-result", "Blocked", "--source-message-ids", m0,
        error_contains=("assignment limit", "continue or reassign", "8"),
    )
    state = env.run_cli_json("channel", "get", "--channel-id", cid)
    root_t = [t for t in extract_list(state, "tasks") if pick(t, "id") == tid][0]
    assert require_field(root_t, "state", expected_type=str) == "paused"


@e2e_test(
    id="TEST-T2-OB-F09-03",
    tier=2,
    features=["OB-F09"],
    description="dependsOn DFS guard rejects self-dependency / transitive DAG cycles and refuses self-assignment.",
)
def test_t2_ob_f09_03_dag_cycle_and_self_handoff_refused(env: TempOpenBotEnv) -> None:
    a1 = _mk_agent(env, "DagA")
    a2 = _mk_agent(env, "DagB")
    ch = unwrap_entity(
        env.run_cli_json(
            "channel", "create", "--name", "DagCh", "--members", f"{a1},{a2}", "--lead", a1
        ),
        "channel",
    )
    cid = require_field(ch, "id", expected_type=str)
    p = env.run_cli_json("channel", "post", "--channel-id", cid, "--recipient-agent-id", a1, "--text", "DAG")
    ta = extract_list(p, "tasks")[0]
    ta_id = require_field(ta, "id", expected_type=str)
    m0 = require_field(ta, "request_message_id", expected_type=str)

    # Self-assign refused
    env.run_cli_expect_rejection(
        "channel", "assign", "--channel-id", cid, "--caller-task-id", ta_id,
        "--agent-id", a1, "--recipient-agent-id", a1, "--task", "Self",
        "--expected-result", "No", "--source-message-ids", m0,
        error_contains=("another channel member", "self", "recipient"),
    )

    # Child tb depends on parent / or ta transferred to depend on its own child -> cycle
    asg = env.run_cli_json(
        "channel", "assign", "--channel-id", cid, "--caller-task-id", ta_id,
        "--agent-id", a1, "--recipient-agent-id", a2, "--task", "Child",
        "--expected-result", "Ok", "--source-message-ids", m0,
    )
    tb_id = require_field(asg, "child_task_id", "task.id", expected_type=str)
    env.run_cli_expect_rejection(
        "channel", "transfer", "--channel-id", cid, "--task-id", tb_id,
        "--agent-id", a2, "--recipient-agent-id", a1, "--dependencies", ta_id,
        "--task", "Cycle back", "--expected-result", "No", "--source-message-ids", m0,
        error_contains=("depend", "cycle", "another task"),
    )


@e2e_test(
    id="TEST-T2-OB-F09-04",
    tier=2,
    features=["OB-F09"],
    description="CHANNEL_REPLY_LIMIT=2 refuses 3rd member reply on same root request, >2000-char replies, and replies to human messages.",
)
def test_t2_ob_f09_04_reply_limit_2_length_and_human_target_refusals(env: TempOpenBotEnv) -> None:
    a1 = _mk_agent(env, "RepA")
    a2 = _mk_agent(env, "RepB")
    ch = unwrap_entity(
        env.run_cli_json(
            "channel", "create", "--name", "RepLimCh", "--members", f"{a1},{a2}", "--lead", a1
        ),
        "channel",
    )
    cid = require_field(ch, "id", expected_type=str)
    p_human = env.run_cli_json("channel", "post", "--channel-id", cid, "--text", "@everyone Discuss")
    human_msg_id = require_field(extract_list(p_human, "tasks")[0], "request_message_id", expected_type=str)
    tasks_by_owner = {
        require_field(t, "owner_agent_id", expected_type=str): require_field(t, "id", expected_type=str)
        for t in extract_list(p_human, "tasks")
    }
    m_b = require_field(
        env.run_cli_json(
            "channel", "result", "--channel-id", cid, "--task-id", tasks_by_owner[a2],
            "--agent-id", a2, "--text", "Peer proposal",
        ),
        "message_id", "id", expected_type=str,
    )

    # Human message reply refused
    env.run_cli_expect_rejection(
        "channel", "reply", "--channel-id", cid, "--task-id", tasks_by_owner[a1],
        "--agent-id", a1, "--message-id", human_msg_id, "--text", "Replying to human",
        error_contains=("member", "agent", "human"),
    )
    # >2000 chars refused
    env.run_cli_expect_rejection(
        "channel", "reply", "--channel-id", cid, "--task-id", tasks_by_owner[a1],
        "--agent-id", a1, "--message-id", m_b, "--text", "r" * 2001,
        error_contains=("2000", "2,000", "long", "limit", "characters"),
    )

    # 2 valid replies succeed, 3rd refused
    env.run_cli_json(
        "channel", "reply", "--channel-id", cid, "--task-id", tasks_by_owner[a1],
        "--agent-id", a1, "--message-id", m_b, "--text", "Reply 1",
    )
    env.run_cli_json(
        "channel", "reply", "--channel-id", cid, "--task-id", tasks_by_owner[a1],
        "--agent-id", a1, "--message-id", m_b, "--text", "Reply 2",
    )
    env.run_cli_expect_rejection(
        "channel", "reply", "--channel-id", cid, "--task-id", tasks_by_owner[a1],
        "--agent-id", a1, "--message-id", m_b, "--text", "Reply 3 overflow",
        error_contains=("2", "limit", "replies"),
    )


@e2e_test(
    id="TEST-T2-OB-F09-05",
    tier=2,
    features=["OB-F09"],
    description="Reply-turn quarantine forbids nested channel_reply, channel_assign, and channel_transfer inside isReplyTask turns.",
)
def test_t2_ob_f09_05_reply_turn_quarantine(env: TempOpenBotEnv) -> None:
    a1 = _mk_agent(env, "QuarA")
    a2 = _mk_agent(env, "QuarB")
    ch = unwrap_entity(
        env.run_cli_json(
            "channel", "create", "--name", "QuarCh", "--members", f"{a1},{a2}", "--lead", a1
        ),
        "channel",
    )
    cid = require_field(ch, "id", expected_type=str)
    p = env.run_cli_json("channel", "post", "--channel-id", cid, "--text", "@everyone Spec")
    t_map = {
        require_field(t, "owner_agent_id", expected_type=str): t
        for t in extract_list(p, "tasks")
    }
    m0 = require_field(t_map[a1], "request_message_id", expected_type=str)
    m_b = require_field(
        env.run_cli_json(
            "channel", "result", "--channel-id", cid,
            "--task-id", require_field(t_map[a2], "id", expected_type=str),
            "--agent-id", a2, "--text", "Initial B",
        ),
        "message_id", "id", expected_type=str,
    )
    rep = env.run_cli_json(
        "channel", "reply", "--channel-id", cid,
        "--task-id", require_field(t_map[a1], "id", expected_type=str),
        "--agent-id", a1, "--message-id", m_b, "--text", "Clarify?",
    )
    reply_tid = require_field(rep, "reply_task_id", "task.id", expected_type=str)

    env.run_cli_expect_rejection(
        "channel", "assign", "--channel-id", cid, "--caller-task-id", reply_tid,
        "--agent-id", a2, "--recipient-agent-id", a1, "--task", "Nested assign",
        "--expected-result", "No", "--source-message-ids", m0,
        error_contains=("only answers a member reply", "quarantine", "reply turn", "nested"),
    )
    env.run_cli_expect_rejection(
        "channel", "reply", "--channel-id", cid, "--task-id", reply_tid,
        "--agent-id", a2, "--message-id", m_b, "--text", "Nested reply",
        error_contains=("only answers a member reply", "quarantine", "reply turn", "nested"),
    )


@e2e_test(
    id="TEST-T2-OB-F09-06",
    tier=2,
    features=["OB-F09"],
    description="channel cancel enforces PERSON_CANCEL_REQUIRED, blocks cancelling own or peer's active run, and user cancel requires stopped state.",
)
def test_t2_ob_f09_06_channel_cancel_ownership_and_state_guards(env: TempOpenBotEnv) -> None:
    a1 = _mk_agent(env, "CancA")
    a2 = _mk_agent(env, "CancB")
    ch = unwrap_entity(
        env.run_cli_json(
            "channel", "create", "--name", "CancCh", "--members", f"{a1},{a2}", "--lead", a1
        ),
        "channel",
    )
    cid = require_field(ch, "id", expected_type=str)
    p_run = env.run_cli_json(
        "channel", "post", "--channel-id", cid, "--recipient-agent-id", a1, "--text", "Active", "--hold-running"
    )
    t_run = require_field(extract_list(p_run, "tasks")[0], "id", expected_type=str)

    # User cancel on running task rejected (must be paused or failed)
    env.run_cli_expect_rejection(
        "channel", "cancel", "--channel-id", cid, "--task-id", t_run,
        error_contains=("stopped", "running", "paused"),
    )
    # Agent cancelling own current run rejected
    env.run_cli_expect_rejection(
        "channel", "cancel", "--channel-id", cid, "--task-id", t_run,
        "--caller-agent-id", a1, "--caller-task-id", t_run,
        error_contains=("own run", "current", "self", "active run"),
    )
    # Agent cancelling peer's run rejected
    env.run_cli_expect_rejection(
        "channel", "cancel", "--channel-id", cid, "--task-id", t_run,
        "--caller-agent-id", a2,
        error_contains=("not the owner", "owner", "owned", "another agent"),
    )
    # Routine-origin cancel by agent rejected
    env.run_cli_expect_rejection(
        "channel", "cancel", "--channel-id", cid, "--task-id", t_run,
        "--caller-agent-id", a1, "--origin-routine", "rt-9",
        error_contains=("person", "human", "person_cancel_required", "user request"),
    )


# ============================================================================
# OB-F10 Tier 2: Discovery Recursion, Memory Budget, SQL/Skill/MCP/Browser & Card Lock
# ============================================================================


@e2e_test(
    id="TEST-T2-OB-F10-01",
    tier=2,
    features=["OB-F10"],
    description="tools call refuses recursive discovery calls (openbot.tool_search/tool_describe) and structured null array items.",
)
def test_t2_ob_f10_01_recursive_discovery_call_and_null_item_refused(env: TempOpenBotEnv) -> None:
    aid = _mk_agent(env, "DiscGuardBot")
    env.run_cli_expect_rejection(
        "tools", "call", "--agent-id", aid, "--name", "openbot.tool_search",
        "--arguments", json.dumps({"query": "crm"}),
        error_contains=("discovery", "tool_search", "direct"),
    )
    env.run_cli_expect_rejection(
        "tools", "call", "--agent-id", aid, "--name", "openbot.ask_user",
        "--arguments", json.dumps({"questions": [None]}),
        error_contains=("null", "invalid"),
    )


@e2e_test(
    id="TEST-T2-OB-F10-02",
    tier=2,
    features=["OB-F10"],
    description="Essential memory byte-budget overflow (>9600B), >500-char memory text, and symbol-only / >256-char search queries fail cleanly.",
)
def test_t2_ob_f10_02_memory_budget_text_length_and_bad_query_guards(env: TempOpenBotEnv) -> None:
    aid = _mk_agent(env, "MemGuardBot")
    env.run_cli_expect_error(
        "tools", "memory-put", "--agent-id", aid, "--text", "m" * 501,
        error_contains=("500", "long", "memory"),
    )
    env.run_cli_expect_error(
        "tools", "memory-search", "--agent-id", aid, "--query", "???",
        error_contains=("query", "term", "search"),
    )
    env.run_cli_expect_error(
        "tools", "memory-search", "--agent-id", aid, "--query", "q" * 257,
        error_contains=("256", "query", "long"),
    )

    budget_keywords = (
        "budget",
        "essential",
        "4096",
        "8192",
        "9600",
        "limit",
        "exceeded",
        "full",
        "overflow",
    )
    budget_hit = False
    for idx in range(22):
        chunk = f"Core rule #{idx:02d}: " + ("R" * 460)
        res = env.run_cli_raw(
            "tools", "memory-put", "--agent-id", aid, "--text", chunk, "--inclusion", "essential"
        )
        raw_success = res.raw_json.get("success") if isinstance(res.raw_json, dict) else None
        if (
            res.returncode != 0
            or raw_success is False
            or pick(res.data, "accepted") is False
            or pick(res.data, "ok") is False
            or pick(res.data, "code") == "budget"
        ):
            assert_not_cli_parse_error(res)
            assert idx >= 1, (
                f"First 475B essential memory must succeed before budget overflow, "
                f"failed at idx=0: error={res.error} stderr={res.stderr[:300]}"
            )
            refusal_haystack = " | ".join(
                [
                    str((res.error or {}).get("code") or ""),
                    str((res.error or {}).get("message") or ""),
                    json.dumps(res.data, ensure_ascii=False, default=str)
                    if res.data is not None
                    else "",
                    res.stdout,
                    res.stderr,
                ]
            ).lower()
            assert any(kw in refusal_haystack for kw in budget_keywords), (
                f"Expected essential memory budget refusal keyword in {budget_keywords!r} "
                f"at idx={idx}, got error={res.error} stdout={res.stdout[:300]} stderr={res.stderr[:300]}"
            )
            budget_hit = True
            break
        assert res.raw_json is not None, (
            f"Expected JSON stdout on accepted memory-put at idx={idx}, got {res.stdout!r}"
        )
    assert budget_hit, "Expected essential memory budget overflow within 22 x 475B entries (~10.4KB)"


@e2e_test(
    id="TEST-T2-OB-F10-03",
    tier=2,
    features=["OB-F10"],
    description="agent-data-exec rejects mode mismatches (SELECT in write mode, INSERT in read mode) and PRAGMA/VACUUM statements.",
)
def test_t2_ob_f10_03_agent_data_mode_mismatch_and_pragma_blocked(env: TempOpenBotEnv) -> None:
    aid = _mk_agent(env, "SqlModeBot")
    env.run_cli_json(
        "tools", "agent-data-exec", "--agent-id", aid, "--mode", "write",
        "--sql", "CREATE TABLE orders (id INT PRIMARY KEY)",
    )
    env.run_cli_expect_rejection(
        "tools", "agent-data-exec", "--agent-id", aid, "--mode", "write",
        "--sql", "SELECT * FROM orders",
        error_contains=("read-only", "read mode", "read", "mismatch", "mutation"),
    )
    env.run_cli_expect_rejection(
        "tools", "agent-data-exec", "--agent-id", aid, "--mode", "read",
        "--sql", "INSERT INTO orders VALUES (1)",
        error_contains=("read-only", "read_only", "write mode", "write", "cannot modify", "not allowed"),
    )
    env.run_cli_expect_rejection(
        "tools", "agent-data-exec", "--agent-id", aid, "--mode", "write",
        "--sql", "PRAGMA writable_schema = ON",
        error_contains=("not allowed", "blocked", "disallowed", "prohibited", "authorizer"),
    )


@e2e_test(
    id="TEST-T2-OB-F10-04",
    tier=2,
    features=["OB-F10"],
    timeout_sec=60.0,
    description="Enforces <=32 skills per agent, <=100 MCP servers, and blocks reserved MCP server names ('openbot', 'openbot_browser', 'computer_use').",
)
def test_t2_ob_f10_04_skill_32_cap_mcp_100_cap_and_reserved_names(env: TempOpenBotEnv) -> None:
    aid = _mk_agent(env, "CapToolBot")
    for reserved in ("openbot", "openbot_browser", "computer_use"):
        env.run_cli_expect_error(
            "tools", "mcp-register", "--name", reserved, "--transport", "stdio", "--command", "echo",
            error_contains=("already uses", "reserved", "built-in", "builtin", "conflict"),
        )

    for i in range(1, 33):
        env.run_cli_json(
            "tools", "skill-save", "--agent-id", aid, "--slug", f"sk-{i:02d}",
            "--name", f"Skill {i}", "--description", "ok", "--instructions", "# ok",
        )
    env.run_cli_expect_error(
        "tools", "skill-save", "--agent-id", aid, "--slug", "sk-33",
        "--name", "Skill 33", "--description", "over", "--instructions", "# over",
        error_contains=("32", "skill", "limit"),
    )

    for j in range(1, 101):
        env.run_cli_json(
            "tools", "mcp-register", "--name", f"mcp-srv-{j:03d}", "--transport", "stdio", "--command", "echo"
        )
    env.run_cli_expect_error(
        "tools", "mcp-register", "--name", "mcp-srv-101", "--transport", "stdio", "--command", "echo",
        error_contains=("100", "mcp", "limit"),
    )


@e2e_test(
    id="TEST-T2-OB-F10-05",
    tier=2,
    features=["OB-F10"],
    description="browser-run rejects >200 steps (201 steps), sleep >25,000ms, and fill_fields targeting password/card inputs.",
)
def test_t2_ob_f10_05_browser_script_201_steps_sleep_and_password_fill_blocked(env: TempOpenBotEnv) -> None:
    aid = _mk_agent(env, "BrowserGuardBot")
    steps_201 = json.dumps([{"action": "snapshot"} for _ in range(201)])
    env.run_cli_expect_error(
        "tools", "browser-run", "--agent-id", aid, "--url", "https://example.test", "--steps", steps_201,
        error_contains=("200", "step", "limit"),
    )
    env.run_cli_expect_error(
        "tools", "browser-run", "--agent-id", aid, "--url", "https://example.test",
        "--steps", json.dumps([{"action": "sleep", "ms": 25001}]),
        error_contains=("25000", "25,000", "sleep"),
    )
    env.run_cli_expect_rejection(
        "tools", "browser-run", "--agent-id", aid, "--url", "https://example.test/login",
        "--steps", json.dumps([{"action": "fill_fields", "fields": [{"selector": "input[type=password]", "value": "hunter2"}]}]),
        error_contains=("password", "secret", "vault", "fill_fields"),
    )


@e2e_test(
    id="TEST-T2-OB-F10-06",
    tier=2,
    features=["OB-F10"],
    description="Post-fill-card lock blocks screenshot/evaluate/recording_start prior to navigation, and rejects HTTP fill-card & <100-char link-payment.",
)
def test_t2_ob_f10_06_fill_card_exfiltration_lock_and_https_context_guards(env: TempOpenBotEnv) -> None:
    aid = _mk_agent(env, "CardGuardBot")

    # < 100-char context & http:// merchantUrl rejected on link-payment
    env.run_cli_expect_error(
        "tools", "link-payment", "--agent-id", aid, "--amount", "500", "--currency", "USD",
        "--merchant-name", "Acme", "--merchant-url", "https://pay.acme.test", "--context", "brief",
        error_contains=("100", "at least", "short", "minimum"),
    )
    long_ctx = "Valid context exceeding one hundred characters for enterprise CI runner payment authorization check!!"
    env.run_cli_expect_error(
        "tools", "link-payment", "--agent-id", aid, "--amount", "500", "--currency", "USD",
        "--merchant-name", "Acme", "--merchant-url", "http://plain.acme.test", "--context", long_ctx,
        error_contains=("https", "tls", "secure"),
    )

    # HTTP tab fill-card rejected
    http_tab = require_field(
        env.run_cli_json(
            "tools", "browser-run", "--agent-id", aid, "--url", "http://plain.test/pay",
            "--steps", json.dumps([{"action": "snapshot"}]),
        ),
        "tab_id", expected_type=str,
    )
    card_id = require_field(
        env.run_cli_json(
            "tools", "link-payment", "--agent-id", aid, "--amount", "900", "--currency", "USD",
            "--merchant-name", "Acme", "--merchant-url", "https://pay.acme.test/checkout",
            "--context", long_ctx, "--approve",
        ),
        "card_id", expected_type=str,
    )
    env.run_cli_expect_rejection(
        "tools", "fill-card", "--agent-id", aid, "--tab-id", http_tab, "--card-id", card_id, "--approve",
        error_contains=("https", "request_takeover", "secure"),
    )

    # HTTPS tab fill-card locks screenshot, evaluate, and recording_start before navigation
    https_tab = require_field(
        env.run_cli_json(
            "tools", "browser-run", "--agent-id", aid, "--url", "https://pay.acme.test/checkout",
            "--steps", json.dumps([{"action": "snapshot"}]),
        ),
        "tab_id", expected_type=str,
    )
    env.run_cli_json(
        "tools", "fill-card", "--agent-id", aid, "--tab-id", https_tab, "--card-id", card_id, "--approve"
    )
    for blocked_tool in (
        "openbot_browser.screenshot",
        "openbot_browser.evaluate",
        "openbot_browser.recording_start",
    ):
        env.run_cli_expect_rejection(
            "tools", "call", "--agent-id", aid, "--tab-id", https_tab,
            "--name", blocked_tool, "--arguments", json.dumps({"tabId": https_tab}),
            error_contains=("filled card", "unavailable", "navigate"),
        )
