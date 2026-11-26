"""Tier 1 Happy-Path Opaque-Box E2E Tests for OB-F16 through OB-F20 (30 tests, 6/feature).

Covers:
  - OB-F16: Rich @/$ Composer, Steerable QueuePanel, VoicePill & SidePanel
  - OB-F17: Multi-Bot Group-Chat ChannelConversation UI & Loop Recovery
  - OB-F18: WorkflowCanvas, RoutineCalendar, Marketplace, CRM & Settings
  - OB-F19: 7 Desktop OS Surfaces (DynamicIsland, AgentDock, BrowserDock, CUA, Recovery)
  - OB-F20: Dual-Surface Fusion, CLI/Headless Probe & Baseline Preservation
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


def _create_agent(env: TempOpenBotEnv, name: str, **extra_flags: str) -> Dict[str, Any]:
    args: List[str] = ["agent", "create", "--name", name]
    for k, v in extra_flags.items():
        args.extend([f"--{k.replace('_', '-')}", str(v)])
    res = env.run_cli_json(*args)
    return unwrap_entity(res, "agent")


def _create_channel(
    env: TempOpenBotEnv, name: str, members: Sequence[str], **extra_flags: str
) -> Dict[str, Any]:
    args: List[str] = ["channel", "create", "--name", name, "--members", ",".join(members)]
    for k, v in extra_flags.items():
        args.extend([f"--{k.replace('_', '-')}", str(v)])
    res = env.run_cli_json(*args)
    return unwrap_entity(res, "channel")


def _probe_ui(
    env: TempOpenBotEnv, scenario: str, wrapper_key: str, **params: Any
) -> Dict[str, Any]:
    arg = json.dumps({"scenario": scenario, **params}) if params else scenario
    res = env.run_cli_json("ui-probe", "--scenario", arg)
    return unwrap_entity(res, wrapper_key)


# ============================================================================
# OB-F16: Rich @/$ Composer, Steerable QueuePanel, VoicePill & SidePanel (6)
# ============================================================================


@e2e_test(
    id="TEST-T1-OB-F16-01",
    tier=1,
    features=["OB-F16"],
    description="Verifies @ token picker 5 ordered sections, @everyone in channels, and 150ms debounced >=3-char thread search.",
)
def test_t1_ob_f16_01_composer_at_picker_sections_and_debounce(env: TempOpenBotEnv) -> None:
    cp_search = _probe_ui(
        env, "composer-queue-sidepanel", "composer_probe", trigger="@", query="auth", in_channel=True
    )
    assert extract_list(cp_search, "at_sections") == ["agents", "files", "tabs", "threads", "messages"]
    assert require_field(cp_search, "messages_debounce_ms", expected_type=int) == 150
    assert require_field(cp_search, "messages_min_query", expected_type=int) == 3

    cp_ev = _probe_ui(
        env, "composer-queue-sidepanel", "composer_probe", trigger="@", query="ev", in_channel=True
    )
    assert require_field(cp_ev, "everyone_shown", expected_type=bool) is True


@e2e_test(
    id="TEST-T1-OB-F16-02",
    tier=1,
    features=["OB-F16"],
    description="Verifies $ token picker ranking Skills and enabled MCP servers with duplicate-name slug disambiguation and skillProblems.",
)
def test_t1_ob_f16_02_composer_dollar_picker_skills_mcp_and_problems(env: TempOpenBotEnv) -> None:
    skills = [
        {"slug": "git-helper-core", "name": "Git Helper", "status": "ok"},
        {"slug": "git-helper-pro", "name": "Git Helper", "status": "ok"},
        {"slug": "git-broken", "name": "Git Broken", "status": "needs-repair"},
    ]
    mcps = [{"id": "mcp-gh", "name": "GitHub MCP", "enabled": True}]
    cp = _probe_ui(
        env,
        "composer-queue-sidepanel",
        "composer_probe",
        trigger="$",
        query="git",
        skills=skills,
        mcp_servers=mcps,
    )
    items = extract_list(cp, "dollar_items")
    assert len(items) >= 3
    kinds = {str(pick(it, "kind", "type")) for it in items if isinstance(it, dict)}
    assert {"skill", "mcp"}.issubset(kinds)
    problems = extract_list(cp, "skill_problems")
    assert len(problems) >= 1


@e2e_test(
    id="TEST-T1-OB-F16-03",
    tier=1,
    features=["OB-F16"],
    description="Verifies >4,000-char paste-to-file (pasted-text.txt), valid attachments, and pi text-only custom model image 1-click fix.",
)
def test_t1_ob_f16_03_composer_paste_to_file_and_image_model_fix(env: TempOpenBotEnv) -> None:
    cp = _probe_ui(
        env,
        "composer-queue-sidepanel",
        "composer_probe",
        paste_length=4500,
        custom_pi_text_only=True,
        attachments=[{"name": "diagram.png", "bytes": 4096, "kind": "image"}],
        allow_images_click=True,
    )
    assert require_field(cp, "pasted_as_file", expected_type=bool) is True
    assert require_field(cp, "pasted_file_name", expected_type=str) == "pasted-text.txt"
    assert require_field(cp, "image_model_warning", expected_type=bool) is True
    assert require_field(cp, "image_inputs_enabled", expected_type=bool) is True


@e2e_test(
    id="TEST-T1-OB-F16-04",
    tier=1,
    features=["OB-F16"],
    description="Verifies QueuePanel FIFO queue-add, queue-reorder, queue-pause/resume, and queue-steer into active user turn.",
)
def test_t1_ob_f16_04_queue_panel_reorder_pause_resume_and_steer(env: TempOpenBotEnv) -> None:
    ag = _create_agent(env, "QueueSteerBot")
    aid = require_field(ag, "id", expected_type=str)

    paused = env.run_cli_json("chat", "queue-pause", "--agent-id", aid)
    assert require_field(paused, "paused", expected_type=bool) is True

    d1 = require_field(
        env.run_cli_json("chat", "queue-add", "--agent-id", aid, "--text", "First queued"),
        "delivery_id",
        expected_type=str,
    )
    d2 = require_field(
        env.run_cli_json("chat", "queue-add", "--agent-id", aid, "--text", "Second queued"),
        "delivery_id",
        expected_type=str,
    )
    d3 = require_field(
        env.run_cli_json("chat", "queue-add", "--agent-id", aid, "--text", "Third queued"),
        "delivery_id",
        expected_type=str,
    )

    reordered = env.run_cli_json(
        "chat", "queue-reorder", "--agent-id", aid, "--delivery-ids", f"{d3},{d1},{d2}"
    )
    q_items = extract_list(reordered, "items", "queue")
    assert [require_field(it, "delivery_id", expected_type=str) for it in q_items[:3]] == [d3, d1, d2]

    resumed = env.run_cli_json("chat", "queue-resume", "--agent-id", aid)
    assert require_field(resumed, "paused", expected_type=bool) is False
    turn_id = require_field(resumed, "active_turn_id", "turn_id", expected_type=str)

    steered = env.run_cli_json(
        "chat",
        "queue-steer",
        "--agent-id",
        aid,
        "--delivery-id",
        d1,
        "--expected-turn-id",
        turn_id,
    )
    assert require_field(steered, "steered", expected_type=bool) is True


@e2e_test(
    id="TEST-T1-OB-F16-05",
    tier=1,
    features=["OB-F16"],
    description="Verifies duplex VoicePill across phases (starting..offline), open vs push talk modes, and 12 waveform bars.",
)
def test_t1_ob_f16_05_duplex_voice_pill_phases_and_talk_modes(env: TempOpenBotEnv) -> None:
    cp_push = _probe_ui(
        env,
        "composer-queue-sidepanel",
        "composer_probe",
        voice_pill={"phase": "listening", "talk_mode": "push"},
    )
    vp_push = require_field(cp_push, "voice_pill", expected_type=dict)
    assert require_field(vp_push, "bars", expected_type=int) == 12
    assert require_field(vp_push, "phase", expected_type=str) == "listening"
    assert require_field(vp_push, "talk_mode", expected_type=str) == "push"

    cp_speak = _probe_ui(
        env,
        "composer-queue-sidepanel",
        "composer_probe",
        voice_pill={"phase": "speaking", "talk_mode": "open"},
    )
    vp_speak = require_field(cp_speak, "voice_pill", expected_type=dict)
    assert require_field(vp_speak, "phase", expected_type=str) == "speaking"
    assert require_field(vp_speak, "talk_mode", expected_type=str) == "open"
    assert require_field(vp_speak, "live", expected_type=bool) is True


@e2e_test(
    id="TEST-T1-OB-F16-06",
    tier=1,
    features=["OB-F16"],
    description="Verifies right SidePanel (360px default, 6 panes, MAX_SIDE_SPLITS=2 vertical split with 120px/160px mins, and maximize).",
)
def test_t1_ob_f16_06_right_side_panel_panes_splits_and_maximize(env: TempOpenBotEnv) -> None:
    panes = ["browser", "files", "threads", "settings", "question", "subagent"]
    cp = _probe_ui(
        env,
        "composer-queue-sidepanel",
        "composer_probe",
        side_panel_panes=panes,
        side_splits=["browser", "files"],
        split_share=0.5,
        maximize_side_panel=True,
    )
    sp = require_field(cp, "side_panel", expected_type=dict)
    assert require_field(sp, "default_width_px", expected_type=int) == 360
    assert extract_list(sp, "available_panes") == panes
    assert extract_list(sp, "active_splits") == ["browser", "files"]
    assert require_field(sp, "max_splits", expected_type=int) == 2
    assert require_field(sp, "split_min_px", expected_type=int) == 120
    assert require_field(sp, "main_group_min_px", expected_type=int) == 160
    assert require_field(sp, "maximized", expected_type=bool) is True


# ============================================================================
# OB-F17: Multi-Bot ChannelConversation UI & Loop Recovery (6 Tests)
# ============================================================================


@e2e_test(
    id="TEST-T1-OB-F17-01",
    tier=1,
    features=["OB-F17"],
    description="Verifies ChannelEditor leadAgentId Crown toggle, 20 CHANNEL_ICONS + custom hue, browser policy, and isolated toggle.",
)
def test_t1_ob_f17_01_channel_editor_crown_icons_and_isolated_toggle(env: TempOpenBotEnv) -> None:
    a1 = require_field(_create_agent(env, "CrownLead"), "id", expected_type=str)
    a2 = require_field(_create_agent(env, "PeerB"), "id", expected_type=str)
    a3 = require_field(_create_agent(env, "PeerC"), "id", expected_type=str)
    ch = _create_channel(
        env,
        "DesignSync",
        [a1, a2, a3],
        lead=a1,
        icon=json.dumps({"name": "workflow", "hue": 280}),
        isolated="true",
        browser=json.dumps({"kind": "members"}),
    )
    cid = require_field(ch, "id", expected_type=str)
    assert require_field(ch, "lead_agent_id", expected_type=str) == a1
    assert require_field(ch, "isolated", expected_type=bool) is True
    assert require_field(ch, "icon", expected_type=dict) == {"name": "workflow", "hue": 280}

    cui = _probe_ui(
        env,
        "channel-conversation-recovery",
        "channel_ui",
        channel_id=cid,
        lead_agent_id=a1,
        icon={"name": "workflow", "hue": 280},
    )
    assert require_field(cui, "icon_catalog_count", expected_type=int) == 20
    assert require_field(cui, "lead_crown_agent_id", expected_type=str) == a1


@e2e_test(
    id="TEST-T1-OB-F17-02",
    tier=1,
    features=["OB-F17"],
    description="Verifies ChannelAvatar header row (<=3 faces) vs sidebar cluster (5..12 -> 3 + '+N', >=13 -> 2 + '+N').",
)
def test_t1_ob_f17_02_channel_avatar_header_row_and_cluster_pills(env: TempOpenBotEnv) -> None:
    c6 = _probe_ui(env, "channel-conversation-recovery", "channel_ui", members_count=6)
    assert require_field(c6, "header_row_faces", expected_type=int) == 3
    assert require_field(c6, "cluster_faces", expected_type=int) == 3
    assert require_field(c6, "cluster_plus_pill", expected_type=str) == "+3"

    c14 = _probe_ui(env, "channel-conversation-recovery", "channel_ui", members_count=14)
    assert require_field(c14, "header_row_faces", expected_type=int) == 3
    assert require_field(c14, "cluster_faces", expected_type=int) == 2
    assert require_field(c14, "cluster_plus_pill", expected_type=str) == "+12"


@e2e_test(
    id="TEST-T1-OB-F17-03",
    tier=1,
    features=["OB-F17"],
    description="Verifies joinRoutingRows merging consecutive channel-routing markers with identical action.",
)
def test_t1_ob_f17_03_channel_routing_marker_row_merging(env: TempOpenBotEnv) -> None:
    events = [
        {"action": "assigned", "agent_id": "a1"},
        {"action": "assigned", "agent_id": "a2"},
        {"action": "transferred", "agent_id": "a3"},
    ]
    cui = _probe_ui(
        env, "channel-conversation-recovery", "channel_ui", routing_events=events
    )
    merged = extract_list(cui, "merged_routing_rows")
    assert len(merged) == 2
    assert merged[0] == {"action": "assigned", "agent_ids": ["a1", "a2"]}
    assert merged[1] == {"action": "transferred", "agent_ids": ["a3"]}


@e2e_test(
    id="TEST-T1-OB-F17-04",
    tier=1,
    features=["OB-F17"],
    description="Verifies ChannelActivityIndicator (-8px face overlap, leaving transition) and AwaitingReplies progress bar.",
)
def test_t1_ob_f17_04_channel_activity_indicator_and_awaiting_replies(env: TempOpenBotEnv) -> None:
    cui = _probe_ui(
        env,
        "channel-conversation-recovery",
        "channel_ui",
        working_ids=["a1", "a2"],
        leaving_ids=["a3"],
        archived=False,
    )
    act = require_field(cui, "activity_indicator", expected_type=dict)
    assert require_field(act, "overlap_px", expected_type=int) == -8
    assert require_field(act, "active_faces", expected_type=int) == 2
    assert require_field(act, "leaving_faces", expected_type=int) == 1
    assert require_field(cui, "awaiting_replies_shown", expected_type=bool) is True


@e2e_test(
    id="TEST-T1-OB-F17-05",
    tier=1,
    features=["OB-F17"],
    description="Verifies ChannelStoppedTasks loop recovery (Continue, Reassign to non-owner member, Cancel) end-to-end.",
)
def test_t1_ob_f17_05_channel_stopped_tasks_continue_reassign_cancel(env: TempOpenBotEnv) -> None:
    a1 = require_field(_create_agent(env, "OwnerBot"), "id", expected_type=str)
    a2 = require_field(_create_agent(env, "RescueBot"), "id", expected_type=str)
    a3 = require_field(_create_agent(env, "ThirdBot"), "id", expected_type=str)
    ch = _create_channel(env, "RecoveryRoom", [a1, a2, a3], lead=a1)
    cid = require_field(ch, "id", expected_type=str)

    post = env.run_cli_json(
        "channel",
        "post",
        "--channel-id",
        cid,
        "--text",
        "Investigate deadlock",
        "--router-mock",
        json.dumps({"agentIds": ["m1"]}),
        "--hold-running",
    )
    msg_id = require_field(post, "message_id", expected_type=str)
    task0 = extract_list(post, "tasks")[0]
    tid = require_field(task0, "id", expected_type=str)

    cur_owner = a1
    for step in range(8):
        nxt_owner = a2 if cur_owner == a1 else a1
        tr = unwrap_entity(
            env.run_cli_json(
                "channel",
                "transfer",
                "--channel-id",
                cid,
                "--task-id",
                tid,
                "--agent-id",
                cur_owner,
                "--recipient-agent-id",
                nxt_owner,
                "--task",
                f"Handoff {step + 1}",
                "--source-message-ids",
                msg_id,
            ),
            "task",
        )
        cur_owner = require_field(tr, "owner_agent_id", expected_type=str)

    env.run_cli_expect_rejection(
        "channel",
        "transfer",
        "--channel-id",
        cid,
        "--task-id",
        tid,
        "--agent-id",
        cur_owner,
        "--recipient-agent-id",
        (a2 if cur_owner == a1 else a1),
        "--task",
        "Handoff 9 pause",
        "--source-message-ids",
        msg_id,
        error_contains=("assignment", "8", "limit", "paused"),
    )

    cui = _probe_ui(
        env,
        "channel-conversation-recovery",
        "channel_ui",
        channel_id=cid,
        stopped_tasks=[{"task_id": tid, "owner_agent_id": cur_owner}],
    )
    banner = require_field(cui, "stopped_tasks_banner", expected_type=dict)
    assert extract_list(banner, "actions") == ["Continue", "Reassign", "Cancel"]
    candidates = extract_list(banner, "reassign_candidates")
    assert cur_owner not in candidates and a3 in candidates

    cont = unwrap_entity(
        env.run_cli_json("channel", "continue", "--channel-id", cid, "--task-id", tid), "task"
    )
    assert require_field(cont, "state", expected_type=str) in ("queued", "running")

    reassigned = unwrap_entity(
        env.run_cli_json(
            "channel", "reassign", "--channel-id", cid, "--task-id", tid, "--recipient-agent-id", a3
        ),
        "task",
    )
    assert require_field(reassigned, "owner_agent_id", expected_type=str) == a3

    # Upstream openbot/src/backend/channel-service.ts:L995-L1030: continue/reassign move the
    # task to queued/running; user cancel strictly requires a stopped (paused|failed) task first.
    env.run_cli_expect_rejection(
        "channel",
        "cancel",
        "--channel-id",
        cid,
        "--task-id",
        tid,
        error_contains=("stopped", "paused", "failed"),
    )

    stopped = unwrap_entity(
        env.run_cli_json("channel", "stop", "--channel-id", cid, "--task-id", tid), "task"
    )
    assert require_field(stopped, "state", expected_type=str) == "paused"

    cancelled = unwrap_entity(
        env.run_cli_json("channel", "cancel", "--channel-id", cid, "--task-id", tid), "task"
    )
    assert require_field(cancelled, "state", expected_type=str) == "cancelled"


@e2e_test(
    id="TEST-T1-OB-F17-06",
    tier=1,
    features=["OB-F17"],
    description="Verifies ChannelSettingsPane archive read-only state (hiding AwaitingReplies) and restore resumption.",
)
def test_t1_ob_f17_06_channel_archive_read_only_and_restore(env: TempOpenBotEnv) -> None:
    a1 = require_field(_create_agent(env, "ArchLead"), "id", expected_type=str)
    ch = _create_channel(env, "ArchiveRoom", [a1], lead=a1)
    cid = require_field(ch, "id", expected_type=str)

    arch = unwrap_entity(env.run_cli_json("channel", "archive", "--channel-id", cid), "channel")
    assert require_field(arch, "archived", expected_type=bool) is True

    cui_arch = _probe_ui(
        env, "channel-conversation-recovery", "channel_ui", channel_id=cid, archived=True
    )
    assert require_field(cui_arch, "read_only", expected_type=bool) is True
    assert require_field(cui_arch, "awaiting_replies_shown", expected_type=bool) is False

    rest = unwrap_entity(env.run_cli_json("channel", "restore", "--channel-id", cid), "channel")
    assert require_field(rest, "archived", expected_type=bool) is False

    post = env.run_cli_json("channel", "post", "--channel-id", cid, "--text", "Back online")
    assert len(require_field(post, "message_id", expected_type=str)) >= 4


# ============================================================================
# OB-F18: WorkflowCanvas, RoutineCalendar, Marketplace, CRM & Settings (6)
# ============================================================================


@e2e_test(
    id="TEST-T1-OB-F18-01",
    tier=1,
    features=["OB-F18"],
    description="Verifies WorkflowCanvas all 12 WorkflowNodeKinds (9 step + 3 annotation), minimap, tidy layout, and .openbot-workflow.json export.",
)
def test_t1_ob_f18_01_workflow_canvas_twelve_kinds_tidy_and_export(env: TempOpenBotEnv) -> None:
    pp = _probe_ui(
        env, "workflow-canvas-pages", "pages_probe", tidy_layout=True, export_workflow=True
    )
    wc = require_field(pp, "workflow_canvas", expected_type=dict)
    kinds = extract_list(wc, "node_kinds")
    assert len(kinds) == 12
    assert set(kinds) == {
        "trigger", "prompt", "asset", "llm", "agent", "image",
        "condition", "approval", "output", "text", "note", "shape",
    }
    assert require_field(wc, "step_kinds_count", expected_type=int) == 9
    assert extract_list(wc, "annotation_kinds") == ["text", "note", "shape"]
    assert require_field(wc, "minimap", expected_type=bool) is True
    assert require_field(wc, "hud", expected_type=bool) is True
    assert require_field(wc, "tidy_applied", expected_type=bool) is True
    assert require_field(wc, "exported_format", expected_type=str) == "openbot.workflow"
    assert require_field(wc, "exported_version", expected_type=int) == 1


@e2e_test(
    id="TEST-T1-OB-F18-02",
    tier=1,
    features=["OB-F18"],
    description="Verifies RoutineCalendar 44-day window across week/month/day views and TasksPanel 50-task per-agent limit.",
)
def test_t1_ob_f18_02_routine_calendar_views_and_tasks_panel(env: TempOpenBotEnv) -> None:
    wk = _probe_ui(
        env, "workflow-canvas-pages", "pages_probe", calendar_view="week", agent_tasks_count=12
    )
    rc_wk = require_field(wk, "routine_calendar", expected_type=dict)
    assert require_field(rc_wk, "window_days", expected_type=int) == 44
    assert require_field(rc_wk, "visible_days", expected_type=int) == 7
    assert require_field(rc_wk, "day_cell_limit", expected_type=int) == 4
    assert require_field(rc_wk, "upcoming_limit", expected_type=int) == 6
    tp = require_field(wk, "tasks_panel", expected_type=dict)
    assert require_field(tp, "max_per_agent", expected_type=int) == 50

    mo = _probe_ui(env, "workflow-canvas-pages", "pages_probe", calendar_view="month")
    assert require_field(require_field(mo, "routine_calendar", expected_type=dict), "day_cell_limit", expected_type=int) == 3

    dy = _probe_ui(env, "workflow-canvas-pages", "pages_probe", calendar_view="day")
    assert require_field(require_field(dy, "routine_calendar", expected_type=dict), "visible_days", expected_type=int) == 1


@e2e_test(
    id="TEST-T1-OB-F18-03",
    tier=1,
    features=["OB-F18"],
    description="Verifies LibraryPanel mosaic/list views with marquee selection and Marketplace 3-tab surface.",
)
def test_t1_ob_f18_03_library_panel_and_marketplace_surface(env: TempOpenBotEnv) -> None:
    mp_cli = unwrap_entity(
        env.run_cli_json("platform", "marketplace-search", "--tab", "skills", "--query", "git"),
        "marketplace",
    )
    assert require_field(mp_cli, "tab", expected_type=str) == "skills"

    pp = _probe_ui(env, "workflow-canvas-pages", "pages_probe")
    lib = require_field(pp, "library_panel", expected_type=dict)
    assert extract_list(lib, "views") == ["mosaic", "list"]
    assert require_field(lib, "marquee_selection", expected_type=bool) is True
    mp = require_field(pp, "marketplace", expected_type=dict)
    assert set(extract_list(mp, "tabs")) == {"agents", "skills", "apps"}


@e2e_test(
    id="TEST-T1-OB-F18-04",
    tier=1,
    features=["OB-F18"],
    description="Verifies CrmPage 13 SQLite tables synchronization and Usage/Credits page probe.",
)
def test_t1_ob_f18_04_crm_page_and_usage_credits_surface(env: TempOpenBotEnv) -> None:
    sync_res = unwrap_entity(
        env.run_cli_json("platform", "crm-sync", "--source", "stripe"), "crm"
    )
    assert require_field(sync_res, "tables_count", expected_type=int) == 13

    pp = _probe_ui(env, "workflow-canvas-pages", "pages_probe")
    crm_page = require_field(pp, "crm_page", expected_type=dict)
    assert require_field(crm_page, "tables_count", expected_type=int) == 13


@e2e_test(
    id="TEST-T1-OB-F18-05",
    tier=1,
    features=["OB-F18"],
    description="Verifies SettingsModal 14 visible App Settings tabs ('hosted-servers' -> 'billing') and 18 ServerSettings sections.",
)
def test_t1_ob_f18_05_app_settings_14_tabs_and_server_settings_18_sections(env: TempOpenBotEnv) -> None:
    pp = _probe_ui(
        env,
        "workflow-canvas-pages",
        "pages_probe",
        settings_tab="hosted-servers",
        chat_window=False,
    )
    app_tabs = extract_list(pp, "app_settings_tabs")
    assert len(app_tabs) == 14
    assert "hosted-servers" not in app_tabs
    assert require_field(pp, "resolved_settings_tab", expected_type=str) == "billing"
    srv_secs = extract_list(pp, "server_settings_sections")
    assert len(srv_secs) == 18


@e2e_test(
    id="TEST-T1-OB-F18-06",
    tier=1,
    features=["OB-F18"],
    description="Verifies 11 configurable SHORTCUT_COMMANDS, 4 FixedShortcut groups on linux vs darwin, and custom chord override.",
)
def test_t1_ob_f18_06_keyboard_shortcuts_platform_defaults_and_override(env: TempOpenBotEnv) -> None:
    overrides = {
        "globalSearch": {"ctrl": True, "alt": True, "shift": False, "meta": False, "code": "KeyP"}
    }
    pp = _probe_ui(
        env,
        "workflow-canvas-pages",
        "pages_probe",
        platform="linux",
        shortcut_overrides=overrides,
    )
    sc = require_field(pp, "shortcuts", expected_type=dict)
    assert require_field(sc, "configurable_count", expected_type=int) == 11
    assert require_field(sc, "fixed_count", expected_type=int) == 4
    resolved = require_field(sc, "resolved", expected_type=dict)
    assert require_field(resolved["toggleSidebar"], "ctrl", expected_type=bool) is True
    assert require_field(resolved["globalSearch"], "code", expected_type=str) == "KeyP"


# ============================================================================
# OB-F19: 7 Desktop OS Surfaces (DynamicIsland, AgentDock, BrowserDock, CUA)
# ============================================================================


@e2e_test(
    id="TEST-T1-OB-F19-01",
    tier=1,
    features=["OB-F19"],
    description="Verifies Main window (min 960x640, #0b0d0e, <721px Linux auto-zoom, close-while-working guard) and detached ChatWindow.",
)
def test_t1_ob_f19_01_main_window_and_detached_chat_window_surfaces(env: TempOpenBotEnv) -> None:
    os_s = _probe_ui(
        env,
        "desktop-os-surfaces",
        "os_surfaces",
        main_width_px=700,
        working_agents_count=2,
        attempt_close_main=True,
        chat_window_agent_id="a1",
    )
    assert extract_list(os_s, "surfaces") == [
        "Main", "ChatWindow", "DynamicIsland", "AgentDock", "BrowserDock", "CUA", "Recovery"
    ]
    mw = require_field(os_s, "main_window", expected_type=dict)
    assert require_field(mw, "min_width_px", expected_type=int) == 960
    assert require_field(mw, "min_height_px", expected_type=int) == 640
    assert require_field(mw, "bg_hex", expected_type=str).lower() == "#0b0d0e"
    assert require_field(mw, "auto_zoomed", expected_type=bool) is True
    assert require_field(mw, "close_guard_prompted", expected_type=bool) is True
    assert extract_list(mw, "close_guard_buttons") == ["Keep Running", "Quit", "Cancel"]

    cw = require_field(os_s, "chat_window", expected_type=dict)
    assert require_field(cw, "min_width_px", expected_type=int) == 380
    assert require_field(cw, "rails_hidden", expected_type=bool) is True
    assert require_field(cw, "sidebar_hidden", expected_type=bool) is True


@e2e_test(
    id="TEST-T1-OB-F19-02",
    tier=1,
    features=["OB-F19"],
    description="Verifies DynamicIsland 192x32 compact notch, spring morph ([0.5,0.8]/[0.45,1.0]), priority 0 question mode, and inline reply.",
)
def test_t1_ob_f19_02_dynamic_island_geometry_spring_and_priority(env: TempOpenBotEnv) -> None:
    os_s = _probe_ui(
        env,
        "desktop-os-surfaces",
        "os_surfaces",
        island_current={"mode": "question", "serverId": "local", "requestId": "r1"},
        island_inline_reply="Ship it now",
    )
    di = require_field(os_s, "dynamic_island", expected_type=dict)
    assert require_field(di, "width_px", expected_type=int) == 192
    assert require_field(di, "height_px", expected_type=int) == 32
    assert require_field(di, "priority", expected_type=int) == 0
    assert extract_list(di, "spring_open") == [0.5, 0.8]
    assert extract_list(di, "spring_close") == [0.45, 1.0]


@e2e_test(
    id="TEST-T1-OB-F19-03",
    tier=1,
    features=["OB-F19"],
    description="Verifies DynamicIsland critical prompt preemption unlock when active approval resolves (heldRequestEnded -> working).",
)
def test_t1_ob_f19_03_dynamic_island_resolved_approval_unlocks(env: TempOpenBotEnv) -> None:
    os_s = _probe_ui(
        env,
        "desktop-os-surfaces",
        "os_surfaces",
        island_current={"mode": "approval", "serverId": "local", "requestId": "r1"},
        island_next={"mode": "working", "serverId": "local", "resolvedRequestId": "r1"},
        island_interacting=True,
        island_typing=False,
    )
    di = require_field(os_s, "dynamic_island", expected_type=dict)
    assert require_field(di, "locked", expected_type=bool) is False
    assert require_field(di, "mode", expected_type=str) == "working"


@e2e_test(
    id="TEST-T1-OB-F19-04",
    tier=1,
    features=["OB-F19"],
    description="Verifies AgentDock (<=24 seats, 900ms auto-tuck into 4px edge strip at 950ms vs untucked at 400ms).",
)
def test_t1_ob_f19_04_agent_dock_seats_and_900ms_edge_strip_tuck(env: TempOpenBotEnv) -> None:
    active = _probe_ui(
        env, "desktop-os-surfaces", "os_surfaces", dock_seats_count=6, dock_idle_ms=400
    )
    ad_active = require_field(active, "agent_dock", expected_type=dict)
    assert require_field(ad_active, "max_seats", expected_type=int) == 24
    assert require_field(ad_active, "tuck_delay_ms", expected_type=int) == 900
    assert require_field(ad_active, "strip_px", expected_type=int) == 4
    assert require_field(ad_active, "tucked", expected_type=bool) is False

    tucked = _probe_ui(
        env, "desktop-os-surfaces", "os_surfaces", dock_seats_count=6, dock_idle_ms=950
    )
    ad_tucked = require_field(tucked, "agent_dock", expected_type=dict)
    assert require_field(ad_tucked, "tucked", expected_type=bool) is True


@e2e_test(
    id="TEST-T1-OB-F19-05",
    tier=1,
    features=["OB-F19"],
    description="Verifies BrowserDock 6000ms auto-close on plain reply and CUA 33ms poll / 12px border glow / virtual cursor.",
)
def test_t1_ob_f19_05_browser_dock_autoclose_and_cua_highlight_cursor(env: TempOpenBotEnv) -> None:
    os_s = _probe_ui(
        env,
        "desktop-os-surfaces",
        "os_surfaces",
        browser_dock_island={"kind": "reply", "question": False},
        browser_dock_elapsed_ms=6100,
        cua_active=True,
    )
    bd = require_field(os_s, "browser_dock", expected_type=dict)
    assert require_field(bd, "reply_close_ms", expected_type=int) == 6000
    assert require_field(bd, "open", expected_type=bool) is False

    cua = require_field(os_s, "cua", expected_type=dict)
    assert require_field(cua, "poll_ms", expected_type=int) == 33
    assert require_field(cua, "corner_radius_px", expected_type=int) == 12
    assert require_field(cua, "visible", expected_type=bool) is True
    assert require_field(cua, "cursor_active", expected_type=bool) is True


@e2e_test(
    id="TEST-T1-OB-F19-06",
    tier=1,
    features=["OB-F19"],
    description="Verifies Recovery surface (cloe static 9-slice plush SVG and 6 recovery actions) on >=3 rapid crashes.",
)
def test_t1_ob_f19_06_recovery_surface_cloe_plush_and_six_actions(env: TempOpenBotEnv) -> None:
    os_s = _probe_ui(
        env,
        "desktop-os-surfaces",
        "os_surfaces",
        crash_timestamps_ms=[1000, 15000, 30000, 45000],
    )
    rec = require_field(os_s, "recovery", expected_type=dict)
    assert require_field(rec, "triggered", expected_type=bool) is True
    assert require_field(rec, "plush_character", expected_type=str) == "cloe"
    assert extract_list(rec, "actions") == [
        "retry",
        "previous-version",
        "latest-version",
        "export-diagnostics",
        "open-data-folder",
        "quit",
    ]


# ============================================================================
# OB-F20: Dual-Surface Fusion, CLI/Headless Probe & Baseline Preservation (6)
# ============================================================================


@e2e_test(
    id="TEST-T1-OB-F20-01",
    tier=1,
    features=["OB-F20"],
    description="Verifies ActiveWorkspacePanel::OpenBot (Cmd+0 / Cmd+Shift+O) while preserving Cmd+1..9 Telegram destinations.",
)
def test_t1_ob_f20_01_workspace_openbot_shortcut_and_cmd_1_to_9_preserved(env: TempOpenBotEnv) -> None:
    expected_1_to_9 = [
        "Chats", "Folders", "Calls", "TaskCenter", "McpTools",
        "TalkMode", "SavedMessages", "Settings", "Browser",
    ]
    for sc in ("Cmd+0", "Cmd+Shift+O"):
        res = env.run_cli_json(
            "ui-probe", "--scenario", json.dumps({"scenario": "workspace-routing", "shortcut": sc})
        )
        assert require_field(res, "active_panel", expected_type=str) == "OpenBot"
        assert require_field(res, "nav_destination", expected_type=str) == "OpenBot"
        tg = unwrap_entity(res, "telegram_baseline")
        assert extract_list(tg, "cmd_1_to_9") == expected_1_to_9
        assert require_field(tg, "shortcuts_preserved", expected_type=bool) is True

    for idx, dest in enumerate(expected_1_to_9, 1):
        res_n = env.run_cli_json(
            "ui-probe",
            "--scenario",
            json.dumps({"scenario": "workspace-routing", "shortcut": f"Cmd+{idx}"}),
        )
        assert require_field(res_n, "nav_destination", expected_type=str) == dest


@e2e_test(
    id="TEST-T1-OB-F20-02",
    tier=1,
    features=["OB-F20"],
    description="Verifies repeated Chats -> OpenBot -> Chats switches preserve exact 52/288/62/160..430/320/44..192px Telegram geometry.",
)
def test_t1_ob_f20_02_workspace_switch_preserves_telegram_geometry(env: TempOpenBotEnv) -> None:
    res = env.run_cli_json(
        "ui-probe",
        "--scenario",
        json.dumps(
            {
                "scenario": "workspace-routing",
                "switch_sequence": ["Chats", "OpenBot", "Chats", "OpenBot", "Chats"],
            }
        ),
    )
    assert require_field(res, "active_panel", expected_type=str) == "Chats"
    tg = unwrap_entity(res, "telegram_baseline")
    assert require_field(tg, "nav_rail_px", expected_type=int) == 52
    assert require_field(tg, "chat_sidebar_px", expected_type=int) == 288
    assert require_field(tg, "sidebar_row_px", expected_type=int) == 62
    assert extract_list(tg, "bubble_min_max_px") == [160, 430]
    assert require_field(tg, "inspector_px", expected_type=int) == 320
    assert extract_list(tg, "composer_min_max_px") == [44, 192]


@e2e_test(
    id="TEST-T1-OB-F20-03",
    tier=1,
    features=["OB-F20"],
    description="Verifies OpenBot Agents and Shared Channels project into Telegram ChatSidebar, ConversationStage, and @mention candidates.",
)
def test_t1_ob_f20_03_bidirectional_telegram_peer_and_mention_projection(env: TempOpenBotEnv) -> None:
    a1 = require_field(_create_agent(env, "FusionAlpha"), "id", expected_type=str)
    a2 = require_field(_create_agent(env, "FusionBeta"), "id", expected_type=str)
    _create_channel(env, "FusionSquad", [a1, a2], lead=a1)

    res = env.run_cli_json(
        "ui-probe",
        "--scenario",
        json.dumps({"scenario": "telegram-bidirectional-fusion", "sync_agents_to_telegram": True}),
    )
    bi = unwrap_entity(res, "bidirectional")
    assert require_field(bi, "projected_agent_peers", expected_type=int) >= 2
    assert require_field(bi, "projected_channel_peers", expected_type=int) >= 1
    mentions = [str(m) for m in extract_list(bi, "mention_candidates")]
    joined_mentions = " ".join(mentions)
    assert ("FusionAlpha" in joined_mentions or a1 in joined_mentions)
    assert ("FusionBeta" in joined_mentions or a2 in joined_mentions)


@e2e_test(
    id="TEST-T1-OB-F20-04",
    tier=1,
    features=["OB-F20"],
    description="Verifies unified Avatar delegation across bloub:*, plush:*, 'classic', and all 8 legacy Grok shapes.",
)
def test_t1_ob_f20_04_unified_avatar_delegation_bloub_plush_and_grok(env: TempOpenBotEnv) -> None:
    grok_shapes = ["blob", "pebble", "squircle", "tablet", "wedge", "hex", "cloud", "teardrop"]
    specs = ["classic", *grok_shapes, "bloub:agent-1", "plush:agent-1"]
    res = env.run_cli_json(
        "ui-probe",
        "--scenario",
        json.dumps({"scenario": "telegram-bidirectional-fusion", "avatar_specs": specs}),
    )
    bi = unwrap_entity(res, "bidirectional")
    engines = require_field(bi, "avatar_engines", expected_type=dict)
    assert engines.get("classic") == "initials"
    for gs in grok_shapes:
        assert engines.get(gs) == "grok_bot", f"Expected grok_bot for {gs}, got {engines.get(gs)}"
    assert engines.get("bloub:agent-1") == "bloub_flat"
    assert engines.get("plush:agent-1") == "bloub_plush"


@e2e_test(
    id="TEST-T1-OB-F20-05",
    tier=1,
    features=["OB-F20"],
    description="Verifies TdesktopThemeMode (Night -> Tinted -> Day -> Classic) and TdesktopThemePalette isolation from --openbot-* tokens.",
)
def test_t1_ob_f20_05_tdesktop_theme_palette_isolation(env: TempOpenBotEnv) -> None:
    env.run_cli_json("theme-eval", "--mode", "dark", "--backdrop", "coast", "--persist")
    res = env.run_cli_json(
        "ui-probe",
        "--scenario",
        json.dumps({"scenario": "workspace-switch-theme-isolation", "tdesktop_mode": "Night"}),
    )
    tg = unwrap_entity(res, "telegram_baseline")
    assert require_field(tg, "palette_untouched", expected_type=bool) is True
    assert extract_list(tg, "tdesktop_cycle") == ["Night", "Tinted", "Day", "Classic"]


@e2e_test(
    id="TEST-T1-OB-F20-06",
    tier=1,
    features=["OB-F20"],
    description="Verifies live openbot serve (/v1/health, POST /v1/agents, /v1/events SSE) and ui-probe state coherence on shared data_dir.",
)
def test_t1_ob_f20_06_live_serve_sse_and_ui_probe_shared_state(env: TempOpenBotEnv) -> None:
    srv = env.start_server()
    with srv.open_sse("/v1/events") as sse:
        sse.read_until(lambda e: e.event == "ready", timeout=5.0)
        st_h, body_h, _ = srv.http_team("GET", "/v1/health")
        assert st_h == 200
        assert require_field(body_h, "schema_version", expected_type=int) == 69

        st_a, body_a, _ = srv.http_team("POST", "/v1/agents", payload={"name": "SharedCoherentBot"})
        assert st_a in (200, 201)
        aid = require_field(unwrap_entity(body_a, "agent"), "id", "agent_id", expected_type=str)
        ev = sse.read_until(lambda e: e.event == "agent.created", timeout=5.0)
        assert aid in ev.data

    sh = _probe_ui(env, "openbot-shell", "openbot_shell")
    assert require_field(sh, "sidebar_width_px", expected_type=int) == 280
    ag_rows = env.query_sqlite("SELECT * FROM projection_agents")
    assert any((r.get("agent_id") or r.get("id")) == aid for r in ag_rows)
