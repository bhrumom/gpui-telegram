"""Opaque-box E2E harness for Code Scene ("代码场景").

Pure Python 3 standard library only. NEVER imports _contract_double.py.
Supports:
- Managed live server mode (spawns real gpui_telegram_server binary; fails fast
  with exit code 2 if absent or unhealthy — ZERO silent fallback).
- External live server mode (--base-url, optional --restart-cmd or
  --allow-reconnect-only).
- Injected handle mode (used exclusively by run_all_e2e.py --self-test).
"""

from __future__ import annotations

import gzip
import http.client
import json
import math
import os
import re
import secrets
import signal
import socket
import subprocess
import tempfile
import threading
import time
import urllib.parse
from dataclasses import dataclass
from pathlib import Path
from typing import Any, Callable, Dict, List, Optional, Sequence, Tuple

from ._scripts import Cs1CorpusBuilder

VALID_ASSET_KINDS = frozenset({"video", "animation", "game"})
VALID_FEED_TABS = frozenset({"for_you", "following", "videos", "animations", "games", "my_works"})
ALL_BEHAVIOR_EVENT_KINDS = frozenset({
    "impression", "dwell", "complete", "replay", "game_interaction",
    "like", "double_tap_like", "unlike", "bookmark", "unbookmark",
    "comment", "share", "follow_author", "unfollow_author",
    "inspect_code", "remix", "fast_skip", "not_interested",
    "hide_author", "hide_tag",
})
SCORE_BREAKDOWN_FIELDS = (
    "embedding_sim", "swing_cf", "tag_kind_affinity", "creator_affinity",
    "quality_ecpm", "freshness", "exploration_bonus", "mmr_penalty",
    "negative_penalty", "final_score",
)


class ServerConfigError(RuntimeError):
    """Raised when server binary is absent, flags invalid, or /health unreachable (exit code 2)."""


@dataclass
class HttpResponse:
    status: int
    headers: Dict[str, str]
    raw: bytes
    body: Any
    elapsed_ms: float


class HttpClient:
    """Thread-safe pure-stdlib HTTP/JSON client opening a fresh connection per call."""

    def __init__(self, base_url: str, timeout: float = 15.0) -> None:
        self.base_url = base_url.rstrip("/")
        self.timeout = timeout
        parsed = urllib.parse.urlparse(self.base_url)
        if parsed.scheme not in ("http", "https") or not parsed.hostname:
            raise ServerConfigError(f"Invalid base URL: {base_url!r}")
        self._scheme = parsed.scheme
        self._host = parsed.hostname
        self._port = parsed.port or (443 if parsed.scheme == "https" else 80)
        self._base_path = parsed.path.rstrip("/")

    def request(
        self,
        method: str,
        path: str,
        raw_body: Optional[bytes] = None,
        headers: Optional[Dict[str, str]] = None,
        token: Optional[str] = None,
    ) -> HttpResponse:
        req_headers = {"Accept": "application/json", "Accept-Encoding": "identity"}
        if token is not None:
            req_headers["Authorization"] = f"Bearer {token}"
        if headers:
            req_headers.update(headers)
        full_path = self._base_path + (path if path.startswith("/") else "/" + path)
        t0 = time.perf_counter()
        conn_cls = http.client.HTTPSConnection if self._scheme == "https" else http.client.HTTPConnection
        conn = conn_cls(self._host, self._port, timeout=self.timeout)
        try:
            conn.request(method.upper(), full_path, body=raw_body, headers=req_headers)
            resp = conn.getresponse()
            data = resp.read()
            hdrs = {k.lower(): v for k, v in resp.getheaders()}
            status = resp.status
        finally:
            conn.close()
        elapsed_ms = (time.perf_counter() - t0) * 1000.0
        if hdrs.get("content-encoding", "").lower() == "gzip" and data:
            try:
                data = gzip.decompress(data)
            except OSError:
                pass
        parsed: Any = None
        if data:
            try:
                parsed = json.loads(data.decode("utf-8", errors="replace"))
            except ValueError:
                parsed = data.decode("utf-8", errors="replace")
        return HttpResponse(status=status, headers=hdrs, raw=data, body=parsed, elapsed_ms=elapsed_ms)

    def json(
        self,
        method: str,
        path: str,
        payload: Any = None,
        token: Optional[str] = None,
        headers: Optional[Dict[str, str]] = None,
    ) -> HttpResponse:
        hdrs = dict(headers or {})
        raw: Optional[bytes] = None
        if payload is not None:
            hdrs.setdefault("Content-Type", "application/json")
            raw = json.dumps(payload, ensure_ascii=False).encode("utf-8")
        return self.request(method, path, raw_body=raw, headers=hdrs, token=token)


def pick_free_port() -> int:
    with socket.socket(socket.AF_INET, socket.SOCK_STREAM) as s:
        s.bind(("127.0.0.1", 0))
        return int(s.getsockname()[1])


def wait_for_health(
    base_url: str,
    timeout_sec: float = 20.0,
    proc: Optional[subprocess.Popen[Any]] = None,
    log_path: Optional[Path] = None,
) -> None:
    deadline = time.monotonic() + timeout_sec
    poll_timeout = min(3.0, max(0.5, timeout_sec))
    client = HttpClient(base_url, timeout=poll_timeout)
    last_err = "no response"
    while time.monotonic() < deadline:
        if proc is not None and proc.poll() is not None:
            tail = ""
            if log_path and log_path.exists():
                tail = log_path.read_text(encoding="utf-8", errors="replace")[-1200:]
            raise ServerConfigError(f"Server exited prematurely (rc={proc.returncode}). Log tail:\n{tail}")
        try:
            r1 = client.request("GET", "/health")
            if r1.status == 200 and str(r1.body).strip() == "OK":
                r2 = client.json("GET", "/api/v1/status")
                if r2.status == 200 and isinstance(r2.body, dict) and r2.body.get("status") == "healthy":
                    return
                last_err = f"/api/v1/status -> {r2.status} {r2.body!r}"
            else:
                last_err = f"/health -> {r1.status} {r1.body!r}"
        except (OSError, http.client.HTTPException) as exc:
            last_err = str(exc)
        rem = deadline - time.monotonic()
        if rem > 0:
            time.sleep(min(0.15, max(0.02, rem)))
    tail = ""
    if log_path and log_path.exists():
        tail = "\nLog tail:\n" + log_path.read_text(encoding="utf-8", errors="replace")[-1200:]
    raise ServerConfigError(f"Server at {base_url} failed readiness within {timeout_sec}s ({last_err}){tail}")


class ServerProcessManager:
    """Manages a real gpui_telegram_server subprocess in a clean TemporaryDirectory."""

    def __init__(self, server_bin: Path, startup_timeout: float = 25.0) -> None:
        self.server_bin = Path(server_bin)
        self.startup_timeout = startup_timeout
        self._tmpdir: Optional[tempfile.TemporaryDirectory[str]] = None
        self._proc: Optional[subprocess.Popen[Any]] = None
        self._log_file: Optional[Any] = None
        self._log_path: Optional[Path] = None
        self._db_path: Optional[Path] = None
        self._media_dir: Optional[Path] = None
        self.base_url: str = ""
        self.mode: str = "spawned_server"
        self.physical_restart_verified: bool = False

    def start(self) -> str:
        if not self.server_bin.is_file() or not os.access(self.server_bin, os.X_OK):
            raise ServerConfigError(
                f"Real gpui_telegram_server binary not found or not executable: {self.server_bin}. "
                f"Build it or pass --server-bin / --base-url / --self-test."
            )
        self._tmpdir = tempfile.TemporaryDirectory(prefix="gpui_cs_e2e_")
        tmp_path = Path(self._tmpdir.name)
        self._db_path = tmp_path / "code_scene.db"
        self._media_dir = tmp_path / "media_cas"
        self._media_dir.mkdir(parents=True, exist_ok=True)
        self._log_path = tmp_path / "server.log"
        self._spawn()
        return self.base_url

    def _spawn(self) -> None:
        assert self._db_path and self._media_dir and self._log_path
        port = pick_free_port()
        self.base_url = f"http://127.0.0.1:{port}"
        env = os.environ.copy()
        env.pop("HERMES_ENABLE_PUBLIC_CLOUD_FALLBACK", None)
        env.update({
            "HOST": "127.0.0.1",
            "PORT": str(port),
            "DATABASE_URL": f"sqlite://{self._db_path}",
            "MEDIA_STORAGE_PATH": str(self._media_dir),
            "GPUI_OFFLINE_TEST": "1",
            "RUST_LOG": "warn",
        })
        self._log_file = open(self._log_path, "ab")
        self._proc = subprocess.Popen(
            [str(self.server_bin)],
            env=env,
            stdout=self._log_file,
            stderr=subprocess.STDOUT,
            start_new_session=True,
        )
        wait_for_health(self.base_url, self.startup_timeout, self._proc, self._log_path)

    def _terminate_proc(self) -> None:
        if self._proc is not None:
            proc = self._proc
            pid = proc.pid
            can_pg = hasattr(os, "killpg") and hasattr(os, "getpgrp")
            pgid: Optional[int] = None
            if can_pg:
                try:
                    pgid = os.getpgid(pid)
                except OSError:
                    pgid = pid
                if pgid == os.getpgrp():
                    pgid = None
            if proc.poll() is None:
                try:
                    if pgid is not None:
                        try:
                            os.killpg(pgid, signal.SIGTERM)
                        except (ProcessLookupError, PermissionError):
                            proc.terminate()
                    else:
                        proc.terminate()
                    proc.wait(timeout=10.0)
                except subprocess.TimeoutExpired:
                    if pgid is not None:
                        try:
                            os.killpg(pgid, signal.SIGKILL)
                        except (ProcessLookupError, PermissionError):
                            proc.kill()
                    else:
                        proc.kill()
                    proc.wait(timeout=5.0)
            # Reap any lingering child subprocesses in the spawned process group
            if pgid is not None:
                try:
                    os.killpg(pgid, signal.SIGKILL)
                except (ProcessLookupError, PermissionError):
                    pass
            self._proc = None
        if self._log_file is not None:
            self._log_file.close()
            self._log_file = None

    def restart(self) -> str:
        self._terminate_proc()
        time.sleep(0.15)
        self._spawn()
        self.physical_restart_verified = True
        return self.base_url

    def stop(self) -> None:
        try:
            self._terminate_proc()
        finally:
            if self._tmpdir is not None:
                self._tmpdir.cleanup()
                self._tmpdir = None


class ExternalServerHandle:
    """Connects strictly to an already-running server at --base-url."""

    def __init__(
        self,
        base_url: str,
        restart_cmd: Optional[str] = None,
        allow_reconnect_only: bool = False,
        timeout: float = 15.0,
    ) -> None:
        self.base_url = base_url.rstrip("/")
        self.restart_cmd = restart_cmd
        self.allow_reconnect_only = allow_reconnect_only
        self.timeout = timeout
        self.mode = "external_base_url"
        self.physical_restart_verified = False

    def start(self) -> str:
        wait_for_health(self.base_url, timeout_sec=min(self.timeout, 10.0))
        return self.base_url

    def restart(self) -> str:
        if self.restart_cmd:
            res = subprocess.run(self.restart_cmd, shell=True, capture_output=True, text=True, timeout=60)
            if res.returncode != 0:
                raise AssertionError(f"--restart-cmd failed (rc={res.returncode}): {res.stderr.strip()}")
            wait_for_health(self.base_url, timeout_sec=25.0)
            self.physical_restart_verified = True
            return self.base_url
        if self.allow_reconnect_only:
            wait_for_health(self.base_url, timeout_sec=10.0)
            self.physical_restart_verified = False
            return self.base_url
        raise AssertionError(
            "Physical server restart required in --base-url mode; supply --restart-cmd "
            "or explicitly opt in to --allow-reconnect-only."
        )

    def stop(self) -> None:
        pass


# ---- Strict Contract Validators & Defensive Envelope Normalizers ----

def has_cjk(text: str) -> bool:
    return bool(re.search(r"[\u4e00-\u9fff]", text or ""))


def unwrap_asset(body: Any) -> Optional[Dict[str, Any]]:
    if not isinstance(body, dict):
        return None
    if isinstance(body.get("asset"), dict):
        return body["asset"]
    pub = body.get("published")
    if isinstance(pub, dict):
        if isinstance(pub.get("asset"), dict):
            return pub["asset"]
        if "scene_id" in pub and "manifest" in pub:
            return pub
    if "scene_id" in body and "manifest" in body:
        return body
    return None


def unwrap_asset_list(body: Any) -> List[Dict[str, Any]]:
    if isinstance(body, list):
        return [x for x in body if isinstance(x, dict)]
    if isinstance(body, dict):
        for key in ("assets", "items"):
            val = body.get(key)
            if isinstance(val, list):
                out = []
                for elem in val:
                    if isinstance(elem, dict) and isinstance(elem.get("asset"), dict):
                        out.append(elem["asset"])
                    elif isinstance(elem, dict):
                        out.append(elem)
                return out
    return []


def unwrap_comments(body: Any) -> List[Dict[str, Any]]:
    if isinstance(body, list):
        return [x for x in body if isinstance(x, dict)]
    if isinstance(body, dict):
        for key in ("comments", "items"):
            if isinstance(body.get(key), list):
                return [x for x in body[key] if isinstance(x, dict)]
    return []


def extract_comment_text(comment: Any) -> str:
    if not isinstance(comment, dict):
        return ""
    for key in ("content", "text", "body"):
        val = comment.get(key)
        if isinstance(val, str):
            return val
    return ""


def assert_all_floats_finite(obj: Any, path: str = "root") -> None:
    if isinstance(obj, float):
        assert math.isfinite(obj), f"Non-finite float leaked at {path}={obj!r}"
    elif isinstance(obj, dict):
        for k, v in obj.items():
            assert_all_floats_finite(v, f"{path}.{k}")
    elif isinstance(obj, list):
        for idx, v in enumerate(obj):
            assert_all_floats_finite(v, f"{path}[{idx}]")


def unwrap_draft(body: Any) -> Dict[str, Any]:
    if not isinstance(body, dict):
        return {}
    merged = dict(body)
    for key in ("draft", "asset"):
        sub = body.get(key)
        if isinstance(sub, dict):
            for k, v in sub.items():
                merged.setdefault(k, v)
    pub = body.get("published")
    if isinstance(pub, dict):
        sub = pub.get("asset") if isinstance(pub.get("asset"), dict) else pub
        if isinstance(sub, dict):
            for k, v in sub.items():
                merged.setdefault(k, v)
    return merged


def extract_follow_flag(body: Any) -> bool:
    assert isinstance(body, dict), f"Follow response must be JSON object, got {body!r}"
    if "following" in body and isinstance(body["following"], bool):
        return body["following"]
    if "followed" in body and isinstance(body["followed"], bool):
        return body["followed"]
    raise AssertionError(f"Missing boolean 'following'/'followed' in {body!r}")


def extract_card_action_payloads(msg: Dict[str, Any]) -> List[str]:
    payloads: List[str] = []
    content = msg.get("content")
    if isinstance(content, dict) and content.get("type") == "GenerativeCard":
        data = content.get("data") or {}
        for comp in data.get("components") or []:
            if isinstance(comp, dict) and comp.get("type") == "Button":
                ap = comp.get("action_payload")
                if isinstance(ap, str):
                    payloads.append(ap)
    for kb_key in ("inline_keyboard", "bot_keyboard"):
        kb = msg.get(kb_key)
        if isinstance(kb, list):
            for row in kb:
                if isinstance(row, list):
                    for btn in row:
                        if isinstance(btn, dict):
                            cb = btn.get("callback_data") or btn.get("action_payload")
                            if isinstance(cb, str):
                                payloads.append(cb)
    return payloads


def assert_valid_manifest(m: Any, expected_kind: Optional[str] = None) -> None:
    assert isinstance(m, dict), f"manifest must be dict, got {m!r}"
    kind = m.get("kind")
    assert kind in VALID_ASSET_KINDS, f"Invalid manifest.kind={kind!r}"
    if expected_kind is not None:
        assert kind == expected_kind, f"manifest.kind={kind!r} != expected {expected_kind!r}"
    w, h, fps = m.get("width"), m.get("height"), m.get("fps")
    assert isinstance(w, int) and not isinstance(w, bool) and 160 <= w <= 720, f"width out of [160,720]: {w!r}"
    assert isinstance(h, int) and not isinstance(h, bool) and 160 <= h <= 1280, f"height out of [160,1280]: {h!r}"
    assert isinstance(fps, int) and not isinstance(fps, bool) and 15 <= fps <= 60, f"fps out of [15,60]: {fps!r}"
    seed = m.get("seed")
    assert isinstance(seed, int) and not isinstance(seed, bool) and seed >= 0, f"Invalid seed={seed!r}"
    dur = m.get("duration_sec")
    if kind == "video":
        assert isinstance(dur, (int, float)) and not isinstance(dur, bool) and math.isfinite(dur) and dur > 0.0, (
            f"Video manifest must have finite duration_sec > 0, got {dur!r}"
        )
    elif dur is not None:
        assert isinstance(dur, (int, float)) and math.isfinite(dur) and dur >= 0.0, f"Bad duration_sec={dur!r}"


def assert_valid_asset(a: Any) -> None:
    assert isinstance(a, dict), f"CodeSceneAssetDto must be dict, got {a!r}"
    assert isinstance(a.get("scene_id"), str) and a["scene_id"].strip(), f"Missing scene_id: {a!r}"
    assert isinstance(a.get("title"), str) and a["title"].strip(), f"Missing title: {a!r}"
    assert isinstance(a.get("description"), str), f"Missing description str: {a!r}"
    kind = a.get("kind")
    assert kind in VALID_ASSET_KINDS, f"Invalid asset.kind={kind!r}"
    tags = a.get("tags")
    assert isinstance(tags, list) and all(isinstance(t, str) for t in tags), f"Invalid tags={tags!r}"
    assert isinstance(a.get("author_user_id"), int) and not isinstance(a["author_user_id"], bool)
    assert isinstance(a.get("author_username"), str) and a["author_username"].strip()
    assert isinstance(a.get("source_code"), str) and len(a["source_code"].strip()) > 0
    assert len(a["source_code"].encode("utf-8")) <= 65_536, "Persisted source_code exceeds 65_536B"
    assert a.get("code_format") == "cs1", f"Expected code_format=='cs1', got {a.get('code_format')!r}"
    assert_valid_manifest(a.get("manifest"), expected_kind=kind)
    assert isinstance(a.get("controls_hint"), str), f"Missing controls_hint: {a!r}"
    for ctr in ("plays_count", "likes_count", "bookmarks_count", "comments_count", "shares_count", "remixes_count"):
        v = a.get(ctr)
        assert isinstance(v, int) and not isinstance(v, bool) and v >= 0, f"Invalid {ctr}={v!r}"
    for flg in ("viewer_liked", "viewer_bookmarked", "viewer_following_author"):
        assert isinstance(a.get(flg), bool), f"Expected bool {flg}, got {a.get(flg)!r}"
    ca, ua = a.get("created_at_ms"), a.get("updated_at_ms")
    assert isinstance(ca, int) and ca > 0, f"Invalid created_at_ms={ca!r}"
    assert isinstance(ua, int) and ua >= ca, f"updated_at_ms ({ua}) < created_at_ms ({ca})"
    for opt_int in ("creator_bot_id",):
        if a.get(opt_int) is not None:
            assert isinstance(a[opt_int], int) and not isinstance(a[opt_int], bool)
    for opt_str in ("creator_bot_username", "remix_parent_id"):
        if a.get(opt_str) is not None:
            assert isinstance(a[opt_str], str)


def assert_valid_score_breakdown(bd: Any) -> None:
    assert isinstance(bd, dict), f"ScoreBreakdown must be dict, got {bd!r}"
    for k in SCORE_BREAKDOWN_FIELDS:
        assert k in bd, f"Missing ScoreBreakdown.{k}"
        v = bd[k]
        assert isinstance(v, (int, float)) and not isinstance(v, bool) and math.isfinite(float(v)), (
            f"Non-finite ScoreBreakdown.{k}={v!r}"
        )


def assert_valid_explanation(exp: Any, require_cjk_zh: bool = False) -> None:
    assert isinstance(exp, dict), f"RecommendationExplanation must be dict, got {exp!r}"
    ch = exp.get("recall_channels")
    assert isinstance(ch, list) and len(ch) >= 1 and all(isinstance(c, str) and c.strip() for c in ch), (
        f"recall_channels must be non-empty list[str], got {ch!r}"
    )
    zh = exp.get("primary_reason_zh")
    en = exp.get("primary_reason_en")
    assert isinstance(zh, str) and zh.strip(), f"Empty primary_reason_zh: {exp!r}"
    assert isinstance(en, str) and en.strip(), f"Empty primary_reason_en: {exp!r}"
    if require_cjk_zh:
        assert has_cjk(zh), f"primary_reason_zh lacks CJK characters: {zh!r}"
        assert re.search(r"[A-Za-z]", en), f"primary_reason_en lacks English characters: {en!r}"
    tt = exp.get("traffic_tier")
    assert isinstance(tt, str) and tt.strip(), f"Empty traffic_tier: {exp!r}"
    assert_valid_score_breakdown(exp.get("breakdown"))


def assert_valid_ranked_item(item: Any, require_cjk_zh: bool = False) -> None:
    assert isinstance(item, dict), f"RankedSceneItemDto must be dict, got {item!r}"
    assert_valid_asset(item.get("asset"))
    assert_valid_explanation(item.get("explanation"), require_cjk_zh=require_cjk_zh)


def assert_valid_feed_response(body: Any, expected_limit: Optional[int] = None) -> List[Dict[str, Any]]:
    assert isinstance(body, dict), f"CodeSceneFeedResponse must be dict, got {body!r}"
    items = body.get("items")
    assert isinstance(items, list), f"feed.items must be list, got {items!r}"
    if expected_limit is not None:
        assert len(items) <= expected_limit, f"len(items)={len(items)} > limit={expected_limit}"
    nc = body.get("next_cursor")
    assert nc is None or (isinstance(nc, str) and len(nc) > 0), f"Invalid next_cursor={nc!r}"
    tc = body.get("total_catalog")
    assert isinstance(tc, int) and not isinstance(tc, bool) and tc >= 0, f"Invalid total_catalog={tc!r}"
    seen: set[str] = set()
    for it in items:
        assert_valid_ranked_item(it)
        sid = it["asset"]["scene_id"]
        assert sid not in seen, f"Duplicate scene_id {sid!r} within a single feed page"
        seen.add(sid)
    return items


def assert_valid_generative_card(msg: Dict[str, Any], scene_id: str) -> List[str]:
    content = msg.get("content")
    assert isinstance(content, dict) and content.get("type") == "GenerativeCard", (
        f"Expected MessageContent::GenerativeCard, got {content!r}"
    )
    data = content.get("data")
    assert isinstance(data, dict), f"GenerativeCard.data missing: {content!r}"
    assert isinstance(data.get("card_id"), str) and data["card_id"].strip(), "Empty card_id"
    assert isinstance(data.get("title"), str) and data["title"].strip(), "Empty card title"
    payloads = extract_card_action_payloads(msg)
    for prefix in (f"code_scene:open:{scene_id}", f"code_scene:remix:{scene_id}", f"code_scene:share:{scene_id}"):
        assert prefix in payloads, f"Missing required card action payload {prefix!r}; got {payloads!r}"
    return payloads


@dataclass
class TestUser:
    user_id: int
    username: str
    password: str
    token: str
    session_id: str = ""


class CodeSceneEnv:
    """Thread-safe high-level E2E client & fixture coordinator."""

    def __init__(self, server_handle: Any, timeout: float = 15.0) -> None:
        self.server_handle = server_handle
        self.base_url: str = server_handle.base_url
        self.mode: str = getattr(server_handle, "mode", "unknown")
        self.timeout = timeout
        self._lock = threading.Lock()
        self._seq = 0
        self.corpus = Cs1CorpusBuilder()
        self._corpus_ready = False
        self._telemetry_kind_key: str = "kind"
        self._telemetry_key_locked: bool = False
        self._comment_text_key: str = "content"
        self._comment_key_locked: bool = False
        self._remix_id_key: str = "parent_scene_id"
        self._remix_key_locked: bool = False
        self._iterate_prompt_key: str = "instruction"
        self._iterate_key_locked: bool = False

    def client(self) -> HttpClient:
        return HttpClient(self.base_url, timeout=self.timeout)

    def ensure_corpus(self, user: TestUser) -> Cs1CorpusBuilder:
        with self._lock:
            if not self._corpus_ready:
                self.corpus.harvest(self, user)
                self._corpus_ready = True
        return self.corpus

    def unique_name(self, prefix: str = "cs") -> str:
        with self._lock:
            self._seq += 1
            n = self._seq
        return f"{prefix}_{int(time.time() * 1000) % 1000000}_{n}_{secrets.token_hex(3)}"

    def register_user(self, prefix: str = "cs") -> TestUser:
        uname = self.unique_name(prefix)
        pwd = "StrongE2ePass!9"
        r = self.client().json("POST", "/api/v1/auth/register", {
            "username": uname, "password": pwd, "first_name": f"E2E_{prefix}", "auto_welcome": False,
        })
        assert r.status in (200, 201) and isinstance(r.body, dict), f"register failed: {r.status} {r.body!r}"
        tok = str(r.body["token"])
        sid = str(r.body.get("session_id") or "")
        if not sid and "." in tok:
            sid = tok.split(".", 1)[0]
        return TestUser(
            user_id=int(r.body["user_id"]),
            username=str(r.body.get("username", uname)),
            password=pwd,
            token=tok,
            session_id=sid,
        )

    def login_user(self, username: str, password: str) -> TestUser:
        r = self.client().json("POST", "/api/v1/auth/login", {"username": username, "password": password})
        assert r.status == 200 and isinstance(r.body, dict), f"login failed: {r.status} {r.body!r}"
        tok = str(r.body["token"])
        sid = str(r.body.get("session_id") or "")
        if not sid and "." in tok:
            sid = tok.split(".", 1)[0]
        return TestUser(
            user_id=int(r.body["user_id"]),
            username=str(r.body.get("username", username)),
            password=password,
            token=tok,
            session_id=sid,
        )

    def revoke_session(self, user: TestUser, session_id: Optional[str] = None) -> HttpResponse:
        sid = session_id if session_id is not None else user.session_id
        return self.client().json(
            "DELETE",
            f"/api/v1/auth/sessions/{urllib.parse.quote(sid, safe='')}",
            token=user.token,
        )

    def create_chat(
        self, user: TestUser, title: Optional[str] = None,
        chat_type: str = "group", member_ids: Optional[List[int]] = None,
        peer_user_id: Optional[int] = None,
    ) -> int:
        payload: Dict[str, Any] = {"chat_type": chat_type}
        if title is not None:
            payload["title"] = title
        if member_ids is not None:
            payload["member_ids"] = member_ids
        if peer_user_id is not None:
            payload["peer_user_id"] = peer_user_id
        r = self.client().json("POST", "/api/v1/chats", payload, token=user.token)
        assert r.status in (200, 201) and isinstance(r.body, dict), f"create_chat failed: {r.status} {r.body!r}"
        cid = r.body.get("chat_id") or r.body.get("id")
        assert isinstance(cid, int), f"Missing chat_id in {r.body!r}"
        return cid

    def list_chat_messages(self, user: TestUser, chat_id: int, limit: int = 50) -> List[Dict[str, Any]]:
        r = self.client().json("GET", f"/api/v1/chats/{chat_id}/messages?limit={limit}", token=user.token)
        assert r.status == 200 and isinstance(r.body, dict), f"list_chat_messages failed: {r.status} {r.body!r}"
        return list(r.body.get("messages") or [])

    def feed(
        self, user: TestUser, tab: str = "for_you", session_id: Optional[str] = None,
        tag_filter: Optional[str] = None, query: Optional[str] = None,
        cursor: Optional[str] = None, limit: int = 10, user_id_override: Optional[int] = None,
    ) -> HttpResponse:
        payload: Dict[str, Any] = {
            "user_id": user.user_id if user_id_override is None else user_id_override,
            "session_id": session_id if session_id is not None else f"sess_{user.username}",
            "tab": tab, "tag_filter": tag_filter, "query": query, "cursor": cursor, "limit": limit,
        }
        return self.client().json("POST", "/api/v1/code-scene/feed", payload, token=user.token)

    def feed_all_pages(
        self,
        user: TestUser,
        tab: str = "for_you",
        session_id: Optional[str] = None,
        tag_filter: Optional[str] = None,
        query: Optional[str] = None,
        limit: int = 20,
        max_pages: Optional[int] = None,
    ) -> Tuple[List[Dict[str, Any]], int]:
        """Walk all cursor pages safely without MAX_FEED_LIMIT truncation."""
        all_items: List[Dict[str, Any]] = []
        seen: set[str] = set()
        cur: Optional[str] = None
        pages = 0
        bound = max_pages if max_pages is not None else 60
        sess = session_id if session_id is not None else f"sess_walk_{user.username}_{secrets.token_hex(3)}"
        while pages < bound:
            r = self.feed(
                user, tab=tab, session_id=sess, tag_filter=tag_filter,
                query=query, cursor=cur, limit=limit,
            )
            assert r.status == 200, f"feed_all_pages failed on page {pages}: {r.status} {r.body!r}"
            items = assert_valid_feed_response(r.body, expected_limit=limit)
            if pages == 0 and max_pages is None:
                tc = int(r.body.get("total_catalog") or 0)
                bound = max(30, (tc // max(1, limit)) + 15)
            for it in items:
                sid = it["asset"]["scene_id"]
                assert sid not in seen, f"Duplicate scene_id {sid!r} across feed_all_pages (page={pages})"
                seen.add(sid)
                all_items.append(it)
            pages += 1
            cur = r.body.get("next_cursor")
            if cur is None:
                break
        assert cur is None, f"feed_all_pages did not exhaust cursor within {bound} pages (seen={len(all_items)})"
        return all_items, pages

    def list_assets(self, user: TestUser, params: Optional[Dict[str, Any]] = None) -> Tuple[HttpResponse, List[Dict[str, Any]]]:
        qs = ""
        if params:
            qs = "?" + urllib.parse.urlencode({k: v for k, v in params.items() if v is not None})
        r = self.client().json("GET", f"/api/v1/code-scene/assets{qs}", token=user.token)
        return r, unwrap_asset_list(r.body) if r.status == 200 else []

    def get_asset(self, user: TestUser, scene_id: str) -> Tuple[HttpResponse, Optional[Dict[str, Any]]]:
        r = self.client().json("GET", f"/api/v1/code-scene/assets/{urllib.parse.quote(scene_id, safe='')}", token=user.token)
        return r, unwrap_asset(r.body) if r.status == 200 else None

    def validate_script(self, user: TestUser, source_code: str, manifest: Dict[str, Any], code_format: str = "cs1") -> HttpResponse:
        return self.client().json("POST", "/api/v1/code-scene/assets/validate", {
            "source_code": source_code, "code_format": code_format, "manifest": manifest,
        }, token=user.token)

    def extract_validation(self, r: HttpResponse) -> Tuple[bool, str, List[str]]:
        if not isinstance(r.body, dict):
            return (False, "", [str(r.body)])
        ok = bool(r.body.get("valid") if "valid" in r.body else r.body.get("ok", False))
        ch = str(r.body.get("cover_hash") or "")
        errs: List[str] = []
        for k in ("errors", "diagnostics"):
            v = r.body.get(k)
            if isinstance(v, list):
                errs.extend(str(x) for x in v)
        if not errs and r.body.get("diagnostic"):
            errs.append(str(r.body["diagnostic"]))
        if not errs and r.body.get("message") and not ok:
            errs.append(str(r.body["message"]))
        return (r.status == 200 and ok, ch, errs)

    def publish_asset(
        self, user: TestUser, title: str, description: str, kind: str, tags: List[str],
        source_code: str, manifest: Dict[str, Any], code_format: str = "cs1",
        controls_hint: str = "Tap / Keys", remix_parent_id: Optional[str] = None,
    ) -> HttpResponse:
        payload: Dict[str, Any] = {
            "title": title, "description": description, "kind": kind, "tags": tags,
            "source_code": source_code, "code_format": code_format,
            "manifest": manifest, "controls_hint": controls_hint,
        }
        if remix_parent_id is not None:
            payload["remix_parent_id"] = remix_parent_id
        return self.client().json("POST", "/api/v1/code-scene/assets", payload, token=user.token)

    def toggle_like(self, user: TestUser, scene_id: str) -> HttpResponse:
        return self.client().json("POST", f"/api/v1/code-scene/assets/{urllib.parse.quote(scene_id, safe='')}/like", {}, token=user.token)

    def toggle_bookmark(self, user: TestUser, scene_id: str) -> HttpResponse:
        return self.client().json("POST", f"/api/v1/code-scene/assets/{urllib.parse.quote(scene_id, safe='')}/bookmark", {}, token=user.token)

    def toggle_follow(self, user: TestUser, creator_id: int) -> HttpResponse:
        return self.client().json("POST", f"/api/v1/code-scene/creators/{creator_id}/follow", {}, token=user.token)

    def list_comments(self, user: TestUser, scene_id: str) -> Tuple[HttpResponse, List[Dict[str, Any]]]:
        r = self.client().json("GET", f"/api/v1/code-scene/assets/{urllib.parse.quote(scene_id, safe='')}/comments", token=user.token)
        return r, unwrap_comments(r.body) if r.status == 200 else []

    def post_comment(self, user: TestUser, scene_id: str, text: str) -> HttpResponse:
        path = f"/api/v1/code-scene/assets/{urllib.parse.quote(scene_id, safe='')}/comments"
        with self._lock:
            key = self._comment_text_key
            locked = self._comment_key_locked
        r = self.client().json("POST", path, {key: text}, token=user.token)
        is_valid_sample = isinstance(text, str) and bool(text.strip()) and len(text.encode("utf-8")) <= 4096
        if r.status in (200, 201) and is_valid_sample and not locked:
            with self._lock:
                self._comment_key_locked = True
            return r
        if r.status in (400, 422) and not locked and is_valid_sample:
            for alt in ("text", "body", "content"):
                if alt == key:
                    continue
                r_alt = self.client().json("POST", path, {alt: text}, token=user.token)
                if r_alt.status in (200, 201):
                    with self._lock:
                        self._comment_text_key = alt
                        self._comment_key_locked = True
                    return r_alt
        return r

    def make_event(self, user: TestUser, scene_id: str, kind: str, **extra: Any) -> Dict[str, Any]:
        opts = dict(extra)
        with self._lock:
            kind_key = self._telemetry_kind_key
        ev: Dict[str, Any] = {
            "event_id": opts.pop("event_id", f"ev_{secrets.token_hex(6)}"),
            "user_id": user.user_id,
            "session_id": opts.pop("session_id", f"sess_{user.username}"),
            "scene_id": scene_id,
            kind_key: kind,
            "timestamp_ms": opts.pop("timestamp_ms", int(time.time() * 1000)),
        }
        if kind == "hide_tag":
            htag = (
                opts.pop("reason", None)
                or opts.pop("target_tag", None)
                or opts.pop("tag", None)
            )
            explicit_tags = opts.pop("tags", None)
            if htag is None and isinstance(explicit_tags, list) and explicit_tags:
                htag = explicit_tags[0]
            if htag is not None:
                ev["reason"] = htag
                ev["tags"] = list(explicit_tags) if isinstance(explicit_tags, list) and explicit_tags else [htag]
                ev["target_tag"] = htag
            elif explicit_tags is not None:
                ev["tags"] = explicit_tags
        if kind == "game_interaction":
            g_play = opts.pop("game_play_ms", None)
            alias_play = opts.pop("play_ms", None)
            if g_play is None:
                g_play = alias_play
            if g_play is not None:
                ev["game_play_ms"] = g_play
                opts.setdefault("dwell_ms", g_play)
            g_inputs = opts.pop("game_inputs", None)
            alias_inputs = opts.pop("inputs", None)
            if g_inputs is None:
                g_inputs = alias_inputs
            if g_inputs is not None:
                ev["game_inputs"] = g_inputs
            g_restarts = opts.pop("game_restarts", None)
            alias_restarts = opts.pop("restarts", None)
            if g_restarts is None:
                g_restarts = alias_restarts
            if g_restarts is not None:
                ev["game_restarts"] = g_restarts
            g_score = opts.pop("game_score", None)
            alias_score = opts.pop("score", None)
            if g_score is None:
                g_score = alias_score
            if g_score is not None:
                ev["game_score"] = g_score
        if "loops" in opts and "loop_count" not in opts:
            opts["loop_count"] = opts.pop("loops")
        ev.update(opts)
        return ev

    @staticmethod
    def _rewrite_telemetry_key(event_or_events: Any, target_key: str) -> Any:
        alt = "event_kind" if target_key == "kind" else "kind"

        def _conv(ev: Any) -> Any:
            if not isinstance(ev, dict):
                return ev
            out = dict(ev)
            if alt in out and target_key not in out:
                out[target_key] = out.pop(alt)
            return out

        if isinstance(event_or_events, list):
            return [_conv(e) for e in event_or_events]
        return _conv(event_or_events)

    @staticmethod
    def _all_known_telemetry_kinds(event_or_events: Any) -> bool:
        evs = event_or_events if isinstance(event_or_events, list) else ([event_or_events] if isinstance(event_or_events, dict) else [])
        if not evs:
            return False
        for ev in evs:
            if not isinstance(ev, dict):
                return False
            ek = ev.get("kind") if "kind" in ev else ev.get("event_kind")
            if ek not in ALL_BEHAVIOR_EVENT_KINDS:
                return False
        return True

    def send_telemetry(self, user: TestUser, event_or_events: Any, wrap_batch: bool = True) -> HttpResponse:
        with self._lock:
            cur_key = self._telemetry_kind_key
            locked = self._telemetry_key_locked
        norm = self._rewrite_telemetry_key(event_or_events, cur_key)
        payload = {"events": norm} if (isinstance(norm, list) and wrap_batch) else norm
        r = self.client().json("POST", "/api/v1/code-scene/telemetry", payload, token=user.token)
        known = self._all_known_telemetry_kinds(event_or_events)
        if r.status == 200 and known and not locked:
            with self._lock:
                self._telemetry_key_locked = True
            return r
        if r.status in (400, 422) and not locked and known:
            alt_key = "event_kind" if cur_key == "kind" else "kind"
            alt_norm = self._rewrite_telemetry_key(event_or_events, alt_key)
            alt_payload = {"events": alt_norm} if (isinstance(alt_norm, list) and wrap_batch) else alt_norm
            r_alt = self.client().json("POST", "/api/v1/code-scene/telemetry", alt_payload, token=user.token)
            if r_alt.status == 200:
                with self._lock:
                    self._telemetry_kind_key = alt_key
                    self._telemetry_key_locked = True
                return r_alt
        return r

    def ai_generate(self, user: TestUser, prompt: str, kind: Optional[str] = None, seed: Optional[int] = None) -> HttpResponse:
        payload: Dict[str, Any] = {"prompt": prompt}
        if kind is not None:
            payload["kind"] = kind
        if seed is not None:
            payload["seed"] = seed
        return self.client().json("POST", "/api/v1/code-scene/ai/generate", payload, token=user.token)

    def ai_iterate(self, user: TestUser, draft_id: Optional[str] = None, prompt: str = "", source_code: Optional[str] = None, manifest: Optional[Dict[str, Any]] = None) -> HttpResponse:
        with self._lock:
            pkey = self._iterate_prompt_key
            locked = self._iterate_key_locked
        # Never send both 'instruction' and its serde alias 'prompt' simultaneously
        payload: Dict[str, Any] = {pkey: prompt}
        if draft_id is not None:
            payload["draft_id"] = draft_id
        if source_code is not None:
            payload["source_code"] = source_code
        if manifest is not None:
            payload["manifest"] = manifest
        r = self.client().json("POST", "/api/v1/code-scene/ai/iterate", payload, token=user.token)
        valid_req = isinstance(prompt, str) and bool(prompt.strip()) and (draft_id is not None or source_code is not None)
        if r.status in (200, 201) and valid_req and not locked:
            with self._lock:
                self._iterate_key_locked = True
            return r
        if r.status in (400, 422) and not locked and valid_req:
            alt = "prompt" if pkey == "instruction" else "instruction"
            alt_payload = dict(payload)
            alt_payload.pop(pkey, None)
            alt_payload[alt] = prompt
            r_alt = self.client().json("POST", "/api/v1/code-scene/ai/iterate", alt_payload, token=user.token)
            if r_alt.status in (200, 201):
                with self._lock:
                    self._iterate_prompt_key = alt
                    self._iterate_key_locked = True
                return r_alt
        return r

    def ai_publish(self, user: TestUser, draft_id: Optional[str] = None, chat_id: Optional[int] = None, overrides: Optional[Dict[str, Any]] = None) -> HttpResponse:
        payload: Dict[str, Any] = dict(overrides or {})
        if draft_id is not None:
            payload["draft_id"] = draft_id
        if chat_id is not None:
            payload["chat_id"] = chat_id
        return self.client().json("POST", "/api/v1/code-scene/ai/publish", payload, token=user.token)

    def ai_remix(self, user: TestUser, scene_id: str, prompt: str, publish: bool = True, chat_id: Optional[int] = None) -> HttpResponse:
        with self._lock:
            id_key = self._remix_id_key
            locked = self._remix_key_locked
        # Avoid sending both 'parent_scene_id' and its serde alias 'scene_id' simultaneously
        payload: Dict[str, Any] = {id_key: scene_id, "prompt": prompt, "auto_publish": publish, "publish": publish}
        if chat_id is not None:
            payload["chat_id"] = chat_id
        r = self.client().json("POST", "/api/v1/code-scene/ai/remix", payload, token=user.token)
        valid_req = isinstance(scene_id, str) and bool(scene_id.strip()) and isinstance(prompt, str) and bool(prompt.strip())
        if r.status in (200, 201) and valid_req and not locked:
            with self._lock:
                self._remix_key_locked = True
            return r
        if r.status in (400, 422) and not locked and valid_req:
            alt = "scene_id" if id_key == "parent_scene_id" else "parent_scene_id"
            alt_payload = dict(payload)
            alt_payload.pop(id_key, None)
            alt_payload[alt] = scene_id
            r_alt = self.client().json("POST", "/api/v1/code-scene/ai/remix", alt_payload, token=user.token)
            if r_alt.status in (200, 201):
                with self._lock:
                    self._remix_id_key = alt
                    self._remix_key_locked = True
                return r_alt
        return r

    def restart_server(self) -> None:
        self.base_url = self.server_handle.restart()


@dataclass
class TestCaseSpec:
    test_id: str
    tier: int
    feature: str
    name: str
    fn: Callable[[CodeSceneEnv], None]


@dataclass
class TestResult:
    test_id: str
    tier: int
    feature: str
    name: str
    passed: bool
    elapsed_ms: float
    error: str = ""


class TestRegistry:
    def __init__(self) -> None:
        self._tests: List[TestCaseSpec] = []
        self._seen_ids: set[str] = set()

    def test(self, test_id: str, tier: int, feature: str, name: str) -> Callable[[Callable[[CodeSceneEnv], None]], Callable[[CodeSceneEnv], None]]:
        def decorator(fn: Callable[[CodeSceneEnv], None]) -> Callable[[CodeSceneEnv], None]:
            assert test_id not in self._seen_ids, f"Duplicate test_id registered: {test_id}"
            self._seen_ids.add(test_id)
            self._tests.append(TestCaseSpec(test_id=test_id, tier=tier, feature=feature, name=name, fn=fn))
            return fn
        return decorator

    def select(self, tier: Optional[int] = None, filter_substr: Optional[str] = None, only_ids: Optional[Sequence[str]] = None) -> List[TestCaseSpec]:
        out = list(self._tests)
        if tier is not None:
            out = [t for t in out if t.tier == tier]
        if filter_substr:
            needle = filter_substr.lower()
            out = [t for t in out if needle in t.test_id.lower() or needle in t.feature.lower() or needle in t.name.lower()]
        if only_ids is not None:
            wanted = set(only_ids)
            out = [t for t in out if t.test_id in wanted]
        return out

    def all_tests(self) -> List[TestCaseSpec]:
        return list(self._tests)
