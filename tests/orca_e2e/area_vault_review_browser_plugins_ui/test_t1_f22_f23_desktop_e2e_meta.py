"""Tier 1 opaque-box E2E tests for F22 (Desktop GPUI UI) and F23 (Suite Meta-Integrity)."""

from __future__ import annotations

import ast
import configparser
import http.server
import json
import os
import re
import subprocess
import sys
import threading
from pathlib import Path
from typing import Any

import pytest

from harness import (
    ALL_EDGE_CASES_40, ALL_FEATURES_24, BOT_CARD_KINDS_10, BotHomeFixture, GitRepoFixture,
    OrcaApiClient, OrcaCliRunner, REPO_ROOT, SUITE_ROOT, TAB_CONTENT_TYPES_10,
    TEMPLATE_CATEGORIES_6, TEMPLATE_IDS_24, TempWorkspace, USAGE_PROVIDERS_9,
)

_RUST_CRATE_PREFIXES = ("crates", "orca_core", "orca_bots", "orca_ide", "gpui_telegram", "bot_browser", "magpie_gateway", "fabushi")
_CARGO_RESULT_RE = re.compile(r"test result:\s+ok\.\s+(\d+)\s+passed;\s+(\d+)\s+failed;")
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


def _read_repo_text(rel: str) -> str:
    target = REPO_ROOT / rel
    assert target.is_file(), f"Missing expected M5 repository artifact: {rel}"
    return target.read_text(encoding="utf-8")


def _orca_view_sources() -> str:
    orca_dir = REPO_ROOT / "crates" / "gpui_telegram_desktop" / "src" / "views" / "orca"
    assert orca_dir.is_dir(), f"Missing M5 Orca desktop views directory: {orca_dir}"
    rs_files = sorted(orca_dir.rglob("*.rs"))
    assert rs_files, f"No Rust view sources found under {orca_dir}"
    return "\n".join(p.read_text(encoding="utf-8") for p in rs_files)


def _run_cargo_test_with_cleanup(cmd: list[str], *, min_passed: int) -> list[tuple[int, int]]:
    env = os.environ.copy()
    env["GPUI_OFFLINE_TEST"] = "1"
    env["PYTHONDONTWRITEBYTECODE"] = "1"
    try:
        proc = subprocess.run(
            cmd, cwd=str(REPO_ROOT), env=env, text=True, capture_output=True, timeout=300
        )
    finally:
        subprocess.run(
            [sys.executable, "-B", str(REPO_ROOT / "scripts" / "clean_build_garbage.py")],
            cwd=str(REPO_ROOT), env=env, text=True, capture_output=True, check=False,
        )
    assert proc.returncode == 0, f"cargo test failed ({proc.returncode}):\n{proc.stdout[-2000:]}\n{proc.stderr[-2000:]}"
    counts = [(int(m.group(1)), int(m.group(2))) for m in _CARGO_RESULT_RE.finditer(proc.stdout)]
    assert counts, f"No cargo test summary lines found:\n{proc.stdout[-2000:]}"
    assert all(f == 0 for _, f in counts), f"Non-zero failures in cargo test: {counts}"
    assert sum(p for p, _ in counts) >= min_passed, f"Passed {counts} < min_passed={min_passed}"
    return counts


def _enum_block(source: str, enum_name: str) -> str:
    m = re.search(rf"pub\s+enum\s+{enum_name}\s*\{{(.*?)\}}", source, re.DOTALL)
    assert m is not None, f"Could not find `pub enum {enum_name}`"
    return m.group(1)


def _extract_id(payload: Any, *keys: str, wrap: str = "") -> str:
    rec = payload[wrap] if wrap and isinstance(payload, dict) and isinstance(payload.get(wrap), dict) else payload
    val = next((rec[k] for k in keys if isinstance(rec, dict) and rec.get(k) not in (None, "") and not isinstance(rec.get(k), bool)), "")
    assert str(val).strip(), f"Missing required ID ({keys}) in payload: {payload!r}"
    return str(val)


@pytest.mark.tier1
@pytest.mark.f22
def test_f22_t1_nav_panel_contracts_and_headless_desktop_test_suite() -> None:
    """F22 T1: NavDestination/ActiveWorkspacePanel Orca variants and headless desktop tests pass."""
    assert "OrcaBots" in _enum_block(_read_repo_text("crates/gpui_telegram_desktop/src/views/nav_rail.rs"), "NavDestination")
    panel_blk = _enum_block(_read_repo_text("crates/gpui_telegram_desktop/src/views/shell.rs"), "ActiveWorkspacePanel")
    assert "OrcaBots" in panel_blk and "OrcaIde" in panel_blk
    assert "pub mod orca;" in _read_repo_text("crates/gpui_telegram_desktop/src/views/mod.rs")
    assert len(_orca_view_sources()) > 100

    tests_dir = REPO_ROOT / "crates" / "gpui_telegram_desktop" / "tests"
    orca_tests = sorted(tests_dir.glob("*orca*.rs"))
    assert orca_tests, f"Missing M5 Orca desktop integration tests under {tests_dir}"
    targets = sorted(["desktop_views_tests", *(f.stem for f in orca_tests)])
    cmd = ["cargo", "test", "-p", "gpui_telegram_desktop", *(a for t in targets for a in ("--test", t))]
    counts = _run_cargo_test_with_cleanup(cmd, min_passed=50 + len(orca_tests))
    assert len(counts) == 1 + len(orca_tests) and counts[targets.index("desktop_views_tests")][0] >= 50 and all(p >= 1 for p, _ in counts), (targets, counts)


@pytest.mark.tier1
@pytest.mark.f22
def test_f22_t1_bots_surface_home_tabs_templates_cards_and_needs_you(orca_api: OrcaApiClient) -> None:
    """F22 T1: Bots surface backing routes and GPUI views cover tabs, templates, cards & Needs You."""
    tpl_blob = json.dumps(orca_api.assert_ok(orca_api.get("/api/v1/orca/bots/templates")))
    for tid in TEMPLATE_IDS_24:
        assert tid in tpl_blob, f"Missing template {tid}"
    for cat in TEMPLATE_CATEGORIES_6:
        assert cat in tpl_blob, f"Missing template category {cat}"

    created = orca_api.assert_ok(orca_api.post("/api/v1/orca/bots/from-template", json_body={"templateId": TEMPLATE_IDS_24[0]}))
    bot_id = _extract_id(created, "bot_id", "botId", "id", wrap="bot")
    ask = {"questions": [{"prompt": "Ship?", "options": [{"label": "Yes"}, {"label": "Later"}], "recommended": 0, "multi": False}]}
    orca_api.assert_ok(orca_api.post(f"/api/v1/orca/bots/{bot_id}/items", json_body={"title": "Confirm deploy", "status": "needs_you", "ask": ask}))
    assert bot_id in json.dumps(orca_api.assert_ok(orca_api.get("/api/v1/orca/needs-you")))
    assert isinstance(orca_api.assert_ok(orca_api.get(f"/api/v1/orca/bots/{bot_id}/chat-tree")), (dict, list))
    assert isinstance(orca_api.assert_ok(orca_api.get(f"/api/v1/orca/bots/{bot_id}/up-next")), (dict, list))

    norm_src = _orca_view_sources().lower().replace(" ", "").replace("_", "")
    for token in ("Chat", "Items", "UpNext", "Settings", "NeedsYou", "Template", "GroupRoom"):
        assert token.lower() in norm_src, f"Missing view token {token}"
    for kind in BOT_CARD_KINDS_10:
        assert kind.lower() in norm_src, f"Missing card {kind}"


@pytest.mark.tier1
@pytest.mark.f22
def test_f22_t1_split_anything_layout_ten_tab_types_and_pinned_boundaries(orca_api: OrcaApiClient) -> None:
    """F22 T1: Split Anything layout persists recursive tree with all 10 tab types & pinned boundary."""
    tabs = [{"id": f"tab-{i}", "kind": k, "pinned": i == 0} for i, k in enumerate(TAB_CONTENT_TYPES_10)]
    tree = {
        "workspace_id": "ws-layout-1",
        "root": {
            "orientation": "horizontal", "ratio": 0.42, "pinned": True,
            "first": {"orientation": "vertical", "ratio": 0.5, "first": {"tabs": tabs[:3]}, "second": {"tabs": tabs[3:6]}},
            "second": {"tabs": tabs[6:]},
        },
    }
    orca_api.assert_rpc_ok("layout.apply", tree)
    got_blob = json.dumps(orca_api.assert_rpc_ok("layout.get", {"workspace_id": "ws-layout-1"}))
    for tab_kind in TAB_CONTENT_TYPES_10:
        assert tab_kind in got_blob, f"Missing tab kind {tab_kind} in layout.get"
    assert "0.42" in got_blob and '"pinned": true' in got_blob.lower()
    norm_src = _orca_view_sources().lower().replace("_", "").replace("-", "")
    for tab_kind in TAB_CONTENT_TYPES_10:
        assert tab_kind.replace("-", "") in norm_src, f"Missing tab kind {tab_kind} in Orca views"


@pytest.mark.tier1
@pytest.mark.f22
def test_f22_t1_cmd_j_scoped_jump_palette_and_cmd_shift_j_chat_terminal_flip(orca_api: OrcaApiClient, temp_workspace: TempWorkspace) -> None:
    """F22 T1: Cmd+J opens Jump Palette and Cmd+Shift+J flips Chat<->Terminal without losing state."""
    shortcuts = _read_repo_text("crates/gpui_telegram_desktop/src/views/app_shell/shell_shortcuts.rs")
    m = re.search(r'"j"(?=\s*(?:if\b|\||=>))(.*?)"n"\s*=>', shortcuts, re.DOTALL)
    assert m is not None and "is_shift" in m.group(1), "Cmd+J arm must branch on is_shift"
    assert "ModalDialogKind::Contacts" not in m.group(1), "Cmd+J arm must not open Contacts modal"

    src = _orca_view_sources().lower().replace("_", "")
    assert "jumppalette" in src and ("chatterminal" in src or ("terminal" in src and "chat" in src))
    sess = orca_api.assert_rpc_ok("agentSession.create", {"agent": "claude", "workspacePath": str(temp_workspace.path), "viewMode": "chat"})
    sess_id = _extract_id(sess, "sessionId", "id")
    ranked = orca_api.assert_rpc_ok("jumpPalette.list", {"candidates": [{"id": "s-idle", "status": "idle"}, {"id": "s-done", "status": "done"}, {"id": sess_id, "status": "needs_you"}]})
    items = ranked.get("items") if isinstance(ranked, dict) else ranked
    assert isinstance(items, list) and len(items) <= 6 and [i["id"] for i in items[:3]] == [sess_id, "s-done", "s-idle"]
    orca_api.assert_rpc_ok("agentSession.saveDraft", {"sessionId": sess_id, "text": "keep me", "attachments": []})
    flipped = orca_api.assert_rpc_ok("nativeChat.toggleViewMode", {"sessionId": sess_id, "viewMode": "terminal"})
    assert isinstance(flipped, dict) and _extract_id(flipped, "sessionId", "id") == sess_id and flipped.get("viewMode") == "terminal", flipped
    draft = orca_api.assert_rpc_ok("agentSession.getDraft", {"sessionId": sess_id})
    assert isinstance(draft, dict) and draft.get("text") == "keep me", draft


@pytest.mark.tier1
@pytest.mark.f22
def test_f22_t1_kanban_diff_annotate_usage_popover_and_handover_surfaces(orca_api: OrcaApiClient, git_repo: GitRepoFixture) -> None:
    """F22 T1: Kanban, diff-annotations, 9-provider usage popover, and hand-over surfaces work."""
    run = orca_api.assert_ok(orca_api.post("/api/v1/orca/orchestration/runs", json_body={"title": "M5 UI Kanban Run"}))
    run_id = _extract_id(run, "run_id", "runId", "id", wrap="run")
    orca_api.assert_ok(orca_api.post("/api/v1/orca/orchestration/tasks", json_body={"runId": run_id, "title": "Render Kanban", "deps": []}))
    assert run_id in json.dumps(orca_api.assert_ok(orca_api.get("/api/v1/orca/orchestration/runs")))
    assert isinstance(orca_api.assert_ok(orca_api.get("/api/v1/orca/ide/diff-annotations")), (dict, list))

    usage_blob = json.dumps(orca_api.assert_ok(orca_api.get("/api/v1/orca/ide/usage")))
    for prov in USAGE_PROVIDERS_9:
        assert prov in usage_blob, f"Missing usage provider {prov}"

    git_repo.commit_file("README.md", "# Handover repo\n", msg="seed")
    git_repo.create_branch("fix-relay-reconnect", checkout=True)
    bot = orca_api.assert_ok(orca_api.post("/api/v1/orca/bots/from-template", json_body={"templateId": TEMPLATE_IDS_24[0]}))
    bot_id = _extract_id(bot, "bot_id", "botId", "id", wrap="bot")
    ho = orca_api.assert_ok(orca_api.post(f"/api/v1/orca/bots/{bot_id}/hand-over", json_body={"workspacePath": str(git_repo.path), "branch": "fix-relay-reconnect", "prompt": "Take over PR fix"}))
    assert isinstance(ho, dict) and ho.get("moved") is True and ho.get("notified") is True, ho
    norm_src = _orca_view_sources().lower().replace("_", "")
    for token in ("kanban", "diff", "usage", "handover"):
        assert token in norm_src, f"Missing GPUI view surface for {token}"


@pytest.mark.tier1
@pytest.mark.f22
def test_f22_t1_browser_design_mode_capture_and_desktop_overlay() -> None:
    """F22 T1: bot-browser DesignModeCapture, desktop browser overlay, and cargo tests pass."""
    dm_rs = _read_repo_text("crates/bot-browser/src/design_mode.rs")
    assert "pub struct DesignModeCapture" in dm_rs
    lib_rs = _read_repo_text("crates/bot-browser/src/lib.rs")
    assert "pub mod design_mode;" in lib_rs and "DesignModeCapture" in lib_rs
    assert len(_read_repo_text("crates/gpui_telegram_desktop/src/views/browser/design_mode.rs")) > 20
    assert "pub mod design_mode;" in _read_repo_text("crates/gpui_telegram_desktop/src/views/browser/mod.rs")
    _run_cargo_test_with_cleanup(["cargo", "test", "-p", "bot-browser", "design_mode"], min_passed=1)


def _attr_chain(node: ast.AST) -> list[str] | None:
    parts: list[str] = []
    cur: ast.AST = node
    while isinstance(cur, ast.Attribute):
        parts.append(cur.attr)
        cur = cur.value
    return list(reversed([*parts, cur.id])) if isinstance(cur, ast.Name) else None


def _audit_imports(py_path: Path, source: str, allowed_roots: set[str]) -> list[str]:
    tree = ast.parse(source, filename=str(py_path))
    violations: list[str] = []
    allowed = set(sys.stdlib_module_names) | allowed_roots
    mods: list[tuple[int, str]] = []
    for node in ast.walk(tree):
        if isinstance(node, ast.Import):
            mods.extend((node.lineno, a.name) for a in node.names)
        elif isinstance(node, ast.ImportFrom) and node.level == 0 and node.module:
            mods.append((node.lineno, node.module))
        elif isinstance(node, ast.Call):
            chain = _attr_chain(node.func)
            is_dyn = (chain == ["importlib", "import_module"]) or (isinstance(node.func, ast.Name) and node.func.id == "__import__")
            if is_dyn and node.args and isinstance(node.args[0], ast.Constant) and isinstance(node.args[0].value, str):
                mods.append((node.lineno, node.args[0].value))
    for lineno, mod in mods:
        top = mod.split(".")[0]
        if any(top == p or top.startswith(p + "_") for p in _RUST_CRATE_PREFIXES):
            violations.append(f"{py_path}:{lineno}: forbidden Rust/crates import {mod!r}")
        elif top not in allowed:
            violations.append(f"{py_path}:{lineno}: forbidden non-stdlib import {mod!r}")
    return violations


def _audit_forbidden_escapes(py_path: Path, source: str) -> list[str]:
    tree = ast.parse(source, filename=str(py_path))
    nodes = list(ast.walk(tree))
    pt_aliases = {"pytest"} | {a.asname or "pytest" for n in nodes if isinstance(n, ast.Import) for a in n.names if a.name == "pytest"}
    ut_aliases = {"unittest"} | {a.asname or "unittest" for n in nodes if isinstance(n, ast.Import) for a in n.names if a.name == "unittest"}
    violations: list[str] = []
    for node in nodes:
        if isinstance(node, ast.ImportFrom) and node.module and node.module.split(".")[0] in ("pytest", "_pytest", "unittest"):
            for a in node.names:
                if a.name in ("skip", "xfail", "importorskip", "SkipTest", "Skipped", "XFailed"):
                    violations.append(f"{py_path}:{node.lineno}: forbidden import {a.name} from {node.module}")
        elif isinstance(node, ast.Attribute):
            ch = _attr_chain(node)
            if ch and ch[0] in pt_aliases and (ch[1:] in (["skip"], ["xfail"], ["importorskip"], ["mark", "skip"], ["mark", "skipif"], ["mark", "xfail"])):
                violations.append(f"{py_path}:{node.lineno}: forbidden escape {'.'.join(ch)}")
            elif ch and ch[0] in ut_aliases and len(ch) >= 2 and (ch[1] == "SkipTest" or ch[1].startswith("skip")):
                violations.append(f"{py_path}:{node.lineno}: forbidden unittest escape {'.'.join(ch)}")
        elif isinstance(node, ast.Raise) and node.exc is not None:
            exc = node.exc.func if isinstance(node.exc, ast.Call) else node.exc
            if isinstance(exc, ast.Name) and exc.id in ("SkipTest", "Skipped", "XFailed"):
                violations.append(f"{py_path}:{node.lineno}: forbidden raise {exc.id}")
        elif isinstance(node, (ast.FunctionDef, ast.AsyncFunctionDef)) and node.name.startswith("test_"):
            body = list(node.body)
            if body and isinstance(body[0], ast.Expr) and isinstance(body[0].value, ast.Constant) and isinstance(body[0].value.value, str):
                body = body[1:]
            if not body or all(
                isinstance(s, ast.Pass)
                or (isinstance(s, ast.Expr) and isinstance(s.value, ast.Constant) and s.value.value is Ellipsis)
                or (isinstance(s, ast.Return) and s.value is None)
                or (isinstance(s, ast.Assert) and isinstance(s.test, ast.Constant) and s.test.value is True)
                for s in body
            ):
                violations.append(f"{py_path}:{node.lineno}: empty/vacuous test body {node.name}")
    return violations


@pytest.mark.tier1
@pytest.mark.f23
def test_f23_t1_test_infra_doc_and_pytest_ini_contract_structure() -> None:
    """F23 T1: TEST_INFRA.md sections/thresholds and pytest.ini strict-markers/30 markers exist."""
    doc = _read_repo_text("TEST_INFRA.md")
    for token in (
        "## 1.", "## 2.", "## 3.", "## 4.", "## 5.", "## 6.",
        "tests/orca_e2e/", "/opt/pipx/bin/pytest", "PYTHONDONTWRITEBYTECODE=1",
        "-p no:cacheprovider --strict-markers", "area_bots_memory_items",
        "area_connectors_relay_groups", "area_orchestration_worktrees_pty",
        "area_vault_review_browser_plugins_ui", "tier3_pairwise", "tier4_scenarios",
        "120", "24", "12", "276",
    ):
        assert token in doc, f"Missing required TEST_INFRA.md contract token: {token!r}"

    cfg = configparser.ConfigParser()
    cfg.read(SUITE_ROOT / "pytest.ini", encoding="utf-8")
    assert cfg.has_section("pytest")
    addopts = cfg["pytest"]["addopts"]
    assert "-p no:cacheprovider" in addopts and "--strict-markers" in addopts
    assert cfg["pytest"]["python_files"].strip() == "test_*.py"
    marker_names = {ln.split(":", 1)[0].split("(", 1)[0].strip() for ln in cfg["pytest"]["markers"].splitlines() if ln.strip()}
    expected = {"tier1", "tier2", "tier3", "tier4", "harness_self_check", "edge_case", *ALL_FEATURES_24}
    assert expected.issubset(marker_names) and len(marker_names) >= 30, marker_names


@pytest.mark.tier1
@pytest.mark.f23
def test_f23_t1_run_e2e_tier_filtering_exit_codes_and_zero_cache(temp_workspace: TempWorkspace) -> None:
    """F23 T1: run_e2e.py filters tiers 1..4/all/self cleanly with zero __pycache__/.pytest_cache."""
    synth = temp_workspace.write(
        "test_synth_tiers.py",
        "import pytest\n"
        "@pytest.mark.tier1\n@pytest.mark.f23\ndef test_t1(): assert 1 == 1\n"
        "@pytest.mark.tier2\n@pytest.mark.f23\ndef test_t2(): assert 2 == 2\n"
        "@pytest.mark.tier3\n@pytest.mark.f23\ndef test_t3(): assert 3 == 3\n"
        "@pytest.mark.tier4\n@pytest.mark.f23\ndef test_t4(): assert 4 == 4\n"
        "@pytest.mark.harness_self_check\ndef test_self(): assert 5 == 5\n",
    )
    clean_env = {k: v for k, v in os.environ.items() if k != "PYTHONDONTWRITEBYTECODE"}
    for tier, expected_n in (("1", 1), ("2", 1), ("3", 1), ("4", 1), ("all", 4), ("self", 1)):
        proc = subprocess.run(
            [sys.executable, "-B", str(SUITE_ROOT / "run_e2e.py"), "--tier", tier, str(synth)],
            env=clean_env, text=True, capture_output=True, timeout=60,
        )
        assert proc.returncode == 0, f"tier={tier} rc={proc.returncode}\n{proc.stdout}\n{proc.stderr}"
        assert f"{expected_n} passed" in proc.stdout, (tier, proc.stdout)

    assert not list(temp_workspace.path.rglob("__pycache__"))
    assert not list(temp_workspace.path.rglob("*.pyc"))
    assert not list(temp_workspace.path.rglob(".pytest_cache"))
    assert not (SUITE_ROOT / ".pytest_cache").exists() and not (REPO_ROOT / ".pytest_cache").exists()


@pytest.mark.tier1
@pytest.mark.f23
def test_f23_t1_ast_zero_crates_or_third_party_imports_in_suite() -> None:
    """F23 T1: Pure-AST audit proves zero crates/ or third-party imports across tests/orca_e2e."""
    py_files = sorted(SUITE_ROOT.rglob("*.py"))
    assert len(py_files) >= 10, f"Expected >= 10 suite python files, found {len(py_files)}"
    local_roots = {"pytest", "harness", "tests", "run_e2e", "conftest", "__future__"}
    local_roots.update(p.name for p in SUITE_ROOT.iterdir() if p.is_dir() and not p.name.startswith("."))
    violations: list[str] = []
    for py_path in py_files:
        sibling_roots = {s.stem for s in py_path.parent.glob("*.py")}
        violations.extend(_audit_imports(py_path, py_path.read_text(encoding="utf-8"), local_roots | sibling_roots))
    assert violations == [], "Import audit violations:\n" + "\n".join(violations)


@pytest.mark.tier1
@pytest.mark.f23
def test_f23_t1_ast_zero_skip_xfail_or_empty_tests_in_suite() -> None:
    """F23 T1: Pure-AST audit proves zero skip/xfail escapes or empty test bodies in suite."""
    py_files = sorted(SUITE_ROOT.rglob("*.py"))
    assert len(py_files) >= 10
    violations: list[str] = []
    for py_path in py_files:
        violations.extend(_audit_forbidden_escapes(py_path, py_path.read_text(encoding="utf-8")))
    assert violations == [], "Forbidden escape/empty test violations:\n" + "\n".join(violations)


@pytest.mark.tier1
@pytest.mark.f23
def test_f23_t1_raii_temp_cleanup_and_live_404_stub_fail_closed() -> None:
    """F23 T1: RAII fixtures delete temp dirs on exit and live 404 stub fails closed."""
    with TempWorkspace() as ws:
        ws_p = ws.path
        assert ws_p.is_dir()
    assert not ws_p.exists()
    with GitRepoFixture() as repo:
        repo_p = repo.path
        assert (repo_p / ".git").exists()
    assert not repo_p.exists()
    with BotHomeFixture() as bh:
        bh_p = bh.root
        assert bh_p.is_dir()
    assert not bh_p.exists()
    cli = OrcaCliRunner(cli_bin=None, server_url="http://127.0.0.1:1")
    cli_p = cli.work_dir
    assert cli_p.is_dir()
    cli.cleanup()
    assert not cli_p.exists()

    class _Stub404(http.server.BaseHTTPRequestHandler):
        def _reply(self) -> None:
            raw = b'{"ok":false,"error":{"code":"not_found","message":"missing"}}'
            self.send_response(404)
            self.send_header("Content-Type", "application/json")
            self.send_header("Content-Length", str(len(raw)))
            self.end_headers()
            self.wfile.write(raw)

        do_GET = do_POST = _reply
        def log_message(self, format: str, *args: Any) -> None:
            return

    srv = http.server.ThreadingHTTPServer(("127.0.0.1", 0), _Stub404)
    th = threading.Thread(target=srv.serve_forever, daemon=True)
    th.start()
    try:
        api = OrcaApiClient(f"http://127.0.0.1:{srv.server_address[1]}")
        with pytest.raises(AssertionError, match="404"):
            api.get("/api/v1/orca/bots").assert_status(200)
        with pytest.raises(AssertionError, match="404"):
            api.assert_ok(api.get("/api/v1/orca/bots"))
        with pytest.raises(AssertionError, match="404"):
            api.assert_rpc_ok("bots.list")
    finally:
        srv.shutdown()
        srv.server_close()


@pytest.mark.tier1
@pytest.mark.f23
def test_f23_t1_full_suite_tier_thresholds_features_and_edge_cases() -> None:
    """F23 T1: Full suite collection meets TEST_INFRA.md tier, feature, and 40-edge-case minimums."""
    env = {**os.environ, "PYTHONDONTWRITEBYTECODE": "1"}
    proc = subprocess.run(
        [sys.executable, "-B", "-c", _COLLECTOR_SCRIPT, str(SUITE_ROOT / "pytest.ini"), str(SUITE_ROOT)],
        env=env, text=True, capture_output=True, timeout=60,
    )
    marker = "__ORCA_INVENTORY__"
    inv_lines = [ln for ln in proc.stdout.splitlines() if marker in ln]
    assert inv_lines, f"Collection script produced no inventory:\n{proc.stdout}\n{proc.stderr}"
    inv = json.loads(inv_lines[-1].split(marker, 1)[1])
    assert inv["rc"] == 0, f"Suite collection exited {inv['rc']}:\n{proc.stdout}\n{proc.stderr}"
    items = inv["items"]
    t_cnt = {t: sum(t in it["markers"] for it in items) for t in ("tier1", "tier2", "tier3", "tier4", "harness_self_check")}
    t1_feat = {f: sum("tier1" in it["markers"] and f in it["markers"] for it in items) for f in ALL_FEATURES_24}
    t2_feat = {f: sum("tier2" in it["markers"] and f in it["markers"] for it in items) for f in ALL_FEATURES_24}
    ecs = {e for it in items for e in it["edge_cases"]}
    total_1_4 = t_cnt["tier1"] + t_cnt["tier2"] + t_cnt["tier3"] + t_cnt["tier4"]
    assert t_cnt["tier1"] >= 120 and t_cnt["tier2"] >= 120, f"Tier counts below min: {t_cnt}"
    assert t_cnt["tier3"] >= 24 and t_cnt["tier4"] >= 12 and total_1_4 >= 276, f"Tier counts: {t_cnt}"
    assert t_cnt["harness_self_check"] >= 12, f"harness_self_check < 12: {t_cnt}"
    assert all(c >= 5 for c in t1_feat.values()), f"Features <5 T1: { {k: v for k, v in t1_feat.items() if v < 5} }"
    assert all(c >= 5 for c in t2_feat.values()), f"Features <5 T2: { {k: v for k, v in t2_feat.items() if v < 5} }"
    missing_ecs = sorted(set(ALL_EDGE_CASES_40) - ecs)
    assert not missing_ecs, f"Missing edge cases ({len(missing_ecs)}): {missing_ecs}"
