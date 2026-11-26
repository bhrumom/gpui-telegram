"""Tier 2 Boundary, Corner & Negative Opaque-Box E2E Tests for OB-F11..OB-F15 (30 tests, 6/feature).

Verifies workflow node/edge/byte caps, DAG cycle & annotation/branch edge rejections, >=180s routine cadence,
HMAC webhook skew/size/tamper guards, 20-wait cap, loopback /run 401/403/413/429 guards, CRM human approval
& sourceOwned/sk_* blocks, static site secret/traversal filters, HTML CSP/RPC & credit media limits,
STT/TTS/Live voice bounds, bridge sender policies, Team API protocol/auth guards, Sidebar hysteresis/cycles,
20,000-char collapse boundary, human-pin ownership ('pinned-by-person'), and ask_ui schema bounds.
"""

from __future__ import annotations

import hashlib
import hmac
import json
import time
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
# OB-F11 Tier 2: Workflow Limits/Cycles, >=180s Cadence, Webhooks & /run Guards
# ============================================================================


@e2e_test(
    id="TEST-T2-OB-F11-01",
    tier=2,
    features=["OB-F11"],
    description="workflow-save accepts exact 200-node boundary and rejects 201 nodes, 401 edges, and >1 MiB workflow JSON.",
)
def test_t2_ob_f11_01_workflow_node_edge_and_byte_limits(env: TempOpenBotEnv) -> None:
    nodes_200 = [
        {
            "id": f"nd_{i:x}",
            "kind": "trigger" if i == 0 else "llm",
            "at": {"x": float((i % 16) * 64), "y": float((i // 16) * 64)},
            "fields": {"trigger": "manual"} if i == 0 else {"prompt": f"Step {i:x}"},
        }
        for i in range(200)
    ]
    ok_200 = unwrap_entity(
        env.run_cli_json(
            "platform",
            "workflow-save",
            "--spec",
            json.dumps({"title": "CapMax", "nodes": nodes_200, "edges": [], "strokes": []}),
        ),
        "workflow",
    )
    assert require_field(ok_200, "node_count", expected_type=int) == 200

    nodes_201 = nodes_200 + [
        {"id": "nd_extra", "kind": "llm", "at": {"x": 0.0, "y": 1200.0}, "fields": {"prompt": "Overflow"}}
    ]
    env.run_cli_expect_error(
        "platform",
        "workflow-save",
        "--spec",
        json.dumps({"title": "OverCap", "nodes": nodes_201, "edges": [], "strokes": []}),
        error_contains=("200", "too many", "limit", "maximum"),
    )

    nodes_30 = [
        {"id": f"m_{i:x}", "kind": "llm", "at": {"x": float(i * 64), "y": 0.0}, "fields": {"prompt": f"M{i:x}"}}
        for i in range(30)
    ]
    edges_401 = []
    for i in range(30):
        for j in range(i + 1, 30):
            edges_401.append(
                {"id": f"e_{len(edges_401):x}", "from": f"m_{i:x}", "to": f"m_{j:x}", "type": "text"}
            )
            if len(edges_401) == 401:
                break
        if len(edges_401) == 401:
            break
    env.run_cli_expect_error(
        "platform",
        "workflow-save",
        "--spec",
        json.dumps({"title": "OverEdges", "nodes": nodes_30, "edges": edges_401, "strokes": []}),
        error_contains=("400", "too many", "limit", "maximum"),
    )

    huge_nodes = [
        {"id": f"big_{i:x}", "kind": "prompt", "at": {"x": float(i * 40), "y": 0.0}, "fields": {"text": "X" * 19_000}}
        for i in range(60)
    ]
    huge_spec = json.dumps({"title": "BigGraph", "nodes": huge_nodes, "edges": [], "strokes": []})
    huge_spec_path = env.write_fixture("oversized_workflow.json", huge_spec)
    env.run_cli_expect_error(
        "platform",
        "workflow-save",
        "--spec-file",
        str(huge_spec_path),
        error_contains=("1048576", "1 mib", "1mb", "byte", "size", "limit", "too large"),
    )


@e2e_test(
    id="TEST-T2-OB-F11-02",
    tier=2,
    features=["OB-F11"],
    description="workflow-save rejects self-loops, strict DAG cycles, edges touching annotation nodes, and missing condition branch.",
)
def test_t2_ob_f11_02_workflow_self_loop_cycle_annotation_and_branch_errors(env: TempOpenBotEnv) -> None:
    aid = _mk_agent(env, "WfCycleBot")

    self_loop = {
        "title": "GraphA",
        "nodes": [{"id": "n1", "kind": "llm", "at": {"x": 0.0, "y": 0.0}, "fields": {"prompt": "Step"}}],
        "edges": [{"id": "e1", "from": "n1", "to": "n1", "type": "text"}],
        "strokes": [],
    }
    env.run_cli_expect_error(
        "platform", "workflow-save", "--spec", json.dumps(self_loop),
        error_contains=("self-edge", "self_edge", "self-loop", "self", "cycle"),
    )

    two_cycle = {
        "title": "GraphB",
        "nodes": [
            {"id": "a", "kind": "agent", "at": {"x": 0.0, "y": 0.0}, "fields": {"agentId": aid, "task": "A"}},
            {"id": "b", "kind": "llm", "at": {"x": 240.0, "y": 0.0}, "fields": {"prompt": "B"}},
        ],
        "edges": [
            {"id": "e1", "from": "a", "to": "b", "type": "text"},
            {"id": "e2", "from": "b", "to": "a", "type": "text"},
        ],
        "strokes": [],
    }
    env.run_cli_expect_error(
        "platform", "workflow-save", "--spec", json.dumps(two_cycle),
        error_contains=("cycle", "loop", "dag", "acyclic"),
    )

    ann_edge = {
        "title": "GraphC",
        "nodes": [
            {"id": "note1", "kind": "note", "at": {"x": 0.0, "y": 0.0}, "fields": {"text": "memo"}},
            {"id": "l1", "kind": "llm", "at": {"x": 200.0, "y": 0.0}, "fields": {"prompt": "Run"}},
        ],
        "edges": [{"id": "e1", "from": "note1", "to": "l1", "type": "text"}],
        "strokes": [],
    }
    env.run_cli_expect_error(
        "platform", "workflow-save", "--spec", json.dumps(ann_edge),
        error_contains=("annotation", "port", "cannot connect"),
    )

    missing_branch = {
        "title": "GraphD",
        "nodes": [
            {"id": "c1", "kind": "condition", "at": {"x": 0.0, "y": 0.0}, "fields": {"rule": "ok"}},
            {"id": "o1", "kind": "output", "at": {"x": 200.0, "y": 0.0}, "fields": {"target": "notify", "message": "Done"}},
        ],
        "edges": [{"id": "e1", "from": "c1", "to": "o1", "type": "text"}],
        "strokes": [],
    }
    env.run_cli_expect_error(
        "platform", "workflow-save", "--spec", json.dumps(missing_branch),
        error_contains=("branch", "condition-branch", "condition_branch", "yes or no", "yes/no"),
    )


@e2e_test(
    id="TEST-T2-OB-F11-03",
    tier=2,
    features=["OB-F11"],
    description="routine-save accepts 180s (3m) boundary and rejects <180s intervals, hour-wraparound cron/advanced, @hourly, and Feb 31.",
)
def test_t2_ob_f11_03_routine_minimum_180s_cadence_and_invalid_cron(env: TempOpenBotEnv) -> None:
    aid = _mk_agent(env, "CadenceGuardBot")
    ok_rt = unwrap_entity(
        env.run_cli_json(
            "platform", "routine-save", "--agent-id", aid, "--name", "Boundary3m", "--schedule", "every 180s"
        ),
        "routine",
    )
    assert require_field(ok_rt, "interval_seconds", expected_type=int) == 180

    bad_schedules = [
        "every 120s",
        "*/2 * * * *",
        "0,59 * * * *",
        json.dumps(
            {
                "kind": "advanced",
                "months": list(range(1, 13)),
                "days": {"kind": "every-day"},
                "time": {"kind": "every", "amount": 29, "unit": "minutes"},
            }
        ),
        "@hourly",
        "0 0 31 2 *",
    ]
    for sched in bad_schedules:
        env.run_cli_expect_error(
            "platform",
            "routine-save",
            "--agent-id",
            aid,
            "--name",
            "BadEntry",
            "--schedule",
            sched,
            error_contains=("180", "3 minute", "3m", "minimum", "cadence", "cron", "invalid"),
        )


@e2e_test(
    id="TEST-T2-OB-F11-04",
    tier=2,
    features=["OB-F11"],
    description="webhook-post rejects tampered HMAC, >300s clock skew, malformed deliveryId, and >64 KiB body without running routine.",
)
def test_t2_ob_f11_04_webhook_tampered_skewed_malformed_and_oversized(env: TempOpenBotEnv) -> None:
    aid = _mk_agent(env, "WebhookGuardBot")
    secret = "wh_guard_secret_42"
    rt = unwrap_entity(
        env.run_cli_json(
            "platform",
            "routine-save",
            "--agent-id",
            aid,
            "--name",
            "GuardedHook",
            "--schedule",
            "webhook",
            "--webhook-secret",
            secret,
        ),
        "routine",
    )
    rt_id = require_field(rt, "id", expected_type=str)
    now_ts = int(time.time())
    body = '{"event":"push"}'

    env.run_cli_expect_error(
        "platform",
        "webhook-post",
        "--routine-id",
        rt_id,
        "--payload",
        body,
        "--timestamp",
        str(now_ts),
        "--delivery-id",
        "deliv-bad-01",
        "--signature",
        "sha256=" + ("0" * 64),
        error_contains=("hmac", "invalid_signature", "invalid signature", "mismatch", "digest"),
    )

    stale_ts = now_ts - 301
    stale_sig = "sha256=" + hmac.new(
        secret.encode("utf-8"), f"{stale_ts}.deliv-old-01.{body}".encode("utf-8"), hashlib.sha256
    ).hexdigest()
    env.run_cli_expect_error(
        "platform",
        "webhook-post",
        "--routine-id",
        rt_id,
        "--payload",
        body,
        "--timestamp",
        str(stale_ts),
        "--delivery-id",
        "deliv-old-01",
        "--signature",
        stale_sig,
        error_contains=("skew", "300", "stale", "expired", "clock"),
    )

    env.run_cli_expect_error(
        "platform",
        "webhook-post",
        "--routine-id",
        rt_id,
        "--payload",
        body,
        "--timestamp",
        str(now_ts),
        "--delivery-id",
        "bad id!!",
        "--signature",
        "sha256=" + ("a" * 64),
        error_contains=("invalid", "malformed", "delivery_id", "delivery id"),
    )

    big_body = json.dumps({"pad": "Z" * 65_600})
    big_sig = "sha256=" + hmac.new(
        secret.encode("utf-8"), f"{now_ts}.deliv-big.{big_body}".encode("utf-8"), hashlib.sha256
    ).hexdigest()
    env.run_cli_expect_error(
        "platform",
        "webhook-post",
        "--routine-id",
        rt_id,
        "--payload",
        big_body,
        "--timestamp",
        str(now_ts),
        "--delivery-id",
        "deliv-big",
        "--signature",
        big_sig,
        error_contains=("65536", "64", "large", "size", "limit", "exceeds"),
    )
    assert env.query_sqlite("SELECT * FROM projection_routine_runs") == []


@e2e_test(
    id="TEST-T2-OB-F11-05",
    tier=2,
    features=["OB-F11"],
    description="routine-run accepts exact 20 open await_event waits and rejects the 21st (> INPUT_LIMITS.agentWaits = 20).",
)
def test_t2_ob_f11_05_await_event_20_open_waits_cap(env: TempOpenBotEnv) -> None:
    aid = _mk_agent(env, "WaitCapBot")
    rt = unwrap_entity(
        env.run_cli_json(
            "platform", "routine-save", "--agent-id", aid, "--name", "WaitRoutine", "--schedule", "every 180s"
        ),
        "routine",
    )
    rt_id = require_field(rt, "id", expected_type=str)

    waits_20 = [{"event": f"evt.{i}", "note": f"Wait {i}"} for i in range(20)]
    ok_res = unwrap_entity(
        env.run_cli_json(
            "platform", "routine-run", "--routine-id", rt_id, "--await-events", json.dumps(waits_20)
        ),
        "routine",
    )
    assert require_field(ok_res, "open_waits_count", expected_type=int) == 20

    env.run_cli_expect_rejection(
        "platform",
        "routine-run",
        "--routine-id",
        rt_id,
        "--await-events",
        json.dumps([{"event": "evt.21", "note": "Overflow wait"}]),
        error_contains=("20", "wait", "limit"),
    )
    wait_rows = env.query_sqlite("SELECT * FROM agent_waits")
    assert len([r for r in wait_rows if (r.get("agent_id") or aid) == aid]) == 20


@e2e_test(
    id="TEST-T2-OB-F11-06",
    tier=2,
    features=["OB-F11"],
    timeout_sec=45.0,
    description="Loopback automation server enforces 401 token, 403 Origin/Host/allowAutomation=false, 413 >4000 payload, and 429 on 31st run/hr.",
)
def test_t2_ob_f11_06_automation_server_security_and_30_per_hour_cap(env: TempOpenBotEnv) -> None:
    denied_aid = _mk_agent(env, "NoAutoAgent")
    allowed_aid = _mk_agent(env, "AllowedAutoAgent", allow_automation="true")
    rt = unwrap_entity(
        env.run_cli_json(
            "platform",
            "routine-save",
            "--agent-id",
            allowed_aid,
            "--name",
            "RateLimitedRoutine",
            "--schedule",
            "every 180s",
        ),
        "routine",
    )
    rt_id = require_field(rt, "id", expected_type=str)

    srv = env.start_server()
    st_no_tok, _, _ = srv.http_automation("POST", "/run", payload={"agent_id": allowed_aid}, token=None)
    assert st_no_tok == 401
    st_bad_tok, _, _ = srv.http_automation(
        "POST", "/run", payload={"agent_id": allowed_aid}, token="wrong-secret-token"
    )
    assert st_bad_tok == 401

    st_orig, _, _ = srv.http_automation(
        "POST", "/run", payload={"agent_id": allowed_aid}, include_origin="https://evil.example"
    )
    assert st_orig == 403
    st_host, _, _ = srv.http_automation(
        "POST", "/run", payload={"agent_id": allowed_aid}, custom_host="evil.rebind.example"
    )
    assert st_host == 403

    st_off, _, _ = srv.automation_run(denied_aid, payload="ping")
    assert st_off == 403

    st_413, _, _ = srv.automation_run(allowed_aid, routine_id=rt_id, payload="P" * 4001)
    assert st_413 == 413

    for i in range(30):
        st_ok, _, _ = srv.automation_run(allowed_aid, routine_id=rt_id, payload=f"run-{i}")
        assert st_ok in (200, 202), f"Expected 200/202 on run {i + 1}, got {st_ok}"
    st_31, body_31, _ = srv.automation_run(allowed_aid, routine_id=rt_id, payload="run-31")
    assert st_31 == 429
    assert pick(body_31, "limit_per_hour", default=30) == 30


# ============================================================================
# OB-F12 Tier 2: CRM Guards, Hosted Site Filters, HTML CSP/RPC & Credit Media
# ============================================================================


@e2e_test(
    id="TEST-T2-OB-F12-01",
    tier=2,
    features=["OB-F12"],
    description="CRM rejects --commit-unapproved, sourceOwned Stripe row edits, built-in 'lead' status delete/rename, and sk_* Stripe secret keys.",
)
def test_t2_ob_f12_01_crm_unapproved_source_owned_builtin_and_secret_key_guards(
    env: TempOpenBotEnv,
) -> None:
    aid = _mk_agent(env, "CrmGuardBot")

    env.run_cli_expect_error(
        "platform",
        "crm-sync",
        "--source",
        "stripe",
        "--api-key",
        "sk_live_999999",
        error_contains=("sourcesecretkey", "source_secret_key", "secret", "restricted", "rk_"),
    )

    fixture = {"customers": [{"id": "stripe:cus_lock", "name": "Locked Corp", "status": "customer"}]}
    env.run_cli_json("platform", "crm-sync", "--source", "stripe", "--fixture", json.dumps(fixture))

    env.run_cli_expect_rejection(
        "platform",
        "crm-propose",
        "--agent-id",
        aid,
        "--operation",
        "customer-create",
        "--payload",
        json.dumps({"name": "Bypass Corp", "currency": "USD", "statusId": "lead"}),
        "--commit-unapproved",
        error_contains=("pending approval", "human approval", "approval", "person"),
    )
    cust_ids = {
        str(r.get("customer_id") or r.get("id") or r.get("name"))
        for r in env.query_sqlite("SELECT * FROM crm_customers")
    }
    assert "Bypass Corp" not in " ".join(cust_ids)

    locked_rows = env.query_sqlite("SELECT * FROM crm_customers")
    locked_cid = (
        str(locked_rows[0].get("customer_id") or locked_rows[0].get("id") or "stripe:cus_lock")
        if locked_rows
        else "stripe:cus_lock"
    )
    locked_rev = int(locked_rows[0].get("revision") or 1) if locked_rows else 1
    env.run_cli_expect_rejection(
        "platform",
        "crm-propose",
        "--agent-id",
        aid,
        "--direct-change",
        json.dumps(
            {
                "kind": "customer-update",
                "customerId": locked_cid,
                "patch": {"name": "Tampered", "stripeCustomerId": "cus_hijacked"},
                "baseRevision": locked_rev,
            }
        ),
        error_contains=("sourceowned", "source_owned", "source-owned", "source owned", "read-only", "read_only", "synced"),
    )

    env.run_cli_expect_rejection(
        "platform",
        "crm-propose",
        "--agent-id",
        aid,
        "--direct-change",
        json.dumps({"kind": "status-delete", "statusId": "lead", "replaceWith": "customer"}),
        error_contains=("builtinstatus", "builtin_status", "built-in", "builtin", "immutable", "cannot delete"),
    )


@e2e_test(
    id="TEST-T2-OB-F12-02",
    tier=2,
    features=["OB-F12"],
    timeout_sec=45.0,
    description="crm-propose rejects amountMinor >10^12 or <0, invalid currency, mismatched payment-split total, and 51st pending proposal.",
)
def test_t2_ob_f12_02_crm_amount_currency_split_and_50_pending_cap(env: TempOpenBotEnv) -> None:
    aid = _mk_agent(env, "CrmBoundsBot")
    env.run_cli_json("platform", "crm-sync", "--source", "stripe")

    seed_c = unwrap_entity(
        env.run_cli_json(
            "platform",
            "crm-propose",
            "--agent-id",
            aid,
            "--operation",
            "customer-create",
            "--payload",
            json.dumps({"name": "Split Target Co", "currency": "USD", "statusId": "customer"}),
            "--reason",
            "Seed",
        ),
        "crm",
    )
    env.run_cli_json(
        "platform",
        "crm-propose",
        "--agent-id",
        aid,
        "--proposal-id",
        require_field(seed_c, "proposal_id", expected_type=str),
        "--approve",
    )
    c_rows = env.query_sqlite("SELECT * FROM crm_customers")
    cust_id = str(c_rows[0].get("customer_id") or c_rows[0].get("id"))

    seed_p = unwrap_entity(
        env.run_cli_json(
            "platform",
            "crm-propose",
            "--agent-id",
            aid,
            "--operation",
            "payment-create",
            "--payload",
            json.dumps(
                {
                    "customerId": cust_id,
                    "label": "Annual Invoice",
                    "amountMinor": 10000,
                    "dueOn": "2026-12-01",
                }
            ),
            "--reason",
            "Seed",
        ),
        "crm",
    )
    env.run_cli_json(
        "platform",
        "crm-propose",
        "--agent-id",
        aid,
        "--proposal-id",
        require_field(seed_p, "proposal_id", expected_type=str),
        "--approve",
    )
    p_rows = env.query_sqlite("SELECT * FROM crm_payments")
    pay_id = str(p_rows[0].get("payment_id") or p_rows[0].get("id"))

    for bad_amt in (1_000_000_000_001, -100):
        env.run_cli_expect_error(
            "platform",
            "crm-propose",
            "--agent-id",
            aid,
            "--operation",
            "payment-create",
            "--payload",
            json.dumps(
                {
                    "customerId": cust_id,
                    "label": "Bad Amt",
                    "amountMinor": bad_amt,
                    "dueOn": "2026-12-31",
                }
            ),
            "--reason",
            "Check",
            error_contains=("1000000000000", "range", "negative", "exceeds", "out of", "invalid"),
        )

    env.run_cli_expect_error(
        "platform",
        "crm-propose",
        "--agent-id",
        aid,
        "--operation",
        "customer-create",
        "--payload",
        json.dumps({"name": "Bad Co", "currency": "US", "statusId": "lead"}),
        "--reason",
        "Check",
        error_contains=("iso", "3-letter", "three-letter", "invalid currency", "currency code", "invalid"),
    )

    env.run_cli_expect_error(
        "platform",
        "crm-propose",
        "--agent-id",
        aid,
        "--operation",
        "payment-split",
        "--payload",
        json.dumps(
            {
                "paymentId": pay_id,
                "parts": [
                    {"amountMinor": 4000, "dueOn": "2026-12-15"},
                    {"amountMinor": 4000, "dueOn": "2026-12-31"},
                ],
            }
        ),
        "--reason",
        "Check",
        error_contains=("splittotal", "split_total", "sum", "total", "mismatch", "equal"),
    )

    for idx in range(50):
        res = unwrap_entity(
            env.run_cli_json(
                "platform",
                "crm-propose",
                "--agent-id",
                aid,
                "--operation",
                "customer-create",
                "--payload",
                json.dumps({"name": f"Lead-{idx:02d}", "currency": "USD", "statusId": "lead"}),
                "--reason",
                "Batch import",
            ),
            "crm",
        )
        assert require_field(res, "approval_required", expected_type=bool) is True

    env.run_cli_expect_rejection(
        "platform",
        "crm-propose",
        "--agent-id",
        aid,
        "--operation",
        "customer-create",
        "--payload",
        json.dumps({"name": "Lead-51", "currency": "USD", "statusId": "lead"}),
        "--reason",
        "Overflow",
        error_contains=("50", "pending", "proposal", "limit"),
    )


@e2e_test(
    id="TEST-T2-OB-F12-03",
    tier=2,
    features=["OB-F12"],
    description="site-publish rejects hidden/unsafe (.env/.git), secret (credentials/worker.js), archive (*.map/*.zip), traversal, >20 files, and >1 MiB file.",
)
def test_t2_ob_f12_03_static_site_path_filters_and_upload_limits(env: TempOpenBotEnv) -> None:
    aid = _mk_agent(env, "SiteGuardBot")
    bad_paths = [
        (".env", "API_KEY=1", ("unsafe", "hidden", "blocked", "disallowed")),
        (".git/config", "[core]", ("hidden", "dotfile", "blocked", "disallowed")),
        ("credentials.json", "{}", ("secret", "sensitive", "blocked", "disallowed")),
        ("worker.js", "export default {}", ("secret", "unsafe", "blocked", "disallowed")),
        ("app.js.map", "{}", ("archive", "sourcemap", "blocked", "disallowed")),
        ("bundle.zip", "PK", ("archive", "blocked", "disallowed")),
        ("../escape.html", "<h1>Escape</h1>", ("traversal", "parent", "outside", "relative")),
    ]
    for path_name, content, errs in bad_paths:
        env.run_cli_expect_error(
            "platform",
            "site-publish",
            "--agent-id",
            aid,
            "--slug",
            "bad-site",
            "--files",
            json.dumps([{"path": path_name, "content": content}]),
            error_contains=errs,
        )

    files_21 = [{"path": f"pg_{chr(97 + i)}.html", "content": "<p>ok</p>"} for i in range(21)]
    env.run_cli_expect_error(
        "platform",
        "site-publish",
        "--agent-id",
        aid,
        "--slug",
        "over-cap-site",
        "--files",
        json.dumps(files_21),
        error_contains=("20", "limit", "maximum", "too many"),
    )

    big_dir = env.workspace_dir / "Agents" / aid / "oversized_site"
    big_dir.mkdir(parents=True, exist_ok=True)
    (big_dir / "huge.html").write_bytes(b"A" * 1_048_577)
    env.run_cli_expect_error(
        "platform",
        "site-publish",
        "--agent-id",
        aid,
        "--slug",
        "too-big-file",
        "--source-dir",
        str(big_dir),
        error_contains=("1048576", "1 mib", "1mb", "byte", "size", "limit"),
    )


@e2e_test(
    id="TEST-T2-OB-F12-04",
    tier=2,
    features=["OB-F12"],
    description="marketplace-search rejects unknown --tab and neutralizes SQL/HTML metacharacters in --query.",
)
def test_t2_ob_f12_04_marketplace_invalid_tab_and_adversarial_query(env: TempOpenBotEnv) -> None:
    env.run_cli_json("platform", "crm-sync", "--source", "stripe")
    env.run_cli_expect_error(
        "platform",
        "marketplace-search",
        "--tab",
        "bogus-kind",
        error_contains=("apps", "agents", "skills", "unknown", "unsupported", "invalid"),
    )

    adv = unwrap_entity(
        env.run_cli_json(
            "platform",
            "marketplace-search",
            "--tab",
            "agents",
            "--query",
            "'; DROP TABLE crm_customers; -- <script>alert(1)</script>",
        ),
        "marketplace",
    )
    assert extract_list(adv, "items") == []
    assert require_field(adv, "alpha_fallback", expected_type=str) == "fishing"
    env.check_sqlite_integrity("openbot")


@e2e_test(
    id="TEST-T2-OB-F12-05",
    tier=2,
    features=["OB-F12"],
    description="html-render rejects >512,000-char HTML & out-of-range height, and refuses javascript:/file: open-link & >2,000-char context RPCs.",
)
def test_t2_ob_f12_05_html_render_size_height_and_openbot_page_rpc_guards(env: TempOpenBotEnv) -> None:
    huge_html_path = env.write_fixture("oversized_widget.html", "H" * 512_001)
    env.run_cli_expect_error(
        "platform",
        "html-render",
        "--mode",
        "page",
        "--html-file",
        str(huge_html_path),
        error_contains=("512000", "512", "limit", "length", "large", "exceeds"),
    )

    for bad_h in ("79", "2001"):
        env.run_cli_expect_error(
            "platform",
            "html-render",
            "--mode",
            "page",
            "--height",
            bad_h,
            "--html",
            "<div>ok</div>",
            error_contains=("80", "2000", "range", "bounds", "between"),
        )

    for bad_url in ("javascript:alert(1)", "file:///etc/passwd"):
        rpc_link = {"jsonrpc": "2.0", "id": 2, "method": "ui/open-link", "params": {"url": bad_url}}
        res = unwrap_entity(
            env.run_cli_json(
                "platform",
                "html-render",
                "--mode",
                "page",
                "--html",
                "<div>rpc</div>",
                "--rpc-message",
                json.dumps(rpc_link),
            ),
            "html_media",
        )
        rpc_out = require_field(res, "rpc_result", expected_type=dict)
        assert require_field(rpc_out, "type", expected_type=str) == "refused"

    rpc_ctx = {
        "jsonrpc": "2.0",
        "id": 3,
        "method": "ui/update-model-context",
        "params": {"context": "C" * 2001},
    }
    res_ctx = unwrap_entity(
        env.run_cli_json(
            "platform",
            "html-render",
            "--mode",
            "page",
            "--html",
            "<div>ctx</div>",
            "--rpc-message",
            json.dumps(rpc_ctx),
        ),
        "html_media",
    )
    assert require_field(require_field(res_ctx, "rpc_result", expected_type=dict), "type", expected_type=str) == "refused"


@e2e_test(
    id="TEST-T2-OB-F12-06",
    tier=2,
    features=["OB-F12"],
    description="Credit media blocks >50-credit generation when declined/unapproved (0 credits charged), short wallet, and agents-credits-allowed=false.",
)
def test_t2_ob_f12_06_credit_media_declined_short_wallet_and_agents_off(env: TempOpenBotEnv) -> None:
    declined = env.run_cli_raw(
        "platform",
        "html-render",
        "--generate-media",
        "video",
        "--media-cost",
        "75",
        "--wallet-balance",
        "200",
        "--approval-threshold",
        "50",
        "--decline-media",
    )
    if declined.returncode == 0:
        d = unwrap_entity(declined, "html_media")
        assert require_field(d, "outcome", expected_type=str) == "declined"
        assert pick(d, "charged_credits", default=0) in (0, None)
        assert require_field(d, "remaining_balance", expected_type=int) == 200
    else:
        env.run_cli_expect_rejection(
            "platform",
            "html-render",
            "--generate-media",
            "video",
            "--media-cost",
            "75",
            "--wallet-balance",
            "200",
            "--approval-threshold",
            "50",
            "--decline-media",
            error_contains=("declined", "approval", "50", "credit"),
        )

    short_res = env.run_cli_raw(
        "platform",
        "html-render",
        "--generate-media",
        "image",
        "--media-cost",
        "40",
        "--wallet-balance",
        "10",
    )
    if short_res.returncode == 0:
        assert require_field(unwrap_entity(short_res, "html_media"), "outcome", expected_type=str) == "short"
    else:
        env.run_cli_expect_rejection(
            "platform",
            "html-render",
            "--generate-media",
            "image",
            "--media-cost",
            "40",
            "--wallet-balance",
            "10",
            error_contains=("short", "balance", "insufficient", "credit"),
        )

    off_res = env.run_cli_raw(
        "platform",
        "html-render",
        "--generate-media",
        "image",
        "--media-cost",
        "20",
        "--wallet-balance",
        "200",
        "--agents-credits-allowed",
        "false",
    )
    if off_res.returncode == 0:
        assert require_field(unwrap_entity(off_res, "html_media"), "outcome", expected_type=str) in (
            "unavailable",
            "agentsOff",
        )
    else:
        env.run_cli_expect_rejection(
            "platform",
            "html-render",
            "--generate-media",
            "image",
            "--media-cost",
            "20",
            "--wallet-balance",
            "200",
            "--agents-credits-allowed",
            "false",
            error_contains=("agentsoff", "agents_off", "unavailable", "disabled", "not allowed"),
        )


# ============================================================================
# OB-F13 Tier 2: Voice Audio/TTS/SDP Limits, Bridge Policies & Team API Errors
# ============================================================================


@e2e_test(
    id="TEST-T2-OB-F13-01",
    tier=2,
    features=["OB-F13"],
    description="voice-eval --mode stt rejects >600s duration, !=16kHz sample rate, >19,200,044 WAV bytes, and unknown STT provider.",
)
def test_t2_ob_f13_01_voice_stt_audio_limits_and_unknown_provider(env: TempOpenBotEnv) -> None:
    env.run_cli_expect_error(
        "platform", "voice-eval", "--mode", "stt", "--audio-seconds", "601",
        error_contains=("600", "duration", "limit", "maximum", "exceeds"),
    )
    env.run_cli_expect_error(
        "platform", "voice-eval", "--mode", "stt", "--audio-rate-hz", "44100",
        error_contains=("16000", "16khz", "sample rate", "mono", "unsupported"),
    )
    env.run_cli_expect_error(
        "platform", "voice-eval", "--mode", "stt", "--audio-bytes", "19200045",
        error_contains=("19200044", "wav", "size", "limit", "exceeds"),
    )
    env.run_cli_expect_error(
        "platform", "voice-eval", "--mode", "stt", "--stt-provider", "bogus-stt-99",
        error_contains=("unknown", "unsupported", "invalid"),
    )


@e2e_test(
    id="TEST-T2-OB-F13-02",
    tier=2,
    features=["OB-F13"],
    description="voice-eval --mode tts/server-config rejects >1,000-char text, unsupported speed 3.5, and VoiceServerSettings containing apiKey.",
)
def test_t2_ob_f13_02_voice_tts_length_speed_and_server_apikey_leak_guard(env: TempOpenBotEnv) -> None:
    env.run_cli_expect_error(
        "platform", "voice-eval", "--mode", "tts", "--tts-text", "T" * 1001,
        error_contains=("1000", "length", "limit", "maximum", "exceeds"),
    )
    env.run_cli_expect_error(
        "platform", "voice-eval", "--mode", "tts", "--tts-speed", "3.5",
        error_contains=("0.75", "2.0", "unsupported", "invalid", "allowed"),
    )
    env.run_cli_expect_error(
        "platform",
        "voice-eval",
        "--mode",
        "server-config",
        "--voice-server-json",
        json.dumps(
            {
                "address": "https://voice.internal.test",
                "hasApiKey": True,
                "apiKey": "xi-leaked-tok-99",
                "routes": {
                    "dictation": False,
                    "speech": True,
                    "transcriptionModel": None,
                    "speechModel": "tts-1",
                    "voice": "alloy",
                },
            }
        ),
        error_contains=("omit", "secret", "redacted", "not allowed", "unknown field"),
    )


@e2e_test(
    id="TEST-T2-OB-F13-03",
    tier=2,
    features=["OB-F13"],
    description="voice-eval --mode live rejects >65,536-byte SDP, unknown live voice, and unsupported turn-pause-ms 999.",
)
def test_t2_ob_f13_03_voice_live_sdp_voice_and_pause_guards(env: TempOpenBotEnv) -> None:
    env.run_cli_expect_error(
        "platform", "voice-eval", "--mode", "live", "--sdp", "S" * 65_537,
        error_contains=("65536", "length", "limit", "size", "exceeds"),
    )
    env.run_cli_expect_error(
        "platform", "voice-eval", "--mode", "live", "--live-voice", "bogus-voice-99",
        error_contains=("unknown", "unsupported", "invalid"),
    )
    env.run_cli_expect_error(
        "platform", "voice-eval", "--mode", "live", "--turn-pause-ms", "999",
        error_contains=("1500", "5000", "unsupported", "invalid", "allowed"),
    )


@e2e_test(
    id="TEST-T2-OB-F13-04",
    tier=2,
    features=["OB-F13"],
    description="bridge-sync enforces Discord allowedRoles, WhatsApp unlisted group ignore, readOnly save-without-reply, ownNumber ignore, and <=3 pending pairs.",
)
def test_t2_ob_f13_04_bridge_discord_roles_and_whatsapp_sender_decisions(env: TempOpenBotEnv) -> None:
    aid = _mk_agent(env, "BridgeGuardBot")

    disc = env.run_cli_raw(
        "platform",
        "bridge-sync",
        "--platform",
        "discord",
        "--direction",
        "inbound",
        "--agent-id",
        aid,
        "--allowed-roles",
        json.dumps(["111222"]),
        "--payload",
        json.dumps({"guildId": "G1", "channelId": "D1", "roles": ["999000"], "text": "No role"}),
    )
    if disc.returncode == 0:
        d = unwrap_entity(disc, "bridge")
        assert require_field(d, "decision", expected_type=str) in ("ignore", "refused")
        assert pick(d, "replied", default=False) is False
    else:
        env.run_cli_expect_rejection(
            "platform",
            "bridge-sync",
            "--platform",
            "discord",
            "--direction",
            "inbound",
            "--agent-id",
            aid,
            "--allowed-roles",
            json.dumps(["111222"]),
            "--payload",
            json.dumps({"guildId": "G1", "channelId": "D1", "roles": ["999000"], "text": "No role"}),
            error_contains=("ignore", "refused", "allowed_roles", "allowedroles", "not permitted"),
        )

    wa_grp = unwrap_entity(
        env.run_cli_json(
            "platform",
            "bridge-sync",
            "--platform",
            "whatsapp",
            "--direction",
            "inbound",
            "--agent-id",
            aid,
            "--allowed-senders",
            json.dumps([]),
            "--payload",
            json.dumps({"sender": "99999@g.us", "isGroup": True, "text": "Unlisted group"}),
        ),
        "bridge",
    )
    assert require_field(wa_grp, "decision", expected_type=str) == "ignore"

    wa_ro = unwrap_entity(
        env.run_cli_json(
            "platform",
            "bridge-sync",
            "--platform",
            "whatsapp",
            "--direction",
            "inbound",
            "--agent-id",
            aid,
            "--allowed-senders",
            json.dumps([{"kind": "contact", "id": "48500100200", "readOnly": True}]),
            "--payload",
            json.dumps({"sender": "48500100200", "text": "Archive only"}),
        ),
        "bridge",
    )
    assert require_field(wa_ro, "decision", expected_type=str) == "save"
    assert require_field(wa_ro, "saved_history_only", expected_type=bool) is True
    assert require_field(wa_ro, "replied", expected_type=bool) is False

    wa_own = unwrap_entity(
        env.run_cli_json(
            "platform",
            "bridge-sync",
            "--platform",
            "whatsapp",
            "--direction",
            "inbound",
            "--agent-id",
            aid,
            "--own-number",
            "true",
            "--allowed-senders",
            json.dumps([]),
            "--payload",
            json.dumps({"sender": "48600700800", "text": "Stranger"}),
        ),
        "bridge",
    )
    assert require_field(wa_own, "decision", expected_type=str) == "ignore"


@e2e_test(
    id="TEST-T2-OB-F13-05",
    tier=2,
    features=["OB-F13"],
    description="Team API rejects unsupported OpenBot-Protocol-Version (0 and 99 -> 400/426) and malformed JSON (400) while staying healthy.",
)
def test_t2_ob_f13_05_team_api_protocol_mismatch_and_malformed_json(env: TempOpenBotEnv) -> None:
    srv = env.start_server()
    for bad_ver in ("0", "99"):
        st_v, _, _ = srv.http_team(
            "GET", "/v1/capabilities", headers={"OpenBot-Protocol-Version": bad_ver}
        )
        assert st_v in (400, 426), f"Expected 400/426 for protocol {bad_ver}, got {st_v}"

    st_bad, _, _ = srv.http_team(
        "POST",
        "/v1/agents",
        payload="{malformed-json",
        headers={"Content-Type": "application/json"},
    )
    assert st_bad == 400

    st_h, body_h, _ = srv.http_team("GET", "/v1/health")
    assert st_h == 200 and require_field(body_h, "ok", expected_type=bool) is True


@e2e_test(
    id="TEST-T2-OB-F13-06",
    tier=2,
    features=["OB-F13"],
    description="Team API returns 404 on unknown routes, 400/422 on invalid agent/channel payloads, and 401 when --team-token is missing/wrong.",
)
def test_t2_ob_f13_06_team_api_404_domain_400_and_401_auth(env: TempOpenBotEnv) -> None:
    srv = env.start_server(extra_args=("--team-token", "team-secret-tok-99"))
    st_unauth, _, _ = srv.http_team("GET", "/v1/agents")
    assert st_unauth == 401
    st_wrong, _, _ = srv.http_team("GET", "/v1/agents", token="wrong-tok")
    assert st_wrong == 401

    st_404, _, _ = srv.http_team("GET", "/v1/unknown-route", token="team-secret-tok-99")
    assert st_404 == 404

    st_blank, _, _ = srv.http_team(
        "POST", "/v1/agents", payload={"name": "   "}, token="team-secret-tok-99"
    )
    assert st_blank in (400, 422)

    members_101 = [f"agent-{i:03d}" for i in range(101)]
    st_ch, _, _ = srv.http_team(
        "POST",
        "/v1/channels",
        payload={"name": "TooBig", "members": members_101},
        token="team-secret-tok-99",
    )
    assert st_ch in (400, 422)


# ============================================================================
# OB-F14 Tier 2: Sidebar Hysteresis, Auto-Compact, Folder Cycles & Pin Caps
# ============================================================================


@e2e_test(
    id="TEST-T2-OB-F14-01",
    tier=2,
    features=["OB-F14"],
    description="Sidebar enforces [128, 400]px clamp and <110px snap hysteresis (109px -> 88px compact, 112px -> 128px, 520px -> 400px).",
)
def test_t2_ob_f14_01_sidebar_clamp_and_snap_hysteresis(env: TempOpenBotEnv) -> None:
    s109 = _probe_ui(env, "openbot-shell", "openbot_shell", drag_sidebar_px=109)
    assert require_field(s109, "sidebar_compact", expected_type=bool) is True
    assert require_field(s109, "sidebar_effective_px", expected_type=int) == 88

    s112 = _probe_ui(env, "openbot-shell", "openbot_shell", drag_sidebar_px=112)
    assert require_field(s112, "sidebar_compact", expected_type=bool) is False
    assert require_field(s112, "sidebar_width_px", expected_type=int) == 128

    s520 = _probe_ui(env, "openbot-shell", "openbot_shell", drag_sidebar_px=520)
    assert require_field(s520, "sidebar_width_px", expected_type=int) == 400


@e2e_test(
    id="TEST-T2-OB-F14-02",
    tier=2,
    features=["OB-F14"],
    description="Sidebar auto-compacts at exact CONVERSATION_MIN_WIDTH = 424px boundary (window - server_rail - sidebar < 424 vs >= 424).",
)
def test_t2_ob_f14_02_sidebar_responsive_auto_compact_424px_boundary(env: TempOpenBotEnv) -> None:
    # At window_width_px=759 (<=800 -> server_rail=56px) + default 280px sidebar -> 759 - 56 - 280 = 423 < 424 -> compact!
    below = _probe_ui(
        env, "openbot-shell", "openbot_shell", server_view="rail", platform="linux", window_width_px=759
    )
    assert require_field(below, "sidebar_compact", expected_type=bool) is True

    # At window_width_px=801 (>800 -> server_rail=64px) + default 280px sidebar -> 801 - 64 - 280 = 457 >= 424 -> not compact!
    above = _probe_ui(
        env, "openbot-shell", "openbot_shell", server_view="rail", platform="linux", window_width_px=801
    )
    assert require_field(above, "sidebar_compact", expected_type=bool) is False


@e2e_test(
    id="TEST-T2-OB-F14-03",
    tier=2,
    features=["OB-F14"],
    description="SidebarLayoutSnapshot clamps >3-level folder nesting to depth <=2 and breaks parent cycles cleanly.",
)
def test_t2_ob_f14_03_sidebar_folder_depth_clamp_and_cycle_breaking(env: TempOpenBotEnv) -> None:
    layout = {
        "revision": 1,
        "sections": [
            {"id": "d0", "name": "L0"},
            {"id": "d1", "name": "L1", "parentId": "d0"},
            {"id": "d2", "name": "L2", "parentId": "d1"},
            {"id": "d3", "name": "L3", "parentId": "d2"},
            {"id": "d4", "name": "L4", "parentId": "d3"},
            {"id": "cyc_a", "name": "CycA", "parentId": "cyc_b"},
            {"id": "cyc_b", "name": "CycB", "parentId": "cyc_a"},
        ],
        "order": ["d0", "d1", "d2", "d3", "d4", "cyc_a", "cyc_b", "people", "unassigned"],
        "agentAssignments": {},
        "agentOrder": [],
    }
    sh = _probe_ui(env, "openbot-shell", "openbot_shell", sidebar_layout=layout)
    depths = require_field(sh, "resolved_folder_depths", expected_type=dict)
    assert max(int(v) for v in depths.values()) <= 2
    assert "cyc_a" in depths and "cyc_b" in depths


@e2e_test(
    id="TEST-T2-OB-F14-04",
    tier=2,
    features=["OB-F14"],
    description="SidebarLayoutSnapshot rejects incomplete order (missing 'unassigned'), >100 sections, and >40-char section names.",
)
def test_t2_ob_f14_04_sidebar_layout_invalid_order_count_and_name_bounds(env: TempOpenBotEnv) -> None:
    r1 = _probe_ui_expect_error(
        env,
        "openbot-shell",
        ("unassigned", "missing", "incomplete", "required"),
        sidebar_layout={
            "revision": 1,
            "sections": [{"id": "s1", "name": "Team"}],
            "order": ["s1", "people"],
            "agentAssignments": {},
            "agentOrder": [],
        },
    )
    assert r1.returncode == 2

    secs_101 = [{"id": f"s_{i:x}", "name": f"Sec{i:x}"} for i in range(101)]
    r2 = _probe_ui_expect_error(
        env,
        "openbot-shell",
        ("100", "limit", "maximum", "too many"),
        sidebar_layout={
            "revision": 1,
            "sections": secs_101,
            "order": [*(f"s_{i:x}" for i in range(101)), "people", "unassigned"],
            "agentAssignments": {},
            "agentOrder": [],
        },
    )
    assert r2.returncode == 2

    r3 = _probe_ui_expect_error(
        env,
        "openbot-shell",
        ("40", "length", "limit", "long", "exceeds"),
        sidebar_layout={
            "revision": 1,
            "sections": [{"id": "long1", "name": "N" * 41}],
            "order": ["long1", "people", "unassigned"],
            "agentAssignments": {},
            "agentOrder": [],
        },
    )
    assert r3.returncode == 2


@e2e_test(
    id="TEST-T2-OB-F14-05",
    tier=2,
    features=["OB-F14"],
    description="SidebarPins rejects duplicate kind:id entries, unsupported kind 'folder', and 1001 > SIDEBAR_PIN_LIMIT (1000) pins.",
)
def test_t2_ob_f14_05_sidebar_pins_duplicate_invalid_kind_and_1000_cap(env: TempOpenBotEnv) -> None:
    r_dup = _probe_ui_expect_error(
        env,
        "openbot-shell",
        ("duplicate", "unique", "already pinned"),
        sidebar_pins=[{"kind": "agent", "id": "a1"}, {"kind": "agent", "id": "a1"}],
    )
    assert r_dup.returncode == 2
    r_kind = _probe_ui_expect_error(
        env,
        "openbot-shell",
        ("unsupported", "invalid", "pin kind", "invalid_kind"),
        sidebar_pins=[{"kind": "folder", "id": "f1"}],
    )
    assert r_kind.returncode == 2
    r_cap = _probe_ui_expect_error(
        env,
        "openbot-shell",
        ("1000", "limit", "maximum", "too many"),
        sidebar_pins=[{"kind": "agent", "id": f"a_{i:x}"} for i in range(1001)],
    )
    assert r_cap.returncode == 2


@e2e_test(
    id="TEST-T2-OB-F14-06",
    tier=2,
    features=["OB-F14"],
    description="reduced_motion disables GlobalSearchPlush look animation & beam() (0ms), and FirstAgentSetup rejects >32,000-char purpose.",
)
def test_t2_ob_f14_06_reduced_motion_shell_and_setup_purpose_cap(env: TempOpenBotEnv) -> None:
    sh = _probe_ui(env, "openbot-shell", "openbot_shell", reduced_motion=True)
    gsp = require_field(sh, "global_search_plush", expected_type=dict)
    assert require_field(gsp, "animated", expected_type=bool) is False
    assert require_field(sh, "beam_duration_ms", expected_type=int) == 0

    _probe_ui_expect_error(
        env,
        "openbot-shell",
        ("32000", "instruction", "limit", "maximum", "long"),
        setup_purpose="P" * 32_001,
    )


# ============================================================================
# OB-F15 Tier 2: 20,000-Char Boundary, RowBoundary, Pin Guard & ask_ui Bounds
# ============================================================================


@e2e_test(
    id="TEST-T2-OB-F15-01",
    tier=2,
    features=["OB-F15"],
    description="Long-message collapse enforces exact 20,000 vs 20,001 boundary, skips collapse while streaming, and preserves UTF-16 surrogate pairs.",
)
def test_t2_ob_f15_01_long_message_20000_boundary_streaming_and_surrogate_safety(
    env: TempOpenBotEnv,
) -> None:
    c20k = _probe_ui(
        env, "conversation-virtualized", "conversation", message_length=20000, streaming=False
    )
    assert require_field(c20k, "collapsed", expected_type=bool) is False

    # Place surrogate-pair emoji 😀 (2 UTF-16 code units) straddling index 2999..3000 inside a >20,000-char string
    surrogate_text = ("A" * 2999) + "😀" + ("B" * 17_005)
    c20k1 = _probe_ui(
        env, "conversation-virtualized", "conversation", message_text=surrogate_text, streaming=False
    )
    assert require_field(c20k1, "collapsed", expected_type=bool) is True
    assert require_field(c20k1, "preview_length", expected_type=int) <= 3000
    assert require_field(c20k1, "preview_surrogate_valid", expected_type=bool) is True

    c_stream = _probe_ui(
        env, "conversation-virtualized", "conversation", message_length=25000, streaming=True
    )
    assert require_field(c_stream, "collapsed", expected_type=bool) is False


@e2e_test(
    id="TEST-T2-OB-F15-02",
    tier=2,
    features=["OB-F15"],
    description="ChatRowBoundary isolates a corrupt/throwing message row without unmounting sibling timeline rows.",
)
def test_t2_ob_f15_02_chat_row_boundary_corrupt_row_isolation(env: TempOpenBotEnv) -> None:
    cv = _probe_ui(
        env, "conversation-virtualized", "conversation", inject_corrupt_row=True
    )
    assert require_field(cv, "row_boundary_isolated", expected_type=bool) is True
    assert require_field(cv, "sibling_rows_rendered", expected_type=int) >= 2


@e2e_test(
    id="TEST-T2-OB-F15-03",
    tier=2,
    features=["OB-F15"],
    timeout_sec=60.0,
    description="Agent is strictly barred from unpinning a human pin ('pinned-by-person'), and 51st pin > MESSAGE_PINS_LIMIT (50) is refused.",
)
def test_t2_ob_f15_03_human_pin_cannot_be_unpinned_by_agent_and_50_pin_cap(
    env: TempOpenBotEnv,
) -> None:
    aid = _mk_agent(env, "PinGuardBot")
    sent = env.run_cli_json("chat", "send", "--agent-id", aid, "--text", "Human protected message")
    mid = require_field(sent, "message_id", expected_type=str)

    pinned = env.run_cli_json(
        "chat", "pin", "--agent-id", aid, "--message-id", mid, "--pinned-by", "user"
    )
    assert require_field(pinned, "pinned", expected_type=bool) is True
    assert require_field(pinned, "pinned_by_kind", expected_type=str) == "user"

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

    for idx in range(2, 51):
        s_i = env.run_cli_json("chat", "send", "--agent-id", aid, "--text", f"Pin item {idx}")
        m_i = require_field(s_i, "message_id", expected_type=str)
        p_i = env.run_cli_json(
            "chat", "pin", "--agent-id", aid, "--message-id", m_i, "--pinned-by", "user"
        )
        assert require_field(p_i, "pinned", expected_type=bool) is True

    s_51 = env.run_cli_json("chat", "send", "--agent-id", aid, "--text", "Pin item overflow")
    m_51 = require_field(s_51, "message_id", expected_type=str)
    env.run_cli_expect_rejection(
        "chat",
        "pin",
        "--agent-id",
        aid,
        "--message-id",
        m_51,
        "--pinned-by",
        "user",
        error_contains=("50", "limit", "maximum", "too many"),
    )


@e2e_test(
    id="TEST-T2-OB-F15-04",
    tier=2,
    features=["OB-F15"],
    description="ask_ui confirm enforces confirmHold [400, 3000]ms integer bounds, premature hold rejection, <=4 unique actions, and no select='action'.",
)
def test_t2_ob_f15_04_ask_ui_confirm_hold_actions_and_field_bounds(env: TempOpenBotEnv) -> None:
    for valid_hold in (400, 3000):
        ok_cv = _probe_ui(
            env,
            "conversation-virtualized",
            "conversation",
            ui_block_spec={
                "version": 1,
                "type": "confirm",
                "title": "Valid",
                "confirmHold": valid_hold,
                "actions": [{"id": "go", "label": "Go", "confirm": True}],
            },
            hold_duration_ms=valid_hold,
        )
        assert require_field(require_field(ok_cv, "ui_block", expected_type=dict), "hold_accepted", expected_type=bool) is True

    short_hold = _probe_ui(
        env,
        "conversation-virtualized",
        "conversation",
        ui_block_spec={
            "version": 1,
            "type": "confirm",
            "title": "Hold 1200",
            "confirmHold": 1200,
            "actions": [{"id": "go", "label": "Go", "confirm": True}],
        },
        hold_duration_ms=350,
    )
    assert require_field(require_field(short_hold, "ui_block", expected_type=dict), "hold_accepted", expected_type=bool) is False

    invalid_specs = [
        {"version": 1, "type": "confirm", "title": "Low", "confirmHold": 399, "actions": [{"id": "a", "label": "A"}]},
        {"version": 1, "type": "confirm", "title": "High", "confirmHold": 3001, "actions": [{"id": "a", "label": "A"}]},
        {"version": 1, "type": "confirm", "title": "Float", "confirmHold": 1200.5, "actions": [{"id": "a", "label": "A"}]},
        {
            "version": 1,
            "type": "confirm",
            "title": "ManyActs",
            "actions": [{"id": f"a_{chr(97 + i)}", "label": f"Act {chr(65 + i)}"} for i in range(5)],
        },
        {
            "version": 1,
            "type": "confirm",
            "title": "SameLabel",
            "actions": [{"id": "a1", "label": "Ship"}, {"id": "a2", "label": "ship"}],
        },
        {
            "version": 1,
            "type": "confirm",
            "title": "BadSel",
            "actions": [{"id": "a1", "label": "Ok"}],
            "fields": [{"label": "Act", "select": "action", "options": ["x"]}],
        },
    ]
    for bad in invalid_specs:
        _probe_ui_expect_error(
            env,
            "conversation-virtualized",
            ("400", "3000", "integer", "u32", "at most 4", "too many", "limit", "maximum", "duplicate", "reserved", "invalid"),
            ui_block_spec=bad,
        )


@e2e_test(
    id="TEST-T2-OB-F15-05",
    tier=2,
    features=["OB-F15"],
    description="ask_ui quick_replies (1..=8 options) and choice (1..=20 options, single-select when multiple=false) reject out-of-bounds specs.",
)
def test_t2_ob_f15_05_ask_ui_quick_replies_and_choice_bounds(env: TempOpenBotEnv) -> None:
    for n_qr in (0, 9):
        _probe_ui_expect_error(
            env,
            "conversation-virtualized",
            ("8", "empty", "at least", "limit", "range", "bounds", "too many"),
            ui_block_spec={
                "version": 1,
                "type": "quick_replies",
                "title": "QR",
                "options": [{"id": f"opt_{chr(97 + i)}", "label": f"Opt {chr(65 + i)}"} for i in range(n_qr)],
            },
        )

    for n_ch in (0, 21):
        _probe_ui_expect_error(
            env,
            "conversation-virtualized",
            ("20", "empty", "at least", "limit", "range", "bounds", "too many"),
            ui_block_spec={
                "version": 1,
                "type": "choice",
                "title": "Choice",
                "options": [{"id": f"ch_{chr(97 + i)}", "label": f"Ch {chr(65 + i)}"} for i in range(n_ch)],
            },
        )

    r_multi = _probe_ui_expect_error(
        env,
        "conversation-virtualized",
        ("single", "one option", "at most one", "multiple is false", "multiple=false", "invalid"),
        ui_block_spec={
            "version": 1,
            "type": "choice",
            "title": "OnePick",
            "multiple": False,
            "options": [{"id": "c1", "label": "One"}, {"id": "c2", "label": "Two"}],
        },
        ui_block_response={"actionId": "submit", "values": {"selected": ["c1", "c2"]}},
    )
    assert r_multi.returncode == 2


@e2e_test(
    id="TEST-T2-OB-F15-06",
    tier=2,
    features=["OB-F15"],
    description="ask_ui form rejects >20 fields, segmented >6, select >50, invalid calendar date, secret textarea/prefill, and reserved IDs.",
)
def test_t2_ob_f15_06_ask_ui_form_field_secret_date_and_reserved_id_guards(
    env: TempOpenBotEnv,
) -> None:
    invalid_forms = [
        {
            "version": 1,
            "type": "form",
            "title": "FormA",
            "fields": [{"id": f"f_{chr(97 + i)}", "kind": "text", "label": f"F{chr(65 + i)}"} for i in range(21)],
        },
        {
            "version": 1,
            "type": "form",
            "title": "FormB",
            "fields": [{"id": "s1", "kind": "segmented", "label": "S", "options": [f"v_{chr(97 + i)}" for i in range(7)]}],
        },
        {
            "version": 1,
            "type": "form",
            "title": "FormC",
            "fields": [{"id": "s1", "kind": "select", "label": "S", "options": [f"v_{i:x}" for i in range(51)]}],
        },
        {
            "version": 1,
            "type": "form",
            "title": "FormD",
            "fields": [{"id": "sec", "kind": "textarea", "label": "Sec", "secret": True}],
        },
        {
            "version": 1,
            "type": "form",
            "title": "FormE",
            "fields": [{"id": "sec", "kind": "text", "label": "Sec", "secret": True, "value": "tok-1"}],
        },
        {
            "version": 1,
            "type": "form",
            "title": "FormF",
            "fields": [{"id": "botId", "kind": "text", "label": "Legacy"}],
        },
        {
            "version": 1,
            "type": "form",
            "title": "FormG",
            "fields": [{"id": "_hidden", "kind": "text", "label": "Hidden"}],
        },
    ]
    for spec in invalid_forms:
        r_spec = _probe_ui_expect_error(
            env,
            "conversation-virtualized",
            ("20", "6", "50", "limit", "too many", "single-line", "not allowed", "prefill", "reserved", "underscore", "invalid"),
            ui_block_spec=spec,
        )
        assert r_spec.returncode == 2

    r_date = _probe_ui_expect_error(
        env,
        "conversation-virtualized",
        ("calendar", "invalid date", "invalid_date", "leap", "out of range", "day", "invalid"),
        ui_block_spec={
            "version": 1,
            "type": "form",
            "title": "DateForm",
            "fields": [{"id": "dt", "kind": "date", "label": "Date"}],
        },
        ui_block_response={"actionId": "submit", "values": {"dt": "2025-02-29"}},
    )
    assert r_date.returncode == 2
