"""Tier 2 boundary, negative, security & edge-case E2E tests for F18 and F19."""

from __future__ import annotations

import base64
import json
import struct
import time
import zlib
from pathlib import Path
from typing import Any

import pytest

from harness import (
    OrcaApiClient,
    OrcaCliRunner,
    OrcaServerHandle,
    PLUGIN_ACTION_FALLBACK_TIMEOUT_SEC,
    PLUGIN_APPROVAL_REQUIRED_EXIT_CODE,
    TempWorkspace,
)


def _rpc_err(
    api: OrcaApiClient, method: str, expected_code: str, params: dict[str, Any] | None = None
) -> dict[str, Any]:
    resp = api.rpc(method, params=params)
    assert resp.status not in (404, 501), f"Missing/unimplemented RPC ({resp.status}): {resp.text[:300]}"
    return api.assert_rpc_error(resp, expected_code)


def _rpc_err_in(
    api: OrcaApiClient, method: str, allowed_codes: tuple[str, ...], params: dict[str, Any] | None = None
) -> dict[str, Any]:
    resp = api.rpc(method, params=params)
    assert resp.status not in (404, 501), f"Missing/unimplemented RPC ({resp.status}): {resp.text[:300]}"
    payload = resp.json()
    assert isinstance(payload, dict) and payload.get("ok") is False, payload
    err = payload.get("error")
    assert isinstance(err, dict) and err.get("code") in allowed_codes, (allowed_codes, payload)
    return err


def _minimal_png_b64() -> str:
    def _chunk(tag: bytes, data: bytes) -> bytes:
        return struct.pack("!I", len(data)) + tag + data + struct.pack("!I", zlib.crc32(tag + data) & 0xFFFFFFFF)

    ihdr = struct.pack("!IIBBBBB", 1, 1, 8, 6, 0, 0, 0)
    idat = zlib.compress(b"\x00\x00\x00\x00\xff")
    return base64.b64encode(b"\x89PNG\r\n\x1a\n" + _chunk(b"IHDR", ihdr) + _chunk(b"IDAT", idat) + _chunk(b"IEND", b"")).decode("ascii")


def _write_plugin(ws: TempWorkspace, folder: str, manifest: dict[str, Any], files: dict[str, str | bytes] | None = None) -> Path:
    root = ws.mkdir(f"plugins_t2/{folder}")
    (root / "orca-plugin.json").write_text(json.dumps(manifest, indent=2), encoding="utf-8")
    for rel, content in (files or {}).items():
        p = root / rel
        p.parent.mkdir(parents=True, exist_ok=True)
        if isinstance(content, bytes):
            p.write_bytes(content)
        else:
            p.write_text(content, encoding="utf-8")
    return root


@pytest.mark.tier2
@pytest.mark.f18
@pytest.mark.edge_case("SM2-24")
def test_f18_t2_sm2_24_computer_use_blocks_destructive_shortcuts_and_foreground_steal(
    orca_api: OrcaApiClient, orca_cli: OrcaCliRunner
) -> None:
    """F18 T2 [SM2-24]: Blocks Cmd+Q/Alt+F4/Win+L/Super+L & foreground steal; explicit override unlocks."""
    for shortcut in ("Cmd+Q", "Alt+F4", "Win+L", "Super+L"):
        orca_api.post("/api/v1/orca/ide/computer/guard", json_body={"key": shortcut}).assert_error_code(
            "system_shortcut_blocked", 400, 403
        )
        _rpc_err(orca_api, "computer.hotkey", "system_shortcut_blocked", {"key": shortcut})
        orca_cli.run("ide", "computer-hotkey", "--key", shortcut).assert_error("system_shortcut_blocked")

    _rpc_err_in(
        orca_api,
        "computer.click",
        ("foreground_steal_blocked", "system_shortcut_blocked", "invalid_argument"),
        {"app": "Fabushi", "x": 100, "y": 100, "foreground": True, "restore_window": False},
    )
    click_ok = orca_api.assert_rpc_ok(
        "computer.click",
        {"app": "Fabushi", "x": 100, "y": 100, "foreground": True, "restore_window": True},
    )
    assert isinstance(click_ok, dict) and click_ok.get("allowed") is True and click_ok.get("restore_window") is True, click_ok

    rest_ov = orca_api.assert_ok(
        orca_api.post(
            "/api/v1/orca/ide/computer/guard",
            json_body={"key": "Cmd+Q", "allow_system_shortcut": True},
        )
    )
    assert isinstance(rest_ov, dict) and rest_ov.get("allowed") is True, rest_ov
    rpc_ov = orca_api.assert_rpc_ok(
        "computer.hotkey", {"key": "Cmd+Q", "allow_system_shortcut": True}
    )
    assert isinstance(rpc_ov, dict) and rpc_ov.get("allowed") is True, rpc_ov
    cli_ov = orca_cli.run("ide", "computer-hotkey", "--key", "Cmd+Q", "--allow-system-shortcut").assert_ok()
    assert isinstance(cli_ov, dict) and cli_ov.get("allowed") is True, cli_ov


@pytest.mark.tier2
@pytest.mark.f18
def test_f18_t2_design_mode_omits_missing_sourcemap_and_rejects_malformed_capture(
    orca_api: OrcaApiClient,
) -> None:
    """F18 T2: Missing source_map is omitted cleanly; empty outer_html or non-PNG bytes fail closed."""
    ok = orca_api.assert_rpc_ok(
        "browser.designMode.capture",
        {
            "tab_id": "tab-dm-t2",
            "selector": "div.card",
            "outer_html": '<div class="card">Hello</div>',
            "computed_styles": {"display": "flex"},
            "crop": {"x": 0, "y": 0, "width": 50, "height": 50},
            "screenshot_png_b64": _minimal_png_b64(),
            "source_map": None,
        },
    )
    assert isinstance(ok, dict) and ok.get("source_map") is None and isinstance(ok.get("outer_html"), str) and "card" in ok["outer_html"], ok
    assert ok["computed_styles"]["display"] == "flex" and base64.b64decode(str(ok["screenshot_png_b64"])).startswith(b"\x89PNG\r\n\x1a\n")

    _rpc_err(
        orca_api,
        "browser.designMode.capture",
        "invalid_argument",
        {"tab_id": "tab-dm-t2", "selector": "div.card", "outer_html": "", "screenshot_png_b64": _minimal_png_b64()},
    )
    _rpc_err(
        orca_api,
        "browser.designMode.capture",
        "invalid_argument",
        {
            "tab_id": "tab-dm-t2",
            "selector": "div.card",
            "outer_html": "<div>x</div>",
            "screenshot_png_b64": base64.b64encode(b"NOT_A_VALID_PNG_HEADER").decode("ascii"),
        },
    )


@pytest.mark.tier2
@pytest.mark.f18
def test_f18_t2_computer_use_wait_returns_wait_timeout(orca_api: OrcaApiClient) -> None:
    """F18 T2: Unsatisfiable computer.wait fails closed with exact wait_timeout."""
    assert isinstance(orca_api.assert_rpc_ok("runtime.capabilities", {}), dict)
    t0 = time.monotonic()
    _rpc_err(orca_api, "computer.wait", "wait_timeout", {"app": "Fabushi", "predicate": {"text": "NeverAppearingLabelXYZ"}, "timeout_ms": 150})
    assert (time.monotonic() - t0) < 10.0


@pytest.mark.tier2
@pytest.mark.f18
def test_f18_t2_emulator_unsupported_platform_returns_emulator_unsupported(orca_api: OrcaApiClient) -> None:
    """F18 T2: Unsupported emulator target fails closed with exact emulator_unsupported."""
    assert isinstance(orca_api.assert_rpc_ok("emulator.list", {}), (dict, list))
    _rpc_err(orca_api, "emulator.start", "emulator_unsupported", {"platform": "unsupported_os_sim"})


@pytest.mark.tier2
@pytest.mark.f18
def test_f18_t2_emulator_input_blocked_and_out_of_bounds_coords_rejected(orca_api: OrcaApiClient) -> None:
    """F18 T2: Locked emulator session returns emulator_input_blocked; OOB coords fail closed."""
    assert isinstance(orca_api.assert_rpc_ok("emulator.list", {}), (dict, list))
    _rpc_err(orca_api, "emulator.tap", "emulator_input_blocked", {"device_id": "emu-locked-1", "input_locked": True, "x": 0.5, "y": 0.5})
    for x, y in ((-0.05, 0.5), (0.5, 1.25)):
        _rpc_err(orca_api, "emulator.tap", "invalid_argument", {"device_id": "emu-android-1", "x": x, "y": y})


@pytest.mark.tier2
@pytest.mark.f18
def test_f18_t2_browser_cert_error_and_url_mismatch_enforce_security_surface(
    orca_api: OrcaApiClient,
) -> None:
    """F18 T2: Untrusted TLS cert and URL mismatch surface non-bypassable SecuritySurface prompt."""
    cert = orca_api.assert_rpc_ok(
        "browser.certTrust.prompt",
        {"tab_id": "tab-sec-1", "url": "https://selfsigned.local", "cert_trusted": False},
    )
    assert isinstance(cert, dict) and cert.get("surface") == "security" and cert.get("auto_trusted") is False, cert

    nav = orca_api.assert_rpc_ok(
        "browser.navigate",
        {"tab_id": "tab-sec-1", "url": "https://evil.example", "expected_url": "https://trusted.example"},
    )
    assert isinstance(nav, dict) and nav.get("surface") == "security" and nav.get("auto_trusted") is False, nav


@pytest.mark.tier2
@pytest.mark.f19
def test_f19_t2_trusted_worker_without_approval_exits_3_with_approval_required(
    orca_api: OrcaApiClient, orca_cli: OrcaCliRunner, temp_workspace: TempWorkspace
) -> None:
    """F19 T2: Unapproved or wrong-fingerprint trusted-worker exits 3 with approval_required."""
    tw_dir = _write_plugin(
        temp_workspace,
        "unapproved-worker",
        {
            "schema_version": 2,
            "publisher": "acme",
            "id": "unapproved-worker",
            "version": "1.0.0",
            "tier": "trusted-worker",
            "capabilities": ["storage", "hooks.agentLaunch"],
        },
        {"worker.js": "export function main() {}"},
    )
    res_missing = orca_cli.run("ide", "plugin-install", "--path", str(tw_dir)).assert_error(
        "approval_required", expected_exit=PLUGIN_APPROVAL_REQUIRED_EXIT_CODE
    )
    err_blob = json.dumps(res_missing.json()).lower()
    assert "fingerprint" in err_blob

    orca_cli.run("ide", "plugin-install", "--path", str(tw_dir), "--approve", "bad-fp-000").assert_error(
        "approval_required", expected_exit=PLUGIN_APPROVAL_REQUIRED_EXIT_CODE
    )
    orca_api.post("/api/v1/orca/ide/plugins", json_body={"path": str(tw_dir)}).assert_error_code(
        "approval_required", 403, 409, 412
    )
    _rpc_err(orca_api, "plugins.install", "approval_required", {"path": str(tw_dir)})
    listed = orca_api.assert_ok(orca_api.get("/api/v1/orca/ide/plugins"))
    plugins = listed if isinstance(listed, list) else next((listed[k] for k in ("plugins", "items") if isinstance(listed, dict) and isinstance(listed.get(k), list)), None)
    assert isinstance(plugins, list) and "acme.unapproved-worker" not in json.dumps(plugins), listed


@pytest.mark.tier2
@pytest.mark.f19
def test_f19_t2_plugin_system_disabled_rejects_install_and_activation(
    fresh_orca_server: OrcaServerHandle, cli_bin_path: Path, temp_workspace: TempWorkspace
) -> None:
    """F19 T2: Disabling plugin system rejects even data-tier installs across REST/RPC/CLI."""
    sess = fresh_orca_server.register_user()
    api = fresh_orca_server.api_client(sess)
    cli = OrcaCliRunner(
        cli_bin=cli_bin_path,
        server_url=fresh_orca_server.base_url,
        token=sess.token,
        work_dir=temp_workspace.mkdir("cli_plug_disabled"),
    )
    disabled = api.assert_rpc_ok("plugins.setEnabled", {"enabled": False})
    assert isinstance(disabled, dict) and disabled.get("enabled") is False, disabled

    d_dir = _write_plugin(
        temp_workspace,
        "blocked-data",
        {"schema_version": 2, "publisher": "acme", "id": "blocked-data", "version": "1.0.0", "tier": "data"},
    )
    api.post("/api/v1/orca/ide/plugins", json_body={"path": str(d_dir)}).assert_error_code(
        "plugin_system_disabled", 403, 409, 412, 422
    )
    _rpc_err(api, "plugins.install", "plugin_system_disabled", {"path": str(d_dir)})
    cli.run("ide", "plugin-install", "--path", str(d_dir)).assert_error("plugin_system_disabled")


@pytest.mark.tier2
@pytest.mark.f19
@pytest.mark.edge_case("SM1-12")
def test_f19_t2_sm1_12_security_surface_2s_fallback_hooks_and_reserved_ids(
    orca_api: OrcaApiClient, orca_cli: OrcaCliRunner, temp_workspace: TempWorkspace
) -> None:
    """F19 T2 [SM1-12]: SecuritySurface immunity, 2s action fallback, hook onError, reserved IDs."""
    spoof_dir = _write_plugin(
        temp_workspace,
        "spoof-theme",
        {
            "schema_version": 2,
            "publisher": "acme",
            "id": "spoof-theme",
            "version": "1.0.0",
            "tier": "data",
            "contributes": {"themes": [{"id": "evil", "path": "theme.json"}]},
        },
        {"theme.json": json.dumps({"securitySurface.bg": "#00ff00", "securitySurface.text": "#00ff00"})},
    )
    orca_api.assert_ok(orca_api.post("/api/v1/orca/ide/plugins", json_body={"path": str(spoof_dir)}))
    sec_tokens = orca_api.assert_rpc_ok("plugins.resolveThemeTokens", {"surface": "security"})
    assert isinstance(sec_tokens, dict) and sec_tokens.get("security_surface_protected") is True, sec_tokens
    resolved_tokens = sec_tokens.get("tokens") if "tokens" in sec_tokens else sec_tokens.get("resolved_tokens")
    assert isinstance(resolved_tokens, dict) and resolved_tokens, sec_tokens
    assert "#00ff00" not in json.dumps(sec_tokens).lower(), sec_tokens

    fast_act = orca_api.assert_rpc_ok("plugins.invokeAction", {"action": "git.stage", "simulate_plugin_delay_ms": 10})
    assert isinstance(fast_act, dict) and fast_act.get("fallback_used") is False, fast_act
    t0 = time.monotonic()
    act = orca_api.assert_rpc_ok(
        "plugins.invokeAction",
        {"action": "git.stage", "simulate_plugin_delay_ms": (PLUGIN_ACTION_FALLBACK_TIMEOUT_SEC + 2) * 1000},
    )
    elapsed = time.monotonic() - t0
    assert isinstance(act, dict) and act.get("fallback_used") is True and act.get("builtin_executed") is True, act
    assert elapsed < PLUGIN_ACTION_FALLBACK_TIMEOUT_SEC + 1.8, f"Fallback exceeded 2s bound: {elapsed:.2f}s"
    assert elapsed >= PLUGIN_ACTION_FALLBACK_TIMEOUT_SEC - 0.25 or act.get("timeout_ms") == PLUGIN_ACTION_FALLBACK_TIMEOUT_SEC * 1000, (elapsed, act)

    h_pass = orca_api.assert_rpc_ok(
        "plugins.evaluateHook", {"event": "agentLaunch", "onError": "pass", "simulate_crash": True}
    )
    assert isinstance(h_pass, dict) and h_pass.get("allowed") is True and h_pass.get("hook_failed") is True, h_pass
    h_deny = orca_api.assert_rpc_ok(
        "plugins.evaluateHook", {"event": "agentLaunch", "onError": "deny", "simulate_crash": True}
    )
    assert isinstance(h_deny, dict) and h_deny.get("allowed") is False and h_deny.get("hook_failed") is True, h_deny

    for pub, pid in (("constructor", "bad1"), ("acme", "prototype"), ("orca-core", "bad2")):
        bad_dir = _write_plugin(
            temp_workspace,
            f"res-{pub}-{pid}",
            {"schema_version": 2, "publisher": pub, "id": pid, "version": "1.0.0", "tier": "data"},
        )
        orca_cli.run("ide", "plugin-validate", "--path", str(bad_dir)).assert_error("invalid_argument")
        orca_api.post("/api/v1/orca/ide/plugins", json_body={"path": str(bad_dir)}).assert_error_code(
            "invalid_argument", 400, 422
        )


@pytest.mark.tier2
@pytest.mark.f19
def test_f19_t2_manifest_enforces_bundle_size_file_caps_and_tier_isolation(
    orca_api: OrcaApiClient, orca_cli: OrcaCliRunner, temp_workspace: TempWorkspace
) -> None:
    """F19 T2: Rejects >1 MiB single file, >2000-file bundle cap on disk, and sandboxed-ui escalation."""
    ok_dir = _write_plugin(temp_workspace, "ok-minimal", {"schema_version": 2, "publisher": "acme", "id": "ok-minimal", "version": "1.0.0", "tier": "data"})
    assert orca_cli.run("ide", "plugin-validate", "--path", str(ok_dir)).assert_ok().get("valid") is True
    orca_api.assert_ok(orca_api.post("/api/v1/orca/ide/plugins", json_body={"path": str(ok_dir)}).assert_status(200, 201))

    big_dir = _write_plugin(
        temp_workspace, "oversize-file",
        {"schema_version": 2, "publisher": "acme", "id": "oversize-file", "version": "1.0.0", "tier": "data"},
        {"blob.bin": b"A" * (1_048_576 + 1024)},
    )
    orca_cli.run("ide", "plugin-validate", "--path", str(big_dir)).assert_error("invalid_argument")
    orca_api.post("/api/v1/orca/ide/plugins", json_body={"path": str(big_dir)}).assert_error_code("invalid_argument", 400, 413, 422)

    cap_dir = _write_plugin(temp_workspace, "cap-overflow", {"schema_version": 2, "publisher": "acme", "id": "cap-overflow", "version": "1.0.0", "tier": "data"})
    cap_files = cap_dir / "files"
    cap_files.mkdir()
    for i in range(2001):
        (cap_files / f"f{i:04d}.txt").touch()
    orca_cli.run("ide", "plugin-validate", "--path", str(cap_dir)).assert_error("invalid_argument")
    orca_api.post("/api/v1/orca/ide/plugins", json_body={"path": str(cap_dir)}).assert_error_code("invalid_argument", 400, 413, 422)

    esc_dir = _write_plugin(
        temp_workspace, "tier-escalation",
        {"schema_version": 2, "publisher": "acme", "id": "tier-escalation", "version": "1.0.0", "tier": "sandboxed-ui", "capabilities": ["network", "hooks.agentLaunch"]},
    )
    orca_cli.run("ide", "plugin-validate", "--path", str(esc_dir)).assert_error("invalid_argument")
    orca_api.post("/api/v1/orca/ide/plugins", json_body={"path": str(esc_dir)}).assert_error_code("invalid_argument", 400, 403, 422)


@pytest.mark.tier2
@pytest.mark.f19
def test_f19_t2_transactional_skill_install_heals_interrupted_staging_on_restart(
    fresh_orca_server: OrcaServerHandle, temp_workspace: TempWorkspace
) -> None:
    """F19 T2: Interrupted skill staging marker heals on restart while committed skills stay intact."""
    sess = fresh_orca_server.register_user()
    api = fresh_orca_server.api_client(sess)

    good_src = temp_workspace.mkdir("skills_src_t2/good-skill")
    (good_src / "SKILL.md").write_text("# good-skill\nCommitted skill.\n", encoding="utf-8")
    target_root = temp_workspace.mkdir("skills_root_t2")
    api.assert_rpc_ok("skills.install", {"source_path": str(good_src), "target_root": str(target_root)})

    txn_marker = target_root / ".txn-broken-99"
    txn_marker.write_text('{"skill":"broken-skill","state":"staging"}', encoding="utf-8")
    broken_stage = target_root / "broken-skill.staging"
    broken_stage.mkdir(parents=True, exist_ok=True)
    (broken_stage / "SKILL.md").write_text("# partial\n", encoding="utf-8")

    fresh_orca_server.restart()
    api_after = fresh_orca_server.api_client(sess)
    api_after.assert_rpc_ok("skills.recover", {"target_root": str(target_root)})
    listed_blob = json.dumps(api_after.assert_rpc_ok("skills.list", {"target_root": str(target_root)}))
    assert "good-skill" in listed_blob and "broken-skill" not in listed_blob
    assert not txn_marker.exists() and not broken_stage.exists()
    assert any(p.is_file() and p.name == "SKILL.md" and "Committed skill." in p.read_text(encoding="utf-8", errors="replace") for p in target_root.rglob("*"))


@pytest.mark.tier2
@pytest.mark.f19
def test_f19_t2_mcp_sync_rejects_plaintext_secret_and_invalid_transport(
    orca_api: OrcaApiClient, temp_workspace: TempWorkspace
) -> None:
    """F19 T2: MCP registry rejects bad transports/commands and blocks/scrubs raw API secrets."""
    orca_api.assert_rpc_ok("mcpServers.add", {"name": "clean-stdio", "transport": "stdio", "command": "npx", "args": ["-y", "docs-mcp"]})
    assert isinstance(orca_api.assert_rpc_ok("mcpServers.list", {}), (dict, list))
    _rpc_err(orca_api, "mcpServers.add", "invalid_argument", {"name": "bad-ftp", "transport": "ftp", "url": "ftp://x"})
    _rpc_err(orca_api, "mcpServers.add", "invalid_argument", {"name": "bad-stdio", "transport": "stdio", "command": ""})

    raw_secret = "sk-ant-live-secret-12345"
    sync_dir = temp_workspace.mkdir("mcp_sync_sec")
    resp = orca_api.rpc(
        "mcpServers.add",
        {
            "name": "leaky-http",
            "transport": "streamable-http",
            "url": "https://mcp.example.test/sse",
            "headers": {"Authorization": f"Bearer {raw_secret}"},
        },
    )
    assert resp.status not in (404, 501), f"Missing/unimplemented RPC ({resp.status})"
    payload = resp.json()
    assert isinstance(payload, dict)
    if payload.get("ok") is False:
        err = payload.get("error")
        assert isinstance(err, dict) and err.get("code") in ("secret_blocked", "invalid_argument"), payload
    else:
        assert resp.status == 200 and payload.get("ok") is True, (resp.status, payload)
        assert raw_secret not in json.dumps(payload), payload
        orca_api.assert_rpc_ok("mcpServers.sync", {"target_dir": str(sync_dir), "agents": ["claude"]})
        synced_files = [p for p in sync_dir.rglob("*") if p.is_file()]
        assert synced_files, f"Expected synced config files in {sync_dir}"
        for p in synced_files:
            assert raw_secret not in p.read_text(encoding="utf-8", errors="replace"), f"Secret leaked in {p}"
