"""Tier 4 Real-World Multi-Step Operational Scenario E2E Tests (12 scenarios, TEST-T4-SCENARIO-01..12).

End-to-end user & multi-bot journeys exercising all 20 OpenBot features (OB-F01..OB-F20):
  01. Multi-Bot Engineering Sprint Channel, Delegation, channel_reply, 9th-Handoff Stop & Recovery
  02. Live /v1/* Team Host + Real-Time /v1/events SSE + Symmetric Team DMs + Channel & Mailbox Sync
  03. Incident Triage: HMAC Webhook -> 12-Node Approval Workflow -> DynamicIsland Lock -> Browser Vault & Takeover
  04. Revenue Operations: Stripe CRM Sync -> Human-Gated crm-propose -> HTML Dashboard -> Static Site Publish
  05. Dual-Surface Seamless Fusion: Chats <-> OpenBot, Telegram Bridge, Bloub/Plush Avatars & Baseline Geometry
  06. Onboarding to Portable Multi-Host Migration: AI Profile Gen, Skills, MCP, CJK/Site Memory, Export/Import & Failover
  07. Secure E-Commerce Procurement: Marketplace, Site Memory, link-payment, fill-card Lock & Post-Nav Receipt
  08. Deep Research & Long-Context Synthesis: Side Threads, Steerable Queue, side-question, Pin Guard & 80% Compaction
  09. Multimodal Voice Studio & Live Theme Customization: Local Model Probe, STT/TTS/WebRTC, Credit Media & CrewPile
  10. Scheduled & Loopback DevOps Mesh: Quiet Routines, /run CSRF/Auth/allowAutomation/30-hr Cap & Multi-Bridge Alerts
  11. Organization-Wide @everyone Review Channel: 8-Bot Cluster, Parallel Limit=2, Sibling Hiding, Delta Packet & Archive
  12. Security, SQLite Authorizer, Privilege-Confinement & Crash-Safety Audit Across All 7 OS Surfaces
"""

from __future__ import annotations

import http.server
import json
import math
import os
import threading
import time
from pathlib import Path
from typing import Any, Dict, List, Optional, Sequence, Tuple

from .harness import (
    BUILTIN_AGENT_PROVIDERS,
    TempOpenBotEnv,
    channel_cluster_layout_oracle,
    dm_thread_id_oracle,
    e2e_test,
    extract_list,
    oklab_readable_chat_bg_oracle,
    pick,
    plush_srgb_tint_pixel_oracle,
    require_field,
    unwrap_entity,
    webhook_hmac_sha256_oracle,
)

_EXACT_12_WORKFLOW_NODE_KINDS: Tuple[str, ...] = (
    "trigger",
    "prompt",
    "asset",
    "llm",
    "agent",
    "image",
    "condition",
    "approval",
    "output",
    "text",
    "note",
    "shape",
)


def _mk_agent(env: TempOpenBotEnv, name: str, **flags: str) -> Dict[str, Any]:
    args: List[str] = ["agent", "create", "--name", name]
    for k, v in flags.items():
        args.extend([f"--{k.replace('_', '-')}", str(v)])
    res = env.run_cli_json(*args)
    return unwrap_entity(res, "agent")


def _mk_channel(
    env: TempOpenBotEnv, name: str, members: Sequence[str], **flags: str
) -> Dict[str, Any]:
    args: List[str] = ["channel", "create", "--name", name, "--members", ",".join(members)]
    for k, v in flags.items():
        args.extend([f"--{k.replace('_', '-')}", str(v)])
    res = env.run_cli_json(*args)
    return unwrap_entity(res, "channel")


def _start_local_models_server(models: List[str]) -> Tuple[http.server.HTTPServer, str]:
    payload = json.dumps(
        {"object": "list", "data": [{"id": m, "object": "model"} for m in models]}
    ).encode("utf-8")

    class _ModelsHandler(http.server.BaseHTTPRequestHandler):
        def do_GET(self) -> None:  # noqa: N802
            self.send_response(200)
            self.send_header("Content-Type", "application/json")
            self.send_header("Content-Length", str(len(payload)))
            self.end_headers()
            self.wfile.write(payload)

        def log_message(self, fmt: str, *args: Any) -> None:
            return

    srv = http.server.HTTPServer(("127.0.0.1", 0), _ModelsHandler)
    port = srv.server_address[1]
    threading.Thread(target=srv.serve_forever, daemon=True).start()
    return srv, f"http://127.0.0.1:{port}/v1"


def _build_12_node_workflow(
    agent_id: str,
    channel_id: Optional[str] = None,
    title: str = "Incident Triage 12-Node DAG",
) -> Dict[str, Any]:
    out_fields: Dict[str, Any] = {
        "target": "channel" if channel_id else "notify",
        "message": "Incident mitigation approved",
        "stepName": "Dispatch Output",
    }
    if channel_id:
        out_fields["channelId"] = channel_id
    nodes: List[Dict[str, Any]] = [
        {"id": "n-trig", "kind": "trigger", "at": {"x": 80.0, "y": 100.0}, "size": {"width": 220.0, "height": 120.0}, "fields": {"trigger": "webhook", "stepName": "Alert Trigger"}},
        {"id": "n-prompt", "kind": "prompt", "at": {"x": 80.0, "y": 260.0}, "size": {"width": 220.0, "height": 120.0}, "fields": {"text": "Analyze PagerAlert payload", "stepName": "Triage Prompt"}},
        {"id": "n-asset", "kind": "asset", "at": {"x": 80.0, "y": 420.0}, "size": {"width": 220.0, "height": 120.0}, "fields": {"fileId": "asset-sre-playbook", "stepName": "SRE Playbook"}},
        {"id": "n-llm", "kind": "llm", "at": {"x": 360.0, "y": 180.0}, "size": {"width": 240.0, "height": 140.0}, "fields": {"provider": "codex", "model": "gpt-6-luna", "prompt": "Score blast radius", "stepName": "Classify"}},
        {"id": "n-agent", "kind": "agent", "at": {"x": 640.0, "y": 180.0}, "size": {"width": 240.0, "height": 140.0}, "fields": {"agentId": agent_id, "task": "Prepare mitigation steps", "stepName": "OnCall SRE"}},
        {"id": "n-image", "kind": "image", "at": {"x": 640.0, "y": 380.0}, "size": {"width": 240.0, "height": 140.0}, "fields": {"media": "image", "model": "gpt-image", "prompt": "Status banner", "stepName": "Banner"}},
        {"id": "n-cond", "kind": "condition", "at": {"x": 920.0, "y": 180.0}, "size": {"width": 220.0, "height": 120.0}, "fields": {"rule": "severity == critical", "stepName": "Critical Gate"}},
        {"id": "n-appr", "kind": "approval", "at": {"x": 1180.0, "y": 120.0}, "size": {"width": 220.0, "height": 120.0}, "fields": {"question": "Approve production failover?", "stepName": "Human Approval"}},
        {"id": "n-out", "kind": "output", "at": {"x": 1460.0, "y": 200.0}, "size": {"width": 220.0, "height": 120.0}, "fields": out_fields},
        {"id": "n-text", "kind": "text", "at": {"x": 80.0, "y": 20.0}, "size": {"width": 320.0, "height": 48.0}, "fields": {"text": "SRE Incident Triage Pipeline"}},
        {"id": "n-note", "kind": "note", "at": {"x": 1180.0, "y": 320.0}, "size": {"width": 240.0, "height": 120.0}, "fields": {"text": "Require human sign-off on failover"}},
        {"id": "n-shape", "kind": "shape", "at": {"x": 900.0, "y": 80.0}, "size": {"width": 540.0, "height": 220.0}, "fields": {"text": "Approval Boundary"}},
    ]
    edges: List[Dict[str, Any]] = [
        {"id": "e1", "from": "n-trig", "to": "n-llm", "type": "text"},
        {"id": "e2", "from": "n-prompt", "to": "n-llm", "type": "text"},
        {"id": "e3", "from": "n-asset", "to": "n-agent", "type": "file"},
        {"id": "e4", "from": "n-llm", "to": "n-agent", "type": "text"},
        {"id": "e5", "from": "n-agent", "to": "n-image", "type": "text"},
        {"id": "e6", "from": "n-agent", "to": "n-cond", "type": "text"},
        {"id": "e7", "from": "n-cond", "to": "n-appr", "type": "text", "branch": "yes"},
        {"id": "e8", "from": "n-cond", "to": "n-out", "type": "text", "branch": "no"},
        {"id": "e9", "from": "n-appr", "to": "n-out", "type": "text"},
        {"id": "e10", "from": "n-image", "to": "n-out", "type": "image"},
    ]
    return {"title": title, "nodes": nodes, "edges": edges, "strokes": []}


# ============================================================================
# Scenarios TEST-T4-SCENARIO-01 .. TEST-T4-SCENARIO-02
# ============================================================================


@e2e_test(
    id="TEST-T4-SCENARIO-01",
    tier=4,
    features=["OB-F02", "OB-F06", "OB-F08", "OB-F09", "OB-F10", "OB-F17"],
    timeout_sec=60.0,
    description="Multi-Bot Engineering Sprint Channel: Lead routing, child assign + agent-data.db metrics, channel_reply, 9th-handoff stop, and reassign recovery.",
)
def test_t4_scenario_01_engineering_sprint_channel_delegation_and_loop_recovery(env: TempOpenBotEnv) -> None:
    lead = require_field(_mk_agent(env, "SprintLead", provider="codex"), "id", expected_type=str)
    backend = require_field(_mk_agent(env, "BackendRust", provider="claude"), "id", expected_type=str)
    sec = require_field(_mk_agent(env, "SecAudit", provider="grok"), "id", expected_type=str)
    rel = require_field(_mk_agent(env, "ReleaseBot", provider="deepseek"), "id", expected_type=str)

    ch = _mk_channel(env, "RustSprint", [lead, backend, sec, rel], lead=lead, isolated="true")
    cid = require_field(ch, "id", expected_type=str)

    epic = env.run_cli_json(
        "channel",
        "post",
        "--channel-id",
        cid,
        "--text",
        "Verify SQLite v69 WAL benchmarks and ship sprint candidate",
        "--router-mock",
        json.dumps({"agentIds": ["m1"]}),
    )
    root_t = extract_list(epic, "tasks")[0]
    t_lead = require_field(root_t, "id", expected_type=str)
    src_mid = require_field(root_t, "request_message_id", expected_type=str)
    assert require_field(root_t, "owner_agent_id", expected_type=str) == lead

    asg = env.run_cli_json(
        "channel",
        "assign",
        "--channel-id",
        cid,
        "--caller-task-id",
        t_lead,
        "--agent-id",
        lead,
        "--recipient-agent-id",
        backend,
        "--task",
        "Benchmark WAL throughput and store p99 latency",
        "--expected-result",
        "Benchmark summary",
        "--source-message-ids",
        src_mid,
    )
    t_child = require_field(asg, "child_task_id", "task.id", expected_type=str)
    assert require_field(asg, "parent_state", expected_type=str) == "waiting"

    env.run_cli_json(
        "tools",
        "agent-data-exec",
        "--agent-id",
        backend,
        "--mode",
        "write",
        "--sql",
        "CREATE TABLE sprint_bench (metric TEXT PRIMARY KEY, val INT)",
    )
    env.run_cli_json(
        "tools",
        "agent-data-exec",
        "--agent-id",
        backend,
        "--mode",
        "write",
        "--sql",
        "INSERT INTO sprint_bench VALUES (?, ?)",
        "--params",
        json.dumps(["wal_p99_us", 420]),
    )
    child_res = env.run_cli_json(
        "channel",
        "result",
        "--channel-id",
        cid,
        "--task-id",
        t_child,
        "--agent-id",
        backend,
        "--text",
        "**BackendRust report:** WAL v69 verified at 420us",
    )
    backend_mid = require_field(child_res, "message_id", "id", expected_type=str)
    assert require_field(child_res, "text", expected_type=str) == "WAL v69 verified at 420us"
    assert require_field(child_res, "released_parent_state", expected_type=str) == "queued"

    rep = env.run_cli_json(
        "channel",
        "reply",
        "--channel-id",
        cid,
        "--task-id",
        t_lead,
        "--agent-id",
        lead,
        "--message-id",
        backend_mid,
        "--text",
        "Confirm synchronous=NORMAL was active?",
    )
    assert require_field(rep, "reply_message_id", expected_type=str).startswith("channel-reply-")
    assert require_field(rep, "is_reply_task", expected_type=bool) is True

    env.run_cli_json(
        "channel",
        "result",
        "--channel-id",
        cid,
        "--task-id",
        t_lead,
        "--agent-id",
        lead,
        "--text",
        "Epic phase 1 signed off",
    )

    loop_post = env.run_cli_json("channel", "post", "--channel-id", cid, "--text", "@SecAudit Fuzz auth")
    t_loop = extract_list(loop_post, "tasks")[0]
    tid_loop = require_field(t_loop, "id", expected_type=str)
    req_loop = require_field(t_loop, "request_message_id", expected_type=str)
    cur = sec
    for step in range(1, 9):
        nxt = backend if cur == sec else sec
        xf = unwrap_entity(
            env.run_cli_json(
                "channel",
                "transfer",
                "--channel-id",
                cid,
                "--task-id",
                tid_loop,
                "--agent-id",
                cur,
                "--recipient-agent-id",
                nxt,
                "--task",
                f"Fuzz pass {step}",
                "--source-message-ids",
                req_loop,
            ),
            "task",
        )
        assert require_field(xf, "assignment_count", expected_type=int) == step
        cur = nxt

    env.run_cli_expect_rejection(
        "channel",
        "transfer",
        "--channel-id",
        cid,
        "--task-id",
        tid_loop,
        "--agent-id",
        cur,
        "--recipient-agent-id",
        backend if cur == sec else sec,
        "--task",
        "Fuzz pass 9",
        "--source-message-ids",
        req_loop,
        error_contains=("assignment limit", "continue or reassign", "8", "paused"),
    )

    cui = unwrap_entity(
        env.run_cli_json(
            "ui-probe",
            "--scenario",
            json.dumps({"scenario": "channel-conversation-recovery", "channel_id": cid, "stopped_tasks": [{"id": tid_loop, "owner_agent_id": cur}]}),
        ),
        "channel_ui",
    )
    assert extract_list(cui, "stopped_tasks_banner.actions") == ["Continue", "Reassign", "Cancel"]

    recovered = unwrap_entity(
        env.run_cli_json(
            "channel", "reassign", "--channel-id", cid, "--task-id", tid_loop, "--recipient-agent-id", rel
        ),
        "task",
    )
    assert require_field(recovered, "owner_agent_id", expected_type=str) == rel
    assert require_field(recovered, "assignment_count", expected_type=int) == 0
    env.run_cli_json(
        "channel", "result", "--channel-id", cid, "--task-id", tid_loop, "--agent-id", rel, "--text", "Fuzz audit green"
    )
    env.check_sqlite_integrity("openbot")
    env.check_sqlite_integrity("agent_data")


@e2e_test(
    id="TEST-T4-SCENARIO-02",
    tier=4,
    features=["OB-F01", "OB-F02", "OB-F07", "OB-F08", "OB-F13", "OB-F20"],
    timeout_sec=60.0,
    description="Live /v1/* Team Host, /v1/events SSE stream, symmetric dm-<sha256> DMs, multi-bot Channel coordination, and Mailbox Status/Result/Evidence sync.",
)
def test_t4_scenario_02_live_team_host_sse_dms_channel_and_mailbox_sync(env: TempOpenBotEnv) -> None:
    lead = require_field(_mk_agent(env, "HostLead"), "id", expected_type=str)
    worker1 = require_field(_mk_agent(env, "HostWorkerA"), "id", expected_type=str)
    worker2 = require_field(_mk_agent(env, "HostWorkerB"), "id", expected_type=str)
    ch = _mk_channel(env, "LiveTeamRoom", [lead, worker1, worker2], lead=lead)
    cid = require_field(ch, "id", expected_type=str)

    srv = env.start_server()
    st_h, body_h, _ = srv.http_team("GET", "/v1/health")
    assert st_h == 200 and require_field(body_h, "schema_version", expected_type=int) == 69

    st_cap, body_cap, _ = srv.http_team("GET", "/v1/capabilities")
    assert st_cap == 200
    assert set(extract_list(body_cap, "providers")) == set(BUILTIN_AGENT_PROVIDERS)
    assert len(extract_list(body_cap, "capabilities")) >= 220

    with srv.open_sse("/v1/events") as sse:
        ev_ready = sse.read_until(lambda e: e.event == "ready", timeout=6.0)
        assert ev_ready.event == "ready"

        st_ca, body_ca, _ = srv.http_team(
            "POST", "/v1/agents", payload={"name": "RemoteJoinedBot", "provider": "claude"}
        )
        assert st_ca in (200, 201)
        remote_aid = require_field(
            unwrap_entity(body_ca, "agent"), "id", "agent_id", expected_type=str
        )
        ev_ag = sse.read_until(lambda e: e.event == "agent.created", timeout=6.0)
        assert ev_ag.event == "agent.created" and remote_aid in ev_ag.data

        st_cc, body_cc, _ = srv.http_team(
            "POST",
            "/v1/channels",
            payload={"name": "RemoteOpsRoom", "members": [lead, worker1], "leadAgentId": lead},
        )
        assert st_cc in (200, 201)
        remote_cid = require_field(
            unwrap_entity(body_cc, "channel"), "id", "channel_id", expected_type=str
        )
        ev_ch = sse.read_until(
            lambda e: e.event in ("channel.created", "channel.updated"), timeout=6.0
        )
        assert remote_cid in ev_ch.data

        env.run_cli_json(
            "channel", "post", "--channel-id", remote_cid, "--text", "@HostLead Remote ops SSE ping"
        )
        ev_msg = sse.read_until(lambda e: e.event == "message.posted", timeout=6.0)
        assert "Remote ops SSE ping" in ev_msg.data or remote_cid in ev_msg.data

        st_la, ag_list_http, _ = srv.http_team("GET", "/v1/agents")
        st_lc, ch_list_http, _ = srv.http_team("GET", "/v1/channels")
        assert st_la == 200 and len(extract_list(ag_list_http, "agents")) >= 4
        ch_ids_http = {
            pick(c, "id", "channel_id") for c in extract_list(ch_list_http, "channels")
        }
        assert st_lc == 200 and {cid, remote_cid}.issubset(ch_ids_http)

    ag_rows = env.query_sqlite("SELECT * FROM projection_agents")
    ch_rows = env.query_sqlite("SELECT * FROM projection_channels")
    assert {lead, worker1, worker2}.issubset({str(r.get("agent_id") or r.get("id")) for r in ag_rows})
    assert cid in {str(r.get("channel_id") or r.get("id")) for r in ch_rows}

    dm_a = env.run_cli_json(
        "chat", "dm-send", "--sender", "alice", "--recipient", "bob", "--client-message-id", "sc02-dm1", "--text", "Kick off sync"
    )
    dm_b = env.run_cli_json(
        "chat", "dm-send", "--sender", "bob", "--recipient", "alice", "--client-message-id", "sc02-dm2", "--text", "Sync green"
    )
    exp_dm = dm_thread_id_oracle("alice", "bob")
    assert require_field(dm_a, "thread_id", expected_type=str) == exp_dm
    assert require_field(dm_b, "thread_id", expected_type=str) == exp_dm

    mb_fan = env.run_cli_json(
        "chat",
        "mailbox-send",
        "--sender-agent-id",
        lead,
        "--recipient-agent-ids",
        f"{worker1},{worker2}",
        "--text",
        "Report cluster health",
        "--expects-reply",
        "true",
    )
    fan_mid = require_field(mb_fan, "message_id", expected_type=str)
    mb_ans = env.run_cli_json(
        "chat",
        "mailbox-send",
        "--sender-agent-id",
        worker1,
        "--recipient-agent-ids",
        lead,
        "--text",
        "Status: done\nResult: All shards synced\nEvidence: /v1/health 200",
        "--reply-to",
        fan_mid,
        "--expects-reply",
        "false",
    )
    assert require_field(mb_ans, "answer_optional", expected_type=bool) is True

    post_ch = env.run_cli_json("channel", "post", "--channel-id", cid, "--text", "@HostLead Publish rollout status")
    tid = require_field(extract_list(post_ch, "tasks")[0], "id", expected_type=str)
    env.run_cli_json(
        "channel", "result", "--channel-id", cid, "--task-id", tid, "--agent-id", lead, "--text", "Rollout status green"
    )
    env.check_sqlite_integrity("openbot")


@e2e_test(
    id="TEST-T4-SCENARIO-03",
    tier=4,
    features=["OB-F10", "OB-F11", "OB-F15", "OB-F18", "OB-F19"],
    timeout_sec=60.0,
    description="Full-Stack Incident Triage: HMAC webhook routine -> 12-node approval workflow -> DynamicIsland critical lock -> Browser Vault & human takeover.",
)
def test_t4_scenario_03_incident_triage_webhook_12_node_workflow_island_and_takeover(
    env: TempOpenBotEnv,
) -> None:
    sre_id = require_field(_mk_agent(env, "OnCallSRE"), "id", expected_type=str)
    wh_secret = "sre_wh_03_hmac_key"
    rt = env.run_cli_json(
        "platform",
        "routine-save",
        "--agent-id",
        sre_id,
        "--name",
        "PagerAlert",
        "--schedule",
        "webhook",
        "--webhook-secret",
        wh_secret,
    )
    rid = require_field(rt, "id", "routine_id", expected_type=str)

    ts_now = int(time.time())
    delivery_id = "deliv-sc03-001"
    alert_json = json.dumps({"service": "api-edge", "severity": "critical"}, separators=(",", ":"))
    sig = "sha256=" + webhook_hmac_sha256_oracle(wh_secret, f"{ts_now}.{delivery_id}.{alert_json}")
    wh = env.run_cli_json(
        "platform",
        "webhook-post",
        "--routine-id",
        rid,
        "--payload",
        alert_json,
        "--signature",
        sig,
        "--timestamp",
        str(ts_now),
        "--delivery-id",
        delivery_id,
    )
    assert require_field(wh, "webhook_verified", expected_type=bool) is True

    wf_spec = _build_12_node_workflow(sre_id, title="Critical Edge Failover")
    assert {n["kind"] for n in wf_spec["nodes"]} == set(_EXACT_12_WORKFLOW_NODE_KINDS)
    wf_saved = unwrap_entity(
        env.run_cli_json("platform", "workflow-save", "--spec", json.dumps(wf_spec)),
        "workflow",
    )
    wf_id = require_field(wf_saved, "id", "workflow_id", expected_type=str)
    assert require_field(wf_saved, "node_count", expected_type=int) == 12
    assert require_field(wf_saved, "step_count", expected_type=int) == 9

    wf_hold = env.run_cli_json(
        "platform",
        "workflow-run",
        "--workflow-id",
        wf_id,
        "--trigger-payload",
        alert_json,
        "--condition-mock",
        json.dumps({"n-cond": "yes"}),
    )
    assert require_field(wf_hold, "status", expected_type=str) in ("awaiting_approval", "approval")

    os_lock = unwrap_entity(
        env.run_cli_json(
            "ui-probe",
            "--scenario",
            json.dumps(
                {
                    "scenario": "desktop-os-surfaces",
                    "island_current": {"mode": "approval", "serverId": "local", "requestId": wf_id},
                    "island_next": {"mode": "working", "serverId": "other"},
                    "island_interacting": True,
                }
            ),
        ),
        "os_surfaces",
        "openbot_shell",
    )
    di = require_field(os_lock, "dynamic_island", expected_type=dict)
    assert require_field(di, "width_px", expected_type=int) == 192
    assert require_field(di, "height_px", expected_type=int) == 32
    assert require_field(di, "locked", "critical_lock", expected_type=bool) is True

    wf_done = env.run_cli_json(
        "platform", "workflow-run", "--workflow-id", wf_id, "--approve-node", "n-appr"
    )
    assert require_field(wf_done, "status", expected_type=str) in ("completed", "done")

    br = env.run_cli_json(
        "tools",
        "browser-run",
        "--agent-id",
        sre_id,
        "--url",
        "https://status.edge.test/admin",
        "--steps",
        json.dumps([{"action": "snapshot"}]),
    )
    tab_id = require_field(br, "tab_id", expected_type=str)
    vs = env.run_cli_json(
        "tools",
        "vault-secret",
        "--agent-id",
        sre_id,
        "--tab-id",
        tab_id,
        "--method",
        "password",
        "--login-id",
        "sre-edge-admin",
        "--submission",
        "enter",
    )
    assert require_field(vs, "secret_document", expected_type=bool) is True
    tk = env.run_cli_json(
        "tools", "takeover", "--agent-id", sre_id, "--tab-id", tab_id, "--reason", "hardware-fido2"
    )
    assert require_field(tk, "takeover_active", expected_type=bool) is True


@e2e_test(
    id="TEST-T4-SCENARIO-04",
    tier=4,
    features=["OB-F02", "OB-F10", "OB-F12", "OB-F15", "OB-F18"],
    timeout_sec=60.0,
    description="Revenue Operations Pipeline: 13-table Stripe CRM sync, human-gated crm-propose, agent-data.db ARR rollup, sandboxed HTML report, and 30-day site.",
)
def test_t4_scenario_04_revops_crm_sync_proposal_html_dashboard_and_site_publish(
    env: TempOpenBotEnv,
) -> None:
    aid = require_field(_mk_agent(env, "RevOpsBot"), "id", expected_type=str)
    fixture = json.dumps(
        {
            "customers": [{"id": "cus_sc04_1", "name": "Helios AI", "email": "ap@helios.test", "currency": "USD"}],
            "subscriptions": [{"id": "sub_sc04_1", "customer": "cus_sc04_1", "plan": "Scale", "amount_minor": 240000, "every": "year"}],
            "invoices": [{"id": "in_sc04_1", "customer": "cus_sc04_1", "amount_minor": 240000, "paid": True}],
        }
    )
    sync = env.run_cli_json("platform", "crm-sync", "--source", "stripe", "--fixture", fixture)
    assert require_field(sync, "tables_count", expected_type=int) == 13
    assert len(env.query_sqlite("SELECT name FROM sqlite_master WHERE type='table' AND name LIKE 'crm_%'")) == 13

    prop_body = json.dumps(
        {"name": "Helios Expansion", "currency": "USD", "domain": "helios.test", "billingEmail": "cfo@helios.test", "statusId": "customer"}
    )
    prop = env.run_cli_json(
        "platform", "crm-propose", "--agent-id", aid, "--operation", "customer-create", "--payload", prop_body
    )
    pid = require_field(prop, "proposal_id", "id", expected_type=str)
    assert require_field(prop, "status", expected_type=str) in ("pending_approval", "pending")

    env.run_cli_expect_rejection(
        "platform",
        "crm-propose",
        "--agent-id",
        aid,
        "--operation",
        "customer-create",
        "--payload",
        prop_body,
        "--commit-unapproved",
        error_contains=("pending approval", "human approval", "approval", "person"),
    )
    committed = env.run_cli_json(
        "platform",
        "crm-propose",
        "--agent-id",
        aid,
        "--proposal-id",
        pid,
        "--operation",
        "customer-create",
        "--payload",
        prop_body,
        "--approve",
    )
    assert require_field(committed, "status", expected_type=str) in ("committed", "approved")

    env.run_cli_json(
        "tools", "agent-data-exec", "--agent-id", aid, "--mode", "write", "--sql", "CREATE TABLE arr_rollup (quarter TEXT PRIMARY KEY, cents INT)"
    )
    env.run_cli_json(
        "tools",
        "agent-data-exec",
        "--agent-id",
        aid,
        "--mode",
        "write",
        "--sql",
        "INSERT INTO arr_rollup VALUES (?, ?)",
        "--params",
        json.dumps(["2026-Q4", 240000]),
    )

    html = env.run_cli_json(
        "platform", "html-render", "--mode", "page", "--html", "<main><h2>Q4 ARR $2,400</h2></main>", "--backdrop", "pond"
    )
    assert require_field(html, "sandboxed", expected_type=bool) is True
    assert require_field(html, "csp_enforced", expected_type=bool) is True
    assert require_field(html, "openbot_page", expected_type=bool) is True

    site = env.run_cli_json(
        "platform",
        "site-publish",
        "--agent-id",
        aid,
        "--slug",
        "q4-arr-portal",
        "--files",
        json.dumps([{"path": "index.html", "content": "<!doctype html><h1>Q4 ARR</h1>"}]),
    )
    assert require_field(site, "expires_in_days", expected_type=int) == 30

    pp = unwrap_entity(env.run_cli_json("ui-probe", "--scenario", "workflow-canvas-pages"), "pages_probe")
    assert require_field(pp, "crm_page.tables_count", expected_type=int) == 13
    env.check_sqlite_integrity("openbot")
    env.check_sqlite_integrity("agent_data")


@e2e_test(
    id="TEST-T4-SCENARIO-05",
    tier=4,
    features=["OB-F01", "OB-F03", "OB-F04", "OB-F05", "OB-F13", "OB-F14", "OB-F20"],
    timeout_sec=60.0,
    description="Dual-Surface Seamless Fusion: Chats <-> OpenBot workspace switches with Plush/Flat avatars, sunset backdrop, Telegram bridge, and 100% baseline.",
)
def test_t4_scenario_05_dual_surface_fusion_avatars_theme_bridge_and_baseline(
    env: TempOpenBotEnv,
) -> None:
    plush_ag = _mk_agent(
        env, "PlushFusionBot", avatar_seed="fusion:poulpe:face:70", avatar_hue="185"
    )
    flat_ag = _mk_agent(env, "FlatFusionBot", avatar_seed="helper:flat:23")
    plush_id = require_field(plush_ag, "id", expected_type=str)

    av_plush = env.run_cli_json(
        "avatar-eval", "--seed", "fusion:poulpe:face:70", "--hue", "185", "--style", "plush"
    )
    ramp = require_field(av_plush, "tint_ramp", expected_type=dict)
    assert tuple(int(x) for x in ramp["140"]) == plush_srgb_tint_pixel_oracle("#2fbfa0", 140)

    av_flat = env.run_cli_json("avatar-eval", "--seed", "helper:flat:23", "--style", "flat")
    assert require_field(av_flat, "rays_count", expected_type=int) == 64
    body_path = require_field(av_flat, "body_path", expected_type=str)
    assert body_path.startswith("M") and "C" in body_path and body_path.endswith("Z")

    th = env.run_cli_json(
        "theme-eval", "--mode", "dark", "--backdrop", "sunset", "--chat-color", "#f75d59", "--persist"
    )
    assert (
        require_field(th, "chat_background.dark", "readable_chat_bg", expected_type=str).lower()
        == oklab_readable_chat_bg_oracle("#f75d59", "dark")
    )

    in_tg = env.run_cli_json(
        "platform",
        "bridge-sync",
        "--platform",
        "telegram",
        "--direction",
        "inbound",
        "--agent-id",
        plush_id,
        "--payload",
        json.dumps({"chat_id": "-1009090", "text": "Summarize fusion status"}),
    )
    assert require_field(in_tg, "delivered", expected_type=bool) is True
    env.run_cli_json("chat", "send", "--agent-id", plush_id, "--text", "Prepare Telegram summary")
    out_tg = env.run_cli_json(
        "platform",
        "bridge-sync",
        "--platform",
        "telegram",
        "--direction",
        "outbound",
        "--agent-id",
        plush_id,
        "--payload",
        json.dumps({"chat_id": "-1009090", "text": "Dual-surface fusion verified"}),
    )
    assert require_field(out_tg, "delivered", expected_type=bool) is True

    ws = env.run_cli_json(
        "ui-probe",
        "--scenario",
        json.dumps({"scenario": "workspace-routing", "switch_sequence": ["Chats", "OpenBot", "Chats", "OpenBot"]}),
    )
    sh = unwrap_entity(ws, "openbot_shell")
    pages = pick(sh, "page_rail_pages")
    assert (len(pages) if isinstance(pages, list) else require_field(sh, "page_rail_count", expected_type=int)) == 11
    assert require_field(sh, "sidebar_width_px", expected_type=int) == 280

    tb = require_field(ws, "telegram_baseline", expected_type=dict)
    assert require_field(tb, "nav_rail_px", expected_type=int) == 52
    assert require_field(tb, "chat_sidebar_px", expected_type=int) == 288
    assert require_field(tb, "inspector_px", expected_type=int) == 320
    assert require_field(tb, "composer_min_max_px", expected_type=list) == [44, 192]
    assert require_field(tb, "shortcuts_preserved", expected_type=bool) is True
    assert require_field(tb, "palette_untouched", expected_type=bool) is True

    bf = env.run_cli_json(
        "ui-probe",
        "--scenario",
        json.dumps({"scenario": "telegram-bidirectional-fusion", "sync_agents_to_telegram": True}),
    )
    bi = unwrap_entity(bf, "bidirectional")
    assert require_field(bi, "projected_agent_peers", expected_type=int) >= 2


@e2e_test(
    id="TEST-T4-SCENARIO-06",
    tier=4,
    features=["OB-F02", "OB-F06", "OB-F10", "OB-F12", "OB-F14"],
    timeout_sec=60.0,
    description="First-Run Onboarding to Portable Multi-Host Migration: AI profile gen, Skills, MCP, CJK & site memories, duplicate, .openbot export/import, and failover.",
)
def test_t4_scenario_06_onboarding_cjk_site_memory_duplicate_export_import_and_failover(
    env: TempOpenBotEnv,
) -> None:
    gen = env.run_cli_json(
        "agent",
        "generate-profile",
        "--prompt",
        "Tokyo cloud infrastructure SRE specialist",
        "--save",
        "--operation-id",
        "cccccccc-3333-4333-8333-333333333333",
    )
    ag = unwrap_entity(require_field(gen, "agent", expected_type=dict), "agent")
    aid = require_field(ag, "id", expected_type=str)
    primary_name = require_field(ag, "name", expected_type=str)
    ws = Path(require_field(ag, "workspace_path", expected_type=str))

    q_list = extract_list(env.run_cli_json("chat", "queue-list", "--agent-id", aid), "items", "queue")
    assert any(pick(it, "welcome") is True for it in q_list)

    backups = json.dumps(
        {
            "automatic": True,
            "order": [
                {"provider": "claude", "model": "claude-haiku-5-5"},
                {"provider": "grok", "model": "grok-4"},
            ],
        }
    )
    env.run_cli_json("agent", "update", "--id", aid, "--backup-models", backups)
    env.run_cli_json(
        "tools",
        "skill-save",
        "--agent-id",
        aid,
        "--slug",
        "tokyo-ops",
        "--name",
        "Tokyo Ops",
        "--description",
        "Region failover checklist",
        "--instructions",
        "# Failover\nCheck ap-northeast-1.",
    )
    env.run_cli_json(
        "tools",
        "mcp-register",
        "--name",
        "cloud-metrics-http",
        "--transport",
        "http",
        "--url",
        "https://mcp.cloud.test/sse",
    )
    env.run_cli_json(
        "tools", "memory-put", "--agent-id", aid, "--text", "東京本番クラスタ手順8812", "--inclusion", "searchable"
    )
    env.run_cli_json(
        "tools",
        "memory-put",
        "--agent-id",
        aid,
        "--text",
        "Use FIDO2 hardware key on Tokyo console",
        "--site",
        "https://console.cloud.test/tokyo",
    )

    (ws / "runbook.md").write_text("ap-northeast-1 checklist\n", encoding="utf-8")
    os.symlink(ws / "runbook.md", ws / "current.md")
    dup = unwrap_entity(
        env.run_cli_json(
            "agent", "duplicate", "--id", aid, "--operation-id", "dddddddd-4444-4444-8444-444444444444"
        ),
        "agent",
    )
    dup_id = require_field(dup, "id", expected_type=str)
    dup_name = require_field(dup, "name", expected_type=str)
    assert dup_name != primary_name
    dup_ws = Path(require_field(dup, "workspace_path", expected_type=str))
    assert (dup_ws / "current.md").is_symlink()
    assert str((dup_ws / "current.md").resolve()).startswith(str(dup_ws.resolve()))
    assert list(dup_ws.parent.glob("*.openbot-stage")) == []

    archive_path = env.root / "migration_sc06.openbot"
    exp = env.run_cli_json("agent", "export", "--ids", f"{aid},{dup_id}", "--out", str(archive_path))
    assert require_field(exp, "exported_count", "count", expected_type=int) == 2

    with TempOpenBotEnv() as clean_host:
        imp = clean_host.run_cli_json(
            "agent", "import", "--archive", str(archive_path), "--keys", "all"
        )
        imp_agents = extract_list(imp, "agents")
        assert len(imp_agents) == 2
        by_name = {require_field(a, "name", expected_type=str): a for a in imp_agents}
        assert primary_name in by_name and dup_name in by_name
        imp_primary = require_field(by_name[primary_name], "id", expected_type=str)

        srch = clean_host.run_cli_json(
            "tools",
            "memory-search",
            "--agent-id",
            imp_primary,
            "--query",
            "手順",
            "--host",
            "sub.console.cloud.test",
        )
        assert any("8812" in str(pick(h, "text", "content") or "") for h in extract_list(srch, "memories", "results"))
        assert any("FIDO2" in str(pick(h, "text", "content") or "") for h in extract_list(srch, "site_memories"))

        fo = clean_host.run_cli_json(
            "chat", "send", "--agent-id", imp_primary, "--text", "Execute runbook", "--simulate-primary-rate-limit"
        )
        assert require_field(fo, "active_provider", "provider", expected_type=str) == "claude"
        assert "Continue this OpenBot conversation" in require_field(fo, "handoff_prompt", expected_type=str)


@e2e_test(
    id="TEST-T4-SCENARIO-07",
    tier=4,
    features=["OB-F06", "OB-F10", "OB-F12", "OB-F16", "OB-F19"],
    timeout_sec=60.0,
    description="Secure E-Commerce Procurement: Marketplace discovery, site memory, raw password block, link-payment, fill-card exfiltration lock, and receipt capture.",
)
def test_t4_scenario_07_secure_procurement_link_payment_fill_card_lock_and_receipt(
    env: TempOpenBotEnv,
) -> None:
    buyer = require_field(_mk_agent(env, "BuyerBot", access="workspace"), "id", expected_type=str)
    mkt = env.run_cli_json("platform", "marketplace-search", "--tab", "apps")
    assert require_field(mkt, "tab", expected_type=str) == "apps"

    env.run_cli_json(
        "tools",
        "memory-put",
        "--agent-id",
        buyer,
        "--text",
        "Select annual invoice billing on supplier checkout",
        "--site",
        "https://pay.supplier.test/checkout",
    )
    tab_res = env.run_cli_json(
        "tools",
        "browser-run",
        "--agent-id",
        buyer,
        "--url",
        "https://pay.supplier.test/checkout",
        "--steps",
        json.dumps([{"action": "snapshot"}]),
    )
    tab_id = require_field(tab_res, "tab_id", expected_type=str)

    env.run_cli_expect_rejection(
        "tools",
        "browser-run",
        "--agent-id",
        buyer,
        "--tab-id",
        tab_id,
        "--url",
        "https://pay.supplier.test/checkout",
        "--steps",
        json.dumps([{"action": "fill_fields", "fields": [{"selector": "input[type=password]", "value": "s3cret"}]}]),
        error_contains=("password", "secret", "vault"),
    )

    pay_ctx = (
        "Procure 1 annual build-cache appliance license from SupplierCorp under approved engineering "
        "purchase order PO-2026-9910."
    )
    assert len(pay_ctx) >= 100
    lp = env.run_cli_json(
        "tools",
        "link-payment",
        "--agent-id",
        buyer,
        "--amount",
        "12500",
        "--currency",
        "USD",
        "--merchant-name",
        "SupplierCorp",
        "--merchant-url",
        "https://pay.supplier.test/checkout",
        "--context",
        pay_ctx,
        "--approve",
    )
    card_id = require_field(lp, "card_id", expected_type=str)

    fc = env.run_cli_json(
        "tools", "fill-card", "--agent-id", buyer, "--tab-id", tab_id, "--card-id", card_id, "--approve"
    )
    assert require_field(fc, "filled_card", expected_type=bool) is True

    for blocked_tool in (
        "openbot_browser.screenshot",
        "openbot_browser.evaluate",
        "openbot_browser.recording_start",
    ):
        env.run_cli_expect_rejection(
            "tools",
            "call",
            "--agent-id",
            buyer,
            "--tab-id",
            tab_id,
            "--name",
            blocked_tool,
            "--arguments",
            json.dumps({"tabId": tab_id}),
            error_contains=("filled card", "unavailable", "navigate"),
        )

    receipt = env.run_cli_json(
        "tools",
        "browser-run",
        "--agent-id",
        buyer,
        "--tab-id",
        tab_id,
        "--url",
        "https://pay.supplier.test/receipt",
        "--steps",
        json.dumps([{"action": "navigate", "url": "https://pay.supplier.test/receipt"}, {"action": "screenshot"}]),
    )
    assert require_field(receipt, "tab_id", expected_type=str) == tab_id

    oss = unwrap_entity(env.run_cli_json("ui-probe", "--scenario", "desktop-os-surfaces"), "os_surfaces", "openbot_shell")
    assert "BrowserDock" in extract_list(oss, "surfaces")


@e2e_test(
    id="TEST-T4-SCENARIO-08",
    tier=4,
    features=["OB-F07", "OB-F15", "OB-F16"],
    timeout_sec=60.0,
    description="Deep Research & Long-Context Synthesis: multi-lane side threads, steerable queue, ephemeral side-question, pin ownership, and 80% compaction.",
)
def test_t4_scenario_08_deep_research_side_threads_queue_steer_pin_and_compaction(
    env: TempOpenBotEnv,
) -> None:
    aid = require_field(
        _mk_agent(env, "ResearchLead", reasoning_effort="ultra"), "id", expected_type=str
    )
    root_msg = env.run_cli_json("chat", "send", "--agent-id", aid, "--text", "Survey consensus protocols")
    mid = require_field(root_msg, "message_id", expected_type=str)

    rx = env.run_cli_json("chat", "react", "--agent-id", aid, "--message-id", mid, "--emoji", "🔬", "--actor", "user")
    assert any(pick(r, "emoji") == "🔬" for r in extract_list(rx, "reactions"))
    env.run_cli_json("chat", "pin", "--agent-id", aid, "--message-id", mid, "--pinned-by", "user")
    env.run_cli_expect_rejection(
        "chat",
        "pin",
        "--agent-id",
        aid,
        "--message-id",
        mid,
        "--pinned-by",
        "agent",
        "--unpin",
        error_contains=("pinned-by-person", "pinned by person", "pinned by user", "person"),
    )

    env.run_cli_json("chat", "queue-pause", "--agent-id", aid)
    d1 = require_field(env.run_cli_json("chat", "queue-add", "--agent-id", aid, "--text", "Angle 1: Raft"), "delivery_id", expected_type=str)
    d2 = require_field(env.run_cli_json("chat", "queue-add", "--agent-id", aid, "--text", "Angle 2: Paxos"), "delivery_id", expected_type=str)
    d3 = require_field(env.run_cli_json("chat", "queue-add", "--agent-id", aid, "--text", "Angle 3: HotStuff"), "delivery_id", expected_type=str)
    env.run_cli_json("chat", "queue-reorder", "--agent-id", aid, "--delivery-ids", f"{d3},{d1},{d2}")
    resumed = env.run_cli_json("chat", "queue-resume", "--agent-id", aid)
    turn_id = require_field(resumed, "active_turn_id", "turn_id", expected_type=str)
    steered = env.run_cli_json(
        "chat", "queue-steer", "--agent-id", aid, "--delivery-id", d1, "--expected-turn-id", turn_id
    )
    assert require_field(steered, "steered", expected_type=bool) is True

    main_before = extract_list(env.run_cli_json("chat", "history", "--agent-id", aid), "messages")
    br = env.run_cli_json(
        "chat", "branch", "--agent-id", aid, "--message-id", mid, "--request-id", "sc08-branch-1"
    )
    side_tid = require_field(br, "task_id", "thread_id", expected_type=str)
    assert side_tid.startswith("openbot-thread-")
    env.run_cli_json(
        "chat", "send", "--agent-id", aid, "--task-id", side_tid, "--text", "Explore Byzantine fault bounds"
    )
    main_after_side = extract_list(env.run_cli_json("chat", "history", "--agent-id", aid), "messages")
    assert len(main_after_side) == len(main_before)

    sq = env.run_cli_json(
        "chat",
        "side-question",
        "--agent-id",
        aid,
        "--question",
        "Compare message complexity?",
        "--earlier",
        json.dumps([{"question": "Q1", "answer": "A1"}, {"question": "Q2", "answer": "A2"}]),
    )
    assert len(require_field(sq, "answer", expected_type=str)) >= 1
    assert len(extract_list(env.run_cli_json("chat", "history", "--agent-id", aid), "messages")) == len(main_before)

    comp = unwrap_entity(
        env.run_cli_json("chat", "compact", "--agent-id", aid, "--simulate-usage", "88000/100000"),
        "compaction",
    )
    assert require_field(comp, "after_tokens", expected_type=int) < 88000

    conv = unwrap_entity(
        env.run_cli_json(
            "ui-probe",
            "--scenario",
            json.dumps({"scenario": "conversation-virtualized", "message_length": 24000, "streaming": False}),
        ),
        "conversation",
    )
    assert require_field(conv, "collapsed", expected_type=bool) is True
    comp_ui = unwrap_entity(
        env.run_cli_json(
            "ui-probe",
            "--scenario",
            json.dumps({"scenario": "composer-queue-sidepanel", "paste_length": 4500}),
        ),
        "composer_probe",
    )
    assert require_field(comp_ui, "pasted_as_file", expected_type=bool) is True


@e2e_test(
    id="TEST-T4-SCENARIO-09",
    tier=4,
    features=["OB-F04", "OB-F05", "OB-F06", "OB-F12", "OB-F13", "OB-F16"],
    timeout_sec=60.0,
    description="Multimodal Voice Studio & Live Theme Customization: local model discovery, avatar WebP, STT/TTS/Live WebRTC voice, credit video, and CrewPile.",
)
def test_t4_scenario_09_multimodal_voice_studio_live_theme_and_crew_pile(
    env: TempOpenBotEnv,
) -> None:
    srv_models, models_url = _start_local_models_server(["whisper-local-v3", "llama3.3-70b"])
    try:
        created = env.run_cli_json(
            "agent", "create", "--name", "StudioDirector", "--probe-servers", models_url
        )
        ag = unwrap_entity(created, "agent")
        aid = require_field(ag, "id", expected_type=str)
        assert len(extract_list(created, "detected_servers", "servers")) == 1
    finally:
        srv_models.shutdown()

    avatar_png = env.write_png_fixture("fixtures/director_800x600.png", width=800, height=600)
    norm = require_field(
        env.run_cli_json("avatar-eval", "--seed", "director-seed", "--normalize-image", str(avatar_png)),
        "normalized_image",
        expected_type=dict,
    )
    assert require_field(norm, "mime", expected_type=str) == "image/webp"
    assert require_field(norm, "byte_length", "normalized_bytes", expected_type=int) <= 524288

    v_stt = env.run_cli_json(
        "platform", "voice-eval", "--mode", "stt", "--stt-provider", "whisper-turbo"
    )
    v_tts = env.run_cli_json(
        "platform", "voice-eval", "--mode", "tts", "--tts-provider", "elevenlabs-flash-v2.5", "--agent-id", aid
    )
    v_live = env.run_cli_json(
        "platform", "voice-eval", "--mode", "live", "--live-voice", "maple", "--agent-id", aid
    )
    assert require_field(v_stt, "stt_providers", expected_type=int) == 8
    assert require_field(v_tts, "tts_providers", expected_type=int) == 7
    assert require_field(v_live, "live_voices", expected_type=int) == 25
    assert require_field(v_live, "webrtc_ready", expected_type=bool) is True

    vid = env.run_cli_json(
        "platform",
        "html-render",
        "--mode",
        "preview",
        "--html",
        "<div>Studio Reel</div>",
        "--backdrop",
        "coast",
        "--generate-media",
        "video",
    )
    assert require_field(vid, "sandboxed", expected_type=bool) is True
    assert require_field(vid, "media_kind", expected_type=str) == "video"

    latte = json.dumps(
        {
            "name": "Studio Light",
            "appearance": "light",
            "colors": {
                "canvas": "#eff1f5",
                "frame": "#e6e9ef",
                "sidebar": "#e6e9ef",
                "surface": "#ccd0da",
                "text": "#4c4f69",
                "textMuted": "#6c6f85",
                "border": "#bcc0cc",
                "accent": "#1e66f5",
                "accentForeground": "#eff1f5",
                "success": "#40a02b",
                "warning": "#df8e1d",
                "danger": "#d20f39",
            },
        }
    )
    env.write_fixture("data/themes/studio-light.json", latte)
    env.write_fixture("config/OpenBot/themes/studio-light.json", latte)
    hr = unwrap_entity(env.run_cli_json("ui-probe", "--scenario", "theme-hot-reload"), "hot_reload")
    assert require_field(hr, "watcher_triggered", "reloaded", expected_type=bool) is True
    assert require_field(hr, "active_canvas", expected_type=str).lower() == "#eff1f5"

    cp = unwrap_entity(env.run_cli_json("ui-probe", "--scenario", "crew-pile-physics"), "crew_pile")
    assert require_field(cp, "step_hz", expected_type=int) == 240


@e2e_test(
    id="TEST-T4-SCENARIO-10",
    tier=4,
    features=["OB-F11", "OB-F13", "OB-F18", "OB-F20"],
    timeout_sec=60.0,
    description="Scheduled & Loopback DevOps Mesh: quiet 180s routines, await_event, /run CSRF/Auth/allowAutomation/30-hr rate cap, multi-bridge alerts, and RoutineCalendar.",
)
def test_t4_scenario_10_scheduled_loopback_devops_mesh_rate_cap_and_bridges(
    env: TempOpenBotEnv,
) -> None:
    auto_id = require_field(
        _mk_agent(env, "AutoOpsBot", allow_automation="true"), "id", expected_type=str
    )
    locked_id = require_field(
        _mk_agent(env, "LockedOpsBot", allow_automation="false"), "id", expected_type=str
    )

    rt = env.run_cli_json(
        "platform",
        "routine-save",
        "--agent-id",
        auto_id,
        "--name",
        "DevOpsPulse",
        "--schedule",
        json.dumps({"kind": "interval", "amount": 3, "unit": "minutes", "anchorAt": "2026-01-01T00:00:00Z"}),
        "--quiet-policy",
        "no_update",
    )
    rid = require_field(rt, "id", "routine_id", expected_type=str)
    assert require_field(rt, "interval_seconds", expected_type=int) == 180

    quiet = env.run_cli_json("platform", "routine-run", "--routine-id", rid, "--quiet-no-update")
    assert require_field(quiet, "status", expected_type=str) == "quiet_suppressed"
    assert require_field(quiet, "notified", expected_type=bool) is False

    waits_run = unwrap_entity(
        env.run_cli_json(
            "platform",
            "routine-run",
            "--routine-id",
            rid,
            "--await-events",
            json.dumps([{"event": "ci.green", "note": "Wait CI"}, {"event": "canary.ok", "note": "Wait canary"}]),
        ),
        "routine",
        "run",
    )
    assert require_field(waits_run, "open_waits_count", expected_type=int) == 2

    srv = env.start_server()
    st_bad_tok, _, _ = srv.http_automation(
        "POST", "/run", payload={"agentId": auto_id, "routineId": rid, "payload": "x"}, token="forged-token"
    )
    assert st_bad_tok == 401
    st_origin, _, _ = srv.http_automation(
        "POST", "/run", payload={"agentId": auto_id, "routineId": rid, "payload": "x"}, include_origin="https://attacker.test"
    )
    assert st_origin == 403
    st_host, _, _ = srv.http_automation(
        "POST", "/run", payload={"agentId": auto_id, "routineId": rid, "payload": "x"}, custom_host="evil.rebind.test:80"
    )
    assert st_host == 403
    st_locked, _, _ = srv.automation_run(locked_id, payload="should-forbid")
    assert st_locked == 403

    for idx in range(1, 31):
        st_ok, _, _ = srv.automation_run(auto_id, routine_id=rid, payload=f"pulse-{idx}")
        assert st_ok in (200, 202)
    st_429, _, _ = srv.automation_run(auto_id, routine_id=rid, payload="pulse-31")
    assert st_429 == 429

    for bridge in ("slack", "discord", "whatsapp"):
        out = env.run_cli_json(
            "platform",
            "bridge-sync",
            "--platform",
            bridge,
            "--direction",
            "outbound",
            "--agent-id",
            auto_id,
            "--payload",
            json.dumps({"target": "ops-room", "text": f"Rate cap verified ({bridge})"}),
        )
        assert require_field(out, "delivered", expected_type=bool) is True

    pp = unwrap_entity(env.run_cli_json("ui-probe", "--scenario", "workflow-canvas-pages"), "pages_probe")
    assert require_field(pp, "routine_calendar.window_days", expected_type=int) == 44


@e2e_test(
    id="TEST-T4-SCENARIO-11",
    tier=4,
    features=["OB-F04", "OB-F08", "OB-F09", "OB-F17"],
    timeout_sec=60.0,
    description="Organization-Wide @everyone Review Channel: 8-bot '+5' cluster, CHANNEL_PARALLEL_LIMIT=2, sibling hiding, 8k/12k delta packet, superseded, and archive.",
)
def test_t4_scenario_11_org_all_hands_everyone_review_sibling_hiding_and_archive(
    env: TempOpenBotEnv,
) -> None:
    dept_ids = [
        require_field(_mk_agent(env, f"DeptBot{i:02d}"), "id", expected_type=str)
        for i in range(1, 9)
    ]
    ch = _mk_channel(env, "OrgAllHands", dept_ids, lead=dept_ids[0])
    cid = require_field(ch, "id", expected_type=str)

    cl = require_field(
        env.run_cli_json("avatar-eval", "--seed", "org-seed", "--cluster-count", "8"),
        "cluster",
        expected_type=dict,
    )
    exp_cl = channel_cluster_layout_oracle(8)
    assert require_field(cl, "shown_faces", "visible_faces", expected_type=int) == exp_cl["shown_faces"] == 3
    assert pick(cl, "plus_pill", "overflow_pill") == "+5"

    ranked = extract_list(
        env.run_cli_json("channel", "list", "--rank-mentions", "--channel-id", cid, "--query", "deptbot01"),
        "ranked_members",
        "members",
    )
    assert pick(ranked[0], "agent_id", "id") == dept_ids[0]

    p_all = env.run_cli_json(
        "channel",
        "post",
        "--channel-id",
        cid,
        "--text",
        "@everyone Submit Q4 architecture review",
        "--hold-running",
        "--resources-map",
        json.dumps({dept_ids[0]: ["browser"], dept_ids[1]: ["browser"]}),
    )
    all_tasks = extract_list(p_all, "tasks")
    active = [t for t in all_tasks if pick(t, "state") in ("running", "starting")]
    assert len(active) <= 2

    by_owner = {require_field(t, "owner_agent_id", expected_type=str): t for t in all_tasks}
    t0_id = require_field(by_owner[dept_ids[0]], "id", expected_type=str)
    t1_id = require_field(by_owner[dept_ids[1]], "id", expected_type=str)

    env.run_cli_json(
        "channel", "result", "--channel-id", cid, "--task-id", t1_id, "--agent-id", dept_ids[1], "--text", "PEER_SECRET_902"
    )
    pkt1 = unwrap_entity(
        env.run_cli_json("channel", "history-packet", "--channel-id", cid, "--task-id", t0_id, "--agent-id", dept_ids[0]),
        "packet",
    )
    assert "PEER_SECRET_902" not in require_field(pkt1, "text", expected_type=str)

    r0 = env.run_cli_json(
        "channel", "result", "--channel-id", cid, "--task-id", t0_id, "--agent-id", dept_ids[0], "--text", "Lead review v1"
    )
    m0_id = require_field(r0, "message_id", "id", expected_type=str)

    pkt2 = unwrap_entity(
        env.run_cli_json(
            "channel",
            "history-packet",
            "--channel-id",
            cid,
            "--task-id",
            t0_id,
            "--agent-id",
            dept_ids[0],
            "--second-turn",
            "--simulate-large-transcript",
        ),
        "packet",
    )
    assert require_field(pkt2, "kind", expected_type=str) == "delta"
    assert 0 < require_field(pkt2, "summary_chars", expected_type=int) <= 12000

    env.run_cli_json(
        "channel", "post", "--channel-id", cid, "--text", "Actually include latency budget", "--reply-to", m0_id
    )
    msgs = {
        require_field(m, "id", expected_type=str): m
        for m in extract_list(env.run_cli_json("channel", "get", "--channel-id", cid), "messages")
    }
    assert require_field(msgs[m0_id], "superseded", expected_type=bool) is True

    assert require_field(unwrap_entity(env.run_cli_json("channel", "archive", "--channel-id", cid), "channel"), "archived", expected_type=bool) is True
    assert require_field(unwrap_entity(env.run_cli_json("channel", "restore", "--channel-id", cid), "channel"), "archived", expected_type=bool) is False


@e2e_test(
    id="TEST-T4-SCENARIO-12",
    tier=4,
    features=["OB-F01", "OB-F02", "OB-F03", "OB-F10", "OB-F19", "OB-F20"],
    timeout_sec=60.0,
    description="End-to-End Security, SQLite Authorizer, Privilege-Confinement, JSON Escape Repair & Crash-Safety Audit Across All 7 OS Surfaces.",
)
def test_t4_scenario_12_security_authorizer_confinement_and_seven_os_surfaces_audit(
    env: TempOpenBotEnv,
) -> None:
    owner = _mk_agent(env, "OwnerFull", access="full", computer_use="true")
    contractor = _mk_agent(env, "ContractorSandboxed", access="workspace", computer_use="false")
    owner_id = require_field(owner, "id", expected_type=str)
    con_id = require_field(contractor, "id", expected_type=str)
    ws_parent = Path(require_field(owner, "workspace_path", expected_type=str)).parent

    env.run_cli_expect_rejection(
        "tools",
        "call",
        "--agent-id",
        con_id,
        "--name",
        "openbot.update_profile",
        "--arguments",
        json.dumps({"access": "full", "computerUse": True}),
        error_contains=("widen", "permission", "only the user", "cannot widen"),
    )

    env.run_cli_json(
        "tools",
        "agent-data-exec",
        "--agent-id",
        owner_id,
        "--mode",
        "write",
        "--sql",
        "CREATE TABLE audit_vault (id INT PRIMARY KEY, entry TEXT)",
    )
    env.run_cli_json(
        "tools",
        "agent-data-exec",
        "--agent-id",
        owner_id,
        "--mode",
        "write",
        "--sql",
        "INSERT INTO audit_vault VALUES (1, 'sealed')",
    )

    env.run_cli_expect_rejection(
        "tools",
        "agent-data-exec",
        "--agent-id",
        con_id,
        "--mode",
        "write",
        "--sql",
        "DROP TABLE audit_vault",
        error_contains=("authorized", "authorizer", "owner", "not table owner"),
    )
    env.run_cli_expect_rejection(
        "tools",
        "agent-data-exec",
        "--agent-id",
        con_id,
        "--mode",
        "write",
        "--sql",
        "PRAGMA writable_schema = ON",
        error_contains=("not allowed", "authorizer", "authorized", "disallowed", "blocked"),
    )

    meta_tbls = env.query_sqlite(
        "SELECT name FROM sqlite_master WHERE type='table' AND name IN ('openbot_metadata', '_openbot_tables')",
        db_kind="agent_data",
    )
    assert len(meta_tbls) >= 1
    meta_name = str(meta_tbls[0]["name"])
    del_meta = env.run_cli_expect_rejection(
        "tools",
        "agent-data-exec",
        "--agent-id",
        con_id,
        "--mode",
        "write",
        "--sql",
        f"DELETE FROM {meta_name}",
        error_contains=("authorized", "authorizer", "protected", "system", "metadata"),
    )
    assert "no such table" not in f"{del_meta.stdout}\n{del_meta.stderr}\n{del_meta.error}".lower()
    surviving = env.query_sqlite(
        f"SELECT owner_agent_id FROM {meta_name} WHERE table_name = 'audit_vault'",
        db_kind="agent_data",
    )
    assert len(surviving) == 1 and surviving[0]["owner_agent_id"] == owner_id

    env.run_cli_expect_rejection(
        "tools",
        "call",
        "--agent-id",
        con_id,
        "--name",
        "openbot.tool_search",
        "--arguments",
        json.dumps({"query": "vault"}),
        error_contains=("discovery", "direct", "recursive", "cannot call"),
    )

    repaired = env.run_cli_json(
        "tools",
        "call",
        "--agent-id",
        con_id,
        "--name",
        "openbot.list_tables",
        "--raw-arguments",
        '{"note":"C:\\work\\new_folder\\audit.txt"} }',
    )
    assert require_field(repaired, "repaired_json", expected_type=bool) is True

    av_rm = env.run_cli_json(
        "avatar-eval", "--seed", "agent-1", "--style", "flat", "--reduced-motion", "--t", "2.5"
    )
    assert math.isclose(float(require_field(av_rm, "breath_sy", expected_type=(int, float))), 1.0, abs_tol=1e-6)
    cp_rm = unwrap_entity(
        env.run_cli_json("ui-probe", "--scenario", "crew-pile-reduced-motion-and-broken-url"),
        "crew_pile_reduced",
    )
    assert require_field(cp_rm, "failed_url_fallback", expected_type=bool) is True

    oss = unwrap_entity(env.run_cli_json("ui-probe", "--scenario", "desktop-os-surfaces"), "os_surfaces", "openbot_shell")
    assert extract_list(oss, "surfaces") == [
        "Main",
        "ChatWindow",
        "DynamicIsland",
        "AgentDock",
        "BrowserDock",
        "CUA",
        "Recovery",
    ]
    ws = env.run_cli_json("ui-probe", "--scenario", "workspace-routing")
    assert require_field(ws, "telegram_baseline.nav_rail_px", expected_type=int) == 52
    assert list(ws_parent.glob("*.openbot-stage")) == []
    env.check_sqlite_integrity("openbot")
    env.check_sqlite_integrity("agent_data")
