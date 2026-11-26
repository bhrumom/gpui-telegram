"""Tier 4 realistic end-to-end workflows (T4-11..T4-14): Security, Connectors, Remote & Release."""

from __future__ import annotations

import base64
import concurrent.futures
import http.client
import json
import os
import re
import stat
import subprocess
import threading
import time
import tomllib
from pathlib import Path
from typing import Any
from urllib.parse import parse_qs, urlparse

import pytest

from harness import (
    E2EE_V2_KEY_SCHEDULE_BYTES, MIN_COMPATIBLE_PROTOCOL_VERSION, REPO_ROOT,
    RUNTIME_PROTOCOL_VERSION, SUITE_ROOT, TARGET_RELEASE_VERSION, TEMPLATE_IDS_24,
    USAGE_PROVIDERS_9, WEBHOOK_ACCOUNT_HOURLY_CAP, WEBHOOK_ADAPTERS_6,
    WEBHOOK_FILTER_OPS_6, WEBHOOK_HOOK_HOURLY_CAP, WS_CLOSE_MESSAGE_TOO_BIG,
    WS_CLOSE_SUPERSEDED, MockMcpUpstreamServer, MockOAuthServer, MockWebhookSink,
    OrcaApiClient, OrcaCliRunner, RawHttpClient, TempWorkspace,
)


def _req(mapping: Any, *keys: str) -> Any:
    assert isinstance(mapping, dict), f"Expected JSON dict, got {type(mapping).__name__}: {mapping!r}"
    for key in keys:
        if key in mapping and mapping[key] is not None:
            return mapping[key]
    raise AssertionError(f"Missing required key(s) {keys!r} in {sorted(mapping.keys())}: {mapping!r}")


def _unwrap(mapping: Any, *keys: str) -> Any:
    cur = mapping["data"] if isinstance(mapping, dict) and mapping.get("success") is True and "data" in mapping else (
        mapping["result"] if isinstance(mapping, dict) and mapping.get("ok") is True and "result" in mapping else mapping
    )
    return next((cur[k] for k in keys if isinstance(cur, dict) and isinstance(cur.get(k), dict)), cur)


def _get(api: OrcaApiClient, path: str, params: dict[str, Any] | None = None) -> Any:
    return api.assert_ok(api.get(path, params=params))


def _post(api: OrcaApiClient, path: str, body: dict[str, Any]) -> Any:
    return api.assert_ok(api.post(path, json_body=body))


def _assert_rpc_err_in(api: OrcaApiClient, method: str, codes: tuple[str, ...], params: dict[str, Any] | None = None, **kw: Any) -> None:
    resp = api.rpc(method, params or {}, **kw)
    assert resp.status in (200, 400, 403, 409, 422, 426) and resp.status not in (404, 500, 501), f"Bad RPC status ({resp.status}): {resp.text[:300]}"
    body = resp.json()
    assert isinstance(body, dict) and body.get("ok") is False, f"Expected ok=false: {body!r}"
    assert str(_req(_req(body, "error"), "code")) in codes, f"Unexpected error code in {body!r}"


def _extract_relay_grant(api: OrcaApiClient, bot_id: str) -> tuple[int, str, str]:
    info = api.assert_rpc_ok("bots.tools.relayInfo", {"botId": bot_id})
    grant = api.assert_rpc_ok("bots.tools.grant", {"botId": bot_id})
    port = int(_req(info if isinstance(info, dict) and "port" in info else grant, "port", "relayPort"))
    token = str(_req(grant, "token", "bearer", "bearerToken"))
    base = str((grant.get("path") or grant.get("basePath") or info.get("basePath") or "") if isinstance(grant, dict) and isinstance(info, dict) else "")
    assert port > 0 and len(token) >= 8
    return port, token, base


def _assert_ledger_closure(led: subprocess.CompletedProcess[str]) -> None:
    """Enforce Gates 1-5 & 6.1-6.7 cleanly on both macOS (staged bundle) and Linux (no .app)."""
    for token in (
        "PASS [Gate 1]:", "PASS [Gate 2]:", "PASS [Gate 3]:", "PASS [Gate 4]:", "PASS [Gate 5]:",
        "[PASS] Gate 6.1 (Inherited Paths):", "[PASS] Gate 6.1 (Target Files):",
        "[PASS] Gate 6.2 (Target Symbols):", "[PASS] Gate 6.2 (Boilerplate Burndown):",
        "[PASS] Gate 6.2 (Forensic Claim Closure):", "[PASS] Gate 6.3 (Workspace Compilation):",
        "[PASS] Gate 6.4 (Cargo Dependencies):", "[PASS] Gate 6.5 (Authentic Imports):",
        "[PASS] Gate 6.6 (Module Source Coverage):",
    ):
        assert token in led.stdout, f"Ledger gate failed ({token}):\n{led.stdout[-2000:]}"

    macos_bundle = "dist/GPUI Telegram.app/Contents/MacOS/gpui_telegram_desktop"
    if (REPO_ROOT / macos_bundle).is_file() or led.returncode == 0:
        assert led.returncode == 0 and "[PASS] Gate 6.7 (Capability Coverage):" in led.stdout
        assert "ALL 6 VERIFICATION GATES PASSED SUCCESSFULLY" in led.stdout
        return

    fail_gates = [ln.strip() for ln in led.stdout.splitlines() if ln.strip().startswith("[FAIL] Gate 6.")]
    assert len(fail_gates) == 1 and fail_gates[0].startswith("[FAIL] Gate 6.7 (Capability Coverage):"), fail_gates
    m_hdr = re.search(r"FAIL \[Gate 6\.7 - Capability Coverage\]: (\d+) stage invariant violation", led.stdout)
    m_sum = re.search(r"\[FAIL\] Gate 6\.7 \(Capability Coverage\): (\d+) violations", led.stdout)
    assert m_hdr and m_sum and int(m_hdr.group(1)) == int(m_sum.group(1)), led.stdout[-1500:]
    reported = int(m_hdr.group(1))

    cov = json.loads((REPO_ROOT / "docs/verification/fusion-20261108/capability-coverage.json").read_text(encoding="utf-8"))
    caps = _req(cov, "capabilities")
    assert isinstance(caps, list) and len(caps) > 0
    bundle_caps = 0
    for cap in caps:
        es = cap.get("evidence_stages", {})
        has_bundle = False
        for st_name in ("ui_evidence_captured", "packaged_accepted"):
            st = es.get(st_name, {})
            if st.get("status") != "verified":
                continue
            arts = _req(st, "artifacts")
            assert isinstance(arts, list) and arts, f"{cap.get('capability_id')}: empty {st_name} artifacts"
            for art in arts:
                if st_name == "packaged_accepted" and art == macos_bundle:
                    has_bundle = True
                    continue
                art_path = REPO_ROOT / art
                assert art_path.exists(), f"{cap.get('capability_id')}: missing {st_name} artifact {art}"
                if art.lower().endswith(".png"):
                    raw_png = art_path.read_bytes()
                    assert len(raw_png) > 10_240 and raw_png.startswith(b"\x89PNG\r\n\x1a\n"), f"Bad PNG: {art}"
        if has_bundle:
            bundle_caps += 1
    assert bundle_caps > 0 and reported == 2 * bundle_caps, f"Expected 2*{bundle_caps}={2 * bundle_caps} viols, got {reported}"
    viols = [ln.strip() for ln in led.stdout.splitlines() if ln.strip().startswith("Violation:")]
    allowed = (f": packaged_accepted artifact missing on disk: '{macos_bundle}'", ": packaged_accepted=verified requires an existing packaged binary/app artifact")
    assert len(viols) == min(10, reported) and all(any(v.endswith(sfx) for sfx in allowed) for v in viols), f"Unexpected Gate 6.7 violation(s): {viols}"


@pytest.mark.tier4
@pytest.mark.f08
@pytest.mark.edge_case("SM2-12")
@pytest.mark.edge_case("SM2-13")
def test_t4_11_tool_relay_oauth21_security_gauntlet(orca_api: OrcaApiClient, bot_home_factory: Any, mock_mcp_upstream: MockMcpUpstreamServer, mock_oauth_server: MockOAuthServer) -> None:
    """Workflow #11: OAuth 2.1 DCR/PKCE -> Tool Relay CSRF/401/404/429 -> 401 refresh -> SSE revoke abort."""
    mock_mcp_upstream.fail_401_once = True
    bot_home = bot_home_factory("relay-gauntlet")
    bot = _unwrap(_post(orca_api, "/api/v1/orca/bots", {"name": "Relay Gauntlet Bot", "agent": "claude", "folder": "relay-gauntlet", "homePath": str(bot_home.root)}), "bot")
    bot_id, upstream_url = str(_req(bot, "id", "botId", "bot_id")), f"{mock_mcp_upstream.base_url}/mcp"
    home_dir = Path(str(bot.get("homePath") or bot.get("home_path") or bot.get("homeDir") or bot.get("folderPath") or bot_home.root))
    tool = orca_api.assert_rpc_ok("bots.tools.add", {"botId": bot_id, "name": "mock-mcp", "transport": "http", "url": upstream_url, "oauth": {"issuer": mock_oauth_server.issuer, "allowLoopbackTest": True}})
    start = orca_api.assert_rpc_ok("bots.tools.oauthStart", {"botId": bot_id, "toolName": "mock-mcp"})
    assert mock_oauth_server.registered_clients, "RFC 7591 DCR POST /register not invoked"
    parsed_auth = urlparse(str(_req(start, "authorizationUrl", "authUrl")))
    qs = {k: v[0] for k, v in parse_qs(parsed_auth.query).items() if v}
    assert qs.get("code_challenge_method") == "S256" and qs.get("code_challenge") and qs.get("client_id") in mock_oauth_server.registered_clients
    assert qs.get("resource", "").rstrip("/") == upstream_url.rstrip("/")
    auth_resp = RawHttpClient(mock_oauth_server.host, mock_oauth_server.port).request("GET", f"{parsed_auth.path}?{parsed_auth.query}").assert_status(200).json()
    orca_api.assert_rpc_ok("bots.tools.oauthCallback", {"botId": bot_id, "toolName": "mock-mcp", "code": str(_req(auth_resp, "code")), "state": str(auth_resp.get("state") or qs.get("state") or ""), "iss": mock_oauth_server.issuer})
    old_access = set(mock_oauth_server.active_access_tokens)
    cid = qs["client_id"]
    secrets_set = old_access | mock_oauth_server.active_refresh_tokens | {str(mock_oauth_server.registered_clients[cid]["client_secret"]), "Authorization:"}
    assert home_dir.is_dir(), f"Expected bot home directory on disk: {home_dir}"
    checked_files = 0
    for path in home_dir.rglob("*"):
        if path.is_file():
            checked_files += 1
            content = path.read_bytes().decode("utf-8", errors="ignore")
            assert not any(sec in content for sec in secrets_set), f"Secret/header leaked in {path}"
    assert checked_files > 0, f"No bot home files scanned under {home_dir}; OAuth secret check would be vacuous"

    relay_port, grant, base_path = _extract_relay_grant(orca_api, bot_id)
    tool_path = str(tool.get("relayPath") or f"{base_path.rstrip('/')}/mock-mcp")
    relay, auth_hdr = RawHttpClient("127.0.0.1", relay_port), {"Authorization": f"Bearer {grant}"}
    relay.request("POST", tool_path, headers={**auth_hdr, "Origin": "https://evil.example"}, json_body={}).assert_status(403)
    relay.request("POST", tool_path, headers={**auth_hdr, "Host": "evil.attacker:1234"}, json_body={}).assert_status(403)
    relay.request("PUT", tool_path, headers=auth_hdr, json_body={}).assert_status(405)
    relay.request("OPTIONS", tool_path, headers=auth_hdr).assert_status(405)
    relay.request("POST", tool_path, headers={"Authorization": "Bearer bad-grant"}, json_body={}).assert_status(401)
    relay.request("POST", "/mcp/unauthorized-bot/tool", headers=auth_hdr, json_body={}).assert_status(404)

    ok_rpc = relay.request("POST", tool_path, headers=auth_hdr, json_body={"jsonrpc": "2.0", "id": 1, "method": "tools/list"}).assert_status(200).json()
    assert _req(_req(ok_rpc, "result"), "ok") is True and len(mock_oauth_server.refresh_calls) == 1
    refreshed_tok = mock_mcp_upstream.requests[-1].headers.get("authorization", "").removeprefix("Bearer ")
    assert refreshed_tok in mock_oauth_server.active_access_tokens and refreshed_tok not in old_access

    def _slow_upstream(_rec: Any) -> tuple[int, dict[str, str], bytes]:
        time.sleep(0.3)
        return 200, {"Content-Type": "application/json"}, b'{"jsonrpc":"2.0","id":1,"result":{"ok":true}}'

    def _burst_call(i: int) -> int:
        return RawHttpClient("127.0.0.1", relay_port).request("POST", tool_path, headers=auth_hdr, json_body={"jsonrpc": "2.0", "id": 100 + i, "method": "tools/list"}).status

    mock_mcp_upstream.custom_responder = _slow_upstream
    try:
        with concurrent.futures.ThreadPoolExecutor(max_workers=24) as pool:
            statuses = list(pool.map(_burst_call, range(48)))
    finally:
        mock_mcp_upstream.custom_responder = None
    assert 200 in statuses and 429 in statuses, f"Expected 200+429 admission cap mix, got {statuses}"

    sse_tool = orca_api.assert_rpc_ok("bots.tools.add", {"botId": bot_id, "name": "sse-mcp", "transport": "http", "url": f"{mock_mcp_upstream.base_url}/sse", "headers": {"Authorization": "Bearer sse-secret-11"}})
    sse_path = str(sse_tool.get("relayPath") or f"{base_path.rstrip('/')}/sse-mcp")

    def _hold_sse() -> None:
        conn = http.client.HTTPConnection("127.0.0.1", relay_port, timeout=5.0)
        try:
            conn.request("GET", sse_path, headers={**auth_hdr, "Accept": "text/event-stream"})
            resp = conn.getresponse()
            while resp.read(64):
                pass
        except OSError:
            pass
        finally:
            conn.close()

    sse_thread = threading.Thread(target=_hold_sse, daemon=True)
    sse_thread.start()
    try:
        assert mock_mcp_upstream.wait_for_stream_start(timeout=5.0) is True, "SSE stream never reached upstream"
        orca_api.assert_rpc_ok("bots.tools.revoke", {"botId": bot_id})
        assert mock_mcp_upstream.wait_for_stream_abort(timeout=5.0) is True, "Revocation did not abort live SSE stream"
    finally:
        sse_thread.join(timeout=5.0)
    after_rev = relay.request("POST", tool_path, headers=auth_hdr, json_body={"jsonrpc": "2.0", "id": 2, "method": "tools/list"})
    assert after_rev.status == 404 and "CREDENTIAL_ENDED" in after_rev.text


@pytest.mark.tier4
@pytest.mark.f09
@pytest.mark.f20
@pytest.mark.f21
@pytest.mark.edge_case("SM2-15")
@pytest.mark.edge_case("SM2-18")
@pytest.mark.edge_case("SM2-26")
def test_t4_12_external_connectors_webhooks_and_wake_ws_under_load(orca_api: OrcaApiClient, orca_cli: OrcaCliRunner) -> None:
    """Workflow #12: 6-adapter webhooks + CLI redaction + Slack ordering barrier + Wake WS + real caps."""
    bot_id = str(_req(_unwrap(_post(orca_api, "/api/v1/orca/bots", {"name": "Hook Ops Bot", "agent": "claude"}), "bot"), "id", "botId", "bot_id"))
    hook_ids: dict[str, str] = {}
    hook_secrets: list[str] = []
    filter_specs = [
        {"path": "action", "op": "=", "value": "opened"}, {"path": "status", "op": "!=", "value": "ignored"},
        {"path": "severity", "op": "in", "value": ["high", "critical"]}, {"path": "title", "op": "contains", "value": "outage"},
        {"path": "service", "op": "exists"}, {"path": "suppressed", "op": "!exists"},
    ]
    assert tuple(f["op"] for f in filter_specs) == WEBHOOK_FILTER_OPS_6
    for adapter, flt in zip(WEBHOOK_ADAPTERS_6, filter_specs):
        hook = _unwrap(_post(orca_api, "/api/v1/orca/webhooks", {"botId": bot_id, "name": f"hook-{adapter}", "adapter": adapter, "rules": {"where": [flt]}}), "webhook")
        hook_ids[adapter] = str(_req(hook, "id", "hookId", "hook_id"))
        for secret_key in ("secret", "signingSecret", "signing_secret", "ingestUrl", "ingest_url", "url"):
            if hook.get(secret_key):
                hook_secrets.append(str(hook[secret_key]))
    assert hook_secrets, "Expected created webhooks to return ingest URL or signing secret for redaction verification"

    first_hook = hook_ids["github"]
    cli_list, cli_show = orca_cli.run("webhooks", "list"), orca_cli.run("webhooks", "show", first_hook)
    cli_list.assert_ok()
    cli_show.assert_ok()
    combined_cli = cli_list.stdout + cli_list.stderr + cli_show.stdout + cli_show.stderr
    for secret in hook_secrets:
        assert secret not in combined_cli, "Webhook CLI leaked ingest URL or signing secret"

    for adapter, hid in hook_ids.items():
        ing = _post(orca_api, f"/api/v1/orca/webhooks/{hid}/ingest", {"action": "opened", "status": "active", "severity": "high", "title": f"{adapter} outage", "service": "payments", "text": "WH_SECRET_TOKEN_999", "url": "https://internal.corp/wh", "actor": "bot_actor", "place": "#ops"})
        assert "WH_SECRET_TOKEN_999" not in json.dumps(ing) and "https://internal.corp/wh" not in json.dumps(ing)
        assert "WH_SECRET_TOKEN_999" not in json.dumps(_get(orca_api, f"/api/v1/orca/webhooks/{hid}/calls"))

    with MockWebhookSink(path_delays={"/slack/mirror-user": 0.25, "/slack/bot-reply": 0.0}) as slack_sink:
        ch = orca_api.assert_rpc_ok("bots.slack.linkMirror", {"botId": bot_id, "slackMirror": True, "mirrorUrl": f"{slack_sink.base_url}/slack/mirror-user", "replyUrl": f"{slack_sink.base_url}/slack/bot-reply"})
        orca_api.assert_rpc_ok("bots.slack.sendOwnerTurn", {"botId": bot_id, "chatId": str(_req(ch, "chatId", "id")), "ownerText": "Deploy?", "replyText": "Ready."})
        reqs = slack_sink.wait_for_requests(2, timeout=5.0)
    user_req = next(r for r in reqs if r.path == "/slack/mirror-user")
    bot_req = next(r for r in reqs if r.path == "/slack/bot-reply")
    assert user_req.timestamp_monotonic < bot_req.timestamp_monotonic

    with orca_api.ws_connect("/api/v1/orca/wake?host_id=t4-12-host") as ws1:
        _post(orca_api, f"/api/v1/orca/webhooks/{first_hook}/ingest", {"action": "opened", "status": "active", "text": "WAKE_SECRET_BODY"})
        frame = ws1.recv_json(timeout=5.0)
        assert str(_req(frame, "kind", "type", "event")) and "WAKE_SECRET_BODY" not in json.dumps(frame)
        with orca_api.ws_connect("/api/v1/orca/wake?host_id=t4-12-host") as ws2:
            assert ws1.recv_until_close(timeout=5.0).close_code == WS_CLOSE_SUPERSEDED
            ws2.send_raw_oversize(256 * 1024)
            assert ws2.recv_until_close(timeout=5.0).close_code == WS_CLOSE_MESSAGE_TOO_BIG

    for i in range(WEBHOOK_HOOK_HOURLY_CAP - 2):
        r = orca_api.post(f"/api/v1/orca/webhooks/{first_hook}/ingest", json_body={"action": "opened", "status": "active", "seq": i}).assert_status(200, 201, 202)
        assert "cap_reached" not in r.text
    for extra in range(6):
        cap_h = orca_api.post(f"/api/v1/orca/webhooks/{first_hook}/ingest", json_body={"action": "opened", "status": "active", "seq": 9001 + extra})
        assert cap_h.status == 429 or (cap_h.status == 202 and ("cap" in cap_h.text.lower() or "refus" in cap_h.text.lower()))
    calls_h = json.dumps(_get(orca_api, f"/api/v1/orca/webhooks/{first_hook}/calls"))
    assert "cap_reached" in calls_h and "refusal_burst" in calls_h

    second_hook = hook_ids["standard"]
    for i in range(WEBHOOK_ACCOUNT_HOURLY_CAP - WEBHOOK_HOOK_HOURLY_CAP - (len(WEBHOOK_ADAPTERS_6) - 1)):
        r = orca_api.post(f"/api/v1/orca/webhooks/{second_hook}/ingest", json_body={"action": "opened", "status": "active", "seq": i}).assert_status(200, 201, 202)
        assert "cap_reached" not in r.text
    cap_a = orca_api.post(f"/api/v1/orca/webhooks/{second_hook}/ingest", json_body={"action": "opened", "status": "active", "seq": 9500})
    assert cap_a.status == 429 or (cap_a.status == 202 and "cap_reached" in cap_a.text)
    assert "cap_reached" in json.dumps(_get(orca_api, f"/api/v1/orca/webhooks/{second_hook}/calls"))

    wait_auto = _unwrap(_post(orca_api, "/api/v1/orca/automations", {"botId": bot_id, "name": "ci-pass-once", "once": True, "trigger": {"source": "github", "kind": "ci_passed"}}), "automation")
    wait_id = str(_req(wait_auto, "id", "automationId", "automation_id"))
    skip = orca_api.assert_rpc_ok("automations.triggerEvent", {"automationId": wait_id, "outcome": "skipped_precheck"})
    assert str(skip.get("status", "")).startswith("skipped_") or skip.get("claimed") is False
    assert wait_id in json.dumps(_get(orca_api, "/api/v1/orca/automations"))
    fired = orca_api.assert_rpc_ok("automations.triggerEvent", {"automationId": wait_id, "outcome": "matched", "source": "github", "kind": "ci_passed"})
    assert fired.get("waitEnding") is True or "WaitEnding" in json.dumps(fired)
    assert wait_id not in json.dumps(_get(orca_api, "/api/v1/orca/automations"))


@pytest.mark.tier4
@pytest.mark.f18
@pytest.mark.f20
@pytest.mark.f21
@pytest.mark.edge_case("SM1-11")
@pytest.mark.edge_case("SM2-24")
def test_t4_13_remote_e2ee_v2_vm_hygiene_and_safe_automation(orca_api: OrcaApiClient, orca_cli: OrcaCliRunner, temp_workspace: TempWorkspace) -> None:
    """Workflow #13: Protocol v3 + 96B E2EE v2 + VM contamination doctor + Safe Computer/Emulator + Artifacts."""
    assert len(_req(orca_api.assert_rpc_ok("runtime.capabilities", {}, protocol_version=RUNTIME_PROTOCOL_VERSION), "capabilities", "methods")) >= 1
    assert len(_req(orca_api.assert_rpc_ok("runtime.capabilities", {}, protocol_version=MIN_COMPATIBLE_PROTOCOL_VERSION), "capabilities", "methods")) >= 1
    _assert_rpc_err_in(orca_api, "runtime.capabilities", ("invalid_argument", "unsupported_protocol_version"), {}, protocol_version=1)

    valid_hs = {"protocol": "orca-mobile-e2ee/v2", "initiator": "mobile", "responder": "desktop", "transport": "relay", "relay_host_id": "relayHost0123456", "client_ephemeral_pubkey": base64.b64encode(bytes(range(10, 42))).decode("ascii"), "client_nonce": base64.b64encode(bytes(range(50, 82))).decode("ascii")}
    hs = _post(orca_api, "/api/v1/orca/ide/e2ee/handshake", valid_hs)
    assert int(_req(hs, "key_schedule_bytes", "keyScheduleBytes")) == E2EE_V2_KEY_SCHEDULE_BYTES
    assert len(base64.b64decode(str(_req(hs, "responder_ephemeral_pubkey", "responderPubkey")))) == 32
    assert len(base64.b64decode(str(_req(hs, "desktop_nonce", "responder_nonce")))) == 32
    assert len(str(_req(hs, "transcript_hash", "transcriptHash"))) >= 32 and str(_req(hs, "session_id", "sessionId"))
    assert str(_req(hs, "mobile_to_desktop_key")) and str(_req(hs, "desktop_to_mobile_key"))

    tampered = orca_api.post("/api/v1/orca/ide/e2ee/handshake", json_body={**valid_hs, "transcript_hash": "0" * 64})
    assert tampered.status in (400, 422), f"Expected 400/422 for tampered transcript, got {tampered.status}: {tampered.text}"
    assert tampered.extract_error_code() in ("invalid_argument", "transcript_mismatch", "e2ee_transcript_mismatch")
    tampered_body = tampered.json()
    assert isinstance(tampered_body, dict) and "mobile_to_desktop_key" not in json.dumps(tampered_body) and "desktop_to_mobile_key" not in json.dumps(tampered_body)

    vm_dir = temp_workspace.mkdir("vm_base_image")
    contaminants = ("orca-devices.json", "orca-e2ee-keypair.json", "agent-session-authority.key")
    for fname in contaminants:
        (vm_dir / fname).write_text('{"contaminated": true}\n', encoding="utf-8")
    dirty_doc = orca_cli.run("ide", "serve", "--recipe-doctor", str(vm_dir))
    assert dirty_doc.returncode != 0
    dirty_text = dirty_doc.stdout + dirty_doc.stderr
    for fname in contaminants:
        assert fname in dirty_text, f"VM recipe doctor missed contaminant: {fname}"
    dirty_rpc = orca_api.rpc("vm.recipeDoctor", {"user_data_dir": str(vm_dir)})
    assert dirty_rpc.status in (200, 400, 409, 422) and dirty_rpc.status not in (404, 500, 501) and all(f in dirty_rpc.text for f in contaminants)
    dj = dirty_rpc.json()
    assert isinstance(dj, dict) and (dj.get("ok") is False or (isinstance(dj.get("result"), dict) and (dj["result"].get("contaminated") is True or bool(dj["result"].get("blocking_issues")))))
    for fname in contaminants:
        (vm_dir / fname).unlink()
    clean_cli = _unwrap(orca_cli.run("ide", "serve", "--recipe-doctor", str(vm_dir)).assert_ok())
    assert isinstance(clean_cli, dict) and clean_cli.get("contaminated") is not True and (clean_cli.get("healthy") is True or clean_cli.get("contaminated") is False or clean_cli.get("clean") is True or clean_cli.get("ok") is True)
    assert not clean_cli.get("blocking_issues") and not clean_cli.get("issues")
    clean_rpc = orca_api.assert_rpc_ok("vm.recipeDoctor", {"user_data_dir": str(vm_dir)})
    assert isinstance(clean_rpc, dict) and clean_rpc.get("contaminated") is not True and (clean_rpc.get("healthy") is True or clean_rpc.get("contaminated") is False) and not clean_rpc.get("blocking_issues")

    for key in ("Cmd+Q", "Alt+F4"):
        orca_api.post("/api/v1/orca/ide/computer/guard", json_body={"key": key}).assert_error_code("system_shortcut_blocked", 400, 403)
        orca_cli.run("ide", "computer-hotkey", "--key", key).assert_error("system_shortcut_blocked")
    _assert_rpc_err_in(orca_api, "computer.click", ("foreground_steal_blocked", "system_shortcut_blocked", "invalid_argument"), {"app": "Fabushi", "x": 50, "y": 50, "foreground": True, "restore_window": False})
    click_ok = orca_api.assert_rpc_ok("computer.click", {"app": "Fabushi", "x": 50, "y": 50, "foreground": True, "restore_window": True})
    assert _req(click_ok, "allowed") is True and _req(click_ok, "restore_window", "restoreWindow") is True
    assert _req(_post(orca_api, "/api/v1/orca/ide/computer/guard", {"key": "Cmd+Q", "allow_system_shortcut": True}), "allowed") is True
    assert _req(_unwrap(orca_cli.run("ide", "computer-hotkey", "--key", "Cmd+Q", "--allow-system-shortcut").assert_ok()), "allowed") is True

    orca_api.assert_rpc_ok("emulator.list", {})
    assert _req(orca_api.assert_rpc_ok("emulator.tap", {"device_id": "emu-android-1", "x": 0.5, "y": 0.25}), "accepted") is True
    _assert_rpc_err_in(orca_api, "emulator.tap", ("invalid_argument",), {"device_id": "emu-android-1", "x": 1.5, "y": -0.1})
    _assert_rpc_err_in(orca_api, "emulator.tap", ("emulator_input_blocked",), {"device_id": "emu-locked-1", "input_locked": True, "x": 0.5, "y": 0.5})
    _assert_rpc_err_in(orca_api, "emulator.start", ("emulator_unsupported",), {"platform": "unsupported_os_sim"})

    art_created = _unwrap(_post(orca_api, "/api/v1/orca/ide/artifacts", {"filename": "report.md", "content": "# Release Readiness\nAll systems go.\n", "content_type": "text/markdown", "explicit_user_intent": True}), "artifact")
    art_id = str(_req(art_created, "id", "artifactId", "artifact_id"))
    art_cli = orca_cli.run("ide", "artifact-share", art_id)
    art_obj = _unwrap(art_cli.assert_ok(), "artifact", "result")
    assert urlparse(str(_req(art_obj, "publicUrl", "public_url", "share_url", "url"))).scheme in ("http", "https")
    art_out = art_cli.stdout + art_cli.stderr
    assert "editToken" not in art_out and "edit_token" not in art_out
    pub_body = _get(orca_api, f"/api/v1/orca/ide/artifacts/{art_id}")
    assert "editToken" not in json.dumps(pub_body) and "edit_token" not in json.dumps(pub_body)
    upd = orca_api.assert_ok(orca_api.patch(f"/api/v1/orca/ide/artifacts/{art_id}", json_body={"content": "# Updated\n"}))
    assert isinstance(upd, dict) and (upd.get("updated") is True or upd.get("ok") is True or "Updated" in json.dumps(upd))
    orca_api.assert_ok(orca_api.delete(f"/api/v1/orca/ide/artifacts/{art_id}"))
    orca_api.get(f"/api/v1/orca/ide/artifacts/{art_id}").assert_status(404)


@pytest.mark.tier4
@pytest.mark.f21
@pytest.mark.f22
@pytest.mark.f23
@pytest.mark.f24
def test_t4_14_fabushi_v170_release_ledger_and_desktop_parity_readiness(orca_api: OrcaApiClient, orca_cli: OrcaCliRunner, temp_workspace: TempWorkspace) -> None:
    """Workflow #14: Fail-closed Fabushi v1.7.0 pins, CI/Deploy/Release, hygiene, 6-gate ledger & Desktop UI."""
    cargo_doc = tomllib.loads((REPO_ROOT / "Cargo.toml").read_text(encoding="utf-8"))
    assert cargo_doc["workspace"]["package"]["version"] == TARGET_RELEASE_VERSION
    m_sw = re.search(r'CFBundleShortVersionString"\)\s+as\?\s+String\s+\?\?\s+"([^"]+)"', (REPO_ROOT / "scripts/macos/FabushiInstaller.swift").read_text(encoding="utf-8"))
    assert m_sw and m_sw.group(1) == TARGET_RELEASE_VERSION
    m_iss = re.search(r'#define\s+AppVersion\s+"([^"]+)"', (REPO_ROOT / "scripts/windows/FabushiSetup.iss").read_text(encoding="utf-8"))
    assert m_iss and m_iss.group(1) == TARGET_RELEASE_VERSION

    for prof in ("dev", "test", "ci", "release"):
        p = cargo_doc["profile"][prof]
        assert p["debug"] == 0 and p["strip"] == "debuginfo" and p["incremental"] is False
    for crate in ("orca_core", "orca_bots", "orca_ide"):
        assert f"crates/{crate}" in cargo_doc["workspace"]["members"] and crate in cargo_doc["workspace"]["dependencies"] and (REPO_ROOT / "crates" / crate / "src" / "lib.rs").is_file()

    ci_yml = (REPO_ROOT / ".github/workflows/ci.yml").read_text(encoding="utf-8")
    assert "verify_ledger_closure.py --ci-without-upstream --skip-cargo" in ci_yml and "clean_build_garbage.py" in ci_yml
    linux_m = re.search(r"(?ms)^  linux-fast-verify:.*?(?=^  \S|\Z)", ci_yml)
    macos_m = re.search(r"(?ms)^  macos-gpui-verify:.*?(?=^  \S|\Z)", ci_yml)
    assert linux_m and macos_m, "Missing linux-fast-verify or macos-gpui-verify job in ci.yml"
    for pkg in ("-p orca_core", "-p orca_bots", "-p orca_ide"):
        assert pkg in linux_m.group(0) and pkg in macos_m.group(0), f"Missing {pkg} in linux/macos CI job"

    deploy_yml = (REPO_ROOT / ".github/workflows/deploy-backend.yml").read_text(encoding="utf-8")
    for glob_pat in ("crates/orca_core/**", "crates/orca_bots/**", "crates/orca_ide/**"):
        assert glob_pat in deploy_yml
    assert "gpui_telegram_server" in deploy_yml and "/health" in deploy_yml and "/api/v1/status" in deploy_yml

    rel_yml = (REPO_ROOT / ".github/workflows/release.yml").read_text(encoding="utf-8")
    assert '"v*"' in rel_yml or "'v*'" in rel_yml
    for job in ("release-macos-arm64", "release-linux-x86_64", "release-windows-x86_64"):
        assert job in rel_yml
    for bin_name in ("gpui_telegram_desktop", "gpui_telegram_server", "fabushi", "fabushi-remote-daemon"):
        assert rel_yml.count(bin_name) >= 3, f"Expected --bin {bin_name} across all 3 OS jobs"
    assert "Fabushi ${TAG_NAME}" in rel_yml or "Fabushi ${{ " in rel_yml

    py_env = {**os.environ, "PYTHONDONTWRITEBYTECODE": "1"}
    sb = temp_workspace.mkdir("clean_sandbox")
    garbage = [sb / p for p in ("target/debug/incremental/x", "target/debug/deps/app.dSYM/p", "target/debug/deps/foo-ab12.rcgu.o", "target/debug/deps/stale_test_bin", "target/tmp/t1", ".cargo-target-alt/x", "crates/sub/target/x")]
    keep = [sb / p for p in ("target/debug/deps/liborca_core-1111.rlib", "target/debug/deps/liborca_core-1111.rmeta", "target/debug/deps/liborca_core-1111.d", "target/debug/.fingerprint/orca_core-1111/ts")]
    for path in garbage + keep:
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_bytes(b"payload")
    os.chmod(sb / "target/debug/deps/stale_test_bin", stat.S_IRWXU)
    clean_cmd = ["/usr/bin/python3", str(REPO_ROOT / "scripts/clean_build_garbage.py"), "--repo-root", str(sb), "--target-dir", str(sb / "target"), "--target-only", "--force"]
    assert subprocess.run(clean_cmd + ["--dry-run"], cwd=str(REPO_ROOT), env=py_env, capture_output=True, text=True, timeout=30).returncode == 0 and all(p.exists() for p in garbage)
    assert subprocess.run(clean_cmd, cwd=str(REPO_ROOT), env=py_env, capture_output=True, text=True, timeout=30).returncode == 0 and not any(p.exists() for p in garbage) and all(p.exists() for p in keep)

    idx = json.loads((REPO_ROOT / "docs/specs/ledger/index.json").read_text(encoding="utf-8"))
    assert idx["total_source_files"] == 3441 and idx["total_mapped_files"] == 3441 and idx["omitted_count"] == 0 and idx["unmapped_count"] == 0 and len(idx["shards"]) == 28
    shard_files = sorted((REPO_ROOT / "docs/specs/ledger/shards").glob("*.json"))
    assert len(shard_files) == 28
    seen_paths: set[str] = set()
    for sf in shard_files:
        for entry in json.loads(sf.read_text(encoding="utf-8")).get("entries", []):
            sp = str(entry["source_path"])
            assert sp not in seen_paths, f"Duplicate ledger source_path: {sp}"
            seen_paths.add(sp)
    assert len(seen_paths) == 3441

    led = subprocess.run(["/usr/bin/python3", str(REPO_ROOT / "scripts/verify_ledger_closure.py"), "--ci-without-upstream", "--skip-cargo"], cwd=str(REPO_ROOT), env=py_env, capture_output=True, text=True, timeout=900)
    _assert_ledger_closure(led)

    col4 = subprocess.run(["/usr/bin/python3", str(SUITE_ROOT / "run_e2e.py"), "--tier", "4", "--collect-only", "-q"], cwd=str(REPO_ROOT), env=py_env, capture_output=True, text=True, timeout=60)
    assert col4.returncode == 0
    m4 = re.search(r"(?m)^(\d+)(?:/\d+)?\s+(?:tests?\s+collected|selected)\b", col4.stdout)
    all_t4 = [ln.strip() for ln in col4.stdout.splitlines() if "::test_" in ln]
    t4_nodes = [ln for ln in all_t4 if "::test_t4_" in ln]
    assert m4 and int(m4.group(1)) == 14 and len(all_t4) == 14 and len(t4_nodes) == 14, col4.stdout
    assert not [p for p in SUITE_ROOT.rglob("*") if p.name in ("__pycache__", ".pytest_cache")]

    assert TARGET_RELEASE_VERSION in str(orca_cli.run("--version", json_mode=False, remote=False).assert_ok())
    tpl_sets = [{str(_req(t, "id", "templateId", "template_id")) for t in (b if isinstance(b, list) else _req(b, "templates", "items"))} for b in (_unwrap(orca_cli.run("bot", "templates").assert_ok()), _get(orca_api, "/api/v1/orca/bots/templates"))]
    assert tpl_sets[0] == tpl_sets[1] == set(TEMPLATE_IDS_24)
    usg_sets = [{str(_req(p, "provider", "id", "name")) for p in (b if isinstance(b, list) else _req(b, "providers", "usage", "items"))} for b in (_unwrap(orca_cli.run("ide", "usage").assert_ok()), _get(orca_api, "/api/v1/orca/ide/usage"))]
    assert usg_sets[0] == usg_sets[1] and set(USAGE_PROVIDERS_9).issubset(usg_sets[1])

    nav_src = (REPO_ROOT / "crates/gpui_telegram_desktop/src/views/nav_rail.rs").read_text(encoding="utf-8")
    assert re.search(r"pub\s+enum\s+NavDestination\s*\{[^}]*\bOrcaBots\b", nav_src, re.S)
    shell_src = (REPO_ROOT / "crates/gpui_telegram_desktop/src/views/shell.rs").read_text(encoding="utf-8")
    panel_m = re.search(r"pub\s+enum\s+ActiveWorkspacePanel\s*\{([^}]+)\}", shell_src, re.S)
    assert panel_m and "OrcaBots" in panel_m.group(1) and "OrcaIde" in panel_m.group(1)
    assert "pub mod orca;" in (REPO_ROOT / "crates/gpui_telegram_desktop/src/views/mod.rs").read_text(encoding="utf-8")
    orca_rs = list((REPO_ROOT / "crates/gpui_telegram_desktop/src/views/orca").rglob("*.rs"))
    assert orca_rs and any("SecuritySurface" in p.read_text(encoding="utf-8") for p in orca_rs)
    assert "pub mod design_mode;" in (REPO_ROOT / "crates/bot-browser/src/lib.rs").read_text(encoding="utf-8")
    assert "pub struct DesignModeCapture" in (REPO_ROOT / "crates/bot-browser/src/design_mode.rs").read_text(encoding="utf-8")
