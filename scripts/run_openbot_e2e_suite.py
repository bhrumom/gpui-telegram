#!/usr/bin/env python3
"""OpenBot 1:1 Pure-Rust + GPUI Fusion 4-Tier Opaque-Box E2E Suite Runner & AST Self-Auditor.

Supports:
  --tier {1,2,3,4} (repeatable & comma-separated)
  --baseline / --all
  --self-audit [--self-audit-features OB-F01..OB-F10]
  --feature <OB-Fxx> / --filter <substr_or_regex>
  --bin <gpui-telegram-cli>
  --json-out / --json-report <path.json>
"""

from __future__ import annotations

import argparse
import ast
import fcntl
import hashlib
import importlib
import json
import os
import py_compile
import re
import subprocess
import sys
import tempfile
import time
import traceback
from pathlib import Path
from typing import Any, Dict, List, Optional, Sequence, Set, Tuple

REPO_ROOT = Path(__file__).resolve().parents[1]
if str(REPO_ROOT) not in sys.path:
    sys.path.insert(0, str(REPO_ROOT))

from tests.e2e_openbot.harness import (  # noqa: E402
    ALL_OPENBOT_FEATURES,
    TEST_REGISTRY,
    BinaryNotReadyError,
    E2ETestCase,
    resolve_cli_bin,
    set_cli_bin_override,
)

TEST_INFRA_IMMUTABLE_BYTES = 152875
TEST_INFRA_IMMUTABLE_SHA256 = (
    "daa96e4165085964ccd552ca1d6213166e70d7a5bd43f4d3f37dc573711ce8ff"
)

BOUNDARY_CALL_NAMES: Set[str] = {
    "run_openbot_cli",
    "run_cli",
    "run_cli_json",
    "run_cli_raw",
    "run_cli_expect_error",
    "run_cli_expect_rejection",
    "start_server",
    "http_team",
    "http_automation",
    "automation_run",
    "open_sse",
}

ASSERT_HELPER_NAMES: Set[str] = {
    "require_field",
    "extract_list",
    "run_cli_expect_error",
    "run_cli_expect_rejection",
    "check_sqlite_integrity",
    "assert_no_rust_panic",
}

SKIP_NAMES: Set[str] = {"skip", "xfail", "SkipTest", "skipIf", "skipUnless"}


def parse_feature_spec(specs: Optional[Sequence[str]]) -> List[str]:
    if not specs:
        return []
    result: List[str] = []
    for raw in specs:
        for token in raw.split(","):
            tok = token.strip().upper()
            if not tok:
                continue
            m_range = re.fullmatch(r"OB-F(\d{1,2})\.\.OB-F(\d{1,2})", tok)
            if m_range:
                lo, hi = int(m_range.group(1)), int(m_range.group(2))
                if lo > hi or lo < 1 or hi > 20:
                    raise ValueError(f"Invalid feature range: {tok}")
                for idx in range(lo, hi + 1):
                    fid = f"OB-F{idx:02d}"
                    if fid not in result:
                        result.append(fid)
            else:
                if tok not in ALL_OPENBOT_FEATURES:
                    raise ValueError(f"Unknown feature ID: {tok}")
                if tok not in result:
                    result.append(tok)
    return result


def parse_tiers(raw_tiers: Optional[Sequence[str]], run_all: bool) -> List[int]:
    if run_all:
        return [1, 2, 3, 4]
    if not raw_tiers:
        return []
    tiers: List[int] = []
    for item in raw_tiers:
        for part in item.split(","):
            p = part.strip()
            if not p:
                continue
            val = int(p)
            if val not in (1, 2, 3, 4):
                raise ValueError(f"Invalid tier {val}; expected 1, 2, 3, or 4")
            if val not in tiers:
                tiers.append(val)
    return sorted(tiers)


def discover_and_import_tier_modules() -> List[Path]:
    pkg_dir = REPO_ROOT / "tests" / "e2e_openbot"
    tier_files = sorted(pkg_dir.glob("tier*.py"))
    for mod_file in tier_files:
        mod_name = f"tests.e2e_openbot.{mod_file.stem}"
        if mod_name not in sys.modules:
            importlib.import_module(mod_name)
    return tier_files


def _call_name(node: ast.Call) -> Optional[str]:
    if isinstance(node.func, ast.Name):
        return node.func.id
    if isinstance(node.func, ast.Attribute):
        return node.func.attr
    return None


def _decorator_name(dec: ast.expr) -> str:
    if isinstance(dec, ast.Call):
        return _decorator_name(dec.func)
    if isinstance(dec, ast.Name):
        return dec.id
    if isinstance(dec, ast.Attribute):
        return dec.attr
    return ""


def _is_trivial_assert(node: ast.Assert) -> Optional[str]:
    t = node.test
    if isinstance(t, ast.Constant):
        return f"constant assert ({t.value!r}) at L{node.lineno}"
    if isinstance(t, ast.UnaryOp) and isinstance(t.op, ast.Not) and isinstance(t.operand, ast.Constant):
        return f"trivial unary constant assert at L{node.lineno}"
    if isinstance(t, ast.Compare) and len(t.comparators) == 1:
        if ast.dump(t.left) == ast.dump(t.comparators[0]):
            return f"reflexive comparison assert at L{node.lineno}"
    return None


def _analyze_tier_ast(tier_file: Path) -> Tuple[List[str], Dict[str, Dict[str, Any]]]:
    """Run AST checks SA-3..SA-6 on a single tier*.py file."""
    errors: List[str] = []
    source = tier_file.read_text(encoding="utf-8")
    tree = ast.parse(source, filename=str(tier_file))
    rel = tier_file.relative_to(REPO_ROOT)

    for node in ast.walk(tree):
        if isinstance(node, ast.Constant) and isinstance(node.value, str):
            s = node.value.strip()
            if s == "/tmp" or s.startswith("/tmp/"):
                errors.append(
                    f"[SA-6] {rel}:L{getattr(node, 'lineno', 0)} hardcoded /tmp literal {node.value!r}"
                )
        if isinstance(node, ast.Call):
            cname = _call_name(node)
            if cname in SKIP_NAMES:
                errors.append(f"[SA-3] {rel}:L{node.lineno} forbidden skip/xfail call {cname!r}")

    funcs: Dict[str, ast.FunctionDef] = {}
    decorated_tests: List[ast.FunctionDef] = []
    for stmt in tree.body:
        if isinstance(stmt, (ast.FunctionDef, ast.AsyncFunctionDef)):
            if isinstance(stmt, ast.AsyncFunctionDef):
                errors.append(f"[SA-3] {rel}:L{stmt.lineno} async test {stmt.name!r} unsupported")
                continue
            funcs[stmt.name] = stmt
            dec_names = [_decorator_name(d) for d in stmt.decorator_list]
            for dname in dec_names:
                if dname in SKIP_NAMES:
                    errors.append(f"[SA-3] {rel}:L{stmt.lineno} forbidden decorator @{dname}")
            has_e2e = "e2e_test" in dec_names
            if stmt.name.startswith("test_") and not has_e2e:
                errors.append(
                    f"[SA-3] {rel}:L{stmt.lineno} function {stmt.name!r} missing @e2e_test"
                )
            if has_e2e:
                decorated_tests.append(stmt)

    local_stats: Dict[str, Dict[str, Any]] = {}
    for fn_name, fn_node in funcs.items():
        non_doc_stmts = [
            s
            for s in fn_node.body
            if not (
                isinstance(s, ast.Expr)
                and isinstance(s.value, ast.Constant)
                and isinstance(s.value.value, str)
            )
            and not isinstance(s, ast.Pass)
        ]
        direct_asserts = 0
        calls: Set[str] = set()
        uses_with_env = False
        for sub in ast.walk(fn_node):
            if isinstance(sub, ast.Assert):
                triv = _is_trivial_assert(sub)
                if triv:
                    errors.append(f"[SA-4] {rel}:{fn_name} {triv}")
                else:
                    direct_asserts += 1
            elif isinstance(sub, ast.Call):
                cname = _call_name(sub)
                if cname:
                    calls.add(cname)
                    if cname in ASSERT_HELPER_NAMES:
                        direct_asserts += 1
            elif isinstance(sub, ast.With):
                for item in sub.items:
                    if isinstance(item.context_expr, ast.Call):
                        wname = _call_name(item.context_expr)
                        if wname in ("TempOpenBotEnv", "OpenBotE2EEnv"):
                            uses_with_env = True
        local_stats[fn_name] = {
            "non_empty": len(non_doc_stmts) > 0,
            "direct_asserts": direct_asserts,
            "calls": calls,
            "uses_with_env": uses_with_env,
            "arg_count": len(fn_node.args.args),
            "lineno": fn_node.lineno,
        }

    fn_metrics: Dict[str, Dict[str, Any]] = {}
    for t_fn in decorated_tests:
        name = t_fn.name
        st = local_stats[name]
        if not st["non_empty"]:
            errors.append(f"[SA-4] {rel}:L{st['lineno']} {name!r} has empty/pass body")
        visited: Set[str] = {name}
        frontier: List[Tuple[str, int]] = [(name, 0)]
        total_asserts = st["direct_asserts"]
        all_calls: Set[str] = set(st["calls"])
        has_env_raii = (st["arg_count"] >= 1) or st["uses_with_env"]
        while frontier:
            cur, depth = frontier.pop(0)
            if depth >= 3:
                continue
            for callee in local_stats[cur]["calls"]:
                if callee in local_stats and callee not in visited:
                    visited.add(callee)
                    c_st = local_stats[callee]
                    total_asserts += c_st["direct_asserts"]
                    all_calls.update(c_st["calls"])
                    has_env_raii = has_env_raii or c_st["uses_with_env"]
                    frontier.append((callee, depth + 1))

        boundary_hits = sorted(all_calls & BOUNDARY_CALL_NAMES)
        if total_asserts < 2:
            errors.append(
                f"[SA-4] {rel}:L{st['lineno']} {name!r} has only {total_asserts} real assertions (< 2)"
            )
        if not boundary_hits:
            errors.append(
                f"[SA-5] {rel}:L{st['lineno']} {name!r} makes zero opaque-box CLI/HTTP boundary calls"
            )
        if not has_env_raii:
            errors.append(
                f"[SA-6] {rel}:L{st['lineno']} {name!r} lacks RAII TempOpenBotEnv isolation"
            )
        fn_metrics[f"tests.e2e_openbot.{tier_file.stem}.{name}"] = {
            "assert_count": total_asserts,
            "boundary_calls": boundary_hits,
            "raii_isolated": has_env_raii,
        }

    return errors, fn_metrics


def run_self_audit(target_features: List[str]) -> Tuple[bool, Dict[str, Any]]:
    errors: List[str] = []
    pkg_dir = REPO_ROOT / "tests" / "e2e_openbot"
    py_targets = [Path(__file__).resolve(), *sorted(pkg_dir.glob("*.py"))]

    # Gate SA-1: py_compile + dynamic import
    with tempfile.TemporaryDirectory(prefix="openbot_pyc_") as pyc_dir:
        for idx_py, py_file in enumerate(py_targets):
            try:
                cfile = str(Path(pyc_dir) / f"{idx_py}_{py_file.stem}.pyc")
                py_compile.compile(str(py_file), cfile=cfile, doraise=True)
            except py_compile.PyCompileError as exc:
                errors.append(f"[SA-1] py_compile failed for {py_file}: {exc}")
    try:
        tier_files = discover_and_import_tier_modules()
    except Exception as exc:
        errors.append(f"[SA-1] Failed importing tier modules: {exc}")
        tier_files = []
    if not tier_files:
        errors.append("[SA-1] Zero tests/e2e_openbot/tier*.py modules found")

    # Gate SA-2: TEST_INFRA.md byte-exact prefix + Section 11
    infra_path = REPO_ROOT / "TEST_INFRA.md"
    if not infra_path.exists():
        errors.append("[SA-2] TEST_INFRA.md missing")
    else:
        raw_infra = infra_path.read_bytes()
        if len(raw_infra) <= TEST_INFRA_IMMUTABLE_BYTES:
            errors.append(
                f"[SA-2] TEST_INFRA.md byte length {len(raw_infra)} <= {TEST_INFRA_IMMUTABLE_BYTES}"
            )
        else:
            prefix_sha = hashlib.sha256(raw_infra[:TEST_INFRA_IMMUTABLE_BYTES]).hexdigest()
            if prefix_sha != TEST_INFRA_IMMUTABLE_SHA256:
                errors.append(
                    f"[SA-2] TEST_INFRA.md prefix SHA-256 mismatch: {prefix_sha} != {TEST_INFRA_IMMUTABLE_SHA256}"
                )
            appended_text = raw_infra[TEST_INFRA_IMMUTABLE_BYTES:].decode("utf-8", errors="replace")
            if not re.search(r"^## 11\.\s+OpenBot", appended_text, re.MULTILINE):
                errors.append("[SA-2] TEST_INFRA.md missing appended `## 11. OpenBot ...` section")

    # Gates SA-3..SA-6
    all_fn_metrics: Dict[str, Dict[str, Any]] = {}
    for tf in tier_files:
        f_errs, f_metrics = _analyze_tier_ast(tf)
        errors.extend(f_errs)
        all_fn_metrics.update(f_metrics)

    # Gate SA-7: Coverage quantitative thresholds
    scope_features = target_features if target_features else list(ALL_OPENBOT_FEATURES)
    per_feature_counts: Dict[str, Dict[str, int]] = {
        f: {"tier1": 0, "tier2": 0, "tier3": 0, "tier4": 0} for f in scope_features
    }
    tier_totals = {1: 0, 2: 0, 3: 0, 4: 0}
    scoped_tier_totals = {1: 0, 2: 0, 3: 0, 4: 0}
    for tc in TEST_REGISTRY.values():
        tier_totals[tc.tier] += 1
        if any(f in per_feature_counts for f in tc.features):
            scoped_tier_totals[tc.tier] += 1
        for f in tc.features:
            if f in per_feature_counts:
                per_feature_counts[f][f"tier{tc.tier}"] += 1
        if tc.tier == 3 and len(tc.features) < 2:
            errors.append(f"[SA-7] Tier 3 test {tc.id} tags < 2 features: {tc.features}")

    for f in scope_features:
        if per_feature_counts[f]["tier1"] < 5:
            errors.append(
                f"[SA-7] Feature {f} has {per_feature_counts[f]['tier1']} Tier 1 tests (< 5)"
            )
        if per_feature_counts[f]["tier2"] < 5:
            errors.append(
                f"[SA-7] Feature {f} has {per_feature_counts[f]['tier2']} Tier 2 tests (< 5)"
            )

    if target_features:
        n_feat = len(scope_features)
        if scoped_tier_totals[1] < 5 * n_feat:
            errors.append(f"[SA-7] Scoped Tier 1 count {scoped_tier_totals[1]} < {5 * n_feat}")
        if scoped_tier_totals[2] < 5 * n_feat:
            errors.append(f"[SA-7] Scoped Tier 2 count {scoped_tier_totals[2]} < {5 * n_feat}")
    else:
        if tier_totals[1] < 100:
            errors.append(f"[SA-7] Total Tier 1 count {tier_totals[1]} < 100")
        if tier_totals[2] < 100:
            errors.append(f"[SA-7] Total Tier 2 count {tier_totals[2]} < 100")
        if tier_totals[3] < 20:
            errors.append(f"[SA-7] Total Tier 3 count {tier_totals[3]} < 20")
        if tier_totals[4] < 10:
            errors.append(f"[SA-7] Total Tier 4 count {tier_totals[4]} < 10")
        if len(TEST_REGISTRY) < 230:
            errors.append(f"[SA-7] Grand total registered tests {len(TEST_REGISTRY)} < 230")

    summary = {
        "passed": len(errors) == 0,
        "scoped_features": scope_features,
        "total_registered": len(TEST_REGISTRY),
        "tier_totals": tier_totals,
        "scoped_tier_totals": scoped_tier_totals,
        "per_feature_counts": per_feature_counts,
        "ast_audited_functions": len(all_fn_metrics),
        "errors": errors,
    }
    return len(errors) == 0, summary


def _resolve_cargo_lock_path() -> Path:
    shared_dir = Path(
        "/home/antigravity/.gemini/jetski/scratch/teamwork_tmp/38332c60ec9b9dd889cdbc56a32459e6"
    )
    if shared_dir.exists() and shared_dir.is_dir():
        return shared_dir / "cargo_build.lock"
    return Path(tempfile.gettempdir()) / "gpui_telegram_cargo_build.lock"


def run_baseline_gate(verbose: bool = False) -> Tuple[bool, Dict[str, Any]]:
    steps: List[Dict[str, Any]] = []
    upstream_env = os.environ.get("TDESKTOP_SOURCE_ROOT", "")
    default_upstream = Path("/Users/bhrum/.gemini/antigravity/scratch/tdesktop/Telegram/SourceFiles")
    has_upstream = bool(
        (upstream_env and Path(upstream_env).exists()) or default_upstream.exists()
    )

    lock_path = _resolve_cargo_lock_path()
    lock_path.parent.mkdir(parents=True, exist_ok=True)
    dist_bin = (
        REPO_ROOT
        / "dist"
        / "GPUI Telegram.app"
        / "Contents"
        / "MacOS"
        / "gpui_telegram_desktop"
    )
    release_bin = REPO_ROOT / "target" / "release" / "gpui_telegram_desktop"
    desktop_targets = (
        "desktop_views_tests",
        "challenger_m1_adversarial_tests",
        "challenger_m2_adversarial_tests",
    )
    all_ok = True
    with lock_path.open("a+", encoding="utf-8") as lock_fh:
        fcntl.flock(lock_fh.fileno(), fcntl.LOCK_EX)
        try:
            if not dist_bin.is_file() or dist_bin.stat().st_size == 0:
                if not release_bin.is_file() or release_bin.stat().st_size == 0:
                    build_cmd = [
                        "cargo",
                        "build",
                        "--release",
                        "--bin",
                        "gpui_telegram_desktop",
                    ]
                    tb = time.monotonic()
                    proc_build = subprocess.run(
                        build_cmd,
                        cwd=str(REPO_ROOT),
                        text=True,
                        capture_output=not verbose,
                    )
                    steps.append(
                        {
                            "step": "cargo_build_release:gpui_telegram_desktop",
                            "cmd": build_cmd,
                            "returncode": proc_build.returncode,
                            "duration_sec": round(time.monotonic() - tb, 3),
                        }
                    )
                    if proc_build.returncode != 0:
                        all_ok = False
                if all_ok:
                    try:
                        os.chmod(release_bin, 0o755)
                    except OSError:
                        pass
                    pkg_cmd = [
                        sys.executable,
                        str(REPO_ROOT / "scripts" / "package_release.py"),
                        "--skip-clean",
                    ]
                    tp = time.monotonic()
                    proc_pkg = subprocess.run(
                        pkg_cmd,
                        cwd=str(REPO_ROOT),
                        text=True,
                        capture_output=not verbose,
                    )
                    steps.append(
                        {
                            "step": "package_release_stage_dist",
                            "cmd": pkg_cmd,
                            "returncode": proc_pkg.returncode,
                            "duration_sec": round(time.monotonic() - tp, 3),
                        }
                    )
                    if proc_pkg.returncode != 0:
                        all_ok = False

            if all_ok:
                ledger_cmd = [
                    sys.executable,
                    str(REPO_ROOT / "scripts" / "verify_ledger_closure.py"),
                ]
                if not has_upstream:
                    ledger_cmd.extend(["--ci-without-upstream", "--skip-cargo"])
                t0 = time.monotonic()
                proc_ledger = subprocess.run(
                    ledger_cmd,
                    cwd=str(REPO_ROOT),
                    text=True,
                    capture_output=not verbose,
                )
                steps.append(
                    {
                        "step": "verify_ledger_closure",
                        "cmd": ledger_cmd,
                        "returncode": proc_ledger.returncode,
                        "duration_sec": round(time.monotonic() - t0, 3),
                    }
                )
                if proc_ledger.returncode != 0:
                    all_ok = False

            if all_ok:
                for target_name in desktop_targets:
                    cmd = [
                        "cargo",
                        "test",
                        "--package",
                        "gpui_telegram_desktop",
                        "--test",
                        target_name,
                    ]
                    ts = time.monotonic()
                    proc = subprocess.run(
                        cmd, cwd=str(REPO_ROOT), text=True, capture_output=not verbose
                    )
                    steps.append(
                        {
                            "step": f"cargo_test:{target_name}",
                            "cmd": cmd,
                            "returncode": proc.returncode,
                            "duration_sec": round(time.monotonic() - ts, 3),
                        }
                    )
                    if proc.returncode != 0:
                        all_ok = False
                        break
        finally:
            clean_cmd = [sys.executable, str(REPO_ROOT / "scripts" / "clean_build_garbage.py")]
            subprocess.run(clean_cmd, cwd=str(REPO_ROOT), text=True, capture_output=not verbose)
            fcntl.flock(lock_fh.fileno(), fcntl.LOCK_UN)

    return all_ok, {"passed": all_ok, "steps": steps}


def select_tests(
    tiers: Sequence[int],
    features: Sequence[str],
    pattern: Optional[str],
) -> List[E2ETestCase]:
    tier_set = set(tiers)
    feat_set = set(features)
    rx = re.compile(pattern, re.IGNORECASE) if pattern else None
    matched: List[E2ETestCase] = []
    for tc in sorted(TEST_REGISTRY.values(), key=lambda c: (c.tier, c.id)):
        if tier_set and tc.tier not in tier_set:
            continue
        if feat_set and not any(f in feat_set for f in tc.features):
            continue
        if rx is not None:
            hay = f"{tc.id} {' '.join(tc.features)} {tc.description} {tc.qualname}"
            if not rx.search(hay):
                continue
        matched.append(tc)
    return matched


def execute_tests(
    cases: Sequence[E2ETestCase], verbose: bool = False
) -> Tuple[bool, Dict[str, Any]]:
    results: List[Dict[str, Any]] = []
    passed = 0
    failed = 0
    for idx, tc in enumerate(cases, 1):
        t0 = time.monotonic()
        try:
            tc.fn()
            dur = round(time.monotonic() - t0, 4)
            passed += 1
            results.append(
                {
                    "id": tc.id,
                    "tier": tc.tier,
                    "features": list(tc.features),
                    "status": "passed",
                    "duration_sec": dur,
                }
            )
            if verbose:
                print(f"  [PASS] ({idx}/{len(cases)}) {tc.id} [{','.join(tc.features)}] ({dur:.3f}s)")
        except BinaryNotReadyError:
            raise
        except Exception as exc:
            dur = round(time.monotonic() - t0, 4)
            failed += 1
            tb_short = "".join(traceback.format_exception_only(type(exc), exc)).strip()
            results.append(
                {
                    "id": tc.id,
                    "tier": tc.tier,
                    "features": list(tc.features),
                    "status": "failed",
                    "duration_sec": dur,
                    "error": tb_short[:1000],
                }
            )
            print(f"  [FAIL] ({idx}/{len(cases)}) {tc.id}: {tb_short}")
            if verbose:
                traceback.print_exc()

    summary = {
        "selected": len(cases),
        "passed": passed,
        "failed": failed,
        "results": results,
    }
    return failed == 0 and len(cases) > 0, summary


def _write_report(path_str: Optional[str], report: Dict[str, Any]) -> None:
    if not path_str:
        return
    out_path = Path(path_str)
    out_path.parent.mkdir(parents=True, exist_ok=True)
    out_path.write_text(json.dumps(report, indent=2) + "\n", encoding="utf-8")


def main(argv: Optional[Sequence[str]] = None) -> int:
    parser = argparse.ArgumentParser(description="OpenBot 4-Tier Opaque-Box E2E Runner")
    parser.add_argument("--tier", action="append", help="Tier(s) 1..4 (repeatable or comma-split)")
    parser.add_argument("--baseline", action="store_true", help="Run ledger + desktop baseline gate")
    parser.add_argument("--baseline-cloud", action="store_true", help="Include optional cloud checks")
    parser.add_argument("--all", action="store_true", help="Run Tiers 1..4 + baseline")
    parser.add_argument("--self-audit", action="store_true", help="Run 7-gate AST & registry audit")
    parser.add_argument("--self-audit-features", action="append", help="Feature range e.g. OB-F01..OB-F10")
    parser.add_argument("--feature", action="append", help="Filter by feature(s)")
    parser.add_argument("--filter", help="Substring/regex filter on test ID or description")
    parser.add_argument("--bin", dest="cli_bin", help="Explicit path to gpui-telegram-cli binary")
    parser.add_argument("--json-out", "--json-report", dest="json_out", help="JSON report output path")
    parser.add_argument("-v", "--verbose", action="store_true", help="Verbose per-test logging")
    args = parser.parse_args(argv)

    if args.cli_bin:
        set_cli_bin_override(args.cli_bin)

    report: Dict[str, Any] = {
        "ok": True,
        "timestamp": time.strftime("%Y-%m-%dT%H:%M:%SZ", time.gmtime()),
    }

    if args.filter is not None:
        try:
            re.compile(args.filter, re.IGNORECASE)
        except re.error as exc:
            msg = f"Invalid --filter regex: {exc}"
            print(f"[USAGE ERROR] {msg}", file=sys.stderr)
            report["ok"] = False
            report["usage_error"] = msg
            _write_report(args.json_out, report)
            return 2

    try:
        tiers = parse_tiers(args.tier, args.all)
        audit_features = parse_feature_spec(args.self_audit_features)
        filter_features = parse_feature_spec(args.feature)
    except ValueError as exc:
        print(f"[USAGE ERROR] {exc}", file=sys.stderr)
        report["ok"] = False
        report["usage_error"] = str(exc)
        _write_report(args.json_out, report)
        return 2

    run_baseline = bool(args.baseline or args.all)
    if not args.self_audit and not tiers and not run_baseline:
        parser.print_help(sys.stderr)
        report["ok"] = False
        report["usage_error"] = "No action specified (--tier, --baseline, --all, or --self-audit required)"
        _write_report(args.json_out, report)
        return 2

    exit_code = 0

    if args.self_audit:
        audit_ok, audit_data = run_self_audit(audit_features)
        report["self_audit"] = audit_data
        print(
            f"[SELF-AUDIT] {'PASS' if audit_ok else 'FAIL'} | "
            f"registered={audit_data['total_registered']} | "
            f"tiers={audit_data['tier_totals']} | errors={len(audit_data['errors'])}"
        )
        for err in audit_data["errors"]:
            print(f"  - {err}", file=sys.stderr)
        if not audit_ok:
            report["ok"] = False
            exit_code = 1

    if tiers:
        discover_and_import_tier_modules()
        try:
            selected = select_tests(tiers, filter_features, args.filter)
        except re.error as exc:
            msg = f"Invalid --filter regex: {exc}"
            print(f"[USAGE ERROR] {msg}", file=sys.stderr)
            report["ok"] = False
            report["usage_error"] = msg
            _write_report(args.json_out, report)
            return 2
        if not selected:
            print("[FILTER ERROR] Zero registered E2E tests matched selection.", file=sys.stderr)
            report["ok"] = False
            _write_report(args.json_out, report)
            return 2
        resolved_bin, checked = resolve_cli_bin(args.cli_bin)
        if resolved_bin is None:
            msg = f"CLI binary not built yet. Checked: {', '.join(checked)}"
            print(f"[BINARY NOT READY] {msg}", file=sys.stderr)
            report["ok"] = False
            report["binary_not_ready"] = {"message": msg, "checked": checked}
            _write_report(args.json_out, report)
            return 2
        print(f"[RUN] Executing {len(selected)} E2E tests against {resolved_bin} (tiers={tiers})...")
        try:
            exec_ok, exec_data = execute_tests(selected, verbose=args.verbose)
        except BinaryNotReadyError as exc:
            print(f"[BINARY NOT READY] {exc}", file=sys.stderr)
            report["ok"] = False
            report["binary_not_ready"] = {"message": str(exc)}
            _write_report(args.json_out, report)
            return 2
        report["e2e_execution"] = exec_data
        print(
            f"[SUMMARY] E2E Tiers {tiers}: {exec_data['passed']}/{exec_data['selected']} passed, "
            f"{exec_data['failed']} failed."
        )
        if not exec_ok:
            report["ok"] = False
            exit_code = 1

    if run_baseline:
        base_ok, base_data = run_baseline_gate(verbose=args.verbose)
        report["baseline"] = base_data
        print(f"[BASELINE] {'PASS' if base_ok else 'FAIL'}")
        if not base_ok:
            report["ok"] = False
            exit_code = 1

    _write_report(args.json_out, report)
    return exit_code


if __name__ == "__main__":
    sys.exit(main())
