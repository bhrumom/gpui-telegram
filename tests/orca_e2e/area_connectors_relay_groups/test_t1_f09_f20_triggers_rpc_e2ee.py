"""Tier 1 Opaque-Box E2E tests for F09 (Connectors/Triggers/Webhooks/Waits/Wake) & F20 (RPC v3/E2EE v2/VM/STT/Artifacts/Watchdog)."""

from __future__ import annotations

import base64
import json
from typing import Any

import pytest

from harness import (
    E2EE_V2_KEY_SCHEDULE_BYTES,
    MIN_COMPATIBLE_PROTOCOL_VERSION,
    RUNTIME_PROTOCOL_VERSION,
    TRIGGER_SOURCES_6_KINDS_31,
    WATCH_PR_MAX_FIX_ATTEMPTS,
    WEBHOOK_ADAPTERS_6,
    WEBHOOK_FILTER_OPS_6,
    AuthSession,
    OrcaApiClient,
    OrcaCliRunner,
    TempWorkspace,
)


def _require_id(data: Any, label: str) -> dict[str, Any]:
    assert isinstance(data, dict) and data.get("id"), f"missing {label} id: {data!r}"
    return data


def _create_bot(api: OrcaApiClient, name: str, *, agent: str = "claude") -> dict[str, Any]:
    return _require_id(api.post("/api/v1/orca/bots", json_body={"name": name, "agent": agent}).assert_status(200, 201).json(), f"bot {name}")


def _create_webhook(api: OrcaApiClient, bot_id: str, name: str, adapter: str = "standard", **extra: Any) -> dict[str, Any]:
    return _require_id(api.post("/api/v1/orca/webhooks", json_body={"botId": bot_id, "name": name, "adapter": adapter, **extra}).assert_status(200, 201).json(), f"webhook {name}")


def _create_automation(api: OrcaApiClient, body: dict[str, Any], label: str = "automation") -> dict[str, Any]:
    return _require_id(api.post("/api/v1/orca/automations", json_body=body).assert_status(200, 201).json(), label)


def _unwrap_cli(payload: Any) -> Any:
    if isinstance(payload, dict):
        if payload.get("success") is True and "data" in payload:
            return payload["data"]
        if payload.get("ok") is True and "result" in payload:
            return payload["result"]
    return payload


def _b64_bytes(length: int, fill: int) -> str:
    return base64.b64encode(bytes([fill]) * length).decode("ascii")


# ---------------------------------------------------------------------------
# F09 — Connectors, Acts For, Access Control, Triggers, Webhooks, Waits & Wake (Tier 1)
# ---------------------------------------------------------------------------


@pytest.mark.tier1
@pytest.mark.f09
def test_f09_t1_acts_for_attribution_and_access_request_approval_decline(
    orca_api: OrcaApiClient, orca_cli: OrcaCliRunner
) -> None:
    bot = _create_bot(orca_api, "TriageBot")
    bot_id = bot["id"]

    dana_req = _require_id(
        orca_api.post(
            f"/api/v1/orca/bots/{bot_id}/access",
            json_body={"connector": "slack", "externalUserId": "U_DANA", "displayName": "Dana"},
        ).assert_status(200, 201).json(),
        "dana access request",
    )
    eve_req = _require_id(
        orca_api.post(
            f"/api/v1/orca/bots/{bot_id}/access",
            json_body={"connector": "slack", "externalUserId": "U_EVE", "displayName": "Eve"},
        ).assert_status(200, 201).json(),
        "eve access request",
    )
    dana_req_id, eve_req_id = dana_req["id"], eve_req["id"]

    pending_rest = orca_api.get(f"/api/v1/orca/bots/{bot_id}/access").assert_status(200).json()
    assert dana_req_id in json.dumps(pending_rest) and eve_req_id in json.dumps(pending_rest)
    pending_cli = _unwrap_cli(orca_cli.run("bot", "access-list", bot_id).assert_ok())
    assert dana_req_id in json.dumps(pending_cli)

    orca_api.post(
        f"/api/v1/orca/bots/{bot_id}/access/decide",
        json_body={"requestId": dana_req_id, "decision": "granted"},
    ).assert_status(200)
    orca_api.post(
        f"/api/v1/orca/bots/{bot_id}/access/decide",
        json_body={"requestId": dana_req_id, "decision": "granted"},
    ).assert_status(200)
    orca_cli.run("bot", "access-decide", bot_id, eve_req_id, "--decision", "declined").assert_ok()

    bot_after = orca_api.get(f"/api/v1/orca/bots/{bot_id}").assert_status(200).json()
    assert isinstance(bot_after, dict), f"expected bot dict: {bot_after!r}"
    members = bot_after.get("members") or []
    dana_entries = [m for m in members if "U_DANA" in json.dumps(m) or "Dana" in json.dumps(m)]
    assert len(dana_entries) == 1, f"expected Dana added idempotently once, got {members!r}"
    assert "U_EVE" not in json.dumps(members)

    comment_res = orca_api.assert_rpc_ok(
        "bots.connectors.externalAction",
        {"botId": bot_id, "action": "comment", "text": "Investigating regression", "actsFor": {"id": "U_DANA", "name": "Dana", "isOwner": False}},
    )
    assert "asked by Dana via TriageBot" in json.dumps(comment_res)
    merge_resp = orca_api.rpc(
        "bots.connectors.externalAction",
        {"botId": bot_id, "action": "merge_pr", "actsFor": {"id": "U_DANA", "name": "Dana", "isOwner": False}},
    )
    orca_api.assert_rpc_error(merge_resp, "not_owner")


@pytest.mark.tier1
@pytest.mark.f09
def test_f09_t1_event_triggers_all_31_kinds_across_6_sources(
    orca_api: OrcaApiClient, orca_cli: OrcaCliRunner
) -> None:
    assert len(TRIGGER_SOURCES_6_KINDS_31) == 6
    total_kinds = sum(len(kinds) for kinds in TRIGGER_SOURCES_6_KINDS_31.values())
    assert total_kinds == 31
    bot = _create_bot(orca_api, "TriggerBot")

    created_ids: list[str] = []
    for source, kinds in TRIGGER_SOURCES_6_KINDS_31.items():
        for kind in kinds:
            trigger: dict[str, Any] = {"source": source, "kind": kind}
            if source == "github" and kind == "pr_opened":
                trigger["fromOwner"] = True
            auto = _create_automation(
                orca_api,
                {
                    "botId": bot["id"],
                    "name": f"trig-{source}-{kind}",
                    "schedule": {"dtstart": "9999-01-01"},
                    "trigger": trigger,
                    "runsIn": "new" if kind.startswith("pr_") else "chat",
                    "resultsTo": "chat",
                },
                f"trig-{source}-{kind}",
            )
            if source in ("gmail", "calendar"):
                assert auto.get("localOnly") is True or auto.get("executionScope") == "local"
            created_ids.append(auto["id"])

    listed = orca_api.get("/api/v1/orca/automations").assert_status(200).json()
    items = listed if isinstance(listed, list) else (listed.get("automations") if isinstance(listed, dict) else None)
    assert isinstance(items, list), f"expected automations list: {listed!r}"
    listed_ids = {item.get("id") for item in items if isinstance(item, dict)}
    assert set(created_ids) <= listed_ids

    cli_created = _unwrap_cli(
        orca_cli.run("automations", "create", "--bot", bot["id"], "--name", "cli-trig", "--source", "slack", "--kind", "mentioned").assert_ok()
    )
    cli_id = _require_id(cli_created, "cli automation")["id"]
    cli_listed = _unwrap_cli(orca_cli.run("automations", "list").assert_ok())
    assert cli_id in json.dumps(cli_listed)
    orca_cli.run("automations", "remove", cli_id).assert_ok()
    orca_api.delete(f"/api/v1/orca/automations/{created_ids[0]}").assert_status(200, 204)


@pytest.mark.tier1
@pytest.mark.f09
def test_f09_t1_webhooks_6_adapters_and_6_filter_ops(orca_api: OrcaApiClient) -> None:
    assert len(WEBHOOK_ADAPTERS_6) == 6 and len(WEBHOOK_FILTER_OPS_6) == 6
    bot = _create_bot(orca_api, "HookAdapterBot")
    op_cases = [
        ("=", "event", "deploy", {"event": "deploy", "env": "prod"}, {"event": "noop"}),
        ("!=", "env", "dev", {"env": "prod"}, {"env": "dev"}),
        ("in", "severity", ["high", "critical"], {"severity": "critical"}, {"severity": "low"}),
        ("contains", "title", "checkout", {"title": "checkout timeout"}, {"title": "health ok"}),
        ("exists", "incident_id", True, {"incident_id": "INC-9"}, {"other": 1}),
        ("!exists", "dry_run", True, {"event": "paid"}, {"event": "paid", "dry_run": True}),
    ]
    for adapter, (op, field, value, match_body, miss_body) in zip(WEBHOOK_ADAPTERS_6, op_cases, strict=True):
        hook = _create_webhook(
            orca_api, bot["id"], f"hook-{adapter}", adapter,
            rules={"where": [{"path": field, "op": op, "value": value}], "pick": [field]},
        )
        hook_id = hook["id"]
        orca_api.post(f"/api/v1/orca/webhooks/{hook_id}/ingest", json_body=match_body).assert_status(200, 201, 202)
        orca_api.post(f"/api/v1/orca/webhooks/{hook_id}/ingest", json_body=miss_body).assert_status(200, 201, 202)
        calls_res = orca_api.get(f"/api/v1/orca/webhooks/{hook_id}/calls").assert_status(200).json()
        calls = calls_res if isinstance(calls_res, list) else (calls_res.get("calls") if isinstance(calls_res, dict) else None)
        assert isinstance(calls, list) and len(calls) >= 2, f"expected >=2 calls for {adapter}: {calls_res!r}"
        states = {str(c.get("state", "")).lower() for c in calls if isinstance(c, dict)}
        assert any("trigger" in s or "match" in s or "deliver" in s or "accepted" in s for s in states), f"no matched state for {adapter}: {calls!r}"
        assert any("filter" in s or "skip" in s or "drop" in s for s in states), f"no filtered state for {adapter}: {calls!r}"


@pytest.mark.tier1
@pytest.mark.f09
def test_f09_t1_watch_pr_two_attempt_autonomous_fix_loop(orca_api: OrcaApiClient, orca_cli: OrcaCliRunner) -> None:
    bot = _create_bot(orca_api, "WatchPrBot")
    watch = _require_id(
        orca_api.post("/api/v1/orca/automations/watch-pr", json_body={"botId": bot["id"], "repo": "acme/app", "prNumber": 42}).assert_status(200, 201).json(),
        "watch-pr",
    )
    watch_id = watch["id"]

    r1 = orca_api.assert_rpc_ok("automations.triggerEvent", {"automationId": watch_id, "source": "github", "kind": "ci_failed", "repo": "acme/app", "prNumber": 42, "runId": "run-1"})
    r2 = orca_api.assert_rpc_ok("automations.triggerEvent", {"automationId": watch_id, "source": "github", "kind": "ci_failed", "repo": "acme/app", "prNumber": 42, "runId": "run-2"})
    r3 = orca_api.assert_rpc_ok("automations.triggerEvent", {"automationId": watch_id, "source": "github", "kind": "ci_failed", "repo": "acme/app", "prNumber": 42, "runId": "run-3"})
    assert isinstance(r1, dict) and r1.get("attempts") == 1 and r1.get("dispatched") is True
    assert isinstance(r2, dict) and r2.get("attempts") == WATCH_PR_MAX_FIX_ATTEMPTS and r2.get("dispatched") is True
    assert isinstance(r3, dict) and r3.get("dispatched") is False and (r3.get("escalated") is True or r3.get("stopped") is True)

    cli_watch = _unwrap_cli(orca_cli.run("automations", "watch-pr", "--bot", bot["id"], "--repo", "acme/app", "--pr", "99").assert_ok())
    _require_id(cli_watch, "cli watch-pr")


@pytest.mark.tier1
@pytest.mark.f09
def test_f09_t1_wake_ws_content_free_frames_and_keepalive(
    orca_api: OrcaApiClient, auth_session: AuthSession
) -> None:
    bot = _create_bot(orca_api, "WakeBot")
    hook_id = _create_webhook(orca_api, bot["id"], "wake-hook")["id"]

    with orca_api.ws_connect("/api/v1/orca/wake?host_id=wake-host-bearer") as ws_b:
        ws_b.send_ping(b"hb-bearer")

    with orca_api.ws_connect(f"/api/v1/orca/wake?host_id=wake-host-query&token={auth_session.token}", include_auth=False) as ws:
        ws.send_ping(b"hb-query")
        orca_api.post(
            f"/api/v1/orca/webhooks/{hook_id}/ingest",
            json_body={"text": "TOP_SECRET_PAYLOAD_9911", "url": "https://secret.internal/x", "actor": "secret-actor", "place": "#private-room"},
        ).assert_status(200, 201, 202)
        frame = ws.recv_json(timeout=5.0)
        assert isinstance(frame, dict) and (frame.get("kind") or frame.get("event") or frame.get("type"))
        serialized = json.dumps(frame)
        assert "TOP_SECRET_PAYLOAD_9911" not in serialized and "secret.internal" not in serialized
        for forbidden_key in ("text", "url", "actor", "place"):
            assert forbidden_key not in frame, f"wake frame leaked forbidden key {forbidden_key}: {frame!r}"


@pytest.mark.tier1
@pytest.mark.f09
def test_f09_t1_webhooks_cli_list_show_calls_check_never_prints_url_or_secret(
    orca_api: OrcaApiClient, orca_cli: OrcaCliRunner
) -> None:
    bot = _create_bot(orca_api, "HookRedactionBot")
    created = _create_webhook(orca_api, bot["id"], "redacted-hook")
    hook_id = created["id"]
    ingest_url = str(created.get("url") or created.get("ingestUrl") or f"/api/v1/orca/webhooks/{hook_id}/ingest")
    secret = str(created.get("secret") or created.get("signingSecret") or "")
    assert secret, f"webhook creation must return initial secret once: {created!r}"

    orca_api.post(f"/api/v1/orca/webhooks/{hook_id}/ingest", json_body={"event": "ping"}).assert_status(200, 201, 202)
    rest_calls_raw = orca_api.get(f"/api/v1/orca/webhooks/{hook_id}/calls").assert_status(200).json()
    rest_calls = rest_calls_raw if isinstance(rest_calls_raw, list) else (rest_calls_raw.get("calls", []) if isinstance(rest_calls_raw, dict) else [])
    assert isinstance(rest_calls, list) and len(rest_calls) >= 1 and isinstance(rest_calls[0], dict), f"expected >=1 call after ingest: {rest_calls_raw!r}"
    call0_id = str(rest_calls[0].get("id") or rest_calls[0].get("callId") or "")
    call0_state = str(rest_calls[0].get("state") or rest_calls[0].get("status") or "")
    assert call0_id or call0_state, f"call record missing id/state: {rest_calls[0]!r}"

    status_keys = ("ok", "healthy", "status", "state", "valid", "active", "enabled", "reachable", "adapter", "checks")
    status_tokens = ("ok", "healthy", "active", "valid", "ready", "enabled", "pass", "standard")
    for json_mode in (True, False):
        for cmd in (("webhooks", "list"), ("webhooks", "show", hook_id), ("webhooks", "calls", hook_id), ("webhooks", "check", hook_id)):
            res = orca_cli.run(*cmd, json_mode=json_mode)
            res.assert_ok()
            assert res.stdout.strip(), f"CLI {cmd} (json={json_mode}) emitted empty stdout"
            subcmd = cmd[1]
            if json_mode:
                try:
                    parsed = _unwrap_cli(json.loads(res.stdout))
                except Exception as exc:
                    raise AssertionError(f"CLI {cmd} (--json) emitted invalid JSON stdout: {res.stdout!r}") from exc
                assert isinstance(parsed, (dict, list)) and len(parsed) > 0, f"CLI {cmd} (--json) returned empty payload: {parsed!r}"
                text_view = json.dumps(parsed)
                if subcmd in ("list", "show"):
                    assert hook_id in text_view and "redacted-hook" in text_view, f"CLI {cmd} (--json) missing hook id/name: {text_view!r}"
                elif subcmd == "calls":
                    calls = parsed if isinstance(parsed, list) else (parsed.get("calls") or parsed.get("items") or [])
                    assert isinstance(calls, list) and any(isinstance(c, dict) and (c.get("id") or c.get("callId") or c.get("state") or c.get("status")) for c in calls), f"CLI calls (--json) missing call record: {parsed!r}"
                else:
                    assert isinstance(parsed, dict) and any(k in parsed for k in status_keys) and (hook_id in text_view or "redacted-hook" in text_view or parsed.get("ok") is True or parsed.get("healthy") is True or bool(parsed.get("status")) or bool(parsed.get("state"))), f"CLI check (--json) missing status/hook ref: {parsed!r}"
            else:
                text_view = res.stdout
                if subcmd in ("list", "show"):
                    assert hook_id in text_view and "redacted-hook" in text_view, f"CLI {cmd} (text) missing hook id/name: {text_view!r}"
                elif subcmd == "calls":
                    assert (call0_id and call0_id in text_view) or (call0_state and call0_state.lower() in text_view.lower()) or hook_id in text_view, f"CLI calls (text) missing call record: {text_view!r}"
                else:
                    assert hook_id in text_view or "redacted-hook" in text_view or any(tok in text_view.lower() for tok in status_tokens), f"CLI check (text) missing status/hook ref: {text_view!r}"
            combined = f"{res.stdout}\n{res.stderr}"
            assert secret not in combined and ingest_url not in combined and "/ingest" not in combined, f"CLI {cmd} (json={json_mode}) leaked secret/URL"


# ---------------------------------------------------------------------------
# F20 — Unified RPC Protocol v3, E2EE v2, Artifacts, STT, Watchdog & VMs (Tier 1)
# ---------------------------------------------------------------------------


@pytest.mark.tier1
@pytest.mark.f20
def test_f20_t1_rpc_protocol_v3_capability_negotiation(orca_api: OrcaApiClient) -> None:
    res_v3 = orca_api.assert_rpc_ok("runtime.capabilities", {}, protocol_version=RUNTIME_PROTOCOL_VERSION, caller_scope="desktop")
    assert isinstance(res_v3, dict)
    assert res_v3.get("protocol_version") == RUNTIME_PROTOCOL_VERSION or res_v3.get("protocolVersion") == RUNTIME_PROTOCOL_VERSION
    min_ver = res_v3.get("min_compatible_version") or res_v3.get("minCompatibleVersion") or res_v3.get("min_protocol_version")
    assert min_ver == MIN_COMPATIBLE_PROTOCOL_VERSION, f"unexpected min_compatible_version: {res_v3!r}"
    caps = res_v3.get("capabilities")
    assert isinstance(caps, (list, dict)) and len(caps) > 0, f"expected non-empty capabilities: {res_v3!r}"

    res_v2 = orca_api.assert_rpc_ok("runtime.capabilities", {}, protocol_version=MIN_COMPATIBLE_PROTOCOL_VERSION, caller_scope="desktop")
    assert isinstance(res_v2, dict) and (res_v2.get("capabilities") is not None)


@pytest.mark.tier1
@pytest.mark.f20
def test_f20_t1_e2ee_v2_handshake_key_schedule_96b(orca_api: OrcaApiClient) -> None:
    req_body = {
        "protocol": "orca-mobile-e2ee/v2",
        "initiator": "mobile",
        "responder": "desktop",
        "transport": "relay",
        "relay_host_id": "AbCdEfGhIjKlMnOp",
        "client_ephemeral_pubkey": _b64_bytes(32, 0x11),
        "client_nonce": _b64_bytes(32, 0x22),
    }
    h1 = orca_api.post("/api/v1/orca/ide/e2ee/handshake", json_body=req_body).assert_status(200, 201).json()
    assert isinstance(h1, dict) and h1.get("protocol") == "orca-mobile-e2ee/v2"
    assert isinstance(h1.get("responder_ephemeral_pubkey"), str) and len(base64.b64decode(h1["responder_ephemeral_pubkey"])) == 32
    assert isinstance(h1.get("desktop_nonce"), str) and len(base64.b64decode(h1["desktop_nonce"])) == 32
    transcript = str(h1.get("transcript_hash", ""))
    assert len(transcript) >= 32, f"invalid transcript_hash: {h1!r}"
    assert h1.get("key_schedule_bytes") == E2EE_V2_KEY_SCHEDULE_BYTES
    assert h1.get("session_id") and h1.get("mobile_to_desktop_key") and h1.get("desktop_to_mobile_key")

    h2 = orca_api.post(
        "/api/v1/orca/ide/e2ee/handshake",
        json_body={**req_body, "client_nonce": _b64_bytes(32, 0x33)},
    ).assert_status(200, 201).json()
    assert isinstance(h2, dict) and h2.get("session_id") and h2.get("transcript_hash")
    assert h2["session_id"] != h1["session_id"] and h2["transcript_hash"] != h1["transcript_hash"]


@pytest.mark.tier1
@pytest.mark.f20
def test_f20_t1_shareable_artifacts_create_list_hides_edit_token(orca_api: OrcaApiClient) -> None:
    created = _require_id(
        orca_api.post(
            "/api/v1/orca/ide/artifacts",
            json_body={"filename": "report.md", "content": "# Sprint Summary\nAll green.", "content_type": "text/markdown", "explicit_user_intent": True},
        ).assert_status(200, 201).json(),
        "artifact",
    )
    art_id = created["id"]
    share_url = created.get("url") or created.get("share_url")
    assert art_id and share_url

    listed = orca_api.get("/api/v1/orca/ide/artifacts").assert_status(200).json()
    items = listed if isinstance(listed, list) else (listed.get("artifacts") if isinstance(listed, dict) else None)
    assert isinstance(items, list) and any(isinstance(a, dict) and a.get("id") == art_id for a in items)
    for item in items:
        assert isinstance(item, dict) and "edit_token" not in item and "editToken" not in item

    detail = orca_api.get(f"/api/v1/orca/ide/artifacts/{art_id}").assert_status(200).json()
    assert isinstance(detail, dict) and detail.get("filename") == "report.md"
    assert "edit_token" not in detail and "editToken" not in detail


@pytest.mark.tier1
@pytest.mark.f20
def test_f20_t1_local_stt_gpu_fallback_marker_and_status(orca_api: OrcaApiClient) -> None:
    orca_api.assert_rpc_ok("speech.recordGpuFailure", {"reason": "metal_oom_simulated"})
    status = orca_api.assert_rpc_ok("speech.status", {})
    assert isinstance(status, dict)
    assert status.get("gpu_fallback_active") is True or status.get("gpuFallbackActive") is True
    backend = str(status.get("backend") or status.get("active_backend") or "").lower()
    assert backend == "cpu", f"expected CPU fallback after GPU failure marker, got {status!r}"


@pytest.mark.tier1
@pytest.mark.f20
def test_f20_t1_hang_watchdog_sleep_aware_guard(orca_api: OrcaApiClient) -> None:
    awake = orca_api.assert_rpc_ok(
        "watchdog.evaluate",
        {"lastHeartbeatDeltaMs": 45_000, "systemSlept": False, "wakeDeltaMs": 0},
    )
    assert isinstance(awake, dict) and awake.get("status") in {"warn", "warning", "hung", "critical"}
    assert awake.get("suppressed_by_sleep") is not True and awake.get("suppressedBySleep") is not True

    post_sleep = orca_api.assert_rpc_ok(
        "watchdog.evaluate",
        {"lastHeartbeatDeltaMs": 45_000, "systemSlept": True, "wakeDeltaMs": 44_500},
    )
    assert isinstance(post_sleep, dict) and (post_sleep.get("suppressed_by_sleep") is True or post_sleep.get("suppressedBySleep") is True)
    assert post_sleep.get("status") not in {"hung", "critical"}


@pytest.mark.tier1
@pytest.mark.f20
def test_f20_t1_headless_serve_and_ephemeral_vm_recipe_doctor(
    orca_api: OrcaApiClient, temp_workspace: TempWorkspace
) -> None:
    clean_user_data = temp_workspace.mkdir("clean_vm_user_data")
    doctor = orca_api.assert_rpc_ok(
        "vm.recipeDoctor",
        {
            "user_data_dir": str(clean_user_data),
            "recipe": {
                "name": "ephemeral-linux",
                "create": "vm-ctl create",
                "suspend": "vm-ctl suspend",
                "resume": "vm-ctl resume",
                "destroy": "vm-ctl destroy",
            },
        },
    )
    assert isinstance(doctor, dict) and (doctor.get("healthy") is True or doctor.get("contaminated") is False)
    assert (doctor.get("blocking_issues") or doctor.get("issues") or []) == []

    serve_status = orca_api.assert_rpc_ok("serve.status", {})
    assert isinstance(serve_status, dict)
    assert serve_status.get("headless") is True or serve_status.get("mode") == "headless"
    assert serve_status.get("reachable") is True or serve_status.get("status") in {"ready", "ok", "running"}
