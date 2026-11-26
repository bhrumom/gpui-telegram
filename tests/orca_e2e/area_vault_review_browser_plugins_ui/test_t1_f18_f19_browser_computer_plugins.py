"""Tier 1 opaque-box E2E tests for F18 (Browser/Computer/Emulator) and F19 (Plugins/MCP/Skills)."""

from __future__ import annotations

import base64
import json
import struct
import zlib
from pathlib import Path
from typing import Any

import pytest

from harness import OrcaApiClient, OrcaCliRunner, PLUGIN_TIERS_4, TempWorkspace


def _minimal_png_b64() -> str:
    """Build a valid 1x1 RGBA PNG using Python stdlib only."""
    def _chunk(tag: bytes, data: bytes) -> bytes:
        crc = zlib.crc32(tag + data) & 0xFFFFFFFF
        return struct.pack("!I", len(data)) + tag + data + struct.pack("!I", crc)

    ihdr = struct.pack("!IIBBBBB", 1, 1, 8, 6, 0, 0, 0)
    idat = zlib.compress(b"\x00\x20\x40\x80\xff")
    png = b"\x89PNG\r\n\x1a\n" + _chunk(b"IHDR", ihdr) + _chunk(b"IDAT", idat) + _chunk(b"IEND", b"")
    return base64.b64encode(png).decode("ascii")


def _write_plugin(ws: TempWorkspace, folder: str, manifest: dict[str, Any], files: dict[str, str] | None = None) -> Path:
    root = ws.mkdir(f"plugins/{folder}")
    (root / "orca-plugin.json").write_text(json.dumps(manifest, indent=2), encoding="utf-8")
    for rel, content in (files or {}).items():
        p = root / rel
        p.parent.mkdir(parents=True, exist_ok=True)
        p.write_text(content, encoding="utf-8")
    return root


def _extract_list(payload: Any, *keys: str) -> list[dict[str, Any]]:
    if isinstance(payload, list):
        return [x for x in payload if isinstance(x, dict)]
    if isinstance(payload, dict):
        for k in (*keys, "items", "plugins", "entries", "servers", "skills", "nodes"):
            if isinstance(payload.get(k), list):
                return [x for x in payload[k] if isinstance(x, dict)]
    raise AssertionError(f"Expected list under {keys!r}, got: {payload!r}")


@pytest.mark.tier1
@pytest.mark.f18
def test_f18_t1_design_mode_element_capture_dom_css_png_sourcemap_bundle(
    orca_api: OrcaApiClient,
) -> None:
    """F18 T1: Design Mode captures outer HTML, computed CSS, cropped PNG, and source map."""
    toggled = orca_api.assert_rpc_ok(
        "browser.designMode.toggle", {"tab_id": "tab-dm-1", "enabled": True}
    )
    assert toggled.get("enabled") is True

    png_b64 = _minimal_png_b64()
    captured = orca_api.assert_rpc_ok(
        "browser.designMode.capture",
        {
            "tab_id": "tab-dm-1",
            "selector": "button.primary-cta",
            "outer_html": '<button class="primary-cta">Deploy</button>',
            "dom_neighborhood": "<main><button class='primary-cta'>Deploy</button></main>",
            "computed_styles": {"color": "rgb(255, 255, 255)", "fontFamily": "Inter", "padding": "8px 16px"},
            "crop": {"x": 10, "y": 20, "width": 120, "height": 40},
            "screenshot_png_b64": png_b64,
            "source_map": {"file": "src/Button.tsx", "line": 42, "column": 8},
        },
    )
    assert "primary-cta" in str(captured["outer_html"])
    assert captured["computed_styles"]["fontFamily"] == "Inter"
    assert base64.b64decode(str(captured["screenshot_png_b64"])).startswith(b"\x89PNG\r\n\x1a\n")
    assert captured["source_map"]["file"] == "src/Button.tsx"
    assert int(captured["source_map"]["line"]) == 42
    attachment = captured.get("chat_attachment") or captured.get("attachment")
    assert isinstance(attachment, dict) and attachment


@pytest.mark.tier1
@pytest.mark.f18
def test_f18_t1_browser_session_profile_isolation_and_viewport_toggle(
    orca_api: OrcaApiClient,
) -> None:
    """F18 T1: Per-workspace vs shared profiles isolate namespaces; Web/Mobile viewport & @e1 refs."""
    prof_a = orca_api.assert_rpc_ok(
        "browser.profile.configure", {"scope": "per-workspace", "workspace_id": "ws-alpha"}
    )
    prof_b = orca_api.assert_rpc_ok(
        "browser.profile.configure", {"scope": "per-workspace", "workspace_id": "ws-beta"}
    )
    prof_shared = orca_api.assert_rpc_ok("browser.profile.configure", {"scope": "shared"})
    raw_ids = [p.get("profile_id") for p in (prof_a, prof_b, prof_shared)]
    assert all(isinstance(pid, str) and pid.strip() for pid in raw_ids) and len(set(raw_ids)) == 3, (prof_a, prof_b, prof_shared)

    vp_web = orca_api.assert_rpc_ok(
        "browser.viewport.set", {"tab_id": "tab-vp-1", "preset": "web", "width": 1280, "height": 800}
    )
    assert int(vp_web["width"]) == 1280 and int(vp_web["height"]) == 800
    vp_mob = orca_api.assert_rpc_ok(
        "browser.viewport.set", {"tab_id": "tab-vp-1", "preset": "mobile", "width": 390, "height": 844}
    )
    assert int(vp_mob["width"]) == 390 and int(vp_mob["height"]) == 844

    snap = orca_api.assert_rpc_ok("browser.snapshot", {"tab_id": "tab-vp-1", "interactive_refs": True})
    assert "@e1" in json.dumps(snap) and "390" in json.dumps(snap)


@pytest.mark.tier1
@pytest.mark.f18
def test_f18_t1_computer_use_guard_allows_safe_hotkeys_rest_and_cli(
    orca_api: OrcaApiClient, orca_cli: OrcaCliRunner
) -> None:
    """F18 T1: Computer-use guard allows safe hotkeys across REST and fabushi CLI."""
    for key in ("Cmd+C", "Ctrl+L", "Shift+Tab"):
        payload = orca_api.assert_ok(
            orca_api.post("/api/v1/orca/ide/computer/guard", json_body={"key": key})
        )
        assert isinstance(payload, dict) and payload.get("allowed") is True, payload
        assert payload.get("requires_override") is False, payload

    cli_res = orca_cli.run("ide", "computer-hotkey", "--key", "Cmd+C").assert_ok()
    assert isinstance(cli_res, dict) and cli_res.get("allowed") is True, cli_res
    assert isinstance(cli_res.get("key"), str) and cli_res["key"].lower() == "cmd+c", cli_res


@pytest.mark.tier1
@pytest.mark.f18
def test_f18_t1_computer_use_ax_app_state_query_tree_diff_and_wait(
    orca_api: OrcaApiClient,
) -> None:
    """F18 T1: Computer-use AX query, diff-only treeDiff (+/-/~), and predicate wait succeed."""
    state = orca_api.assert_rpc_ok(
        "computer.getAppState", {"app": "Fabushi", "query": "Submit"}
    )
    assert "Submit" in json.dumps(state)

    before = {"id": "root", "children": [{"id": "n1", "label": "Static"}, {"id": "n2", "label": "Draft"}]}
    after = {"id": "root", "children": [{"id": "n1", "label": "Static"}, {"id": "n2", "label": "Ready"}, {"id": "n3", "label": "Badge"}]}
    diff = orca_api.assert_rpc_ok("computer.diffAppState", {"before": before, "after": after})
    diff_blob = json.dumps(diff)
    assert any(tok in diff_blob for tok in ("~", "+", "treeDiff", "changes")), diff
    assert "Ready" in diff_blob and "Badge" in diff_blob

    waited = orca_api.assert_rpc_ok(
        "computer.wait",
        {"app": "Fabushi", "predicate": {"text": "Ready", "enabled": True}, "timeout_ms": 1000},
    )
    assert isinstance(waited, dict) and waited.get("matched") is True, waited


@pytest.mark.tier1
@pytest.mark.f18
def test_f18_t1_computer_use_background_delivery_and_stdin_secret_redaction(
    orca_api: OrcaApiClient, orca_cli: OrcaCliRunner
) -> None:
    """F18 T1: Background input delivery avoids focus theft and redacts stdin/secret values."""
    secret = "super-secret-pin-4920"
    res = orca_api.assert_rpc_ok(
        "computer.setValue",
        {"app": "Fabushi", "target": "@e2", "value": secret, "secret": True, "background_delivery": True},
    )
    assert isinstance(res, dict) and res.get("background_delivery") is True and res.get("steals_focus") is False, res
    assert secret not in json.dumps(res), res

    cli_out = orca_cli.run("ide", "computer-hotkey", "--text-stdin", stdin_text=secret).assert_ok()
    assert isinstance(cli_out, dict) and cli_out and secret not in json.dumps(cli_out), cli_out

    audit = orca_api.assert_rpc_ok("computer.auditLog", {"app": "Fabushi"})
    audit_entries = _extract_list(audit, "entries", "events", "logs", "records")
    assert audit_entries and secret not in json.dumps(audit_entries), audit


@pytest.mark.tier1
@pytest.mark.f18
def test_f18_t1_mobile_emulator_normalized_coords_and_500_node_ax_cap(
    orca_api: OrcaApiClient,
) -> None:
    """F18 T1: Mobile emulator enforces 500-node AX cap and accepts normalized [0.0, 1.0] coords."""
    listed = orca_api.assert_rpc_ok("emulator.list", {})
    assert isinstance(_extract_list(listed, "devices", "emulators"), list)

    raw_nodes = [{"id": f"ax-{i}", "role": "button", "label": f"Item {i}"} for i in range(620)]
    ax = orca_api.assert_rpc_ok(
        "emulator.axTree", {"device_id": "emu-android-1", "raw_nodes": raw_nodes}
    )
    nodes = _extract_list(ax, "nodes")
    assert 0 < len(nodes) <= 500 and ax.get("truncated") is True

    tap = orca_api.assert_rpc_ok("emulator.tap", {"device_id": "emu-android-1", "x": 0.5, "y": 0.25})
    assert isinstance(tap, dict) and tap.get("accepted") is True, tap
    gest = orca_api.assert_rpc_ok(
        "emulator.gesture",
        {"device_id": "emu-android-1", "kind": "swipe", "from_x": 0.5, "from_y": 0.8, "to_x": 0.5, "to_y": 0.2},
    )
    assert isinstance(gest, dict) and gest.get("accepted") is True, gest


@pytest.mark.tier1
@pytest.mark.f19
def test_f19_t1_data_tier_plugin_validates_and_installs_without_approval(
    orca_api: OrcaApiClient, orca_cli: OrcaCliRunner, temp_workspace: TempWorkspace
) -> None:
    """F19 T1: data-tier plugin validates and installs cleanly without --approve."""
    p_dir = _write_plugin(
        temp_workspace,
        "nord-theme",
        {
            "schema_version": 2,
            "publisher": "acme",
            "id": "nord-theme",
            "version": "1.0.0",
            "tier": "data",
            "contributes": {
                "themes": [{"id": "nord", "path": "theme.json"}],
                "syntaxes": [{"scope": "source.orca", "path": "orca.tmLanguage.json"}],
                "languagePacks": [{"locale": "en", "path": "en.json"}],
            },
        },
        {"theme.json": '{"bg":"#2e3440"}', "orca.tmLanguage.json": "{}", "en.json": "{}"},
    )
    val = orca_cli.run("ide", "plugin-validate", "--path", str(p_dir)).assert_ok()
    assert val.get("valid") is True and val.get("tier") == "data"
    assert val.get("approval_required") is False

    orca_cli.run("ide", "plugin-install", "--path", str(p_dir)).assert_ok()
    listed = _extract_list(orca_api.assert_ok(orca_api.get("/api/v1/orca/ide/plugins")), "plugins")
    assert any(
        "acme.nord-theme" in (p.get("qualified_id"), f"{p.get('publisher')}.{p.get('id')}")
        for p in listed
    ), listed


@pytest.mark.tier1
@pytest.mark.f19
def test_f19_t1_declarative_and_sandboxed_ui_plugins_register_contributions(
    orca_api: OrcaApiClient, orca_cli: OrcaCliRunner, temp_workspace: TempWorkspace
) -> None:
    """F19 T1: declarative and sandboxed-ui plugins register contributions with network_access=false."""
    decl_dir = _write_plugin(
        temp_workspace,
        "decl-ops",
        {
            "schema_version": 2,
            "publisher": "acme",
            "id": "decl-ops",
            "version": "1.0.0",
            "tier": "declarative",
            "contributes": {
                "commands": [{"id": "acme.ping", "title": "Ping"}],
                "keybindings": [{"key": "Cmd+Alt+P", "command": "acme.ping"}],
                "statusBar": [{"id": "acme.status", "text": "Ready"}],
                "vmRecipes": [{"id": "ubuntu-dev", "image": "ubuntu:24.04"}],
            },
        },
    )
    ui_dir = _write_plugin(
        temp_workspace,
        "sandbox-panel",
        {
            "schema_version": 2,
            "publisher": "acme",
            "id": "sandbox-panel",
            "version": "1.0.0",
            "tier": "sandboxed-ui",
            "contributes": {
                "panels": [{"id": "acme.metrics", "entry": "panel.tsx"}],
                "customEditors": [{"id": "acme.csv", "entry": "editor.tsx"}],
            },
        },
        {"panel.tsx": "export default () => <box>Metrics</box>;", "editor.tsx": "export default () => <box/>;"},
    )
    assert orca_cli.run("ide", "plugin-validate", "--path", str(decl_dir)).assert_ok().get("valid") is True
    assert orca_cli.run("ide", "plugin-validate", "--path", str(ui_dir)).assert_ok().get("valid") is True

    orca_api.assert_ok(orca_api.post("/api/v1/orca/ide/plugins", json_body={"path": str(decl_dir)}))
    ui_inst = orca_api.assert_ok(orca_api.post("/api/v1/orca/ide/plugins", json_body={"path": str(ui_dir)}))
    assert ui_inst.get("tier") == "sandboxed-ui" and ui_inst.get("network_access") is False

    contribs = orca_api.assert_rpc_ok("plugins.contributions", {})
    blob = json.dumps(contribs)
    assert "acme.ping" in blob and "acme.metrics" in blob


@pytest.mark.tier1
@pytest.mark.f19
def test_f19_t1_trusted_worker_install_with_fingerprint_approval_and_audit_log(
    orca_api: OrcaApiClient, orca_cli: OrcaCliRunner, temp_workspace: TempWorkspace
) -> None:
    """F19 T1: trusted-worker installs with --approve <fingerprint> and logs plugin:<id> audit."""
    tw_dir = _write_plugin(
        temp_workspace,
        "audit-hooks",
        {
            "schema_version": 2,
            "publisher": "acme",
            "id": "audit-hooks",
            "version": "1.0.0",
            "tier": "trusted-worker",
            "capabilities": ["storage", "hooks.agentLaunch"],
            "contributes": {"hooks": [{"event": "agentLaunch", "onError": "pass"}]},
        },
        {"worker.js": "export function onAgentLaunch() { return { ok: true }; }"},
    )
    val = orca_cli.run("ide", "plugin-validate", "--path", str(tw_dir)).assert_ok()
    fp = str(val.get("capability_fingerprint") or val.get("fingerprint") or "")
    assert len(fp) >= 16, val

    orca_cli.run("ide", "plugin-install", "--path", str(tw_dir), "--approve", fp).assert_ok()
    audit = orca_api.assert_rpc_ok("plugins.auditLog", {"plugin_id": "acme.audit-hooks"})
    assert "plugin:acme.audit-hooks" in json.dumps(audit)


@pytest.mark.tier1
@pytest.mark.f19
def test_f19_t1_mcp_server_registry_and_agent_config_sync(
    orca_api: OrcaApiClient, temp_workspace: TempWorkspace
) -> None:
    """F19 T1: MCP server registry stores stdio + streamable-http handles and syncs clean configs."""
    orca_api.assert_rpc_ok(
        "mcpServers.add",
        {"name": "local-docs", "transport": "stdio", "command": "npx", "args": ["-y", "docs-mcp"]},
    )
    orca_api.assert_rpc_ok(
        "mcpServers.add",
        {
            "name": "remote-jira",
            "transport": "streamable-http",
            "url": "https://mcp.example.test/sse",
            "credential_handle": "vault://jira-oauth",
        },
    )
    listed = _extract_list(orca_api.assert_rpc_ok("mcpServers.list", {}), "servers")
    names = {str(s.get("name")) for s in listed}
    assert {"local-docs", "remote-jira"}.issubset(names), listed

    sync_dir = temp_workspace.mkdir("mcp_sync_out")
    orca_api.assert_rpc_ok(
        "mcpServers.sync", {"target_dir": str(sync_dir), "agents": ["claude", "codex"]}
    )
    synced_files = [p for p in sync_dir.rglob("*") if p.is_file()]
    assert synced_files, f"Expected synced agent configs in {sync_dir}"
    combined = "\n".join(p.read_text(encoding="utf-8", errors="replace") for p in synced_files)
    assert "local-docs" in combined and "remote-jira" in combined
    assert "sk-" not in combined


@pytest.mark.tier1
@pytest.mark.f19
def test_f19_t1_transactional_skill_install_discovery_and_share_link(
    orca_api: OrcaApiClient, temp_workspace: TempWorkspace
) -> None:
    """F19 T1: Transactional skill install commits atomically with zero staging leftovers."""
    src_dir = temp_workspace.mkdir("skills_src/rust-perf")
    (src_dir / "SKILL.md").write_text("# rust-perf\nProfile Rust hot loops cleanly.\n", encoding="utf-8")
    target_root = temp_workspace.mkdir("skills_installed")

    inst = orca_api.assert_rpc_ok(
        "skills.install",
        {"source_path": str(src_dir), "target_root": str(target_root), "share_enabled": True},
    )
    assert isinstance(inst, dict) and isinstance(inst.get("share_link"), str) and inst["share_link"].startswith("orca://"), inst
    assert any(p.is_file() and p.name == "SKILL.md" for p in target_root.rglob("*")), list(target_root.rglob("*"))
    leftovers = [p for p in target_root.rglob("*") if ".txn" in p.name or ".staging" in p.name]
    assert leftovers == [], leftovers

    listed = orca_api.assert_rpc_ok("skills.list", {"target_root": str(target_root)})
    discovered = orca_api.assert_rpc_ok("skills.discover", {"target_root": str(target_root)})
    assert "rust-perf" in json.dumps(listed) and "rust-perf" in json.dumps(discovered)


@pytest.mark.tier1
@pytest.mark.f19
def test_f19_t1_plugin_v2_manifest_16_contribution_points_and_tier_enumeration(
    orca_api: OrcaApiClient, orca_cli: OrcaCliRunner, temp_workspace: TempWorkspace
) -> None:
    """F19 T1: Plugin V2 validates all 4 PLUGIN_TIERS_4 spanning all 16 contribution points."""
    tier_contribs: dict[str, dict[str, Any]] = {
        "data": {
            "themes": [{"id": "t1", "path": "a.json"}],
            "syntaxes": [{"scope": "source.x", "path": "a.json"}],
            "fonts": [{"family": "OrcaMono", "path": "a.json"}],
            "languagePacks": [{"locale": "en", "path": "a.json"}],
        },
        "declarative": {
            "commands": [{"id": "c1", "title": "C1"}],
            "keybindings": [{"key": "Cmd+Alt+1", "command": "c1"}],
            "menus": [{"slot": "toolbar", "command": "c1"}],
            "statusBar": [{"id": "sb1", "text": "OK"}],
            "vmRecipes": [{"id": "vm1", "image": "debian:12"}],
            "settings": [{"key": "acme.flag", "type": "boolean"}],
        },
        "sandboxed-ui": {
            "panels": [{"id": "p1", "entry": "p.tsx"}],
            "customEditors": [{"id": "e1", "entry": "p.tsx"}],
        },
        "trusted-worker": {
            "mcpServers": [{"name": "tw-mcp", "transport": "stdio", "command": "echo"}],
            "events": [{"name": "workspace.opened"}],
            "actionReplacements": [{"action": "git.stage", "handler": "w.js"}],
            "hooks": [{"event": "agentLaunch", "onError": "pass"}],
        },
    }
    assert tuple(tier_contribs.keys()) == PLUGIN_TIERS_4
    assert sum(len(v) for v in tier_contribs.values()) == 16

    for tier, contributes in tier_contribs.items():
        manifest: dict[str, Any] = {
            "schema_version": 2,
            "publisher": "acme",
            "id": f"enum-{tier}",
            "version": "1.0.0",
            "tier": tier,
            "contributes": contributes,
        }
        if tier == "trusted-worker":
            manifest["capabilities"] = ["storage", "hooks.agentLaunch"]
        p_dir = _write_plugin(temp_workspace, f"enum-{tier}", manifest, {"a.json": "{}", "p.tsx": "", "w.js": ""})
        cli_v1 = orca_cli.run("ide", "plugin-validate", "--path", str(p_dir)).assert_ok()
        rpc_v1 = orca_api.assert_rpc_ok("plugins.validate", {"path": str(p_dir)})
        assert cli_v1.get("valid") is True and rpc_v1.get("valid") is True
        assert cli_v1.get("tier") == tier and rpc_v1.get("tier") == tier
        fp_cli = cli_v1.get("capability_fingerprint") or cli_v1.get("fingerprint")
        fp_rpc = rpc_v1.get("capability_fingerprint") or rpc_v1.get("fingerprint")
        assert fp_cli == fp_rpc and isinstance(fp_cli, str) and len(fp_cli) >= 16
