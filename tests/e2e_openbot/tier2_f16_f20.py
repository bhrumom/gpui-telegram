"""Tier 2 Boundary, Corner & Negative Opaque-Box E2E Tests for OB-F16..OB-F20 (30 tests, 6/feature).

Verifies 4,000-char paste-to-file boundary, 100/250 MiB & 10-attachment caps, routine-turn steerRefused,
@/$ picker filtering, SidePanel split bounds, ChannelEditor non-reorderable members & 0/101-member guards,
channel reaction/pin prohibitions ('not-pinnable-chat'), archived read-only channel blocks, WorkflowGraph
edge/size/loop guards, Marketplace alpha fallback, detached chatWindow settings hiding, shortcut collisions,
DynamicIsland critical preemption lock, ChatWindow auto-close, AgentDock/BrowserDock/CUA/Recovery bounds,
CLI resilience (malformed JSON, unknown flags, corrupt JSON fallback), loopback /run guards, and baseline safety.
"""

from __future__ import annotations

import json
from typing import Any, Dict, List, Sequence

from .harness import (
    TempOpenBotEnv,
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


def _mk_channel(
    env: TempOpenBotEnv, name: str, members: Sequence[str], **flags: str
) -> Dict[str, Any]:
    args: List[str] = ["channel", "create", "--name", name, "--members", ",".join(members)]
    for k, v in flags.items():
        args.extend([f"--{k.replace('_', '-')}", str(v)])
    res = env.run_cli_json(*args)
    return unwrap_entity(res, "channel")


def _probe_ui(
    env: TempOpenBotEnv, scenario: str, wrapper_key: str, **params: Any
) -> Dict[str, Any]:
    arg = json.dumps({"scenario": scenario, **params}) if params else scenario
    res = env.run_cli_json("ui-probe", "--scenario", arg)
    return unwrap_entity(res, wrapper_key)


def _probe_ui_expect_error(
    env: TempOpenBotEnv,
    scenario: str,
    error_contains: Sequence[str],
    **params: Any,
) -> Any:
    arg = json.dumps({"scenario": scenario, **params})
    res = env.run_cli_expect_error(
        "ui-probe",
        "--scenario",
        arg,
        expected_exit_codes=(2, 4),
        error_contains=error_contains,
    )
    assert res.returncode in (2, 4)
    return res


# ============================================================================
# OB-F16 Tier 2: Paste Boundary, Attachment Caps, steerRefused & Split Bounds
# ============================================================================


@e2e_test(
    id="TEST-T2-OB-F16-01",
    tier=2,
    features=["OB-F16"],
    description="Composer enforces exact 4,000 (inline) vs 4,001 (pasted-text.txt) paste boundary and 100,000-char messageText cap.",
)
def test_t2_ob_f16_01_composer_paste_4000_boundary_and_100k_cap(env: TempOpenBotEnv) -> None:
    p4000 = _probe_ui(env, "composer-queue-sidepanel", "composer_probe", paste_length=4000)
    assert require_field(p4000, "pasted_as_file", expected_type=bool) is False
    assert require_field(p4000, "inline_text_length", expected_type=int) == 4000

    p4001 = _probe_ui(env, "composer-queue-sidepanel", "composer_probe", paste_length=4001)
    assert require_field(p4001, "pasted_as_file", expected_type=bool) is True
    assert require_field(p4001, "pasted_file_name", expected_type=str) == "pasted-text.txt"

    p105k = _probe_ui(env, "composer-queue-sidepanel", "composer_probe", paste_length=105_000)
    assert require_field(p105k, "inline_text_length", expected_type=int) <= 100_000


@e2e_test(
    id="TEST-T2-OB-F16-02",
    tier=2,
    features=["OB-F16"],
    description="Composer and chat send reject >100 MiB single file, >250 MiB total attachments, and >10 message attachments.",
)
def test_t2_ob_f16_02_attachment_100mib_250mib_and_10_count_limits(env: TempOpenBotEnv) -> None:
    mib = 1024 * 1024
    _probe_ui_expect_error(
        env,
        "composer-queue-sidepanel",
        ("100", "size", "limit", "exceeds", "maximum"),
        attachments=[
            {
                "id": "att_big",
                "name": "huge.bin",
                "size": 101 * mib,
                "bytes": 101 * mib,
                "kind": "file",
                "mimeType": "application/octet-stream",
                "previewKind": "none",
                "previewUrl": None,
            }
        ],
    )
    _probe_ui_expect_error(
        env,
        "composer-queue-sidepanel",
        ("250", "total", "limit", "exceeds", "maximum"),
        attachments=[
            {
                "id": f"att_p{i}",
                "name": f"part{i}.bin",
                "size": 90 * mib,
                "bytes": 90 * mib,
                "kind": "file",
                "mimeType": "application/octet-stream",
                "previewKind": "none",
                "previewUrl": None,
            }
            for i in range(3)
        ],
    )

    aid = _mk_agent(env, "AttachGuardBot")
    eleven_files = [
        {
            "id": f"att_{chr(97 + i)}",
            "name": f"f_{chr(97 + i)}.txt",
            "size": 128,
            "bytes": 128,
            "kind": "file",
            "mimeType": "text/plain",
            "previewKind": "none",
            "previewUrl": None,
        }
        for i in range(11)
    ]
    env.run_cli_expect_error(
        "chat",
        "send",
        "--agent-id",
        aid,
        "--text",
        "Over cap",
        "--attachments",
        json.dumps(eleven_files),
        error_contains=("10", "limit", "maximum", "too many"),
    )


@e2e_test(
    id="TEST-T2-OB-F16-03",
    tier=2,
    features=["OB-F16"],
    description="steerRefused routine-turn guard blocks both chat queue-steer --routine-turn and Composer Tab steer.",
)
def test_t2_ob_f16_03_routine_turn_steer_refused_guard(env: TempOpenBotEnv) -> None:
    aid = _mk_agent(env, "RoutineSteerGuard")
    env.run_cli_json("chat", "queue-pause", "--agent-id", aid)
    deliv = env.run_cli_json("chat", "queue-add", "--agent-id", aid, "--text", "Attempt steer")
    did = require_field(deliv, "delivery_id", expected_type=str)

    env.run_cli_expect_rejection(
        "chat",
        "queue-steer",
        "--agent-id",
        aid,
        "--delivery-id",
        did,
        "--expected-turn-id",
        "t-active-01",
        "--routine-turn",
        error_contains=("steer_refused", "cannot steer", "not steerable", "routine turn", "refused"),
    )

    cp = _probe_ui(
        env,
        "composer-queue-sidepanel",
        "composer_probe",
        active_turn_routine=True,
        press_tab_steer=True,
    )
    qp = require_field(cp, "queue_panel", expected_type=dict)
    assert require_field(qp, "can_steer", expected_type=bool) is False
    assert require_field(qp, "steer_refused", expected_type=bool) is True
    assert require_field(qp, "steered_on_tab", expected_type=bool) is False


@e2e_test(
    id="TEST-T2-OB-F16-04",
    tier=2,
    features=["OB-F16"],
    description="@ picker <3-char query omits debounced messages section and hides @everyone in 1:1 chat.",
)
def test_t2_ob_f16_04_at_picker_short_query_and_dm_hides_everyone(env: TempOpenBotEnv) -> None:
    cp = _probe_ui(
        env, "composer-queue-sidepanel", "composer_probe", trigger="@", query="ab", in_channel=False
    )
    assert "messages" not in extract_list(cp, "at_sections")
    assert require_field(cp, "everyone_shown", expected_type=bool) is False


@e2e_test(
    id="TEST-T2-OB-F16-05",
    tier=2,
    features=["OB-F16"],
    description="$ picker excludes disabled MCP servers and 'needs-repair' skills from selectable items while listing skill_problems.",
)
def test_t2_ob_f16_05_dollar_picker_excludes_disabled_mcp_and_broken_skills(
    env: TempOpenBotEnv,
) -> None:
    cp = _probe_ui(
        env,
        "composer-queue-sidepanel",
        "composer_probe",
        trigger="$",
        query="sec",
        skills=[
            {"slug": "sec-ok", "name": "Sec Valid", "status": "ok"},
            {"slug": "sec-broken", "name": "Sec Broken", "status": "needs-repair"},
        ],
        mcp_servers=[
            {"id": "mcp-off", "name": "Sec Disabled MCP", "enabled": False},
            {"id": "mcp-on", "name": "Sec Active MCP", "enabled": True},
        ],
    )
    selectable = json.dumps(extract_list(cp, "dollar_items"))
    assert "mcp-off" not in selectable and "Sec Disabled MCP" not in selectable
    assert "sec-broken" not in selectable
    problems = json.dumps(extract_list(cp, "skill_problems"))
    assert "sec-broken" in problems or "Sec Broken" in problems


@e2e_test(
    id="TEST-T2-OB-F16-06",
    tier=2,
    features=["OB-F16"],
    description="SidePanel rejects 3-way vertical split (> MAX_SIDE_SPLITS=2), <120px split height, and out-of-range split_share (0.0, 1.0, -0.5).",
)
def test_t2_ob_f16_06_side_panel_max_splits_min_px_and_share_bounds(env: TempOpenBotEnv) -> None:
    r3 = _probe_ui_expect_error(
        env,
        "composer-queue-sidepanel",
        ("2", "max", "limit", "too many"),
        side_splits=["browser", "files", "threads"],
    )
    assert r3.returncode == 2
    r_px = _probe_ui_expect_error(
        env,
        "composer-queue-sidepanel",
        ("120", "minimum", "min", "too small", "bounds"),
        side_splits=["browser", "files"],
        split_height_px=80,
    )
    assert r_px.returncode == 2
    for bad_share in (0.0, 1.0, -0.5):
        r_sh = _probe_ui_expect_error(
            env,
            "composer-queue-sidepanel",
            ("range", "between", "bounds", "invalid", "ratio"),
            side_splits=["browser", "files"],
            split_share=bad_share,
        )
        assert r_sh.returncode == 2


# ============================================================================
# OB-F17 Tier 2: Non-Reorderable Roster, No Channel Pin/React & Archive Guard
# ============================================================================


@e2e_test(
    id="TEST-T2-OB-F17-01",
    tier=2,
    features=["OB-F17"],
    description="ChannelEditor forbids member drag-reorder (member_drag_reorder_supported=False) and channel create rejects 0 or >100 members.",
)
def test_t2_ob_f17_01_channel_editor_no_drag_reorder_and_roster_bounds(env: TempOpenBotEnv) -> None:
    cui = _probe_ui(
        env, "channel-conversation-recovery", "channel_ui", attempt_member_drag_reorder=True
    )
    assert require_field(cui, "member_drag_reorder_supported", expected_type=bool) is False

    env.run_cli_expect_error(
        "channel",
        "create",
        "--name",
        "NoRoster",
        "--members",
        "",
        error_contains=("empty", "at least", "required", "minimum"),
    )

    ids_101 = ",".join(f"ag-x{i:x}" for i in range(101))
    env.run_cli_expect_error(
        "channel",
        "create",
        "--name",
        "HugeRoster",
        "--members",
        ids_101,
        error_contains=("100", "limit", "maximum", "too many"),
    )


@e2e_test(
    id="TEST-T2-OB-F17-02",
    tier=2,
    features=["OB-F17"],
    description="ChannelAvatar handles unknown icon name ('dragon'), invalid hue fallback (215), and broken imageUrl fallback to member faces.",
)
def test_t2_ob_f17_02_channel_unknown_icon_hue_fallback_and_broken_image(env: TempOpenBotEnv) -> None:
    c_bad_icon = _probe_ui(
        env,
        "channel-conversation-recovery",
        "channel_ui",
        icon={"name": "dragon", "hue": 999},
        simulate_broken_image=True,
        members_count=3,
    )
    assert pick(c_bad_icon, "resolved_icon") is None
    assert require_field(c_bad_icon, "image_fallback_active", expected_type=bool) is True
    assert require_field(c_bad_icon, "header_row_faces", expected_type=int) == 3

    c_hue_norm = _probe_ui(
        env,
        "channel-conversation-recovery",
        "channel_ui",
        icon={"name": "shield", "hue": 999},
        members_count=2,
    )
    resolved = require_field(c_hue_norm, "resolved_icon", expected_type=dict)
    assert require_field(resolved, "name", expected_type=str) == "shield"
    assert require_field(resolved, "hue", expected_type=int) == 215


@e2e_test(
    id="TEST-T2-OB-F17-03",
    tier=2,
    features=["OB-F17"],
    description="Channel timeline forbids 1:1 reactions and pins (reactions_enabled=False, pins_enabled=False, CLI 'not-pinnable-chat').",
)
def test_t2_ob_f17_03_channel_forbids_one_on_one_reactions_and_pins(env: TempOpenBotEnv) -> None:
    a1 = _mk_agent(env, "ChanNoPinLead")
    ch = _mk_channel(env, "NoPinRoom", [a1], lead=a1)
    cid = require_field(ch, "id", expected_type=str)
    posted = env.run_cli_json("channel", "post", "--channel-id", cid, "--text", "Channel message")
    mid = require_field(posted, "message_id", expected_type=str)

    cui = _probe_ui(env, "channel-conversation-recovery", "channel_ui", channel_id=cid)
    assert require_field(cui, "reactions_enabled", expected_type=bool) is False
    assert require_field(cui, "pins_enabled", expected_type=bool) is False

    env.run_cli_expect_rejection(
        "chat",
        "pin",
        "--agent-id",
        a1,
        "--channel-id",
        cid,
        "--message-id",
        mid,
        error_contains=("not-pinnable-chat", "pinnable", "unsupported", "not supported"),
    )
    env.run_cli_expect_rejection(
        "chat",
        "react",
        "--agent-id",
        a1,
        "--channel-id",
        cid,
        "--message-id",
        mid,
        "--emoji",
        "🔥",
        error_contains=("not-supported-in-channel", "unsupported", "not supported"),
    )


@e2e_test(
    id="TEST-T2-OB-F17-04",
    tier=2,
    features=["OB-F17"],
    description="joinRoutingRows deduplicates repeated agentId within an action and does not merge across distinct actions.",
)
def test_t2_ob_f17_04_join_routing_rows_dedup_and_action_boundary(env: TempOpenBotEnv) -> None:
    events = [
        {"action": "assigned", "agent_id": "a1"},
        {"action": "assigned", "agent_id": "a1"},
        {"action": "transferred", "agent_id": "a2"},
    ]
    cui = _probe_ui(env, "channel-conversation-recovery", "channel_ui", routing_events=events)
    merged = extract_list(cui, "merged_routing_rows")
    assert merged == [
        {"action": "assigned", "agent_ids": ["a1"]},
        {"action": "transferred", "agent_ids": ["a2"]},
    ]


@e2e_test(
    id="TEST-T2-OB-F17-05",
    tier=2,
    features=["OB-F17"],
    description="ChannelStoppedTasks Reassign rejects non-member agent ID and excludes current ownerAgentId from candidates.",
)
def test_t2_ob_f17_05_stopped_task_reassign_rejects_non_member_and_excludes_owner(
    env: TempOpenBotEnv,
) -> None:
    a1 = _mk_agent(env, "MemberOwner")
    a2 = _mk_agent(env, "MemberPeer")
    outsider = _mk_agent(env, "OutsiderAgent")
    ch = _mk_channel(env, "GuardedReassignRoom", [a1, a2], lead=a1)
    cid = require_field(ch, "id", expected_type=str)

    post = env.run_cli_json(
        "channel",
        "post",
        "--channel-id",
        cid,
        "--text",
        "Check reassign",
        "--router-mock",
        json.dumps({"agentIds": ["m1"]}),
        "--hold-running",
    )
    tid = require_field(extract_list(post, "tasks")[0], "id", expected_type=str)

    env.run_cli_expect_rejection(
        "channel",
        "reassign",
        "--channel-id",
        cid,
        "--task-id",
        tid,
        "--recipient-agent-id",
        outsider,
        error_contains=("not a member", "non-member", "channel member", "must be a member"),
    )

    cui = _probe_ui(
        env,
        "channel-conversation-recovery",
        "channel_ui",
        channel_id=cid,
        stopped_tasks=[{"task_id": tid, "owner_agent_id": a1}],
    )
    candidates = extract_list(require_field(cui, "stopped_tasks_banner", expected_type=dict), "reassign_candidates")
    assert a1 not in candidates and outsider not in candidates and a2 in candidates


@e2e_test(
    id="TEST-T2-OB-F17-06",
    tier=2,
    features=["OB-F17"],
    description="Archived channel (readOnly) blocks channel post, assign, continue, and reassign without mutating messages/tasks.",
)
def test_t2_ob_f17_06_archived_channel_blocks_mutations_without_side_effects(
    env: TempOpenBotEnv,
) -> None:
    a1 = _mk_agent(env, "ArchOwner")
    a2 = _mk_agent(env, "ArchPeer")
    ch = _mk_channel(env, "FrozenRoom", [a1, a2], lead=a1)
    cid = require_field(ch, "id", expected_type=str)

    post = env.run_cli_json(
        "channel",
        "post",
        "--channel-id",
        cid,
        "--text",
        "Before archive",
        "--router-mock",
        json.dumps({"agentIds": ["m1"]}),
        "--hold-running",
    )
    tid = require_field(extract_list(post, "tasks")[0], "id", expected_type=str)
    mid = require_field(post, "message_id", expected_type=str)

    # Upstream openbot/src/backend/channel-service.ts:L731-L734 (channelArchiveRunning) and
    # TEST-T2-OB-F08-02: archive refuses while a task is running; stop the held task first.
    env.run_cli_expect_error(
        "channel",
        "archive",
        "--channel-id",
        cid,
        error_contains=("running", "stop"),
    )
    stopped = unwrap_entity(
        env.run_cli_json("channel", "stop", "--channel-id", cid, "--task-id", tid),
        "task",
    )
    assert require_field(stopped, "state", expected_type=str) == "paused"

    arch = unwrap_entity(
        env.run_cli_json("channel", "archive", "--channel-id", cid),
        "channel",
    )
    assert require_field(arch, "archived", expected_type=bool) is True
    snap_before = env.run_cli_json("channel", "get", "--channel-id", cid)
    msgs_before = len(extract_list(snap_before, "messages"))
    tasks_before = len(extract_list(snap_before, "tasks"))

    env.run_cli_expect_rejection(
        "channel", "post", "--channel-id", cid, "--text", "Blocked post",
        error_contains=("archived", "read_only", "read-only"),
    )
    env.run_cli_expect_rejection(
        "channel",
        "assign",
        "--channel-id",
        cid,
        "--caller-task-id",
        tid,
        "--agent-id",
        a1,
        "--recipient-agent-id",
        a2,
        "--task",
        "Blocked assign",
        "--source-message-ids",
        mid,
        error_contains=("archived", "read_only", "read-only"),
    )
    env.run_cli_expect_rejection(
        "channel", "continue", "--channel-id", cid, "--task-id", tid,
        error_contains=("archived", "read_only", "read-only"),
    )
    env.run_cli_expect_rejection(
        "channel", "reassign", "--channel-id", cid, "--task-id", tid, "--recipient-agent-id", a2,
        error_contains=("archived", "read_only", "read-only"),
    )

    snap_after = env.run_cli_json("channel", "get", "--channel-id", cid)
    assert len(extract_list(snap_after, "messages")) == msgs_before
    assert len(extract_list(snap_after, "tasks")) == tasks_before


# ============================================================================
# OB-F18 Tier 2: WorkflowGraph Problems, Alpha Fallback, Settings & Shortcuts
# ============================================================================


@e2e_test(
    id="TEST-T2-OB-F18-01",
    tier=2,
    features=["OB-F18"],
    description="WorkflowGraph rejects Annotation edges, Port mismatches (image -> condition), missing ConditionBranch, and self-edges.",
)
def test_t2_ob_f18_01_workflow_graph_annotation_port_branch_and_self_edge_guards(
    env: TempOpenBotEnv,
) -> None:
    aid = _mk_agent(env, "CanvasEdgeGuard")
    bad_graphs = [
        (
            {
                "title": "BadGraph1",
                "nodes": [
                    {"id": "n1", "kind": "note", "at": {"x": 0.0, "y": 0.0}, "fields": {"text": "Memo"}},
                    {"id": "a1", "kind": "agent", "at": {"x": 200.0, "y": 0.0}, "fields": {"agentId": aid, "task": "Run"}},
                ],
                "edges": [{"id": "e1", "from": "n1", "to": "a1", "type": "text"}],
                "strokes": [],
            },
            ("annotation", "port", "cannot connect"),
        ),
        (
            {
                "title": "BadGraph2",
                "nodes": [
                    {"id": "img1", "kind": "image", "at": {"x": 0.0, "y": 0.0}, "fields": {"media": "image", "model": "gpt-image", "prompt": "Card"}},
                    {"id": "c1", "kind": "condition", "at": {"x": 200.0, "y": 0.0}, "fields": {"rule": "ok"}},
                ],
                "edges": [{"id": "e1", "from": "img1", "to": "c1", "type": "image"}],
                "strokes": [],
            },
            ("port", "mismatch", "incompatible"),
        ),
        (
            {
                "title": "BadGraph3",
                "nodes": [
                    {"id": "c1", "kind": "condition", "at": {"x": 0.0, "y": 0.0}, "fields": {"rule": "ok"}},
                    {"id": "o1", "kind": "output", "at": {"x": 200.0, "y": 0.0}, "fields": {"target": "notify", "message": "Done"}},
                ],
                "edges": [{"id": "e1", "from": "c1", "to": "o1", "type": "text"}],
                "strokes": [],
            },
            ("branch", "condition-branch", "condition_branch", "yes or no", "yes/no"),
        ),
        (
            {
                "title": "BadGraph4",
                "nodes": [{"id": "l1", "kind": "llm", "at": {"x": 0.0, "y": 0.0}, "fields": {"prompt": "Step"}}],
                "edges": [{"id": "e1", "from": "l1", "to": "l1", "type": "text"}],
                "strokes": [],
            },
            ("self-edge", "self_edge", "self-loop", "self", "loop", "cycle"),
        ),
    ]
    for graph, errs in bad_graphs:
        env.run_cli_expect_error(
            "platform", "workflow-save", "--spec", json.dumps(graph), error_contains=errs
        )


@e2e_test(
    id="TEST-T2-OB-F18-02",
    tier=2,
    features=["OB-F18"],
    description="WorkflowGraph rejects >200 nodes & >500 strokes and WorkflowCanvas reports SCC back-edges in loop_edges.",
)
def test_t2_ob_f18_02_workflow_graph_caps_and_canvas_loop_edge_detection(env: TempOpenBotEnv) -> None:
    strokes_501 = [
        {
            "id": f"st_{i:x}",
            "brush": "pen",
            "color": "blue",
            "width": 2.0,
            "points": [{"x": 0.0, "y": 0.0}, {"x": 10.0, "y": 10.0}],
        }
        for i in range(501)
    ]
    env.run_cli_expect_error(
        "platform",
        "workflow-save",
        "--spec",
        json.dumps(
            {
                "title": "DrawCap",
                "nodes": [{"id": "t1", "kind": "trigger", "at": {"x": 0.0, "y": 0.0}, "fields": {"trigger": "manual"}}],
                "edges": [],
                "strokes": strokes_501,
            }
        ),
        error_contains=("500", "limit", "maximum", "too many"),
    )

    cyclic_graph = {
        "nodes": [
            {"id": "s1", "kind": "llm", "at": {"x": 100.0, "y": 100.0}, "fields": {"prompt": "Step 1"}},
            {"id": "s2", "kind": "llm", "at": {"x": 360.0, "y": 100.0}, "fields": {"prompt": "Step 2"}},
        ],
        "edges": [
            {"id": "e_fwd", "from": "s1", "to": "s2", "type": "text"},
            {"id": "e_back", "from": "s2", "to": "s1", "type": "text"},
        ],
        "strokes": [],
    }
    pp = _probe_ui(env, "workflow-canvas-pages", "pages_probe", workflow_graph=cyclic_graph)
    wc = require_field(pp, "workflow_canvas", expected_type=dict)
    assert len(extract_list(wc, "loop_edges")) >= 1


@e2e_test(
    id="TEST-T2-OB-F18-03",
    tier=2,
    features=["OB-F18"],
    description="MarketplaceNoMatchPlush swaps fishing.webm to fishing-still.webp when alpha >= 128 or reduced_motion=True.",
)
def test_t2_ob_f18_03_marketplace_no_match_plush_alpha_probe_fallback(env: TempOpenBotEnv) -> None:
    p_trans = _probe_ui(
        env, "workflow-canvas-pages", "pages_probe", marketplace_query="zz_none", video_alpha_pixel=0
    )
    assert require_field(require_field(p_trans, "marketplace", expected_type=dict), "no_match_asset", expected_type=str) == "fishing.webm"

    for alpha_px in (128, 255):
        p_opaque = _probe_ui(
            env, "workflow-canvas-pages", "pages_probe", marketplace_query="zz_none", video_alpha_pixel=alpha_px
        )
        assert require_field(require_field(p_opaque, "marketplace", expected_type=dict), "no_match_asset", expected_type=str) == "fishing-still.webp"

    p_rm = _probe_ui(
        env, "workflow-canvas-pages", "pages_probe", marketplace_query="zz_none", video_alpha_pixel=0, reduced_motion=True
    )
    assert require_field(require_field(p_rm, "marketplace", expected_type=dict), "no_match_asset", expected_type=str) == "fishing-still.webp"


@e2e_test(
    id="TEST-T2-OB-F18-04",
    tier=2,
    features=["OB-F18"],
    description="TasksPanel rejects 51 > 50 per-agent tasks and RoutineCalendar preserves 44-day / 7-day grid across DST transition.",
)
def test_t2_ob_f18_04_tasks_panel_50_cap_and_routine_calendar_dst_grid(env: TempOpenBotEnv) -> None:
    _probe_ui_expect_error(
        env,
        "workflow-canvas-pages",
        ("50", "limit", "maximum", "too many"),
        agent_tasks_count=51,
    )

    dst = _probe_ui(
        env,
        "workflow-canvas-pages",
        "pages_probe",
        calendar_view="week",
        calendar_date="2026-03-29",
        timezone="Europe/Warsaw",
    )
    rc = require_field(dst, "routine_calendar", expected_type=dict)
    assert require_field(rc, "window_days", expected_type=int) == 44
    assert require_field(rc, "visible_days", expected_type=int) == 7


@e2e_test(
    id="TEST-T2-OB-F18-05",
    tier=2,
    features=["OB-F18"],
    description="Detached chat_window=True hides 'dynamic-island' & 'agent-dock' settings tabs (12 tabs), and unknown tab fails cleanly.",
)
def test_t2_ob_f18_05_chat_window_hides_surface_settings_and_unknown_tab_rejected(
    env: TempOpenBotEnv,
) -> None:
    pp = _probe_ui(env, "workflow-canvas-pages", "pages_probe", chat_window=True)
    tabs = extract_list(pp, "app_settings_tabs")
    assert len(tabs) == 12
    assert "dynamic-island" not in tabs and "agent-dock" not in tabs

    _probe_ui_expect_error(
        env,
        "workflow-canvas-pages",
        ("unknown", "unsupported", "invalid", "unrecognized"),
        settings_tab="nonexistent-entry",
    )


@e2e_test(
    id="TEST-T2-OB-F18-06",
    tier=2,
    features=["OB-F18"],
    description="keyboard-shortcuts rejects duplicate chord collisions and forbids overriding FixedShortcut groups.",
)
def test_t2_ob_f18_06_keyboard_shortcuts_collision_and_fixed_override_guards(
    env: TempOpenBotEnv,
) -> None:
    same_chord = {"ctrl": True, "alt": False, "shift": True, "meta": False, "code": "KeyK"}
    r_col = _probe_ui_expect_error(
        env,
        "workflow-canvas-pages",
        ("duplicate", "collision", "conflict", "already bound"),
        shortcut_overrides={"newThread": same_chord, "globalSearch": same_chord},
    )
    assert r_col.returncode == 2

    r_fix = _probe_ui_expect_error(
        env,
        "workflow-canvas-pages",
        ("fixed", "immutable", "reserved", "cannot override", "built-in"),
        shortcut_overrides={"openSettings": same_chord},
    )
    assert r_fix.returncode == 2


# ============================================================================
# OB-F19 Tier 2: DynamicIsland Lock, ChatWindow Auto-Close, Dock/CUA/Recovery
# ============================================================================


@e2e_test(
    id="TEST-T2-OB-F19-01",
    tier=2,
    features=["OB-F19"],
    description="DynamicIsland interactionLocksPresentation holds critical approval and active typing against incoming presentations.",
)
def test_t2_ob_f19_01_dynamic_island_interaction_locks_presentation(env: TempOpenBotEnv) -> None:
    os_crit = _probe_ui(
        env,
        "desktop-os-surfaces",
        "os_surfaces",
        island_current={"mode": "approval", "serverId": "local", "requestId": "r1"},
        island_next={"mode": "question", "serverId": "local", "requestId": "r2"},
        island_interacting=True,
        island_typing=False,
    )
    di = require_field(os_crit, "dynamic_island", expected_type=dict)
    assert require_field(di, "locked", expected_type=bool) is True
    assert require_field(di, "mode", expected_type=str) == "approval"
    assert require_field(di, "queued_mode", expected_type=str) == "question"

    os_type = _probe_ui(
        env,
        "desktop-os-surfaces",
        "os_surfaces",
        island_current={"mode": "message", "serverId": "local", "requestId": "m1"},
        island_next={"mode": "working", "serverId": "local", "requestId": "w2"},
        island_interacting=True,
        island_typing=True,
    )
    di_t = require_field(os_type, "dynamic_island", expected_type=dict)
    assert require_field(di_t, "locked", expected_type=bool) is True
    assert require_field(di_t, "mode", expected_type=str) == "message"


@e2e_test(
    id="TEST-T2-OB-F19-02",
    tier=2,
    features=["OB-F19"],
    description="DynamicIslandPreference rejects out-of-range/non-step-5 sizes (15, 135, 103) and blank or >20,000-char inline replies.",
)
def test_t2_ob_f19_02_dynamic_island_preference_step5_and_reply_bounds(env: TempOpenBotEnv) -> None:
    base_pref = {
        "enabled": True,
        "hapticsEnabled": True,
        "idleVisible": True,
        "additionalDisplaysEnabled": True,
        "serverVisibility": {},
        "newServersVisible": True,
    }
    for bad_dims in (
        {"widthPercent": 15, "heightPercent": 100},
        {"widthPercent": 135, "heightPercent": 100},
        {"widthPercent": 103, "heightPercent": 100},
        {"widthPercent": 100, "heightPercent": 70},
        {"widthPercent": 100, "heightPercent": 130},
    ):
        r_pref = _probe_ui_expect_error(
            env,
            "desktop-os-surfaces",
            ("step", "multiple of 5", "range", "bounds", "between", "invalid"),
            island_preference={**base_pref, **bad_dims},
        )
        assert r_pref.returncode == 2

    for bad_reply in ("   ", "R" * 20_001):
        r_rep = _probe_ui_expect_error(
            env,
            "desktop-os-surfaces",
            ("20000", "empty", "blank", "limit", "length"),
            island_inline_reply=bad_reply,
        )
        assert r_rep.returncode == 2


@e2e_test(
    id="TEST-T2-OB-F19-03",
    tier=2,
    features=["OB-F19"],
    description="Detached ChatWindow auto-closes when its agentId is deleted and enforces minWidth=380px (Main minWidth=960px).",
)
def test_t2_ob_f19_03_detached_chat_window_agent_delete_autoclose_and_min_widths(
    env: TempOpenBotEnv,
) -> None:
    os_s = _probe_ui(
        env,
        "desktop-os-surfaces",
        "os_surfaces",
        chat_window_agent_id="a1",
        delete_chat_window_agent=True,
        chat_window_width_px=240,
        main_width_px=600,
    )
    cw = require_field(os_s, "chat_window", expected_type=dict)
    assert require_field(cw, "auto_closed", expected_type=bool) is True
    assert require_field(cw, "min_width_px", expected_type=int) == 380
    mw = require_field(os_s, "main_window", expected_type=dict)
    assert require_field(mw, "min_width_px", expected_type=int) == 960


@e2e_test(
    id="TEST-T2-OB-F19-04",
    tier=2,
    features=["OB-F19"],
    description="AgentDock rejects 25 > 24 seats and suppresses 900ms auto-tuck while popover/pointer is active.",
)
def test_t2_ob_f19_04_agent_dock_24_seat_cap_and_active_popover_no_tuck(env: TempOpenBotEnv) -> None:
    _probe_ui_expect_error(
        env,
        "desktop-os-surfaces",
        ("24", "limit", "maximum", "too many"),
        dock_seats_count=25,
    )

    os_s = _probe_ui(
        env,
        "desktop-os-surfaces",
        "os_surfaces",
        dock_seats_count=4,
        dock_idle_ms=1500,
        dock_popover_open=True,
    )
    ad = require_field(os_s, "agent_dock", expected_type=dict)
    assert require_field(ad, "tucked", expected_type=bool) is False


@e2e_test(
    id="TEST-T2-OB-F19-05",
    tier=2,
    features=["OB-F19"],
    description="BrowserDock holds secret/approval islands open past 6000ms (needs_answer=True) and CUA hides overlay after 60,000ms idle.",
)
def test_t2_ob_f19_05_browser_dock_needs_answer_hold_and_cua_60s_idle_hide(
    env: TempOpenBotEnv,
) -> None:
    os_s = _probe_ui(
        env,
        "desktop-os-surfaces",
        "os_surfaces",
        browser_dock_island={"kind": "secret"},
        browser_dock_elapsed_ms=15000,
        cua_active=True,
        cua_idle_ms=61000,
    )
    bd = require_field(os_s, "browser_dock", expected_type=dict)
    assert require_field(bd, "needs_answer", expected_type=bool) is True
    assert require_field(bd, "open", expected_type=bool) is True
    cua = require_field(os_s, "cua", expected_type=dict)
    assert require_field(cua, "visible", expected_type=bool) is False


@e2e_test(
    id="TEST-T2-OB-F19-06",
    tier=2,
    features=["OB-F19"],
    description="Recovery does not trigger for <=3 reloads in 60s or spaced crashes, and blocks all non-data: external URLs.",
)
def test_t2_ob_f19_06_recovery_crash_window_thresholds_and_external_url_block(
    env: TempOpenBotEnv,
) -> None:
    few = _probe_ui(
        env, "desktop-os-surfaces", "os_surfaces", crash_timestamps_ms=[1000, 20000, 40000]
    )
    assert require_field(require_field(few, "recovery", expected_type=dict), "triggered", expected_type=bool) is False

    spaced = _probe_ui(
        env, "desktop-os-surfaces", "os_surfaces", crash_timestamps_ms=[1000, 70000, 140000, 210000]
    )
    assert require_field(require_field(spaced, "recovery", expected_type=dict), "triggered", expected_type=bool) is False

    trig = _probe_ui(
        env,
        "desktop-os-surfaces",
        "os_surfaces",
        crash_timestamps_ms=[1000, 10000, 20000, 30000],
        recovery_request_url="https://evil.test/payload.js",
    )
    rec = require_field(trig, "recovery", expected_type=dict)
    assert require_field(rec, "triggered", expected_type=bool) is True
    assert require_field(rec, "external_url_blocked", expected_type=bool) is True


# ============================================================================
# OB-F20 Tier 2: CLI Resilience, Parser Errors, Serve Guards & Baseline Cleanup
# ============================================================================


@e2e_test(
    id="TEST-T2-OB-F20-01",
    tier=2,
    features=["OB-F20"],
    description="openbot ui-probe rejects malformed JSON ('{bad') and unknown scenario ('no-such-probe') with validation_error and zero panics.",
)
def test_t2_ob_f20_01_ui_probe_malformed_json_and_unknown_scenario(env: TempOpenBotEnv) -> None:
    r_bad = env.run_cli_expect_error(
        "ui-probe",
        "--scenario",
        "{bad",
        expected_exit_codes=(2,),
        error_contains=("json", "parse", "malformed", "syntax", "invalid"),
    )
    assert r_bad.returncode == 2

    r_unk = env.run_cli_expect_error(
        "ui-probe",
        "--scenario",
        "no-such-probe",
        expected_exit_codes=(2,),
        error_contains=("unknown", "unsupported", "unrecognized", "invalid"),
    )
    assert r_unk.returncode == 2


@e2e_test(
    id="TEST-T2-OB-F20-02",
    tier=2,
    features=["OB-F20"],
    description="gpui-telegram-cli openbot rejects unknown subcommands, unknown flags, and missing required args with structured usage_error/unknown_command.",
)
def test_t2_ob_f20_02_cli_parser_unknown_subcommand_flag_and_missing_arg(
    env: TempOpenBotEnv,
) -> None:
    r_sub = env.run_cli_expect_error(
        "unknown-sub",
        expected_exit_codes=(2,),
        allow_cli_parse_error=True,
        error_contains=("unknown_command", "usage_error", "unknown", "unrecognized"),
    )
    assert r_sub.returncode == 2

    r_flag = env.run_cli_expect_error(
        "ui-probe",
        "--unknown-flag",
        expected_exit_codes=(2,),
        allow_cli_parse_error=True,
        error_contains=("usage_error", "unknown", "unexpected"),
    )
    assert r_flag.returncode == 2

    r_missing = env.run_cli_expect_error(
        "chat",
        "send",
        expected_exit_codes=(2,),
        allow_cli_parse_error=True,
        error_contains=("usage_error", "missing", "required"),
    )
    assert r_missing.returncode == 2


@e2e_test(
    id="TEST-T2-OB-F20-03",
    tier=2,
    features=["OB-F20"],
    description="Corrupt openbot-*-v1.json preference files degrade gracefully to defaults without corrupting openbot.db.",
)
def test_t2_ob_f20_03_corrupt_preference_json_graceful_fallback(env: TempOpenBotEnv) -> None:
    _mk_agent(env, "IntegritySeedBot")
    (env.data_dir / "openbot-sidebar-v1.json").write_text("{truncated-json", encoding="utf-8")
    (env.data_dir / "openbot-rail-v1.json").write_bytes(b"\x00\xff{corrupt")

    sh = _probe_ui(env, "openbot-shell", "openbot_shell")
    assert require_field(sh, "sidebar_width_px", expected_type=int) == 280
    env.check_sqlite_integrity("openbot")


@e2e_test(
    id="TEST-T2-OB-F20-04",
    tier=2,
    features=["OB-F20"],
    timeout_sec=45.0,
    description="openbot serve enforces loopback 401 bad token, 403 Origin CSRF, 403 Host DNS-rebinding, and 429 on 31st /run call.",
)
def test_t2_ob_f20_04_serve_loopback_csrf_host_rebinding_and_rate_limit(
    env: TempOpenBotEnv,
) -> None:
    aid = _mk_agent(env, "ServeGuardBot", allow_automation="true")
    rt = unwrap_entity(
        env.run_cli_json(
            "platform", "routine-save", "--agent-id", aid, "--name", "ServeRateGuard", "--schedule", "every 180s"
        ),
        "routine",
    )
    rt_id = require_field(rt, "id", expected_type=str)

    srv = env.start_server()
    st_401, _, _ = srv.http_automation("POST", "/run", payload={"agent_id": aid}, token="invalid-token")
    assert st_401 == 401
    st_csrf, _, _ = srv.http_automation(
        "POST", "/run", payload={"agent_id": aid}, include_origin="https://attacker.example"
    )
    assert st_csrf == 403
    st_dns, _, _ = srv.http_automation(
        "POST", "/run", payload={"agent_id": aid}, custom_host="attacker.localhost"
    )
    assert st_dns == 403

    for idx in range(30):
        st_ok, _, _ = srv.automation_run(aid, routine_id=rt_id, payload=f"probe-{idx}")
        assert st_ok in (200, 202)
    st_429, body_429, _ = srv.automation_run(aid, routine_id=rt_id, payload="probe-31")
    assert st_429 == 429
    assert pick(body_429, "limit_per_hour", default=30) == 30


@e2e_test(
    id="TEST-T2-OB-F20-05",
    tier=2,
    features=["OB-F20"],
    description="Deleting an OpenBot Agent or archiving a Channel removes projected Telegram entries while keeping baseline geometry intact.",
)
def test_t2_ob_f20_05_agent_delete_and_channel_archive_cleanup_telegram_projection(
    env: TempOpenBotEnv,
) -> None:
    a_keep = _mk_agent(env, "KeepPeer")
    a_gone = _mk_agent(env, "GonePeer")
    ch = _mk_channel(env, "ArchivedProjRoom", [a_keep], lead=a_keep)
    cid = require_field(ch, "id", expected_type=str)

    env.run_cli_json("agent", "delete", "--id", a_gone)
    env.run_cli_json("channel", "archive", "--channel-id", cid)

    res = env.run_cli_json(
        "ui-probe",
        "--scenario",
        json.dumps({"scenario": "telegram-bidirectional-fusion", "sync_agents_to_telegram": True}),
    )
    bi = unwrap_entity(res, "bidirectional")
    assert require_field(bi, "projected_agent_peers", expected_type=int) == 1
    assert require_field(bi, "projected_channel_peers", expected_type=int) == 0
    mentions_str = " ".join(str(m) for m in extract_list(bi, "mention_candidates"))
    assert "GonePeer" not in mentions_str and a_gone not in mentions_str

    tg = unwrap_entity(res, "telegram_baseline")
    assert require_field(tg, "nav_rail_px", expected_type=int) == 52
    assert require_field(tg, "chat_sidebar_px", expected_type=int) == 288
    assert require_field(tg, "inspector_px", expected_type=int) == 320
    assert extract_list(tg, "composer_min_max_px") == [44, 192]


@e2e_test(
    id="TEST-T2-OB-F20-06",
    tier=2,
    features=["OB-F20"],
    description="Unknown Avatar prefix ('unknown:foo') and 'bloub:goutte' exclusion degrade safely without breaking 8 legacy Grok shapes.",
)
def test_t2_ob_f20_06_avatar_unknown_prefix_and_goutte_exclusion_fallback(
    env: TempOpenBotEnv,
) -> None:
    res = env.run_cli_json(
        "ui-probe",
        "--scenario",
        json.dumps(
            {
                "scenario": "telegram-bidirectional-fusion",
                "avatar_specs": ["unknown:foo", "bloub:goutte", "blob", "teardrop"],
            }
        ),
    )
    bi = unwrap_entity(res, "bidirectional")
    engines = require_field(bi, "avatar_engines", expected_type=dict)
    assert engines.get("blob") == "grok_bot"
    assert engines.get("teardrop") == "grok_bot"
    assert engines.get("unknown:foo") in ("initials", "fallback", "classic")
    assert engines.get("bloub:goutte") == "bloub_flat"
    assert "goutte" not in str(pick(bi, "resolved_goutte_shape", default="capsule")).lower()
