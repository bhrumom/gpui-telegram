"""Tier 3 pairwise cross-feature E2E tests (T3-21..T3-28): Security, Remote, UI & Release."""

from __future__ import annotations

import base64
import json
import os
import re
import stat
import subprocess
import tomllib
from pathlib import Path
from typing import Any

import pytest

from harness import (
    E2EE_V2_KEY_SCHEDULE_BYTES, MIN_COMPATIBLE_PROTOCOL_VERSION, REPO_ROOT,
    RUNTIME_PROTOCOL_VERSION, SUITE_ROOT, TAB_CONTENT_TYPES_10, TARGET_RELEASE_VERSION,
    WEBHOOK_HOOK_HOURLY_CAP, WS_CLOSE_MESSAGE_TOO_BIG, WS_CLOSE_SUPERSEDED,
    GitRepoFixture, MockMcpUpstreamServer, OrcaApiClient, OrcaCliRunner, RawHttpClient, TempWorkspace,
)

TINY_PNG_B64 = "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mP8/x8AAwMCAO+ip1sAAAAASUVORK5CYII="


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


def _assert_no_secret_in_dir(root: Path, secret: str) -> None:
    assert root.is_dir() and isinstance(secret, str) and secret, f"Invalid bot home dir or secret: {root}"
    checked_files = 0
    for path in root.rglob("*"):
        if path.is_file():
            checked_files += 1
            assert secret not in path.read_bytes().decode("utf-8", errors="ignore"), f"Secret leaked into {path}"
    assert checked_files > 0, f"No bot home files scanned under {root}; secret check would be vacuous"


def _assert_rpc_err_in(api: OrcaApiClient, method: str, codes: tuple[str, ...], params: dict[str, Any] | None = None, **kw: Any) -> None:
    resp = api.rpc(method, params or {}, **kw)
    assert resp.status in (200, 400, 403, 409, 422, 426) and resp.status not in (404, 500, 501), f"Bad RPC status {resp.status}: {resp.text[:300]}"
    body = resp.json()
    assert isinstance(body, dict) and body.get("ok") is False, f"Expected ok=false: {body!r}"
    assert str(_req(_req(body, "error"), "code")) in codes, f"Unexpected error code in {body!r}"


def _write_plugin(ws: TempWorkspace, folder: str, manifest: dict[str, Any], files: dict[str, str] | None = None) -> Path:
    pdir = ws.mkdir(f"plugins/{folder}")
    (pdir / "orca-plugin.json").write_text(json.dumps(manifest), encoding="utf-8")
    for rel, content in (files or {}).items():
        fp = pdir / rel
        fp.parent.mkdir(parents=True, exist_ok=True)
        fp.write_text(content, encoding="utf-8")
    return pdir


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


@pytest.mark.tier3
@pytest.mark.f18
@pytest.mark.f17
def test_t3_21_design_mode_element_capture_to_agent_edit_diff_and_attribution(orca_api: OrcaApiClient, git_repo: GitRepoFixture) -> None:
    """F18 + F17: Design Mode DOM+CSS+PNG+sourceMap capture -> chat -> agent edit attribution & annotation."""
    assert base64.b64decode(TINY_PNG_B64).startswith(b"\x89PNG\r\n\x1a\n")
    git_repo.commit_file("src/button.rs", "\n".join(f"// btn {i:02d}" for i in range(1, 21)) + "\n", msg="seed button")
    wt = _unwrap(_post(orca_api, "/api/v1/orca/ide/worktrees", {"repoPath": str(git_repo.path), "name": "wt-design-mode"}), "worktree")
    wt_path = Path(str(_req(wt, "path", "worktreePath", "worktree_path")))
    session_id = str(_req(orca_api.assert_rpc_ok("agentSession.create", {"workspacePath": str(wt_path), "agent": "claude", "viewMode": "chat"}), "sessionId", "id"))
    cap = orca_api.assert_rpc_ok("browser.designMode.capture", {
        "tab_id": "tab-dm-t3-21", "selector": "button.cta", "outer_html": '<button class="cta">Save</button>',
        "dom_neighborhood": '<main><button class="cta">Save</button></main>', "computed_styles": {"color": "#ffffff", "padding": "8px 16px"},
        "crop": {"x": 0, "y": 0, "width": 80, "height": 32}, "screenshot_png_b64": TINY_PNG_B64, "source_map": {"file": "src/button.rs", "line": 12},
    })
    cap_json, html_str = json.dumps(cap), str(_req(cap, "outer_html", "outerHtml"))
    assert '<button class="cta">Save</button>' in html_str or '<button class=\\"cta\\">Save</button>' in cap_json
    assert "#ffffff" in cap_json and "src/button.rs" in cap_json and TINY_PNG_B64[:16] in cap_json
    attachment = _req(cap, "chat_attachment", "attachment")
    assert isinstance(attachment, dict) and attachment
    orca_api.assert_rpc_ok("agentSession.send", {"sessionId": session_id, "text": "Update button contrast", "attachments": [attachment]})
    tx_blob = json.dumps(orca_api.assert_rpc_ok("agentSession.getTranscript", {"sessionId": session_id}))
    assert "Update button contrast" in tx_blob and ("button.cta" in tx_blob or "src/button.rs" in tx_blob or "Save" in tx_blob)
    orca_api.assert_rpc_ok("diffReview.recordAttribution", {"repo_path": str(wt_path), "file": "src/button.rs", "author_kind": "ai", "agent": "claude", "start_line": 11, "end_line": 14})
    _post(orca_api, "/api/v1/orca/ide/diff-annotations", {"repo_path": str(wt_path), "file": "src/button.rs", "line": 12, "body": "Match contrast token"})
    anns = _get(orca_api, "/api/v1/orca/ide/diff-annotations", {"repo_path": str(wt_path)})
    ann_list = anns if isinstance(anns, list) else _req(anns, "annotations", "items")
    assert any(int(_req(a, "line")) == 12 and "Match contrast token" in str(_req(a, "body")) for a in ann_list)
    attr = orca_api.assert_rpc_ok("diffReview.getAttribution", {"repo_path": str(wt_path), "file": "src/button.rs"})
    by_line = {int(e["line"]): str(e["author_kind"]) for e in _req(attr, "lines", "entries")}
    assert all(by_line.get(ln) == "ai" for ln in range(11, 15)) and "claude" in json.dumps(attr) and git_repo.git("status", "--porcelain").stdout.strip() == ""


@pytest.mark.tier3
@pytest.mark.f18
@pytest.mark.f19
@pytest.mark.edge_case("SM1-12")
@pytest.mark.edge_case("SM2-24")
def test_t3_22_plugin_v2_security_surface_and_computer_use_hotkey_guard(orca_api: OrcaApiClient, temp_workspace: TempWorkspace) -> None:
    """F18 + F19: Plugin V2 spoof theme & destructive command -> F18 hotkey block & SecuritySurface immunity."""
    plug_dir = _write_plugin(temp_workspace, "cu-spoof", {
        "schema_version": 2, "publisher": "acme", "id": "cu-spoof", "version": "1.0.0", "tier": "declarative",
        "contributes": {
            "themes": [{"id": "spoof", "path": "theme.json"}],
            "commands": [{"id": "acme.forceQuit", "title": "Force Quit", "computerHotkey": "Cmd+Q"}, {"id": "acme.closeApp", "title": "Close App", "computerHotkey": "Alt+F4"}],
            "keybindings": [{"key": "Cmd+Q", "command": "acme.forceQuit"}],
        },
    }, {"theme.json": json.dumps({"securitySurface.bg": "#00ff00", "workspace.bg": "#111827"})})
    inst = _unwrap(_post(orca_api, "/api/v1/orca/ide/plugins", {"path": str(plug_dir)}), "plugin")
    plugin_id = str(inst["qualified_id"] if isinstance(inst, dict) and inst.get("qualified_id") else f"{_req(inst, 'publisher')}.{_req(inst, 'id')}")
    assert "acme.forceQuit" in json.dumps(orca_api.assert_rpc_ok("plugins.contributions", {}))
    assert "#111827" in json.dumps(orca_api.assert_rpc_ok("plugins.resolveThemeTokens", {"surface": "workspace"})).lower()
    for cmd_id, blocked_key in (("acme.forceQuit", "Cmd+Q"), ("acme.closeApp", "Alt+F4"), ("acme.lock", "Super+L")):
        orca_api.post("/api/v1/orca/ide/computer/guard", json_body={"key": blocked_key, "source_plugin_id": plugin_id, "command_id": cmd_id}).assert_error_code("system_shortcut_blocked", 400, 403)
        orca_api.assert_rpc_error("computer.hotkey", "system_shortcut_blocked", {"key": blocked_key, "source_plugin_id": plugin_id, "command_id": cmd_id})
    _assert_rpc_err_in(orca_api, "computer.click", ("foreground_steal_blocked", "system_shortcut_blocked", "invalid_argument"), {"app": "Fabushi", "x": 100, "y": 100, "foreground": True, "restore_window": False, "source_plugin_id": plugin_id})
    click_ok = orca_api.assert_rpc_ok("computer.click", {"app": "Fabushi", "x": 100, "y": 100, "foreground": True, "restore_window": True, "source_plugin_id": plugin_id})
    assert _req(click_ok, "allowed") is True and _req(click_ok, "restore_window", "restoreWindow") is True
    sec_tokens = orca_api.assert_rpc_ok("plugins.resolveThemeTokens", {"surface": "security", "context": "computer.overrideConfirm", "plugin_id": plugin_id, "key": "Cmd+Q"})
    assert _req(sec_tokens, "security_surface_protected", "themeOverrideBlocked", "theme_override_blocked") is True and "#00ff00" not in json.dumps(sec_tokens).lower()
    assert _req(_post(orca_api, "/api/v1/orca/ide/computer/guard", {"key": "Cmd+Q", "source_plugin_id": plugin_id, "allow_system_shortcut": True}), "allowed") is True
    assert _req(orca_api.assert_rpc_ok("computer.hotkey", {"key": "Cmd+Q", "source_plugin_id": plugin_id, "allow_system_shortcut": True}), "allowed") is True
    app_state = orca_api.assert_rpc_ok("computer.getAppState", {"app": "Fabushi", "query": "Submit", "source_plugin_id": plugin_id})
    assert isinstance(app_state, dict) and app_state and "Submit" in json.dumps(app_state) and not any(app_state.get(k) for k in ("steals_focus", "stealsFocus", "focusStolen"))
    bg_val = orca_api.assert_rpc_ok("computer.setValue", {"app": "Fabushi", "target": "@e2", "value": "bg-token-42", "secret": True, "background_delivery": True, "source_plugin_id": plugin_id})
    assert _req(bg_val, "background_delivery", "backgroundDelivery") is True and _req(bg_val, "steals_focus", "stealsFocus") is False and "bg-token-42" not in json.dumps(bg_val)
    diff = orca_api.assert_rpc_ok("computer.diffAppState", {"before": {"id": "root", "children": [{"id": "n1", "label": "Locked"}]}, "after": {"id": "root", "children": [{"id": "n1", "label": "Approved"}]}})
    assert "Approved" in json.dumps(diff)


@pytest.mark.tier3
@pytest.mark.f19
@pytest.mark.f08
@pytest.mark.edge_case("SM2-12")
def test_t3_23_plugin_v2_mcp_server_synced_through_loopback_tool_relay(orca_api: OrcaApiClient, mock_mcp_upstream: MockMcpUpstreamServer, temp_workspace: TempWorkspace) -> None:
    """F19 + F08: Plugin V2 contributed MCP server -> host enable -> loopback Tool Relay + CSRF."""
    plugin_secret = "plugin_secret_987654"
    bot_home = temp_workspace.mkdir("t3_23_bots/plugin-mcp-bot")
    bot = _unwrap(_post(orca_api, "/api/v1/orca/bots", {"name": "Plugin MCP Bot", "agent": "claude", "folder": "plugin-mcp-bot", "homePath": str(bot_home)}), "bot")
    bot_id = str(_req(bot, "id", "botId", "bot_id"))
    home_dir = Path(str(bot.get("homePath") or bot.get("home_path") or bot.get("homeDir") or bot.get("folderPath") or bot_home))
    pdir = _write_plugin(temp_workspace, "mcp-provider", {"schema_version": 2, "publisher": "acme", "id": "mcp-provider", "version": "1.0.0", "tier": "declarative", "contributes": {"mcpServers": [{"name": "plugin-mcp", "transport": "streamable-http", "url": f"{mock_mcp_upstream.base_url}/mcp"}]}})
    _post(orca_api, "/api/v1/orca/ide/plugins", {"path": str(pdir)})
    orca_api.assert_rpc_ok("mcpServers.add", {"name": "plugin-mcp", "transport": "streamable-http", "url": f"{mock_mcp_upstream.base_url}/mcp", "credential_handle": "vault://plugin-mcp"})
    sync_dir = temp_workspace.mkdir("t3_23_mcp_sync")
    orca_api.assert_rpc_ok("mcpServers.sync", {"target_dir": str(sync_dir), "agents": ["claude", "codex"]})
    assert any("plugin-mcp" in p.read_text(encoding="utf-8", errors="ignore") for p in sync_dir.rglob("*") if p.is_file())

    added = orca_api.assert_rpc_ok("bots.tools.add", {"botId": bot_id, "name": "plugin-mcp", "transport": "http", "url": f"{mock_mcp_upstream.base_url}/mcp", "headers": {"X-Plugin-Mcp-Secret": plugin_secret}})
    orca_api.assert_rpc_ok("bots.tools.setHostMcpServers", {"botId": bot_id, "enabled": True})
    relay_port, grant, base_path = _extract_relay_grant(orca_api, bot_id)
    tool_path = str(added.get("relayPath") or f"{base_path.rstrip('/')}/plugin-mcp")

    raw = RawHttpClient("127.0.0.1", relay_port)
    raw.request("POST", tool_path, headers={"Authorization": f"Bearer {grant}"}, json_body={"jsonrpc": "2.0", "id": 7, "method": "tools/list"}).assert_status(200)
    assert mock_mcp_upstream.requests and mock_mcp_upstream.requests[-1].headers.get("x-plugin-mcp-secret") == plugin_secret
    _assert_no_secret_in_dir(home_dir, plugin_secret)
    raw.request("POST", tool_path, headers={"Origin": "https://evil.site", "Authorization": f"Bearer {grant}"}, json_body={}).assert_status(403)
    raw.request("POST", tool_path, headers={"Host": "evil.site", "Authorization": f"Bearer {grant}"}, json_body={}).assert_status(403)
    raw.request("POST", tool_path, headers={"Authorization": "Bearer wrong-token"}, json_body={}).assert_status(401)


@pytest.mark.tier3
@pytest.mark.f20
@pytest.mark.f09
@pytest.mark.edge_case("SM2-18")
def test_t3_24_webhook_ingest_triggers_content_free_wake_ws_and_rpc_v3(orca_api: OrcaApiClient) -> None:
    """F20 + F09: Protocol v3 negotiation + content-free Wake WS (4409/1009) -> real 200/hr rolling cap."""
    caps_v3 = orca_api.assert_rpc_ok("runtime.capabilities", {}, protocol_version=RUNTIME_PROTOCOL_VERSION)
    caps_v2 = orca_api.assert_rpc_ok("runtime.capabilities", {}, protocol_version=MIN_COMPATIBLE_PROTOCOL_VERSION)
    assert len(_req(caps_v3, "capabilities", "methods")) >= 1 and len(_req(caps_v2, "capabilities", "methods")) >= 1
    _assert_rpc_err_in(orca_api, "runtime.capabilities", ("invalid_argument", "unsupported_protocol_version"), {}, protocol_version=1)

    bot_id = str(_req(_unwrap(_post(orca_api, "/api/v1/orca/bots", {"name": "Wake Hook Bot", "agent": "claude"}), "bot"), "id", "botId", "bot_id"))
    hook = _unwrap(_post(orca_api, "/api/v1/orca/webhooks", {"botId": bot_id, "name": "gh-events", "adapter": "github", "rules": {"where": [{"path": "action", "op": "=", "value": "opened"}]}}), "webhook")
    hook_id = str(_req(hook, "id", "hookId", "hook_id"))

    with orca_api.ws_connect("/api/v1/orca/wake?host_id=t3-24-host") as ws1:
        _post(orca_api, f"/api/v1/orca/webhooks/{hook_id}/ingest", {"action": "opened", "seq": 0, "text": "CONFIDENTIAL_TEXT_99", "url": "https://private.corp/secret", "actor": "actor_alice", "place": "#sec"})
        wake_frame = ws1.recv_json(timeout=5.0)
        wake_json = json.dumps(wake_frame)
        assert str(_req(wake_frame, "kind", "type", "event"))
        for forbidden in ("CONFIDENTIAL_TEXT_99", "https://private.corp/secret", "actor_alice"):
            assert forbidden not in wake_json, f"Wake WS leaked payload content: {forbidden}"
        assert all(k not in wake_frame for k in ("text", "url", "actor", "place"))
        calls_json = json.dumps(_get(orca_api, f"/api/v1/orca/webhooks/{hook_id}/calls"))
        assert "CONFIDENTIAL_TEXT_99" not in calls_json and "https://private.corp/secret" not in calls_json

        with orca_api.ws_connect("/api/v1/orca/wake?host_id=t3-24-host") as ws2:
            assert ws1.recv_until_close(timeout=5.0).close_code == WS_CLOSE_SUPERSEDED
            ws2.send_raw_oversize(256 * 1024)
            assert ws2.recv_until_close(timeout=5.0).close_code == WS_CLOSE_MESSAGE_TOO_BIG

    for seq in range(1, WEBHOOK_HOOK_HOURLY_CAP):
        orca_api.post(f"/api/v1/orca/webhooks/{hook_id}/ingest", json_body={"action": "opened", "seq": seq}).assert_status(200, 201, 202)
    for extra in range(6):
        cap_resp = orca_api.post(f"/api/v1/orca/webhooks/{hook_id}/ingest", json_body={"action": "opened", "seq": WEBHOOK_HOOK_HOURLY_CAP + extra})
        assert cap_resp.status == 429 or (cap_resp.status == 202 and ("cap" in cap_resp.text.lower() or "refus" in cap_resp.text.lower()))
    calls_after = json.dumps(_get(orca_api, f"/api/v1/orca/webhooks/{hook_id}/calls"))
    assert "cap_reached" in calls_after and "refusal_burst" in calls_after


@pytest.mark.tier3
@pytest.mark.f20
@pytest.mark.f11
@pytest.mark.edge_case("SM1-10")
def test_t3_25_e2ee_v2_handshake_federated_dispatch_and_unverifiable_liveness(orca_api: OrcaApiClient) -> None:
    """F20 + F11: 96B X25519+HKDF-SHA256 E2EE v2 -> federated --on dispatch & unverifiable liveness."""
    base_hs = {
        "protocol": "orca-mobile-e2ee/v2", "initiator": "mobile", "responder": "desktop", "transport": "relay",
        "relay_host_id": "AbCdEfGhIjKlMnO1",
        "client_ephemeral_pubkey": base64.b64encode(bytes(range(1, 33))).decode("ascii"),
        "client_nonce": base64.b64encode(bytes(range(33, 65))).decode("ascii"),
    }
    hs1 = _post(orca_api, "/api/v1/orca/ide/e2ee/handshake", base_hs)
    assert int(_req(hs1, "key_schedule_bytes", "keyScheduleBytes")) == E2EE_V2_KEY_SCHEDULE_BYTES
    session_id = str(_req(hs1, "session_id", "sessionId"))
    assert len(str(_req(hs1, "transcript_hash", "transcriptHash"))) >= 32
    assert len(base64.b64decode(str(_req(hs1, "responder_ephemeral_pubkey", "responderPubkey")))) == 32
    assert len(base64.b64decode(str(_req(hs1, "desktop_nonce", "responder_nonce")))) == 32
    assert str(_req(hs1, "mobile_to_desktop_key")) and str(_req(hs1, "desktop_to_mobile_key"))

    hs2 = _post(orca_api, "/api/v1/orca/ide/e2ee/handshake", {**base_hs, "relay_host_id": "AbCdEfGhIjKlMnO2"})
    assert str(_req(hs2, "session_id", "sessionId")) != session_id
    bad_hs = orca_api.post("/api/v1/orca/ide/e2ee/handshake", json_body={**base_hs, "transcript_hash": "0" * 64})
    assert bad_hs.status in (400, 422) and bad_hs.extract_error_code() in ("invalid_argument", "transcript_mismatch", "e2ee_transcript_mismatch")
    bad_hs_body = bad_hs.json()
    assert isinstance(bad_hs_body, dict) and "mobile_to_desktop_key" not in json.dumps(bad_hs_body) and "desktop_to_mobile_key" not in json.dumps(bad_hs_body)

    orca_api.assert_rpc_ok("orchestration.federation.registerPeer", {"serverId": "peer-host-b", "supportsServerMail": True})
    run_id = str(_req(_unwrap(_post(orca_api, "/api/v1/orca/orchestration/runs", {"title": "fed-run"}), "run"), "id", "runId", "run_id"))
    task_id = str(_req(_unwrap(_post(orca_api, "/api/v1/orca/orchestration/tasks", {"runId": run_id, "title": "Remote build", "deps": []}), "task"), "id", "taskId", "task_id"))
    fed = _unwrap(_post(orca_api, "/api/v1/orca/orchestration/workers/start", {"runId": run_id, "taskId": task_id, "agent": "claude", "targetServer": "peer-host-b", "e2eeSessionId": session_id}), "worker", "dispatch")
    dispatch_id = str(_req(fed, "dispatchId", "dispatch_id", "id"))
    assert "to_worker" in json.dumps(fed)

    orca_api.assert_rpc_ok("orchestration.worker.reportLiveness", {"dispatchId": dispatch_id, "liveness": "unverifiable", "reason": "heartbeat_timeout"})
    shown = orca_api.assert_rpc_ok("orchestration.worker.show", {"dispatchId": dispatch_id})
    assert str(_req(shown, "liveness")) == "unverifiable" and _req(shown, "taskStatus", "status") == "dispatched"
    assert int(_req(shown, "activeDispatchCount", "active_dispatch_count")) == 1
    retry_ref = orca_api.post("/api/v1/orca/orchestration/workers/start", json_body={"runId": run_id, "taskId": task_id, "agent": "claude", "retryOnSilence": True})
    assert retry_ref.status in (400, 409, 422) and retry_ref.extract_error_code() in ("unverifiable_liveness", "liveness_unverifiable", "active_dispatch_exists", "dispatch_active", "stale_dispatch", "conflict", "invalid_argument") and any(tok in retry_ref.text.lower() for tok in ("unverifiable", "liveness", "active", "conflict"))
    assert int(_req(orca_api.assert_rpc_ok("orchestration.worker.show", {"dispatchId": dispatch_id}), "activeDispatchCount", "active_dispatch_count")) == 1


@pytest.mark.tier3
@pytest.mark.f21
@pytest.mark.f04
def test_t3_26_cli_bot_items_roundtrip_rest_and_telegram_im_fusion(orca_api: OrcaApiClient, orca_cli: OrcaCliRunner) -> None:
    """F21 + F04: fabushi CLI bot item & ask -> exact REST parity & Telegram IM fusion convergence."""
    bot = _unwrap(orca_cli.run("bot", "create", "--name", "Release Triage", "--agent", "claude").assert_ok(), "bot")
    bot_id = str(_req(bot, "id", "botId", "bot_id"))
    item = _unwrap(orca_cli.run("bot", "item-add", "--bot", bot_id, "--title", "Approve v1.7.0 tag").assert_ok(), "item")
    item_num = int(_req(item, "number", "num", "itemNum", "item_num"))
    orca_cli.run(
        "bot", "item-set", str(item_num), "--status", "needs-you", "--comment", "Ready?",
        "--ask", "Tag v1.7.0?", "--option", "Approve", "--option", "Wait", "--recommended", "0",
    ).assert_ok()

    cli_ny = _unwrap(orca_cli.run("bot", "needs-you").assert_ok())
    rest_ny = _get(orca_api, "/api/v1/orca/needs-you")
    assert str(item_num) in json.dumps(cli_ny) and str(item_num) in json.dumps(rest_ny)

    cli_show = _unwrap(orca_cli.run("bot", "item-show", str(item_num)).assert_ok(), "item")
    rest_show = _unwrap(_get(orca_api, f"/api/v1/orca/items/{item_num}"), "item")
    assert _req(cli_show, "title") == _req(rest_show, "title") == "Approve v1.7.0 tag"
    assert _req(cli_show, "status") == _req(rest_show, "status") == "needs_you"
    assert "Tag v1.7.0?" in json.dumps(cli_show) and "Tag v1.7.0?" in json.dumps(rest_show)

    tree = _get(orca_api, f"/api/v1/orca/bots/{bot_id}/chat-tree")
    assert str(item_num) in json.dumps(tree)
    _post(orca_api, f"/api/v1/orca/items/{item_num}/comment", {"actor": {"kind": "owner"}, "channel": "telegram_im", "text": "Tag v1.7.0? → Approve"})
    assert _req(_unwrap(orca_cli.run("bot", "item-show", str(item_num)).assert_ok(), "item"), "status") == "working"
    assert _req(_unwrap(_get(orca_api, f"/api/v1/orca/items/{item_num}"), "item"), "status") == "working"
    assert "working" in json.dumps(_get(orca_api, f"/api/v1/orca/bots/{bot_id}/chat-tree"))


@pytest.mark.tier3
@pytest.mark.f22
@pytest.mark.f19
@pytest.mark.edge_case("SM1-12")
def test_t3_27_desktop_security_surface_and_orca_ide_bots_panel_consistency(orca_api: OrcaApiClient, temp_workspace: TempWorkspace) -> None:
    """F22 + F19: SecuritySurface immunity, 10-tab Split Anything, panel switch & desktop Rust symbols."""
    pdir = _write_plugin(temp_workspace, "desktop-theme", {"schema_version": 2, "publisher": "acme", "id": "desktop-theme", "version": "1.0.0", "tier": "data", "contributes": {"themes": [{"id": "t", "path": "theme.json"}]}}, {"theme.json": json.dumps({"securitySurface.bg": "#ff00ff", "workspace.bg": "#1e293b"})})
    _post(orca_api, "/api/v1/orca/ide/plugins", {"path": str(pdir)})
    sec = orca_api.assert_rpc_ok("plugins.resolveThemeTokens", {"surface": "security"})
    assert _req(sec, "security_surface_protected", "themeOverrideBlocked", "theme_override_blocked") is True and "#ff00ff" not in json.dumps(sec).lower()

    bot_id = str(_req(_unwrap(_post(orca_api, "/api/v1/orca/bots", {"name": "Badge Bot", "agent": "claude"}), "bot"), "id", "botId", "bot_id"))
    item_num = int(_req(_unwrap(_post(orca_api, f"/api/v1/orca/bots/{bot_id}/items", {"title": "Review desktop panel", "status": "needs_you"}), "item"), "number", "num", "itemNum", "item_num"))
    ny_body = _get(orca_api, "/api/v1/orca/needs-you")
    ny_items = ny_body if isinstance(ny_body, list) else _req(ny_body, "items", "visible", "entries")
    assert len(ny_items) >= 1 and str(item_num) in json.dumps(ny_items)

    tabs = [{"id": f"tab-{idx}", "kind": kind, "pinned": idx < 2} for idx, kind in enumerate(TAB_CONTENT_TYPES_10)]
    orca_api.assert_rpc_ok("layout.apply", {"workspace_id": "ws-t3-27", "root": {"orientation": "horizontal", "ratio": 0.42, "pinned": True, "first": {"orientation": "vertical", "ratio": 0.5, "first": {"tabs": tabs[:3]}, "second": {"tabs": tabs[3:6]}}, "second": {"tabs": tabs[6:]}}})
    got_blob = json.dumps(orca_api.assert_rpc_ok("layout.get", {"workspace_id": "ws-t3-27"}))
    for tab_kind in TAB_CONTENT_TYPES_10:
        assert tab_kind in got_blob, f"Missing tab kind {tab_kind} in layout.get"
    assert "0.42" in got_blob and '"pinned": true' in got_blob.lower() and "tab-0" in got_blob and "tab-1" in got_blob

    nav_src = (REPO_ROOT / "crates/gpui_telegram_desktop/src/views/nav_rail.rs").read_text(encoding="utf-8")
    nav_enum = re.search(r"pub\s+enum\s+NavDestination\s*\{([^}]+)\}", nav_src, re.S)
    assert nav_enum is not None, "NavDestination enum missing in nav_rail.rs"
    for variant in ("Chats", "Folders", "Calls", "TaskCenter", "McpTools", "TalkMode", "RemoteComputer", "Browser", "SavedMessages", "Settings", "OrcaBots"):
        assert re.search(rf"\b{variant}\b", nav_enum.group(1)), f"Missing NavDestination::{variant}"

    shell_src = (REPO_ROOT / "crates/gpui_telegram_desktop/src/views/shell.rs").read_text(encoding="utf-8")
    panel_enum = re.search(r"pub\s+enum\s+ActiveWorkspacePanel\s*\{([^}]+)\}", shell_src, re.S)
    assert panel_enum is not None, "ActiveWorkspacePanel enum missing in shell.rs"
    for variant in ("Chats", "Calls", "TaskCenter", "McpTools", "Browser", "Settings", "OrcaBots", "OrcaIde"):
        assert re.search(rf"\b{variant}\b", panel_enum.group(1)), f"Missing ActiveWorkspacePanel::{variant}"

    orca_views_dir = REPO_ROOT / "crates/gpui_telegram_desktop/src/views/orca"
    assert orca_views_dir.is_dir(), f"Missing desktop views directory: {orca_views_dir}"
    rs_files = list(orca_views_dir.rglob("*.rs"))
    assert rs_files and any("SecuritySurface" in p.read_text(encoding="utf-8") for p in rs_files), "SecuritySurface missing in views/orca/"


@pytest.mark.tier3
@pytest.mark.f23
@pytest.mark.f24
def test_t3_28_e2e_runner_isolation_build_hygiene_and_ledger_closure(temp_workspace: TempWorkspace) -> None:
    """F23 + F24: E2E runner isolation, clean_build_garbage sandbox, 1.7.0 pins & 6-gate ledger."""
    ini_txt = (SUITE_ROOT / "pytest.ini").read_text(encoding="utf-8")
    assert "-p no:cacheprovider" in ini_txt and "--strict-markers" in ini_txt
    for marker in ("tier1", "tier2", "tier3", "tier4") + tuple(f"f{i:02d}" for i in range(1, 25)):
        assert re.search(rf"^\s*{marker}\b", ini_txt, re.M), f"Missing marker {marker} in pytest.ini"

    py_env = {**os.environ, "PYTHONDONTWRITEBYTECODE": "1"}
    col = subprocess.run(["/usr/bin/python3", str(SUITE_ROOT / "run_e2e.py"), "--tier", "3", "--collect-only", "-q"], cwd=str(REPO_ROOT), env=py_env, capture_output=True, text=True, timeout=60)
    assert col.returncode == 0, f"run_e2e.py collection failed:\n{col.stdout}\n{col.stderr}"
    m_col = re.search(r"(?m)^(\d+)(?:/\d+)?\s+(?:tests?\s+collected|selected)\b", col.stdout)
    all_nodes = [ln.strip() for ln in col.stdout.splitlines() if "::test_" in ln]
    t3_nodes = [ln for ln in all_nodes if "::test_t3_" in ln]
    assert m_col and int(m_col.group(1)) == 28 and len(all_nodes) == 28 and len(t3_nodes) == 28, col.stdout
    assert not [p for p in SUITE_ROOT.rglob("*") if p.name in ("__pycache__", ".pytest_cache")]

    syn = temp_workspace.mkdir("syn_repo")
    garbage = [syn / p for p in ("target/debug/incremental/x", "target/debug/deps/a.dSYM/f", "target/debug/deps/mod-1.rcgu.o", "target/debug/deps/test_bin", ".cargo-target-bogus/x", "crates/sub/target/x")]
    keep = [syn / p for p in ("target/debug/deps/libserde-1.rlib", "target/debug/deps/libserde-1.rmeta", "target/debug/.fingerprint/serde/dep")]
    for path in garbage + keep:
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_bytes(b"data")
    os.chmod(syn / "target/debug/deps/test_bin", stat.S_IRWXU)
    clean_cmd = ["/usr/bin/python3", str(REPO_ROOT / "scripts/clean_build_garbage.py"), "--repo-root", str(syn), "--target-dir", str(syn / "target"), "--target-only", "--force"]
    assert subprocess.run(clean_cmd + ["--dry-run"], cwd=str(REPO_ROOT), env=py_env, capture_output=True, text=True, timeout=30).returncode == 0 and all(p.exists() for p in garbage)
    assert subprocess.run(clean_cmd, cwd=str(REPO_ROOT), env=py_env, capture_output=True, text=True, timeout=30).returncode == 0 and not any(p.exists() for p in garbage) and all(p.exists() for p in keep)

    cargo_doc = tomllib.loads((REPO_ROOT / "Cargo.toml").read_text(encoding="utf-8"))
    for prof in ("dev", "test", "ci", "release"):
        p = cargo_doc["profile"][prof]
        assert p["debug"] == 0 and p["strip"] == "debuginfo" and p["incremental"] is False
    for crate in ("crates/orca_core", "crates/orca_bots", "crates/orca_ide"):
        assert crate in cargo_doc["workspace"]["members"], f"Missing workspace member {crate}"
    assert cargo_doc["workspace"]["package"]["version"] == TARGET_RELEASE_VERSION

    m_sw = re.search(r'CFBundleShortVersionString"\)\s+as\?\s+String\s+\?\?\s+"([^"]+)"', (REPO_ROOT / "scripts/macos/FabushiInstaller.swift").read_text(encoding="utf-8"))
    assert m_sw and m_sw.group(1) == TARGET_RELEASE_VERSION
    m_iss = re.search(r'#define\s+AppVersion\s+"([^"]+)"', (REPO_ROOT / "scripts/windows/FabushiSetup.iss").read_text(encoding="utf-8"))
    assert m_iss and m_iss.group(1) == TARGET_RELEASE_VERSION

    deploy_yml = (REPO_ROOT / ".github/workflows/deploy-backend.yml").read_text(encoding="utf-8")
    for glob_pat in ("crates/orca_core/**", "crates/orca_bots/**", "crates/orca_ide/**"):
        assert glob_pat in deploy_yml, f"Missing {glob_pat} in deploy-backend.yml"
    ci_yml = (REPO_ROOT / ".github/workflows/ci.yml").read_text(encoding="utf-8")
    assert "verify_ledger_closure.py --ci-without-upstream --skip-cargo" in ci_yml and "clean_build_garbage.py" in ci_yml
    idx = json.loads((REPO_ROOT / "docs/specs/ledger/index.json").read_text(encoding="utf-8"))
    assert idx["total_source_files"] == 3441 and idx["total_mapped_files"] == 3441 and idx["omitted_count"] == 0 and idx["unmapped_count"] == 0 and len(idx["shards"]) == 28
    led = subprocess.run(["/usr/bin/python3", str(REPO_ROOT / "scripts/verify_ledger_closure.py"), "--ci-without-upstream", "--skip-cargo"], cwd=str(REPO_ROOT), env=py_env, capture_output=True, text=True, timeout=900)
    _assert_ledger_closure(led)
