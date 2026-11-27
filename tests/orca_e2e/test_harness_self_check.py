"""Self-integrity checks for the Orca Pure-Rust opaque-box E2E harness.

All tests in this module are marked `harness_self_check` (excluded from Tiers 1-4)
and prove that the harness fails closed on missing binaries/endpoints, preserves
exact raw HTTP wire semantics, decodes RFC 6455 WebSocket frames & close codes,
drives external upstream mocks accurately, cleans up RAII fixtures, and encodes
canonical spec constants verbatim.
"""

from __future__ import annotations

import base64
import gc
import hashlib
import http.client
import json
import socket
import sqlite3
import stat
import struct
import threading
import time
import warnings
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from pathlib import Path
from typing import Any
from urllib.parse import urlencode

import pytest

from harness import (
    ALL_EDGE_CASES_40,
    ALL_FEATURES_24,
    BOT_AGENTS,
    BOT_ERROR_CODES,
    BOT_MAX_CONNECTOR_WORKSPACES,
    BOT_MAX_MEMBERS,
    BOT_MEMORY_INDEX_LINE_CAP,
    BUNDLED_SKILLS_13,
    CLOSED_TAB_TOMBSTONE_DAYS,
    E2EE_V2_KEY_SCHEDULE_BYTES,
    IDE_ERROR_CODES,
    ORCA_YAML_MAX_BYTES,
    READD_GUARD_DAYS,
    RUNTIME_PROTOCOL_VERSION,
    TAB_CONTENT_TYPES_10,
    TEMPLATE_CATEGORIES_6,
    TEMPLATE_IDS_24,
    TIDY_UP_MAX_FILED,
    TIDY_UP_MAX_NEW_FACTS,
    TIDY_UP_TARGET_FACTS,
    TRIGGER_SOURCES_6_KINDS_31,
    WEBHOOK_ACCOUNT_HOURLY_CAP,
    WEBHOOK_ADAPTERS_6,
    WEBHOOK_FILTER_OPS_6,
    WEBHOOK_HOOK_HOURLY_CAP,
    WORKTREE_INCLUDE_MAX_BYTES,
    WORKTREE_INCLUDE_MAX_ENTRIES,
    WS_CLOSE_MESSAGE_TOO_BIG,
    WS_CLOSE_SUPERSEDED,
    AuthSession,
    BotHomeFixture,
    GitRepoFixture,
    HttpResponse,
    MockMcpUpstreamServer,
    MockOAuthServer,
    MockWebhookSink,
    OrcaApiClient,
    OrcaCliRunner,
    OrcaServerHandle,
    RawHttpClient,
    SqliteFixtureHelper,
    TempWorkspace,
    WsClient,
    pkce_s256_challenge,
)
from harness.transport import WS_GUID

pytestmark = pytest.mark.harness_self_check


def _ws_pack_server_frame(opcode: int, payload: bytes, fin: bool = True) -> bytes:
    header = bytearray([(0x80 if fin else 0x00) | (opcode & 0x0F)])
    n = len(payload)
    if n < 126:
        header.append(n)
    elif n < 65536:
        header.append(126)
        header.extend(struct.pack("!H", n))
    else:
        header.append(127)
        header.extend(struct.pack("!Q", n))
    return bytes(header) + payload


def _ws_read_exact(conn: socket.socket, buf: bytearray, n: int) -> bytes:
    while len(buf) < n:
        chunk = conn.recv(max(4096, n - len(buf)))
        if not chunk:
            raise EOFError("peer closed")
        buf.extend(chunk)
    out = bytes(buf[:n])
    del buf[:n]
    return out


def _ws_read_client_frame(conn: socket.socket, buf: bytearray) -> tuple[int, bytes]:
    b1, b2 = _ws_read_exact(conn, buf, 2)
    opcode = b1 & 0x0F
    masked = (b2 & 0x80) != 0
    length = b2 & 0x7F
    if length == 126:
        (length,) = struct.unpack("!H", _ws_read_exact(conn, buf, 2))
    elif length == 127:
        (length,) = struct.unpack("!Q", _ws_read_exact(conn, buf, 8))
    mask = _ws_read_exact(conn, buf, 4) if masked else b""
    data = _ws_read_exact(conn, buf, length)
    if masked and data:
        data = bytes(b ^ mask[i % 4] for i, b in enumerate(data))
    return opcode, data


class _MiniLoopbackWsServer:
    """Minimal RFC 6455 loopback server used strictly to test WsClient wire handling."""

    def __init__(self, mode: str = "echo", max_frame_bytes: int = 4096) -> None:
        self.mode = mode
        self.max_frame_bytes = max_frame_bytes
        self.pong_received = threading.Event()
        self._sock = socket.socket(socket.AF_INET, socket.SOCK_STREAM)
        self._sock.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
        self._sock.bind(("127.0.0.1", 0))
        self._sock.listen(4)
        self._sock.settimeout(5.0)
        self.port = int(self._sock.getsockname()[1])
        self._stop = threading.Event()
        self._thread = threading.Thread(target=self._serve, daemon=True)
        self._thread.start()

    @property
    def ws_url(self) -> str:
        return f"ws://127.0.0.1:{self.port}/api/v1/orca/wake"

    def _serve(self) -> None:
        try:
            conn, _ = self._sock.accept()
        except OSError:
            return
        with conn:
            conn.settimeout(5.0)
            buf = bytearray()
            while b"\r\n\r\n" not in buf:
                chunk = conn.recv(4096)
                if not chunk:
                    return
                buf.extend(chunk)
            sep = buf.index(b"\r\n\r\n")
            head = bytes(buf[:sep]).decode("utf-8", errors="replace")
            del buf[: sep + 4]
            ws_key = ""
            for line in head.split("\r\n")[1:]:
                if line.lower().startswith("sec-websocket-key:"):
                    ws_key = line.split(":", 1)[1].strip()
            accept = base64.b64encode(
                hashlib.sha1((ws_key + WS_GUID).encode("ascii")).digest()
            ).decode("ascii")
            status_line = (
                "HTTP/1.1 101\r\n"
                if self.mode == "frag_and_64bit"
                else "HTTP/1.1 101 Switching Protocols\r\n"
            )
            resp = (
                status_line
                + "Upgrade: websocket\r\n"
                + "Connection: Upgrade\r\n"
                + f"Sec-WebSocket-Accept: {accept}\r\n\r\n"
            ).encode("ascii")
            conn.sendall(resp)

            if self.mode == "close_4409":
                conn.sendall(
                    _ws_pack_server_frame(0x8, struct.pack("!H", 4409) + b"superseded")
                )
                return

            if self.mode == "periodic_ping":
                end_at = time.monotonic() + 1.5
                while not self._stop.is_set() and time.monotonic() < end_at:
                    try:
                        conn.sendall(_ws_pack_server_frame(0x9, b"keep"))
                    except OSError:
                        return
                    self._stop.wait(0.08)
                try:
                    conn.sendall(_ws_pack_server_frame(0x8, struct.pack("!H", 1000) + b"done"))
                except OSError:
                    pass
                return

            if self.mode == "frag_and_64bit":
                conn.sendall(_ws_pack_server_frame(0x1, b"hello-", fin=False))
                conn.sendall(_ws_pack_server_frame(0x9, b"mid"))
                conn.sendall(_ws_pack_server_frame(0x0, b"fragmented", fin=True))
                while not self._stop.is_set():
                    try:
                        opcode, data = _ws_read_client_frame(conn, buf)
                    except (EOFError, OSError):
                        return
                    if opcode == 0xA:
                        continue
                    if opcode == 0x8:
                        conn.sendall(_ws_pack_server_frame(0x8, struct.pack("!H", 1000)))
                        return
                    if opcode == 0x2:
                        conn.sendall(_ws_pack_server_frame(0x2, data))
                return

            if self.mode == "echo":
                conn.sendall(_ws_pack_server_frame(0x9, b"hb"))
                while not self._stop.is_set():
                    try:
                        opcode, data = _ws_read_client_frame(conn, buf)
                    except (EOFError, OSError):
                        return
                    if opcode == 0xA and data == b"hb":
                        self.pong_received.set()
                        continue
                    if opcode == 0x8:
                        conn.sendall(_ws_pack_server_frame(0x8, struct.pack("!H", 1000)))
                        return
                    if opcode in (0x1, 0x2):
                        conn.sendall(_ws_pack_server_frame(opcode, data))
                return

            if self.mode == "reject_oversize":
                try:
                    b1, b2 = _ws_read_exact(conn, buf, 2)
                    _ = b1
                    length = b2 & 0x7F
                    if length == 126:
                        (length,) = struct.unpack("!H", _ws_read_exact(conn, buf, 2))
                    elif length == 127:
                        (length,) = struct.unpack("!Q", _ws_read_exact(conn, buf, 8))
                    if length > self.max_frame_bytes:
                        conn.sendall(
                            _ws_pack_server_frame(
                                0x8, struct.pack("!H", 1009) + b"message too big"
                            )
                        )
                except (EOFError, OSError):
                    return

    def stop(self) -> None:
        self._stop.set()
        try:
            self._sock.close()
        except OSError:
            pass
        if self._thread.is_alive():
            self._thread.join(timeout=2.0)

    def __enter__(self) -> _MiniLoopbackWsServer:
        return self

    def __exit__(self, exc_type: Any, exc: Any, tb: Any) -> None:
        self.stop()


def test_missing_server_binary_fails_closed(temp_workspace: TempWorkspace) -> None:
    missing_bin = temp_workspace.path / "nonexistent_gpui_telegram_server"
    handle = OrcaServerHandle(server_bin=missing_bin)
    try:
        with pytest.raises(AssertionError) as exc_info:
            handle.start()
        msg = str(exc_info.value)
        assert "Missing or non-executable server binary" in msg
        assert str(missing_bin) in msg
    finally:
        handle.stop(cleanup=True)


def test_missing_cli_binary_fails_closed(temp_workspace: TempWorkspace) -> None:
    missing_cli = temp_workspace.path / "nonexistent_fabushi_cli"
    runner = OrcaCliRunner(
        cli_bin=missing_cli,
        server_url="http://127.0.0.1:18080",
        token="tok-dummy",
        work_dir=temp_workspace.mkdir("cli_work"),
    )
    with pytest.raises(AssertionError) as exc_info:
        runner.run("bot", "list")
    msg = str(exc_info.value)
    assert "Missing or non-executable CLI binary" in msg
    assert str(missing_cli) in msg


def test_api_client_404_stub_fails_loudly_on_assert_status_and_rpc() -> None:
    class _Stub404(BaseHTTPRequestHandler):
        def log_message(self, format: str, *args: Any) -> None:
            return

        def _send_404(self) -> None:
            raw = json.dumps(
                {"ok": False, "error": {"code": "not_found", "message": "route missing"}}
            ).encode("utf-8")
            self.send_response(404)
            self.send_header("Content-Type", "application/json")
            self.send_header("Content-Length", str(len(raw)))
            self.end_headers()
            self.wfile.write(raw)

        def do_GET(self) -> None:
            self._send_404()

        def do_POST(self) -> None:
            length = int(self.headers.get("Content-Length", "0"))
            if length:
                self.rfile.read(length)
            self._send_404()

    httpd = ThreadingHTTPServer(("127.0.0.1", 0), _Stub404)
    port = httpd.server_address[1]
    thread = threading.Thread(target=httpd.serve_forever, kwargs={"poll_interval": 0.02}, daemon=True)
    thread.start()
    try:
        api = OrcaApiClient(f"http://127.0.0.1:{port}", token="bearer-abc")
        resp = api.get("/api/v1/orca/bots")
        assert resp.status == 404
        resp.assert_error_code("not_found", 404)

        with pytest.raises(AssertionError) as status_exc:
            resp.assert_status(200)
        assert "404" in str(status_exc.value)

        with pytest.raises(AssertionError) as code_exc:
            resp.assert_error_code("bot_not_found")
        assert "not_found" in str(code_exc.value)

        with pytest.raises(AssertionError) as rpc_exc:
            api.assert_rpc_ok("bots.list")
        assert "404" in str(rpc_exc.value)
    finally:
        httpd.shutdown()
        httpd.server_close()
        thread.join(timeout=2.0)


def test_raw_http_client_transmits_spoofed_host_origin_and_disallowed_methods() -> None:
    captured: list[dict[str, Any]] = []

    class _WireEcho(BaseHTTPRequestHandler):
        def log_message(self, format: str, *args: Any) -> None:
            return

        def _capture(self, method: str) -> None:
            length = int(self.headers.get("Content-Length", "0"))
            body = self.rfile.read(length) if length else b""
            hdrs = {k.lower(): v for k, v in self.headers.items()}
            entry = {"method": method, "path": self.path, "headers": hdrs, "body": body.decode()}
            captured.append(entry)
            raw = json.dumps(entry).encode("utf-8")
            self.send_response(200)
            self.send_header("Content-Type", "application/json")
            self.send_header("Content-Length", str(len(raw)))
            self.end_headers()
            self.wfile.write(raw)

        def do_POST(self) -> None:
            self._capture("POST")

        def do_PUT(self) -> None:
            self._capture("PUT")

        def do_OPTIONS(self) -> None:
            self._capture("OPTIONS")

    httpd = ThreadingHTTPServer(("127.0.0.1", 0), _WireEcho)
    port = httpd.server_address[1]
    thread = threading.Thread(target=httpd.serve_forever, kwargs={"poll_interval": 0.02}, daemon=True)
    thread.start()
    try:
        raw = RawHttpClient("127.0.0.1", port)
        r1 = raw.request(
            "POST",
            "/relay/mcp",
            headers={"Host": "evil.example", "Origin": "https://evil.example"},
            json_body={"ping": 1},
        ).assert_status(200)
        assert r1.json()["headers"]["host"] == "evil.example"
        assert r1.json()["headers"]["origin"] == "https://evil.example"

        r2 = raw.request("PUT", "/relay/mcp", headers={"Host": "127.0.0.1"}).assert_status(200)
        assert r2.json()["method"] == "PUT"

        r3 = raw.request("OPTIONS", "/relay/mcp").assert_status(200)
        assert r3.json()["method"] == "OPTIONS"
        assert [c["method"] for c in captured] == ["POST", "PUT", "OPTIONS"]
    finally:
        httpd.shutdown()
        httpd.server_close()
        thread.join(timeout=2.0)


def test_ws_client_rfc6455_handshake_text_binary_and_ping_pong() -> None:
    with _MiniLoopbackWsServer(mode="echo") as ws_srv:
        with WsClient.connect(ws_srv.ws_url, headers={"X-Test": "orca"}) as ws:
            ws.send_text("hello-orca")
            f1 = ws.recv(timeout=3.0)
            assert f1.opcode == 0x1
            assert f1.text == "hello-orca"
            assert ws_srv.pong_received.wait(timeout=2.0)

            big_bin = bytes(range(256)) * 4  # 1024 bytes -> 16-bit extended length
            ws.send_binary(big_bin)
            f2 = ws.recv(timeout=3.0)
            assert f2.opcode == 0x2
            assert f2.payload == big_bin


def test_ws_client_captures_close_code_4409_superseded() -> None:
    with _MiniLoopbackWsServer(mode="close_4409") as ws_srv:
        with WsClient.connect(ws_srv.ws_url) as ws:
            close_frame = ws.recv_until_close(timeout=3.0)
            assert close_frame.opcode == 0x8
            assert close_frame.close_code == WS_CLOSE_SUPERSEDED == 4409
            assert close_frame.close_reason == "superseded"


def test_ws_client_captures_close_code_1009_oversize() -> None:
    with _MiniLoopbackWsServer(mode="reject_oversize", max_frame_bytes=4096) as ws_srv:
        with WsClient.connect(ws_srv.ws_url) as ws:
            ws.send_raw_oversize(8192)
            close_frame = ws.recv_until_close(timeout=3.0)
            assert close_frame.opcode == 0x8
            assert close_frame.close_code == WS_CLOSE_MESSAGE_TOO_BIG == 1009
            assert close_frame.close_reason == "message too big"


def test_mock_mcp_upstream_401_once_then_200_and_headers() -> None:
    with MockMcpUpstreamServer(fail_401_once=True) as mcp:
        raw = RawHttpClient(mcp.host, mcp.port)
        first = raw.request(
            "POST",
            "/mcp",
            headers={"Authorization": "Bearer expired-token", "X-Custom": "alpha"},
            json_body={"jsonrpc": "2.0", "id": 7, "method": "tools/list"},
        )
        first.assert_status(401)
        assert "Bearer" in (first.header("www-authenticate") or "")

        second = raw.request(
            "POST",
            "/mcp",
            headers={"Authorization": "Bearer fresh-token", "X-Custom": "beta"},
            json_body={"jsonrpc": "2.0", "id": 7, "method": "tools/list"},
        )
        second.assert_status(200)
        body = second.json()
        assert body["id"] == 7
        assert body["result"]["received_headers"]["authorization"] == "Bearer fresh-token"
        assert len(mcp.requests) == 2
        assert mcp.requests[0].headers["x-custom"] == "alpha"
        assert mcp.requests[1].headers["x-custom"] == "beta"


def test_mock_mcp_upstream_detects_sse_client_abort() -> None:
    with MockMcpUpstreamServer() as mcp:
        conn = http.client.HTTPConnection(mcp.host, mcp.port, timeout=5.0)
        conn.request("GET", "/sse", headers={"Accept": "text/event-stream"})
        resp = conn.getresponse()
        assert resp.status == 200
        assert "text/event-stream" in (resp.getheader("Content-Type") or "")
        first_line = resp.readline()
        assert b"event: ready" in first_line
        assert mcp.wait_for_stream_start(timeout=2.0)

        # Simulate relay revoking credential and aborting upstream TCP stream
        resp.close()
        conn.close()
        assert mcp.wait_for_stream_abort(timeout=3.0), (
            "MockMcpUpstreamServer failed to detect SSE client disconnect"
        )


def test_mock_oauth_server_pkce_s256_refresh_and_revoke(
    mock_oauth_server: MockOAuthServer,
) -> None:
    raw = RawHttpClient(mock_oauth_server.host, mock_oauth_server.port)

    meta = raw.request("GET", "/.well-known/oauth-authorization-server").assert_status(200).json()
    assert meta["issuer"] == mock_oauth_server.issuer
    assert "S256" in meta["code_challenge_methods_supported"]

    reg = raw.request(
        "POST",
        "/register",
        json_body={"client_name": "Orca Bot Relay", "redirect_uris": ["http://127.0.0.1/cb"]},
    ).assert_status(201).json()
    client_id = reg["client_id"]
    assert client_id in mock_oauth_server.registered_clients

    verifier = "orca_pkce_verifier_secret_value_0123456789"
    challenge = pkce_s256_challenge(verifier)

    bad_code = mock_oauth_server.issue_auth_code(
        client_id=client_id, code_challenge=challenge, redirect_uri="http://127.0.0.1/cb"
    )
    bad_tok = raw.request(
        "POST",
        "/token",
        headers={"Content-Type": "application/x-www-form-urlencoded"},
        body=urlencode(
            {
                "grant_type": "authorization_code",
                "code": bad_code,
                "code_verifier": "wrong_verifier",
                "redirect_uri": "http://127.0.0.1/cb",
            }
        ),
    )
    bad_tok.assert_error_code("invalid_grant", 400)

    good_code = mock_oauth_server.issue_auth_code(
        client_id=client_id, code_challenge=challenge, redirect_uri="http://127.0.0.1/cb"
    )
    tok = raw.request(
        "POST",
        "/token",
        headers={"Content-Type": "application/x-www-form-urlencoded"},
        body=urlencode(
            {
                "grant_type": "authorization_code",
                "code": good_code,
                "code_verifier": verifier,
                "redirect_uri": "http://127.0.0.1/cb",
            }
        ),
    ).assert_status(200).json()
    access_1 = tok["access_token"]
    refresh_1 = tok["refresh_token"]
    assert access_1 and refresh_1

    refreshed = raw.request(
        "POST",
        "/token",
        json_body={"grant_type": "refresh_token", "refresh_token": refresh_1},
    ).assert_status(200).json()
    assert refreshed["access_token"] != access_1
    assert len(mock_oauth_server.refresh_calls) == 1

    raw.request("POST", "/revoke", json_body={"token": refresh_1}).assert_status(200)
    assert refresh_1 in mock_oauth_server.revoked_tokens
    assert len(mock_oauth_server.revoke_calls) == 1

    revoked_attempt = raw.request(
        "POST",
        "/token",
        json_body={"grant_type": "refresh_token", "refresh_token": refresh_1},
    )
    revoked_attempt.assert_error_code("invalid_grant", 400)


def test_mock_webhook_sink_records_ordered_timestamps(
    mock_webhook_sink: MockWebhookSink,
) -> None:
    raw = RawHttpClient(mock_webhook_sink.host, mock_webhook_sink.port)
    for seq in (1, 2, 3):
        resp = raw.request(
            "POST",
            f"/hook/{seq}",
            headers={"X-Seq": str(seq)},
            json_body={"seq": seq, "kind": "mirror"},
        )
        resp.assert_status(200)
        time.sleep(0.005)

    recs = mock_webhook_sink.wait_for_requests(3, timeout=2.0)
    assert [r.json_body["seq"] for r in recs] == [1, 2, 3]
    assert [r.headers["x-seq"] for r in recs] == ["1", "2", "3"]
    assert recs[0].timestamp_monotonic <= recs[1].timestamp_monotonic <= recs[2].timestamp_monotonic


def test_git_repo_fixture_commits_ignored_files_and_raii_cleanup() -> None:
    repo = GitRepoFixture()
    repo_path = repo.path
    try:
        sha = repo.commit_file("README.md", "# Orca E2E\n", msg="initial")
        assert len(sha) == 40
        assert repo.current_branch() == "main"

        ignored = repo.create_ignored_file(".env.local", "SECRET=1\n")
        assert ignored.is_file()
        check_ignore = repo.git("check-ignore", ".env.local", check=False)
        assert check_ignore.returncode == 0

        repo.write_orca_yaml("scripts:\n  setup: echo ready\n")
        repo.write_worktreeinclude([".env.local"])
        repo.create_branch("feature/worktree-test")
        branches = repo.git("branch", "--list").stdout
        assert "feature/worktree-test" in branches
        assert (repo_path / "orca.yaml").is_file()
        assert (repo_path / ".worktreeinclude").is_file()
    finally:
        repo.cleanup()

    assert not repo_path.exists(), f"GitRepoFixture leaked temp dir: {repo_path}"


def test_bot_home_fixture_and_foreign_trees_with_raii_cleanup() -> None:
    bot = BotHomeFixture()
    bot_root = bot.root
    ws = TempWorkspace(prefix="orca_e2e_foreign_")
    foreign_root = ws.path
    try:
        bot.populate_default(bot_id="bot-42", name="Triage Bot", folder="triage-bot")
        for rel in (
            "SOUL.md",
            "MEMORY.md",
            "AGENTS.md",
            "CLAUDE.md",
            "bot.json",
            "memory/ci.md",
            "memory/journal/2026-11.md",
            "skills/pr-review-checklist/SKILL.md",
        ):
            assert (bot_root / rel).is_file(), f"Missing expected bot home file {rel}"

        meta = json.loads((bot_root / "bot.json").read_text(encoding="utf-8"))
        assert meta["id"] == "bot-42"
        assert meta["folder"] == "triage-bot"

        oc = BotHomeFixture.create_foreign_openclaw(foreign_root, bot_name="ClawOps")
        hm = BotHomeFixture.create_foreign_hermes(foreign_root, profile="ops")
        ob = BotHomeFixture.create_foreign_openbot(foreign_root, agent_name="Deployer")
        assert (oc / "IDENTITY.md").is_file()
        assert (hm / "SOUL.md").is_file()
        assert (hm / "config.yaml").is_file()
        assert (ob / "SOUL.md").is_file()
    finally:
        bot.cleanup()
        ws.cleanup()

    assert not bot_root.exists(), f"BotHomeFixture leaked temp dir: {bot_root}"
    assert not foreign_root.exists(), f"TempWorkspace leaked temp dir: {foreign_root}"


def test_sqlite_fixture_helper_user_version_triggers_and_corrupt_header() -> None:
    ws = TempWorkspace(prefix="orca_e2e_sql_")
    ws_path = ws.path
    try:
        db_path = ws.resolve("state.db")
        helper = SqliteFixtureHelper(db_path)
        assert helper.get_user_version() == 0
        helper.set_user_version(99)
        assert helper.get_user_version() == 99

        # Verify rollback-safe AFTER UPDATE trigger preserving newer JSON field
        helper.exec_script(
            """
            CREATE TABLE bot_groups (
                id TEXT PRIMARY KEY,
                payload TEXT NOT NULL
            );
            CREATE TRIGGER preserve_remote_members
            AFTER UPDATE ON bot_groups
            WHEN json_extract(OLD.payload, '$.remoteMembers') IS NOT NULL
             AND json_extract(NEW.payload, '$.remoteMembers') IS NULL
            BEGIN
                UPDATE bot_groups
                SET payload = json_set(
                    NEW.payload,
                    '$.remoteMembers',
                    json(json_extract(OLD.payload, '$.remoteMembers'))
                )
                WHERE id = NEW.id;
            END;
            """
        )
        helper.exec_sql(
            "INSERT INTO bot_groups (id, payload) VALUES (?, ?)",
            ("grp-1", json.dumps({"title": "Eng", "remoteMembers": [{"id": "rem-1"}]})),
        )
        # Simulated older build overwrites row without remoteMembers
        helper.exec_sql(
            "UPDATE bot_groups SET payload = ? WHERE id = ?",
            (json.dumps({"title": "Eng Renamed"}), "grp-1"),
        )
        rows = helper.exec_sql("SELECT payload FROM bot_groups WHERE id = ?", ("grp-1",))
        updated = json.loads(rows[0][0])
        assert updated["title"] == "Eng Renamed"
        assert updated["remoteMembers"] == [{"id": "rem-1"}]

        # Corrupt SQLite header and confirm sqlite3 rejects it
        SqliteFixtureHelper.corrupt_header(db_path)
        with pytest.raises(sqlite3.DatabaseError):
            helper.exec_sql("SELECT count(*) FROM bot_groups;")
    finally:
        ws.cleanup()

    assert not ws_path.exists(), f"SQLite TempWorkspace leaked dir: {ws_path}"


def test_canonical_spec_constants_match_project_and_surveys() -> None:
    assert len(BOT_AGENTS) == 22
    assert len(set(BOT_AGENTS)) == 22
    assert len(BOT_ERROR_CODES) == 29
    assert len(set(BOT_ERROR_CODES)) == 29
    assert len(IDE_ERROR_CODES) == 17
    assert len(set(IDE_ERROR_CODES)) == 17

    assert len(TEMPLATE_CATEGORIES_6) == 6
    assert len(TEMPLATE_IDS_24) == 24
    assert len(set(TEMPLATE_IDS_24)) == 24
    assert len(BUNDLED_SKILLS_13) == 13
    assert len(set(BUNDLED_SKILLS_13)) == 13

    assert len(TRIGGER_SOURCES_6_KINDS_31) == 6
    total_trigger_kinds = sum(len(v) for v in TRIGGER_SOURCES_6_KINDS_31.values())
    assert total_trigger_kinds == 31
    assert len(TRIGGER_SOURCES_6_KINDS_31["github"]) == 21
    assert len(TRIGGER_SOURCES_6_KINDS_31["slack"]) == 3
    assert len(TRIGGER_SOURCES_6_KINDS_31["linear"]) == 4

    assert len(WEBHOOK_ADAPTERS_6) == 6
    assert len(WEBHOOK_FILTER_OPS_6) == 6
    assert len(TAB_CONTENT_TYPES_10) == 10
    assert len(ALL_FEATURES_24) == 24
    assert ALL_FEATURES_24[0] == "f01" and ALL_FEATURES_24[-1] == "f24"
    assert len(ALL_EDGE_CASES_40) == 40
    assert ALL_EDGE_CASES_40[0] == "SM1-01" and ALL_EDGE_CASES_40[13] == "SM1-14"
    assert ALL_EDGE_CASES_40[14] == "SM2-01" and ALL_EDGE_CASES_40[-1] == "SM2-26"

    assert BOT_MEMORY_INDEX_LINE_CAP == 200
    assert READD_GUARD_DAYS == 14
    assert TIDY_UP_TARGET_FACTS == 60
    assert TIDY_UP_MAX_NEW_FACTS == 6
    assert TIDY_UP_MAX_FILED == 20
    assert BOT_MAX_MEMBERS == 200
    assert BOT_MAX_CONNECTOR_WORKSPACES == 8
    assert WEBHOOK_HOOK_HOURLY_CAP == 200
    assert WEBHOOK_ACCOUNT_HOURLY_CAP == 300
    assert ORCA_YAML_MAX_BYTES == 262_144
    assert WORKTREE_INCLUDE_MAX_BYTES == 2_147_483_648
    assert WORKTREE_INCLUDE_MAX_ENTRIES == 50_000
    assert CLOSED_TAB_TOMBSTONE_DAYS == 30
    assert RUNTIME_PROTOCOL_VERSION == 3
    assert E2EE_V2_KEY_SCHEDULE_BYTES == 96


def test_dual_import_paths_and_auth_session_signature() -> None:
    import harness as short_harness
    import harness.config as short_config
    import harness.fixtures as short_fixtures
    import harness.mock_servers as short_mock_servers
    import harness.runners as short_runners
    import harness.transport as short_transport
    import tests.orca_e2e.harness as full_harness
    import tests.orca_e2e.harness.config as full_config
    import tests.orca_e2e.harness.fixtures as full_fixtures
    import tests.orca_e2e.harness.mock_servers as full_mock_servers
    import tests.orca_e2e.harness.runners as full_runners
    import tests.orca_e2e.harness.transport as full_transport
    from harness import BOT_AGENTS as short_agents
    from tests.orca_e2e.harness import BOT_AGENTS as full_agents
    from tests.orca_e2e.harness import AuthSession as FullAuthSession
    from tests.orca_e2e.harness import HttpResponse as FullHttpResponse
    from tests.orca_e2e.harness import OrcaApiClient as FullOrcaApiClient
    from tests.orca_e2e.harness import RawHttpClient as FullRawHttpClient

    assert short_harness is full_harness
    assert short_config is full_config
    assert short_fixtures is full_fixtures
    assert short_mock_servers is full_mock_servers
    assert short_runners is full_runners
    assert short_transport is full_transport

    assert FullAuthSession is AuthSession
    assert FullHttpResponse is HttpResponse
    assert FullOrcaApiClient is OrcaApiClient
    assert FullRawHttpClient is RawHttpClient

    assert short_agents is full_agents is BOT_AGENTS
    assert len(full_agents) == 22

    # 4-arg positional constructor matching TEST_INFRA.md:75
    sess = AuthSession(7, "alice", "tok-abc", "sess-xyz")
    assert isinstance(sess, FullAuthSession)
    assert sess == FullAuthSession(7, "alice", "tok-abc", "sess-xyz")
    assert sess.user_id == 7
    assert sess.username == "alice"
    assert sess.token == "tok-abc"
    assert sess.session_id == "sess-xyz"
    assert sess.password == ""

    sess_pw = FullAuthSession(8, "bob", "tok-def", "sess-999", password="pw!")
    assert isinstance(sess_pw, AuthSession)
    assert sess_pw.password == "pw!"

    # Cross-import HttpResponse passed into assert_rpc_ok / assert_rpc_error
    ok_resp = FullHttpResponse(
        status=200,
        reason="OK",
        headers={"content-type": "application/json"},
        body=json.dumps({"ok": True, "result": {"bots": ["alpha"]}}).encode("utf-8"),
    )
    err_resp = HttpResponse(
        status=200,
        reason="OK",
        headers={"content-type": "application/json"},
        body=json.dumps(
            {"ok": False, "error": {"code": "bot_not_found", "message": "missing"}}
        ).encode("utf-8"),
    )
    short_api = OrcaApiClient("http://127.0.0.1:1", token="tok-short")
    full_api = FullOrcaApiClient("http://127.0.0.1:1", token="tok-full")
    assert short_api.assert_rpc_ok(ok_resp) == {"bots": ["alpha"]}
    assert full_api.assert_rpc_error(err_resp, "bot_not_found") == {
        "code": "bot_not_found",
        "message": "missing",
    }


def test_assert_rpc_error_fail_closed_and_raw_accept_encoding_no_dup() -> None:
    scenario: dict[str, Any] = {"status": 404, "payload": {"error": {"code": "not_found"}}}
    seen_accept_encodings: list[list[str]] = []

    class _RpcEdgeStub(BaseHTTPRequestHandler):
        def log_message(self, format: str, *args: Any) -> None:
            return

        def do_GET(self) -> None:
            seen_accept_encodings.append(self.headers.get_all("Accept-Encoding") or [])
            raw = b'{"ok":true}'
            self.send_response(200)
            self.send_header("Content-Type", "application/json")
            self.send_header("Content-Length", str(len(raw)))
            self.end_headers()
            self.wfile.write(raw)

        def do_POST(self) -> None:
            length = int(self.headers.get("Content-Length", "0"))
            if length:
                self.rfile.read(length)
            raw = json.dumps(scenario["payload"]).encode("utf-8")
            self.send_response(int(scenario["status"]))
            self.send_header("Content-Type", "application/json")
            self.send_header("Content-Length", str(len(raw)))
            self.end_headers()
            self.wfile.write(raw)

    httpd = ThreadingHTTPServer(("127.0.0.1", 0), _RpcEdgeStub)
    port = httpd.server_address[1]
    thread = threading.Thread(target=httpd.serve_forever, kwargs={"poll_interval": 0.02}, daemon=True)
    thread.start()
    try:
        api = OrcaApiClient(f"http://127.0.0.1:{port}", token="tok")

        # HTTP 404 missing "ok": false MUST fail closed
        scenario.update(status=404, payload={"error": {"code": "not_found"}})
        with pytest.raises(AssertionError):
            api.assert_rpc_error("bots.get", "not_found")

        # HTTP 501 MUST fail closed both without and with "ok": false
        scenario.update(status=501, payload={"error": {"code": "not_found"}})
        with pytest.raises(AssertionError):
            api.assert_rpc_error("bots.get", "not_found")

        scenario.update(status=501, payload={"ok": False, "error": {"code": "not_found"}})
        with pytest.raises(AssertionError):
            api.assert_rpc_error("bots.get", "not_found")

        # Non-dict "error" MUST fail closed
        scenario.update(status=200, payload={"ok": False, "error": "not_found"})
        with pytest.raises(AssertionError):
            api.assert_rpc_error("bots.get", "not_found")

        # Canonical Unified RPC error envelope MUST pass and return error dict
        scenario.update(
            status=200,
            payload={"ok": False, "error": {"code": "bot_not_found", "message": "missing"}},
        )
        err = api.assert_rpc_error("bots.get", "bot_not_found")
        assert err == {"code": "bot_not_found", "message": "missing"}

        # RawHttpClient MUST NOT duplicate Accept-Encoding when caller supplies it
        RawHttpClient("127.0.0.1", port).request(
            "GET", "/ae", headers={"Accept-Encoding": "gzip"}
        ).assert_status(200)
        assert seen_accept_encodings == [["gzip"]]
    finally:
        httpd.shutdown()
        httpd.server_close()
        thread.join(timeout=2.0)


def test_ws_client_ping_deadline_64bit_extended_and_fragmented_frames() -> None:
    # 1. Monotonic deadline must fire promptly even under periodic server Pings every 0.08s
    with _MiniLoopbackWsServer(mode="periodic_ping") as ping_srv:
        with WsClient.connect(ping_srv.ws_url) as ws:
            t0 = time.monotonic()
            with pytest.raises(TimeoutError):
                ws.recv_until_close(timeout=0.35)
            elapsed = time.monotonic() - t0
            assert 0.25 <= elapsed < 0.85, f"Expected prompt ~0.35s timeout, got {elapsed:.3f}s"

    # 2. Empty reason-phrase 101, 2-fragment text message with mid-Ping, and 64-bit 70000B frame
    with _MiniLoopbackWsServer(mode="frag_and_64bit") as ws_srv:
        with WsClient.connect(ws_srv.ws_url) as ws:
            frag = ws.recv(timeout=3.0)
            assert frag.opcode == 0x1
            assert frag.text == "hello-fragmented"

            payload_70k = (b"ORCA64BIT!" * 7000)
            assert len(payload_70k) == 70_000
            ws.send_binary(payload_70k)
            echoed = ws.recv(timeout=3.0)
            assert echoed.opcode == 0x2
            assert len(echoed.payload) == 70_000
            assert echoed.payload == payload_70k


def test_mock_mcp_concurrent_32_chunked_post_and_sse_bearer_gate() -> None:
    # 1. N=32 concurrent requests with 0 errors and exact 1x 401
    with MockMcpUpstreamServer(fail_401_once=True) as mcp:
        barrier = threading.Barrier(32)
        statuses: list[int] = []
        errors: list[str] = []
        lock = threading.Lock()

        def _worker(idx: int) -> None:
            try:
                barrier.wait(timeout=5.0)
                resp = RawHttpClient(mcp.host, mcp.port).request(
                    "POST", "/mcp", json_body={"jsonrpc": "2.0", "id": idx}
                )
                with lock:
                    statuses.append(resp.status)
            except Exception as exc:
                with lock:
                    errors.append(repr(exc))

        threads = [threading.Thread(target=_worker, args=(i,)) for i in range(32)]
        for t in threads:
            t.start()
        for t in threads:
            t.join(timeout=10.0)

        assert errors == [], f"Concurrent requests failed: {errors[:5]}"
        assert len(statuses) == 32
        assert statuses.count(401) == 1
        assert statuses.count(200) == 31
        assert len(mcp.requests) == 32
        timestamps = [r.timestamp_monotonic for r in mcp.requests]
        assert timestamps == sorted(timestamps)

    # 2. HTTP/1.1 Transfer-Encoding: chunked POST decoding (with extension & trailer)
    with MockMcpUpstreamServer() as mcp:
        payload_obj = {"jsonrpc": "2.0", "id": 99, "method": "tools/call", "params": {"n": 42}}
        raw_payload = json.dumps(payload_obj).encode("utf-8")
        p1, p2 = raw_payload[:19], raw_payload[19:]
        chunked_wire = (
            f"{len(p1):x};ext=1\r\n".encode("ascii")
            + p1
            + b"\r\n"
            + f"{len(p2):x}\r\n".encode("ascii")
            + p2
            + b"\r\n0\r\nX-Trailer: ok\r\n\r\n"
        )
        resp = RawHttpClient(mcp.host, mcp.port).request(
            "POST",
            "/mcp",
            headers={"Content-Type": "application/json", "Transfer-Encoding": "chunked"},
            body=chunked_wire,
        ).assert_status(200)
        assert mcp.requests[0].body == raw_payload
        assert mcp.requests[0].json_body == payload_obj
        assert resp.json()["result"]["echo"] == payload_obj

    # 3. /sse enforces expected_bearer and drains stray bytes without busy-spinning
    with MockMcpUpstreamServer(expected_bearer="secret-sse-tok") as mcp:
        unauth = RawHttpClient(mcp.host, mcp.port).request(
            "GET", "/sse", headers={"Authorization": "Bearer wrong-tok"}
        )
        unauth.assert_status(401)
        assert not mcp.stream_started.is_set()

        with socket.create_connection((mcp.host, mcp.port), timeout=5.0) as sock:
            req = (
                f"GET /sse HTTP/1.1\r\n"
                f"Host: {mcp.host}:{mcp.port}\r\n"
                f"Accept: text/event-stream\r\n"
                f"Authorization: Bearer secret-sse-tok\r\n\r\n"
            ).encode("ascii")
            sock.sendall(req)
            head = b""
            while b"event: ready" not in head:
                chunk = sock.recv(4096)
                assert chunk, "SSE stream closed prematurely"
                head += chunk
            assert b"200" in head.split(b"\r\n", 1)[0]
            assert mcp.wait_for_stream_start(timeout=2.0)

            # Send stray byte on open SSE socket; server must drain it instead of busy-spinning
            sock.sendall(b"X")
            time.sleep(0.15)
            sock.setblocking(False)
            received = 0
            while True:
                try:
                    chunk = sock.recv(8192)
                except BlockingIOError:
                    break
                if not chunk:
                    break
                received += len(chunk)
            assert received < 2048, f"SSE handler busy-spun on stray byte ({received}B)"
        assert mcp.wait_for_stream_abort(timeout=3.0)

    # 4. MockWebhookSink handles N=32 concurrent POSTs using shared Condition(self._lock)
    with MockWebhookSink() as sink:
        barrier = threading.Barrier(32)
        wh_errors: list[str] = []
        wh_lock = threading.Lock()

        def _wh_worker(idx: int) -> None:
            try:
                barrier.wait(timeout=5.0)
                RawHttpClient(sink.host, sink.port).request(
                    "POST", f"/wh/{idx}", json_body={"idx": idx}
                ).assert_status(200)
            except Exception as exc:
                with wh_lock:
                    wh_errors.append(repr(exc))

        threads = [threading.Thread(target=_wh_worker, args=(i,)) for i in range(32)]
        for t in threads:
            t.start()
        recs = sink.wait_for_requests(32, timeout=5.0)
        for t in threads:
            t.join(timeout=5.0)
        assert wh_errors == [], f"Concurrent webhook requests failed: {wh_errors[:5]}"
        assert len(recs) == 32
        wh_ts = [r.timestamp_monotonic for r in recs]
        assert wh_ts == sorted(wh_ts)


def test_mock_oauth_returns_400_on_non_ascii_verifier_and_malformed_json(
    mock_oauth_server: MockOAuthServer,
) -> None:
    raw = RawHttpClient(mock_oauth_server.host, mock_oauth_server.port)
    challenge = pkce_s256_challenge("valid_ascii_verifier_0123456789")
    code = mock_oauth_server.issue_auth_code(
        client_id="client-1",
        code_challenge=challenge,
        redirect_uri="http://127.0.0.1/cb",
    )

    non_ascii_resp = raw.request(
        "POST",
        "/token",
        headers={"Content-Type": "application/x-www-form-urlencoded"},
        body=urlencode(
            {
                "grant_type": "authorization_code",
                "code": code,
                "code_verifier": "bad_é_verifier",
                "redirect_uri": "http://127.0.0.1/cb",
            }
        ),
    )
    non_ascii_resp.assert_error_code("invalid_grant", 400)

    bad_token_json = raw.request(
        "POST",
        "/token",
        headers={"Content-Type": "application/json"},
        body=b"{not valid json",
    )
    bad_token_json.assert_error_code("invalid_request", 400)

    bad_reg_json = raw.request(
        "POST",
        "/register",
        headers={"Content-Type": "application/json"},
        body=b"{not valid json",
    )
    bad_reg_json.assert_error_code("invalid_request", 400)


def test_server_handle_startup_failure_cleans_log_fd_tempdir_and_hygiene(
    temp_workspace: TempWorkspace,
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    # 1. Missing binary inside context manager cleans temp dir with zero ResourceWarnings
    missing_bin = temp_workspace.path / "missing_srv_bin"
    with warnings.catch_warnings(record=True) as caught:
        warnings.simplefilter("always", ResourceWarning)
        handle_missing = OrcaServerHandle(server_bin=missing_bin)
        missing_dir = handle_missing.data_dir
        assert missing_dir.is_dir()
        with pytest.raises(AssertionError):
            with handle_missing:
                pass
        assert handle_missing._log_file is None
        assert handle_missing._proc is None
        assert not missing_dir.exists()
        del handle_missing
        gc.collect()
    res_warns = [w for w in caught if issubclass(w.category, ResourceWarning)]
    assert res_warns == [], f"Unexpected ResourceWarnings on missing bin: {res_warns}"

    # 2. Crashing binary flushes logs, closes _log_file, and cleans temp dir cleanly
    crash_script = temp_workspace.write(
        "crash_server.sh",
        "#!/bin/sh\necho 'boom-startup-crash-log' >&2\nexit 42\n",
    )
    crash_script.chmod(crash_script.stat().st_mode | stat.S_IXUSR)

    with warnings.catch_warnings(record=True) as caught_crash:
        warnings.simplefilter("always", ResourceWarning)
        handle_crash = OrcaServerHandle(server_bin=crash_script, startup_timeout=3.0)
        crash_dir = handle_crash.data_dir
        with pytest.raises(AssertionError) as crash_exc:
            with handle_crash:
                pass
        msg = str(crash_exc.value)
        assert "code 42" in msg
        assert "boom-startup-crash-log" in msg
        assert handle_crash._log_file is None
        assert handle_crash._proc is None
        assert not crash_dir.exists()
        del handle_crash
        gc.collect()
    crash_warns = [w for w in caught_crash if issubclass(w.category, ResourceWarning)]
    assert crash_warns == [], f"Unexpected ResourceWarnings on crash: {crash_warns}"

    # 3. OrcaCliRunner strips ambient RUST_LOG by default while allowing explicit extra_env override
    monkeypatch.setenv("RUST_LOG", "debug")
    cli = OrcaCliRunner(
        cli_bin=temp_workspace.path / "dummy_cli",
        server_url="http://127.0.0.1:18080",
        work_dir=temp_workspace.mkdir("cli_env_check"),
    )
    default_env = cli._build_env()
    assert "RUST_LOG" not in default_env
    override_env = cli._build_env(extra_env={"RUST_LOG": "trace"})
    assert override_env["RUST_LOG"] == "trace"

    # 4. SqliteFixtureHelper.set_user_version rejects bool explicitly
    sql_helper = SqliteFixtureHelper(temp_workspace.resolve("bool_guard.db"))
    with pytest.raises(ValueError):
        sql_helper.set_user_version(True)  # type: ignore[arg-type]
    with pytest.raises(ValueError):
        sql_helper.set_user_version(False)  # type: ignore[arg-type]


def test_run_e2e_combines_tier_and_markers_and_handles_color_flag() -> None:
    import run_e2e

    suite_str = str(run_e2e.SUITE_DIR)

    for variant in (["--tier", "1", "-m", "f01"], ["--tier", "1", "-m=f01"], ["--tier", "1", "-mf01"]):
        argv = run_e2e.build_pytest_argv(variant)
        m_indices = [i for i, tok in enumerate(argv) if tok == "-m"]
        assert len(m_indices) == 1, f"Expected single -m for {variant}: {argv}"
        assert argv[m_indices[0] + 1] == "(tier1) and (f01)"
        assert argv[-1] == suite_str

    # Value-taking pytest options such as --color yes MUST NOT be mistaken for test targets
    color_argv = run_e2e.build_pytest_argv(["--tier", "self", "--color", "yes", "--collect-only", "-q"])
    assert color_argv[-1] == suite_str
    assert "--color" in color_argv and "yes" in color_argv

    # Unknown value-taking flag with non-path value MUST NOT be mistaken for a test target
    unknown_argv = run_e2e.build_pytest_argv(["--tier", "self", "--future-opt", "yes"])
    assert unknown_argv[-1] == suite_str

    # Explicit test path MUST suppress appending default SUITE_DIR
    explicit_path = "tests/orca_e2e/test_harness_self_check.py"
    explicit_argv = run_e2e.build_pytest_argv(["--tier", "1", explicit_path])
    assert explicit_argv[-1] == explicit_path
    assert suite_str not in explicit_argv

    # Explicit -m without --tier preserves user marker untouched; bare call defaults to all tiers
    user_only = run_e2e.build_pytest_argv(["-m", "f01"])
    assert user_only[user_only.index("-m") + 1] == "f01"
    default_argv = run_e2e.build_pytest_argv([])
    assert default_argv[default_argv.index("-m") + 1] == "tier1 or tier2 or tier3 or tier4"


def _send_raw_http_probe(
    host: str,
    port: int,
    raw_request: bytes,
    *,
    half_close: bool = False,
    timeout: float = 2.0,
) -> tuple[int, dict[str, str], bytes]:
    with socket.create_connection((host, port), timeout=timeout) as sock:
        sock.settimeout(timeout)
        sock.sendall(raw_request)
        if half_close:
            sock.shutdown(socket.SHUT_WR)
        chunks: list[bytes] = []
        while True:
            chunk = sock.recv(4096)
            if not chunk:
                break
            chunks.append(chunk)
    raw = b"".join(chunks)
    assert b"\r\n\r\n" in raw, f"Incomplete HTTP response: {raw!r}"
    head, body = raw.split(b"\r\n\r\n", 1)
    lines = head.decode("iso-8859-1").split("\r\n")
    status = int(lines[0].split(" ", 2)[1])
    headers: dict[str, str] = {}
    for line in lines[1:]:
        if ":" in line:
            hk, hv = line.split(":", 1)
            headers[hk.strip().lower()] = hv.strip()
    return status, headers, body


@pytest.mark.harness_self_check
def test_mock_servers_reject_malformed_chunked_and_content_length_with_400() -> None:
    malformed_cases: list[tuple[str, str, bytes, bool]] = [
        ("missing_crlf_before_0", "Transfer-Encoding: chunked", b"5\r\nhello0\r\n\r\n", False),
        ("ext_only_no_size", "Transfer-Encoding: chunked", b";ext=1\r\n", False),
        ("non_hex_size", "Transfer-Encoding: chunked", b"ZZZ\r\nhello\r\n0\r\n\r\n", False),
        ("hex_0x_prefix", "Transfer-Encoding: chunked", b"0x5\r\nhello\r\n0\r\n\r\n", False),
        ("truncated_payload", "Transfer-Encoding: chunked", b"10\r\nhi", True),
        ("eof_before_0_chunk", "Transfer-Encoding: chunked", b"5\r\nhello\r\n", True),
        ("garbage_delimiter", "Transfer-Encoding: chunked", b"5\r\nhelloGARBAGE\r\n0\r\n\r\n", False),
        ("truncated_trailers", "Transfer-Encoding: chunked", b"0\r\nX-Tr: 1\r\n", True),
        ("negative_content_length", "Content-Length: -5", b"", False),
        ("non_numeric_content_length", "Content-Length: abc", b"", False),
        ("truncated_content_length", "Content-Length: 20", b"hi", True),
    ]

    with (
        MockMcpUpstreamServer() as mcp,
        MockOAuthServer() as oauth,
        MockWebhookSink() as sink,
    ):
        targets: list[tuple[str, Any, str]] = [
            ("mcp", mcp, "/mcp"),
            ("oauth", oauth, "/token"),
            ("webhook", sink, "/hook"),
        ]
        for label, srv, path in targets:
            for case_name, framing_hdr, body_bytes, half_close in malformed_cases:
                raw_req = (
                    f"POST {path} HTTP/1.1\r\n"
                    f"Host: {srv.host}:{srv.port}\r\n"
                    f"Content-Type: application/json\r\n"
                    f"{framing_hdr}\r\n\r\n"
                ).encode("ascii") + body_bytes
                status, hdrs, resp_body = _send_raw_http_probe(
                    srv.host, srv.port, raw_req, half_close=half_close, timeout=2.0
                )
                assert status == 400, (
                    f"{label}/{case_name}: expected HTTP 400, got {status} body={resp_body!r}"
                )
                assert hdrs.get("connection", "").lower() == "close", (
                    f"{label}/{case_name}: expected Connection: close, got {hdrs!r}"
                )
                parsed = json.loads(resp_body.decode("utf-8"))
                assert parsed["error"] == "invalid_request"
                assert parsed.get("error_description")

            assert len(srv.requests) == 0, (
                f"{label}: expected 0 recorded corrupted requests, got {len(srv.requests)}"
            )

        # Confirm all 3 servers remain healthy and process valid requests afterwards
        RawHttpClient(mcp.host, mcp.port).request(
            "POST", "/mcp", json_body={"jsonrpc": "2.0", "id": 1}
        ).assert_status(200)
        assert len(mcp.requests) == 1

        RawHttpClient(oauth.host, oauth.port).request(
            "POST", "/register", json_body={"client_name": "healthy"}
        ).assert_status(201)
        assert len(oauth.requests) == 1

        RawHttpClient(sink.host, sink.port).request(
            "POST", "/hook", json_body={"ok": True}
        ).assert_status(200)
        assert len(sink.requests) == 1
