"""Subprocess lifecycle manager for gpui_telegram_server and isolated fabushi CLI runner.

Both runners FAIL CLOSED with explicit AssertionError (NEVER pytest.skip) when a
compiled binary is missing or fails to boot, enforcing non-vacuous E2E verification.
"""

from __future__ import annotations

import json
import os
import signal
import socket
import subprocess
import tempfile
import time
import uuid
from dataclasses import dataclass
from pathlib import Path
from typing import Any

from .config import CLI_BIN, SERVER_BIN
from .transport import OrcaApiClient

AMBIENT_CLI_PREFIXES: tuple[str, ...] = (
    "FABUSHI_",
    "GPUI_TELEGRAM_",
    "GPUI_TG_",
)


def allocate_free_port() -> int:
    """Bind an ephemeral loopback port on 127.0.0.1:0 and return the free port number."""
    with socket.socket(socket.AF_INET, socket.SOCK_STREAM) as sock:
        sock.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
        sock.bind(("127.0.0.1", 0))
        return int(sock.getsockname()[1])


@dataclass(frozen=True)
class AuthSession:
    """Authenticated test user credentials and bearer session token."""

    user_id: int
    username: str
    token: str
    session_id: str
    password: str = ""


class OrcaServerHandle:
    """Manages an isolated gpui_telegram_server subprocess or external server URL."""

    def __init__(
        self,
        server_bin: Path | None = None,
        external_url: str | None = None,
        startup_timeout: float = 20.0,
        extra_env: dict[str, str] | None = None,
    ) -> None:
        self.server_bin: Path = Path(server_bin) if server_bin is not None else SERVER_BIN
        self.external_url: str | None = external_url.rstrip("/") if external_url else None
        self.startup_timeout: float = startup_timeout
        self.extra_env: dict[str, str] = dict(extra_env or {})

        self._tempdir: tempfile.TemporaryDirectory[str] | None = None
        self._proc: subprocess.Popen[bytes] | None = None
        self._log_file: Any = None
        self._port: int | None = None
        self._started: bool = False

        if not self.external_url:
            self._tempdir = tempfile.TemporaryDirectory(prefix="orca_e2e_srv_")
            self._data_dir = Path(self._tempdir.name)
            self._media_dir = self._data_dir / "media"
            self._media_dir.mkdir(parents=True, exist_ok=True)
            self._db_path = self._data_dir / "e2e.db"
            self._log_path = self._data_dir / "server.log"
        else:
            self._data_dir = Path(tempfile.gettempdir())
            self._media_dir = self._data_dir
            self._db_path = self._data_dir / "external.db"
            self._log_path = self._data_dir / "external_server.log"

    @property
    def is_external(self) -> bool:
        return self.external_url is not None

    @property
    def port(self) -> int | None:
        return self._port

    @property
    def data_dir(self) -> Path:
        return self._data_dir

    @property
    def db_path(self) -> Path:
        return self._db_path

    @property
    def media_dir(self) -> Path:
        return self._media_dir

    @property
    def log_path(self) -> Path:
        return self._log_path

    @property
    def base_url(self) -> str:
        if self.external_url:
            return self.external_url
        if self._port is None:
            raise AssertionError("OrcaServerHandle has not been started yet")
        return f"http://127.0.0.1:{self._port}"

    @property
    def ws_url(self) -> str:
        url = self.base_url
        if url.startswith("https://"):
            return "wss://" + url[len("https://") :]
        if url.startswith("http://"):
            return "ws://" + url[len("http://") :]
        return url

    def tail_logs(self, max_bytes: int = 8192) -> str:
        if self.is_external or not self._log_path.is_file():
            return "<no local server logs>"
        data = self._log_path.read_bytes()
        if len(data) > max_bytes:
            data = data[-max_bytes:]
        return data.decode("utf-8", errors="replace")

    def _poll_health(self, timeout: float) -> None:
        deadline = time.monotonic() + timeout
        last_err: str = "no response yet"
        client = OrcaApiClient(self.base_url, timeout=2.0)
        while time.monotonic() < deadline:
            if self._proc is not None:
                rc = self._proc.poll()
                if rc is not None:
                    if self._log_file is not None:
                        try:
                            self._log_file.flush()
                        except Exception:
                            pass
                    logs = self.tail_logs()
                    self._terminate_process()
                    raise AssertionError(
                        f"gpui_telegram_server exited prematurely with code {rc} "
                        f"before /health became ready.\nLogs:\n{logs}"
                    )
            try:
                resp = client.get("/health")
                if resp.status == 200:
                    return
                last_err = f"HTTP {resp.status} {resp.reason}: {resp.text[:200]!r}"
            except Exception as exc:
                last_err = f"{type(exc).__name__}: {exc}"
            time.sleep(0.05)

        if self._log_file is not None:
            try:
                self._log_file.flush()
            except Exception:
                pass
        logs = self.tail_logs()
        self._terminate_process()
        raise AssertionError(
            f"Server at {self.base_url} failed /health readiness within {timeout:.1f}s "
            f"(last error: {last_err}).\nLogs:\n{logs}"
        )

    def start(self) -> OrcaServerHandle:
        if self._started:
            return self

        if self.external_url:
            self._poll_health(timeout=min(self.startup_timeout, 10.0))
            self._started = True
            return self

        if not self.server_bin.is_file() or not os.access(self.server_bin, os.X_OK):
            raise AssertionError(
                f"Missing or non-executable server binary: {self.server_bin}. "
                "Fail-closed (never skipped). Build with: "
                "`cargo build --bin gpui_telegram_server && python3 scripts/clean_build_garbage.py`"
            )

        self._port = allocate_free_port()
        env = os.environ.copy()
        env.update(
            {
                "PORT": str(self._port),
                "HOST": "127.0.0.1",
                "DATABASE_URL": str(self._db_path),
                "MEDIA_STORAGE_PATH": str(self._media_dir),
                "GPUI_OFFLINE_TEST": "1",
                "RUST_LOG": "info",
            }
        )
        env.update(self.extra_env)

        try:
            self._log_file = open(self._log_path, "ab")
            self._proc = subprocess.Popen(
                [str(self.server_bin)],
                env=env,
                cwd=str(self._data_dir),
                stdout=self._log_file,
                stderr=subprocess.STDOUT,
            )
            self._poll_health(timeout=self.startup_timeout)
        except BaseException:
            self._terminate_process()
            raise
        self._started = True
        return self

    def _terminate_process(self) -> None:
        if self._proc is not None:
            if self._proc.poll() is None:
                try:
                    self._proc.send_signal(signal.SIGTERM)
                    self._proc.wait(timeout=5.0)
                except subprocess.TimeoutExpired:
                    self._proc.kill()
                    self._proc.wait(timeout=5.0)
                except ProcessLookupError:
                    pass
            self._proc = None
        if self._log_file is not None:
            try:
                self._log_file.flush()
                self._log_file.close()
            except Exception:
                pass
            self._log_file = None
        self._started = False

    def stop(self, cleanup: bool = True) -> None:
        self._terminate_process()
        if cleanup and self._tempdir is not None:
            self._tempdir.cleanup()
            self._tempdir = None

    def restart(self) -> OrcaServerHandle:
        if self.is_external:
            raise AssertionError(
                "Cannot restart an external --server-url instance; run without --server-url "
                "or use a locally spawned fresh_orca_server fixture."
            )
        self.stop(cleanup=False)
        return self.start()

    def register_user(
        self,
        username: str | None = None,
        password: str = "e2e-password-123!",
        first_name: str = "E2E",
        device_name: str = "Orca E2E",
    ) -> AuthSession:
        uname = username or f"e2e_{uuid.uuid4().hex[:10]}"
        client = OrcaApiClient(self.base_url)
        resp = client.post(
            "/api/v1/auth/register",
            json_body={
                "username": uname,
                "password": password,
                "first_name": first_name,
                "device_name": device_name,
                "auto_welcome": False,
            },
        )
        resp.assert_status(201)
        data = resp.json()
        for required_key in ("user_id", "username", "token", "session_id"):
            if required_key not in data:
                raise AssertionError(
                    f"POST /api/v1/auth/register missing field {required_key!r}: {data!r}"
                )
        return AuthSession(
            user_id=int(data["user_id"]),
            username=str(data["username"]),
            token=str(data["token"]),
            session_id=str(data["session_id"]),
            password=password,
        )

    def api_client(self, session: AuthSession | None = None) -> OrcaApiClient:
        return OrcaApiClient(
            self.base_url,
            token=session.token if session is not None else None,
        )

    def __enter__(self) -> OrcaServerHandle:
        try:
            return self.start()
        except BaseException:
            self.stop(cleanup=True)
            raise

    def __exit__(self, exc_type: Any, exc: Any, tb: Any) -> None:
        self.stop(cleanup=True)


@dataclass(frozen=True)
class CliResult:
    """Result of executing the fabushi CLI binary with fail-closed helpers."""

    returncode: int
    stdout: str
    stderr: str
    cmd: list[str]

    def _try_parse_json(self, raw: str) -> tuple[bool, Any]:
        text = raw.strip()
        if not text:
            return False, None
        try:
            return True, json.loads(text)
        except Exception:
            return False, None

    def json(self) -> Any:
        ok, data = self._try_parse_json(self.stdout)
        if ok:
            return data
        ok_err, err_data = self._try_parse_json(self.stderr)
        if ok_err:
            return err_data
        raise AssertionError(
            f"CLI output is not valid JSON (rc={self.returncode}, cmd={self.cmd!r}).\n"
            f"stdout={self.stdout[:800]!r}\nstderr={self.stderr[:800]!r}"
        )

    def assert_ok(self) -> Any:
        if self.returncode != 0:
            raise AssertionError(
                f"Expected CLI exit code 0, got {self.returncode} for cmd={self.cmd!r}.\n"
                f"stdout={self.stdout[:800]!r}\nstderr={self.stderr[:800]!r}"
            )
        ok, data = self._try_parse_json(self.stdout)
        return data if ok else self.stdout

    def extract_error_code(self) -> str | None:
        for stream in (self.stderr, self.stdout):
            ok, payload = self._try_parse_json(stream)
            if not ok or not isinstance(payload, dict):
                continue
            err = payload.get("error")
            if isinstance(err, dict) and isinstance(err.get("code"), str):
                return err["code"]
            detail = payload.get("error_detail")
            if isinstance(detail, dict) and isinstance(detail.get("code"), str):
                return detail["code"]
            if isinstance(err, str):
                return err
            code = payload.get("code")
            if isinstance(code, str):
                return code
        return None

    def assert_error(
        self, expected_code: str, expected_exit: int | None = None
    ) -> CliResult:
        if expected_exit is not None:
            if self.returncode != expected_exit:
                raise AssertionError(
                    f"Expected CLI exit code {expected_exit} for error {expected_code!r}, "
                    f"got {self.returncode} (cmd={self.cmd!r}).\n"
                    f"stdout={self.stdout[:800]!r}\nstderr={self.stderr[:800]!r}"
                )
        elif self.returncode == 0:
            raise AssertionError(
                f"Expected non-zero CLI exit code for error {expected_code!r}, got 0 "
                f"(cmd={self.cmd!r}).\nstdout={self.stdout[:800]!r}"
            )

        actual = self.extract_error_code()
        if actual != expected_code:
            raise AssertionError(
                f"Expected CLI error code {expected_code!r}, got {actual!r} "
                f"(rc={self.returncode}, cmd={self.cmd!r}).\n"
                f"stdout={self.stdout[:800]!r}\nstderr={self.stderr[:800]!r}"
            )
        return self


class OrcaCliRunner:
    """Isolated fabushi CLI runner stripping ambient env vars and pinning session state."""

    def __init__(
        self,
        cli_bin: Path | None,
        server_url: str,
        token: str | None = None,
        work_dir: Path | None = None,
    ) -> None:
        self.cli_bin: Path = Path(cli_bin) if cli_bin is not None else CLI_BIN
        self.server_url: str = server_url.rstrip("/")
        self.token: str | None = token
        self._owned_tempdir: tempfile.TemporaryDirectory[str] | None = None
        if work_dir is None:
            self._owned_tempdir = tempfile.TemporaryDirectory(prefix="orca_e2e_cli_")
            self.work_dir = Path(self._owned_tempdir.name)
        else:
            self.work_dir = Path(work_dir)
            self.work_dir.mkdir(parents=True, exist_ok=True)
        self.fabushi_home = self.work_dir / "fabushi_home"
        self.fabushi_home.mkdir(parents=True, exist_ok=True)
        self.session_path = self.work_dir / "cli_session.json"

    def _build_env(self, extra_env: dict[str, str] | None = None) -> dict[str, str]:
        env = {
            k: v
            for k, v in os.environ.items()
            if not k.startswith(AMBIENT_CLI_PREFIXES) and k != "RUST_LOG"
        }
        env["FABUSHI_HOME"] = str(self.fabushi_home)
        env["FABUSHI_CLI_SESSION_PATH"] = str(self.session_path)
        env["GPUI_OFFLINE_TEST"] = "1"
        if extra_env:
            env.update(extra_env)
        return env

    def run(
        self,
        *args: str,
        json_mode: bool = True,
        stdin_text: str | None = None,
        extra_env: dict[str, str] | None = None,
        timeout: float = 20.0,
        cwd: Path | None = None,
        remote: bool = True,
    ) -> CliResult:
        if not self.cli_bin.is_file() or not os.access(self.cli_bin, os.X_OK):
            raise AssertionError(
                f"Missing or non-executable CLI binary: {self.cli_bin}. "
                "Fail-closed (never skipped). Build with: "
                "`cargo build --bin fabushi && python3 scripts/clean_build_garbage.py`"
            )

        cmd: list[str] = [str(self.cli_bin), "--server-url", self.server_url]
        if self.token:
            cmd.extend(["--token", self.token])
        if remote:
            cmd.append("--remote")
        cmd.extend(args)
        if json_mode and "--json" not in args:
            cmd.append("--json")

        proc = subprocess.run(
            cmd,
            input=stdin_text,
            text=True,
            capture_output=True,
            env=self._build_env(extra_env),
            cwd=str(cwd) if cwd is not None else None,
            timeout=timeout,
        )
        return CliResult(
            returncode=proc.returncode,
            stdout=proc.stdout,
            stderr=proc.stderr,
            cmd=cmd,
        )

    def cleanup(self) -> None:
        if self._owned_tempdir is not None:
            self._owned_tempdir.cleanup()
            self._owned_tempdir = None
