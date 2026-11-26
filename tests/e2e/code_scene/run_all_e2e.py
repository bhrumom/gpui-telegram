#!/usr/bin/env python3
"""Unified CLI runner for the Code Scene ("代码场景") 4-Tier Opaque-Box E2E Suite.

Modes (mutually exclusive, ZERO silent fallback):
  1. Default: spawns real gpui_telegram_server (--server-bin / GPUI_SERVER_BIN).
     Exits with code 2 if binary is absent or unhealthy.
  2. --base-url <url>: targets an already-running server over HTTP/JSON.
     Exits with code 2 if /health is unreachable or combined with --self-test.
  3. --self-test: spawns ephemeral SQLite-WAL StrictContractDoubleServer
     (imported ONLY here) + runs all 188 tests + 14 real HTTP mutation checks.

Pure Python 3 standard library only.
"""

from __future__ import annotations

import argparse
import datetime
import json
import os
import subprocess
import sys
import time
import traceback
from pathlib import Path
from typing import Any, Dict, List, Optional, Sequence, Tuple

_REPO_ROOT = Path(__file__).resolve().parents[3]
if __package__ in (None, ""):
    if str(_REPO_ROOT) not in sys.path:
        sys.path.insert(0, str(_REPO_ROOT))
    from tests.e2e.code_scene import SUITE_NAME, SUITE_VERSION
    from tests.e2e.code_scene.harness import (
        CodeSceneEnv,
        ExternalServerHandle,
        ServerConfigError,
        ServerProcessManager,
        TestCaseSpec,
        TestRegistry,
        TestResult,
    )
    from tests.e2e.code_scene.tier1_feature_tests import register_tier1_tests
    from tests.e2e.code_scene.tier2_boundary_tests import register_tier2_tests
    from tests.e2e.code_scene.tier3_cross_feature_tests import register_tier3_tests
    from tests.e2e.code_scene.tier4_real_world_tests import register_tier4_tests
else:
    from . import SUITE_NAME, SUITE_VERSION
    from .harness import (
        CodeSceneEnv,
        ExternalServerHandle,
        ServerConfigError,
        ServerProcessManager,
        TestCaseSpec,
        TestRegistry,
        TestResult,
    )
    from .tier1_feature_tests import register_tier1_tests
    from .tier2_boundary_tests import register_tier2_tests
    from .tier3_cross_feature_tests import register_tier3_tests
    from .tier4_real_world_tests import register_tier4_tests


def build_registry() -> TestRegistry:
    reg = TestRegistry()
    register_tier1_tests(reg)
    register_tier2_tests(reg)
    register_tier3_tests(reg)
    register_tier4_tests(reg)
    return reg


def execute_specs(env: CodeSceneEnv, specs: Sequence[TestCaseSpec], verbose: bool = False) -> List[TestResult]:
    results: List[TestResult] = []
    for sp in specs:
        t0 = time.perf_counter()
        try:
            sp.fn(env)
            elapsed = (time.perf_counter() - t0) * 1000.0
            res = TestResult(sp.test_id, sp.tier, sp.feature, sp.name, True, elapsed, "")
        except Exception as exc:
            elapsed = (time.perf_counter() - t0) * 1000.0
            tb_line = traceback.format_exc().strip().splitlines()[-1]
            msg = f"{type(exc).__name__}: {exc}" if str(exc) else tb_line
            res = TestResult(sp.test_id, sp.tier, sp.feature, sp.name, False, elapsed, msg)
        results.append(res)
        if verbose:
            tag = "PASS" if res.passed else "FAIL"
            extra = f" :: {res.error}" if not res.passed else ""
            print(f"[{tag}] {res.test_id} ({res.feature}) {res.elapsed_ms:.1f}ms — {res.name}{extra}", flush=True)
    return results


def resolve_server_binary(cli_path: Optional[str]) -> Path:
    if cli_path:
        return Path(cli_path)
    env_bin = os.environ.get("GPUI_SERVER_BIN")
    if env_bin:
        return Path(env_bin)
    dbg = _REPO_ROOT / "target" / "debug" / "gpui_telegram_server"
    if dbg.is_file():
        return dbg
    rel = _REPO_ROOT / "target" / "release" / "gpui_telegram_server"
    if rel.is_file():
        return rel
    return dbg


def run_rust_verification(verbose: bool = False) -> Dict[str, Any]:
    """Execute --with-rust cargo suites + mandatory AGENTS.md clean_build_garbage.py."""
    cmds: List[Tuple[str, List[str]]] = [
        ("gpui_telegram_proto:code_scene", ["cargo", "test", "-p", "gpui_telegram_proto", "code_scene"]),
        ("gpui_telegram_code_scene", ["cargo", "test", "-p", "gpui_telegram_code_scene"]),
        ("gpui_telegram_server:code_scene", ["cargo", "test", "-p", "gpui_telegram_server", "code_scene"]),
        ("gpui_telegram_agent:code_scene", ["cargo", "test", "-p", "gpui_telegram_agent", "code_scene"]),
        ("gpui_telegram_client:code_scene", ["cargo", "test", "-p", "gpui_telegram_client", "code_scene"]),
    ]
    desktop_tests_dir = _REPO_ROOT / "crates" / "gpui_telegram_desktop" / "tests"
    if desktop_tests_dir.is_dir():
        for rs in sorted(desktop_tests_dir.glob("*code_scene*.rs")):
            cmds.append((f"desktop:{rs.stem}", ["cargo", "test", "-p", "gpui_telegram_desktop", "--test", rs.stem]))
    env = os.environ.copy()
    env["GPUI_OFFLINE_TEST"] = "1"
    records: List[Dict[str, Any]] = []
    all_ok = True
    hygiene_cleaned = False
    try:
        for label, cmd in cmds:
            t0 = time.perf_counter()
            if verbose:
                print(f"[RUST] Running {label}: {' '.join(cmd)}", flush=True)
            proc = subprocess.run(cmd, cwd=str(_REPO_ROOT), env=env, capture_output=True, text=True)
            dur = round(time.perf_counter() - t0, 2)
            ok = proc.returncode == 0
            if not ok:
                all_ok = False
            records.append({
                "label": label, "command": " ".join(cmd), "returncode": proc.returncode,
                "passed": ok, "duration_sec": dur,
                "stderr_tail": proc.stderr[-600:] if not ok else "",
            })
    finally:
        clean_script = _REPO_ROOT / "scripts" / "clean_build_garbage.py"
        if clean_script.is_file():
            cproc = subprocess.run([sys.executable, str(clean_script)], cwd=str(_REPO_ROOT), capture_output=True, text=True)
            hygiene_cleaned = cproc.returncode == 0
            if not hygiene_cleaned:
                all_ok = False
        else:
            hygiene_cleaned = False
            all_ok = False
    return {
        "enabled": True,
        "commands": records,
        "hygiene_cleaned": hygiene_cleaned,
        "passed": all_ok,
    }


def run_self_test_mutations(
    reg: TestRegistry,
    timeout: float,
    full_mutations: bool,
    verbose: bool,
) -> Dict[str, Any]:
    """Run all 14 real HTTP mutation checks against isolated StrictContractDoubleServer instances."""
    if __package__ in (None, ""):
        from tests.e2e.code_scene._contract_double import MUTATION_SPECS, StrictContractDoubleServer
    else:
        from ._contract_double import MUTATION_SPECS, StrictContractDoubleServer

    details: List[Dict[str, Any]] = []
    killed_count = 0
    for mspec in MUTATION_SPECS:
        mid = mspec["id"]
        flag = mspec["flag"]
        target_specs = reg.all_tests() if full_mutations else reg.select(only_ids=mspec["canaries"])
        double = StrictContractDoubleServer(mutations={flag})
        t0 = time.perf_counter()
        try:
            double.start()
            env = CodeSceneEnv(double, timeout=timeout)
            mres = execute_specs(env, target_specs, verbose=False)
        finally:
            double.stop()
        caught_by = [r.test_id for r in mres if not r.passed]
        killed = len(caught_by) >= 1
        if killed:
            killed_count += 1
        dur = round(time.perf_counter() - t0, 3)
        details.append({
            "id": mid, "name": flag, "killed": killed,
            "tested": len(mres), "caught_by": caught_by, "duration_sec": dur,
        })
        if verbose:
            status_str = "KILLED" if killed else "SURVIVED"
            print(f"[MUT {status_str}] {mid} ({flag}) caught_by={caught_by} ({dur:.2f}s)", flush=True)
    total = len(MUTATION_SPECS)
    survived = total - killed_count
    return {
        "enabled": True,
        "full_suite_per_mutation": full_mutations,
        "total": total,
        "killed": killed_count,
        "survived": survived,
        "kill_rate_pct": round((killed_count / total) * 100.0, 2) if total else 0.0,
        "details": details,
    }


def build_json_report(
    mode: str,
    target_url: str,
    partial_run: bool,
    physical_restart_verified: bool,
    duration_sec: float,
    results: List[TestResult],
    mutations: Dict[str, Any],
    rust_info: Dict[str, Any],
) -> Dict[str, Any]:
    planned = len(results)
    passed = sum(1 for r in results if r.passed)
    failed = planned - passed
    tiers: Dict[str, Dict[str, Any]] = {}
    for t in (1, 2, 3, 4):
        tr = [r for r in results if r.tier == t]
        tiers[str(t)] = {
            "planned": len(tr),
            "passed": sum(1 for r in tr if r.passed),
            "failed": sum(1 for r in tr if not r.passed),
            "duration_sec": round(sum(r.elapsed_ms for r in tr) / 1000.0, 3),
        }
    features: Dict[str, Dict[str, int]] = {}
    for idx in range(1, 16):
        fid = f"CS-F{idx:02d}"
        t1_p = sum(1 for r in results if r.tier == 1 and r.feature == fid and r.passed)
        t2_p = sum(1 for r in results if r.tier == 2 and r.feature == fid and r.passed)
        f_fail = sum(1 for r in results if fid in r.feature.split("+") and not r.passed)
        features[fid] = {"t1_passed": t1_p, "t2_passed": t2_p, "failed": f_fail}
    failures = [
        {"test_id": r.test_id, "tier": r.tier, "feature": r.feature, "name": r.name, "error": r.error}
        for r in results if not r.passed
    ]
    mut_ok = (not mutations.get("enabled")) or (mutations.get("survived", 0) == 0)
    rust_ok = (not rust_info.get("enabled")) or bool(rust_info.get("passed"))
    verdict = "PASS" if (failed == 0 and mut_ok and rust_ok) else "FAIL"
    return {
        "suite": SUITE_NAME,
        "version": SUITE_VERSION,
        "mode": mode,
        "target_url": target_url,
        "timestamp_utc": datetime.datetime.now(datetime.timezone.utc).strftime("%Y-%m-%dT%H:%M:%SZ"),
        "partial_run": partial_run,
        "physical_restart_verified": physical_restart_verified,
        "duration_sec": round(duration_sec, 3),
        "summary": {
            "planned": planned, "executed": planned, "passed": passed, "failed": failed,
            "pass_rate_pct": round((passed / planned) * 100.0, 2) if planned else 0.0,
        },
        "tiers": tiers,
        "features": features,
        "mutations": mutations,
        "rust_verification": rust_info,
        "failures": failures,
        "verdict": verdict,
    }


def render_markdown_report(rep: Dict[str, Any]) -> str:
    s = rep["summary"]
    lines = [
        f"# Code Scene E2E Verification Report — **{rep['verdict']}**",
        "",
        f"- **Mode**: `{rep['mode']}` (`{rep['target_url']}`)",
        f"- **Timestamp (UTC)**: `{rep['timestamp_utc']}`",
        f"- **Total**: `{s['passed']}/{s['executed']}` passed (`{s['pass_rate_pct']}%`), `{s['failed']}` failed in `{rep['duration_sec']}s`",
        f"- **Physical Restart Verified**: `{rep['physical_restart_verified']}` | **Partial Run**: `{rep['partial_run']}`",
        "",
        "## Per-Tier Summary",
        "| Tier | Planned | Passed | Failed | Duration (s) |",
        "|---|---:|---:|---:|---:|",
    ]
    for t in ("1", "2", "3", "4"):
        td = rep["tiers"][t]
        lines.append(f"| Tier {t} | {td['planned']} | {td['passed']} | {td['failed']} | {td['duration_sec']} |")
    lines += [
        "",
        "## 15-Feature Coverage Matrix (`CS-F01`..`CS-F15`)",
        "| Feature | Tier 1 Passed | Tier 2 Passed | Failures |",
        "|---|---:|---:|---:|",
    ]
    for fid, fd in rep["features"].items():
        lines.append(f"| `{fid}` | {fd['t1_passed']} | {fd['t2_passed']} | {fd['failed']} |")
    mut = rep["mutations"]
    if mut.get("enabled"):
        lines += [
            "",
            f"## Mutation Fault-Injection Proof (`{mut['killed']}/{mut['total']}` Killed — `{mut['kill_rate_pct']}%`)",
            "| ID | Mutation Flag | Killed | Caught By |",
            "|---|---|---|---|",
        ]
        for md in mut["details"]:
            lines.append(f"| `{md['id']}` | `{md['name']}` | `{md['killed']}` | `{', '.join(md['caught_by'])}` |")
    rv = rep["rust_verification"]
    if rv.get("enabled"):
        lines += [
            "",
            f"## Rust & Headless GPUI Verification (Hygiene Cleaned: `{rv['hygiene_cleaned']}`)",
            "| Target | Command | RC | Passed |",
            "|---|---|---:|---|",
        ]
        for cmd in rv["commands"]:
            lines.append(f"| `{cmd['label']}` | `{cmd['command']}` | {cmd['returncode']} | `{cmd['passed']}` |")
    if rep["failures"]:
        lines += ["", "## Failure Diagnostics"]
        for fl in rep["failures"]:
            lines.append(f"- **`{fl['test_id']}`** ({fl['feature']}): {fl['name']} — `{fl['error']}`")
    lines.append("")
    return "\n".join(lines)


def parse_args(argv: Optional[Sequence[str]] = None) -> argparse.Namespace:
    p = argparse.ArgumentParser(description="Code Scene 4-Tier Opaque-Box E2E Test Suite")
    p.add_argument("--tier", choices=("1", "2", "3", "4", "all"), default="all", help="Tier to execute (default: all)")
    p.add_argument("--base-url", default=None, help="Target an already-running server (mutually exclusive with --self-test)")
    p.add_argument("--server-bin", default=None, help="Real gpui_telegram_server binary path (default: target/debug/gpui_telegram_server)")
    p.add_argument("--self-test", action="store_true", help="Run suite + 14 real HTTP mutations against ephemeral SQLite contract double")
    p.add_argument("--self-test-full-mutations", action="store_true", help="Run all 188 tests per mutation during --self-test")
    p.add_argument("--with-rust", action="store_true", help="Also run cargo code_scene suites + scripts/clean_build_garbage.py")
    p.add_argument("--restart-cmd", default=None, help="Shell command to physically restart an external --base-url server")
    p.add_argument("--allow-reconnect-only", action="store_true", help="Allow restart tests in --base-url mode without --restart-cmd (marks physical_restart_verified=false)")
    p.add_argument("--filter", default=None, help="Case-insensitive substring filter on test_id / feature / name")
    p.add_argument("--json-out", default=None, help="Path to write structured JSON report")
    p.add_argument("--md-out", default=None, help="Path to write Markdown summary report")
    p.add_argument("--timeout", type=float, default=None, help="Per-request HTTP timeout in seconds (default: 15.0; also bounds managed startup wait unless --startup-timeout is set)")
    p.add_argument("--startup-timeout", type=float, default=None, help="Managed server startup readiness timeout in seconds (default: --timeout if explicitly passed, else 20.0)")
    p.add_argument("-v", "--verbose", action="store_true", help="Stream per-test PASS/FAIL output")
    return p.parse_args(argv)


def main(argv: Optional[Sequence[str]] = None) -> int:
    args = parse_args(argv)
    if args.self_test and args.base_url:
        print("ERROR: --self-test and --base-url are mutually exclusive.", file=sys.stderr)
        return 2
    if args.server_bin and (args.self_test or args.base_url):
        print("ERROR: --server-bin cannot be combined with --self-test or --base-url.", file=sys.stderr)
        return 2
    if args.self_test_full_mutations and not args.self_test:
        print("ERROR: --self-test-full-mutations requires --self-test.", file=sys.stderr)
        return 2
    http_timeout = 15.0 if args.timeout is None else float(args.timeout)
    startup_timeout = (
        float(args.startup_timeout)
        if args.startup_timeout is not None
        else (float(args.timeout) if args.timeout is not None else 20.0)
    )
    if http_timeout <= 0.0 or startup_timeout <= 0.0:
        print("ERROR: --timeout and --startup-timeout must be positive numbers.", file=sys.stderr)
        return 2

    reg = build_registry()
    tier_int = None if args.tier == "all" else int(args.tier)
    specs = reg.select(tier=tier_int, filter_substr=args.filter)
    partial_run = (tier_int is not None) or bool(args.filter)
    if not specs:
        print(f"ERROR: No tests matched tier={args.tier!r}, filter={args.filter!r}.", file=sys.stderr)
        return 2

    t_start = time.perf_counter()
    handle: Any = None
    try:
        if args.self_test:
            if __package__ in (None, ""):
                from tests.e2e.code_scene._contract_double import StrictContractDoubleServer
            else:
                from ._contract_double import StrictContractDoubleServer
            handle = StrictContractDoubleServer()
        elif args.base_url:
            handle = ExternalServerHandle(
                base_url=args.base_url,
                restart_cmd=args.restart_cmd,
                allow_reconnect_only=args.allow_reconnect_only,
                timeout=http_timeout,
            )
        else:
            server_bin = resolve_server_binary(args.server_bin)
            handle = ServerProcessManager(server_bin=server_bin, startup_timeout=startup_timeout)

        target_url = handle.start()
        env = CodeSceneEnv(handle, timeout=http_timeout)
        results = execute_specs(env, specs, verbose=args.verbose)
        phys_restart = bool(getattr(handle, "physical_restart_verified", False))
        mode = str(getattr(handle, "mode", "unknown"))
    except ServerConfigError as exc:
        print(f"CONFIG ERROR: {exc}", file=sys.stderr)
        return 2
    finally:
        if handle is not None:
            handle.stop()

    mutations: Dict[str, Any] = {"enabled": False, "total": 0, "killed": 0, "survived": 0, "kill_rate_pct": 0.0, "details": []}
    if args.self_test and (not partial_run or args.self_test_full_mutations):
        mutations = run_self_test_mutations(
            reg=reg, timeout=http_timeout, full_mutations=args.self_test_full_mutations, verbose=args.verbose,
        )

    rust_info: Dict[str, Any] = {"enabled": False, "commands": [], "hygiene_cleaned": False, "passed": True}
    if args.with_rust:
        rust_info = run_rust_verification(verbose=args.verbose)

    total_dur = time.perf_counter() - t_start
    report = build_json_report(
        mode=mode, target_url=target_url, partial_run=partial_run,
        physical_restart_verified=phys_restart, duration_sec=total_dur,
        results=results, mutations=mutations, rust_info=rust_info,
    )
    md_text = render_markdown_report(report)

    if args.json_out:
        out_p = Path(args.json_out)
        out_p.parent.mkdir(parents=True, exist_ok=True)
        out_p.write_text(json.dumps(report, indent=2, ensure_ascii=False) + "\n", encoding="utf-8")
    if args.md_out:
        md_p = Path(args.md_out)
        md_p.parent.mkdir(parents=True, exist_ok=True)
        md_p.write_text(md_text, encoding="utf-8")

    s = report["summary"]
    mut_summary = f" | Mutations: {mutations['killed']}/{mutations['total']}" if mutations["enabled"] else ""
    print(
        f"[{report['verdict']}] {SUITE_NAME} ({mode}): "
        f"{s['passed']}/{s['executed']} passed ({s['pass_rate_pct']}%), "
        f"{s['failed']} failed{mut_summary} in {report['duration_sec']}s"
    )
    if report["failures"]:
        for fl in report["failures"]:
            print(f"  FAIL {fl['test_id']} ({fl['feature']}): {fl['error']}", file=sys.stderr)
    if mutations["enabled"] and mutations["survived"] > 0:
        for md in mutations["details"]:
            if not md["killed"]:
                print(f"  SURVIVED MUTATION {md['id']} ({md['name']})", file=sys.stderr)
    return 0 if report["verdict"] == "PASS" else 1


if __name__ == "__main__":
    raise SystemExit(main())
