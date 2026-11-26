"""Tier 3 Cross-Feature Pairwise Combination Opaque-Box E2E Tests (24 tests, TEST-T3-PAIR-01..24).

Exercises multi-subsystem integration seams across OB-F01..OB-F20:
  - Channel roster transitions + FNV-1a seeds + ChannelAvatar +N cluster pills + working-face cycle
  - Isolated channel memory vs personal FTS5 memory + agent-data.db cross-agent DML & DDL authorizer
  - ChannelBrowser policy + browser/workspace mutexes + zero-knowledge Vault secret autofill
  - Inbound Telegram bridge -> 12-node DAG workflow (9 step + 3 annotation kinds) -> Channel routing
  - HMAC-SHA256 webhook routine + loopback /run CSRF/token/30-hr rate cap + DynamicIsland critical lock
  - Custom JSON theme hot-reload + OKLab chat color + OpenBot <-> Telegram Chats dual-surface switch
  - ask_ui form/Vault redaction + link-payment + fill-card screenshot lock + post-navigation unlock
  - Portable .openbot bundle export/import + SKILL.md + MCP + FTS5 memory + backupModels failover
  - 'goutte' FNV-1a replacement + reduced-motion BloubBot/backdrop + horizon culling & departFige
  - AI generate-profile + idempotent .openbot-stage duplicate + CQRS receipts + shell preference stores
  - CrewPile 240Hz vs reduced-motion settle + broken avatarUrl fallback + WebP normalizer + Recovery
  - QueuePanel pause/reorder/resume/steer + routine steer guard + human-pin ownership + SidePanel splits
  - 9th-handoff CHANNEL_ASSIGNMENT_LIMIT=8 stop + ChannelStoppedTasks UI + reassign + delta packet
  - Live /v1/* Team API + /v1/events SSE + symmetric dm-<sha256> DMs + Status/Result/Evidence Mailbox
  - 13-table Stripe CRM sync + human-gated crm-propose + 30-day static site publish + CrmPage
  - Sandboxed html-render (openbotPage + CSP) + jellyfish backdrop + credit-metered image/video media
  - 3-tab Marketplace ('fishing' fallback) + SKILL.md installation + live localhost /v1/models discovery
  - Voice subsystem (8 STT, 7 TTS, 25 WebRTC Live voices + voiceAgent) + VoicePill & @/$ token picker
  - Acyclic DAG WorkflowEngine cycle rejection + >=180s RoutineEngine + WorkflowCanvas/RoutineCalendar
  - Live /v1/capabilities (v1..v10, 18 providers, >=220 caps) + 4-platform bridges + workspace-routing
  - Sidebar clamp/compact + multi-split SidePanel + ChatWindow/CUA + privilege self-widening guard
  - 80% context auto-compaction + read-only side-question + reactions/pins across timeline & shell
  - Diacritic-insensitive @mention ranking + follow-up superseded=true + mouse pointer gaze & responded
  - Channel archive/restore read-only guard + PNG->WebP avatar normalizer + Settings & baseline geometry
"""

from __future__ import annotations

import http.server
import json
import math
import threading
from pathlib import Path
from typing import Any, Dict, List, Optional, Sequence, Tuple

from .harness import (
    AVATAR_EXPRESSIONS,
    BUILTIN_AGENT_PROVIDERS,
    TempOpenBotEnv,
    channel_cluster_layout_oracle,
    dm_thread_id_oracle,
    e2e_test,
    extract_list,
    oklab_readable_chat_bg_oracle,
    pick,
    require_field,
    resolve_bloub_avatar_profile_oracle,
    stable_index,
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
        "message": "Workflow approved and dispatched",
        "stepName": "Dispatch Output",
    }
    if channel_id:
        out_fields["channelId"] = channel_id
    nodes: List[Dict[str, Any]] = [
        {"id": "n-trig", "kind": "trigger", "at": {"x": 80.0, "y": 100.0}, "size": {"width": 220.0, "height": 120.0}, "fields": {"trigger": "telegram", "event": "mention", "stepName": "Inbound Trigger"}},
        {"id": "n-prompt", "kind": "prompt", "at": {"x": 80.0, "y": 260.0}, "size": {"width": 220.0, "height": 120.0}, "fields": {"text": "Summarize incident severity", "stepName": "Triage Prompt"}},
        {"id": "n-asset", "kind": "asset", "at": {"x": 80.0, "y": 420.0}, "size": {"width": 220.0, "height": 120.0}, "fields": {"fileId": "asset-runbook-01", "stepName": "Runbook Asset"}},
        {"id": "n-llm", "kind": "llm", "at": {"x": 360.0, "y": 180.0}, "size": {"width": 240.0, "height": 140.0}, "fields": {"provider": "codex", "model": "gpt-6-luna", "prompt": "Classify P0 vs P2", "stepName": "Classify"}},
        {"id": "n-agent", "kind": "agent", "at": {"x": 640.0, "y": 180.0}, "size": {"width": 240.0, "height": 140.0}, "fields": {"agentId": agent_id, "task": "Draft hotfix plan", "stepName": "Triage Agent"}},
        {"id": "n-image", "kind": "image", "at": {"x": 640.0, "y": 380.0}, "size": {"width": 240.0, "height": 140.0}, "fields": {"media": "image", "model": "gpt-image", "prompt": "Incident card", "stepName": "Status Image"}},
        {"id": "n-cond", "kind": "condition", "at": {"x": 920.0, "y": 180.0}, "size": {"width": 220.0, "height": 120.0}, "fields": {"rule": "severity == P0", "stepName": "Needs Approval"}},
        {"id": "n-appr", "kind": "approval", "at": {"x": 1180.0, "y": 120.0}, "size": {"width": 220.0, "height": 120.0}, "fields": {"question": "Approve rollout?", "stepName": "Human Approval"}},
        {"id": "n-out", "kind": "output", "at": {"x": 1460.0, "y": 200.0}, "size": {"width": 220.0, "height": 120.0}, "fields": out_fields},
        {"id": "n-text", "kind": "text", "at": {"x": 80.0, "y": 20.0}, "size": {"width": 300.0, "height": 48.0}, "fields": {"text": "Production Incident DAG"}},
        {"id": "n-note", "kind": "note", "at": {"x": 1180.0, "y": 320.0}, "size": {"width": 240.0, "height": 120.0}, "fields": {"text": "Canvas annotation note"}},
        {"id": "n-shape", "kind": "shape", "at": {"x": 900.0, "y": 80.0}, "size": {"width": 540.0, "height": 220.0}, "fields": {"text": "Approval Zone"}},
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
# Pairwise Tests TEST-T3-PAIR-01 .. TEST-T3-PAIR-04
# ============================================================================


@e2e_test(
    id="TEST-T3-PAIR-01",
    tier=3,
    features=["OB-F01", "OB-F04", "OB-F08", "OB-F17"],
    description="Channel roster transitions (3->8->13->4) align FNV-1a avatar profiles, ChannelAvatar +N cluster pills, and 1800ms working-faces cycle.",
)
def test_t3_pair_01_channel_roster_cluster_pills_and_working_faces(env: TempOpenBotEnv) -> None:
    member_ids: List[str] = []
    for idx in range(1, 14):
        seed = f"member-seed-{idx:02d}"
        extra = {"avatar_seed": seed, "avatar_hue": "215"} if idx == 1 else {"avatar_seed": seed}
        ag = _mk_agent(env, f"RosterBot{idx:02d}", **extra)
        member_ids.append(require_field(ag, "id", expected_type=str))

    av01 = env.run_cli_json("avatar-eval", "--seed", "member-seed-01", "--hue", "215")
    prof01 = pick(av01, "profile") if isinstance(pick(av01, "profile"), dict) else av01
    exp_prof = resolve_bloub_avatar_profile_oracle("member-seed-01", hue=215)
    assert {
        "style": require_field(prof01, "style", expected_type=str),
        "shape": require_field(prof01, "shape", expected_type=str),
        "expression": require_field(prof01, "expression", expected_type=str),
        "color": require_field(prof01, "color", expected_type=str).lower(),
    } == exp_prof

    ch = _mk_channel(env, "ClusterRoom", member_ids[:3], lead=member_ids[0])
    cid = require_field(ch, "id", expected_type=str)
    prev_rev = require_field(ch, "revision", expected_type=int)

    for target_count in (3, 8, 13, 4):
        if target_count != 3:
            upd = env.run_cli_json(
                "channel",
                "update",
                "--channel-id",
                cid,
                "--members",
                ",".join(member_ids[:target_count]),
                "--lead",
                member_ids[0],
            )
            ch_upd = unwrap_entity(upd, "channel")
            cur_rev = require_field(ch_upd, "revision", expected_type=int)
            assert cur_rev > prev_rev
            prev_rev = cur_rev
        cl_res = env.run_cli_json(
            "avatar-eval", "--seed", "member-seed-01", "--cluster-count", str(target_count)
        )
        cl = require_field(cl_res, "cluster", expected_type=dict)
        exp_cl = channel_cluster_layout_oracle(target_count)
        assert require_field(cl, "shown_faces", "visible_faces", expected_type=int) == exp_cl["shown_faces"]
        assert pick(cl, "plus_pill", "overflow_pill") == exp_cl["plus_pill"]

    env.run_cli_json(
        "channel",
        "post",
        "--channel-id",
        cid,
        "--recipient-agent-id",
        member_ids[0],
        "--text",
        "Audit cluster layout",
        "--hold-running",
    )
    wf_t0 = env.run_cli_json(
        "avatar-eval", "--seed", "member-seed-01", "--style", "plush", "--mood", "working", "--working-faces", "--t", "0.0"
    )
    wf_t1 = env.run_cli_json(
        "avatar-eval", "--seed", "member-seed-01", "--style", "plush", "--mood", "working", "--working-faces", "--t", "1.85"
    )
    assert require_field(wf_t0, "state", expected_type=str) == "thinking"
    e0 = require_field(wf_t0, "profile.expression", "expression", expected_type=str)
    e1 = require_field(wf_t1, "profile.expression", "expression", expected_type=str)
    assert e0 in AVATAR_EXPRESSIONS and e1 in AVATAR_EXPRESSIONS and e0 != e1

    probe = env.run_cli_json(
        "ui-probe",
        "--scenario",
        json.dumps({"scenario": "channel-conversation-recovery", "channel_id": cid, "members_count": 4, "working_ids": [member_ids[0]]}),
    )
    cui = unwrap_entity(probe, "channel_ui")
    assert require_field(cui, "icon_catalog_count", expected_type=int) == 20
    assert require_field(cui, "reactions_enabled", expected_type=bool) is False


@e2e_test(
    id="TEST-T3-PAIR-02",
    tier=3,
    features=["OB-F02", "OB-F08", "OB-F10"],
    description="Isolated channel memory stays separate from personal FTS5 memory while agent-data.db enforces cross-agent DML and owner_agent_id DDL guard.",
)
def test_t3_pair_02_isolated_channel_memory_fts5_and_agent_data_ddl_guard(env: TempOpenBotEnv) -> None:
    a1 = require_field(_mk_agent(env, "LeadSql"), "id", expected_type=str)
    a2 = require_field(_mk_agent(env, "PeerMem"), "id", expected_type=str)
    ch = _mk_channel(env, "SecOpsRoom", [a1, a2], lead=a1, isolated="true")
    cid = require_field(ch, "id", expected_type=str)
    assert require_field(ch, "isolated", expected_type=bool) is True

    env.run_cli_json(
        "tools", "memory-put", "--agent-id", a2, "--text", "Personal rotation runbook 7741", "--inclusion", "searchable"
    )
    env.run_cli_json(
        "tools",
        "call",
        "--agent-id",
        a1,
        "--name",
        "openbot.channel_remember",
        "--arguments",
        json.dumps({"channelId": cid, "text": "Channel incident SLA 15m"}),
    )
    ch_mem_rows = env.query_sqlite(
        "SELECT * FROM projection_channel_memories WHERE channel_id = ?", (cid,)
    )
    assert len(ch_mem_rows) >= 1 and any(
        "Channel incident SLA 15m" in str(r.get("text") or r.get("content") or "")
        for r in ch_mem_rows
    )

    s_a1_sla = extract_list(
        env.run_cli_json("tools", "memory-search", "--agent-id", a1, "--query", "SLA"),
        "memories",
        "results",
    )
    assert s_a1_sla == [] and all(
        "Channel incident SLA 15m" not in str(pick(m, "text", "content") or "")
        for m in s_a1_sla
    )

    s_a1 = extract_list(
        env.run_cli_json("tools", "memory-search", "--agent-id", a1, "--query", "7741"),
        "memories",
        "results",
    )
    s_a2 = extract_list(
        env.run_cli_json("tools", "memory-search", "--agent-id", a2, "--query", "7741"),
        "memories",
        "results",
    )
    assert s_a1 == []
    assert len(s_a2) >= 1 and any("7741" in str(pick(m, "text", "content") or "") for m in s_a2)

    c_tbl = env.run_cli_json(
        "tools", "agent-data-exec", "--agent-id", a1, "--mode", "write", "--sql", "CREATE TABLE secops_kpi (k TEXT PRIMARY KEY, v INT)"
    )
    assert require_field(c_tbl, "owner_agent_id", expected_type=str) == a1
    ins = env.run_cli_json(
        "tools", "agent-data-exec", "--agent-id", a2, "--mode", "write", "--sql", "INSERT INTO secops_kpi VALUES (?, ?)", "--params", json.dumps(["mttr", 9])
    )
    assert require_field(ins, "changes", expected_type=int) == 1
    sel = env.run_cli_json(
        "tools", "agent-data-exec", "--agent-id", a1, "--mode", "read", "--sql", "SELECT k, v FROM secops_kpi WHERE k = ?", "--params", json.dumps(["mttr"])
    )
    rows = extract_list(sel, "rows")
    assert rows == [["mttr", 9]] or rows == [{"k": "mttr", "v": 9}]

    env.run_cli_expect_rejection(
        "tools",
        "agent-data-exec",
        "--agent-id",
        a2,
        "--mode",
        "write",
        "--sql",
        "DROP TABLE secops_kpi",
        error_contains=("authorized", "authorizer", "owner", "not table owner"),
    )
    env.check_sqlite_integrity("openbot")
    env.check_sqlite_integrity("agent_data")


@e2e_test(
    id="TEST-T3-PAIR-03",
    tier=3,
    features=["OB-F08", "OB-F09", "OB-F10"],
    description="Designated ChannelBrowser member policy interacts with browser/workspace mutexes, zero-knowledge vault-secret, and member-removal reset.",
)
def test_t3_pair_03_channel_browser_policy_mutexes_and_vault_secret(env: TempOpenBotEnv) -> None:
    a1 = require_field(_mk_agent(env, "CoordLead"), "id", expected_type=str)
    a2 = require_field(_mk_agent(env, "BrowserOwner"), "id", expected_type=str)
    a3 = require_field(_mk_agent(env, "RepoPeer"), "id", expected_type=str)
    ch = _mk_channel(
        env,
        "SsoChannel",
        [a1, a2, a3],
        lead=a1,
        browser=json.dumps({"kind": "member", "agentId": a2}),
    )
    cid = require_field(ch, "id", expected_type=str)
    br_cfg = require_field(ch, "browser", expected_type=dict)
    assert require_field(br_cfg, "kind", expected_type=str) == "member"
    assert require_field(br_cfg, "agent_id", "agentId", expected_type=str) == a2

    ws_repo = str(env.root / "repo")
    ws_sub = str(env.root / "repo" / "crate_a")
    post_res = env.run_cli_json(
        "channel",
        "post",
        "--channel-id",
        cid,
        "--text",
        "@everyone Verify SSO portal and crate_a",
        "--hold-running",
        "--resources-map",
        json.dumps({a1: [f"workspace:{ws_repo}"], a2: ["browser"], a3: ["browser", f"workspace:{ws_sub}"]}),
    )
    tasks = extract_list(post_res, "tasks")
    active = [t for t in tasks if pick(t, "state") in ("running", "starting")]
    assert len(active) <= 2
    wait_reasons = {
        pick(w, "reason")
        for t in tasks
        for w in (pick(t, "waits", default=[]) or [])
    }
    assert "resource-busy" in wait_reasons or "channel-full" in wait_reasons

    br_run = env.run_cli_json(
        "tools",
        "browser-run",
        "--agent-id",
        a2,
        "--url",
        "https://sso.corp.test/login",
        "--steps",
        json.dumps([{"action": "snapshot"}]),
    )
    tab_id = require_field(br_run, "tab_id", expected_type=str)
    vs = env.run_cli_json(
        "tools",
        "vault-secret",
        "--agent-id",
        a2,
        "--tab-id",
        tab_id,
        "--method",
        "password",
        "--login-id",
        "corp-sso",
        "--submission",
        "enter",
    )
    assert require_field(vs, "secret_document", expected_type=bool) is True
    assert "password_value" not in json.dumps(vs).lower()

    upd = unwrap_entity(
        env.run_cli_json(
            "channel", "update", "--channel-id", cid, "--members", f"{a1},{a3}", "--lead", a1
        ),
        "channel",
    )
    assert require_field(upd, "browser.kind", expected_type=str) == "none"


@e2e_test(
    id="TEST-T3-PAIR-04",
    tier=3,
    features=["OB-F08", "OB-F11", "OB-F13"],
    description="Inbound Telegram bridge triggers 12-node DAG workflow (all 12 WorkflowNodeKinds + approval hold) into Channel Lead routing and outbound reply.",
)
def test_t3_pair_04_telegram_bridge_12_node_workflow_and_channel_routing(env: TempOpenBotEnv) -> None:
    a1 = require_field(_mk_agent(env, "BridgeLead"), "id", expected_type=str)
    a2 = require_field(_mk_agent(env, "BridgeDev"), "id", expected_type=str)
    ch = _mk_channel(env, "TgOps", [a1, a2], lead=a1)
    cid = require_field(ch, "id", expected_type=str)

    inbound = env.run_cli_json(
        "platform",
        "bridge-sync",
        "--platform",
        "telegram",
        "--direction",
        "inbound",
        "--agent-id",
        a1,
        "--payload",
        json.dumps({"chat_id": "-1007788", "message_id": "401", "text": "Hotfix v2 severity=P0"}),
    )
    assert require_field(inbound, "delivered", expected_type=bool) is True
    mapped_tid = require_field(inbound, "mapped_thread_id", expected_type=str)
    assert len(mapped_tid) >= 4

    wf_spec = _build_12_node_workflow(a1, channel_id=cid, title="TgOps Hotfix Workflow")
    assert {n["kind"] for n in wf_spec["nodes"]} == set(_EXACT_12_WORKFLOW_NODE_KINDS)
    wf_saved = unwrap_entity(
        env.run_cli_json("platform", "workflow-save", "--spec", json.dumps(wf_spec)),
        "workflow",
    )
    wf_id = require_field(wf_saved, "id", "workflow_id", expected_type=str)
    assert require_field(wf_saved, "node_count", expected_type=int) == 12
    assert require_field(wf_saved, "step_count", expected_type=int) == 9

    run_hold = env.run_cli_json(
        "platform",
        "workflow-run",
        "--workflow-id",
        wf_id,
        "--trigger-payload",
        json.dumps({"severity": "P0", "text": "Hotfix v2"}),
        "--condition-mock",
        json.dumps({"n-cond": "yes"}),
    )
    assert require_field(run_hold, "status", expected_type=str) in ("awaiting_approval", "approval")

    run_done = env.run_cli_json(
        "platform", "workflow-run", "--workflow-id", wf_id, "--approve-node", "n-appr"
    )
    assert require_field(run_done, "status", expected_type=str) in ("completed", "done")

    routed = env.run_cli_json(
        "channel",
        "post",
        "--channel-id",
        cid,
        "--text",
        "Deploy approved Hotfix v2",
        "--router-mock",
        json.dumps({"agentIds": ["m2"]}),
    )
    t_dev = extract_list(routed, "tasks")[0]
    assert require_field(t_dev, "owner_agent_id", expected_type=str) == a2
    env.run_cli_json(
        "channel",
        "result",
        "--channel-id",
        cid,
        "--task-id",
        require_field(t_dev, "id", expected_type=str),
        "--agent-id",
        a2,
        "--text",
        "Hotfix v2 deployed cleanly",
    )

    outbound = env.run_cli_json(
        "platform",
        "bridge-sync",
        "--platform",
        "telegram",
        "--direction",
        "outbound",
        "--agent-id",
        a1,
        "--payload",
        json.dumps({"chat_id": "-1007788", "text": "Hotfix v2 deployed cleanly"}),
    )
    assert require_field(outbound, "delivered", expected_type=bool) is True


@e2e_test(
    id="TEST-T3-PAIR-05",
    tier=3,
    features=["OB-F11", "OB-F19", "OB-F20"],
    timeout_sec=45.0,
    description="HMAC-SHA256 webhook routine, quiet no_update suppression, loopback /run CSRF/token/30-hr rate cap (--allow-automation true), and DynamicIsland lock.",
)
def test_t3_pair_05_webhook_routine_automation_rate_limit_and_dynamic_island(env: TempOpenBotEnv) -> None:
    aid = require_field(
        _mk_agent(env, "AutoHookBot", allow_automation="true"), "id", expected_type=str
    )
    secret = "wh_pair05_secret_key"
    rt = env.run_cli_json(
        "platform",
        "routine-save",
        "--agent-id",
        aid,
        "--name",
        "WebhookMonitor",
        "--schedule",
        "webhook",
        "--quiet-policy",
        "no_update",
        "--webhook-secret",
        secret,
    )
    rid = require_field(rt, "id", "routine_id", expected_type=str)

    raw_payload = json.dumps({"event": "deploy.health", "ok": True}, separators=(",", ":"))
    sig = webhook_hmac_sha256_oracle(secret, raw_payload)
    wh_res = env.run_cli_json(
        "platform",
        "webhook-post",
        "--routine-id",
        rid,
        "--payload",
        raw_payload,
        "--signature",
        sig,
    )
    assert require_field(wh_res, "webhook_verified", expected_type=bool) is True

    quiet_res = env.run_cli_json(
        "platform", "routine-run", "--routine-id", rid, "--quiet-no-update"
    )
    assert require_field(quiet_res, "status", expected_type=str) == "quiet_suppressed"
    assert require_field(quiet_res, "notified", expected_type=bool) is False

    srv = env.start_server()
    st_no_tok, _, _ = srv.http_automation(
        "POST", "/run", payload={"agentId": aid, "routineId": rid, "payload": "ping"}, token=None
    )
    assert st_no_tok == 401
    st_csrf, _, _ = srv.http_automation(
        "POST",
        "/run",
        payload={"agentId": aid, "routineId": rid, "payload": "ping"},
        include_origin="https://evil.example",
    )
    assert st_csrf == 403

    for run_idx in range(1, 31):
        st_ok, _, _ = srv.automation_run(aid, routine_id=rid, payload=f"tick-{run_idx}")
        assert st_ok in (200, 202), f"Expected 200/202 on run {run_idx}, got {st_ok}"
    st_cap, body_cap, _ = srv.automation_run(aid, routine_id=rid, payload="tick-31")
    assert st_cap == 429
    cap_err = unwrap_entity(body_cap, "error")
    assert require_field(cap_err, "limit_per_hour", "limitPerHour", expected_type=int) == 30

    os_probe = env.run_cli_json(
        "ui-probe",
        "--scenario",
        json.dumps(
            {
                "scenario": "desktop-os-surfaces",
                "island_current": {"mode": "approval", "serverId": "local", "requestId": "req-1"},
                "island_next": {"mode": "message", "serverId": "local", "requestId": "msg-2"},
                "island_interacting": True,
            }
        ),
    )
    oss = unwrap_entity(os_probe, "os_surfaces", "openbot_shell")
    di = require_field(oss, "dynamic_island", expected_type=dict)
    assert require_field(di, "width_px", expected_type=int) == 192
    assert require_field(di, "height_px", expected_type=int) == 32
    assert require_field(di, "locked", "critical_lock", expected_type=bool) is True
    ad = require_field(oss, "agent_dock", expected_type=dict)
    assert require_field(ad, "tuck_delay_ms", expected_type=int) == 900
    assert require_field(ad, "strip_px", expected_type=int) == 4


@e2e_test(
    id="TEST-T3-PAIR-06",
    tier=3,
    features=["OB-F05", "OB-F14", "OB-F20"],
    description="Custom JSON theme hot-reload, OKLab chat color, and coast ChatBackdrop isolate cleanly across OpenBot <-> Telegram Chats switches.",
)
def test_t3_pair_06_theme_hot_reload_oklab_backdrop_and_workspace_switch(env: TempOpenBotEnv) -> None:
    t_eval = env.run_cli_json(
        "theme-eval", "--mode", "dark", "--backdrop", "coast", "--chat-color", "#4699fe", "--persist"
    )
    tokens = require_field(t_eval, "tokens", expected_type=dict)
    assert len(tokens) >= 52
    got_dark_bg = require_field(
        t_eval, "chat_background.dark", "readable_chat_bg", expected_type=str
    ).lower()
    assert got_dark_bg == oklab_readable_chat_bg_oracle("#4699fe", "dark")
    bd = require_field(t_eval, "backdrop", expected_type=dict)
    assert require_field(bd, "base_hex", expected_type=str).lower() == "#1a1030"

    latte = json.dumps(
        {
            "name": "Catppuccin Latte",
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
    env.write_fixture("data/themes/catppuccin-latte.json", latte)
    env.write_fixture("config/OpenBot/themes/catppuccin-latte.json", latte)

    hr = unwrap_entity(env.run_cli_json("ui-probe", "--scenario", "theme-hot-reload"), "hot_reload")
    assert require_field(hr, "watcher_triggered", "reloaded", expected_type=bool) is True
    assert require_field(hr, "active_canvas", expected_type=str).lower() == "#eff1f5"

    ws = env.run_cli_json("ui-probe", "--scenario", "workspace-switch-theme-isolation")
    sh = unwrap_entity(ws, "openbot_shell")
    pages = pick(sh, "page_rail_pages")
    page_cnt = len(pages) if isinstance(pages, list) else require_field(sh, "page_rail_count", expected_type=int)
    assert page_cnt == 11
    assert require_field(sh, "sidebar_width_px", expected_type=int) == 280
    assert require_field(sh, "sidebar_clamp_px", expected_type=list) == [128, 400]

    tb = require_field(ws, "telegram_baseline", expected_type=dict)
    assert require_field(tb, "nav_rail_px", expected_type=int) == 52
    assert require_field(tb, "chat_sidebar_px", expected_type=int) == 288
    assert require_field(tb, "inspector_px", expected_type=int) == 320
    assert require_field(tb, "composer_min_max_px", expected_type=list) == [44, 192]
    assert require_field(tb, "shortcuts_preserved", expected_type=bool) is True
    assert require_field(tb, "palette_untouched", expected_type=bool) is True


@e2e_test(
    id="TEST-T3-PAIR-07",
    tier=3,
    features=["OB-F10", "OB-F15", "OB-F19"],
    description="ask_ui form/Vault redaction, one-time link-payment, fill-card screenshot/evaluate exfiltration lock, post-navigation unlock, and BrowserDock.",
)
def test_t3_pair_07_ask_ui_vault_link_payment_fill_card_lock_and_browser_dock(env: TempOpenBotEnv) -> None:
    conv_probe = env.run_cli_json(
        "ui-probe",
        "--scenario",
        json.dumps(
            {
                "scenario": "conversation-virtualized",
                "ui_block_spec": {
                    "type": "form",
                    "title": "Checkout Vault",
                    "fields": [
                        {"id": "api_key", "kind": "text", "label": "Token", "secret": True},
                        {"id": "tier", "kind": "segmented", "label": "Tier", "options": ["team", "enterprise"]},
                    ],
                },
                "ui_block_response": {"actionId": "submit", "values": {"api_key": "sk-secret-pair07", "tier": "enterprise"}},
            }
        ),
    )
    conv = unwrap_entity(conv_probe, "conversation")
    ub = require_field(conv, "ui_block", expected_type=dict)
    assert require_field(ub, "valid", expected_type=bool) is True
    assert require_field(ub, "secret_redacted_in_storage", expected_type=bool) is True

    aid = require_field(_mk_agent(env, "CheckoutBot"), "id", expected_type=str)
    br = env.run_cli_json(
        "tools",
        "browser-run",
        "--agent-id",
        aid,
        "--url",
        "https://pay.vendor.test/checkout",
        "--steps",
        json.dumps([{"action": "snapshot"}]),
    )
    tab_id = require_field(br, "tab_id", expected_type=str)

    ctx_text = (
        "Authorize one-time virtual card for enterprise artifact storage renewal on vendor checkout portal "
        "verified by procurement ticket #4021."
    )
    assert len(ctx_text) >= 100
    lp = env.run_cli_json(
        "tools",
        "link-payment",
        "--agent-id",
        aid,
        "--amount",
        "4900",
        "--currency",
        "USD",
        "--merchant-name",
        "VendorCloud",
        "--merchant-url",
        "https://pay.vendor.test/checkout",
        "--context",
        ctx_text,
        "--approve",
    )
    card_id = require_field(lp, "card_id", expected_type=str)

    fc = env.run_cli_json(
        "tools", "fill-card", "--agent-id", aid, "--tab-id", tab_id, "--card-id", card_id, "--approve"
    )
    assert require_field(fc, "filled_card", expected_type=bool) is True
    assert "4111" not in json.dumps(fc)

    for blocked in ("openbot_browser.screenshot", "openbot_browser.evaluate"):
        env.run_cli_expect_rejection(
            "tools",
            "call",
            "--agent-id",
            aid,
            "--tab-id",
            tab_id,
            "--name",
            blocked,
            "--arguments",
            json.dumps({"tabId": tab_id}),
            error_contains=("filled card", "unavailable", "navigate"),
        )

    unlocked = env.run_cli_json(
        "tools",
        "browser-run",
        "--agent-id",
        aid,
        "--tab-id",
        tab_id,
        "--url",
        "https://pay.vendor.test/receipt",
        "--steps",
        json.dumps([{"action": "navigate", "url": "https://pay.vendor.test/receipt"}, {"action": "screenshot"}]),
    )
    assert require_field(unlocked, "tab_id", expected_type=str) == tab_id

    os_probe = env.run_cli_json(
        "ui-probe",
        "--scenario",
        json.dumps({"scenario": "desktop-os-surfaces", "browser_dock_island": {"kind": "secret"}, "browser_dock_elapsed_ms": 9000}),
    )
    oss = unwrap_entity(os_probe, "os_surfaces", "openbot_shell")
    assert "BrowserDock" in extract_list(oss, "surfaces")


@e2e_test(
    id="TEST-T3-PAIR-08",
    tier=3,
    features=["OB-F06", "OB-F07", "OB-F10"],
    description="Portable .openbot bundle export/import round-trips multi-agent profiles, SKILL.md, MCP registration, FTS5 memory, and backupModels failover.",
)
def test_t3_pair_08_portable_archive_skills_memory_and_failover(env: TempOpenBotEnv) -> None:
    backups = json.dumps(
        {
            "automatic": True,
            "order": [
                {"provider": "claude", "model": "claude-haiku-5-5"},
                {"provider": "deepseek", "model": "deepseek-v4-pro"},
            ],
        }
    )
    a1 = _mk_agent(env, "PortableLead", backup_models=backups, title="Failover Lead")
    a2 = _mk_agent(env, "PortableGrok", provider="grok", title="Grok Analyst")
    id1 = require_field(a1, "id", expected_type=str)
    id2 = require_field(a2, "id", expected_type=str)

    sk = env.run_cli_json(
        "tools",
        "skill-save",
        "--agent-id",
        id1,
        "--slug",
        "crate-bench",
        "--name",
        "Crate Bench",
        "--description",
        "Run deterministic benchmarks",
        "--instructions",
        "# Bench\nRun cargo bench.",
    )
    assert require_field(sk, "assigned_skill_count", expected_type=int) == 1
    mcp = unwrap_entity(
        env.run_cli_json(
            "tools", "mcp-register", "--name", "git-stdio", "--transport", "stdio", "--command", "echo"
        ),
        "mcp_server",
    )
    assert require_field(mcp, "transport", expected_type=str) == "stdio"
    env.run_cli_json(
        "tools", "memory-put", "--agent-id", id1, "--text", "Portable failover token 8829", "--inclusion", "searchable"
    )

    bundle = env.root / "portable_pair08.openbot"
    exp = env.run_cli_json("agent", "export", "--ids", f"{id1},{id2}", "--out", str(bundle))
    assert bundle.exists() and require_field(exp, "exported_count", "count", expected_type=int) == 2

    with TempOpenBotEnv() as env2:
        imp = env2.run_cli_json("agent", "import", "--archive", str(bundle), "--keys", "all")
        imp_agents = extract_list(imp, "agents")
        assert len(imp_agents) == 2
        by_name = {require_field(ag, "name", expected_type=str): ag for ag in imp_agents}
        imp_id1 = require_field(by_name["PortableLead"], "id", expected_type=str)

        mem_hits = extract_list(
            env2.run_cli_json("tools", "memory-search", "--agent-id", imp_id1, "--query", "8829"),
            "memories",
            "results",
        )
        assert len(mem_hits) >= 1

        failover = env2.run_cli_json(
            "chat", "send", "--agent-id", imp_id1, "--text", "Continue benchmark", "--simulate-primary-rate-limit"
        )
        assert require_field(failover, "active_provider", "provider", expected_type=str) == "claude"
        ulr = require_field(failover, "usage_limit_return", expected_type=dict)
        assert require_field(ulr, "provider", expected_type=str) == "codex"
        assert "Continue this OpenBot conversation" in require_field(failover, "handoff_prompt", expected_type=str)


@e2e_test(
    id="TEST-T3-PAIR-09",
    tier=3,
    features=["OB-F01", "OB-F03", "OB-F05"],
    description="'goutte' index-7 replacement pairs with reduced-motion BloubBot frozenAt=0, backdrop sweep suppression, horizon culling, and departFige continuity.",
)
def test_t3_pair_09_goutte_replacement_reduced_motion_and_morph_invariants(env: TempOpenBotEnv) -> None:
    assert stable_index("agent-0:shape", 11) == 7
    d_seed = env.run_cli_json("avatar-eval", "--seed", "agent-0")
    prof_src = pick(d_seed, "profile") if isinstance(pick(d_seed, "profile"), dict) else d_seed
    exp_seed = resolve_bloub_avatar_profile_oracle("agent-0")
    assert require_field(prof_src, "shape", expected_type=str) == exp_seed["shape"] != "goutte"

    live_av = env.run_cli_json("avatar-eval", "--seed", "agent-0", "--style", "flat", "--t", "0.85")
    assert math.isclose(float(require_field(live_av, "breath_sy", expected_type=(int, float))), 1.005, abs_tol=1e-3)
    assert require_field(live_av, "eyes.wander_active", "wander_active", expected_type=bool) is True

    rm_t0 = env.run_cli_json("avatar-eval", "--seed", "agent-0", "--style", "flat", "--reduced-motion", "--t", "0.0")
    rm_t1 = env.run_cli_json("avatar-eval", "--seed", "agent-0", "--style", "flat", "--reduced-motion", "--t", "0.85")
    assert require_field(rm_t0, "body_path", expected_type=str) == require_field(rm_t1, "body_path", expected_type=str)
    assert math.isclose(float(require_field(rm_t1, "breath_sy", expected_type=(int, float))), 1.0, abs_tol=1e-6)

    th_rm = env.run_cli_json("theme-eval", "--mode", "dark", "--backdrop", "pond", "--reduced-motion")
    assert require_field(th_rm, "backdrop.animated", expected_type=bool) is False
    assert require_field(th_rm, "transition.duration_ms", expected_type=int) == 0

    hz = unwrap_entity(env.run_cli_json("ui-probe", "--scenario", "avatar-horizon-cull"), "horizon_gaze")
    assert require_field(hz, "culled_at_0_02", expected_type=bool) is True
    mm = unwrap_entity(env.run_cli_json("ui-probe", "--scenario", "avatar-mid-morph"), "mid_morph")
    assert require_field(mm, "snapped_to_rest", expected_type=bool) is False


@e2e_test(
    id="TEST-T3-PAIR-10",
    tier=3,
    features=["OB-F02", "OB-F06", "OB-F14"],
    description="AI generate-profile welcome card, idempotent .openbot-stage duplicate, CQRS orchestration_command_receipts, and shell layout stores stay coherent.",
)
def test_t3_pair_10_generate_profile_idempotent_duplicate_cqrs_and_shell(env: TempOpenBotEnv) -> None:
    gen = env.run_cli_json(
        "agent",
        "generate-profile",
        "--prompt",
        "Rust fuzzing and formal verification bot",
        "--save",
        "--operation-id",
        "aaaaaaaa-1111-4111-8111-111111111111",
    )
    draft = require_field(gen, "draft", expected_type=dict)
    assert len(require_field(draft, "name", expected_type=str)) >= 1
    ag = unwrap_entity(require_field(gen, "agent", expected_type=dict), "agent")
    aid = require_field(ag, "id", expected_type=str)
    ws_path = Path(require_field(ag, "workspace_path", expected_type=str))

    q_items = extract_list(env.run_cli_json("chat", "queue-list", "--agent-id", aid), "items", "queue")
    assert any(pick(it, "welcome") is True for it in q_items)

    dup_op = "bbbbbbbb-2222-4222-8222-222222222222"
    dup1 = unwrap_entity(
        env.run_cli_json("agent", "duplicate", "--id", aid, "--operation-id", dup_op), "agent"
    )
    dup2 = unwrap_entity(
        env.run_cli_json("agent", "duplicate", "--id", aid, "--operation-id", dup_op), "agent"
    )
    dup_id = require_field(dup1, "id", expected_type=str)
    assert dup_id != aid and require_field(dup2, "id", expected_type=str) == dup_id
    assert list(ws_path.parent.glob("*.openbot-stage")) == []

    proj_rows = env.query_sqlite("SELECT * FROM projection_agents")
    proj_ids = {str(r.get("agent_id") or r.get("id")) for r in proj_rows}
    assert proj_ids == {aid, dup_id}
    ev_rows = env.query_sqlite("SELECT * FROM orchestration_events")
    assert len(ev_rows) >= 2
    env.check_sqlite_integrity("openbot")

    sh_probe = unwrap_entity(env.run_cli_json("ui-probe", "--scenario", "openbot-shell"), "openbot_shell")
    assert require_field(sh_probe, "sidebar_width_px", expected_type=int) == 280
    assert require_field(sh_probe, "sidebar_compact_px", expected_type=int) == 88


@e2e_test(
    id="TEST-T3-PAIR-11",
    tier=3,
    features=["OB-F03", "OB-F04", "OB-F19"],
    description="PNG & animated GIF <=512KB WebP normalization pairs with 240Hz vs reduced-motion CrewPile, broken avatarUrl fallback, and cloe Recovery surface.",
)
def test_t3_pair_11_webp_normalizer_crew_pile_and_recovery_surface(env: TempOpenBotEnv) -> None:
    png_file = env.write_png_fixture("fixtures/pair11_1024x768.png", width=1024, height=768)
    gif_file = env.write_gif_fixture("fixtures/pair11_anim.gif", frames=4)

    n_png = require_field(
        env.run_cli_json("avatar-eval", "--seed", "cloe", "--normalize-image", str(png_file)),
        "normalized_image",
        expected_type=dict,
    )
    assert require_field(n_png, "mime", expected_type=str) == "image/webp"
    assert require_field(n_png, "byte_length", "normalized_bytes", expected_type=int) <= 524288
    assert require_field(n_png, "magic", expected_type=str).startswith("RIFF")

    n_gif = require_field(
        env.run_cli_json("avatar-eval", "--seed", "cloe", "--normalize-image", str(gif_file)),
        "normalized_image",
        expected_type=dict,
    )
    assert require_field(n_gif, "animated", expected_type=bool) is True
    assert require_field(n_gif, "byte_length", "normalized_bytes", expected_type=int) <= 524288

    cp_live = unwrap_entity(env.run_cli_json("ui-probe", "--scenario", "crew-pile-physics"), "crew_pile")
    assert require_field(cp_live, "step_hz", expected_type=int) == 240
    assert "cloe" in extract_list(cp_live, "characters", "cast")

    cp_rm = unwrap_entity(
        env.run_cli_json("ui-probe", "--scenario", "crew-pile-reduced-motion-and-broken-url"),
        "crew_pile_reduced",
    )
    assert math.isclose(float(require_field(cp_rm, "max_squash", expected_type=(int, float))), 0.0, abs_tol=1e-6)
    assert require_field(cp_rm, "all_faces_idle", expected_type=bool) is True
    assert require_field(cp_rm, "failed_url_fallback", expected_type=bool) is True

    os_rec = unwrap_entity(
        env.run_cli_json(
            "ui-probe",
            "--scenario",
            json.dumps({"scenario": "desktop-os-surfaces", "crash_timestamps_ms": [1000, 12000, 24000, 36000]}),
        ),
        "os_surfaces",
        "openbot_shell",
    )
    assert "Recovery" in extract_list(os_rec, "surfaces")
    rec_info = require_field(os_rec, "recovery", expected_type=dict)
    assert require_field(rec_info, "triggered", expected_type=bool) is True
    assert require_field(rec_info, "plush_character", expected_type=str) == "cloe"


@e2e_test(
    id="TEST-T3-PAIR-12",
    tier=3,
    features=["OB-F07", "OB-F15", "OB-F16"],
    description="Steerable QueuePanel (pause/add/reorder/resume/steer + routine steer guard) pairs with human-pin ownership, thread branch, and SidePanel splits.",
)
def test_t3_pair_12_queue_steer_routine_guard_human_pin_and_sidepanel(env: TempOpenBotEnv) -> None:
    aid = require_field(_mk_agent(env, "QueuePinBot"), "id", expected_type=str)
    p_res = env.run_cli_json("chat", "queue-pause", "--agent-id", aid)
    assert require_field(p_res, "paused", expected_type=bool) is True

    d1 = require_field(env.run_cli_json("chat", "queue-add", "--agent-id", aid, "--text", "Step A"), "delivery_id", expected_type=str)
    d2 = require_field(env.run_cli_json("chat", "queue-add", "--agent-id", aid, "--text", "Step B"), "delivery_id", expected_type=str)
    d3 = require_field(env.run_cli_json("chat", "queue-add", "--agent-id", aid, "--text", "Step C"), "delivery_id", expected_type=str)
    env.run_cli_json("chat", "queue-reorder", "--agent-id", aid, "--delivery-ids", f"{d3},{d1},{d2}")

    resumed = env.run_cli_json("chat", "queue-resume", "--agent-id", aid)
    assert require_field(resumed, "paused", expected_type=bool) is False
    turn_id = require_field(resumed, "active_turn_id", "turn_id", expected_type=str)

    st_ok = env.run_cli_json(
        "chat", "queue-steer", "--agent-id", aid, "--delivery-id", d1, "--expected-turn-id", turn_id
    )
    assert require_field(st_ok, "steered", expected_type=bool) is True

    env.run_cli_expect_rejection(
        "chat",
        "queue-steer",
        "--agent-id",
        aid,
        "--delivery-id",
        d2,
        "--expected-turn-id",
        "rt-turn-99",
        "--routine-turn",
        error_contains=("refused", "cannot steer", "not steerable", "routine turn", "routine_runs_turn"),
    )

    sent = env.run_cli_json("chat", "send", "--agent-id", aid, "--text", "Anchor design decision")
    mid = require_field(sent, "message_id", expected_type=str)
    pin_u = env.run_cli_json("chat", "pin", "--agent-id", aid, "--message-id", mid, "--pinned-by", "user")
    assert require_field(pin_u, "pinned", expected_type=bool) is True

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

    br_res = env.run_cli_json(
        "chat", "branch", "--agent-id", aid, "--message-id", mid, "--request-id", "req-pair12-branch"
    )
    assert require_field(br_res, "task_id", "thread_id", expected_type=str).startswith("openbot-thread-")

    comp = unwrap_entity(
        env.run_cli_json(
            "ui-probe",
            "--scenario",
            json.dumps({"scenario": "composer-queue-sidepanel", "side_splits": ["browser", "files"], "split_share": 0.5}),
        ),
        "composer_probe",
    )
    sp = require_field(comp, "side_panel", expected_type=dict)
    assert require_field(sp, "default_width_px", expected_type=int) == 360
    assert extract_list(sp, "active_splits") == ["browser", "files"]


@e2e_test(
    id="TEST-T3-PAIR-13",
    tier=3,
    features=["OB-F09", "OB-F17"],
    description="CHANNEL_ASSIGNMENT_LIMIT=8 9th-handoff pause surfaces in ChannelStoppedTasks, recovers via reassign, strips <OwnName>:, and bounds delta packet.",
)
def test_t3_pair_13_assignment_limit_stopped_tasks_reassign_and_delta_packet(env: TempOpenBotEnv) -> None:
    a1 = require_field(_mk_agent(env, "PingBot"), "id", expected_type=str)
    a2 = require_field(_mk_agent(env, "PongBot"), "id", expected_type=str)
    a3 = require_field(_mk_agent(env, "RescueBot"), "id", expected_type=str)
    ch = _mk_channel(env, "LoopRoom", [a1, a2, a3], lead=a1)
    cid = require_field(ch, "id", expected_type=str)

    post0 = env.run_cli_json("channel", "post", "--channel-id", cid, "--text", "@PingBot Stabilize build")
    t0 = extract_list(post0, "tasks")[0]
    tid = require_field(t0, "id", expected_type=str)
    req_mid = require_field(t0, "request_message_id", expected_type=str)

    cur_owner = a1
    for step in range(1, 9):
        nxt_owner = a2 if cur_owner == a1 else a1
        xf = unwrap_entity(
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
                f"Handoff {step}",
                "--source-message-ids",
                req_mid,
            ),
            "task",
        )
        assert require_field(xf, "assignment_count", expected_type=int) == step
        cur_owner = nxt_owner

    ninth_target = a2 if cur_owner == a1 else a1
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
        ninth_target,
        "--task",
        "Handoff 9 overflow",
        "--source-message-ids",
        req_mid,
        error_contains=("assignment limit", "continue or reassign", "8", "paused"),
    )

    ui_rec = unwrap_entity(
        env.run_cli_json(
            "ui-probe",
            "--scenario",
            json.dumps({"scenario": "channel-conversation-recovery", "channel_id": cid, "stopped_tasks": [{"id": tid, "owner_agent_id": cur_owner}]}),
        ),
        "channel_ui",
    )
    banner = require_field(ui_rec, "stopped_tasks_banner", expected_type=dict)
    assert extract_list(banner, "actions") == ["Continue", "Reassign", "Cancel"]

    re_task = unwrap_entity(
        env.run_cli_json(
            "channel", "reassign", "--channel-id", cid, "--task-id", tid, "--recipient-agent-id", a3
        ),
        "task",
    )
    assert require_field(re_task, "owner_agent_id", expected_type=str) == a3
    assert require_field(re_task, "assignment_count", expected_type=int) == 0
    assert require_field(re_task, "state", expected_type=str) == "queued"

    res_msg = env.run_cli_json(
        "channel",
        "result",
        "--channel-id",
        cid,
        "--task-id",
        tid,
        "--agent-id",
        a3,
        "--text",
        "**RescueBot report:** Fixed linker flags",
    )
    assert require_field(res_msg, "text", expected_type=str) == "Fixed linker flags"

    pkt = unwrap_entity(
        env.run_cli_json(
            "channel",
            "history-packet",
            "--channel-id",
            cid,
            "--task-id",
            tid,
            "--agent-id",
            a3,
            "--second-turn",
            "--simulate-large-transcript",
        ),
        "packet",
    )
    assert require_field(pkt, "kind", expected_type=str) == "delta"
    assert 0 < require_field(pkt, "summary_chars", expected_type=int) <= 12000


@e2e_test(
    id="TEST-T3-PAIR-14",
    tier=3,
    features=["OB-F07", "OB-F09", "OB-F13"],
    timeout_sec=45.0,
    description="Live openbot serve (/v1/agents, /v1/channels, /v1/events SSE) synchronizes with symmetric dm-<sha256> DMs and Status/Result/Evidence Mailbox.",
)
def test_t3_pair_14_live_team_api_sse_symmetric_dms_and_mailbox(env: TempOpenBotEnv) -> None:
    a1 = require_field(_mk_agent(env, "MailLead"), "id", expected_type=str)
    a2 = require_field(_mk_agent(env, "MailPeer"), "id", expected_type=str)
    ch = _mk_channel(env, "LiveSyncRoom", [a1, a2], lead=a1)
    cid = require_field(ch, "id", expected_type=str)

    srv = env.start_server()
    with srv.open_sse("/v1/events") as sse:
        ev_ready = sse.read_until(lambda e: e.event == "ready", timeout=6.0)
        assert ev_ready.event == "ready"

        st_post_ag, created_http, _ = srv.http_team(
            "POST", "/v1/agents", payload={"name": "LiveSseAgent", "provider": "codex"}
        )
        assert st_post_ag in (200, 201)
        sse_aid = require_field(
            unwrap_entity(created_http, "agent"), "id", "agent_id", expected_type=str
        )
        ev_ag = sse.read_until(lambda e: e.event == "agent.created", timeout=6.0)
        assert ev_ag.event == "agent.created" and sse_aid in ev_ag.data

        st_post_ch, created_ch_http, _ = srv.http_team(
            "POST",
            "/v1/channels",
            payload={"name": "SseRoom2", "members": [a1, a2], "leadAgentId": a1},
        )
        assert st_post_ch in (200, 201)
        sse_cid = require_field(
            unwrap_entity(created_ch_http, "channel"), "id", "channel_id", expected_type=str
        )
        ev_ch = sse.read_until(
            lambda e: e.event in ("channel.created", "channel.updated"), timeout=6.0
        )
        assert sse_cid in ev_ch.data

        env.run_cli_json(
            "channel", "post", "--channel-id", sse_cid, "--text", "@MailLead Sync SSE ping"
        )
        ev_msg = sse.read_until(lambda e: e.event == "message.posted", timeout=6.0)
        assert "Sync SSE ping" in ev_msg.data or sse_cid in ev_msg.data

        st_ags, ag_body, _ = srv.http_team("GET", "/v1/agents")
        st_chs, ch_body, _ = srv.http_team("GET", "/v1/channels")
        assert st_ags == 200 and len(extract_list(ag_body, "agents")) >= 3
        ch_ids_http = {
            pick(c, "id", "channel_id") for c in extract_list(ch_body, "channels")
        }
        assert st_chs == 200 and {cid, sse_cid}.issubset(ch_ids_http)

    dm1 = env.run_cli_json(
        "chat", "dm-send", "--sender", "alice", "--recipient", "bob", "--client-message-id", "dm-p14-1", "--text", "Sync ready?"
    )
    dm2 = env.run_cli_json(
        "chat", "dm-send", "--sender", "bob", "--recipient", "alice", "--client-message-id", "dm-p14-2", "--text", "Confirmed"
    )
    exp_dm_tid = dm_thread_id_oracle("alice", "bob")
    assert require_field(dm1, "thread_id", expected_type=str) == exp_dm_tid
    assert require_field(dm2, "thread_id", expected_type=str) == exp_dm_tid

    mb_req = env.run_cli_json(
        "chat",
        "mailbox-send",
        "--sender-agent-id",
        a1,
        "--recipient-agent-ids",
        a2,
        "--text",
        "Verify WAL checkpoint",
        "--expects-reply",
        "true",
    )
    req_mid = require_field(mb_req, "message_id", expected_type=str)
    sre_reply = "Status: done\nResult: WAL v69 healthy\nEvidence: quick_check ok"
    mb_rep = env.run_cli_json(
        "chat",
        "mailbox-send",
        "--sender-agent-id",
        a2,
        "--recipient-agent-ids",
        a1,
        "--text",
        sre_reply,
        "--reply-to",
        req_mid,
        "--expects-reply",
        "false",
    )
    assert require_field(mb_rep, "answer_optional", expected_type=bool) is True


@e2e_test(
    id="TEST-T3-PAIR-15",
    tier=3,
    features=["OB-F02", "OB-F12", "OB-F18"],
    description="13-table SQLite CRM Stripe sync, human-gated crm-propose (--commit-unapproved blocked, --approve committed), static site-publish, and CrmPage.",
)
def test_t3_pair_15_crm_stripe_sync_proposal_gate_site_publish_and_crm_page(env: TempOpenBotEnv) -> None:
    aid = require_field(_mk_agent(env, "CrmOpsBot"), "id", expected_type=str)
    stripe_fixture = json.dumps(
        {
            "customers": [{"id": "cus_p15_1", "name": "Nordic Labs", "email": "ap@nordic.test", "currency": "USD"}],
            "subscriptions": [{"id": "sub_p15_1", "customer": "cus_p15_1", "plan": "Enterprise", "amount_minor": 99000, "every": "month"}],
            "invoices": [{"id": "in_p15_1", "customer": "cus_p15_1", "amount_minor": 99000, "paid": True}],
        }
    )
    sync_res = env.run_cli_json("platform", "crm-sync", "--source", "stripe", "--fixture", stripe_fixture)
    assert require_field(sync_res, "tables_count", expected_type=int) == 13

    crm_tbls = env.query_sqlite(
        "SELECT name FROM sqlite_master WHERE type='table' AND name LIKE 'crm_%'"
    )
    assert len(crm_tbls) == 13

    cust_payload = json.dumps(
        {"name": "Acme Renewal", "currency": "USD", "domain": "acme.test", "billingEmail": "fin@acme.test", "statusId": "customer"}
    )
    prop = env.run_cli_json(
        "platform",
        "crm-propose",
        "--agent-id",
        aid,
        "--operation",
        "customer-create",
        "--payload",
        cust_payload,
    )
    pid = require_field(prop, "proposal_id", "id", expected_type=str)
    assert require_field(prop, "status", expected_type=str) in ("pending_approval", "pending")
    assert require_field(prop, "approval_required", expected_type=bool) is True

    env.run_cli_expect_rejection(
        "platform",
        "crm-propose",
        "--agent-id",
        aid,
        "--operation",
        "customer-create",
        "--payload",
        cust_payload,
        "--commit-unapproved",
        error_contains=("pending approval", "human approval", "approval", "person"),
    )

    approved = env.run_cli_json(
        "platform",
        "crm-propose",
        "--agent-id",
        aid,
        "--proposal-id",
        pid,
        "--operation",
        "customer-create",
        "--payload",
        cust_payload,
        "--approve",
    )
    assert require_field(approved, "status", expected_type=str) in ("committed", "approved")

    site_files = json.dumps([{"path": "index.html", "content": "<!doctype html><h1>CRM Portal</h1>"}])
    pub = env.run_cli_json(
        "platform", "site-publish", "--agent-id", aid, "--slug", "crm-portal-p15", "--files", site_files
    )
    assert require_field(pub, "expires_in_days", expected_type=int) == 30
    deleted = env.run_cli_json("platform", "site-delete", "--slug", "crm-portal-p15")
    assert require_field(deleted, "deleted", expected_type=bool) is True

    pages = unwrap_entity(env.run_cli_json("ui-probe", "--scenario", "workflow-canvas-pages"), "pages_probe")
    assert require_field(pages, "crm_page.tables_count", expected_type=int) == 13


@e2e_test(
    id="TEST-T3-PAIR-16",
    tier=3,
    features=["OB-F05", "OB-F12", "OB-F15"],
    description="Sandboxed html-render (openbotPage + CSP) pairs with jellyfish ChatBackdrop and credit-metered image/video generation in virtualized timeline.",
)
def test_t3_pair_16_sandboxed_html_render_jellyfish_backdrop_and_credit_media(env: TempOpenBotEnv) -> None:
    h_img = env.run_cli_json(
        "platform",
        "html-render",
        "--mode",
        "page",
        "--html",
        "<div class='card'>Interactive Telemetry</div>",
        "--backdrop",
        "jellyfish",
        "--generate-media",
        "image",
    )
    assert require_field(h_img, "sandboxed", expected_type=bool) is True
    assert require_field(h_img, "csp_enforced", expected_type=bool) is True
    assert require_field(h_img, "openbot_page", expected_type=bool) is True
    assert require_field(h_img, "media_kind", expected_type=str) == "image"

    h_vid = env.run_cli_json(
        "platform",
        "html-render",
        "--mode",
        "preview",
        "--html",
        "<section>Preview Reel</section>",
        "--backdrop",
        "jellyfish",
        "--generate-media",
        "video",
    )
    assert require_field(h_vid, "sandboxed", expected_type=bool) is True
    assert require_field(h_vid, "media_kind", expected_type=str) == "video"

    th = env.run_cli_json("theme-eval", "--mode", "dark", "--backdrop", "jellyfish")
    bd = require_field(th, "backdrop", expected_type=dict)
    assert require_field(bd, "base_hex", expected_type=str).lower() == "#0e1a2b"
    assert require_field(bd, "cell_px", expected_type=int) == 6
    assert math.isclose(
        float(require_field(bd, "calm_center_alpha", expected_type=(int, float))), 0.55, abs_tol=0.01
    )

    conv = unwrap_entity(
        env.run_cli_json("ui-probe", "--scenario", "conversation-virtualized"), "conversation"
    )
    assert require_field(conv, "stream_timings_ms.step_ms", expected_type=int) == 40


@e2e_test(
    id="TEST-T3-PAIR-17",
    tier=3,
    features=["OB-F06", "OB-F12", "OB-F18"],
    description="3-tab Marketplace ('fishing' zero-match fallback) pairs with SKILL.md installation and live localhost /v1/models auto-discovery.",
)
def test_t3_pair_17_marketplace_tabs_fishing_fallback_skills_and_model_probe(env: TempOpenBotEnv) -> None:
    for tab in ("agents", "apps"):
        m_res = env.run_cli_json("platform", "marketplace-search", "--tab", tab)
        assert require_field(m_res, "tab", expected_type=str) == tab
        assert isinstance(extract_list(m_res, "items"), list)

    empty_m = env.run_cli_json(
        "platform", "marketplace-search", "--tab", "skills", "--query", "zzzz_none_9999"
    )
    assert extract_list(empty_m, "items") == []
    assert require_field(empty_m, "alpha_fallback", expected_type=str) == "fishing"

    srv_models, models_url = _start_local_models_server(["qwen2.5-coder:32b"])
    try:
        c_res = env.run_cli_json(
            "agent", "create", "--name", "LocalDiscBot", "--probe-servers", models_url
        )
        ag = unwrap_entity(c_res, "agent")
        aid = require_field(ag, "id", expected_type=str)
        servers = extract_list(c_res, "detected_servers", "servers")
        assert len(servers) == 1
    finally:
        srv_models.shutdown()

    sk = env.run_cli_json(
        "tools",
        "skill-save",
        "--agent-id",
        aid,
        "--slug",
        "marketplace-linter",
        "--name",
        "Marketplace Linter",
        "--description",
        "Validate manifest schemas",
        "--instructions",
        "# Linter\nCheck JSON schema.",
    )
    assert require_field(sk, "assigned_skill_count", expected_type=int) == 1

    pp = unwrap_entity(env.run_cli_json("ui-probe", "--scenario", "workflow-canvas-pages"), "pages_probe")
    assert set(extract_list(pp, "marketplace.tabs")) == {"agents", "skills", "apps"}


@e2e_test(
    id="TEST-T3-PAIR-18",
    tier=3,
    features=["OB-F13", "OB-F16"],
    description="Voice subsystem (8 STT, 7 TTS, 25 WebRTC Live voices + voiceAgent attribution) pairs with Composer duplex VoicePill and @/$ token picker.",
)
def test_t3_pair_18_voice_stt_tts_webrtc_and_composer_voice_pill(env: TempOpenBotEnv) -> None:
    aid = require_field(_mk_agent(env, "VoiceStudioBot", provider="codex"), "id", expected_type=str)

    v_stt = env.run_cli_json(
        "platform", "voice-eval", "--mode", "stt", "--stt-provider", "parakeet-tdt-0.6b-v3"
    )
    assert require_field(v_stt, "stt_providers", expected_type=int) == 8

    v_tts = env.run_cli_json(
        "platform", "voice-eval", "--mode", "tts", "--tts-provider", "supertonic-3", "--agent-id", aid
    )
    assert require_field(v_tts, "tts_providers", expected_type=int) == 7
    assert require_field(v_tts, "voice_agent_id", expected_type=str) == aid

    v_live = env.run_cli_json(
        "platform", "voice-eval", "--mode", "live", "--live-voice", "juniper", "--agent-id", aid
    )
    assert require_field(v_live, "live_voices", expected_type=int) == 25
    assert require_field(v_live, "voice_agent_id", expected_type=str) == aid
    assert require_field(v_live, "webrtc_ready", expected_type=bool) is True

    comp = unwrap_entity(
        env.run_cli_json(
            "ui-probe",
            "--scenario",
            json.dumps(
                {
                    "scenario": "composer-queue-sidepanel",
                    "trigger": "@",
                    "query": "voi",
                    "in_channel": True,
                    "voice_pill": {"phase": "speaking", "talk_mode": "open"},
                }
            ),
        ),
        "composer_probe",
    )
    vp = require_field(comp, "voice_pill", expected_type=dict)
    assert require_field(vp, "bars", expected_type=int) == 12
    assert extract_list(comp, "at_sections") == ["agents", "files", "tabs", "threads", "messages"]


@e2e_test(
    id="TEST-T3-PAIR-19",
    tier=3,
    features=["OB-F11", "OB-F18"],
    description="WorkflowEngine rejects cyclic step graphs, executes 12-node DAG across approval hold, schedules 180s routine, and aligns WorkflowCanvas & RoutineCalendar.",
)
def test_t3_pair_19_acyclic_workflow_180s_routine_and_canvas_calendar_pages(env: TempOpenBotEnv) -> None:
    aid = require_field(_mk_agent(env, "CanvasBot"), "id", expected_type=str)
    cyclic_spec = {
        "title": "Invalid Graph",
        "nodes": [
            {"id": "n-a", "kind": "agent", "at": {"x": 100.0, "y": 100.0}, "fields": {"agentId": aid, "task": "Step A"}},
            {"id": "n-b", "kind": "llm", "at": {"x": 360.0, "y": 100.0}, "fields": {"prompt": "Step B"}},
        ],
        "edges": [
            {"id": "e-ab", "from": "n-a", "to": "n-b", "type": "text"},
            {"id": "e-ba", "from": "n-b", "to": "n-a", "type": "text"},
        ],
        "strokes": [],
    }
    env.run_cli_expect_error(
        "platform",
        "workflow-save",
        "--spec",
        json.dumps(cyclic_spec),
        error_contains=("cycle", "acyclic", "loop", "dag"),
    )

    valid_spec = _build_12_node_workflow(aid, title="Canvas 12-Node DAG")
    saved = unwrap_entity(
        env.run_cli_json("platform", "workflow-save", "--spec", json.dumps(valid_spec)),
        "workflow",
    )
    wf_id = require_field(saved, "id", "workflow_id", expected_type=str)
    assert require_field(saved, "node_count", expected_type=int) == 12
    assert require_field(saved, "step_count", expected_type=int) == 9

    r_hold = env.run_cli_json(
        "platform", "workflow-run", "--workflow-id", wf_id, "--condition-mock", json.dumps({"n-cond": "yes"})
    )
    assert require_field(r_hold, "status", expected_type=str) in ("awaiting_approval", "approval")
    r_done = env.run_cli_json(
        "platform", "workflow-run", "--workflow-id", wf_id, "--approve-node", "n-appr"
    )
    assert require_field(r_done, "status", expected_type=str) in ("completed", "done")

    rt = env.run_cli_json(
        "platform",
        "routine-save",
        "--agent-id",
        aid,
        "--name",
        "Cadence180s",
        "--schedule",
        json.dumps({"kind": "interval", "amount": 3, "unit": "minutes", "anchorAt": "2026-01-01T00:00:00Z"}),
    )
    assert require_field(rt, "interval_seconds", expected_type=int) == 180

    pp = unwrap_entity(
        env.run_cli_json(
            "ui-probe",
            "--scenario",
            json.dumps({"scenario": "workflow-canvas-pages", "tidy_layout": True, "calendar_view": "week"}),
        ),
        "pages_probe",
    )
    wc = require_field(pp, "workflow_canvas", expected_type=dict)
    assert set(extract_list(wc, "node_kinds")) == set(_EXACT_12_WORKFLOW_NODE_KINDS)
    assert require_field(wc, "step_kinds_count", expected_type=int) == 9
    assert set(extract_list(wc, "annotation_kinds")) == {"text", "note", "shape"}
    assert require_field(pp, "routine_calendar.window_days", expected_type=int) == 44


@e2e_test(
    id="TEST-T3-PAIR-20",
    tier=3,
    features=["OB-F01", "OB-F13", "OB-F20"],
    timeout_sec=45.0,
    description="Live /v1/health (schema v69) & /v1/capabilities (v1..v10, 18 providers, >=220 caps) pair with 4-platform bridges and workspace-routing.",
)
def test_t3_pair_20_team_capabilities_four_bridges_and_workspace_routing(env: TempOpenBotEnv) -> None:
    aid = require_field(_mk_agent(env, "MeshBridgeBot"), "id", expected_type=str)
    srv = env.start_server()

    st_h, body_h, _ = srv.http_team("GET", "/v1/health")
    assert st_h == 200
    assert require_field(body_h, "schema_version", expected_type=int) == 69

    st_c, body_c, _ = srv.http_team("GET", "/v1/capabilities")
    assert st_c == 200
    versions = extract_list(body_c, "api_versions")
    assert [f"v{i}" for i in range(1, 11)] == versions or set(versions) >= {f"v{i}" for i in range(1, 11)}
    provs = extract_list(body_c, "providers")
    assert set(provs) == set(BUILTIN_AGENT_PROVIDERS)
    caps = extract_list(body_c, "capabilities")
    assert len(caps) >= 220

    bridge_cases = [
        ("slack", ["--payload", json.dumps({"channel": "C01", "ts": "1700.1", "text": "Slack ping"})]),
        (
            "discord",
            [
                "--allowed-roles",
                json.dumps(["778899"]),
                "--payload",
                json.dumps({"guild_id": "G1", "channel_id": "D1", "roles": ["778899"], "text": "Discord ping"}),
            ],
        ),
        ("telegram", ["--payload", json.dumps({"chat_id": "-10055", "text": "Telegram ping"})]),
        (
            "whatsapp",
            [
                "--allowed-senders",
                json.dumps([{"kind": "contact", "id": "15550109999", "readOnly": False}]),
                "--payload",
                json.dumps({"sender": "15550109999", "isSelfChat": True, "text": "WhatsApp ping"}),
            ],
        ),
    ]
    for platform_name, extra_args in bridge_cases:
        in_res = env.run_cli_json(
            "platform", "bridge-sync", "--platform", platform_name, "--direction", "inbound", "--agent-id", aid, *extra_args
        )
        assert require_field(in_res, "delivered", expected_type=bool) is True
        out_res = env.run_cli_json(
            "platform",
            "bridge-sync",
            "--platform",
            platform_name,
            "--direction",
            "outbound",
            "--agent-id",
            aid,
            "--payload",
            json.dumps({"target": "peer-1", "text": f"Ack {platform_name}"}),
        )
        assert require_field(out_res, "delivered", expected_type=bool) is True

    ws = env.run_cli_json("ui-probe", "--scenario", "workspace-routing")
    tb = require_field(ws, "telegram_baseline", expected_type=dict)
    assert require_field(tb, "nav_rail_px", expected_type=int) == 52
    assert require_field(tb, "palette_untouched", expected_type=bool) is True


@e2e_test(
    id="TEST-T3-PAIR-21",
    tier=3,
    features=["OB-F14", "OB-F16", "OB-F19"],
    description="OpenBotWorkspaceShell sidebar clamp/compact, multi-split SidePanel, and ChatWindow/CUA OS surfaces pair with privilege self-widening guard.",
)
def test_t3_pair_21_sidebar_clamp_sidepanel_cua_and_privilege_confinement(env: TempOpenBotEnv) -> None:
    cua_id = require_field(_mk_agent(env, "CuaOperator", computer_use="true"), "id", expected_type=str)
    confined_id = require_field(
        _mk_agent(env, "ConfinedWorker", access="workspace", computer_use="false"), "id", expected_type=str
    )

    env.run_cli_expect_rejection(
        "tools",
        "call",
        "--agent-id",
        confined_id,
        "--name",
        "openbot.update_profile",
        "--arguments",
        json.dumps({"access": "full", "computerUse": True}),
        error_contains=("widen", "permission", "only the user", "cannot widen"),
    )

    sh_compact = unwrap_entity(
        env.run_cli_json("ui-probe", "--scenario", json.dumps({"scenario": "openbot-shell", "drag_sidebar_px": 105})),
        "openbot_shell",
    )
    assert require_field(sh_compact, "sidebar_compact_px", expected_type=int) == 88
    sh_max = unwrap_entity(
        env.run_cli_json("ui-probe", "--scenario", json.dumps({"scenario": "openbot-shell", "drag_sidebar_px": 520})),
        "openbot_shell",
    )
    assert require_field(sh_max, "sidebar_width_px", expected_type=int) == 400

    comp = unwrap_entity(
        env.run_cli_json(
            "ui-probe",
            "--scenario",
            json.dumps({"scenario": "composer-queue-sidepanel", "side_splits": ["browser", "settings"], "split_share": 0.4}),
        ),
        "composer_probe",
    )
    assert require_field(comp, "side_panel.max_splits", expected_type=int) == 2

    oss = unwrap_entity(
        env.run_cli_json(
            "ui-probe",
            "--scenario",
            json.dumps({"scenario": "desktop-os-surfaces", "chat_window_agent_id": cua_id, "cua_active": True}),
        ),
        "os_surfaces",
        "openbot_shell",
    )
    assert {"ChatWindow", "CUA"}.issubset(set(extract_list(oss, "surfaces")))


@e2e_test(
    id="TEST-T3-PAIR-22",
    tier=3,
    features=["OB-F07", "OB-F14", "OB-F15"],
    description="80% context auto-compaction, read-only side-question (<=20 turns), and emoji react + user pin stay consistent across history and shell.",
)
def test_t3_pair_22_compaction_side_question_reactions_pins_and_shell(env: TempOpenBotEnv) -> None:
    aid = require_field(_mk_agent(env, "SynthBot"), "id", expected_type=str)
    sent = env.run_cli_json("chat", "send", "--agent-id", aid, "--text", "Draft synthesis report")
    mid = require_field(sent, "message_id", expected_type=str)

    rx = env.run_cli_json("chat", "react", "--agent-id", aid, "--message-id", mid, "--emoji", "🚀", "--actor", "user")
    reactions = extract_list(rx, "reactions")
    assert any(pick(r, "emoji") == "🚀" for r in reactions)

    pin_res = env.run_cli_json("chat", "pin", "--agent-id", aid, "--message-id", mid, "--pinned-by", "user")
    assert require_field(pin_res, "pinned", expected_type=bool) is True

    hist_before = extract_list(env.run_cli_json("chat", "history", "--agent-id", aid), "messages")
    sq = env.run_cli_json(
        "chat",
        "side-question",
        "--agent-id",
        aid,
        "--question",
        "What is the complexity bound?",
        "--earlier",
        json.dumps([{"question": "Context?", "answer": "O(n log n)"}]),
    )
    assert len(require_field(sq, "answer", expected_type=str)) >= 1
    hist_after = extract_list(env.run_cli_json("chat", "history", "--agent-id", aid), "messages")
    assert len(hist_after) == len(hist_before)

    comp = unwrap_entity(
        env.run_cli_json("chat", "compact", "--agent-id", aid, "--simulate-usage", "85000/100000"),
        "compaction",
    )
    assert require_field(comp, "after_tokens", expected_type=int) < 85000

    sh = unwrap_entity(env.run_cli_json("ui-probe", "--scenario", "openbot-shell"), "openbot_shell")
    conv = unwrap_entity(env.run_cli_json("ui-probe", "--scenario", "conversation-virtualized"), "conversation")
    assert require_field(sh, "sidebar_width_px", expected_type=int) == 280
    assert require_field(conv, "stream_timings_ms.step_ms", expected_type=int) == 40


@e2e_test(
    id="TEST-T3-PAIR-23",
    tier=3,
    features=["OB-F03", "OB-F08", "OB-F16", "OB-F17"],
    description="Diacritic-insensitive @mention ranking, follow-up superseded=true continuation, mouse pointer gaze (-13.2/+16.5 deg), and responded mood align.",
)
def test_t3_pair_23_diacritic_mention_followup_superseded_and_pointer_gaze(env: TempOpenBotEnv) -> None:
    a_elodie = require_field(_mk_agent(env, "Élodie", avatar_seed="elodie-flat:flat:11"), "id", expected_type=str)
    a_peer = require_field(_mk_agent(env, "Marco"), "id", expected_type=str)
    ch = _mk_channel(env, "IntlReview", [a_elodie, a_peer], lead=a_elodie)
    cid = require_field(ch, "id", expected_type=str)

    ranked_res = env.run_cli_json(
        "channel", "list", "--rank-mentions", "--channel-id", cid, "--query", "elodie"
    )
    ranked = extract_list(ranked_res, "ranked_members", "members")
    assert len(ranked) >= 1
    assert pick(ranked[0], "agent_id", "id") == a_elodie
    assert 0 <= int(require_field(ranked[0], "rank", expected_type=int)) <= 6

    p1 = env.run_cli_json("channel", "post", "--channel-id", cid, "--text", "@Élodie Suggest string type")
    tid = require_field(extract_list(p1, "tasks")[0], "id", expected_type=str)
    r1 = env.run_cli_json(
        "channel", "result", "--channel-id", cid, "--task-id", tid, "--agent-id", a_elodie, "--text", "Use String"
    )
    m1_id = require_field(r1, "message_id", "id", expected_type=str)

    p2 = env.run_cli_json(
        "channel", "post", "--channel-id", cid, "--text", "Actually use Cow<str>", "--reply-to", m1_id
    )
    t2 = extract_list(p2, "tasks")[0]
    assert require_field(t2, "id", expected_type=str) == tid
    ch_get = env.run_cli_json("channel", "get", "--channel-id", cid)
    msgs = {require_field(m, "id", expected_type=str): m for m in extract_list(ch_get, "messages")}
    assert require_field(msgs[m1_id], "superseded", expected_type=bool) is True

    d_resp = env.run_cli_json("avatar-eval", "--seed", "agent-1", "--style", "plush", "--mood", "responded")
    assert pick(d_resp, "profile.expression", "expression") == "heureux"
    d_gaze = env.run_cli_json(
        "avatar-eval", "--seed", "agent-1", "--style", "flat", "--t", "1.2", "--pointer", "0.8,-0.5", "--pointer-type", "mouse"
    )
    assert math.isclose(float(require_field(d_gaze, "eyes.gaze.yaw", "gaze.yaw", expected_type=(int, float))), -13.2, abs_tol=0.25)
    assert math.isclose(float(require_field(d_gaze, "eyes.gaze.pitch", "gaze.pitch", expected_type=(int, float))), 16.5, abs_tol=0.25)
    assert require_field(d_gaze, "eyes.wander_active", "wander_active", expected_type=bool) is False


@e2e_test(
    id="TEST-T3-PAIR-24",
    tier=3,
    features=["OB-F04", "OB-F17", "OB-F18", "OB-F20"],
    description="Channel archive/restore read-only guard, 640x640 PNG->WebP avatar normalizer, 14/18 Settings sections, and Telegram baseline geometry align.",
)
def test_t3_pair_24_channel_archive_restore_webp_settings_and_baseline(env: TempOpenBotEnv) -> None:
    png_640 = env.write_png_fixture("fixtures/ch_icon_640.png", width=640, height=640)
    norm = require_field(
        env.run_cli_json("avatar-eval", "--seed", "shield-bot", "--normalize-image", str(png_640)),
        "normalized_image",
        expected_type=dict,
    )
    assert require_field(norm, "mime", expected_type=str) == "image/webp"
    assert require_field(norm, "byte_length", "normalized_bytes", expected_type=int) <= 524288

    a1 = require_field(_mk_agent(env, "ArchiveLead"), "id", expected_type=str)
    ch = _mk_channel(
        env, "SecArchive", [a1], lead=a1, icon=json.dumps({"name": "shield", "hue": 150})
    )
    cid = require_field(ch, "id", expected_type=str)

    arch = unwrap_entity(env.run_cli_json("channel", "archive", "--channel-id", cid), "channel")
    assert require_field(arch, "archived", expected_type=bool) is True
    env.run_cli_expect_rejection(
        "channel",
        "post",
        "--channel-id",
        cid,
        "--text",
        "Blocked post",
        error_contains=("archived", "read-only", "read_only", "restore"),
    )

    rest = unwrap_entity(env.run_cli_json("channel", "restore", "--channel-id", cid), "channel")
    assert require_field(rest, "archived", expected_type=bool) is False
    ok_post = env.run_cli_json("channel", "post", "--channel-id", cid, "--text", "Restored post ok")
    assert len(extract_list(ok_post, "tasks")) == 1

    pp = unwrap_entity(env.run_cli_json("ui-probe", "--scenario", "workflow-canvas-pages"), "pages_probe")
    assert len(extract_list(pp, "app_settings_tabs")) == 14
    assert len(extract_list(pp, "server_settings_sections")) == 18

    ws = env.run_cli_json("ui-probe", "--scenario", "workspace-routing")
    tb = require_field(ws, "telegram_baseline", expected_type=dict)
    assert require_field(tb, "nav_rail_px", expected_type=int) == 52
    assert require_field(tb, "chat_sidebar_px", expected_type=int) == 288
    assert require_field(tb, "inspector_px", expected_type=int) == 320
    assert require_field(tb, "composer_min_max_px", expected_type=list) == [44, 192]
