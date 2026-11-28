"""Opaque-box E2E harness, fail-closed runtime UI probe, RAII sandbox, and baseline verifiers.

100% Python 3.10+ standard library. Zero internal Rust imports, zero external packages.
"""

from __future__ import annotations

import copy
import dataclasses
import functools
import hashlib
import inspect
import json
import os
import re
import subprocess
import sys
import tempfile
from pathlib import Path
from typing import Any, Callable, Dict, List, Optional, Sequence, Tuple

from tests.e2e_openbot_uiux.oracles import DEFECT_KEYS_D1_TO_D12

REPO_ROOT: Path = Path(__file__).resolve().parents[2]
ALL_FEATURES: Tuple[str, ...] = tuple(f"UI-F{i:02d}" for i in range(1, 21))
PROBE_SCHEMA_VERSION: int = 1
PROBE_SUITE_NAME: str = "openbot-uiux-refactor-v1"
PROBE_BIN_TEST_NAME: str = "emit_openbot_uiux_probe_json"

TEST_INFRA_PREFIX_152875_SHA256 = (
    "daa96e4165085964ccd552ca1d6213166e70d7a5bd43f4d3f37dc573711ce8ff"
)
TEST_INFRA_PREFIX_224319_SHA256 = (
    "543ac94ed88e4d1536f9e94a0e632463bb5de28300fb0f2c134196d7ccb370e9"
)

RUST_PANIC_EXIT_CODES = frozenset({101, 132, 134, 135, 136, 139})
_MISSING = object()


class ProbeConfigError(RuntimeError):
    """Raised when an explicitly provided --probe-json/--probe-bin/--cli-bin is invalid."""


@dataclasses.dataclass(frozen=True)
class UiUxE2ETestCase:
    id: str
    tier: int
    features: Tuple[str, ...]
    description: str
    qualname: str
    module_name: str
    fn: Callable[[], None]


TEST_REGISTRY: List[UiUxE2ETestCase] = []
_REGISTERED_IDS: set[str] = set()

_PROBE_JSON_OVERRIDE: Optional[str] = None
_PROBE_BIN_OVERRIDE: Optional[str] = None
_CLI_BIN_OVERRIDE: Optional[str] = None
_CACHED_PROBE: Optional[Tuple[Optional[Dict[str, Any]], str, Optional[str]]] = None


def configure_probe_overrides(
    probe_json: Optional[str] = None,
    probe_bin: Optional[str] = None,
    cli_bin: Optional[str] = None,
) -> None:
    """Configure runtime probe overrides for the active runner session and clear cache."""
    global _PROBE_JSON_OVERRIDE, _PROBE_BIN_OVERRIDE, _CLI_BIN_OVERRIDE, _CACHED_PROBE
    _PROBE_JSON_OVERRIDE = probe_json
    _PROBE_BIN_OVERRIDE = probe_bin
    _CLI_BIN_OVERRIDE = cli_bin
    _CACHED_PROBE = None


def _deep_merge_dict(base: Dict[str, Any], overlay: Dict[str, Any]) -> Dict[str, Any]:
    merged = copy.deepcopy(base)
    for k, v in overlay.items():
        if isinstance(v, dict) and isinstance(merged.get(k), dict):
            merged[k] = _deep_merge_dict(merged[k], v)
        else:
            merged[k] = copy.deepcopy(v)
    return merged


def assert_no_rust_panic(returncode: int, stdout: str, stderr: str) -> None:
    combined = f"{stdout}\n{stderr}"
    if returncode in RUST_PANIC_EXIT_CODES or "panicked at" in combined or "RUST_BACKTRACE" in stderr:
        raise AssertionError(
            f"Rust panic/crash detected (exit={returncode}): {combined[-600:]}"
        )


def _validate_probe_envelope(data: Any, source: str) -> Dict[str, Any]:
    if not isinstance(data, dict):
        raise AssertionError(f"Probe telemetry from {source} must be a JSON object")
    schema_ver = data.get("schema_version")
    if (
        isinstance(schema_ver, bool)
        or not isinstance(schema_ver, int)
        or schema_ver != PROBE_SCHEMA_VERSION
    ):
        raise AssertionError(
            f"Probe schema_version mismatch from {source}: got {schema_ver!r}, "
            f"expected {PROBE_SCHEMA_VERSION}"
        )
    if data.get("suite") != PROBE_SUITE_NAME:
        raise AssertionError(
            f"Probe suite mismatch from {source}: got {data.get('suite')!r}, "
            f"expected {PROBE_SUITE_NAME!r}"
        )
    if "panics" not in data:
        raise AssertionError(f"Probe telemetry missing required 'panics' field ({source})")
    panics = data["panics"]
    if isinstance(panics, bool) or not isinstance(panics, int) or panics != 0:
        raise AssertionError(f"Probe telemetry reported non-zero/invalid panics={panics!r} ({source})")
    return data


class UiUxRuntimeProbe:
    """Strict fail-closed resolver for live OpenBot UI/UX runtime probe telemetry."""

    @staticmethod
    def resolve(temp_dir: Path) -> Tuple[Optional[Dict[str, Any]], str, Optional[str]]:
        global _CACHED_PROBE
        if _CACHED_PROBE is not None:
            return _CACHED_PROBE

        # Priority 1: --probe-json / OPENBOT_UIUX_PROBE_JSON
        explicit_json = _PROBE_JSON_OVERRIDE or os.environ.get("OPENBOT_UIUX_PROBE_JSON")
        if explicit_json and explicit_json.strip():
            path = Path(explicit_json.strip()).expanduser().resolve()
            if not path.is_file():
                raise ProbeConfigError(f"Explicit --probe-json not found: {path}")
            try:
                raw = json.loads(path.read_text(encoding="utf-8"))
            except Exception as exc:
                raise ProbeConfigError(f"Malformed JSON in --probe-json {path}: {exc}") from exc
            if not isinstance(raw, dict):
                raise ProbeConfigError(f"--probe-json {path} root must be a JSON object")
            source_tag = f"probe_json:{path}"
            try:
                validated = _validate_probe_envelope(raw, source_tag)
            except AssertionError as exc:
                _CACHED_PROBE = (None, source_tag, str(exc))
                return _CACHED_PROBE
            _CACHED_PROBE = (validated, source_tag, None)
            return _CACHED_PROBE

        # Priority 2: --probe-bin / OPENBOT_UIUX_PROBE_BIN
        explicit_bin = _PROBE_BIN_OVERRIDE or os.environ.get("OPENBOT_UIUX_PROBE_BIN")
        if explicit_bin and explicit_bin.strip():
            bpath = Path(explicit_bin.strip()).expanduser().resolve()
            if not (bpath.is_file() and os.access(bpath, os.X_OK)):
                raise ProbeConfigError(f"Explicit --probe-bin not executable: {bpath}")
            source_tag = f"probe_bin:{bpath}"
            out_json = temp_dir / "probe_bin_out.json"
            env = dict(os.environ)
            env["OPENBOT_UIUX_EMIT_PROBE_JSON"] = str(out_json)
            env["GPUI_TELEGRAM_CONFIG_DIR"] = str(temp_dir / "cfg")
            env["GPUI_OFFLINE_TEST"] = "1"
            cmd = [str(bpath), "--exact", PROBE_BIN_TEST_NAME, "--nocapture", "--test-threads=1"]
            try:
                proc = subprocess.run(
                    cmd,
                    cwd=str(REPO_ROOT),
                    env=env,
                    capture_output=True,
                    text=True,
                    timeout=180,
                    check=False,
                )
            except (OSError, subprocess.TimeoutExpired) as exc:
                reason = f"probe_bin execution failed: {type(exc).__name__}: {exc}"
                _CACHED_PROBE = (None, source_tag, reason)
                return _CACHED_PROBE
            stderr_lower = (proc.stderr or "").lower()
            if (
                proc.returncode == 2
                and not out_json.is_file()
                and proc.returncode not in RUST_PANIC_EXIT_CODES
                and (
                    "--exact" in (proc.stderr or "")
                    or "unrecognized" in stderr_lower
                    or "unexpected argument" in stderr_lower
                )
            ):
                try:
                    proc = subprocess.run(
                        [str(bpath)],
                        cwd=str(REPO_ROOT),
                        env=env,
                        capture_output=True,
                        text=True,
                        timeout=180,
                        check=False,
                    )
                except (OSError, subprocess.TimeoutExpired) as exc:
                    reason = f"probe_bin fallback execution failed: {type(exc).__name__}: {exc}"
                    _CACHED_PROBE = (None, source_tag, reason)
                    return _CACHED_PROBE
            try:
                assert_no_rust_panic(proc.returncode, proc.stdout, proc.stderr)
            except AssertionError as exc:
                _CACHED_PROBE = (None, source_tag, str(exc))
                return _CACHED_PROBE
            if proc.returncode != 0 or not out_json.is_file():
                reason = (
                    f"probe_bin exited {proc.returncode} (json_exists={out_json.is_file()}): "
                    f"{(proc.stderr or proc.stdout)[-400:]}"
                )
                _CACHED_PROBE = (None, source_tag, reason)
                return _CACHED_PROBE
            try:
                raw = json.loads(out_json.read_text(encoding="utf-8"))
            except (OSError, json.JSONDecodeError) as exc:
                reason = f"probe_bin emitted invalid JSON: {type(exc).__name__}: {exc}"
                _CACHED_PROBE = (None, source_tag, reason)
                return _CACHED_PROBE
            if not isinstance(raw, dict):
                reason = f"probe_bin JSON root must be a JSON object, got {type(raw).__name__}"
                _CACHED_PROBE = (None, source_tag, reason)
                return _CACHED_PROBE
            try:
                validated = _validate_probe_envelope(raw, source_tag)
            except AssertionError as exc:
                _CACHED_PROBE = (None, source_tag, str(exc))
                return _CACHED_PROBE
            _CACHED_PROBE = (validated, source_tag, None)
            return _CACHED_PROBE

        # Priority 3: --cli-bin / GPUI_TELEGRAM_CLI_BIN
        explicit_cli = _CLI_BIN_OVERRIDE or os.environ.get("GPUI_TELEGRAM_CLI_BIN")
        if explicit_cli and explicit_cli.strip():
            cpath = Path(explicit_cli.strip()).expanduser().resolve()
            if not (cpath.is_file() and os.access(cpath, os.X_OK)):
                raise ProbeConfigError(f"Explicit --cli-bin not executable: {cpath}")
            source_tag = f"cli_bin:{cpath}"
            cli_env = dict(os.environ)
            cli_env["GPUI_TELEGRAM_CONFIG_DIR"] = str(temp_dir / "cli_cfg")
            cli_env["GPUI_OFFLINE_TEST"] = "1"
            try:
                proc = subprocess.run(
                    [
                        str(cpath),
                        "openbot",
                        "ui-probe",
                        "--scenario",
                        "openbot-uiux-live-shell",
                        "--data-dir",
                        str(temp_dir / "cli_data"),
                        "--json",
                    ],
                    cwd=str(REPO_ROOT),
                    env=cli_env,
                    capture_output=True,
                    text=True,
                    timeout=120,
                    check=False,
                )
            except (OSError, subprocess.TimeoutExpired) as exc:
                reason = f"cli_bin execution failed: {type(exc).__name__}: {exc}"
                _CACHED_PROBE = (None, source_tag, reason)
                return _CACHED_PROBE
            try:
                assert_no_rust_panic(proc.returncode, proc.stdout, proc.stderr)
            except AssertionError as exc:
                _CACHED_PROBE = (None, source_tag, str(exc))
                return _CACHED_PROBE
            if proc.returncode != 0:
                reason = (
                    f"cli_bin openbot-uiux-live-shell returned exit {proc.returncode}: "
                    f"{(proc.stderr or proc.stdout)[-300:]}"
                )
                _CACHED_PROBE = (None, source_tag, reason)
                return _CACHED_PROBE
            try:
                raw = json.loads(proc.stdout)
            except (OSError, json.JSONDecodeError) as exc:
                reason = f"cli_bin emitted invalid JSON: {type(exc).__name__}: {exc}"
                _CACHED_PROBE = (None, source_tag, reason)
                return _CACHED_PROBE
            if (
                isinstance(raw, dict)
                and "schema_version" not in raw
                and raw.get("success") is True
                and isinstance(raw.get("data"), dict)
            ):
                raw = raw["data"]
            if not isinstance(raw, dict):
                reason = f"cli_bin JSON root must be a JSON object, got {type(raw).__name__}"
                _CACHED_PROBE = (None, source_tag, reason)
                return _CACHED_PROBE
            try:
                validated = _validate_probe_envelope(raw, source_tag)
            except AssertionError as exc:
                _CACHED_PROBE = (None, source_tag, str(exc))
                return _CACHED_PROBE
            _CACHED_PROBE = (validated, source_tag, None)
            return _CACHED_PROBE

        # Phase A Fail-Closed (no probe supplied)
        reason = (
            "Live OpenBot UI/UX runtime probe unavailable (Phase B binary "
            "openbot_uiux_refactor_e2e not yet built and no --probe-json supplied) "
            "— honest Phase A fail-closed"
        )
        _CACHED_PROBE = (None, "unavailable", reason)
        return _CACHED_PROBE

    @staticmethod
    def probe_source_summary() -> Tuple[bool, str]:
        if _CACHED_PROBE is None:
            return (False, "unavailable")
        data, src, _ = _CACHED_PROBE
        return (data is not None, src)


class TempUiUxEnv:
    """RAII temporary sandbox and fail-closed opaque-box runtime probe interface."""

    def __init__(self) -> None:
        self._tmp: Optional[tempfile.TemporaryDirectory[str]] = None
        self.root: Path = REPO_ROOT

    def __enter__(self) -> "TempUiUxEnv":
        self._tmp = tempfile.TemporaryDirectory(prefix="openbot_uiux_e2e_")
        self.root = Path(self._tmp.name)
        return self

    def __exit__(self, exc_type: Any, exc: Any, tb: Any) -> None:
        if self._tmp is not None:
            self._tmp.cleanup()
            self._tmp = None

    def require_probe(self) -> Dict[str, Any]:
        data, src, err = UiUxRuntimeProbe.resolve(self.root)
        if data is None:
            raise AssertionError(err or f"Runtime probe unavailable ({src})")
        return _validate_probe_envelope(data, src)

    def probe_full(self) -> Dict[str, Any]:
        return self.require_probe()

    def probe_section(self, section: str) -> Dict[str, Any]:
        root = self.require_probe()
        sec = root.get(section)
        if not isinstance(sec, dict) or not sec:
            raise AssertionError(
                f"Runtime probe missing required non-empty section {section!r}"
            )
        return sec

    def probe_scenario(self, scenario_id: str, section: str) -> Dict[str, Any]:
        root = self.require_probe()
        base = root.get(section)
        if base is not None and not isinstance(base, dict):
            raise AssertionError(
                f"Runtime probe section {section!r} must be a JSON object, got {type(base).__name__}"
            )
        base_dict: Dict[str, Any] = base if isinstance(base, dict) else {}
        scenarios = root.get("scenarios")
        if scenarios is not None and not isinstance(scenarios, dict):
            raise AssertionError(
                f"Runtime probe 'scenarios' must be a JSON object, got {type(scenarios).__name__}"
            )
        overlay_dict: Dict[str, Any] = {}
        if isinstance(scenarios, dict) and scenario_id in scenarios:
            sc_entry = scenarios[scenario_id]
            if not isinstance(sc_entry, dict):
                raise AssertionError(
                    f"Runtime probe scenarios[{scenario_id!r}] must be a JSON object, "
                    f"got {type(sc_entry).__name__}"
                )
            if section in sc_entry:
                sec_ov = sc_entry[section]
                if not isinstance(sec_ov, dict):
                    raise AssertionError(
                        f"Runtime probe scenarios[{scenario_id!r}][{section!r}] must be a JSON object, "
                        f"got {type(sec_ov).__name__}"
                    )
                overlay_dict = sec_ov
        merged = _deep_merge_dict(base_dict, overlay_dict) if overlay_dict else copy.deepcopy(base_dict)
        if not merged:
            raise AssertionError(
                f"Runtime probe missing section {section!r} for scenario {scenario_id!r}"
            )
        return merged

    def probe_cli_reject(self, scenario_id: str) -> Dict[str, Any]:
        """Inspect negative CLI/probe rejection telemetry (fail-closed if probe absent)."""
        return self.probe_scenario(scenario_id, "compat_and_ledger")


# ============================================================================
# Strict Opaque-Box Assertion Helpers
# ============================================================================


def require_field(obj: Dict[str, Any], path: str) -> Any:
    cur: Any = obj
    for seg in path.split("."):
        if not isinstance(cur, dict) or seg not in cur:
            raise AssertionError(f"Missing required field {path!r} (stopped at {seg!r})")
        cur = cur[seg]
    return cur


def require_fields(obj: Dict[str, Any], paths: Sequence[str]) -> Dict[str, Any]:
    return {p: require_field(obj, p) for p in paths}


def assert_hex_eq(actual: Any, expected: str, label: str = "") -> None:
    if not isinstance(actual, str):
        raise AssertionError(f"{label or 'color'}: expected hex {expected!r}, got {actual!r}")
    if actual.strip().lower() != expected.strip().lower():
        raise AssertionError(f"{label or 'color'}: expected {expected!r}, got {actual!r}")


def assert_within(actual: Any, lo: float, hi: float, label: str = "") -> None:
    if isinstance(actual, bool) or not isinstance(actual, (int, float)):
        raise AssertionError(f"{label or 'value'}: expected number in [{lo}, {hi}], got {actual!r}")
    if not (lo <= float(actual) <= hi):
        raise AssertionError(f"{label or 'value'}: {actual} outside [{lo}, {hi}]")


def assert_token_map(actual_map: Dict[str, Any], expected_map: Dict[str, str], label: str) -> None:
    if not isinstance(actual_map, dict):
        raise AssertionError(f"{label}: expected token dict, got {type(actual_map).__name__}")
    for key, exp in expected_map.items():
        val = require_field(actual_map, key)
        if not isinstance(val, str):
            raise AssertionError(f"{label}.{key}: expected string {exp!r}, got {val!r}")
        if exp.startswith("#"):
            assert_hex_eq(val, exp, f"{label}.{key}")
        else:
            norm_actual = re.sub(r"\s+", "", val.lower())
            norm_exp = re.sub(r"\s+", "", exp.lower())
            if norm_actual != norm_exp:
                raise AssertionError(f"{label}.{key}: expected {exp!r}, got {val!r}")


def assert_defect_eliminated(defects: Dict[str, Any], key: str) -> None:
    val = require_field(defects, key)
    if val is not False:
        raise AssertionError(f"Defect regression: {key} must be False, got {val!r}")


def assert_all_defects_eliminated(defects: Dict[str, Any]) -> None:
    for key in DEFECT_KEYS_D1_TO_D12:
        assert_defect_eliminated(defects, key)


# ============================================================================
# Real Read-Only Workspace Baseline & Artifact Oracles (Zero Cargo/Rustc)
# ============================================================================


def check_workspace_plush_assets() -> Dict[str, Any]:
    """Verify exact 73 .webp (RIFF..WEBP) + 1 .webm (EBML) = 74 plush assets on disk."""
    assets_dir = REPO_ROOT / "crates" / "gpui_telegram_desktop" / "assets" / "openbot_plush"
    if not assets_dir.is_dir():
        raise AssertionError(f"Plush assets directory missing: {assets_dir}")
    webp_files: List[str] = []
    webm_files: List[str] = []
    invalid_headers: List[str] = []
    for p in sorted(assets_dir.rglob("*")):
        if not p.is_file():
            continue
        rel = p.relative_to(assets_dir).as_posix()
        data = p.read_bytes()
        if len(data) == 0:
            invalid_headers.append(f"{rel}:empty")
            continue
        if p.suffix == ".webp":
            webp_files.append(rel)
            if len(data) < 12 or data[:4] != b"RIFF" or data[8:12] != b"WEBP":
                invalid_headers.append(f"{rel}:bad_webp_magic")
        elif p.suffix == ".webm":
            webm_files.append(rel)
            if len(data) < 4 or data[:4] != b"\x1a\x45\xdf\xa3":
                invalid_headers.append(f"{rel}:bad_webm_magic")
    if invalid_headers:
        raise AssertionError(f"Corrupt plush asset headers: {invalid_headers[:10]}")
    if len(webp_files) != 73 or len(webm_files) != 1:
        raise AssertionError(
            f"Expected 73 .webp + 1 .webm plush assets, found {len(webp_files)} .webp + {len(webm_files)} .webm"
        )
    pkg_script = REPO_ROOT / "scripts" / "package_release.py"
    pkg_bundles_assets = pkg_script.is_file() and ("assets" in pkg_script.read_text(encoding="utf-8"))
    return {
        "webp_count": len(webp_files),
        "webm_count": len(webm_files),
        "total_files": len(webp_files) + len(webm_files),
        "has_boule": "bloub/boule.webp" in webp_files,
        "has_fishing_webm": "ui/fishing.webm" in webm_files,
        "has_fishing_still": "ui/fishing-still.webp" in webp_files,
        "pkg_bundles_assets": pkg_bundles_assets,
    }


def check_cargo_toml_lean_profiles() -> Dict[str, Any]:
    """Verify AGENTS.md lean Cargo profiles and zero sibling .cargo-target-* dirs."""
    cargo_toml = REPO_ROOT / "Cargo.toml"
    text = cargo_toml.read_text(encoding="utf-8")
    profiles: Dict[str, Dict[str, str]] = {}
    current: Optional[str] = None
    for raw_line in text.splitlines():
        line = raw_line.strip()
        m_hdr = re.fullmatch(r"\[profile\.(dev|test|ci|release)\]", line)
        if m_hdr:
            current = m_hdr.group(1)
            profiles[current] = {}
            continue
        if line.startswith("["):
            current = None
            continue
        if current and "=" in line and not line.startswith("#"):
            k, v = [part.strip() for part in line.split("=", 1)]
            profiles[current][k] = v.split("#", 1)[0].strip()
    for prof in ("dev", "test", "ci", "release"):
        sec = profiles.get(prof)
        if not sec:
            raise AssertionError(f"Cargo.toml missing [profile.{prof}]")
        if sec.get("debug") != "0":
            raise AssertionError(f"[profile.{prof}].debug={sec.get('debug')!r}, expected '0'")
        if sec.get("strip") != '"debuginfo"':
            raise AssertionError(f"[profile.{prof}].strip={sec.get('strip')!r}, expected '\"debuginfo\"'")
        if sec.get("incremental") != "false":
            raise AssertionError(f"[profile.{prof}].incremental={sec.get('incremental')!r}, expected 'false'")
    extra_targets = [p.name for p in REPO_ROOT.glob(".cargo-target-*")]
    if extra_targets:
        raise AssertionError(f"Forbidden extra cargo target dirs present: {extra_targets}")
    cfg_path = REPO_ROOT / ".cargo" / "config.toml"
    cfg_text = cfg_path.read_text(encoding="utf-8") if cfg_path.is_file() else ""
    return {
        "profiles_checked": sorted(profiles.keys()),
        "extra_target_dirs": extra_targets,
        "has_clang_mold": ("clang" in cfg_text and "mold" in cfg_text),
    }


def check_capability_coverage_json() -> Dict[str, Any]:
    """Verify docs/verification/fusion-20261108/capability-coverage.json integrity."""
    cov_path = REPO_ROOT / "docs" / "verification" / "fusion-20261108" / "capability-coverage.json"
    data = json.loads(cov_path.read_text(encoding="utf-8"))
    if data.get("schema_version") != 2:
        raise AssertionError(f"Expected capability-coverage schema_version=2, got {data.get('schema_version')!r}")
    caps = data.get("capabilities", [])
    if len(caps) != 58:
        raise AssertionError(f"Expected 58 capabilities, found {len(caps)}")
    entrypoints: set[str] = set()
    ui_artifacts: set[str] = set()
    png_artifacts: set[str] = set()
    for cap in caps:
        stages = cap.get("evidence_stages", {})
        if not isinstance(stages, dict):
            continue
        ep_stage = stages.get("entrypoint_reachable", {})
        if isinstance(ep_stage, dict):
            for ep in ep_stage.get("verified_entrypoints", []):
                if isinstance(ep, str):
                    entrypoints.add(ep)
        ev_stage = stages.get("ui_evidence_captured", {})
        if isinstance(ev_stage, dict):
            for art in ev_stage.get("artifacts", []):
                if isinstance(art, str):
                    ui_artifacts.add(art)
                    if art.endswith(".png"):
                        png_artifacts.add(art)
    if len(entrypoints) != 127:
        raise AssertionError(f"Expected 127 unique verified_entrypoints, found {len(entrypoints)}")
    if len(ui_artifacts) != 18 or len(png_artifacts) != 17:
        raise AssertionError(
            f"Expected 18 unique ui_evidence artifacts (17 PNGs), found {len(ui_artifacts)} ({len(png_artifacts)} PNGs)"
        )
    missing_eps: List[str] = []
    context_inspector_found = False
    for ep in sorted(entrypoints):
        file_part = ep.split("::", 1)[0]
        if "context_inspector.rs::ContextInspector" in ep:
            context_inspector_found = True
        if not (REPO_ROOT / file_part).is_file():
            missing_eps.append(ep)
    if missing_eps:
        raise AssertionError(f"Missing verified_entrypoints on disk: {missing_eps[:10]}")
    if not context_inspector_found:
        raise AssertionError("ContextInspector entrypoint missing from capability-coverage.json")
    missing_arts: List[str] = []
    for rel_art in sorted(ui_artifacts):
        p = REPO_ROOT / rel_art
        if not p.is_file() or p.stat().st_size == 0:
            missing_arts.append(rel_art)
        elif rel_art.endswith(".png") and (
            p.stat().st_size < 1024 or not p.read_bytes().startswith(b"\x89PNG\r\n\x1a\n")
        ):
            missing_arts.append(rel_art)
    if missing_arts:
        raise AssertionError(f"Invalid/missing UI evidence artifacts: {missing_arts[:10]}")
    accepted_caps = sum(
        1
        for cap in caps
        if isinstance(cap, dict)
        and isinstance(cap.get("evidence_stages"), dict)
        and isinstance(cap["evidence_stages"].get("packaged_accepted"), dict)
        and cap["evidence_stages"]["packaged_accepted"].get("status") == "verified"
    )
    return {
        "schema_version": data["schema_version"],
        "capabilities_count": len(caps),
        "accepted_capabilities_count": accepted_caps,
        "entrypoints_count": len(entrypoints),
        "ui_evidence_artifacts_count": len(ui_artifacts),
        "png_evidence_count": len(png_artifacts),
        "context_inspector_found": context_inspector_found,
    }


def check_preserved_pub_consts() -> Dict[str, Any]:
    """Verify backward-compatible pub consts & proto fn remain intact on disk."""
    specs = [
        ("nav_rail_width", "crates/gpui_telegram_desktop/src/views/nav_rail.rs", r"pub\s+const\s+NAV_RAIL_WIDTH\s*:\s*f32\s*=\s*52\.0\s*;"),
        ("chat_sidebar_default_width", "crates/gpui_telegram_desktop/src/views/chat_sidebar.rs", r"pub\s+const\s+DEFAULT_WIDTH\s*:\s*f32\s*=\s*288\.0\s*;"),
        ("chat_sidebar_collapsed_width", "crates/gpui_telegram_desktop/src/views/chat_sidebar.rs", r"pub\s+const\s+COLLAPSED_WIDTH\s*:\s*f32\s*=\s*64\.0\s*;"),
        ("chat_sidebar_row_height", "crates/gpui_telegram_desktop/src/views/chat_sidebar.rs", r"pub\s+const\s+ROW_HEIGHT\s*:\s*f32\s*=\s*62\.0\s*;"),
        ("context_inspector_drawer_width", "crates/gpui_telegram_desktop/src/views/context_inspector.rs", r"pub\s+const\s+DRAWER_WIDTH\s*:\s*f32\s*=\s*320\.0\s*;"),
        ("msg_max_width", "crates/gpui_telegram_desktop/src/views/stage/history_element.rs", r"pub\s+const\s+MSG_MAX_WIDTH\s*:\s*f32\s*=\s*430\.0\s*;"),
        ("msg_min_width", "crates/gpui_telegram_desktop/src/views/stage/history_element.rs", r"pub\s+const\s+MSG_MIN_WIDTH\s*:\s*f32\s*=\s*160\.0\s*;"),
        ("default_fabushi_avatar_shape", "crates/gpui_telegram_proto/src/miniapp_contracts.rs", r"pub\s+fn\s+default_fabushi_avatar_shape\s*\("),
    ]
    matched: Dict[str, bool] = {}
    missing: List[str] = []
    for key, rel, pat in specs:
        p = REPO_ROOT / rel
        ok = p.is_file() and bool(re.search(pat, p.read_text(encoding="utf-8")))
        matched[key] = ok
        if not ok:
            missing.append(f"{key} ({rel})")
    if missing:
        raise AssertionError(f"Broken backward-compatible symbols: {missing}")
    return {"all_preserved": True, "symbols": matched}


def _strip_rust_comments(code: str) -> str:
    """Strip Rust // line comments and /* */ block comments outside string literals."""
    out: List[str] = []
    i = 0
    n = len(code)
    in_str = False
    block_depth = 0
    while i < n:
        ch = code[i]
        nxt = code[i + 1] if i + 1 < n else ""
        if block_depth > 0:
            if ch == "/" and nxt == "*":
                block_depth += 1
                i += 2
            elif ch == "*" and nxt == "/":
                block_depth -= 1
                i += 2
            else:
                if ch == "\n":
                    out.append("\n")
                i += 1
            continue
        if in_str:
            out.append(ch)
            if ch == "\\" and i + 1 < n:
                out.append(code[i + 1])
                i += 2
                continue
            if ch == '"':
                in_str = False
            i += 1
            continue
        if ch == '"':
            in_str = True
            out.append(ch)
            i += 1
        elif ch == "/" and nxt == "/":
            while i < n and code[i] != "\n":
                i += 1
        elif ch == "/" and nxt == "*":
            block_depth = 1
            i += 2
        else:
            out.append(ch)
            i += 1
    return "".join(out)


def check_no_allow_dead_code() -> Dict[str, Any]:
    """Verify zero crate/module-level #![allow(dead_code)] or #![allow(unused)] in desktop src."""
    src_root = REPO_ROOT / "crates" / "gpui_telegram_desktop" / "src"
    if not src_root.is_dir():
        raise AssertionError(f"Desktop src directory missing: {src_root}")
    pat = re.compile(r"(?m)^\s*#!\[allow\([^]]*\b(?:dead_code|unused)\b[^]]*\)\]")
    offenders: List[str] = []
    scanned = 0
    for rs in sorted(src_root.rglob("*.rs")):
        scanned += 1
        txt = _strip_rust_comments(rs.read_text(encoding="utf-8"))
        if pat.search(txt):
            offenders.append(rs.relative_to(REPO_ROOT).as_posix())
    if offenders:
        raise AssertionError(f"Forbidden #![allow(dead_code/unused)] found in: {offenders[:10]}")
    return {"scanned_files": scanned, "offenders_count": 0}


def check_immutable_governance_files() -> Dict[str, Any]:
    """Verify TEST_INFRA.md immutable prefixes, 28 ledger shards (3,441 entries), and roadmap."""
    infra_bytes = (REPO_ROOT / "TEST_INFRA.md").read_bytes()
    if len(infra_bytes) <= 224319:
        raise AssertionError(f"TEST_INFRA.md length {len(infra_bytes)} <= 224319")
    h152 = hashlib.sha256(infra_bytes[:152875]).hexdigest()
    h224 = hashlib.sha256(infra_bytes[:224319]).hexdigest()
    if h152 != TEST_INFRA_PREFIX_152875_SHA256:
        raise AssertionError(f"TEST_INFRA.md[:152875] SHA-256 mismatch: {h152}")
    if h224 != TEST_INFRA_PREFIX_224319_SHA256:
        raise AssertionError(f"TEST_INFRA.md[:224319] SHA-256 mismatch: {h224}")
    shards_dir = REPO_ROOT / "docs" / "specs" / "ledger" / "shards"
    shard_files = sorted(shards_dir.glob("*.json"))
    if len(shard_files) != 28:
        raise AssertionError(f"Expected 28 ledger shards, found {len(shard_files)}")
    total_entries = 0
    for sf in shard_files:
        doc = json.loads(sf.read_text(encoding="utf-8"))
        entries = doc.get("entries") if isinstance(doc, dict) else doc
        if isinstance(entries, list):
            total_entries += len(entries)
    if total_entries != 3441:
        raise AssertionError(f"Expected 3441 ledger entries across 28 shards, found {total_entries}")
    roadmap = REPO_ROOT / "docs" / "specs" / "MODULE_BY_MODULE_ROADMAP.md"
    if not roadmap.is_file():
        raise AssertionError("MODULE_BY_MODULE_ROADMAP.md missing")
    return {
        "test_infra_prefix_ok": True,
        "shards_count": len(shard_files),
        "ledger_entries": total_entries,
        "roadmap_exists": True,
    }


_LEDGER_CLOSURE_CACHE: Dict[bool, Dict[str, Any]] = {}


REQUIRED_CORE_PASS_MARKERS: Tuple[str, ...] = (
    "PASS [Gate 1]:",
    "PASS [Gate 2]:",
    "PASS [Gate 3]:",
    "PASS [Gate 4]:",
    "PASS [Gate 5]:",
    "[PASS] Gate 6.1 (Inherited Paths):",
    "[PASS] Gate 6.1 (Target Files):",
    "[PASS] Gate 6.2 (Target Symbols):",
    "[PASS] Gate 6.2 (Boilerplate Burndown):",
    "[PASS] Gate 6.2 (Forensic Claim Closure):",
    "[PASS] Gate 6.3 (Workspace Compilation):",
    "[PASS] Gate 6.4 (Cargo Dependencies):",
    "[PASS] Gate 6.5 (Authentic Imports):",
    "[PASS] Gate 6.6 (Module Source Coverage):",
)


def run_ledger_closure_baseline(strict_dist: bool = False) -> Dict[str, Any]:
    """Run verify_ledger_closure.py --ci-without-upstream --skip-cargo (read-only, zero cargo)."""
    if strict_dist in _LEDGER_CLOSURE_CACHE:
        return dict(_LEDGER_CLOSURE_CACHE[strict_dist])
    script_path = REPO_ROOT / "scripts" / "verify_ledger_closure.py"
    if not script_path.is_file():
        raise AssertionError(f"Missing verify_ledger_closure.py: {script_path}")
    dist_bin = REPO_ROOT / "dist" / "GPUI Telegram.app" / "Contents" / "MacOS" / "gpui_telegram_desktop"
    dist_staged = dist_bin.is_file()
    cmd = [
        sys.executable,
        "-B",
        str(script_path),
        "--ci-without-upstream",
        "--skip-cargo",
    ]
    env = dict(os.environ)
    env["PYTHONDONTWRITEBYTECODE"] = "1"
    proc = subprocess.run(
        cmd,
        cwd=str(REPO_ROOT),
        env=env,
        capture_output=True,
        text=True,
        timeout=300,
        check=False,
    )
    out = proc.stdout + "\n" + proc.stderr
    if "Traceback (most recent call last):" in out or proc.returncode not in (0, 1):
        raise AssertionError(
            f"verify_ledger_closure.py crashed (rc={proc.returncode}): {out[-600:]}"
        )

    missing_markers = [m for m in REQUIRED_CORE_PASS_MARKERS if m not in out]
    if missing_markers:
        raise AssertionError(
            f"verify_ledger_closure.py missing required PASS markers: {missing_markers}"
        )

    allow_unstaged_g67 = (not strict_dist) and (not dist_staged)
    unexpected_fails: List[str] = []
    for raw_line in out.splitlines():
        stripped = raw_line.strip()
        if "FAIL [Gate " in stripped or "[FAIL] Gate 6." in stripped:
            is_allowed_g67 = allow_unstaged_g67 and (
                stripped.startswith("FAIL [Gate 6.7 - Capability Coverage]:")
                or stripped.startswith("FAIL [Gate 6]:")
                or stripped.startswith("[FAIL] Gate 6.7 (Capability Coverage):")
            )
            if not is_allowed_g67:
                unexpected_fails.append(stripped)
    if unexpected_fails:
        raise AssertionError(f"Ledger closure core gates failed: {unexpected_fails}")

    if strict_dist or dist_staged:
        if proc.returncode != 0:
            raise AssertionError(
                f"verify_ledger_closure.py failed (strict_dist={strict_dist}, dist_staged={dist_staged}, "
                f"rc={proc.returncode}): {out[-600:]}"
            )
        if "[PASS] Gate 6.7 (Capability Coverage):" not in out or "PASS [Gate 6]:" not in out:
            raise AssertionError(
                "verify_ledger_closure.py missing '[PASS] Gate 6.7' or 'PASS [Gate 6]:' in strict/staged mode"
            )
        result = {
            "passed": True,
            "returncode": proc.returncode,
            "dist_binary_staged": dist_staged,
            "non_dist_gate67_violations": 0,
        }
        _LEDGER_CLOSURE_CACHE[strict_dist] = result
        return dict(result)

    # Unstaged non-strict branch
    if proc.returncode == 0:
        if "[PASS] Gate 6.7 (Capability Coverage):" not in out or "PASS [Gate 6]:" not in out:
            raise AssertionError(
                "verify_ledger_closure.py exited 0 without '[PASS] Gate 6.7' and 'PASS [Gate 6]:'"
            )
        result = {
            "passed": True,
            "returncode": 0,
            "dist_binary_staged": False,
            "unstaged_dist_violations": 0,
            "printed_dist_violations": 0,
            "non_dist_gate67_violations": 0,
        }
        _LEDGER_CLOSURE_CACHE[strict_dist] = result
        return dict(result)

    if "[FAIL] Gate 6.7 (Capability Coverage):" not in out:
        raise AssertionError(
            f"verify_ledger_closure.py exited {proc.returncode} without '[FAIL] Gate 6.7 (Capability Coverage):'"
        )
    m_g67 = re.search(
        r"FAIL \[Gate 6\.7 - Capability Coverage\]:\s*(\d+)\s+stage invariant violation",
        out,
    )
    if m_g67 is None:
        raise AssertionError(
            "verify_ledger_closure.py exited 1 without parseable Gate 6.7 violation count"
        )
    reported_g67 = int(m_g67.group(1))
    violation_lines = [ln.strip() for ln in out.splitlines() if ln.strip().startswith("Violation:")]
    if not violation_lines:
        raise AssertionError(
            "verify_ledger_closure.py exited 1 with Gate 6.7 failure but printed 0 'Violation:' lines"
        )
    non_dist = [
        ln for ln in violation_lines
        if (
            "dist/GPUI Telegram.app/Contents/MacOS/gpui_telegram_desktop" not in ln
            and "packaged_accepted=verified requires an existing packaged binary/app artifact" not in ln
        )
    ]
    if non_dist:
        raise AssertionError(f"Non-dist Gate 6.7 violations detected: {non_dist[:8]}")
    cap_info = check_capability_coverage_json()
    expected_dist_violations = 2 * cap_info["accepted_capabilities_count"]
    if reported_g67 != expected_dist_violations:
        raise AssertionError(
            f"Gate 6.7 reported {reported_g67} violations, expected exactly "
            f"{expected_dist_violations} (2 * {cap_info['accepted_capabilities_count']} accepted capabilities)"
        )
    result = {
        "passed": True,
        "returncode": proc.returncode,
        "dist_binary_staged": False,
        "unstaged_dist_violations": reported_g67,
        "printed_dist_violations": len(violation_lines),
        "non_dist_gate67_violations": 0,
    }
    _LEDGER_CLOSURE_CACHE[strict_dist] = result
    return dict(result)


def uiux_e2e_test(
    *,
    id: str,
    tier: int,
    features: Sequence[str],
    description: str,
) -> Callable[[Callable[..., None]], Callable[[], None]]:
    """Register an opaque-box OpenBot UI/UX E2E test with strict import-time validation."""
    if not isinstance(id, str) or not re.fullmatch(r"TEST-UIUX-T[1-4]-[A-Z0-9-]+", id):
        raise ValueError(f"Invalid test ID format: {id!r}")
    if id in _REGISTERED_IDS:
        raise ValueError(f"Duplicate test ID registered: {id!r}")
    if tier not in (1, 2, 3, 4):
        raise ValueError(f"Invalid tier {tier!r} for {id}")
    feat_tuple = tuple(features)
    if not feat_tuple or any(f not in ALL_FEATURES for f in feat_tuple):
        raise ValueError(f"Invalid features {feat_tuple!r} for {id}")
    if tier == 3 and len(set(feat_tuple)) < 2:
        raise ValueError(f"Tier 3 test {id} must reference >= 2 distinct features")
    if not isinstance(description, str) or len(description.strip()) < 10:
        raise ValueError(f"Test {id} description must be >= 10 chars")

    def decorator(fn: Callable[..., None]) -> Callable[[], None]:
        sig = inspect.signature(fn)
        n_params = len(sig.parameters)

        @functools.wraps(fn)
        def wrapped() -> None:
            if n_params == 1:
                with TempUiUxEnv() as env:
                    fn(env)
            elif n_params == 0:
                fn()
            else:
                raise TypeError(f"Test {id} must take 0 or 1 (env) parameter, got {n_params}")

        _REGISTERED_IDS.add(id)
        TEST_REGISTRY.append(
            UiUxE2ETestCase(
                id=id,
                tier=tier,
                features=feat_tuple,
                description=description.strip(),
                qualname=f"{fn.__module__}.{fn.__qualname__}",
                module_name=fn.__module__,
                fn=wrapped,
            )
        )
        return wrapped

    return decorator
