"""Tier 2 boundary, negative, security & meta-integrity E2E tests for F22 and F23."""

from __future__ import annotations

import ast
import base64
import http.server
import json
import os
import re
import stat
import struct
import subprocess
import sys
import threading
import zlib
from pathlib import Path
from typing import Any

import pytest

from harness import (
    ALL_EDGE_CASES_40, ALL_FEATURES_24, BotHomeFixture, CLOSED_TAB_TOMBSTONE_DAYS,
    GitRepoFixture, OrcaApiClient, OrcaCliRunner, OrcaServerHandle, REPO_ROOT,
    SUITE_ROOT, TAB_CONTENT_TYPES_10, TEMPLATE_IDS_24, TempWorkspace,
)

_RUST_CRATE_PREFIXES = ("crates", "orca_core", "orca_bots", "orca_ide", "gpui_telegram", "bot_browser", "magpie_gateway", "fabushi")
_COLLECTOR_SCRIPT = (
    "import json, sys, pytest\n"
    "class _P:\n"
    "    def __init__(self): self.items = []\n"
    "    def pytest_collection_modifyitems(self, items):\n"
    "        for it in items:\n"
    "            ms = sorted({m.name for m in it.iter_markers()})\n"
    "            ecs = [str(m.args[0]) for m in it.iter_markers('edge_case') if m.args]\n"
    "            self.items.append({'nodeid': it.nodeid, 'markers': ms, 'edge_cases': ecs})\n"
    "p = _P()\n"
    "rc = int(pytest.main(['-c', sys.argv[1], '--collect-only', '-q', sys.argv[2]], plugins=[p]))\n"
    "print('__ORCA_INVENTORY__' + json.dumps({'rc': rc, 'items': p.items}))\n"
)


def _rpc_err(api: OrcaApiClient, method: str, expected_code: str, params: dict[str, Any] | None = None) -> dict[str, Any]:
    resp = api.rpc(method, params=params)
    assert resp.status not in (404, 501), f"Missing/unimplemented RPC ({resp.status}): {resp.text[:300]}"
    return api.assert_rpc_error(resp, expected_code)


def _rpc_err_in(api: OrcaApiClient, method: str, allowed_codes: tuple[str, ...], params: dict[str, Any] | None = None) -> dict[str, Any]:
    resp = api.rpc(method, params=params)
    assert resp.status not in (404, 501), f"Missing/unimplemented RPC ({resp.status}): {resp.text[:300]}"
    payload = resp.json()
    assert isinstance(payload, dict) and payload.get("ok") is False, payload
    err = payload.get("error")
    assert isinstance(err, dict) and err.get("code") in allowed_codes, (allowed_codes, payload)
    return err


def _read_repo_text(rel: str) -> str:
    target = REPO_ROOT / rel
    assert target.is_file(), f"Missing expected M5 repository artifact: {rel}"
    return target.read_text(encoding="utf-8")


def _orca_view_sources() -> str:
    orca_dir = REPO_ROOT / "crates" / "gpui_telegram_desktop" / "src" / "views" / "orca"
    assert orca_dir.is_dir(), f"Missing M5 Orca desktop views directory: {orca_dir}"
    rs_files = sorted(orca_dir.rglob("*.rs"))
    assert rs_files, f"No Rust view sources under {orca_dir}"
    return "\n".join(p.read_text(encoding="utf-8") for p in rs_files)


def _enum_block(source: str, enum_name: str) -> str:
    m = re.search(rf"pub\s+enum\s+{enum_name}\s*\{{(.*?)\}}", source, re.DOTALL)
    assert m is not None, f"Missing `pub enum {enum_name}`"
    return m.group(1)


def _minimal_png_b64() -> str:
    def _chunk(tag: bytes, data: bytes) -> bytes:
        return struct.pack("!I", len(data)) + tag + data + struct.pack("!I", zlib.crc32(tag + data) & 0xFFFFFFFF)

    ihdr = struct.pack("!IIBBBBB", 1, 1, 8, 6, 0, 0, 0)
    idat = zlib.compress(b"\x00\x00\x00\x00\xff")
    return base64.b64encode(b"\x89PNG\r\n\x1a\n" + _chunk(b"IHDR", ihdr) + _chunk(b"IDAT", idat) + _chunk(b"IEND", b"")).decode("ascii")


def _extract_id(payload: Any, *keys: str, wrap: str = "") -> str:
    rec = payload[wrap] if wrap and isinstance(payload, dict) and isinstance(payload.get(wrap), dict) else payload
    val = next((rec[k] for k in keys if isinstance(rec, dict) and rec.get(k) not in (None, "") and not isinstance(rec.get(k), bool)), "")
    assert str(val).strip(), f"Missing required ID ({keys}) in payload: {payload!r}"
    return str(val)


@pytest.mark.tier2
@pytest.mark.f22
def test_f22_t2_security_surface_desktop_view_immune_to_plugin_theme_restyling(orca_api: OrcaApiClient) -> None:
    """F22 T2: SecuritySurface tokens resist plugin restyling on RPC and GPUI desktop views."""
    res = orca_api.assert_rpc_ok("plugins.resolveThemeTokens", {"surface": "security", "plugin_overrides": {"securitySurface.bg": "#ff00ff"}})
    assert isinstance(res, dict) and res.get("security_surface_protected") is True, res
    tokens = res.get("tokens") if "tokens" in res else res.get("resolved_tokens")
    assert isinstance(tokens, dict) and len(tokens) > 0 and "#ff00ff" not in json.dumps(res).lower(), res
    assert "SecuritySurface" in _orca_view_sources(), "Desktop Orca views must define non-themable SecuritySurface"


@pytest.mark.tier2
@pytest.mark.f22
def test_f22_t2_split_layout_rejects_invalid_tab_ratio_pinned_close_and_tombstone_replay(orca_api: OrcaApiClient) -> None:
    """F22 T2: Split layout rejects invalid tab/ratio, unconfirmed pinned close & tombstone replay."""
    assert "unknown-pane-kind" not in TAB_CONTENT_TYPES_10
    _rpc_err(orca_api, "layout.apply", "invalid_argument", {"workspace_id": "ws-bad", "root": {"tabs": [{"id": "t1", "kind": "unknown-pane-kind"}]}})
    for bad_ratio in (0.0, 1.0, -0.5, 1.5):
        _rpc_err(orca_api, "layout.apply", "invalid_argument", {"workspace_id": "ws-bad", "root": {"orientation": "horizontal", "ratio": bad_ratio, "first": {"tabs": []}, "second": {"tabs": []}}})
    orca_api.assert_rpc_ok("layout.apply", {"workspace_id": "ws-pin", "root": {"tabs": [{"id": "tab-pin-1", "kind": "terminal", "pinned": True}]}})
    _rpc_err_in(orca_api, "layout.closeTab", ("pinned_tab_protected", "invalid_argument"), {"workspace_id": "ws-pin", "tab_id": "tab-pin-1", "confirm_pinned": False})
    closed = orca_api.assert_rpc_ok("layout.closeTab", {"workspace_id": "ws-pin", "tab_id": "tab-pin-1", "confirm_pinned": True})
    assert isinstance(closed, dict) and closed.get("tombstone_ttl_days") is not None and int(closed["tombstone_ttl_days"]) == CLOSED_TAB_TOMBSTONE_DAYS, closed
    _rpc_err(orca_api, "terminal.create", "tab_not_found", {"tabId": "tab-pin-1", "cols": 80, "rows": 24})


@pytest.mark.tier2
@pytest.mark.f22
def test_f22_t2_cmd_j_jump_palette_bounds_and_contacts_regression_guard(orca_api: OrcaApiClient, temp_workspace: TempWorkspace) -> None:
    """F22 T2: Cmd+J guards Contacts regression, caps recents <=6 omitting active idle, rejects bad flip."""
    shortcuts = _read_repo_text("crates/gpui_telegram_desktop/src/views/app_shell/shell_shortcuts.rs")
    m = re.search(r'"j"(?=\s*(?:if\b|\||=>))(.*?)"n"\s*=>', shortcuts, re.DOTALL)
    assert m is not None and "is_shift" in m.group(1) and "ModalDialogKind::Contacts" not in m.group(1)
    cands = [{"id": "s-active-idle", "status": "idle"}] + [{"id": f"s-{i}", "status": "done"} for i in range(8)]
    res = orca_api.assert_rpc_ok("jumpPalette.list", {"query": "", "active_id": "s-active-idle", "candidates": cands})
    items = res.get("items") if isinstance(res, dict) else res
    assert isinstance(items, list) and 1 <= len(items) <= 6 and all(isinstance(i, dict) and bool(i.get("id")) and i["id"] != "s-active-idle" for i in items), items
    sess = orca_api.assert_rpc_ok("agentSession.create", {"agent": "claude", "workspacePath": str(temp_workspace.path), "viewMode": "chat"})
    sid = _extract_id(sess, "sessionId", "id")
    ok_flip = orca_api.assert_rpc_ok("nativeChat.toggleViewMode", {"sessionId": sid, "viewMode": "terminal"})
    assert isinstance(ok_flip, dict) and ok_flip.get("viewMode") == "terminal", ok_flip
    _rpc_err_in(orca_api, "nativeChat.toggleViewMode", ("not_found", "tab_not_found", "invalid_argument"), {"sessionId": "sess-does-not-exist", "viewMode": "terminal"})


@pytest.mark.tier2
@pytest.mark.f22
def test_f22_t2_needs_you_inbox_cap_keeps_approvals_unhidden_and_handover_conflict(orca_api: OrcaApiClient, git_repo: GitRepoFixture) -> None:
    """F22 T2: Needs You caps non-approvals at 5 keeping approvals visible; closed ask & stale handover fail."""
    bot = orca_api.assert_ok(orca_api.post("/api/v1/orca/bots/from-template", json_body={"templateId": TEMPLATE_IDS_24[0]}))
    bot_id = _extract_id(bot, "bot_id", "botId", "id", wrap="bot")
    ask = {"questions": [{"prompt": "Ship?", "options": [{"label": "Yes"}, {"label": "Later"}], "recommended": 0, "multi": False}]}
    first_item: Any = None
    for i in range(7):
        cr = orca_api.assert_ok(orca_api.post(f"/api/v1/orca/bots/{bot_id}/items", json_body={"kind": "ask", "title": f"Ask {i}", "status": "needs_you", "priority": "normal", "ask": ask}))
        if i == 0:
            first_item = cr
    item_num = _extract_id(first_item, "number", "item_num", "id", wrap="item")
    for j in range(2):
        orca_api.assert_ok(orca_api.post(f"/api/v1/orca/bots/{bot_id}/items", json_body={"kind": "approval", "title": f"Approve {j}", "status": "needs_you", "priority": "approval", "ask": ask}))
    inbox = orca_api.assert_ok(orca_api.get("/api/v1/orca/needs-you"))
    visible = (inbox["visible_items"] if isinstance(inbox.get("visible_items"), list) else inbox.get("items")) if isinstance(inbox, dict) else inbox
    assert isinstance(visible, list), inbox
    approvals = [x for x in visible if isinstance(x, dict) and (x.get("kind") == "approval" or x.get("priority") == "approval")]
    non_approvals = [x for x in visible if x not in approvals]
    assert len(approvals) >= 2 and 1 <= len(non_approvals) <= 5, (len(approvals), len(non_approvals))

    orca_api.assert_ok(orca_api.patch(f"/api/v1/orca/items/{item_num}", json_body={"status": "canceled", "actor": {"kind": "owner"}, "close_note": "Closed"}))
    reopen = orca_api.patch(f"/api/v1/orca/items/{item_num}", json_body={"status": "working", "actor": {"kind": "bot", "bot_id": bot_id}})
    assert reopen.status in (400, 403, 409, 412, 422), f"Unexpected status {reopen.status}: {reopen.text[:200]}"
    assert reopen.extract_error_code() in ("bot_offer_already_answered", "question_closed", "item_closed"), reopen.text[:200]
    git_repo.commit_file("README.md", "# Handover conflict repo\n", msg="seed")
    ho = orca_api.post(
        f"/api/v1/orca/bots/{bot_id}/hand-over",
        json_body={"workspacePath": str(git_repo.path), "originalParent": "ws-parent-orig", "simulateNotifyFailure": True, "concurrentParent": "ws-parent-user-new"},
    )
    assert ho.status in (200, 409, 502), f"Unexpected status {ho.status}: {ho.text[:200]}"
    ho_data = ho.json()
    assert isinstance(ho_data, dict) and ho_data.get("placement") == "moved-since" and (ho_data.get("currentParent") or ho_data.get("current_parent")) == "ws-parent-user-new", ho_data
    assert ho_data.get("reason") == "lineage_parent_changed" or ho.extract_error_code() == "lineage_parent_changed", ho_data


@pytest.mark.tier2
@pytest.mark.f22
def test_f22_t2_design_mode_desktop_boundary_missing_sourcemap_and_bad_crop(orca_api: OrcaApiClient) -> None:
    """F22 T2: Design Mode omits missing source_map cleanly and rejects empty selector or <=0 crop."""
    ok = orca_api.assert_rpc_ok(
        "browser.designMode.capture",
        {"tab_id": "tab-dm-ui", "selector": "section.hero", "outer_html": '<section class="hero">Hi</section>', "computed_styles": {"padding": "12px"}, "crop": {"x": 0, "y": 0, "width": 80, "height": 40}, "screenshot_png_b64": _minimal_png_b64(), "source_map": None},
    )
    assert isinstance(ok, dict) and ok.get("source_map") is None and isinstance(ok.get("outer_html"), str) and "hero" in ok["outer_html"] and bool(ok.get("computed_styles")), ok
    _rpc_err(orca_api, "browser.designMode.capture", "invalid_argument", {"tab_id": "tab-dm-ui", "selector": "", "outer_html": "<div>x</div>", "crop": {"x": 0, "y": 0, "width": 10, "height": 10}})
    _rpc_err(orca_api, "browser.designMode.capture", "invalid_argument", {"tab_id": "tab-dm-ui", "selector": "div", "outer_html": "<div>x</div>", "crop": {"x": 0, "y": 0, "width": 0, "height": -5}})


@pytest.mark.tier2
@pytest.mark.f22
def test_f22_t2_tdesktop_ledger_enum_and_module_preservation_with_orca_additions() -> None:
    """F22 T2: Preserves all baseline tdesktop enums/bot-browser modules while requiring Orca additions."""
    nav_blk = _enum_block(_read_repo_text("crates/gpui_telegram_desktop/src/views/nav_rail.rs"), "NavDestination")
    for v in ("Chats", "Folders", "Calls", "TaskCenter", "McpTools", "TalkMode", "RemoteComputer", "Browser", "SavedMessages", "Settings", "OrcaBots"):
        assert re.search(rf"\b{v}\b", nav_blk), f"Missing NavDestination::{v}"

    panel_blk = _enum_block(_read_repo_text("crates/gpui_telegram_desktop/src/views/shell.rs"), "ActiveWorkspacePanel")
    for v in ("Chats", "Calls", "TaskCenter", "McpTools", "Browser", "Settings", "OrcaBots", "OrcaIde"):
        assert re.search(rf"\b{v}\b", panel_blk), f"Missing ActiveWorkspacePanel::{v}"

    sem = _read_repo_text("crates/gpui_telegram_desktop/src/views/app_shell/semantic_tree.rs")
    for arm in ("ActiveWorkspacePanel::OrcaBots", "ActiveWorkspacePanel::OrcaIde", "NavDestination::OrcaBots"):
        assert arm in sem, f"Missing semantic_tree arm {arm}"

    bb_lib = _read_repo_text("crates/bot-browser/src/lib.rs")
    for mod in ("backend", "cdp", "e2ee_sync", "orchestrator", "script_app", "session_import", "sidecar", "totp", "types", "vault", "design_mode"):
        assert f"pub mod {mod};" in bb_lib, f"Missing bot-browser module pub mod {mod};"


def _attr_chain(node: ast.AST) -> list[str] | None:
    parts: list[str] = []
    cur: ast.AST = node
    while isinstance(cur, ast.Attribute):
        parts.append(cur.attr)
        cur = cur.value
    return list(reversed([*parts, cur.id])) if isinstance(cur, ast.Name) else None


def _audit_imports(py_path: Path, source: str, allowed_roots: set[str]) -> list[str]:
    tree = ast.parse(source, filename=str(py_path))
    allowed = set(sys.stdlib_module_names) | allowed_roots
    mods: list[tuple[int, str]] = []
    for n in ast.walk(tree):
        if isinstance(n, ast.Import):
            mods.extend((n.lineno, a.name) for a in n.names)
        elif isinstance(n, ast.ImportFrom) and n.level == 0 and n.module:
            mods.append((n.lineno, n.module))
        elif isinstance(n, ast.Call):
            ch = _attr_chain(n.func)
            is_dyn = (ch == ["importlib", "import_module"]) or (isinstance(n.func, ast.Name) and n.func.id == "__import__")
            if is_dyn and n.args and isinstance(n.args[0], ast.Constant) and isinstance(n.args[0].value, str):
                mods.append((n.lineno, n.args[0].value))
    out: list[str] = []
    for ln, m in mods:
        top = m.split(".")[0]
        if any(top == p or top.startswith(p + "_") for p in _RUST_CRATE_PREFIXES) or top not in allowed:
            out.append(f"{py_path}:{ln}:{m}")
    return out


def _audit_forbidden_escapes(py_path: Path, source: str) -> list[str]:
    nodes = list(ast.walk(ast.parse(source, filename=str(py_path))))
    pt = {"pytest"} | {a.asname or "pytest" for n in nodes if isinstance(n, ast.Import) for a in n.names if a.name == "pytest"}
    ut = {"unittest"} | {a.asname or "unittest" for n in nodes if isinstance(n, ast.Import) for a in n.names if a.name == "unittest"}
    out: list[str] = []
    for n in nodes:
        if isinstance(n, ast.ImportFrom) and n.module and n.module.split(".")[0] in ("pytest", "_pytest", "unittest"):
            out.extend(f"{py_path}:{n.lineno}:{a.name}" for a in n.names if a.name in ("skip", "xfail", "importorskip", "SkipTest", "Skipped", "XFailed"))
        elif isinstance(n, ast.Attribute):
            ch = _attr_chain(n)
            if ch and ((ch[0] in pt and ch[1:] in (["skip"], ["xfail"], ["importorskip"], ["mark", "skip"], ["mark", "skipif"], ["mark", "xfail"])) or (ch[0] in ut and len(ch) >= 2 and (ch[1] == "SkipTest" or ch[1].startswith("skip")))):
                out.append(f"{py_path}:{n.lineno}:{'.'.join(ch)}")
        elif isinstance(n, ast.Raise) and n.exc is not None:
            exc = n.exc.func if isinstance(n.exc, ast.Call) else n.exc
            if isinstance(exc, ast.Name) and exc.id in ("SkipTest", "Skipped", "XFailed"):
                out.append(f"{py_path}:{n.lineno}:raise:{exc.id}")
        elif isinstance(n, (ast.FunctionDef, ast.AsyncFunctionDef)) and n.name.startswith("test_"):
            body = n.body[1:] if n.body and isinstance(n.body[0], ast.Expr) and isinstance(n.body[0].value, ast.Constant) and isinstance(n.body[0].value.value, str) else list(n.body)
            if not body or all(isinstance(s, ast.Pass) or (isinstance(s, ast.Expr) and isinstance(s.value, ast.Constant) and s.value.value is Ellipsis) or (isinstance(s, ast.Return) and s.value is None) or (isinstance(s, ast.Assert) and isinstance(s.test, ast.Constant) and s.test.value is True) for s in body):
                out.append(f"{py_path}:{n.lineno}:empty:{n.name}")
    return out


@pytest.mark.tier2
@pytest.mark.f23
def test_f23_t2_ast_escape_auditor_catches_evasions_and_allows_string_literals(
    temp_workspace: TempWorkspace,
) -> None:
    """F23 T2: Pure-AST escape auditor catches all 11 evasion patterns and ignores string literals."""
    evasions = (
        "import pytest\ndef test_a(): pytest.skip('no')\n",
        "import pytest as pt\ndef test_b(): pt.xfail('no')\n",
        "import pytest\n@pytest.mark.skip\ndef test_c(): assert 1\n",
        "import pytest\n@pytest.mark.skipif(True, reason='x')\ndef test_d(): assert 1\n",
        "import pytest\n@pytest.mark.xfail\ndef test_e(): assert 1\n",
        "from pytest import skip\ndef test_f(): skip('no')\n",
        "import pytest\ndef test_g(): pytest.importorskip('missing')\n",
        "import pytest\n@pytest.mark.parametrize('x', [pytest.param(1, marks=pytest.mark.xfail)])\ndef test_h(x): assert x\n",
        "import unittest\ndef test_i(): raise unittest.SkipTest('no')\n",
        "def test_j():\n    '''docstring'''\n    pass\n",
        "def test_k():\n    '''docstring'''\n    assert True\n",
    )
    for idx, src in enumerate(evasions):
        p = temp_workspace.write(f"evasion_{idx}.py", src)
        assert _audit_forbidden_escapes(p, src), f"Auditor missed evasion #{idx}:\n{src}"

    clean = '"""NEVER pytest.skip, xfail, or assert True."""\nMSG = "pytest.mark.skipif unittest.SkipTest"\ndef test_ok(): assert len(MSG) > 0\n'
    clean_p = temp_workspace.write("clean_literals.py", clean)
    assert _audit_forbidden_escapes(clean_p, clean) == []


@pytest.mark.tier2
@pytest.mark.f23
def test_f23_t2_ast_import_auditor_catches_crates_and_third_party(
    temp_workspace: TempWorkspace,
) -> None:
    """F23 T2: Pure-AST import auditor catches crates/*, Rust crate names, and 3rd-party imports."""
    allowed = {"pytest", "harness", "tests", "run_e2e", "conftest", "__future__"}
    bad_mods = (
        "import crates.orca_core\n",
        "from crates import gpui_telegram_server\n",
        "import orca_bots\n",
        "import requests\n",
        'import importlib\nimportlib.import_module("crates.orca_ide")\n',
    )
    for idx, src in enumerate(bad_mods):
        p = temp_workspace.write(f"bad_imp_{idx}.py", src)
        assert _audit_imports(p, src, allowed), f"Import auditor missed case #{idx}: {src!r}"

    clean = 'import json\nimport pytest\nfrom harness import SUITE_ROOT\nDOC = "crates/orca_core"\n'
    assert _audit_imports(temp_workspace.write("clean_imp.py", clean), clean, allowed) == []


@pytest.mark.tier2
@pytest.mark.f23
def test_f23_t2_run_e2e_negative_exit_codes_and_strict_marker_rejection(
    temp_workspace: TempWorkspace,
) -> None:
    """F23 T2: run_e2e.py returns exact non-zero codes (1/5/2/strict-marker) with zero cache."""
    runner = [sys.executable, "-B", str(SUITE_ROOT / "run_e2e.py")]
    clean_env = {k: v for k, v in os.environ.items() if k != "PYTHONDONTWRITEBYTECODE"}

    f_fail = temp_workspace.write("test_neg_fail.py", "import pytest\n@pytest.mark.tier1\n@pytest.mark.f23\ndef test_boom(): assert 1 == 2\n")
    assert subprocess.run([*runner, "--tier", "1", str(f_fail)], env=clean_env, capture_output=True, text=True, timeout=60).returncode == 1
    assert subprocess.run([*runner, "--tier", "3", str(f_fail)], env=clean_env, capture_output=True, text=True, timeout=60).returncode == 5
    assert subprocess.run([*runner, "--tier", "bogus"], env=clean_env, capture_output=True, text=True, timeout=60).returncode == 2

    f_bad_mark = temp_workspace.write("test_neg_mark.py", "import pytest\n@pytest.mark.f99_unknown\ndef test_u(): assert 1 == 1\n")
    rc_mark = subprocess.run([*runner, "--tier", "all", str(f_bad_mark)], env=clean_env, capture_output=True, text=True, timeout=60).returncode
    assert rc_mark != 0 and rc_mark in (2, 4), f"Expected strict-markers rejection, got rc={rc_mark}"
    assert not list(temp_workspace.path.rglob("__pycache__")) and not list(temp_workspace.path.rglob(".pytest_cache"))


@pytest.mark.tier2
@pytest.mark.f23
def test_f23_t2_suite_marker_exclusivity_and_valid_edge_case_ids() -> None:
    """F23 T2: Every suite test has exactly 1 tier/meta marker, valid feature, and canonical edge_case ID."""
    env = {**os.environ, "PYTHONDONTWRITEBYTECODE": "1"}
    proc = subprocess.run(
        [sys.executable, "-B", "-c", _COLLECTOR_SCRIPT, str(SUITE_ROOT / "pytest.ini"), str(SUITE_ROOT)],
        env=env, text=True, capture_output=True, timeout=60,
    )
    inv_lines = [ln for ln in proc.stdout.splitlines() if "__ORCA_INVENTORY__" in ln]
    assert inv_lines, f"No inventory output:\n{proc.stdout}\n{proc.stderr}"
    inv = json.loads(inv_lines[-1].split("__ORCA_INVENTORY__", 1)[1])
    assert inv["rc"] == 0 and len(inv["items"]) >= 20, inv
    tier_set, feat_set, ec_set = {"tier1", "tier2", "tier3", "tier4", "harness_self_check"}, set(ALL_FEATURES_24), set(ALL_EDGE_CASES_40)
    for it in inv["items"]:
        ms = set(it["markers"])
        tiers = ms & tier_set
        assert len(tiers) == 1, f"Item must have exactly 1 tier/meta marker: {it}"
        if "harness_self_check" not in tiers:
            assert ms & feat_set, f"Tier 1-4 item missing feature marker: {it}"
        for ec in it["edge_cases"]:
            assert ec in ec_set, f"Invalid edge_case ID {ec!r} on {it['nodeid']}"


@pytest.mark.tier2
@pytest.mark.f23
def test_f23_t2_missing_binaries_and_non_json_501_endpoints_fail_closed(
    temp_workspace: TempWorkspace,
) -> None:
    """F23 T2: Missing server/CLI binaries and HTTP 404 HTML / 501 JSON / 200 ok=false all fail closed."""
    missing = temp_workspace.resolve("no_such_binary")
    srv_handle = OrcaServerHandle(server_bin=missing, startup_timeout=2.0)
    srv_dir = srv_handle.data_dir
    with pytest.raises(AssertionError):
        with srv_handle:
            pass
    assert not srv_dir.exists()

    cli = OrcaCliRunner(cli_bin=missing, server_url="http://127.0.0.1:1", work_dir=temp_workspace.mkdir("cli_miss"))
    with pytest.raises(AssertionError):
        cli.run("ide", "profile-state")

    class _Stub(http.server.BaseHTTPRequestHandler):
        def _reply(self) -> None:
            if self.path == "/html404":
                code, ct, body = 404, "text/html", b"<html>Not Found</html>"
            elif self.path == "/rpc501":
                code, ct, body = 501, "application/json", b'{"ok":false,"error":{"code":"not_found"}}'
            else:
                code, ct, body = 200, "application/json", b'{"ok":false,"error":{"code":"invalid_argument"}}'
            self.send_response(code)
            self.send_header("Content-Type", ct)
            self.send_header("Content-Length", str(len(body)))
            self.end_headers()
            self.wfile.write(body)

        do_GET = do_POST = _reply
        def log_message(self, format: str, *args: Any) -> None:
            return

    httpd = http.server.ThreadingHTTPServer(("127.0.0.1", 0), _Stub)
    threading.Thread(target=httpd.serve_forever, daemon=True).start()
    try:
        api = OrcaApiClient(f"http://127.0.0.1:{httpd.server_address[1]}")
        r404 = api.get("/html404")
        with pytest.raises(AssertionError):
            r404.json()
        with pytest.raises(AssertionError):
            api.assert_ok(r404)
        r501 = api.post("/rpc501", json_body={})
        with pytest.raises(AssertionError, match="501"):
            api.assert_rpc_error(r501, "not_found")
        r200 = api.post("/rpc200err", json_body={})
        with pytest.raises(AssertionError):
            api.assert_ok(r200)
        with pytest.raises(AssertionError):
            api.assert_rpc_ok(r200)
    finally:
        httpd.shutdown()
        httpd.server_close()


@pytest.mark.tier2
@pytest.mark.f23
def test_f23_t2_raii_cleanup_under_exceptions_and_zero_repo_leaks() -> None:
    """F23 T2: RAII fixtures clean up under exceptions/crashes and SUITE_ROOT has zero leaked artifacts."""
    paths: list[Path] = []
    for cls, attr in ((TempWorkspace, "path"), (GitRepoFixture, "path"), (BotHomeFixture, "root")):
        with pytest.raises(RuntimeError, match="boom"):
            with cls() as inst:
                p = getattr(inst, attr)
                assert p.is_dir()
                paths.append(p)
                raise RuntimeError("boom")
    assert len(paths) == 3 and all(not p.exists() for p in paths), [str(p) for p in paths if p.exists()]

    with TempWorkspace() as ws:
        crash_bin = ws.write("crash_srv.sh", "#!/bin/sh\nexit 37\n")
        crash_bin.chmod(crash_bin.stat().st_mode | stat.S_IXUSR)
        handle = OrcaServerHandle(server_bin=crash_bin, startup_timeout=3.0)
        h_dir = handle.data_dir
        with pytest.raises(AssertionError):
            with handle:
                pass
        assert not h_dir.exists()

    assert not list(SUITE_ROOT.rglob(".pytest_cache"))
    assert not [p for p in SUITE_ROOT.rglob("orca_e2e_*") if p.is_dir()]
    assert not [p for p in SUITE_ROOT.rglob("*.db")] and not [p for p in SUITE_ROOT.rglob("*.db-wal")]
