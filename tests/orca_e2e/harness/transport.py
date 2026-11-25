"""Pure-stdlib HTTP/REST/RPC and RFC 6455 WebSocket transport primitives.

Uses http.client.HTTPConnection (with skip_host support for authentic Host/Origin
wire tests) and socket/struct/hashlib/base64 for RFC 6455 WebSocket framing with
explicit Close-frame code extraction (e.g., 4409 supersede, 1009 oversize).
"""

from __future__ import annotations

import base64
import hashlib
import http.client
import json
import os
import socket
import ssl
import struct
import time
import uuid
from dataclasses import dataclass
from typing import Any
from urllib.parse import urlencode, urlparse

WS_GUID = "258EAFA5-E914-47DA-95CA-C5AB0DC85B11"


@dataclass(frozen=True)
class HttpResponse:
    """Immutable HTTP response wrapper with fail-closed assertion helpers."""

    status: int
    reason: str
    headers: dict[str, str]
    body: bytes

    @property
    def text(self) -> str:
        return self.body.decode("utf-8", errors="replace")

    def header(self, name: str, default: str | None = None) -> str | None:
        return self.headers.get(name.lower(), default)

    def json(self) -> Any:
        if not self.body:
            raise AssertionError(
                f"Expected JSON body but got empty body (HTTP {self.status} {self.reason})"
            )
        try:
            return json.loads(self.body.decode("utf-8"))
        except Exception as exc:
            raise AssertionError(
                f"Expected valid JSON body (HTTP {self.status} {self.reason}), "
                f"got decode error {exc}: {self.text[:800]!r}"
            ) from exc

    def assert_status(self, *codes: int) -> HttpResponse:
        if self.status not in codes:
            raise AssertionError(
                f"Expected HTTP status in {codes}, got {self.status} {self.reason}; "
                f"body={self.text[:1000]!r}"
            )
        return self

    def extract_error_code(self) -> str | None:
        if not self.body:
            return None
        try:
            payload = json.loads(self.body.decode("utf-8"))
        except Exception:
            return None
        if not isinstance(payload, dict):
            return None
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

    def assert_error_code(
        self, expected_code: str, *allowed_statuses: int
    ) -> HttpResponse:
        if allowed_statuses and self.status not in allowed_statuses:
            raise AssertionError(
                f"Expected HTTP status in {allowed_statuses} for error_code={expected_code!r}, "
                f"got {self.status} {self.reason}; body={self.text[:1000]!r}"
            )
        actual = self.extract_error_code()
        if actual != expected_code:
            raise AssertionError(
                f"Expected error code {expected_code!r}, got {actual!r} "
                f"(HTTP {self.status} {self.reason}); body={self.text[:1000]!r}"
            )
        return self


class RawHttpClient:
    """Low-level HTTP/HTTPS client preserving exact Host, Origin, and HTTP method."""

    def __init__(
        self,
        host: str,
        port: int,
        timeout: float = 15.0,
        scheme: str = "http",
    ) -> None:
        self.host = host
        self.port = port
        self.timeout = timeout
        self.scheme = scheme

    def _open(self) -> http.client.HTTPConnection:
        if self.scheme == "https":
            return http.client.HTTPSConnection(self.host, self.port, timeout=self.timeout)
        return http.client.HTTPConnection(self.host, self.port, timeout=self.timeout)

    def request(
        self,
        method: str,
        path: str,
        headers: dict[str, str] | None = None,
        body: bytes | str | None = None,
        json_body: Any = None,
    ) -> HttpResponse:
        hdrs: dict[str, str] = dict(headers or {})
        lower_keys = {k.lower() for k in hdrs}

        raw_body: bytes | None
        if json_body is not None:
            raw_body = json.dumps(json_body).encode("utf-8")
            if "content-type" not in lower_keys:
                hdrs["Content-Type"] = "application/json"
                lower_keys.add("content-type")
        elif isinstance(body, str):
            raw_body = body.encode("utf-8")
        else:
            raw_body = body

        has_custom_host = "host" in lower_keys
        has_custom_accept_encoding = "accept-encoding" in lower_keys
        conn = self._open()
        try:
            conn.putrequest(
                method,
                path,
                skip_host=has_custom_host,
                skip_accept_encoding=has_custom_accept_encoding,
            )
            for key, value in hdrs.items():
                conn.putheader(key, value)
            if (
                raw_body is not None
                and "content-length" not in lower_keys
                and "transfer-encoding" not in lower_keys
            ):
                conn.putheader("Content-Length", str(len(raw_body)))
            conn.endheaders(message_body=raw_body)
            resp = conn.getresponse()
            resp_body = resp.read()
            resp_headers = {k.lower(): v for k, v in resp.getheaders()}
            return HttpResponse(
                status=resp.status,
                reason=resp.reason,
                headers=resp_headers,
                body=resp_body,
            )
        finally:
            conn.close()


class OrcaApiClient:
    """High-level opaque-box REST, Unified RPC, and WebSocket client for Orca Server."""

    def __init__(
        self,
        base_url: str,
        token: str | None = None,
        timeout: float = 15.0,
    ) -> None:
        self.base_url = base_url.rstrip("/")
        parsed = urlparse(self.base_url)
        scheme = parsed.scheme or "http"
        host = parsed.hostname or "127.0.0.1"
        port = parsed.port or (443 if scheme == "https" else 80)
        self.base_path = parsed.path.rstrip("/")
        self.token = token
        self.timeout = timeout
        self.raw = RawHttpClient(host=host, port=port, timeout=timeout, scheme=scheme)

    def with_token(self, token: str | None) -> OrcaApiClient:
        return OrcaApiClient(self.base_url, token=token, timeout=self.timeout)

    def _build_path(self, path: str, params: dict[str, Any] | None = None) -> str:
        rel = path if path.startswith("/") else f"/{path}"
        full = f"{self.base_path}{rel}"
        if params:
            sep = "&" if "?" in full else "?"
            full = f"{full}{sep}{urlencode(params, doseq=True)}"
        return full

    def raw_request(
        self,
        method: str,
        path: str,
        headers: dict[str, str] | None = None,
        body: bytes | str | None = None,
        json_body: Any = None,
        include_auth: bool = True,
        params: dict[str, Any] | None = None,
    ) -> HttpResponse:
        hdrs = dict(headers or {})
        lower_keys = {k.lower() for k in hdrs}
        if include_auth and self.token and "authorization" not in lower_keys:
            hdrs["Authorization"] = f"Bearer {self.token}"
        return self.raw.request(
            method,
            self._build_path(path, params=params),
            headers=hdrs,
            body=body,
            json_body=json_body,
        )

    def get(
        self,
        path: str,
        headers: dict[str, str] | None = None,
        params: dict[str, Any] | None = None,
        include_auth: bool = True,
    ) -> HttpResponse:
        return self.raw_request(
            "GET", path, headers=headers, params=params, include_auth=include_auth
        )

    def post(
        self,
        path: str,
        json_body: Any = None,
        body: bytes | str | None = None,
        headers: dict[str, str] | None = None,
        include_auth: bool = True,
    ) -> HttpResponse:
        return self.raw_request(
            "POST",
            path,
            headers=headers,
            body=body,
            json_body=json_body,
            include_auth=include_auth,
        )

    def patch(
        self,
        path: str,
        json_body: Any = None,
        body: bytes | str | None = None,
        headers: dict[str, str] | None = None,
        include_auth: bool = True,
    ) -> HttpResponse:
        return self.raw_request(
            "PATCH",
            path,
            headers=headers,
            body=body,
            json_body=json_body,
            include_auth=include_auth,
        )

    def delete(
        self,
        path: str,
        json_body: Any = None,
        headers: dict[str, str] | None = None,
        include_auth: bool = True,
    ) -> HttpResponse:
        return self.raw_request(
            "DELETE",
            path,
            headers=headers,
            json_body=json_body,
            include_auth=include_auth,
        )

    def rpc(
        self,
        method: str,
        params: dict[str, Any] | None = None,
        protocol_version: int = 3,
        caller_scope: str = "desktop",
        operation_id: str | None = None,
        headers: dict[str, str] | None = None,
    ) -> HttpResponse:
        envelope = {
            "method": method,
            "params": params if params is not None else {},
            "protocol_version": protocol_version,
            "caller_scope": caller_scope,
            "operation_id": operation_id or str(uuid.uuid4()),
        }
        return self.post("/api/v1/orca/rpc", json_body=envelope, headers=headers)

    def assert_ok(self, resp: HttpResponse) -> Any:
        """Fail-closed check for a 2xx REST or ok=true RPC HttpResponse."""
        resp.assert_status(200, 201, 202, 204)
        if not resp.body:
            return None
        payload = resp.json()
        if isinstance(payload, dict) and payload.get("ok") is False:
            raise AssertionError(
                f"Expected ok response, got ok=false (HTTP {resp.status}): {payload!r}"
            )
        return payload

    def assert_rpc_ok(
        self,
        method_or_resp: str | HttpResponse,
        params: dict[str, Any] | None = None,
        *,
        protocol_version: int = 3,
        caller_scope: str = "desktop",
        operation_id: str | None = None,
    ) -> Any:
        resp = (
            self.rpc(
                method_or_resp,
                params=params,
                protocol_version=protocol_version,
                caller_scope=caller_scope,
                operation_id=operation_id,
            )
            if isinstance(method_or_resp, str)
            else method_or_resp
        )
        resp.assert_status(200)
        payload = resp.json()
        if not isinstance(payload, dict) or payload.get("ok") is not True:
            raise AssertionError(
                f"RPC call failed (HTTP {resp.status}); expected ok=true, got: {payload!r}"
            )
        return payload.get("result")

    def assert_rpc_error(
        self,
        method_or_resp: str | HttpResponse,
        expected_code: str,
        params: dict[str, Any] | None = None,
        *,
        protocol_version: int = 3,
        caller_scope: str = "desktop",
        operation_id: str | None = None,
    ) -> dict[str, Any]:
        resp = (
            self.rpc(
                method_or_resp,
                params=params,
                protocol_version=protocol_version,
                caller_scope=caller_scope,
                operation_id=operation_id,
            )
            if isinstance(method_or_resp, str)
            else method_or_resp
        )
        if resp.status == 501:
            raise AssertionError(
                f"RPC endpoint returned HTTP 501 Not Implemented (fail-closed) while expecting "
                f"RPC error {expected_code!r}; body={resp.text[:1000]!r}"
            )
        payload = resp.json()
        if not isinstance(payload, dict):
            raise AssertionError(
                f"Expected RPC error object (HTTP {resp.status}), got: {payload!r}"
            )
        if payload.get("ok") is not False:
            raise AssertionError(
                f"Expected Unified RPC error envelope with ok=false and error.code={expected_code!r} "
                f"(HTTP {resp.status} {resp.reason}), got: {payload!r}"
            )
        err = payload.get("error")
        if not isinstance(err, dict):
            raise AssertionError(
                f"Expected Unified RPC error object 'error' dict for code {expected_code!r} "
                f"(HTTP {resp.status} {resp.reason}), got: {payload!r}"
            )
        actual_code = err.get("code")
        if actual_code != expected_code:
            raise AssertionError(
                f"Expected RPC error code {expected_code!r}, got {actual_code!r} "
                f"(HTTP {resp.status} {resp.reason}); payload={payload!r}"
            )
        return err

    def ws_connect(
        self,
        path: str,
        headers: dict[str, str] | None = None,
        timeout: float = 10.0,
        include_auth: bool = True,
    ) -> WsClient:
        hdrs = dict(headers or {})
        lower_keys = {k.lower() for k in hdrs}
        if include_auth and self.token and "authorization" not in lower_keys:
            hdrs["Authorization"] = f"Bearer {self.token}"
        ws_scheme = "wss" if self.raw.scheme == "https" else "ws"
        full_path = self._build_path(path)
        url = f"{ws_scheme}://{self.raw.host}:{self.raw.port}{full_path}"
        return WsClient.connect(url, headers=hdrs, timeout=timeout)


@dataclass(frozen=True)
class WsFrame:
    """Decoded RFC 6455 WebSocket frame with parsed Close status code & reason."""

    opcode: int
    payload: bytes
    close_code: int | None = None
    close_reason: str = ""
    fin: bool = True

    @property
    def text(self) -> str:
        return self.payload.decode("utf-8", errors="replace")

    def json(self) -> Any:
        return json.loads(self.payload.decode("utf-8"))


def _xor_mask(payload: bytes, mask: bytes) -> bytes:
    n = len(payload)
    if n == 0:
        return b""
    repeated = (mask * ((n // 4) + 1))[:n]
    return (int.from_bytes(payload, "big") ^ int.from_bytes(repeated, "big")).to_bytes(n, "big")


class WsClient:
    """Pure-stdlib RFC 6455 WebSocket client with Close-code extraction."""

    def __init__(
        self,
        url: str,
        headers: dict[str, str] | None = None,
        timeout: float = 10.0,
    ) -> None:
        parsed = urlparse(url)
        is_tls = parsed.scheme == "wss"
        host = parsed.hostname or "127.0.0.1"
        port = parsed.port or (443 if is_tls else 80)

        sock = socket.create_connection((host, port), timeout=timeout)
        if is_tls:
            try:
                sock = ssl.create_default_context().wrap_socket(sock, server_hostname=host)
            except BaseException:
                try:
                    sock.close()
                except Exception:
                    pass
                raise
        sock.settimeout(timeout)
        self._sock: socket.socket = sock
        self._buf = bytearray()
        self._closed = False

        target = parsed.path or "/"
        if parsed.query:
            target = f"{target}?{parsed.query}"

        try:
            self._handshake(host=host, port=port, target=target, headers=headers, timeout=timeout)
        except BaseException:
            try:
                self._sock.close()
            except Exception:
                pass
            raise

    def _handshake(
        self,
        *,
        host: str,
        port: int,
        target: str,
        headers: dict[str, str] | None,
        timeout: float,
    ) -> None:
        key = base64.b64encode(os.urandom(16)).decode("ascii")
        hdrs = dict(headers or {})
        lower_keys = {k.lower() for k in hdrs}
        req_lines = [f"GET {target} HTTP/1.1"]
        if "host" not in lower_keys:
            req_lines.append(f"Host: {host}:{port}")
        req_lines.extend(
            [
                "Upgrade: websocket",
                "Connection: Upgrade",
                f"Sec-WebSocket-Key: {key}",
                "Sec-WebSocket-Version: 13",
            ]
        )
        for k, v in hdrs.items():
            req_lines.append(f"{k}: {v}")
        req_bytes = ("\r\n".join(req_lines) + "\r\n\r\n").encode("utf-8")

        deadline = time.monotonic() + timeout
        self._sock.settimeout(timeout)
        self._sock.sendall(req_bytes)
        while b"\r\n\r\n" not in self._buf:
            remaining = deadline - time.monotonic()
            if remaining <= 0:
                raise TimeoutError("Timed out waiting for WebSocket handshake response")
            self._sock.settimeout(remaining)
            chunk = self._sock.recv(4096)
            if not chunk:
                raise ConnectionError("WebSocket closed before handshake completed")
            self._buf.extend(chunk)
        sep = self._buf.index(b"\r\n\r\n")
        raw_head = bytes(self._buf[:sep]).decode("utf-8", errors="replace")
        del self._buf[: sep + 4]

        lines = raw_head.split("\r\n")
        status_line = lines[0] if lines else ""
        parts = status_line.split(" ", 2)
        if len(parts) < 2 or parts[1] != "101":
            raise ConnectionError(f"WebSocket handshake rejected: {status_line!r}\n{raw_head}")

        resp_headers: dict[str, str] = {}
        for line in lines[1:]:
            if ":" in line:
                hk, hv = line.split(":", 1)
                resp_headers[hk.strip().lower()] = hv.strip()

        expected_accept = base64.b64encode(
            hashlib.sha1((key + WS_GUID).encode("ascii")).digest()
        ).decode("ascii")
        actual_accept = resp_headers.get("sec-websocket-accept", "")
        if actual_accept != expected_accept:
            raise ConnectionError(
                f"Invalid Sec-WebSocket-Accept: expected {expected_accept!r}, "
                f"got {actual_accept!r}"
            )

    @classmethod
    def connect(
        cls,
        url: str,
        headers: dict[str, str] | None = None,
        timeout: float = 10.0,
    ) -> WsClient:
        return cls(url=url, headers=headers, timeout=timeout)

    def __enter__(self) -> WsClient:
        return self

    def __exit__(self, exc_type: Any, exc: Any, tb: Any) -> None:
        self.close()

    def _read_exact(self, n: int, deadline: float | None = None) -> bytes:
        while len(self._buf) < n:
            if deadline is not None:
                remaining = deadline - time.monotonic()
                if remaining <= 0:
                    raise TimeoutError("Timed out reading WebSocket frame")
                self._sock.settimeout(remaining)
            try:
                chunk = self._sock.recv(max(4096, n - len(self._buf)))
            except socket.timeout as exc:
                raise TimeoutError("Timed out reading WebSocket frame") from exc
            if not chunk:
                raise EOFError("WebSocket stream closed unexpectedly")
            self._buf.extend(chunk)
        out = bytes(self._buf[:n])
        del self._buf[:n]
        return out

    def _send_frame(self, opcode: int, payload: bytes, fin: bool = True) -> None:
        first_byte = (0x80 if fin else 0x00) | (opcode & 0x0F)
        header = bytearray([first_byte])
        length = len(payload)
        if length < 126:
            header.append(0x80 | length)
        elif length < 65536:
            header.append(0x80 | 126)
            header.extend(struct.pack("!H", length))
        else:
            header.append(0x80 | 127)
            header.extend(struct.pack("!Q", length))
        mask = os.urandom(4)
        header.extend(mask)
        masked = _xor_mask(payload, mask)
        self._sock.sendall(bytes(header) + masked)

    def send_text(self, text: str) -> None:
        self._send_frame(0x1, text.encode("utf-8"))

    def send_json(self, obj: Any) -> None:
        self.send_text(json.dumps(obj))

    def send_binary(self, data: bytes) -> None:
        self._send_frame(0x2, data)

    def send_ping(self, payload: bytes = b"") -> None:
        self._send_frame(0x9, payload)

    def send_raw_oversize(
        self, byte_count: int, opcode: int = 0x2, fill_byte: int = 0x41
    ) -> None:
        payload = bytes([fill_byte]) * byte_count
        try:
            self._send_frame(opcode, payload)
        except (BrokenPipeError, ConnectionResetError):
            # Server may immediately send Close(1009) and reset TCP before reading full body.
            pass

    def _read_raw_frame(self, deadline: float | None = None) -> tuple[bool, int, bytes]:
        b1, b2 = self._read_exact(2, deadline=deadline)
        fin = (b1 & 0x80) != 0
        opcode = b1 & 0x0F
        masked = (b2 & 0x80) != 0
        length = b2 & 0x7F
        if length == 126:
            (length,) = struct.unpack("!H", self._read_exact(2, deadline=deadline))
        elif length == 127:
            (length,) = struct.unpack("!Q", self._read_exact(8, deadline=deadline))
        mask = self._read_exact(4, deadline=deadline) if masked else b""
        data = self._read_exact(length, deadline=deadline)
        if masked:
            data = _xor_mask(data, mask)
        return fin, opcode, data

    def recv(self, timeout: float = 5.0) -> WsFrame:
        deadline = time.monotonic() + timeout
        frag_opcode: int | None = None
        frag_chunks: list[bytes] = []

        while True:
            remaining = deadline - time.monotonic()
            if remaining <= 0:
                raise TimeoutError(
                    f"Timed out waiting for WebSocket frame after {timeout:.2f}s"
                )
            self._sock.settimeout(remaining)
            fin, opcode, data = self._read_raw_frame(deadline=deadline)
            if opcode == 0x9:  # Ping -> auto-Pong
                try:
                    rem_send = deadline - time.monotonic()
                    if rem_send > 0:
                        self._sock.settimeout(rem_send)
                        self._send_frame(0xA, data)
                except OSError:
                    pass
                continue
            if opcode == 0xA:  # Unsolicited Pong
                continue
            if opcode == 0x8:  # Close
                close_code: int | None = None
                close_reason = ""
                if len(data) >= 2:
                    (close_code,) = struct.unpack("!H", data[:2])
                    close_reason = data[2:].decode("utf-8", errors="replace")
                return WsFrame(
                    opcode=0x8,
                    payload=data,
                    close_code=close_code,
                    close_reason=close_reason,
                    fin=fin,
                )

            if not fin:
                if frag_opcode is None:
                    frag_opcode = opcode
                frag_chunks.append(data)
                continue

            if frag_opcode is not None:
                frag_chunks.append(data)
                return WsFrame(
                    opcode=frag_opcode,
                    payload=b"".join(frag_chunks),
                    fin=True,
                )
            return WsFrame(opcode=opcode, payload=data, fin=True)

    def recv_json(self, timeout: float = 5.0) -> Any:
        frame = self.recv(timeout=timeout)
        if frame.opcode == 0x8:
            raise EOFError(
                f"WebSocket closed with code={frame.close_code} reason={frame.close_reason!r}"
            )
        return frame.json()

    def recv_until_close(self, timeout: float = 5.0) -> WsFrame:
        deadline = time.monotonic() + timeout
        seen: list[WsFrame] = []
        while True:
            remaining = deadline - time.monotonic()
            if remaining <= 0:
                raise TimeoutError(
                    f"Timed out waiting for WebSocket Close frame after {timeout:.1f}s; "
                    f"seen={[(f.opcode, len(f.payload)) for f in seen]}"
                )
            try:
                frame = self.recv(timeout=remaining)
            except TimeoutError as exc:
                raise TimeoutError(
                    f"Timed out waiting for WebSocket Close frame after {timeout:.1f}s; "
                    f"seen={[(f.opcode, len(f.payload)) for f in seen]}"
                ) from exc
            if frame.opcode == 0x8:
                return frame
            seen.append(frame)

    def close(self, code: int = 1000, reason: str = "") -> None:
        if self._closed:
            return
        self._closed = True
        try:
            try:
                payload = struct.pack("!H", code) + reason.encode("utf-8")
                self._send_frame(0x8, payload)
            except Exception:
                pass
        finally:
            try:
                self._sock.close()
            except Exception:
                pass
