"""Tier 1 Happy-Path Opaque-Box E2E Tests for OB-F11 through OB-F15 (30 tests, 6/feature).

Covers:
  - OB-F11: Routines, 12-Node Workflows & Loopback Local Scripts API
  - OB-F12: CRM, Static Sites, Marketplace, HTML Widgets & Credit Media
  - OB-F13: Voice (STT/TTS/WebRTC Live Voice), Chat Bridges & /v1/* Team API
  - OB-F14: 1:1 OpenBot Shell, Rails, Reorderable Sidebar & Agent Studio
  - OB-F15: Virtualized Conversation, AgentSteps, TraceWaterfall & ask_ui
"""

from __future__ import annotations

import hashlib
import hmac
import json
import math
import os
import stat
from typing import Any, Dict, List, Sequence

from .harness import (
    BUILTIN_AGENT_PROVIDERS,
    TempOpenBotEnv,
    e2e_test,
    extract_list,
    pick,
    require_field,
    unwrap_entity,
    webhook_hmac_sha256_oracle,
)

_WORKFLOW_NODE_KINDS_12: Sequence[str] = (
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


def _build_12_node_workflow_spec(agent_id: str, channel_id: str) -> Dict[str, Any]:
    return {
        "title": "All-12-Kinds-DAG",
        "nodes": [
            {"id": "n_trig", "kind": "trigger", "at": {"x": 0.0, "y": 0.0}, "fields": {"trigger": "manual", "stepName": "Start"}},
            {"id": "n_prompt", "kind": "prompt", "at": {"x": 120.0, "y": -80.0}, "fields": {"text": "Summarize PR"}},
            {"id": "n_asset", "kind": "asset", "at": {"x": 120.0, "y": 80.0}, "fields": {"fileId": "asset-spec-01"}},
            {"id": "n_llm", "kind": "llm", "at": {"x": 280.0, "y": 0.0}, "fields": {"provider": "codex", "model": "gpt-6-luna", "prompt": "Classify"}},
            {"id": "n_agent", "kind": "agent", "at": {"x": 440.0, "y": 0.0}, "fields": {"agentId": agent_id, "task": "Review diff"}},
            {"id": "n_img", "kind": "image", "at": {"x": 440.0, "y": 140.0}, "fields": {"media": "image", "model": "gpt-image", "prompt": "Status card"}},
            {"id": "n_cond", "kind": "condition", "at": {"x": 600.0, "y": 0.0}, "fields": {"rule": "approved == true"}},
            {"id": "n_appr", "kind": "approval", "at": {"x": 760.0, "y": -60.0}, "fields": {"question": "Ship?"}},
            {"id": "n_out", "kind": "output", "at": {"x": 920.0, "y": 0.0}, "fields": {"target": "channel", "channelId": channel_id, "message": "Done"}},
            {"id": "n_txt", "kind": "text", "at": {"x": 0.0, "y": -180.0}, "fields": {"text": "Pipeline Header"}},
            {"id": "n_note", "kind": "note", "at": {"x": 280.0, "y": -180.0}, "fields": {"text": "Sticky note"}},
            {"id": "n_shape", "kind": "shape", "at": {"x": 600.0, "y": -180.0}, "size": {"width": 200.0, "height": 100.0}, "fields": {"text": "Gate Zone"}},
        ],
        "edges": [
            {"id": "e1", "from": "n_trig", "to": "n_llm", "type": "text"},
            {"id": "e2", "from": "n_prompt", "to": "n_llm", "type": "text"},
            {"id": "e3", "from": "n_asset", "to": "n_llm", "type": "file"},
            {"id": "e4", "from": "n_llm", "to": "n_agent", "type": "text"},
            {"id": "e5", "from": "n_agent", "to": "n_cond", "type": "text"},
            {"id": "e6", "from": "n_agent", "to": "n_img", "type": "text"},
            {"id": "e7", "from": "n_cond", "to": "n_appr", "type": "text", "branch": "yes"},
            {"id": "e8", "from": "n_cond", "to": "n_out", "type": "text", "branch": "no"},
            {"id": "e9", "from": "n_appr", "to": "n_out", "type": "text"},
            {"id": "e10", "from": "n_img", "to": "n_out", "type": "image"},
        ],
        "strokes": [],
    }


# ============================================================================
# OB-F11: Routines, 12-Node Workflows & Loopback Local Scripts API (6 Tests)
# ============================================================================


@e2e_test(
    id="TEST-T1-OB-F11-01",
    tier=1,
    features=["OB-F11"],
    description="Saves a 12-node DAG exercising all 12 WorkflowNodeKinds (9 step + 3 annotation) and verifies SQLite workflow tables.",
)
def test_t1_ob_f11_01_workflow_save_all_12_node_kinds_and_sqlite(env: TempOpenBotEnv) -> None:
    ag = _create_agent(env, "WfBuilder")
    aid = require_field(ag, "id", expected_type=str)
    ch = _create_channel(env, "WfOutRoom", [aid], lead=aid)
    cid = require_field(ch, "id", expected_type=str)

    spec = _build_12_node_workflow_spec(aid, cid)
    saved = unwrap_entity(
        env.run_cli_json("platform", "workflow-save", "--spec", json.dumps(spec)),
        "workflow",
    )
    wf_id = require_field(saved, "id", expected_type=str)
    assert require_field(saved, "node_count", expected_type=int) == 12
    assert require_field(saved, "step_count", expected_type=int) == 9
    assert require_field(saved, "edge_count", expected_type=int) == 10
    assert require_field(saved, "revision", expected_type=int) >= 1
    assert pick(saved, "loop_edges", default=[]) == []

    node_rows = env.query_sqlite("SELECT * FROM workflow_nodes")
    wf_nodes = [r for r in node_rows if (r.get("workflow_id") or wf_id) == wf_id]
    assert len(wf_nodes) == 12
    saved_kinds = {str(r.get("kind") or r.get("node_kind")) for r in wf_nodes}
    assert saved_kinds == set(_WORKFLOW_NODE_KINDS_12)
    edge_rows = env.query_sqlite("SELECT * FROM workflow_edges")
    assert len([r for r in edge_rows if (r.get("workflow_id") or wf_id) == wf_id]) == 10
    ver_rows = env.query_sqlite("SELECT * FROM workflow_versions")
    assert len(ver_rows) >= 1
    env.check_sqlite_integrity("openbot")


@e2e_test(
    id="TEST-T1-OB-F11-02",
    tier=1,
    features=["OB-F11"],
    description="Executes condition-branched workflow pausing at approval hold and resuming to completed on approve-node.",
)
def test_t1_ob_f11_02_workflow_condition_branch_and_approval_resume(env: TempOpenBotEnv) -> None:
    ag = _create_agent(env, "WfApprover")
    aid = require_field(ag, "id", expected_type=str)
    ch = _create_channel(env, "WfAlerts", [aid], lead=aid)
    cid = require_field(ch, "id", expected_type=str)
    spec = {
        "title": "PR-Gate-DAG",
        "nodes": [
            {"id": "t1", "kind": "trigger", "at": {"x": 0.0, "y": 0.0}, "fields": {"trigger": "github", "event": "pr.opened", "repositories": "nightly-labs/openbot"}},
            {"id": "c1", "kind": "condition", "at": {"x": 220.0, "y": 0.0}, "fields": {"rule": "pr.draft == false"}},
            {"id": "app1", "kind": "approval", "at": {"x": 440.0, "y": -60.0}, "fields": {"question": "Approve Telegram release note?"}},
            {"id": "out_tg", "kind": "output", "at": {"x": 660.0, "y": -60.0}, "fields": {"target": "telegram", "connectionId": "conn-tg-01", "placeId": "-10042", "action": "post", "message": "Ship PR"}},
            {"id": "out_ch", "kind": "output", "at": {"x": 660.0, "y": 80.0}, "fields": {"target": "channel", "channelId": cid, "message": "Draft PR skipped"}},
        ],
        "edges": [
            {"id": "e1", "from": "t1", "to": "c1", "type": "text"},
            {"id": "e2", "from": "c1", "to": "app1", "type": "text", "branch": "yes"},
            {"id": "e3", "from": "c1", "to": "out_ch", "type": "text", "branch": "no"},
            {"id": "e4", "from": "app1", "to": "out_tg", "type": "text"},
        ],
        "strokes": [],
    }
    wf = unwrap_entity(
        env.run_cli_json("platform", "workflow-save", "--spec", json.dumps(spec)), "workflow"
    )
    wf_id = require_field(wf, "id", expected_type=str)

    paused = unwrap_entity(
        env.run_cli_json(
            "platform",
            "workflow-run",
            "--workflow-id",
            wf_id,
            "--trigger-payload",
            json.dumps({"event": "pr.opened", "pr": {"number": 88, "draft": False}}),
            "--condition-mock",
            json.dumps({"c1": "yes"}),
        ),
        "workflow",
    )
    run_id = require_field(paused, "run_id", expected_type=str)
    assert require_field(paused, "status", expected_type=str) in ("awaiting_approval", "approval")
    paused_steps = {
        str(pick(s, "node_id")): str(pick(s, "status"))
        for s in pick(paused, "steps", default=[])
        if isinstance(s, dict)
    }
    assert paused_steps.get("out_tg") not in ("done", "completed")

    resumed = unwrap_entity(
        env.run_cli_json(
            "platform",
            "workflow-run",
            "--workflow-id",
            wf_id,
            "--run-id",
            run_id,
            "--approve-node",
            "app1",
        ),
        "workflow",
    )
    assert require_field(resumed, "status", expected_type=str) == "completed"
    outputs = require_field(resumed, "outputs", expected_type=dict)
    assert len(outputs) >= 1


@e2e_test(
    id="TEST-T1-OB-F11-03",
    tier=1,
    features=["OB-F11"],
    description="Saves interval (3m=180s), 5-field custom cron, and future once routines; once auto-pauses after firing.",
)
def test_t1_ob_f11_03_routine_schedules_and_once_autopause(env: TempOpenBotEnv) -> None:
    ag = _create_agent(env, "RoutineHost")
    aid = require_field(ag, "id", expected_type=str)

    rt_int = unwrap_entity(
        env.run_cli_json(
            "platform",
            "routine-save",
            "--agent-id",
            aid,
            "--name",
            "MinCadence3m",
            "--schedule",
            "every 180s",
        ),
        "routine",
    )
    assert require_field(rt_int, "interval_seconds", expected_type=int) == 180
    assert require_field(rt_int, "active", expected_type=bool) is True

    rt_cron = unwrap_entity(
        env.run_cli_json(
            "platform",
            "routine-save",
            "--agent-id",
            aid,
            "--name",
            "Cron5mWarsaw",
            "--schedule",
            "*/5 * * * *",
            "--timezone",
            "Europe/Warsaw",
        ),
        "routine",
    )
    assert require_field(rt_cron, "interval_seconds", expected_type=int) == 300

    once_spec = json.dumps({"kind": "once", "date": "2099-12-31", "time": "14:00"})
    rt_once = unwrap_entity(
        env.run_cli_json(
            "platform",
            "routine-save",
            "--agent-id",
            aid,
            "--name",
            "OnceDeploy",
            "--schedule",
            once_spec,
        ),
        "routine",
    )
    once_id = require_field(rt_once, "id", expected_type=str)
    run_res = unwrap_entity(
        env.run_cli_json("platform", "routine-run", "--routine-id", once_id, "--source", "scheduled"),
        "routine",
    )
    assert require_field(run_res, "status", expected_type=str) == "completed"

    rt_rows = env.query_sqlite("SELECT * FROM projection_agent_routines")
    once_rows = [r for r in rt_rows if (r.get("routine_id") or r.get("id")) == once_id]
    assert len(once_rows) == 1
    assert once_rows[0].get("active") in (0, False)
    run_rows = env.query_sqlite("SELECT * FROM projection_routine_runs")
    assert any((r.get("routine_id") or once_id) == once_id for r in run_rows)


@e2e_test(
    id="TEST-T1-OB-F11-04",
    tier=1,
    features=["OB-F11"],
    description="Verifies HMAC-SHA256 webhook routine delivery (sha256={ts}.{delivery_id}.{body} & bare hex) with RFC-6901 filters.",
)
def test_t1_ob_f11_04_webhook_hmac_sha256_and_rfc6901_filter(env: TempOpenBotEnv) -> None:
    ag = _create_agent(env, "WebhookBot")
    aid = require_field(ag, "id", expected_type=str)
    secret = "wh_sec_alpha_99"
    rt = unwrap_entity(
        env.run_cli_json(
            "platform",
            "routine-save",
            "--agent-id",
            aid,
            "--name",
            "IssueOpenedHook",
            "--schedule",
            "webhook",
            "--webhook-secret",
            secret,
            "--filters",
            json.dumps([{"pointer": "/action", "value": "opened"}]),
        ),
        "routine",
    )
    rt_id = require_field(rt, "id", expected_type=str)

    raw_body = json.dumps({"action": "opened", "issue": {"id": 501}}, separators=(",", ":"))
    ts = int(os.environ.get("OPENBOT_FIXED_TS", "0")) or int(__import__("time").time())
    deliv_id = "deliv-2026-0001"
    sig_prefixed = "sha256=" + hmac.new(
        secret.encode("utf-8"), f"{ts}.{deliv_id}.{raw_body}".encode("utf-8"), hashlib.sha256
    ).hexdigest()

    post_res = unwrap_entity(
        env.run_cli_json(
            "platform",
            "webhook-post",
            "--routine-id",
            rt_id,
            "--payload",
            raw_body,
            "--timestamp",
            str(ts),
            "--delivery-id",
            deliv_id,
            "--signature",
            sig_prefixed,
        ),
        "routine",
    )
    assert require_field(post_res, "webhook_verified", expected_type=bool) is True
    assert require_field(post_res, "matched_filters", expected_type=bool) is True

    bare_sig = webhook_hmac_sha256_oracle(secret, raw_body)
    bare_res = unwrap_entity(
        env.run_cli_json(
            "platform",
            "webhook-post",
            "--routine-id",
            rt_id,
            "--payload",
            raw_body,
            "--signature",
            bare_sig,
        ),
        "routine",
    )
    assert require_field(bare_res, "webhook_verified", expected_type=bool) is True
    runs = env.query_sqlite("SELECT * FROM projection_routine_runs")
    # Upstream v69 projection_routine_runs column is `run_source` (0035-routine-run-source.ts:L8)
    assert any(
        str(r.get("run_source") or "") == "webhook" and str(r.get("run_kind") or "") == "manual"
        for r in runs
    )


@e2e_test(
    id="TEST-T1-OB-F11-05",
    tier=1,
    features=["OB-F11"],
    description="Verifies routine_no_update quiet suppression vs manual test notification and <=20 await_event waits.",
)
def test_t1_ob_f11_05_quiet_no_update_and_await_events(env: TempOpenBotEnv) -> None:
    ag = _create_agent(env, "QuietWaitBot")
    aid = require_field(ag, "id", expected_type=str)
    rt = unwrap_entity(
        env.run_cli_json(
            "platform",
            "routine-save",
            "--agent-id",
            aid,
            "--name",
            "QuietWatch",
            "--schedule",
            "every 300s",
            "--quiet-policy",
            "no_update",
        ),
        "routine",
    )
    rt_id = require_field(rt, "id", expected_type=str)

    quiet_res = unwrap_entity(
        env.run_cli_json(
            "platform",
            "routine-run",
            "--routine-id",
            rt_id,
            "--source",
            "scheduled",
            "--quiet-no-update",
        ),
        "routine",
    )
    assert require_field(quiet_res, "status", expected_type=str) == "quiet_suppressed"
    assert require_field(quiet_res, "notified", expected_type=bool) is False
    assert require_field(quiet_res, "quiet_item_type", expected_type=str) == "routine_quiet_answer"

    waits_spec = [
        {"event": "ci.completed", "note": "Wait for Linux build"},
        {"event": "deploy.ready", "note": "Wait for canary health"},
        {"event": "audit.signed", "note": "Wait for QA sign-off"},
    ]
    manual_res = unwrap_entity(
        env.run_cli_json(
            "platform",
            "routine-run",
            "--routine-id",
            rt_id,
            "--source",
            "test",
            "--report-text",
            "All systems nominal",
            "--await-events",
            json.dumps(waits_spec),
        ),
        "routine",
    )
    assert require_field(manual_res, "notified", expected_type=bool) is True
    assert require_field(manual_res, "open_waits_count", expected_type=int) == 3
    wait_rows = env.query_sqlite("SELECT * FROM agent_waits")
    assert len([r for r in wait_rows if (r.get("agent_id") or aid) == aid]) == 3


@e2e_test(
    id="TEST-T1-OB-F11-06",
    tier=1,
    features=["OB-F11"],
    description="Starts openbot serve, verifies 0600 automation token/headers/url files inside 0700 dir, and executes loopback /run.",
)
def test_t1_ob_f11_06_loopback_automation_server_files_and_run(env: TempOpenBotEnv) -> None:
    ag = _create_agent(env, "AutoRunner", allow_automation="true")
    aid = require_field(ag, "id", expected_type=str)
    assert require_field(ag, "allow_automation", expected_type=bool) is True

    rt = unwrap_entity(
        env.run_cli_json(
            "platform",
            "routine-save",
            "--agent-id",
            aid,
            "--name",
            "LocalScriptRoutine",
            "--schedule",
            "every 180s",
        ),
        "routine",
    )
    rt_id = require_field(rt, "id", expected_type=str)

    srv = env.start_server()
    auto_dir = env.data_dir / "automation"
    assert auto_dir.is_dir()
    assert stat.S_IMODE(auto_dir.stat().st_mode) == 0o700
    for fname in ("token", "headers", "url"):
        fpath = auto_dir / fname
        assert fpath.is_file(), f"Missing automation file {fpath}"
        assert stat.S_IMODE(fpath.stat().st_mode) == 0o600
    assert (auto_dir / "token").read_text(encoding="utf-8").strip() == srv.automation_token

    st_ag, body_ag, _ = srv.http_automation("GET", "/v1/agents")
    assert st_ag == 200
    assert any(
        (pick(a, "id", "agent_id") == aid) for a in extract_list(body_ag, "agents")
    )

    st_run, body_run, _ = srv.automation_run(aid, routine_id=rt_id, payload="check health")
    assert st_run in (200, 202)
    assert len(require_field(body_run, "run_id", "runId", expected_type=str)) >= 4
    assert len(require_field(body_run, "delivery_id", "deliveryId", expected_type=str)) >= 4


# ============================================================================
# OB-F12: CRM, Static Sites, Marketplace, HTML Widgets & Credit Media (6 Tests)
# ============================================================================


@e2e_test(
    id="TEST-T1-OB-F12-01",
    tier=1,
    features=["OB-F12"],
    description="Syncs Stripe CRM fixture into all 13 SQLite crm_* tables with 3 built-in statuses and monotonic counters.",
)
def test_t1_ob_f12_01_crm_stripe_sync_and_13_sqlite_tables(env: TempOpenBotEnv) -> None:
    fixture = {
        "customers": [
            {"id": "stripe:cus_101", "name": "Acme Robotics", "email": "ops@acme.test", "status": "customer"}
        ],
        "plans": [{"id": "stripe:sub_201", "customerId": "stripe:cus_101", "amountMinor": 49000, "currency": "USD"}],
        "payments": [{"id": "stripe:in_301", "customerId": "stripe:cus_101", "amountMinor": 49000, "currency": "USD"}],
    }
    res = unwrap_entity(
        env.run_cli_json(
            "platform",
            "crm-sync",
            "--source",
            "stripe",
            "--api-key",
            "rk_test_restricted_valid_01",
            "--fixture",
            json.dumps(fixture),
        ),
        "crm",
    )
    assert require_field(res, "tables_count", expected_type=int) == 13
    assert set(extract_list(res, "builtin_statuses")) == {"lead", "customer", "former"}

    crm_tables = {
        str(r["name"])
        for r in env.query_sqlite(
            "SELECT name FROM sqlite_master WHERE type='table' AND name LIKE 'crm_%'"
        )
    }
    expected_13 = {
        "crm_counters",
        "crm_statuses",
        "crm_sources",
        "crm_source_ignored",
        "crm_customers",
        "crm_tags",
        "crm_customer_tags",
        "crm_plans",
        "crm_payments",
        "crm_activity",
        "crm_proposals",
        "crm_private_fields",
        "crm_agent_access",
    }
    assert crm_tables == expected_13
    statuses = env.query_sqlite("SELECT * FROM crm_statuses")
    builtin_ids = {str(r.get("status_id") or r.get("id")) for r in statuses if r.get("label") is None}
    assert {"lead", "customer", "former"}.issubset(builtin_ids)
    cust_rows = env.query_sqlite("SELECT * FROM crm_customers")
    # Upstream v69 crm_customers enforces CHECK(length(customer_id) = 32) and stores Stripe refs in stripe_customer_id/source_ref (0063-crm.ts:L62-L88)
    assert any(
        len(str(r.get("customer_id") or "")) == 32
        and str(r.get("stripe_customer_id") or "") == "cus_101"
        and str(r.get("source_ref") or "") == "stripe:cus_101"
        and bool(r.get("source_id"))
        for r in cust_rows
    )
    env.check_sqlite_integrity("openbot")


@e2e_test(
    id="TEST-T1-OB-F12-02",
    tier=1,
    features=["OB-F12"],
    description="Verifies crm-propose pending state, human --approve transactional commit, proposed_by audit, and private-field redaction.",
)
def test_t1_ob_f12_02_crm_propose_human_approval_and_private_redaction(env: TempOpenBotEnv) -> None:
    ag = _create_agent(env, "CrmAnalyst")
    aid = require_field(ag, "id", expected_type=str)
    env.run_cli_json(
        "platform",
        "crm-sync",
        "--source",
        "stripe",
        "--private-fields",
        "taxId,billingEmail,notes",
        "--for-agent-id",
        aid,
    )

    prop_payload = {
        "name": "Nordic Labs",
        "currency": "USD",
        "statusId": "lead",
        "billingEmail": "cfo@nordic.test",
        "taxId": "EU-998877",
        "notes": "Confidential discount tier",
    }
    pending = unwrap_entity(
        env.run_cli_json(
            "platform",
            "crm-propose",
            "--agent-id",
            aid,
            "--operation",
            "customer-create",
            "--payload",
            json.dumps(prop_payload),
            "--reason",
            "Inbound enterprise inquiry",
        ),
        "crm",
    )
    pid = require_field(pending, "proposal_id", expected_type=str)
    assert require_field(pending, "status", expected_type=str) in ("pending", "pending_approval")
    assert require_field(pending, "approval_required", expected_type=bool) is True
    assert env.query_sqlite("SELECT * FROM crm_customers") == []
    prop_rows = env.query_sqlite("SELECT * FROM crm_proposals")
    assert any((r.get("proposal_id") or r.get("id")) == pid and r.get("state") == "pending" for r in prop_rows)

    approved = unwrap_entity(
        env.run_cli_json(
            "platform", "crm-propose", "--agent-id", aid, "--proposal-id", pid, "--approve"
        ),
        "crm",
    )
    assert require_field(approved, "status", expected_type=str) == "committed"
    assert len(env.query_sqlite("SELECT * FROM crm_customers")) == 1
    act_rows = env.query_sqlite("SELECT * FROM crm_activity")
    assert any(str(r.get("proposed_by") or "") == aid for r in act_rows)

    view_sync = unwrap_entity(
        env.run_cli_json(
            "platform",
            "crm-sync",
            "--source",
            "stripe",
            "--private-fields",
            "taxId,billingEmail,notes",
            "--for-agent-id",
            aid,
        ),
        "crm",
    )
    agent_view = require_field(view_sync, "agent_view", expected_type=dict)
    redacted = set(extract_list(agent_view, "redacted_fields"))
    assert {"taxId", "billingEmail", "notes"}.issubset(redacted)
    dumped_customers = json.dumps(extract_list(agent_view, "customers"))
    assert "EU-998877" not in dumped_customers and "cfo@nordic.test" not in dumped_customers


@e2e_test(
    id="TEST-T1-OB-F12-03",
    tier=1,
    features=["OB-F12"],
    description="Publishes static site (30-day expiry), replaces files preserving URL, and deletes hosted site.",
)
def test_t1_ob_f12_03_static_site_publish_replace_and_delete(env: TempOpenBotEnv) -> None:
    ag = _create_agent(env, "SitePublisher")
    aid = require_field(ag, "id", expected_type=str)
    site_dir = env.workspace_dir / "Agents" / aid / "site_dist"
    site_dir.mkdir(parents=True, exist_ok=True)
    (site_dir / "index.html").write_text("<!doctype html><h1>Docs v1</h1>", encoding="utf-8")
    (site_dir / "styles.css").write_text("body { color: #111; }", encoding="utf-8")
    (site_dir / "app.js").write_text("console.log('v1');", encoding="utf-8")

    pub = unwrap_entity(
        env.run_cli_json(
            "platform",
            "site-publish",
            "--agent-id",
            aid,
            "--slug",
            "docs-v1",
            "--title",
            "Docs Portal",
            "--source-dir",
            str(site_dir),
            "--spa-fallback",
        ),
        "site",
    )
    sid = require_field(pub, "site_id", expected_type=str)
    url1 = require_field(pub, "url", expected_type=str)
    assert require_field(pub, "status", expected_type=str) == "active"
    assert require_field(pub, "file_count", expected_type=int) == 3
    assert require_field(pub, "expires_in_days", expected_type=int) == 30

    (site_dir / "index.html").write_text("<!doctype html><h1>Docs v2</h1>", encoding="utf-8")
    rep = unwrap_entity(
        env.run_cli_json(
            "platform",
            "site-publish",
            "--agent-id",
            aid,
            "--slug",
            "docs-v1",
            "--site-id",
            sid,
            "--replace",
            "--source-dir",
            str(site_dir),
        ),
        "site",
    )
    assert require_field(rep, "url", expected_type=str) == url1
    assert require_field(rep, "replaced", expected_type=bool) is True
    assert require_field(rep, "expires_in_days", expected_type=int) == 30

    deleted = unwrap_entity(
        env.run_cli_json(
            "platform", "site-delete", "--slug", "docs-v1", "--site-id", sid, "--agent-id", aid
        ),
        "site",
    )
    assert require_field(deleted, "deleted", expected_type=bool) is True


@e2e_test(
    id="TEST-T1-OB-F12-04",
    tier=1,
    features=["OB-F12"],
    description="Searches 3-tab Marketplace (apps, agents, skills) and verifies fishing.webm / fishing-still.webp empty-state fallback.",
)
def test_t1_ob_f12_04_marketplace_three_tabs_and_fishing_fallback(env: TempOpenBotEnv) -> None:
    for tab in ("apps", "agents", "skills"):
        res = unwrap_entity(
            env.run_cli_json("platform", "marketplace-search", "--tab", tab),
            "marketplace",
        )
        assert require_field(res, "tab", expected_type=str) == tab
        assert set(extract_list(res, "tabs")) == {"apps", "agents", "skills"}
        assert len(extract_list(res, "items")) >= 1

    empty_anim = unwrap_entity(
        env.run_cli_json(
            "platform", "marketplace-search", "--tab", "skills", "--query", "no-match-xyz-9999"
        ),
        "marketplace",
    )
    assert extract_list(empty_anim, "items") == []
    assert require_field(empty_anim, "empty_state", expected_type=bool) is True
    assert require_field(empty_anim, "alpha_fallback", expected_type=str) == "fishing"
    assert require_field(empty_anim, "plush_asset", expected_type=str) == "fishing.webm"

    for flag in ("--reduced-motion", "--alpha-unsupported"):
        empty_still = unwrap_entity(
            env.run_cli_json(
                "platform",
                "marketplace-search",
                "--tab",
                "skills",
                "--query",
                "no-match-xyz-9999",
                flag,
            ),
            "marketplace",
        )
        assert require_field(empty_still, "plush_asset", expected_type=str) == "fishing-still.webp"


@e2e_test(
    id="TEST-T1-OB-F12-05",
    tier=1,
    features=["OB-F12"],
    description="Evaluates sandboxed html-render across preview (no openbotPage), page (openbotPage JSON-RPC), and backdrop modes.",
)
def test_t1_ob_f12_05_sandboxed_html_render_preview_page_and_backdrop(env: TempOpenBotEnv) -> None:
    prev = unwrap_entity(
        env.run_cli_json(
            "platform",
            "html-render",
            "--mode",
            "preview",
            "--width",
            "360",
            "--html",
            "<section>Preview Card</section>",
        ),
        "html_media",
    )
    assert require_field(prev, "sandboxed", expected_type=bool) is True
    assert require_field(prev, "csp_enforced", expected_type=bool) is True
    assert require_field(prev, "openbot_page", expected_type=bool) is False
    assert require_field(prev, "preview_width", expected_type=int) == 360

    rpc_msg = {"jsonrpc": "2.0", "id": 1, "method": "ui/message", "params": {"text": "Ship widget"}}
    page = unwrap_entity(
        env.run_cli_json(
            "platform",
            "html-render",
            "--mode",
            "page",
            "--height",
            "320",
            "--html",
            "<main id='app'>Interactive Page</main>",
            "--rpc-message",
            json.dumps(rpc_msg),
        ),
        "html_media",
    )
    assert require_field(page, "openbot_page", expected_type=bool) is True
    assert "sandbox allow-scripts allow-forms" in require_field(page, "csp_header", expected_type=str)
    rpc_res = require_field(page, "rpc_result", expected_type=dict)
    assert require_field(rpc_res, "type", expected_type=str) == "message"

    bd = unwrap_entity(
        env.run_cli_json(
            "platform",
            "html-render",
            "--mode",
            "backdrop",
            "--backdrop",
            "pond",
            "--html",
            "<canvas id='scene'></canvas>",
            "--bubbles",
            json.dumps({"yours": "#3b93f0", "agents": "#212121"}),
        ),
        "html_media",
    )
    assert require_field(bd, "openbot_page", expected_type=bool) is False
    assert "openbot-bubbles" in str(require_field(bd, "bubbles_meta", expected_type=str))


@e2e_test(
    id="TEST-T1-OB-F12-06",
    tier=1,
    features=["OB-F12"],
    description="Verifies credit-metered generate_image (<=50 direct) and generate_video (>50 human-approved) wallet accounting.",
)
def test_t1_ob_f12_06_credit_media_generation_threshold_and_wallet(env: TempOpenBotEnv) -> None:
    img = unwrap_entity(
        env.run_cli_json(
            "platform",
            "html-render",
            "--generate-media",
            "image",
            "--media-model",
            "openbot-image-1",
            "--media-prompt",
            "Pixel lighthouse",
            "--media-cost",
            "30",
            "--wallet-balance",
            "200",
            "--approval-threshold",
            "50",
            "--agents-credits-allowed",
            "true",
        ),
        "html_media",
    )
    assert require_field(img, "approval_required", expected_type=bool) is False
    assert require_field(img, "outcome", expected_type=str) == "generated"
    assert require_field(img, "charged_credits", expected_type=int) == 30
    assert require_field(img, "remaining_balance", expected_type=int) == 170

    vid = unwrap_entity(
        env.run_cli_json(
            "platform",
            "html-render",
            "--generate-media",
            "video",
            "--media-model",
            "openbot-video-1",
            "--media-prompt",
            "Ocean waves",
            "--media-cost",
            "80",
            "--wallet-balance",
            "170",
            "--approval-threshold",
            "50",
            "--agents-credits-allowed",
            "true",
            "--approve-media",
        ),
        "html_media",
    )
    assert require_field(vid, "approval_required", expected_type=bool) is True
    assert require_field(vid, "outcome", expected_type=str) == "generated"
    assert require_field(vid, "charged_credits", expected_type=int) == 80
    assert require_field(vid, "remaining_balance", expected_type=int) == 90


# ============================================================================
# OB-F13: Voice (STT/TTS/WebRTC Live Voice), Chat Bridges & Team API (6 Tests)
# ============================================================================


@e2e_test(
    id="TEST-T1-OB-F13-01",
    tier=1,
    features=["OB-F13"],
    description="Evaluates all 8 STT VOICE_MODEL_IDS, bark-3.0-plus retired flag, 16kHz <=600s WAV bounds, and locale routing.",
)
def test_t1_ob_f13_01_voice_stt_eight_models_and_locale_routing(env: TempOpenBotEnv) -> None:
    expected_stt = {
        "parakeet-tdt-0.6b-v3",
        "parakeet-unified-en-0.6b",
        "bark-3.0-plus",
        "whisper-turbo",
        "cohere-transcribe-03-2026",
        "elevenlabs-scribe-v2-realtime",
        "groq-whisper-large-v3-turbo",
        "groq-whisper-large-v3",
    }
    res_pl = unwrap_entity(
        env.run_cli_json(
            "platform",
            "voice-eval",
            "--mode",
            "stt",
            "--locale",
            "pl-PL",
            "--audio-rate-hz",
            "16000",
            "--audio-seconds",
            "120",
        ),
        "voice",
    )
    assert require_field(res_pl, "stt_providers", expected_type=int) == 8
    assert set(extract_list(res_pl, "stt_models")) == expected_stt
    assert require_field(res_pl, "selected_stt_model", expected_type=str) == "parakeet-tdt-0.6b-v3"
    assert require_field(res_pl, "live_preview", expected_type=bool) is True

    res_zh = unwrap_entity(
        env.run_cli_json("platform", "voice-eval", "--mode", "stt", "--locale", "zh-CN"),
        "voice",
    )
    assert require_field(res_zh, "selected_stt_model", expected_type=str) == "whisper-turbo"

    res_bark = unwrap_entity(
        env.run_cli_json(
            "platform", "voice-eval", "--mode", "stt", "--stt-provider", "bark-3.0-plus"
        ),
        "voice",
    )
    assert require_field(res_bark, "retired", expected_type=bool) is True


@e2e_test(
    id="TEST-T1-OB-F13-02",
    tier=1,
    features=["OB-F13"],
    description="Evaluates all 7 TTS SPEECH_MODEL_IDS, speech speed cycle, VoiceServerSettings apiKey omission, and voiceAgent attribution.",
)
def test_t1_ob_f13_02_voice_tts_seven_models_speed_cycle_and_attribution(env: TempOpenBotEnv) -> None:
    a1 = require_field(_create_agent(env, "LeadSpeaker"), "id", expected_type=str)
    a2 = require_field(_create_agent(env, "MemberTwo"), "id", expected_type=str)
    ch = _create_channel(env, "VoiceRoom", [a1, a2], lead=a1)
    cid = require_field(ch, "id", expected_type=str)

    expected_tts = {
        "supertonic-3",
        "elevenlabs-flash-v2.5",
        "elevenlabs-multilingual-v2",
        "elevenlabs-v4-turbo",
        "elevenlabs-v4",
        "elevenlabs-v3",
        "xai-tts",
    }
    tts_res = unwrap_entity(
        env.run_cli_json(
            "platform",
            "voice-eval",
            "--mode",
            "tts",
            "--tts-provider",
            "supertonic-3",
            "--tts-text",
            "Build verification succeeded.",
            "--tts-speed",
            "1.0",
            "--cycle-speed",
            "--channel-id",
            cid,
        ),
        "voice",
    )
    assert require_field(tts_res, "tts_providers", expected_type=int) == 7
    assert set(extract_list(tts_res, "tts_models")) == expected_tts
    assert math.isclose(float(require_field(tts_res, "next_speed", expected_type=(int, float))), 1.25)
    assert require_field(tts_res, "voice_agent_id", expected_type=str) == a1

    expected_cycle = {1.0: 1.25, 1.25: 1.5, 1.5: 2.0, 2.0: 0.75, 0.75: 1.0}
    for cur_sp, nxt_sp in expected_cycle.items():
        cy = unwrap_entity(
            env.run_cli_json(
                "platform",
                "voice-eval",
                "--mode",
                "tts",
                "--tts-speed",
                str(cur_sp),
                "--cycle-speed",
            ),
            "voice",
        )
        assert math.isclose(float(require_field(cy, "next_speed", expected_type=(int, float))), nxt_sp)

    srv_cfg = unwrap_entity(
        env.run_cli_json(
            "platform",
            "voice-eval",
            "--mode",
            "server-config",
            "--voice-server-json",
            json.dumps(
                {
                    "address": "https://voice.internal.test",
                    "hasApiKey": True,
                    "routes": {
                        "dictation": True,
                        "speech": True,
                        "transcriptionModel": "whisper-large-v3",
                        "speechModel": "tts-1",
                        "voice": "alloy",
                    },
                }
            ),
        ),
        "voice",
    )
    vs = require_field(srv_cfg, "voice_server", expected_type=dict)
    assert require_field(vs, "has_api_key", "hasApiKey", expected_type=bool) is True
    assert "apiKey" not in vs and "api_key" not in vs


@e2e_test(
    id="TEST-T1-OB-F13-03",
    tier=1,
    features=["OB-F13"],
    description="Evaluates all 25 Codex WebRTC Live Voices (juniper..viola), turn pauses, and SDP offer/answer handshake.",
)
def test_t1_ob_f13_03_voice_live_webrtc_25_voices_and_sdp(env: TempOpenBotEnv) -> None:
    ag = _create_agent(env, "LiveVoiceAgent")
    aid = require_field(ag, "id", expected_type=str)
    live = unwrap_entity(
        env.run_cli_json(
            "platform",
            "voice-eval",
            "--mode",
            "live",
            "--live-voice",
            "juniper",
            "--turn-pause-ms",
            "2000",
            "--sdp",
            "v=0\r\no=- 1 1 IN IP4 127.0.0.1\r\ns=OpenBotLive\r\n",
            "--agent-id",
            aid,
        ),
        "voice",
    )
    assert require_field(live, "live_voices", expected_type=int) == 25
    assert require_field(live, "selected_live_voice", expected_type=str) == "juniper"
    assert require_field(live, "webrtc_ready", expected_type=bool) is True
    assert len(require_field(live, "session_id", expected_type=str)) >= 4
    assert "v=0" in require_field(live, "answer_sdp", expected_type=str)


@e2e_test(
    id="TEST-T1-OB-F13-04",
    tier=1,
    features=["OB-F13"],
    description="Syncs inbound and outbound messages across Slack, Discord (allowedRoles), Telegram, and WhatsApp (reply mode).",
)
def test_t1_ob_f13_04_four_platform_messaging_bridges_sync(env: TempOpenBotEnv) -> None:
    ag = _create_agent(env, "BridgeOrchestrator")
    aid = require_field(ag, "id", expected_type=str)

    cases = [
        ("slack", {"workspaceId": "T01", "channel": "C99", "text": "Hello Slack"}, []),
        (
            "discord",
            {"guildId": "G01", "channelId": "D99", "roles": ["777888"], "text": "Hello Discord"},
            ["--allowed-roles", json.dumps(["777888"])],
        ),
        ("telegram", {"chat_id": "-100909", "text": "Hello Telegram"}, []),
        (
            "whatsapp",
            {"sender": "5511987654321", "text": "Hello WhatsApp"},
            ["--allowed-senders", json.dumps([{"kind": "contact", "id": "551187654321", "readOnly": False}])],
        ),
    ]
    for platform, payload, extra in cases:
        in_res = unwrap_entity(
            env.run_cli_json(
                "platform",
                "bridge-sync",
                "--platform",
                platform,
                "--direction",
                "inbound",
                "--agent-id",
                aid,
                "--payload",
                json.dumps(payload),
                *extra,
            ),
            "bridge",
        )
        assert require_field(in_res, "decision", expected_type=str) == "allow"
        assert require_field(in_res, "delivered", expected_type=bool) is True
        tid = require_field(in_res, "mapped_thread_id", expected_type=str)

        out_res = unwrap_entity(
            env.run_cli_json(
                "platform",
                "bridge-sync",
                "--platform",
                platform,
                "--direction",
                "outbound",
                "--agent-id",
                aid,
                "--payload",
                json.dumps({"mappedThreadId": tid, "text": f"Ack {platform}"}),
            ),
            "bridge",
        )
        assert require_field(out_res, "delivered", expected_type=bool) is True

    assert len(env.query_sqlite("SELECT * FROM projection_messaging_threads")) >= 4
    assert len(env.query_sqlite("SELECT * FROM projection_messaging_history")) >= 4


@e2e_test(
    id="TEST-T1-OB-F13-05",
    tier=1,
    features=["OB-F13"],
    description="Verifies live /v1/health, /v1/capabilities & /v1/compatibility (v1..v10, 18 providers, >=220 caps), and /v1/agents & /v1/channels.",
)
def test_t1_ob_f13_05_live_team_api_capabilities_and_crud(env: TempOpenBotEnv) -> None:
    srv = env.start_server()
    st_h, body_h, _ = srv.http_team("GET", "/v1/health")
    assert st_h == 200
    assert require_field(body_h, "schema_version", expected_type=int) == 69

    for route in ("/v1/capabilities", "/v1/compatibility"):
        st_c, body_c, hdrs_c = srv.http_team(
            "GET", route, headers={"OpenBot-Protocol-Version": "10"}
        )
        assert st_c == 200
        versions = extract_list(body_c, "api_versions")
        assert versions == [f"v{i}" for i in range(1, 11)]
        assert set(extract_list(body_c, "providers")) == set(BUILTIN_AGENT_PROVIDERS)
        caps = extract_list(body_c, "capabilities")
        assert len(caps) >= 220
        assert "openbot-capabilities" in hdrs_c

    st_pa, body_pa, _ = srv.http_team("POST", "/v1/agents", payload={"name": "RestAgent", "provider": "codex"})
    assert st_pa in (200, 201)
    created_ag = unwrap_entity(body_pa, "agent")
    aid = require_field(created_ag, "id", "agent_id", expected_type=str)

    st_pc, body_pc, _ = srv.http_team(
        "POST", "/v1/channels", payload={"name": "RestChannel", "members": [aid], "leadAgentId": aid}
    )
    assert st_pc in (200, 201)
    created_ch = unwrap_entity(body_pc, "channel")
    cid = require_field(created_ch, "id", "channel_id", expected_type=str)

    ag_rows = env.query_sqlite("SELECT * FROM projection_agents")
    assert any((r.get("agent_id") or r.get("id")) == aid for r in ag_rows)
    ch_rows = env.query_sqlite("SELECT * FROM projection_channels")
    assert any((r.get("channel_id") or r.get("id")) == cid for r in ch_rows)


@e2e_test(
    id="TEST-T1-OB-F13-06",
    tier=1,
    features=["OB-F13"],
    description="Connects SSE client to /v1/events and verifies real-time ready, agent.created, channel.updated, and message.posted frames.",
)
def test_t1_ob_f13_06_live_team_api_sse_events_stream(env: TempOpenBotEnv) -> None:
    srv = env.start_server()
    with srv.open_sse("/v1/events") as sse:
        ev_ready = sse.read_until(lambda e: e.event == "ready", timeout=5.0)
        assert ev_ready.event == "ready"

        st_a, body_a, _ = srv.http_team("POST", "/v1/agents", payload={"name": "SseAgent"})
        assert st_a in (200, 201)
        aid = require_field(unwrap_entity(body_a, "agent"), "id", "agent_id", expected_type=str)
        ev_ag = sse.read_until(lambda e: e.event == "agent.created", timeout=5.0)
        assert aid in ev_ag.data

        st_c, body_c, _ = srv.http_team(
            "POST", "/v1/channels", payload={"name": "SseRoom", "members": [aid], "leadAgentId": aid}
        )
        assert st_c in (200, 201)
        cid = require_field(unwrap_entity(body_c, "channel"), "id", "channel_id", expected_type=str)
        ev_ch = sse.read_until(lambda e: e.event in ("channel.updated", "channel.created"), timeout=5.0)
        assert cid in ev_ch.data

        env.run_cli_json("channel", "post", "--channel-id", cid, "--text", "Live SSE ping")
        ev_msg = sse.read_until(lambda e: e.event == "message.posted", timeout=5.0)
        assert "Live SSE ping" in ev_msg.data or cid in ev_msg.data


# ============================================================================
# OB-F14: 1:1 OpenBot Shell, Rails, Reorderable Sidebar & Studio (6 Tests)
# ============================================================================


@e2e_test(
    id="TEST-T1-OB-F14-01",
    tier=1,
    features=["OB-F14"],
    description="Verifies PageRail 11 pages, 7 pinnable pages (5 default pinned), 3 hideable items, and 34x34px UsageRings.",
)
def test_t1_ob_f14_01_page_rail_destinations_pins_and_usage_rings(env: TempOpenBotEnv) -> None:
    sh = _probe_ui(env, "openbot-shell", "openbot_shell")
    pages = extract_list(sh, "page_rail_pages")
    assert len(pages) == 11
    assert set(pages) == {
        "agents", "workflows", "tasks", "schedule", "usage", "library",
        "providers", "crm", "marketplace", "credits", "settings",
    }
    assert extract_list(sh, "pinnable_pages") == [
        "workflows", "tasks", "schedule", "usage", "library", "providers", "crm"
    ]
    assert set(extract_list(sh, "pinned_pages")) == {"tasks", "workflows", "usage", "library", "crm"}
    assert extract_list(sh, "hideable_items") == ["credits", "marketplace", "help"]
    ring = require_field(sh, "usage_ring_spec", expected_type=dict)
    assert require_field(ring, "size_px", expected_type=int) == 34
    assert math.isclose(float(require_field(ring, "stroke_px", expected_type=(int, float))), 2.25)
    assert math.isclose(float(require_field(ring, "spacing_px", expected_type=(int, float))), 4.5)


@e2e_test(
    id="TEST-T1-OB-F14-02",
    tier=1,
    features=["OB-F14"],
    description="Verifies ServerRail responsive widths (72px darwin, 64px linux, 56px narrow <=800px), 4-icon folder preview, and 1400ms beam().",
)
def test_t1_ob_f14_02_server_rail_responsive_widths_and_beam(env: TempOpenBotEnv) -> None:
    mac = _probe_ui(env, "openbot-shell", "openbot_shell", server_view="rail", platform="darwin", window_width_px=1280)
    assert require_field(mac, "server_rail_px", expected_type=int) == 72
    assert require_field(mac, "hidden_server_preview_limit", expected_type=int) == 4
    assert require_field(mac, "beam_duration_ms", expected_type=int) == 1400

    lin = _probe_ui(env, "openbot-shell", "openbot_shell", server_view="rail", platform="linux", window_width_px=1280)
    assert require_field(lin, "server_rail_px", expected_type=int) == 64

    narrow = _probe_ui(env, "openbot-shell", "openbot_shell", server_view="rail", platform="linux", window_width_px=760)
    assert require_field(narrow, "server_rail_px", expected_type=int) == 56


@e2e_test(
    id="TEST-T1-OB-F14-03",
    tier=1,
    features=["OB-F14"],
    description="Verifies Sidebar 280px default, <110px snap to 88px compact, >118px expand to 128px, and Cmd+B close to 0px.",
)
def test_t1_ob_f14_03_sidebar_resize_compact_snap_and_cmd_b_close(env: TempOpenBotEnv) -> None:
    base = _probe_ui(env, "openbot-shell", "openbot_shell")
    assert require_field(base, "sidebar_width_px", expected_type=int) == 280

    snapped = _probe_ui(env, "openbot-shell", "openbot_shell", drag_sidebar_px=105)
    assert require_field(snapped, "sidebar_compact", expected_type=bool) is True
    assert require_field(snapped, "sidebar_effective_px", expected_type=int) == 88

    expanded = _probe_ui(env, "openbot-shell", "openbot_shell", drag_sidebar_px=124)
    assert require_field(expanded, "sidebar_compact", expected_type=bool) is False
    assert require_field(expanded, "sidebar_effective_px", expected_type=int) == 128

    closed = _probe_ui(env, "openbot-shell", "openbot_shell", toggle_cmd_b=True)
    assert require_field(closed, "sidebar_closed", expected_type=bool) is True
    assert require_field(closed, "sidebar_effective_px", expected_type=int) == 0


@e2e_test(
    id="TEST-T1-OB-F14-04",
    tier=1,
    features=["OB-F14"],
    description="Verifies SidebarNav vertical group order (pinned..hidden) and 3-level folder layout persistence.",
)
def test_t1_ob_f14_04_sidebar_group_order_and_folder_layout(env: TempOpenBotEnv) -> None:
    layout = {
        "revision": 1,
        "sections": [
            {"id": "lvl1", "name": "Engineering"},
            {"id": "lvl2", "name": "Backend", "parentId": "lvl1"},
            {"id": "lvl3", "name": "Rust", "parentId": "lvl2"},
        ],
        "order": ["lvl1", "lvl2", "lvl3", "people", "unassigned"],
        "agentAssignments": {"a1": "lvl3"},
        "agentOrder": ["a1"],
    }
    sh = _probe_ui(
        env,
        "openbot-shell",
        "openbot_shell",
        sidebar_layout=layout,
        sidebar_pins=[{"kind": "agent", "id": "a1"}],
    )
    assert extract_list(sh, "sidebar_group_order") == ["pinned", "waiting", "unread", "sections", "hidden"]
    assert require_field(sh, "folder_levels_max", expected_type=int) == 3
    depths = require_field(sh, "resolved_folder_depths", expected_type=dict)
    assert depths.get("lvl1") == 0 and depths.get("lvl2") == 1 and depths.get("lvl3") == 2
    assert len(extract_list(sh, "persisted_files")) >= 1


@e2e_test(
    id="TEST-T1-OB-F14-05",
    tier=1,
    features=["OB-F14"],
    description="Verifies GlobalSearchPlush caret gaze tracking (GAZE_REACH=160 -> [-1,1]) and 500ms idle +-4px look scan.",
)
def test_t1_ob_f14_05_global_search_plush_caret_gaze_and_idle_scan(env: TempOpenBotEnv) -> None:
    sh = _probe_ui(env, "openbot-shell", "openbot_shell", search_caret_x=80.0, searching=False)
    gsp = require_field(sh, "global_search_plush", expected_type=dict)
    assert math.isclose(float(require_field(gsp, "gaze_x", expected_type=(int, float))), 0.5, abs_tol=0.02)
    assert require_field(gsp, "idle_ms", expected_type=int) == 500
    assert require_field(gsp, "look_px", expected_type=int) == 4
    assert require_field(gsp, "animated", expected_type=bool) is True


@e2e_test(
    id="TEST-T1-OB-F14-06",
    tier=1,
    features=["OB-F14"],
    description="Verifies FirstAgentSetup (12 faces, eyeScale=1.25, 17 colors, 6 suggestions) and 360px AgentSettingsPanel.",
)
def test_t1_ob_f14_06_first_agent_setup_and_agent_settings_panel(env: TempOpenBotEnv) -> None:
    sh = _probe_ui(env, "openbot-shell", "openbot_shell", setup_suggestion_id="trip-planner")
    setup = require_field(sh, "first_agent_setup", expected_type=dict)
    assert require_field(setup, "face_count", expected_type=int) == 12
    assert math.isclose(float(require_field(setup, "eye_scale", expected_type=(int, float))), 1.25)
    assert require_field(setup, "color_choices", expected_type=int) == 17
    assert require_field(setup, "suggestions_count", expected_type=int) == 6
    assert require_field(setup, "matched_suggestion", expected_type=str) == "trip-planner"
    assert require_field(sh, "agent_settings_min_px", expected_type=int) == 360


# ============================================================================
# OB-F15: Virtualized Conversation, AgentSteps, TraceWaterfall & ask_ui (6)
# ============================================================================


@e2e_test(
    id="TEST-T1-OB-F15-01",
    tier=1,
    features=["OB-F15"],
    description="Verifies 40ms/300ms/400ms paced streaming timings and >20,000-char auto-collapse to <=3,000-char preview.",
)
def test_t1_ob_f15_01_paced_streaming_and_long_message_collapse(env: TempOpenBotEnv) -> None:
    cv = _probe_ui(env, "conversation-virtualized", "conversation", message_length=25000, streaming=False)
    timings = require_field(cv, "stream_timings_ms", expected_type=dict)
    assert timings == {"step_ms": 40, "fade_ms": 300, "catch_up_ms": 400}
    assert require_field(cv, "collapsed", expected_type=bool) is True
    prev_len = require_field(cv, "preview_length", expected_type=int)
    assert 0 < prev_len <= 3000
    assert require_field(cv, "show_more_available", expected_type=bool) is True

    expanded = _probe_ui(
        env, "conversation-virtualized", "conversation", message_length=25000, streaming=False, expand_message=True
    )
    assert require_field(expanded, "collapsed", expected_type=bool) is False


@e2e_test(
    id="TEST-T1-OB-F15-02",
    tier=1,
    features=["OB-F15"],
    description="Verifies rich timeline blocks (Markdown, KaTeX, Mermaid, Tables, Code, Lightbox) and VoicePlayer speed cycle.",
)
def test_t1_ob_f15_02_rich_message_renderers_and_voice_player_cycle(env: TempOpenBotEnv) -> None:
    cv = _probe_ui(
        env, "conversation-virtualized", "conversation", voice_speed=1.0, cycle_voice_speed=True
    )
    renderers = set(extract_list(cv, "rich_renderers"))
    assert {
        "markdown", "katex", "mermaid", "data_table", "comparison_table",
        "code_block", "lightbox", "voice_player",
    }.issubset(renderers)
    vp = require_field(cv, "voice_player", expected_type=dict)
    assert require_field(vp, "bar_count", expected_type=int) == 40
    assert math.isclose(float(require_field(vp, "seek_step_sec", expected_type=(int, float))), 0.5)
    assert math.isclose(float(require_field(vp, "next_speed", expected_type=(int, float))), 1.25)

    for cur_sp, nxt_sp in ((1.25, 1.5), (1.5, 2.0), (2.0, 0.75), (0.75, 1.0)):
        step_cv = _probe_ui(
            env, "conversation-virtualized", "conversation", voice_speed=cur_sp, cycle_voice_speed=True
        )
        vp_step = require_field(step_cv, "voice_player", expected_type=dict)
        assert math.isclose(float(require_field(vp_step, "next_speed", expected_type=(int, float))), nxt_sp)


@e2e_test(
    id="TEST-T1-OB-F15-03",
    tier=1,
    features=["OB-F15"],
    description="Verifies 1:1 chat emoji reactions and message pins (agent pin followed by human unpin).",
)
def test_t1_ob_f15_03_one_on_one_chat_reactions_and_pins(env: TempOpenBotEnv) -> None:
    ag = _create_agent(env, "PinReactBot")
    aid = require_field(ag, "id", expected_type=str)
    sent = env.run_cli_json("chat", "send", "--agent-id", aid, "--text", "Pin and react to this turn")
    mid = require_field(sent, "message_id", expected_type=str)

    reacted = env.run_cli_json(
        "chat", "react", "--agent-id", aid, "--message-id", mid, "--emoji", "👍", "--actor", "user"
    )
    reactions = extract_list(reacted, "reactions")
    assert any(pick(r, "emoji") == "👍" and int(pick(r, "count", default=0)) >= 1 for r in reactions)

    pinned = env.run_cli_json(
        "chat", "pin", "--agent-id", aid, "--message-id", mid, "--pinned-by", "agent"
    )
    assert require_field(pinned, "pinned", expected_type=bool) is True
    assert require_field(pinned, "pinned_by_kind", expected_type=str) == "agent"

    unpinned = env.run_cli_json(
        "chat", "pin", "--agent-id", aid, "--message-id", mid, "--pinned-by", "user", "--unpin"
    )
    assert require_field(unpinned, "pinned", expected_type=bool) is False


@e2e_test(
    id="TEST-T1-OB-F15-04",
    tier=1,
    features=["OB-F15"],
    description="Verifies AgentSteps (220px cap, <s> memory diff strikethrough, expandable stderr), TraceWaterfallDialog, and SubagentDock.",
)
def test_t1_ob_f15_04_agent_steps_trace_waterfall_and_subagent_dock(env: TempOpenBotEnv) -> None:
    steps = [
        {"kind": "thinking", "text": "Analyzing call graph"},
        {"kind": "progress", "label": "cargo test", "failed": True, "error": "assertion failed"},
        {"kind": "memory", "op": "updated", "previousText": "Old endpoint", "text": "New endpoint"},
        {"kind": "memory", "op": "forgotten", "text": "Deprecated token"},
    ]
    spans = [
        {"id": "s1", "kind": "model", "startMs": 0, "endMs": 120},
        {"id": "s2", "kind": "tool", "startMs": 120, "endMs": 260},
        {"id": "s3", "kind": "waiting", "startMs": 260, "endMs": 310},
        {"id": "s4", "kind": "handoff", "startMs": 310, "endMs": 340},
    ]
    subagents = [
        {"id": "sub1", "status": "running"},
        {"id": "sub2", "status": "running"},
        {"id": "sub3", "status": "finished"},
    ]
    cv = _probe_ui(
        env,
        "conversation-virtualized",
        "conversation",
        steps=steps,
        trace_spans=spans,
        subagents=subagents,
    )
    ag_steps = require_field(cv, "agent_steps", expected_type=dict)
    assert require_field(ag_steps, "max_height_px", expected_type=int) == 220
    assert require_field(ag_steps, "strikethrough_count", expected_type=int) >= 2
    assert require_field(ag_steps, "failed_error_expandable", expected_type=bool) is True

    tw = require_field(cv, "trace_waterfall", expected_type=dict)
    assert extract_list(tw, "span_kinds") == ["model", "tool", "waiting", "handoff"]
    assert extract_list(tw, "speed_levels") == ["1", "2", "5", "10"]

    dock = require_field(cv, "subagent_dock", expected_type=dict)
    assert dock == {"in_ms": 340, "out_ms": 260, "running": 2, "finished": 1}


@e2e_test(
    id="TEST-T1-OB-F15-05",
    tier=1,
    features=["OB-F15"],
    description="Verifies ask_ui confirm (confirmHold: 1200ms within [400, 3000]ms) and quick_replies (allowText _text).",
)
def test_t1_ob_f15_05_ask_ui_confirm_hold_and_quick_replies(env: TempOpenBotEnv) -> None:
    confirm_spec = {
        "version": 1,
        "type": "confirm",
        "title": "Deploy Production Release",
        "confirmHold": 1200,
        "actions": [
            {"id": "ship", "label": "Deploy", "style": "danger", "confirm": True},
            {"id": "abort", "label": "Abort", "style": "ghost"},
        ],
    }
    cv_conf = _probe_ui(
        env,
        "conversation-virtualized",
        "conversation",
        ui_block_spec=confirm_spec,
        hold_duration_ms=1250,
    )
    ub_conf = require_field(cv_conf, "ui_block", expected_type=dict)
    assert require_field(ub_conf, "valid", expected_type=bool) is True
    assert require_field(ub_conf, "type", expected_type=str) == "confirm"
    assert require_field(ub_conf, "hold_accepted", expected_type=bool) is True

    qr_spec = {
        "version": 1,
        "type": "quick_replies",
        "title": "Choose target cluster",
        "allowText": True,
        "options": [
            {"id": "eu_west", "label": "EU West"},
            {"id": "us_east", "label": "US East"},
        ],
    }
    cv_qr = _probe_ui(
        env,
        "conversation-virtualized",
        "conversation",
        ui_block_spec=qr_spec,
        ui_block_response={"actionId": "_text", "text": "ap-northeast-1"},
    )
    ub_qr = require_field(cv_qr, "ui_block", expected_type=dict)
    assert require_field(ub_qr, "valid", expected_type=bool) is True
    assert require_field(ub_qr, "type", expected_type=str) == "quick_replies"


@e2e_test(
    id="TEST-T1-OB-F15-06",
    tier=1,
    features=["OB-F15"],
    description="Verifies ask_ui choice (multiple: true) and form (secret: true storage redaction and vault card masking).",
)
def test_t1_ob_f15_06_ask_ui_choice_and_form_secret_vault_redaction(env: TempOpenBotEnv) -> None:
    choice_spec = {
        "version": 1,
        "type": "choice",
        "title": "Select audit modules",
        "multiple": True,
        "options": [
            {"id": "tls", "label": "TLS 1.3", "selected": True},
            {"id": "wal", "label": "SQLite WAL"},
            {"id": "csp", "label": "Sandbox CSP"},
        ],
    }
    cv_choice = _probe_ui(
        env,
        "conversation-virtualized",
        "conversation",
        ui_block_spec=choice_spec,
        ui_block_response={"actionId": "submit", "values": {"selected": ["tls", "csp"]}},
    )
    assert require_field(require_field(cv_choice, "ui_block", expected_type=dict), "valid", expected_type=bool) is True

    secret_val = "sk-live-ultra-secret-token-9911"
    form_spec = {
        "version": 1,
        "type": "form",
        "title": "Vault & Token Provisioning",
        "vault": {"kind": "card"},
        "fields": [
            {"id": "api_token", "kind": "text", "label": "API Token", "secret": True},
            {"id": "env", "kind": "segmented", "label": "Env", "options": ["dev", "prod"]},
            {"id": "cutover", "kind": "date", "label": "Cutover Date"},
        ],
    }
    cv_form = _probe_ui(
        env,
        "conversation-virtualized",
        "conversation",
        ui_block_spec=form_spec,
        ui_block_response={
            "actionId": "submit",
            "values": {"api_token": secret_val, "env": "prod", "cutover": "2026-12-01"},
        },
    )
    ub_form = require_field(cv_form, "ui_block", expected_type=dict)
    assert require_field(ub_form, "valid", expected_type=bool) is True
    assert require_field(ub_form, "secret_redacted_in_storage", expected_type=bool) is True
    assert extract_list(ub_form, "vault_masked_fields") == ["password", "number", "securityCode"]
    for p in env.data_dir.rglob("*.json"):
        assert secret_val not in p.read_text(encoding="utf-8", errors="replace")
