#!/usr/bin/env python3
"""Canonical entrypoint for running the Orca Pure-Rust E2E test suite.

Resolves /opt/pipx/bin/pytest first (falling back to /usr/bin/python3 -m pytest),
forces PYTHONDONTWRITEBYTECODE=1, applies tests/orca_e2e/pytest.ini, and maps
--tier {1,2,3,4,all,self} to strict pytest marker expressions.
"""

from __future__ import annotations

import argparse
import os
import shutil
import subprocess
import sys
from pathlib import Path

SUITE_DIR = Path(__file__).resolve().parent
PYTEST_INI = SUITE_DIR / "pytest.ini"

TIER_MARKER_EXPRESSIONS: dict[str, str] = {
    "1": "tier1",
    "2": "tier2",
    "3": "tier3",
    "4": "tier4",
    "all": "tier1 or tier2 or tier3 or tier4",
    "self": "harness_self_check",
}

OPTIONS_TAKING_VALUE = {
    "-k",
    "-m",
    "-o",
    "-c",
    "-p",
    "-W",
    "-r",
    "--override-ini",
    "--pythonwarnings",
    "--tb",
    "--maxfail",
    "--rootdir",
    "--junitxml",
    "--junit-prefix",
    "--basetemp",
    "--durations",
    "--durations-min",
    "--color",
    "--code-highlight",
    "--capture",
    "--show-capture",
    "--import-mode",
    "--assert",
    "--confcutdir",
    "--ignore",
    "--ignore-glob",
    "--deselect",
    "--lfnf",
    "--log-level",
    "--log-format",
    "--log-date-format",
    "--log-cli-level",
    "--log-cli-format",
    "--log-cli-date-format",
    "--log-file",
    "--log-file-level",
    "--log-file-format",
    "--log-file-date-format",
    "--server-url",
    "--server-bin",
    "--cli-bin",
}


def resolve_pytest_command() -> list[str]:
    """Return the preferred pytest invocation vector using stdlib-only probing."""
    pipx_pytest = Path("/opt/pipx/bin/pytest")
    if pipx_pytest.is_file() and os.access(pipx_pytest, os.X_OK):
        return [str(pipx_pytest)]

    usr_python = Path("/usr/bin/python3")
    if usr_python.is_file() and os.access(usr_python, os.X_OK):
        return [str(usr_python), "-m", "pytest"]

    found = shutil.which("pytest")
    if found:
        return [found]

    return [sys.executable, "-m", "pytest"]


def _looks_like_test_target(token: str) -> bool:
    if not token:
        return False
    if "::" in token or token.endswith(".py") or "/" in token:
        return True
    return Path(token).exists()


def _extract_user_markers(passthrough_args: list[str]) -> tuple[list[str], list[str]]:
    """Extract user -m expressions (-m expr, -m=expr, -mexpr) and return remaining args."""
    markers: list[str] = []
    remaining: list[str] = []
    i = 0
    n = len(passthrough_args)
    after_double_dash = False
    while i < n:
        tok = passthrough_args[i]
        if after_double_dash:
            remaining.append(tok)
            i += 1
            continue
        if tok == "--":
            after_double_dash = True
            remaining.append(tok)
            i += 1
            continue
        if tok == "-m":
            if i + 1 < n:
                markers.append(passthrough_args[i + 1])
                i += 2
                continue
            remaining.append(tok)
            i += 1
            continue
        if tok.startswith("-m="):
            markers.append(tok[3:])
            i += 1
            continue
        if tok.startswith("-m") and len(tok) > 2 and not tok.startswith("--"):
            markers.append(tok[2:])
            i += 1
            continue
        if tok in OPTIONS_TAKING_VALUE:
            remaining.append(tok)
            if i + 1 < n:
                remaining.append(passthrough_args[i + 1])
                i += 2
                continue
            i += 1
            continue
        remaining.append(tok)
        i += 1
    return markers, remaining


def _has_explicit_test_target(passthrough_args: list[str]) -> bool:
    """Detect whether the caller already provided explicit test file/dir/nodeid targets."""
    skip_next = False
    after_double_dash = False
    for token in passthrough_args:
        if after_double_dash:
            if _looks_like_test_target(token):
                return True
            continue
        if skip_next:
            skip_next = False
            continue
        if token == "--":
            after_double_dash = True
            continue
        if token in OPTIONS_TAKING_VALUE:
            skip_next = True
            continue
        if token.startswith("-"):
            continue
        if _looks_like_test_target(token):
            return True
    return False


def build_pytest_argv(argv: list[str] | None = None) -> list[str]:
    """Parse runner CLI options and construct the full pytest command line."""
    parser = argparse.ArgumentParser(
        description="Run Orca Pure-Rust opaque-box E2E tests (Tiers 1-4 or harness self-checks).",
    )
    parser.add_argument(
        "--tier",
        choices=("1", "2", "3", "4", "all", "self"),
        default=None,
        help="Test tier to execute (default: all -> tier1..tier4; self -> harness_self_check).",
    )
    parser.add_argument("--server-url", default=None, help="Connect to an already-running server URL.")
    parser.add_argument("--server-bin", default=None, help="Path to gpui_telegram_server binary.")
    parser.add_argument("--cli-bin", default=None, help="Path to fabushi CLI binary.")
    parser.add_argument("--collect-only", action="store_true", help="Collect tests without executing.")

    args, passthrough = parser.parse_known_args(argv)

    cmd = resolve_pytest_command()
    cmd.extend(["-c", str(PYTEST_INI)])

    user_markers, cleaned_passthrough = _extract_user_markers(passthrough)
    if args.tier is not None:
        tier_expr = TIER_MARKER_EXPRESSIONS[args.tier]
        user_expr = user_markers[-1].strip() if user_markers else ""
        if user_expr:
            cmd.extend(["-m", f"({tier_expr}) and ({user_expr})"])
            effective_passthrough = cleaned_passthrough
        else:
            cmd.extend(["-m", tier_expr])
            effective_passthrough = cleaned_passthrough if user_markers else passthrough
    elif user_markers:
        effective_passthrough = passthrough
    else:
        cmd.extend(["-m", TIER_MARKER_EXPRESSIONS["all"]])
        effective_passthrough = passthrough

    if args.server_url:
        cmd.append(f"--server-url={args.server_url}")
    if args.server_bin:
        cmd.append(f"--server-bin={args.server_bin}")
    if args.cli_bin:
        cmd.append(f"--cli-bin={args.cli_bin}")
    if args.collect_only:
        cmd.append("--collect-only")

    cmd.extend(effective_passthrough)

    if not _has_explicit_test_target(effective_passthrough):
        cmd.append(str(SUITE_DIR))

    return cmd


def main(argv: list[str] | None = None) -> int:
    cmd = build_pytest_argv(argv)
    env = os.environ.copy()
    env["PYTHONDONTWRITEBYTECODE"] = "1"
    completed = subprocess.run(cmd, env=env)
    return int(completed.returncode)


if __name__ == "__main__":
    raise SystemExit(main())
