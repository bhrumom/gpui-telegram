"""Opaque-box E2E harness, RAII sandbox, live HTTP/SSE client, and reference math oracles.

100% Python 3.10+ standard library. Zero internal Rust imports, zero external packages.
"""

from __future__ import annotations

import dataclasses
import functools
import hashlib
import hmac
import http.client
import inspect
import json
import math
import os
import queue
import re
import shutil
import signal
import sqlite3
import struct
import subprocess
import tempfile
import threading
import time
import urllib.parse
import zlib
from dataclasses import dataclass, field
from pathlib import Path
from typing import Any, Callable, Dict, List, Optional, Sequence, Tuple, Union

REPO_ROOT: Path = Path(__file__).resolve().parents[2]
ALL_OPENBOT_FEATURES: Tuple[str, ...] = tuple(f"OB-F{i:02d}" for i in range(1, 21))

BUILTIN_AGENT_PROVIDERS: Tuple[str, ...] = (
    "codex",
    "claude",
    "grok",
    "opencode",
    "antigravity",
    "cursor",
    "cline",
    "pi",
    "muse",
    "inception",
    "deepseek",
    "openrouter",
    "hermes",
    "acp",
    "kimi",
    "devin",
    "copilot",
    "mistral",
)

AGENT_REASONING_EFFORTS: Tuple[str, ...] = (
    "none",
    "minimal",
    "low",
    "medium",
    "high",
    "xhigh",
    "max",
    "ultra",
)

SEEDED_AVATAR_SILHOUETTES: Tuple[str, ...] = (
    "cercle",
    "galet",
    "squircle",
    "capsule",
    "triangle",
    "hexagone",
    "nuage",
    "goutte",
    "carre",
    "losange",
    "pentagone",
)

SEEDED_SUPPORTED_SILHOUETTES: Tuple[str, ...] = (
    "cercle",
    "galet",
    "squircle",
    "capsule",
    "triangle",
    "hexagone",
    "nuage",
    "carre",
    "losange",
    "pentagone",
)

SUPPORTED_AVATAR_SILHOUETTES: Tuple[str, ...] = SEEDED_SUPPORTED_SILHOUETTES + (
    "fantome",
    "meduse",
    "poulpe",
)

AVATAR_EXPRESSIONS: Tuple[str, ...] = (
    "neutre",
    "attentif",
    "surpris",
    "excite",
    "heureux",
    "hilare",
    "colere",
    "triste",
    "effraye",
    "mefiant",
    "confus",
    "curieux",
    "fier",
    "timide",
    "blase",
    "somnolent",
)

AVATAR_STATES: Tuple[str, ...] = (
    "idle",
    "thinking",
    "wink",
    "wide",
    "alert",
    "notify",
    "exclaim",
    "sleep",
    "egg",
    "hexagon",
    "play",
    "orbit",
    "swirl",
    "burst",
    "comet",
)

AVATAR_MOODS: Tuple[str, ...] = (
    "idle",
    "working",
    "waiting",
    "failed",
    "responded",
    "connecting",
    "asleep",
)

AVATAR_HUES: Tuple[int, ...] = (0, 30, 55, 100, 150, 185, 215, 245, 280, 320)

AUTOMATIC_COLORS: Tuple[str, ...] = (
    "rouge",
    "orange",
    "ambre",
    "vert",
    "turquoise",
    "bleu",
    "violet",
    "rose",
)

COLOR_BY_ID: Dict[str, str] = {
    "rouge": "#e8483f",
    "orange": "#f08a24",
    "ambre": "#f0b429",
    "vert": "#3ecf8e",
    "turquoise": "#2fbfa0",
    "bleu": "#3b93f0",
    "violet": "#8b5cf6",
    "rose": "#e152b0",
    "encre": "#0a0a0c",
    "brun": "#8b5e3c",
    "gris": "#a3a3a3",
    "creme": "#f1efe9",
}

HUE_COLORS: Dict[int, str] = {
    0: "#e8483f",
    30: "#f08a24",
    55: "#f0b429",
    100: "#8ece34",
    150: "#3ecf8e",
    185: "#2fbfa0",
    215: "#3b93f0",
    245: "#5d67ee",
    280: "#8b5cf6",
    320: "#e152b0",
}

CHAT_BACKDROP_SCENES: Dict[str, Tuple[str, str, str]] = {
    # cli_name: (canonical_scene, light_hex, dark_hex)
    "pond": ("pond", "#e8f6f3", "#0f2426"),
    "forest": ("jellyfish", "#ebf3fa", "#0e1a2b"),
    "jellyfish": ("jellyfish", "#ebf3fa", "#0e1a2b"),
    "city": ("sunset", "#fbf0f3", "#1d1528"),
    "sunset": ("sunset", "#fbf0f3", "#1d1528"),
    "snow": ("snowfall", "#eef3f7", "#111a26"),
    "snowfall": ("snowfall", "#eef3f7", "#111a26"),
    "desert": ("fireflies", "#f2f5ea", "#121a14"),
    "fireflies": ("fireflies", "#f2f5ea", "#121a14"),
    "coast": ("coast", "#fdf0ec", "#1a1030"),
}


class BinaryNotReadyError(RuntimeError):
    """Raised when no compiled gpui-telegram-cli binary can be resolved on disk."""


GPUI_TELEGRAM_CLI_BIN: Optional[str] = os.environ.get("GPUI_TELEGRAM_CLI_BIN")
_CLI_BIN_OVERRIDE: Optional[str] = None
_MISSING = object()


def set_cli_bin_override(path: Optional[str]) -> None:
    """Override CLI binary resolution globally for the current test process."""
    global _CLI_BIN_OVERRIDE
    _CLI_BIN_OVERRIDE = path


def resolve_cli_bin(cli_bin: Optional[str] = None) -> Tuple[Optional[Path], List[str]]:
    """Resolve CLI executable in authoritative priority order; fail closed on explicit override."""
    for explicit in (cli_bin, _CLI_BIN_OVERRIDE, os.environ.get("GPUI_TELEGRAM_CLI_BIN")):
        if explicit is not None and explicit.strip():
            cand = Path(explicit.strip()).expanduser().resolve()
            checked = [str(cand)]
            if cand.exists() and cand.is_file() and os.access(cand, os.X_OK):
                return cand, checked
            return None, checked

    checked = []
    candidates: List[Path] = [
        REPO_ROOT / "target" / "debug" / "gpui-telegram-cli",
        REPO_ROOT / "target" / "debug" / "gpui_telegram_cli",
        REPO_ROOT / "target" / "release" / "gpui-telegram-cli",
        REPO_ROOT / "target" / "release" / "gpui_telegram_cli",
        REPO_ROOT / "target" / "debug" / "fabushi",
        REPO_ROOT / "target" / "release" / "fabushi",
    ]

    seen = set()
    for cand in candidates:
        key = str(cand)
        if key in seen:
            continue
        seen.add(key)
        checked.append(key)
        if cand.exists() and cand.is_file() and os.access(cand, os.X_OK):
            return cand, checked
    return None, checked


def get_cli_bin_or_raise(cli_bin: Optional[str] = None) -> Path:
    resolved, checked = resolve_cli_bin(cli_bin)
    if resolved is None:
        raise BinaryNotReadyError(
            "OpenBot CLI binary not found or not executable. Checked: "
            + ", ".join(checked)
        )
    return resolved


def to_snake_case(key: str) -> str:
    s1 = re.sub(r"(.)([A-Z][a-z]+)", r"\1_\2", key.replace("-", "_"))
    return re.sub(r"([a-z0-9])([A-Z])", r"\1_\2", s1).lower()


def to_camel_case(key: str) -> str:
    parts = re.split(r"[_\-]+", key)
    if not parts:
        return key
    return parts[0].lower() + "".join(p[:1].upper() + p[1:] for p in parts[1:] if p)


def normalize_key(key: str) -> str:
    return re.sub(r"[_\-]+", "", key).lower()


def unwrap_payload(raw_json: Any) -> Any:
    """Unwrap ok_envelope({'success': True, 'data': ...}) while preserving error objects."""
    if isinstance(raw_json, dict):
        if raw_json.get("success") is False or raw_json.get("ok") is False:
            return raw_json
        if (
            "data" in raw_json
            and raw_json["data"] is not None
            and ("success" in raw_json or "timestamp" in raw_json or "error" in raw_json)
        ):
            return raw_json["data"]
        if "result" in raw_json and raw_json["result"] is not None and "ok" in raw_json:
            return raw_json["result"]
    return raw_json


def _lookup_segment(mapping: Any, seg: str) -> Any:
    if not isinstance(mapping, dict):
        return _MISSING
    for variant in (
        seg,
        to_snake_case(seg),
        to_camel_case(seg),
        seg.replace("-", "_"),
        seg.replace("_", "-"),
    ):
        if variant in mapping:
            return mapping[variant]
    target_norm = normalize_key(seg)
    for k, v in mapping.items():
        if isinstance(k, str) and normalize_key(k) == target_norm:
            return v
    return _MISSING


def pick(obj: Any, *candidate_keys: str, default: Any = None) -> Any:
    """Extract a field across snake_case/camelCase and dotted paths from payload or envelope."""
    roots = [unwrap_payload(obj)]
    if obj is not roots[0]:
        roots.append(obj)
    for key_path in candidate_keys:
        segments = key_path.split(".")
        for root in roots:
            cur = root
            found = True
            for seg in segments:
                val = _lookup_segment(cur, seg)
                if val is _MISSING:
                    found = False
                    break
                cur = val
            if found:
                return cur
    return default


def unwrap_entity(res: Any, *wrapper_keys: str) -> Any:
    """Unwrap payload and return nested dict under wrapper_keys if present, else unwrapped payload."""
    raw = res.data if (hasattr(res, "data") and hasattr(res, "returncode")) else res
    d = unwrap_payload(raw)
    if isinstance(d, dict) and wrapper_keys:
        sub = pick(d, *wrapper_keys, default=None)
        if isinstance(sub, dict):
            return sub
    return d


def require_field(
    obj: Any,
    *candidate_keys: str,
    expected_type: Optional[Union[type, Tuple[type, ...]]] = None,
    allow_none: bool = False,
) -> Any:
    """Strictly extract and type-check a field, raising AssertionError on absence or mismatch."""
    val = pick(obj, *candidate_keys, default=_MISSING)
    preview = json.dumps(obj, default=str)[:400]
    if val is _MISSING:
        raise AssertionError(
            f"Missing required field {candidate_keys!r} in payload: {preview}"
        )
    allows_none = allow_none or (
        expected_type is not None
        and (
            expected_type is type(None)
            or (isinstance(expected_type, tuple) and type(None) in expected_type)
        )
    )
    if val is None and not allows_none:
        raise AssertionError(
            f"Field {candidate_keys!r} was unexpectedly None in payload: {preview}"
        )
    if expected_type is not None and val is not None:
        if expected_type is int and isinstance(val, bool):
            raise AssertionError(
                f"Field {candidate_keys!r} expected int, got bool ({val!r}) in {preview}"
            )
        if not isinstance(val, expected_type):
            raise AssertionError(
                f"Field {candidate_keys!r} expected {expected_type!r}, got {type(val).__name__} ({val!r}) in {preview}"
            )
    return val


def extract_list(obj: Any, *collection_keys: str) -> List[Any]:
    unwrapped = unwrap_payload(obj)
    if isinstance(unwrapped, list):
        return unwrapped
    keys = tuple(collection_keys) + ("items", "entries", "rows", "list")
    val = pick(obj, *keys, default=_MISSING)
    if isinstance(val, list):
        return val
    preview = json.dumps(obj, default=str)[:400]
    raise AssertionError(f"Expected list under {keys!r}, got: {preview}")


def extract_error_info(
    stdout_json: Any,
    stderr_json: Any,
    stderr_text: str,
    returncode: int,
    stdout_text: str = "",
) -> Dict[str, Any]:
    """Normalize CLI/HTTP/tool error envelopes into {'code', 'message', 'status', 'returncode'}."""
    code = ""
    message = ""
    status: Optional[int] = None
    for candidate in (stderr_json, stdout_json):
        if isinstance(candidate, dict):
            if status is None and isinstance(candidate.get("status"), int):
                status = candidate["status"]
            err = candidate.get("error_detail") or candidate.get("error")
            if isinstance(err, dict):
                code = code or str(err.get("code") or "")
                message = message or str(err.get("message") or "")
                if status is None and isinstance(err.get("status"), int):
                    status = err["status"]
            elif isinstance(err, str) and err:
                message = message or err
            if not code and isinstance(candidate.get("code"), str):
                code = candidate["code"]
            if not message:
                for msg_key in ("message", "reason"):
                    if isinstance(candidate.get(msg_key), str) and candidate[msg_key]:
                        message = candidate[msg_key]
                        break
            data_sub = candidate.get("data")
            if isinstance(data_sub, dict):
                if not code and isinstance(data_sub.get("code"), str):
                    code = data_sub["code"]
                for msg_key in ("reason", "message", "error"):
                    if not message and isinstance(data_sub.get(msg_key), str):
                        message = data_sub[msg_key]
    if not message:
        non_json_err = stderr_text.strip() if stderr_json is None else ""
        non_json_out = stdout_text.strip() if stdout_json is None else ""
        message = (non_json_err or non_json_out)[:1000]
    return {
        "code": code,
        "message": message,
        "status": status,
        "returncode": returncode,
    }


_PANIC_REGEX = re.compile(
    r"(?:thread '[^']*' panicked at|panicked at [^\n]+\.rs:\d+|stack backtrace:)",
    re.IGNORECASE,
)


def _parse_json_stream(text: str) -> Any:
    stripped = text.strip()
    if not stripped:
        return None
    try:
        return json.loads(stripped)
    except json.JSONDecodeError:
        pass
    for line in reversed(stripped.splitlines()):
        line_s = line.strip()
        if line_s.startswith("{") or line_s.startswith("["):
            try:
                return json.loads(line_s)
            except json.JSONDecodeError:
                continue
    return None


@dataclass
class CliRunResult:
    cmd: List[str]
    returncode: int
    stdout: str
    stderr: str
    raw_json: Any
    stderr_json: Any
    data: Any
    error: Dict[str, Any]
    panicked: bool
    duration_sec: float


def assert_no_rust_panic(res: CliRunResult) -> None:
    """Fail immediately if the Rust process panicked, aborted, or segfaulted."""
    if (
        res.panicked
        or res.returncode < 0
        or res.returncode in (101, 132, 134, 135, 136, 139)
        or _PANIC_REGEX.search(res.stdout)
        or _PANIC_REGEX.search(res.stderr)
    ):
        raise AssertionError(
            f"Rust process panic/crash detected (rc={res.returncode})!\n"
            f"CMD: {res.cmd}\nSTDOUT:\n{res.stdout[:800]}\nSTDERR:\n{res.stderr[:800]}"
        )


def run_openbot_cli(
    subcmd_args: Sequence[str],
    *,
    data_dir: Path,
    root_dir: Optional[Path] = None,
    config_dir: Optional[Path] = None,
    workspace_dir: Optional[Path] = None,
    cli_bin: Optional[str] = None,
    json_output: bool = True,
    check: bool = False,
    timeout: float = 20.0,
    stdin_text: Optional[str] = None,
    extra_env: Optional[Dict[str, str]] = None,
) -> CliRunResult:
    """Invoke `gpui-telegram-cli openbot ...` as a genuine isolated subprocess."""
    bin_path = get_cli_bin_or_raise(cli_bin)
    args_list = [str(a) for a in subcmd_args]
    cmd = [str(bin_path), "openbot", *args_list]
    if "--data-dir" not in args_list:
        cmd.extend(["--data-dir", str(data_dir)])
    if json_output and "--json" not in args_list:
        cmd.append("--json")

    eff_root = root_dir or data_dir.parent
    eff_cfg = config_dir or (eff_root / "config")
    eff_ws = workspace_dir or (eff_root / "OpenBot")

    env = dict(os.environ)
    env.update(
        {
            "HOME": str(eff_root),
            "XDG_CONFIG_HOME": str(eff_cfg),
            "XDG_DATA_HOME": str(data_dir),
            "OPENBOT_DATA_DIR": str(data_dir),
            "OPENBOT_WORKSPACE_DIR": str(eff_ws),
            "NO_COLOR": "1",
            "RUST_BACKTRACE": "1",
        }
    )
    if extra_env:
        env.update(extra_env)

    t0 = time.monotonic()
    proc = subprocess.run(
        cmd,
        input=stdin_text,
        text=True,
        capture_output=True,
        timeout=timeout,
        env=env,
        cwd=str(eff_root),
    )
    elapsed = time.monotonic() - t0

    stdout_json = _parse_json_stream(proc.stdout)
    stderr_json = _parse_json_stream(proc.stderr)
    effective_raw = stdout_json if stdout_json is not None else stderr_json
    panicked = bool(
        proc.returncode < 0
        or proc.returncode in (101, 132, 134, 135, 136, 139)
        or _PANIC_REGEX.search(proc.stdout)
        or _PANIC_REGEX.search(proc.stderr)
    )
    err_info = extract_error_info(
        stdout_json, stderr_json, proc.stderr, proc.returncode, proc.stdout
    )
    res = CliRunResult(
        cmd=cmd,
        returncode=proc.returncode,
        stdout=proc.stdout,
        stderr=proc.stderr,
        raw_json=effective_raw,
        stderr_json=stderr_json,
        data=unwrap_payload(effective_raw),
        error=err_info,
        panicked=panicked,
        duration_sec=elapsed,
    )
    assert_no_rust_panic(res)
    if check:
        if res.returncode != 0:
            raise AssertionError(
                f"Expected exit 0, got {res.returncode} for {res.cmd}\n"
                f"Error: {res.error}\nSTDOUT: {res.stdout[:600]}\nSTDERR: {res.stderr[:600]}"
            )
    return res


def run_cli_json(
    subcmd_args: Sequence[str],
    *,
    data_dir: Path,
    cli_bin: Optional[str] = None,
    timeout: float = 20.0,
    extra_env: Optional[Dict[str, str]] = None,
) -> Any:
    res = run_openbot_cli(
        subcmd_args,
        data_dir=data_dir,
        cli_bin=cli_bin,
        json_output=True,
        check=True,
        timeout=timeout,
        extra_env=extra_env,
    )
    if res.raw_json is None:
        raise AssertionError(f"CLI exited 0 but emitted non-JSON stdout: {res.stdout!r}")
    return res.data


def run_cli_raw(
    subcmd_args: Sequence[str],
    *,
    data_dir: Path,
    cli_bin: Optional[str] = None,
    json_output: bool = True,
    timeout: float = 20.0,
    extra_env: Optional[Dict[str, str]] = None,
) -> CliRunResult:
    res = run_openbot_cli(
        subcmd_args,
        data_dir=data_dir,
        cli_bin=cli_bin,
        json_output=json_output,
        check=False,
        timeout=timeout,
        extra_env=extra_env,
    )
    assert_no_rust_panic(res)
    return res


@dataclass
class SseEvent:
    event: str
    data: str
    json_data: Any
    event_id: Optional[str] = None


class SseStreamClient:
    """Incremental stdlib SSE stream reader over persistent http.client.HTTPConnection."""

    def __init__(
        self,
        host: str,
        port: int,
        path: str = "/v1/events",
        token: Optional[str] = None,
        last_event_id: Optional[str] = None,
        timeout: float = 10.0,
    ) -> None:
        self._conn = http.client.HTTPConnection(host, port, timeout=timeout)
        headers = {
            "Accept": "text/event-stream",
            "Cache-Control": "no-cache",
        }
        if token:
            headers["Authorization"] = f"Bearer {token}"
        if last_event_id:
            headers["Last-Event-ID"] = last_event_id
        self._conn.request("GET", path, headers=headers)
        self._resp = self._conn.getresponse()
        ctype = (self._resp.getheader("Content-Type") or "").lower()
        if self._resp.status != 200 or "text/event-stream" not in ctype:
            body = self._resp.read(512).decode("utf-8", errors="replace")
            self.close()
            raise AssertionError(
                f"SSE handshake failed: status={self._resp.status}, ctype={ctype!r}, body={body!r}"
            )

    def read_event(self, timeout: float = 5.0) -> Optional[SseEvent]:
        if self._conn.sock is not None:
            self._conn.sock.settimeout(timeout)
        ev_type = "message"
        ev_id: Optional[str] = None
        data_lines: List[str] = []
        deadline = time.monotonic() + timeout
        while time.monotonic() < deadline:
            raw = self._resp.fp.readline()
            if not raw:
                break
            line = raw.decode("utf-8", errors="replace").rstrip("\r\n")
            if not line:
                if data_lines or ev_id is not None or ev_type != "message":
                    joined = "\n".join(data_lines)
                    parsed = _parse_json_stream(joined)
                    return SseEvent(
                        event=ev_type, data=joined, json_data=parsed, event_id=ev_id
                    )
                continue
            if line.startswith(":"):
                continue
            if ":" in line:
                field_name, val = line.split(":", 1)
                val = val[1:] if val.startswith(" ") else val
            else:
                field_name, val = line, ""
            if field_name == "event":
                ev_type = val
            elif field_name == "data":
                data_lines.append(val)
            elif field_name == "id":
                ev_id = val
        return None

    def read_until(
        self, predicate: Callable[[SseEvent], bool], timeout: float = 8.0
    ) -> SseEvent:
        deadline = time.monotonic() + timeout
        seen: List[SseEvent] = []
        while time.monotonic() < deadline:
            rem = max(0.2, deadline - time.monotonic())
            ev = self.read_event(timeout=rem)
            if ev is None:
                continue
            seen.append(ev)
            if predicate(ev):
                return ev
        raise AssertionError(f"Timed out waiting for matching SSE event; seen={seen!r}")

    def close(self) -> None:
        try:
            self._conn.close()
        except OSError:
            pass

    def __enter__(self) -> "SseStreamClient":
        return self

    def __exit__(self, exc_type: Any, exc_val: Any, exc_tb: Any) -> None:
        self.close()


class OpenBotServerSession:
    """RAII live handle to `gpui-telegram-cli openbot serve` (/v1/* Team API + Automation)."""

    def __init__(
        self,
        bin_path: Path,
        data_dir: Path,
        root_dir: Path,
        bind: str = "127.0.0.1:0",
        startup_timeout: float = 10.0,
        extra_args: Sequence[str] = (),
    ) -> None:
        self._stdout_q: "queue.Queue[str]" = queue.Queue()
        self._stderr_lines: List[str] = []
        self._sse_clients: List[SseStreamClient] = []
        cmd = [
            str(bin_path),
            "openbot",
            "serve",
            "--data-dir",
            str(data_dir),
            "--bind",
            bind,
            *[str(a) for a in extra_args],
        ]
        env = dict(os.environ)
        env.update(
            {
                "HOME": str(root_dir),
                "OPENBOT_DATA_DIR": str(data_dir),
                "NO_COLOR": "1",
                "RUST_BACKTRACE": "1",
            }
        )
        self._proc = subprocess.Popen(
            cmd,
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
            text=True,
            bufsize=1,
            start_new_session=True,
            env=env,
            cwd=str(root_dir),
        )
        threading.Thread(
            target=self._drain_stdout, daemon=True, name="openbot-serve-stdout"
        ).start()
        threading.Thread(
            target=self._drain_stderr, daemon=True, name="openbot-serve-stderr"
        ).start()

        startup_obj: Any = None
        deadline = time.monotonic() + startup_timeout
        while time.monotonic() < deadline:
            if self._proc.poll() is not None:
                break
            try:
                line = self._stdout_q.get(timeout=0.15)
            except queue.Empty:
                continue
            parsed = _parse_json_stream(line)
            if isinstance(parsed, dict):
                cand = unwrap_payload(parsed)
                if isinstance(cand, dict) and pick(cand, "team_api_url") is not None:
                    startup_obj = cand
                    break

        if startup_obj is None:
            stderr_dump = "".join(self._stderr_lines)[:800]
            rc = self._proc.poll()
            self.stop()
            raise AssertionError(
                f"`openbot serve` failed to emit startup JSON within {startup_timeout}s "
                f"(rc={rc}). STDERR:\n{stderr_dump}"
            )

        self.team_api_url: str = str(require_field(startup_obj, "team_api_url", expected_type=str))
        self.automation_url: str = str(
            require_field(startup_obj, "automation_url", expected_type=str)
        )
        self.automation_token: str = str(
            require_field(startup_obj, "automation_token", expected_type=str)
        )
        t_parsed = urllib.parse.urlparse(self.team_api_url)
        a_parsed = urllib.parse.urlparse(self.automation_url)
        self.team_host = t_parsed.hostname or "127.0.0.1"
        self.team_port = int(t_parsed.port or 80)
        self.auto_host = a_parsed.hostname or "127.0.0.1"
        self.auto_port = int(a_parsed.port or 80)

        healthy = False
        while time.monotonic() < deadline:
            try:
                st, _, _ = self.http_team("GET", "/v1/health", timeout=1.0)
                if st == 200:
                    healthy = True
                    break
            except OSError:
                time.sleep(0.05)
        if not healthy:
            self.stop()
            raise AssertionError("`openbot serve` /v1/health did not return 200 before timeout")

    def _drain_stdout(self) -> None:
        assert self._proc.stdout is not None
        for line in self._proc.stdout:
            self._stdout_q.put(line)

    def _drain_stderr(self) -> None:
        assert self._proc.stderr is not None
        for line in self._proc.stderr:
            self._stderr_lines.append(line)

    def _raw_http(
        self,
        host: str,
        port: int,
        method: str,
        path: str,
        payload: Any = None,
        headers: Optional[Dict[str, str]] = None,
        custom_host: Optional[str] = None,
        timeout: float = 10.0,
    ) -> Tuple[int, Any, Dict[str, str]]:
        conn = http.client.HTTPConnection(host, port, timeout=timeout)
        try:
            req_headers: Dict[str, str] = dict(headers or {})
            body_bytes: Optional[bytes] = None
            if payload is not None:
                if isinstance(payload, (bytes, bytearray)):
                    body_bytes = bytes(payload)
                elif isinstance(payload, str):
                    body_bytes = payload.encode("utf-8")
                else:
                    body_bytes = json.dumps(payload).encode("utf-8")
                    req_headers.setdefault("Content-Type", "application/json")
            if custom_host is not None:
                req_headers["Host"] = custom_host
                conn.putrequest(method.upper(), path, skip_host=True)
                for hk, hv in req_headers.items():
                    conn.putheader(hk, hv)
                if body_bytes is not None and "Content-Length" not in req_headers:
                    conn.putheader("Content-Length", str(len(body_bytes)))
                conn.endheaders(body_bytes)
            else:
                conn.request(method.upper(), path, body=body_bytes, headers=req_headers)
            resp = conn.getresponse()
            raw_body = resp.read().decode("utf-8", errors="replace")
            resp_headers = {k.lower(): v for k, v in resp.getheaders()}
            parsed = _parse_json_stream(raw_body)
            return resp.status, (parsed if parsed is not None else raw_body), resp_headers
        finally:
            conn.close()

    def http_team(
        self,
        method: str,
        path: str,
        payload: Any = None,
        token: Optional[str] = None,
        headers: Optional[Dict[str, str]] = None,
        timeout: float = 10.0,
    ) -> Tuple[int, Any, Dict[str, str]]:
        hdrs = dict(headers or {})
        if token:
            hdrs["Authorization"] = f"Bearer {token}"
        return self._raw_http(
            self.team_host, self.team_port, method, path, payload=payload, headers=hdrs, timeout=timeout
        )

    def http_automation(
        self,
        method: str,
        path: str,
        payload: Any = None,
        token: Optional[str] = "__default__",
        headers: Optional[Dict[str, str]] = None,
        include_origin: Optional[str] = None,
        custom_host: Optional[str] = None,
        timeout: float = 10.0,
    ) -> Tuple[int, Any, Dict[str, str]]:
        hdrs = dict(headers or {})
        eff_token = self.automation_token if token == "__default__" else token
        if eff_token is not None:
            hdrs["Authorization"] = f"Bearer {eff_token}"
        if include_origin is not None:
            hdrs["Origin"] = include_origin
        eff_host = custom_host if custom_host is not None else f"127.0.0.1:{self.auto_port}"
        return self._raw_http(
            self.auto_host,
            self.auto_port,
            method,
            path,
            payload=payload,
            headers=hdrs,
            custom_host=eff_host,
            timeout=timeout,
        )

    def automation_run(
        self,
        agent_id: str,
        routine_id: Optional[str] = None,
        payload: str = "",
        wait_id: Optional[str] = None,
        client_message_id: Optional[str] = None,
        token: Optional[str] = "__default__",
    ) -> Tuple[int, Any, Dict[str, str]]:
        body: Dict[str, Any] = {
            "agent_id": agent_id,
            "agentId": agent_id,
            "payload": payload,
        }
        if routine_id is not None:
            body["routine_id"] = routine_id
            body["routineId"] = routine_id
        if wait_id is not None:
            body["wait_id"] = wait_id
        if client_message_id is not None:
            body["client_message_id"] = client_message_id
        st, data, hdrs = self.http_automation("POST", "/run", payload=body, token=token)
        if st == 404 and routine_id is not None:
            return self.http_automation(
                "POST",
                f"/v1/agents/{agent_id}/routines/{routine_id}/run",
                payload=body,
                token=token,
            )
        return st, data, hdrs

    def open_sse(
        self,
        path: str = "/v1/events",
        token: Optional[str] = None,
        last_event_id: Optional[str] = None,
        timeout: float = 10.0,
    ) -> SseStreamClient:
        client = SseStreamClient(
            self.team_host,
            self.team_port,
            path=path,
            token=token,
            last_event_id=last_event_id,
            timeout=timeout,
        )
        self._sse_clients.append(client)
        return client

    def stop(self) -> None:
        for sse in self._sse_clients:
            sse.close()
        self._sse_clients.clear()
        if self._proc.poll() is None:
            try:
                pgid = os.getpgid(self._proc.pid)
                os.killpg(pgid, signal.SIGTERM)
                self._proc.wait(timeout=2.0)
            except (ProcessLookupError, subprocess.TimeoutExpired, OSError):
                try:
                    pgid = os.getpgid(self._proc.pid)
                    os.killpg(pgid, signal.SIGKILL)
                except OSError:
                    self._proc.kill()
                try:
                    self._proc.wait(timeout=1.0)
                except subprocess.TimeoutExpired:
                    pass

    def __enter__(self) -> "OpenBotServerSession":
        return self

    def __exit__(self, exc_type: Any, exc_val: Any, exc_tb: Any) -> None:
        self.stop()


LiveServerHandle = OpenBotServerSession


def _png_chunk(chunk_type: bytes, data: bytes) -> bytes:
    crc = zlib.crc32(chunk_type + data) & 0xFFFFFFFF
    return struct.pack(">I", len(data)) + chunk_type + data + struct.pack(">I", crc)


def _encode_rgb_png(
    width: int, height: int, rgb: Tuple[int, int, int], noisy: bool = False
) -> bytes:
    ihdr = struct.pack(">IIBBBBB", width, height, 8, 2, 0, 0, 0)
    rows = bytearray()
    r, g, b = rgb
    for y in range(height):
        rows.append(0)  # filter None
        if noisy:
            for x in range(width):
                h = ((x * 2654435761) ^ (y * 2246822519)) & 0xFFFFFFFF
                rows.extend(((h & 0xFF), ((h >> 8) & 0xFF), ((h >> 16) & 0xFF)))
        else:
            rows.extend(bytes((r, g, b)) * width)
    compressed = zlib.compress(bytes(rows), level=1 if noisy else 6)
    return (
        b"\x89PNG\r\n\x1a\n"
        + _png_chunk(b"IHDR", ihdr)
        + _png_chunk(b"IDAT", compressed)
        + _png_chunk(b"IEND", b"")
    )


def _encode_minimal_animated_gif(frames: int = 4) -> bytes:
    # Minimal valid 2x2 GIF89a with NETSCAPE2.0 loop and `frames` graphic control + image blocks
    header = b"GIF89a" + struct.pack("<HHBBB", 2, 2, 0x80, 0, 0) + b"\x10\x20\x30\xe0\x50\x40"
    netscape = b"!\xff\x0bNETSCAPE2.0\x03\x01\x00\x00\x00"
    body = bytearray(header + netscape)
    for idx in range(max(1, frames)):
        gce = b"!\xf9\x04\x00" + struct.pack("<H", 8 + idx) + b"\x00\x00"
        img_desc = b"," + struct.pack("<HHHHB", 0, 0, 2, 2, 0)
        # LZW min code size 2 -> clear(4), 4 pixels (0 or 1), EOI(5)
        lzw = b"\x02\x02\x44\x01\x00" if (idx % 2 == 0) else b"\x02\x02\x4c\x01\x00"
        body.extend(gce + img_desc + lzw)
    body.append(0x3B)
    return bytes(body)


_CLI_PARSE_ERROR_PATTERNS: Tuple[str, ...] = (
    "unexpected argument",
    "unrecognized subcommand",
    "unknown option",
    "unknown argument",
    "invalid subcommand",
    "found argument '--",
    "required arguments were not provided",
    "unknown flag",
    "unknown global flag",
    "unknown subcommand",
    "unknown action",
    "unknown input field",
    "missing required argument",
    "requires an action subcommand",
    "requires a value",
    "does not take a value",
    "unexpected extra",
    "usage error:",
    "option '--",
)


def _assert_not_cli_parse_error(res: CliRunResult) -> None:
    err_code = str((res.error or {}).get("code") or "").strip().lower()
    if err_code in ("unknown_command", "usage_error"):
        raise AssertionError(
            f"CLI failed with {err_code} rather than domain validation/refusal for {res.cmd}!\n"
            f"error={res.error}\nstdout={res.stdout[:500]}\nstderr={res.stderr[:500]}"
        )
    combined = f"{res.stdout}\n{res.stderr}\n{(res.error or {}).get('message') or ''}".lower()
    for pat in _CLI_PARSE_ERROR_PATTERNS:
        if pat in combined:
            raise AssertionError(
                f"CLI failed with parser/argument error ({pat!r}) rather than domain validation/refusal for {res.cmd}!\n"
                f"error={res.error}\nstdout={res.stdout[:500]}\nstderr={res.stderr[:500]}"
            )


assert_not_cli_parse_error = _assert_not_cli_parse_error


_GENERIC_CLI_ERROR_CODES = frozenset(
    {
        "usage_error",
        "unknown_command",
        "validation_error",
        "unauthorized",
        "approval_required",
        "forbidden",
        "http_error",
        "server_error",
        "execution_error",
        "check_error",
    }
)
_GENERIC_CLI_ERROR_PREFIXES: Tuple[str, ...] = (
    "input validation error:",
    "action denied or forbidden:",
    "authentication required or rejected:",
    "http/network error:",
    "execution error:",
    "check error:",
)


def _strip_generic_cli_prefix(msg: str) -> str:
    out = msg.strip()
    low = out.lower()
    for pfx in _GENERIC_CLI_ERROR_PREFIXES:
        if low.startswith(pfx):
            return out[len(pfx) :].strip()
    return out


def _assert_error_contains(
    res: CliRunResult,
    error_contains: Optional[Union[str, Sequence[str]]],
    extra_haystack: str = "",
    allow_cli_parse_error: bool = False,
) -> None:
    if error_contains is None:
        return
    if allow_cli_parse_error:
        parts = [
            str(res.error.get("code") or ""),
            str(res.error.get("message") or ""),
            extra_haystack,
            res.stdout,
            res.stderr,
        ]
    else:
        clean_msg = _strip_generic_cli_prefix(str(res.error.get("message") or ""))
        non_json_stderr = res.stderr if res.stderr_json is None else ""
        non_json_stdout = res.stdout if res.raw_json is None else ""
        parts = [clean_msg, extra_haystack]
        if not clean_msg and not extra_haystack.strip():
            parts.extend([non_json_stderr, non_json_stdout])
        elif non_json_stderr.strip():
            parts.append(non_json_stderr)
    haystack = " | ".join(parts).lower()
    patterns = [error_contains] if isinstance(error_contains, str) else list(error_contains)
    if not any(p.lower() in haystack for p in patterns):
        raise AssertionError(
            f"Expected error matching {patterns!r}, got:\n"
            f"error={res.error}\nstdout={res.stdout[:500]}\nstderr={res.stderr[:500]}"
        )


assert_error_contains = _assert_error_contains


class TempOpenBotEnv:
    """Hermetic per-test RAII sandbox backed by tempfile.TemporaryDirectory."""

    def __init__(
        self, prefix: str = "openbot_e2e_", cli_bin: Optional[str] = None
    ) -> None:
        self._prefix = prefix
        self._cli_bin = cli_bin
        self._tmpdir: Optional[tempfile.TemporaryDirectory[str]] = None
        self._servers: List[OpenBotServerSession] = []
        self._sqlite_conns: List[sqlite3.Connection] = []
        self.root: Path = Path()
        self.data_dir: Path = Path()
        self.workspace_dir: Path = Path()
        self.config_dir: Path = Path()

    def __enter__(self) -> "TempOpenBotEnv":
        base_dir = os.environ.get("OPENBOT_E2E_TMPDIR")
        self._tmpdir = tempfile.TemporaryDirectory(prefix=self._prefix, dir=base_dir)
        self.root = Path(self._tmpdir.name)
        self.data_dir = self.root / "data"
        self.workspace_dir = self.root / "OpenBot"
        self.config_dir = self.root / "config"
        self.data_dir.mkdir(parents=True, exist_ok=True, mode=0o700)
        os.chmod(self.data_dir, 0o700)
        (self.data_dir / "Shared" / "Data").mkdir(parents=True, exist_ok=True, mode=0o700)
        (self.data_dir / "themes").mkdir(parents=True, exist_ok=True)
        for sub in ("Agents", "Shared/Data", "Downloads"):
            (self.workspace_dir / sub).mkdir(parents=True, exist_ok=True, mode=0o700)
        (self.config_dir / "OpenBot" / "themes").mkdir(parents=True, exist_ok=True)
        return self

    def __exit__(self, exc_type: Any, exc_val: Any, exc_tb: Any) -> None:
        for srv in reversed(self._servers):
            srv.stop()
        self._servers.clear()
        for conn in self._sqlite_conns:
            try:
                conn.close()
            except sqlite3.Error:
                pass
        self._sqlite_conns.clear()
        if self._tmpdir is not None:
            try:
                self._tmpdir.cleanup()
            except OSError:
                shutil.rmtree(self.root, ignore_errors=True)
            self._tmpdir = None

    @property
    def openbot_db_path(self) -> Path:
        primary = self.data_dir / "openbot.db"
        alt = self.workspace_dir / "openbot.db"
        if not primary.exists() and alt.exists():
            return alt
        return primary

    @property
    def agent_data_db_path(self) -> Path:
        for cand in (
            self.data_dir / "Shared" / "Data" / "agent-data.db",
            self.workspace_dir / "Shared" / "Data" / "agent-data.db",
            self.data_dir / "agent-data.db",
        ):
            if cand.exists():
                return cand
        return self.data_dir / "Shared" / "Data" / "agent-data.db"

    def run_cli(
        self,
        *subcmd_args: str,
        json_output: bool = True,
        check: bool = False,
        timeout: float = 20.0,
        stdin_text: Optional[str] = None,
        extra_env: Optional[Dict[str, str]] = None,
    ) -> CliRunResult:
        return run_openbot_cli(
            subcmd_args,
            data_dir=self.data_dir,
            root_dir=self.root,
            config_dir=self.config_dir,
            workspace_dir=self.workspace_dir,
            cli_bin=self._cli_bin,
            json_output=json_output,
            check=check,
            timeout=timeout,
            stdin_text=stdin_text,
            extra_env=extra_env,
        )

    def run_cli_json(
        self,
        *subcmd_args: str,
        timeout: float = 20.0,
        extra_env: Optional[Dict[str, str]] = None,
    ) -> Any:
        res = self.run_cli(
            *subcmd_args, json_output=True, check=True, timeout=timeout, extra_env=extra_env
        )
        if res.raw_json is None:
            raise AssertionError(f"Expected JSON stdout from {res.cmd}, got {res.stdout!r}")
        return res.data

    def run_cli_raw(
        self,
        *subcmd_args: str,
        json_output: bool = True,
        timeout: float = 20.0,
        extra_env: Optional[Dict[str, str]] = None,
    ) -> CliRunResult:
        res = self.run_cli(
            *subcmd_args, json_output=json_output, check=False, timeout=timeout, extra_env=extra_env
        )
        assert_no_rust_panic(res)
        return res

    def run_cli_expect_error(
        self,
        *subcmd_args: str,
        expected_exit_codes: Optional[Sequence[int]] = None,
        error_contains: Optional[Union[str, Sequence[str]]] = None,
        allow_cli_parse_error: bool = False,
        timeout: float = 20.0,
    ) -> CliRunResult:
        res = self.run_cli(*subcmd_args, json_output=True, check=False, timeout=timeout)
        assert_no_rust_panic(res)
        if res.returncode == 0:
            raise AssertionError(
                f"Expected non-zero exit code for {res.cmd}, got 0!\nSTDOUT: {res.stdout[:500]}"
            )
        if not allow_cli_parse_error:
            _assert_not_cli_parse_error(res)
        if expected_exit_codes is not None and res.returncode not in expected_exit_codes:
            raise AssertionError(
                f"Expected exit code in {expected_exit_codes}, got {res.returncode} for {res.cmd}"
            )
        _assert_error_contains(
            res, error_contains, allow_cli_parse_error=allow_cli_parse_error
        )
        return res

    def run_cli_expect_rejection(
        self,
        *subcmd_args: str,
        allow_soft_refusal: bool = True,
        error_contains: Optional[Union[str, Sequence[str]]] = None,
        allow_cli_parse_error: bool = False,
        timeout: float = 20.0,
    ) -> CliRunResult:
        """Verify either non-zero CLI error OR structured tool/task refusal without panic."""
        res = self.run_cli(*subcmd_args, json_output=True, check=False, timeout=timeout)
        assert_no_rust_panic(res)
        if not allow_cli_parse_error:
            _assert_not_cli_parse_error(res)
        soft_refused = False
        extra_msg = ""
        if allow_soft_refusal and res.raw_json is not None:
            payload = res.data
            accepted = pick(payload, "accepted")
            ok_flag = pick(payload, "ok")
            state = pick(payload, "state", "task.state")
            raw_success = (
                res.raw_json.get("success") if isinstance(res.raw_json, dict) else None
            )
            if (
                accepted is False
                or ok_flag is False
                or raw_success is False
                or state == "paused"
            ):
                soft_refused = True
            for k in ("reason", "message", "task.error"):
                v = pick(payload, k)
                if isinstance(v, str) and v.strip():
                    extra_msg += f" {_strip_generic_cli_prefix(v)}"
            err_v = pick(payload, "error")
            if (
                isinstance(err_v, str)
                and err_v.strip()
                and err_v.strip().lower() != "approval required"
            ):
                extra_msg += f" {_strip_generic_cli_prefix(err_v)}"
            code_v = pick(payload, "code")
            if (
                isinstance(code_v, str)
                and code_v.strip()
                and code_v.strip().lower() not in _GENERIC_CLI_ERROR_CODES
            ):
                extra_msg += f" {code_v.strip()}"
            if state == "paused":
                extra_msg += " paused"
            if pick(payload, "steer_refused") is True:
                extra_msg += " steer_refused"
        if res.returncode == 0 and not soft_refused:
            raise AssertionError(
                f"Expected error/refusal for {res.cmd}, but exited 0 with accepted payload: "
                f"{res.stdout[:500]}"
            )
        _assert_error_contains(
            res,
            error_contains,
            extra_haystack=extra_msg,
            allow_cli_parse_error=allow_cli_parse_error,
        )
        return res

    def start_server(
        self,
        bind: str = "127.0.0.1:0",
        startup_timeout: float = 10.0,
        extra_args: Sequence[str] = (),
    ) -> OpenBotServerSession:
        bin_path = get_cli_bin_or_raise(self._cli_bin)
        srv = OpenBotServerSession(
            bin_path=bin_path,
            data_dir=self.data_dir,
            root_dir=self.root,
            bind=bind,
            startup_timeout=startup_timeout,
            extra_args=extra_args,
        )
        self._servers.append(srv)
        return srv

    def open_sqlite(self, db_kind: str = "openbot") -> sqlite3.Connection:
        db_path = self.openbot_db_path if db_kind == "openbot" else self.agent_data_db_path
        if not db_path.exists():
            raise AssertionError(f"Expected SQLite database {db_kind!r} at {db_path}, not found")
        conn = sqlite3.connect(str(db_path), timeout=5.0)
        conn.row_factory = sqlite3.Row
        self._sqlite_conns.append(conn)
        return conn

    def query_sqlite(
        self, sql: str, params: Sequence[Any] = (), db_kind: str = "openbot"
    ) -> List[Dict[str, Any]]:
        conn = self.open_sqlite(db_kind=db_kind)
        cur = conn.execute(sql, tuple(params))
        return [dict(row) for row in cur.fetchall()]

    def check_sqlite_integrity(self, db_kind: str = "openbot") -> None:
        conn = self.open_sqlite(db_kind=db_kind)
        fk_rows = conn.execute("PRAGMA foreign_key_check").fetchall()
        if fk_rows:
            raise AssertionError(f"SQLite foreign_key_check failed on {db_kind}: {fk_rows!r}")
        qc_rows = [r[0] for r in conn.execute("PRAGMA quick_check(20)").fetchall()]
        if qc_rows != ["ok"]:
            raise AssertionError(f"SQLite quick_check(20) failed on {db_kind}: {qc_rows!r}")

    def write_fixture(
        self, rel_path: str, content: Union[str, bytes], mode: Optional[int] = None
    ) -> Path:
        target = self.root / rel_path
        target.parent.mkdir(parents=True, exist_ok=True)
        if isinstance(content, bytes):
            target.write_bytes(content)
        else:
            target.write_text(content, encoding="utf-8")
        if mode is not None:
            os.chmod(target, mode)
        return target

    def write_png_fixture(
        self,
        rel_path: str,
        width: int = 64,
        height: int = 64,
        rgb: Tuple[int, int, int] = (59, 147, 240),
        noisy: bool = False,
    ) -> Path:
        return self.write_fixture(
            rel_path, _encode_rgb_png(width, height, rgb=rgb, noisy=noisy)
        )

    def write_gif_fixture(self, rel_path: str, frames: int = 4) -> Path:
        return self.write_fixture(rel_path, _encode_minimal_animated_gif(frames=frames))


OpenBotE2EEnv = TempOpenBotEnv


@dataclass
class E2ETestCase:
    id: str
    tier: int
    features: Tuple[str, ...]
    description: str
    fn: Callable[..., Any]
    module: str
    qualname: str
    tags: Tuple[str, ...] = ()
    timeout_sec: float = 30.0


TEST_REGISTRY: Dict[str, E2ETestCase] = {}


def e2e_test(
    id: Optional[str] = None,
    tier: int = 1,
    features: Sequence[str] = (),
    description: str = "",
    *,
    test_id: Optional[str] = None,
    tags: Sequence[str] = (),
    timeout_sec: float = 30.0,
) -> Callable[[Callable[..., Any]], Callable[..., Any]]:
    """Register an opaque-box E2E test and auto-wrap single-arg tests in TempOpenBotEnv."""
    eff_id = (id or test_id or "").strip()
    if not eff_id:
        raise ValueError("@e2e_test requires a non-empty `id` / `test_id`")
    if tier not in (1, 2, 3, 4):
        raise ValueError(f"@e2e_test({eff_id}): invalid tier {tier!r}, expected 1..4")
    feat_tuple = tuple(dict.fromkeys(f.strip() for f in features if f.strip()))
    if not feat_tuple:
        raise ValueError(f"@e2e_test({eff_id}): `features` must not be empty")
    unknown = [f for f in feat_tuple if f not in ALL_OPENBOT_FEATURES]
    if unknown:
        raise ValueError(f"@e2e_test({eff_id}): unknown feature IDs {unknown!r}")
    if tier == 3 and len(feat_tuple) < 2:
        raise ValueError(f"@e2e_test({eff_id}): Tier 3 tests must tag >= 2 distinct features")
    if len(description.strip()) < 10:
        raise ValueError(f"@e2e_test({eff_id}): `description` must be >= 10 chars")

    def decorator(fn: Callable[..., Any]) -> Callable[..., Any]:
        if eff_id in TEST_REGISTRY:
            prev = TEST_REGISTRY[eff_id]
            raise ValueError(
                f"Duplicate @e2e_test ID {eff_id!r} in {fn.__module__}.{fn.__qualname__} "
                f"(already registered by {prev.module}.{prev.qualname})"
            )
        sig = inspect.signature(fn)
        params = [
            p
            for p in sig.parameters.values()
            if p.kind in (p.POSITIONAL_ONLY, p.POSITIONAL_OR_KEYWORD)
        ]

        @functools.wraps(fn)
        def wrapper(*args: Any, **kwargs: Any) -> Any:
            if args or kwargs or len(params) == 0:
                return fn(*args, **kwargs)
            with TempOpenBotEnv() as env:
                return fn(env)

        TEST_REGISTRY[eff_id] = E2ETestCase(
            id=eff_id,
            tier=tier,
            features=feat_tuple,
            description=description.strip(),
            fn=wrapper,
            module=fn.__module__,
            qualname=fn.__qualname__,
            tags=tuple(tags),
            timeout_sec=timeout_sec,
        )
        return wrapper

    return decorator


# ============================================================================
# Closed-Form Reference Math, Color & Crypto Oracles (1:1 Upstream Parity)
# ============================================================================


def fnv1a_32_utf16(value: str) -> int:
    """Exact 32-bit FNV-1a over UTF-16 code units (packages/brand/src/bloub-avatar.ts:274)."""
    h = 0x811C9DC5
    raw = value.encode("utf-16-le")
    for idx in range(0, len(raw), 2):
        unit = raw[idx] | (raw[idx + 1] << 8)
        h ^= unit
        h = (h * 0x01000193) & 0xFFFFFFFF
    return h


def stable_index(value: str, length: int) -> int:
    if length <= 0:
        raise ValueError("length must be > 0")
    return fnv1a_32_utf16(value) % length


def resolve_bloub_avatar_profile_oracle(
    seed: str,
    hue: Optional[int] = None,
    color: Optional[str] = None,
    coerce_empty_to_openbot: bool = False,
) -> Dict[str, str]:
    """Exact bloubAvatarProfile resolver including index-7 'goutte' replacement."""
    eff_seed = "openbot" if (seed == "" and coerce_empty_to_openbot) else seed
    style = "flat" if re.search(r":flat:\d+$", eff_seed) else "plush"
    m_named = re.search(r":(fantome|meduse|poulpe):(?:face|flat):\d+$", eff_seed)
    if m_named:
        shape = m_named.group(1)
    else:
        raw_shape = SEEDED_AVATAR_SILHOUETTES[stable_index(f"{eff_seed}:shape", 11)]
        if raw_shape == "goutte":
            shape = SEEDED_SUPPORTED_SILHOUETTES[
                stable_index(f"{eff_seed}:shape:replacement", 10)
            ]
        else:
            shape = raw_shape
    expression = AVATAR_EXPRESSIONS[stable_index(f"{eff_seed}:expression", 16)]
    if color is not None:
        resolved_color = color.strip().lower()
    elif hue is not None:
        if hue not in HUE_COLORS:
            raise ValueError(f"Unsupported AvatarHue: {hue}")
        resolved_color = HUE_COLORS[hue]
    else:
        color_id = AUTOMATIC_COLORS[stable_index(f"{eff_seed}:color", 8)]
        resolved_color = COLOR_BY_ID[color_id]
    return {
        "style": style,
        "shape": shape,
        "expression": expression,
        "color": resolved_color,
    }


def nearest_avatar_hue_oracle(color: str) -> Optional[int]:
    """Exact nearestAvatarHue (ipc-agent-identity.ts:37-66; sat < 0.15 -> None)."""
    c = color.strip().lower()
    if not re.fullmatch(r"#[0-9a-f]{6}", c):
        raise ValueError(f"Invalid hex color: {color!r}")
    r = int(c[1:3], 16) / 255.0
    g = int(c[3:5], 16) / 255.0
    b = int(c[5:7], 16) / 255.0
    mx, mn = max(r, g, b), min(r, g, b)
    delta = mx - mn
    lightness = (mx + mn) / 2.0
    saturation = 0.0 if delta == 0.0 else delta / (1.0 - abs(2.0 * lightness - 1.0))
    if saturation < 0.15:
        return None
    if mx == r:
        sector = ((g - b) / delta) % 6.0
    elif mx == g:
        sector = ((b - r) / delta) + 2.0
    else:
        sector = ((r - g) / delta) + 4.0
    angle = (sector * 60.0) % 360.0
    best_hue = AVATAR_HUES[0]
    best_dist = 360.0
    for h in AVATAR_HUES:
        diff = abs(angle - h)
        dist = min(diff, 360.0 - diff)
        if dist < best_dist:
            best_dist = dist
            best_hue = h
    return best_hue


def mulberry32_blink_schedule_oracle(
    seed: int = 24301, limit_sec: float = 900.0
) -> List[float]:
    """Exact Mulberry32 blink timestamp generator (@norbert_bodziony/bloub@0.7.0)."""
    state = seed & 0xFFFFFFFF

    def rand() -> float:
        nonlocal state
        state = (state + 0x6D2B79F5) & 0xFFFFFFFF
        t = state
        t = ((t ^ (t >> 15)) * (t | 1)) & 0xFFFFFFFF
        t = (t ^ (t + (((t ^ (t >> 7)) * (t | 61)) & 0xFFFFFFFF))) & 0xFFFFFFFF
        return ((t ^ (t >> 14)) & 0xFFFFFFFF) / 4294967296.0

    schedule: List[float] = []
    t_cur = 1.4
    while t_cur < limit_sec:
        schedule.append(t_cur)
        t_cur += 1.9 + rand() * 2.7
        if rand() < 0.18:
            schedule.append(t_cur)
            t_cur += 0.24
    return schedule


def blink_open_squash_oracle(
    t_sec: float, rest_open: float = 1.0, seed: int = 24301
) -> float:
    """Compute squashed eye openness I(open) = 0.06 + 0.94 * clamp01(open * mult)."""
    mult = 1.0
    for start in mulberry32_blink_schedule_oracle(seed=seed, limit_sec=max(20.0, t_sec + 2.0)):
        if start <= t_sec <= start + 0.18:
            r = (t_sec - start) / 0.18
            f = (1.0 - r / 0.45) if r < 0.45 else ((r - 0.45) / 0.55)
            mult = min(mult, max(0.0, min(1.0, f)))
    raw_open = max(0.0, min(1.0, rest_open * mult))
    return 0.06 + 0.94 * raw_open


def srgb_to_linear(v: float) -> float:
    return v / 12.92 if v <= 0.04045 else ((v + 0.055) / 1.055) ** 2.4


def linear_to_srgb(v: float) -> float:
    clamped = max(0.0, min(1.0, v))
    return (
        clamped * 12.92
        if clamped <= 0.0031308
        else 1.055 * (clamped ** (1.0 / 2.4)) - 0.055
    )


def _js_round(val: float) -> int:
    return math.floor(val + 0.5)


def plush_srgb_tint_pixel_oracle(
    hex_color: str, grey_byte: int
) -> Tuple[int, int, int, int]:
    """Exact plush-felt.ts:393-413 RGBA tinting ramp (GREY=0.55, gamma=0.6, sheen=0.08)."""
    c = hex_color.strip().lstrip("#").lower()
    ink = [srgb_to_linear(int(c[i : i + 2], 16) / 255.0) for i in (0, 2, 4)]
    g_norm = max(0, min(255, grey_byte)) / 255.0
    light = (srgb_to_linear(g_norm) / srgb_to_linear(0.55)) ** 0.6
    sheen = 0.08 * (max(0.0, (g_norm - 0.55) / 0.45) ** 2)
    rgba: List[int] = []
    for ch in ink:
        tinted = min(1.0, ch * light)
        mixed = tinted + (1.0 - tinted) * sheen
        rgba.append(max(0, min(255, _js_round(255.0 * linear_to_srgb(mixed)))))
    rgba.append(255)
    return (rgba[0], rgba[1], rgba[2], rgba[3])


def oklab_readable_chat_bg_oracle(hex_color: str, scheme: str) -> str:
    """Exact packages/brand/src/chat-colors.ts:70-96 OKLab readableChatBackground."""
    c = hex_color.strip().lstrip("#").lower()
    r, g, b = [srgb_to_linear(int(c[i : i + 2], 16) / 255.0) for i in (0, 2, 4)]
    cbrt = lambda x: math.copysign(abs(x) ** (1.0 / 3.0), x)
    l = cbrt(0.4122214708 * r + 0.5363325363 * g + 0.0514459929 * b)
    m = cbrt(0.2119034982 * r + 0.6806995451 * g + 0.1073969566 * b)
    s = cbrt(0.0883024619 * r + 0.2817188376 * g + 0.6299787005 * b)
    L = 0.2104542553 * l + 0.7936177850 * m - 0.0040720468 * s
    a = 1.9779984951 * l - 2.4285922050 * m + 0.4505937099 * s
    bb = 0.0259040371 * l + 0.7827717662 * m - 0.8086757660 * s
    chroma = math.hypot(a, bb)
    if scheme == "light":
        L2, C2 = max(L, 0.95), min(chroma, 0.02)
    else:
        L2, C2 = min(L, 0.30), min(chroma, 0.05)
    scale = (C2 / chroma) if chroma > 1e-9 else 0.0
    a2, b2 = a * scale, bb * scale
    l3 = (L2 + 0.3963377774 * a2 + 0.2158037573 * b2) ** 3
    m3 = (L2 - 0.1055613458 * a2 - 0.0638541728 * b2) ** 3
    s3 = (L2 - 0.0894841775 * a2 - 1.2914855480 * b2) ** 3
    rr = +4.0767416621 * l3 - 3.3077115913 * m3 + 0.2309699292 * s3
    gg = -1.2684380046 * l3 + 2.6097574011 * m3 - 0.3413193965 * s3
    bb2 = -0.0041960863 * l3 - 0.7034186147 * m3 + 1.7076147010 * s3
    rgb_out = [max(0, min(255, _js_round(255.0 * linear_to_srgb(v)))) for v in (rr, gg, bb2)]
    return f"#{rgb_out[0]:02x}{rgb_out[1]:02x}{rgb_out[2]:02x}"


def channel_cluster_layout_oracle(member_count: int) -> Dict[str, Any]:
    """Exact ChannelAvatar.tsx:67-73 cluster partitioning."""
    if member_count <= 0:
        return {"shown_faces": 0, "hidden_count": 0, "plus_pill": None, "fallback": "UsersRound"}
    if member_count <= 4:
        return {"shown_faces": member_count, "hidden_count": 0, "plus_pill": None, "fallback": None}
    if member_count - 3 < 10:
        hidden = member_count - 3
        return {"shown_faces": 3, "hidden_count": hidden, "plus_pill": f"+{hidden}", "fallback": None}
    hidden = member_count - 2
    return {"shown_faces": 2, "hidden_count": hidden, "plus_pill": f"+{hidden}", "fallback": None}


def dm_thread_id_oracle(member_a: str, member_b: str) -> str:
    """Exact TeamChatStore directThreadId ('dm-' + sha256(sorted_a + '\\0' + sorted_b)[:32])."""
    lo, hi = sorted((member_a, member_b))
    digest = hashlib.sha256(f"{lo}\x00{hi}".encode("utf-8")).hexdigest()[:32]
    return f"dm-{digest}"


def webhook_hmac_sha256_oracle(secret: str, payload: Union[str, bytes]) -> str:
    raw = payload.encode("utf-8") if isinstance(payload, str) else payload
    return hmac.new(secret.encode("utf-8"), raw, hashlib.sha256).hexdigest()
