"""Loopback mock servers for EXTERNAL third-party upstreams ONLY.

NEVER mocks Orca product code. Provides controllable local third-party servers for:
- MockMcpUpstreamServer: remote MCP HTTP/SSE endpoint (401 refresh retry, SSE abort detection)
- MockOAuthServer: external OAuth 2.1 + PKCE S256 + RFC 7591 DCR + /token + /revoke
- MockWebhookSink: external outbound HTTP receiver (Slack Mirror barrier & webhook triggers)
"""

from __future__ import annotations

import base64
import hashlib
import json
import secrets
import select
import socket
import threading
import time
from dataclasses import dataclass
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from typing import Any, Callable
from urllib.parse import parse_qs, urlparse


@dataclass(frozen=True)
class RecordedRequest:
    """Captured wire request received by a mock third-party upstream server."""

    method: str
    path: str
    headers: dict[str, str]
    body: bytes
    timestamp_monotonic: float
    json_body: Any = None


class _QuietThreadingHTTPServer(ThreadingHTTPServer):
    allow_reuse_address = True
    daemon_threads = True
    request_queue_size = 256


class _LoopbackMockBase:
    """Shared lifecycle management for loopback ThreadingHTTPServer mocks."""

    def __init__(self, handler_cls: type[BaseHTTPRequestHandler]) -> None:
        self._lock = getattr(self, "_lock", None) or threading.Lock()
        self._shutdown_event = threading.Event()
        self.requests: list[RecordedRequest] = []
        self._httpd = _QuietThreadingHTTPServer(("127.0.0.1", 0), handler_cls)
        self._httpd.mock_owner = self  # type: ignore[attr-defined]
        self.host, self.port = self._httpd.server_address[:2]
        self._thread = threading.Thread(
            target=self._httpd.serve_forever,
            kwargs={"poll_interval": 0.02},
            daemon=True,
        )
        self._thread.start()

    @property
    def base_url(self) -> str:
        return f"http://{self.host}:{self.port}"

    def _record(
        self, method: str, path: str, headers: dict[str, str], body: bytes
    ) -> RecordedRequest:
        parsed_json: Any = None
        if body:
            try:
                parsed_json = json.loads(body.decode("utf-8"))
            except Exception:
                parsed_json = None
        with self._lock:
            rec = RecordedRequest(
                method=method,
                path=path,
                headers=headers,
                body=body,
                timestamp_monotonic=time.monotonic(),
                json_body=parsed_json,
            )
            self.requests.append(rec)
        return rec

    def stop(self) -> None:
        self._shutdown_event.set()
        try:
            self._httpd.shutdown()
        except Exception:
            pass
        try:
            self._httpd.server_close()
        except Exception:
            pass
        if self._thread.is_alive():
            self._thread.join(timeout=3.0)

    def __enter__(self) -> Any:
        return self

    def __exit__(self, exc_type: Any, exc: Any, tb: Any) -> None:
        self.stop()


_HEX_DIGITS = frozenset(b"0123456789abcdefABCDEF")
_MAX_MOCK_BODY_BYTES = 64 * 1024 * 1024


def _read_chunked_body(rfile: Any) -> bytes:
    chunks: list[bytes] = []
    total_bytes = 0
    while True:
        size_line = rfile.readline()
        if not size_line or not size_line.endswith(b"\n"):
            raise ValueError("Truncated chunked body before terminating 0-chunk")
        line_core = size_line[:-2] if size_line.endswith(b"\r\n") else size_line[:-1]
        size_token = line_core.split(b";", 1)[0].strip(b" \t")
        if not size_token or not all(b in _HEX_DIGITS for b in size_token):
            raise ValueError(f"Malformed chunk size: {size_line!r}")
        chunk_size = int(size_token, 16)
        if chunk_size == 0:
            while True:
                trailer = rfile.readline()
                if not trailer or not trailer.endswith(b"\n"):
                    raise ValueError("Truncated chunked trailers")
                if trailer in (b"\r\n", b"\n"):
                    break
            break
        if chunk_size > _MAX_MOCK_BODY_BYTES or total_bytes + chunk_size > _MAX_MOCK_BODY_BYTES:
            raise ValueError(f"Chunked body exceeds limit ({_MAX_MOCK_BODY_BYTES} bytes)")
        chunk_data = rfile.read(chunk_size)
        if len(chunk_data) != chunk_size:
            raise ValueError(
                f"Truncated chunk payload: expected {chunk_size} bytes, got {len(chunk_data)}"
            )
        delim = rfile.readline()
        if delim not in (b"\r\n", b"\n"):
            raise ValueError(f"Malformed chunk delimiter: {delim!r}")
        chunks.append(chunk_data)
        total_bytes += chunk_size
    return b"".join(chunks)


def _read_request_body(handler: BaseHTTPRequestHandler) -> bytes:
    te_values = handler.headers.get_all("Transfer-Encoding") or []
    te_joined = ",".join(te_values).lower()
    if "chunked" in te_joined:
        return _read_chunked_body(handler.rfile)
    cl_values = handler.headers.get_all("Content-Length") or []
    if not cl_values:
        return b""
    normalized = [v.strip() for v in cl_values]
    if len(set(normalized)) > 1:
        raise ValueError(f"Conflicting Content-Length headers: {cl_values!r}")
    raw_len = normalized[0]
    if not raw_len or not raw_len.isascii() or not raw_len.isdigit():
        raise ValueError(f"Invalid Content-Length header: {cl_values[0]!r}")
    length = int(raw_len)
    if length == 0:
        return b""
    if length > _MAX_MOCK_BODY_BYTES:
        raise ValueError(f"Content-Length exceeds limit ({_MAX_MOCK_BODY_BYTES} bytes)")
    data = handler.rfile.read(length)
    if len(data) != length:
        raise ValueError(
            f"Truncated request body: expected {length} bytes, got {len(data)}"
        )
    return data


def _send_bad_request(handler: BaseHTTPRequestHandler, exc: Exception) -> None:
    handler.close_connection = True
    payload = json.dumps(
        {"error": "invalid_request", "error_description": str(exc)}
    ).encode("utf-8")
    try:
        handler.send_response(400)
        handler.send_header("Content-Type", "application/json")
        handler.send_header("Content-Length", str(len(payload)))
        handler.send_header("Connection", "close")
        handler.end_headers()
        handler.wfile.write(payload)
        handler.wfile.flush()
    except (BrokenPipeError, ConnectionResetError, TimeoutError, socket.timeout, OSError):
        pass


def _lower_headers(handler: BaseHTTPRequestHandler) -> dict[str, str]:
    return {k.lower(): v for k, v in handler.headers.items()}


class _McpHandler(BaseHTTPRequestHandler):
    protocol_version = "HTTP/1.1"
    timeout = 5.0

    def log_message(self, format: str, *args: Any) -> None:
        return

    def _dispatch(self, method: str) -> None:
        owner: MockMcpUpstreamServer = self.server.mock_owner  # type: ignore[attr-defined]
        try:
            body = _read_request_body(self)
        except (ValueError, TimeoutError, socket.timeout) as exc:
            _send_bad_request(self, exc)
            return
        hdrs = _lower_headers(self)
        rec = owner._record(method, self.path, hdrs, body)
        parsed_path = urlparse(self.path).path

        if owner.custom_responder is not None:
            status, resp_hdrs, resp_body = owner.custom_responder(rec)
            self._send_bytes(status, resp_body, resp_hdrs)
            return

        with owner._lock:
            should_fail_401 = owner.fail_401_once and owner._failed_401_count == 0
            if should_fail_401:
                owner._failed_401_count += 1

        if should_fail_401:
            self._send_json(
                401,
                {"error": "invalid_token", "detail": "token expired"},
                {"WWW-Authenticate": 'Bearer realm="mock-mcp", error="invalid_token"'},
            )
            return

        if owner.expected_bearer is not None:
            auth = hdrs.get("authorization", "")
            if auth != f"Bearer {owner.expected_bearer}":
                self._send_json(
                    401,
                    {"error": "invalid_token"},
                    {"WWW-Authenticate": 'Bearer realm="mock-mcp"'},
                )
                return

        if parsed_path == owner.sse_stream_path:
            self._handle_sse(owner)
            return

        req_id = rec.json_body.get("id", 1) if isinstance(rec.json_body, dict) else 1
        session_id = hdrs.get("mcp-session-id") or owner.session_id
        self._send_json(
            200,
            {
                "jsonrpc": "2.0",
                "id": req_id,
                "result": {
                    "ok": True,
                    "method": method,
                    "path": self.path,
                    "echo": rec.json_body,
                    "received_headers": hdrs,
                },
            },
            {"Mcp-Session-Id": session_id},
        )

    def _handle_sse(self, owner: MockMcpUpstreamServer) -> None:
        self.send_response(200)
        self.send_header("Content-Type", "text/event-stream")
        self.send_header("Cache-Control", "no-cache")
        self.send_header("Connection", "close")
        self.end_headers()
        try:
            self.wfile.write(b'event: ready\ndata: {"status":"streaming"}\n\n')
            self.wfile.flush()
            owner.stream_started.set()
            while not owner._shutdown_event.is_set():
                readable, _, _ = select.select([self.connection], [], [], 0.05)
                if readable:
                    chunk = self.connection.recv(4096)
                    if chunk == b"":
                        owner.stream_aborted.set()
                        return
                self.wfile.write(b": keepalive\n\n")
                self.wfile.flush()
        except (BrokenPipeError, ConnectionResetError, OSError):
            owner.stream_aborted.set()

    def _send_json(
        self, status: int, payload: Any, extra_headers: dict[str, str] | None = None
    ) -> None:
        raw = json.dumps(payload).encode("utf-8")
        hdrs = {"Content-Type": "application/json"}
        if extra_headers:
            hdrs.update(extra_headers)
        self._send_bytes(status, raw, hdrs)

    def _send_bytes(
        self, status: int, body: bytes, headers: dict[str, str] | None = None
    ) -> None:
        self.send_response(status)
        for k, v in (headers or {}).items():
            self.send_header(k, v)
        self.send_header("Content-Length", str(len(body)))
        self.send_header("Connection", "close")
        self.end_headers()
        if body:
            self.wfile.write(body)

    def do_GET(self) -> None:
        self._dispatch("GET")

    def do_POST(self) -> None:
        self._dispatch("POST")

    def do_DELETE(self) -> None:
        self._dispatch("DELETE")


class MockMcpUpstreamServer(_LoopbackMockBase):
    """Controllable loopback external MCP HTTP + SSE upstream server."""

    def __init__(
        self,
        *,
        fail_401_once: bool = False,
        expected_bearer: str | None = None,
        sse_stream_path: str = "/sse",
        custom_responder: Callable[[RecordedRequest], tuple[int, dict[str, str], bytes]] | None = None,
    ) -> None:
        self.fail_401_once = fail_401_once
        self.expected_bearer = expected_bearer
        self.sse_stream_path = sse_stream_path
        self.custom_responder = custom_responder
        self.session_id = f"mcp-sess-{secrets.token_hex(6)}"
        self._failed_401_count = 0
        self.stream_started = threading.Event()
        self.stream_aborted = threading.Event()
        super().__init__(_McpHandler)

    def wait_for_stream_start(self, timeout: float = 5.0) -> bool:
        return self.stream_started.wait(timeout=timeout)

    def wait_for_stream_abort(self, timeout: float = 5.0) -> bool:
        return self.stream_aborted.wait(timeout=timeout)


def pkce_s256_challenge(code_verifier: str) -> str:
    """Compute RFC 7636 S256 PKCE code_challenge from a code_verifier."""
    digest = hashlib.sha256(code_verifier.encode("ascii")).digest()
    return base64.urlsafe_b64encode(digest).rstrip(b"=").decode("ascii")


def _parse_form_or_json(body: bytes, content_type: str) -> dict[str, str]:
    if not body:
        return {}
    if "application/json" in content_type.lower():
        raw = json.loads(body.decode("utf-8"))
        if not isinstance(raw, dict):
            raise ValueError("JSON request body must be an object")
        return {str(k): str(v) for k, v in raw.items()}
    parsed = parse_qs(body.decode("utf-8", errors="replace"), keep_blank_values=True)
    return {k: v[0] if v else "" for k, v in parsed.items()}


class _OAuthHandler(BaseHTTPRequestHandler):
    protocol_version = "HTTP/1.1"
    timeout = 5.0

    def log_message(self, format: str, *args: Any) -> None:
        return

    def _send_json(self, status: int, payload: Any) -> None:
        raw = json.dumps(payload).encode("utf-8")
        self.send_response(status)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(raw)))
        self.send_header("Connection", "close")
        self.end_headers()
        self.wfile.write(raw)

    def do_GET(self) -> None:
        owner: MockOAuthServer = self.server.mock_owner  # type: ignore[attr-defined]
        hdrs = _lower_headers(self)
        owner._record("GET", self.path, hdrs, b"")
        parsed = urlparse(self.path)

        if parsed.path == "/.well-known/oauth-authorization-server":
            self._send_json(
                200,
                {
                    "issuer": owner.issuer,
                    "authorization_endpoint": f"{owner.base_url}/authorize",
                    "token_endpoint": f"{owner.base_url}/token",
                    "registration_endpoint": f"{owner.base_url}/register",
                    "revocation_endpoint": f"{owner.base_url}/revoke",
                    "response_types_supported": ["code"],
                    "grant_types_supported": ["authorization_code", "refresh_token"],
                    "code_challenge_methods_supported": ["S256"],
                    "token_endpoint_auth_methods_supported": ["none", "client_secret_post"],
                },
            )
            return

        if parsed.path == "/authorize":
            qs = {k: v[0] for k, v in parse_qs(parsed.query).items() if v}
            code = owner.issue_auth_code(
                client_id=qs.get("client_id", ""),
                code_challenge=qs.get("code_challenge", ""),
                redirect_uri=qs.get("redirect_uri", ""),
                resource=qs.get("resource"),
                scope=qs.get("scope", "mcp"),
            )
            self._send_json(200, {"code": code, "state": qs.get("state", ""), "iss": owner.issuer})
            return

        self._send_json(404, {"error": "not_found"})

    def do_POST(self) -> None:
        owner: MockOAuthServer = self.server.mock_owner  # type: ignore[attr-defined]
        try:
            body = _read_request_body(self)
        except (ValueError, TimeoutError, socket.timeout) as exc:
            _send_bad_request(self, exc)
            return
        hdrs = _lower_headers(self)
        rec = owner._record("POST", self.path, hdrs, body)
        parsed = urlparse(self.path)

        if parsed.path == "/register":
            if body:
                try:
                    decoded = json.loads(body.decode("utf-8"))
                except (UnicodeDecodeError, json.JSONDecodeError, ValueError):
                    self._send_json(
                        400,
                        {"error": "invalid_request", "error_description": "malformed JSON body"},
                    )
                    return
                if not isinstance(decoded, dict):
                    self._send_json(
                        400,
                        {"error": "invalid_request", "error_description": "JSON object required"},
                    )
                    return
                payload = decoded
            else:
                payload = {}
            client_id = f"dcr_client_{secrets.token_hex(6)}"
            client_secret = f"dcr_secret_{secrets.token_hex(8)}"
            record = {
                "client_id": client_id,
                "client_secret": client_secret,
                "client_name": payload.get("client_name", "Orca E2E Client"),
                "redirect_uris": payload.get("redirect_uris", ["http://127.0.0.1/callback"]),
                "token_endpoint_auth_method": payload.get("token_endpoint_auth_method", "none"),
            }
            with owner._lock:
                owner.registered_clients[client_id] = record
            self._send_json(201, record)
            return

        if parsed.path == "/token":
            try:
                params = _parse_form_or_json(body, hdrs.get("content-type", ""))
            except (UnicodeDecodeError, json.JSONDecodeError, ValueError):
                self._send_json(
                    400,
                    {"error": "invalid_request", "error_description": "malformed token request body"},
                )
                return
            self._handle_token(owner, params)
            return

        if parsed.path == "/revoke":
            try:
                params = _parse_form_or_json(body, hdrs.get("content-type", ""))
            except (UnicodeDecodeError, json.JSONDecodeError, ValueError):
                self._send_json(
                    400,
                    {"error": "invalid_request", "error_description": "malformed revoke request body"},
                )
                return
            token = params.get("token", "")
            with owner._lock:
                if token:
                    owner.revoked_tokens.add(token)
                    owner.revoke_calls.append(params)
            self._send_json(200, {"revoked": True})
            return

        self._send_json(404, {"error": "not_found"})

    def _handle_token(self, owner: MockOAuthServer, params: dict[str, str]) -> None:
        grant_type = params.get("grant_type", "")
        status = 400
        response: dict[str, Any] = {"error": "unsupported_grant_type"}

        with owner._lock:
            owner.token_calls.append(params)

            if grant_type == "authorization_code":
                code = params.get("code", "")
                verifier = params.get("code_verifier", "")
                entry = owner._auth_codes.pop(code, None)
                if entry is None:
                    status, response = 400, {
                        "error": "invalid_grant",
                        "error_description": "unknown code",
                    }
                else:
                    pkce_ok = True
                    pkce_err = "PKCE S256 mismatch"
                    if entry["code_challenge"]:
                        try:
                            computed = pkce_s256_challenge(verifier)
                            pkce_ok = computed == entry["code_challenge"]
                        except (UnicodeEncodeError, ValueError):
                            pkce_ok = False
                            pkce_err = "invalid code_verifier encoding"

                    if not pkce_ok:
                        status, response = 400, {
                            "error": "invalid_grant",
                            "error_description": pkce_err,
                        }
                    elif entry["redirect_uri"] and params.get("redirect_uri") not in (
                        None,
                        "",
                        entry["redirect_uri"],
                    ):
                        status, response = 400, {
                            "error": "invalid_grant",
                            "error_description": "redirect_uri mismatch",
                        }
                    else:
                        owner._token_seq += 1
                        access_token = f"mock_access_{owner._token_seq}_{secrets.token_hex(6)}"
                        refresh_token = f"mock_refresh_{owner._token_seq}_{secrets.token_hex(6)}"
                        owner.active_access_tokens.add(access_token)
                        owner.active_refresh_tokens.add(refresh_token)
                        status, response = 200, {
                            "access_token": access_token,
                            "refresh_token": refresh_token,
                            "token_type": "Bearer",
                            "expires_in": 3600,
                            "scope": entry.get("scope", "mcp"),
                        }

            elif grant_type == "refresh_token":
                rt = params.get("refresh_token", "")
                owner.refresh_calls.append(params)
                if not rt or rt in owner.revoked_tokens or rt not in owner.active_refresh_tokens:
                    status, response = 400, {
                        "error": "invalid_grant",
                        "error_description": "refresh token invalid or revoked",
                    }
                else:
                    owner._token_seq += 1
                    new_access = f"mock_access_{owner._token_seq}_{secrets.token_hex(6)}"
                    new_refresh = f"mock_refresh_{owner._token_seq}_{secrets.token_hex(6)}"
                    owner.active_access_tokens.add(new_access)
                    owner.active_refresh_tokens.add(new_refresh)
                    status, response = 200, {
                        "access_token": new_access,
                        "refresh_token": new_refresh,
                        "token_type": "Bearer",
                        "expires_in": 3600,
                    }

        self._send_json(status, response)


class MockOAuthServer(_LoopbackMockBase):
    """Loopback OAuth 2.1 + PKCE S256 + RFC 7591 DCR + RFC 7009 Revoke mock server."""

    def __init__(self, issuer_override: str | None = None) -> None:
        self._issuer_override = issuer_override
        self._auth_codes: dict[str, dict[str, Any]] = {}
        self._token_seq = 0
        self.registered_clients: dict[str, dict[str, Any]] = {}
        self.active_access_tokens: set[str] = set()
        self.active_refresh_tokens: set[str] = set()
        self.revoked_tokens: set[str] = set()
        self.token_calls: list[dict[str, str]] = []
        self.refresh_calls: list[dict[str, str]] = []
        self.revoke_calls: list[dict[str, str]] = []
        super().__init__(_OAuthHandler)

    @property
    def issuer(self) -> str:
        return self._issuer_override or self.base_url

    def issue_auth_code(
        self,
        *,
        client_id: str,
        code_challenge: str,
        redirect_uri: str = "http://127.0.0.1/callback",
        resource: str | None = None,
        scope: str = "mcp",
    ) -> str:
        code = f"authcode_{secrets.token_hex(8)}"
        with self._lock:
            self._auth_codes[code] = {
                "client_id": client_id,
                "code_challenge": code_challenge,
                "redirect_uri": redirect_uri,
                "resource": resource,
                "scope": scope,
            }
        return code


class _WebhookSinkHandler(BaseHTTPRequestHandler):
    protocol_version = "HTTP/1.1"
    timeout = 5.0

    def log_message(self, format: str, *args: Any) -> None:
        return

    def _reply(self, status: int, payload: Any) -> None:
        raw = json.dumps(payload).encode("utf-8")
        self.send_response(status)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(raw)))
        self.send_header("Connection", "close")
        self.end_headers()
        self.wfile.write(raw)

    def do_GET(self) -> None:
        self._reply(200, {"ok": True})

    def do_POST(self) -> None:
        owner: MockWebhookSink = self.server.mock_owner  # type: ignore[attr-defined]
        try:
            body = _read_request_body(self)
        except (ValueError, TimeoutError, socket.timeout) as exc:
            _send_bad_request(self, exc)
            return
        hdrs = _lower_headers(self)
        delay = owner.path_delays.get(urlparse(self.path).path, owner.default_delay)
        if delay > 0:
            time.sleep(delay)
        rec = owner._record("POST", self.path, hdrs, body)
        with owner._cond:
            received_seq = len(owner.requests)
            owner._cond.notify_all()
        status = owner.response_status
        self._reply(status, {"ok": status < 400, "received_seq": received_seq, "path": rec.path})


class MockWebhookSink(_LoopbackMockBase):
    """Loopback outbound HTTP receiver recording payloads, headers & monotonic timestamps."""

    def __init__(
        self,
        *,
        response_status: int = 200,
        default_delay: float = 0.0,
        path_delays: dict[str, float] | None = None,
    ) -> None:
        self.response_status = response_status
        self.default_delay = default_delay
        self.path_delays: dict[str, float] = dict(path_delays or {})
        self._lock = threading.Lock()
        self._cond = threading.Condition(self._lock)
        super().__init__(_WebhookSinkHandler)

    def wait_for_requests(self, count: int, timeout: float = 5.0) -> list[RecordedRequest]:
        deadline = time.monotonic() + timeout
        with self._cond:
            while len(self.requests) < count:
                remaining = deadline - time.monotonic()
                if remaining <= 0:
                    raise TimeoutError(
                        f"MockWebhookSink expected {count} requests within {timeout:.1f}s, "
                        f"received {len(self.requests)}"
                    )
                self._cond.wait(timeout=remaining)
            return list(self.requests)
