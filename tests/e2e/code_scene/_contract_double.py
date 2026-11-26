"""Strict SQLite-WAL Contract Double & 14 Mutation Fault Injectors.

ARCHITECTURAL INVARIANT:
This module MUST ONLY be imported by run_all_e2e.py when --self-test is passed.
harness.py and tier1..tier4 MUST NEVER import this file.
Pure Python 3 stdlib only.
"""

from __future__ import annotations

import copy
import hashlib
import html
import json
import math
import re
import secrets
import sqlite3
import tempfile
import threading
import time
import urllib.parse
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from pathlib import Path
from typing import Any, Dict, List, Optional, Set, Tuple

from ._scripts import (
    CANONICAL_ANIMATION_CS1,
    CANONICAL_GAME_CS1,
    CANONICAL_VIDEO_CS1,
    OFFICIAL_BOTS,
    _find_on_render_span,
    build_seed_blueprints,
)
from .harness import ALL_BEHAVIOR_EVENT_KINDS, VALID_ASSET_KINDS, VALID_FEED_TABS

MUTATION_SPECS: List[Dict[str, Any]] = [
    {"id": "MUT-01", "flag": "MUT_AUTH_BYPASS", "canaries": ["CS-T2-F09-01", "CS-T2-F11-02"]},
    {"id": "MUT-02", "flag": "MUT_ACCEPT_OVERSIZED", "canaries": ["CS-T2-F02-01", "CS-T4-06"]},
    {"id": "MUT-03", "flag": "MUT_ACCEPT_INFINITE_LOOP", "canaries": ["CS-T2-F02-02", "CS-T4-06"]},
    {"id": "MUT-04", "flag": "MUT_ACCEPT_NAN_COORDS", "canaries": ["CS-T2-F02-05", "CS-T2-F03-02"]},
    {"id": "MUT-05", "flag": "MUT_LEAK_DISLIKED", "canaries": ["CS-T1-F06-05", "CS-T2-F06-02", "CS-T3-17"]},
    {"id": "MUT-06", "flag": "MUT_NO_PIVOT", "canaries": ["CS-T3-02", "CS-T4-03"]},
    {"id": "MUT-07", "flag": "MUT_NO_SCATTER", "canaries": ["CS-T1-F06-04", "CS-T3-03"]},
    {"id": "MUT-08", "flag": "MUT_DROP_REMIX_PARENT", "canaries": ["CS-T1-F10-04", "CS-T3-04"]},
    {"id": "MUT-09", "flag": "MUT_BROKEN_RESTART_PERSIST", "canaries": ["CS-T1-F09-06", "CS-T3-07"]},
    {"id": "MUT-10", "flag": "MUT_USER_LEAKAGE", "canaries": ["CS-T1-F11-01", "CS-T1-F11-02", "CS-T3-08"]},
    {"id": "MUT-11", "flag": "MUT_MISSING_CARD_ACTIONS", "canaries": ["CS-T1-F15-01", "CS-T3-06"]},
    {"id": "MUT-12", "flag": "MUT_NON_IDEMPOTENT_LIKE", "canaries": ["CS-T1-F09-02", "CS-T2-F13-01"]},
    {"id": "MUT-13", "flag": "MUT_EMPTY_SCORE_BREAKDOWN", "canaries": ["CS-T1-F01-04", "CS-T1-F15-04", "CS-T3-16"]},
    {"id": "MUT-14", "flag": "MUT_TAB_KIND_POLLUTION", "canaries": ["CS-T1-F07-01", "CS-T1-F07-03", "CS-T3-03"]},
]

SCHEMA_SQL = """
CREATE TABLE IF NOT EXISTS users (
    user_id INTEGER PRIMARY KEY,
    username TEXT UNIQUE NOT NULL,
    password TEXT NOT NULL,
    first_name TEXT NOT NULL,
    is_bot INTEGER NOT NULL DEFAULT 0,
    created_at_ms INTEGER NOT NULL
);
CREATE TABLE IF NOT EXISTS sessions (
    token TEXT PRIMARY KEY,
    session_id TEXT NOT NULL,
    user_id INTEGER NOT NULL REFERENCES users(user_id),
    revoked INTEGER NOT NULL DEFAULT 0,
    created_at_ms INTEGER NOT NULL
);
CREATE TABLE IF NOT EXISTS chats (
    chat_id INTEGER PRIMARY KEY AUTOINCREMENT,
    title TEXT NOT NULL,
    chat_type TEXT NOT NULL,
    created_by INTEGER NOT NULL,
    created_at_ms INTEGER NOT NULL
);
CREATE TABLE IF NOT EXISTS chat_members (
    chat_id INTEGER NOT NULL REFERENCES chats(chat_id),
    user_id INTEGER NOT NULL REFERENCES users(user_id),
    PRIMARY KEY (chat_id, user_id)
);
CREATE TABLE IF NOT EXISTS chat_messages (
    message_id INTEGER PRIMARY KEY AUTOINCREMENT,
    chat_id INTEGER NOT NULL REFERENCES chats(chat_id),
    sender_id INTEGER NOT NULL,
    text TEXT NOT NULL,
    content_json TEXT,
    inline_keyboard_json TEXT,
    date_ms INTEGER NOT NULL
);
CREATE TABLE IF NOT EXISTS code_scenes (
    scene_id TEXT PRIMARY KEY,
    title TEXT NOT NULL,
    description TEXT NOT NULL,
    kind TEXT NOT NULL,
    tags_json TEXT NOT NULL,
    author_user_id INTEGER NOT NULL,
    author_username TEXT NOT NULL,
    creator_bot_id INTEGER,
    creator_bot_username TEXT,
    remix_parent_id TEXT,
    source_code TEXT NOT NULL,
    code_format TEXT NOT NULL,
    manifest_json TEXT NOT NULL,
    controls_hint TEXT NOT NULL,
    plays_count INTEGER NOT NULL DEFAULT 0,
    likes_count INTEGER NOT NULL DEFAULT 0,
    bookmarks_count INTEGER NOT NULL DEFAULT 0,
    comments_count INTEGER NOT NULL DEFAULT 0,
    shares_count INTEGER NOT NULL DEFAULT 0,
    remixes_count INTEGER NOT NULL DEFAULT 0,
    created_at_ms INTEGER NOT NULL,
    updated_at_ms INTEGER NOT NULL
);
CREATE TABLE IF NOT EXISTS scene_likes (
    user_id INTEGER NOT NULL,
    scene_id TEXT NOT NULL,
    PRIMARY KEY (user_id, scene_id)
);
CREATE TABLE IF NOT EXISTS scene_bookmarks (
    user_id INTEGER NOT NULL,
    scene_id TEXT NOT NULL,
    PRIMARY KEY (user_id, scene_id)
);
CREATE TABLE IF NOT EXISTS scene_comments (
    comment_id TEXT PRIMARY KEY,
    scene_id TEXT NOT NULL REFERENCES code_scenes(scene_id),
    user_id INTEGER NOT NULL,
    username TEXT NOT NULL,
    content TEXT NOT NULL,
    created_at_ms INTEGER NOT NULL
);
CREATE TABLE IF NOT EXISTS creator_follows (
    follower_id INTEGER NOT NULL,
    creator_id INTEGER NOT NULL,
    PRIMARY KEY (follower_id, creator_id)
);
CREATE TABLE IF NOT EXISTS user_profiles (
    user_id INTEGER PRIMARY KEY,
    profile_json TEXT NOT NULL
);
CREATE TABLE IF NOT EXISTS ai_drafts (
    draft_id TEXT PRIMARY KEY,
    owner_user_id INTEGER NOT NULL,
    title TEXT NOT NULL,
    description TEXT NOT NULL,
    kind TEXT NOT NULL,
    tags_json TEXT NOT NULL,
    source_code TEXT NOT NULL,
    manifest_json TEXT NOT NULL,
    controls_hint TEXT NOT NULL,
    published_scene_id TEXT,
    created_at_ms INTEGER NOT NULL
);
"""


def _now_ms() -> int:
    return int(time.time() * 1000)


def _check_balanced_and_depth(code: str) -> Tuple[bool, int, str]:
    stack: List[str] = []
    max_depth = 0
    pairs = {")": "(", "}": "{", "]": "["}
    in_str = False
    for ch in code:
        if ch == '"':
            in_str = not in_str
            continue
        if in_str:
            continue
        if ch in "({[":
            stack.append(ch)
            if len(stack) > max_depth:
                max_depth = len(stack)
        elif ch in ")}]":
            if not stack or stack[-1] != pairs[ch]:
                return (False, max_depth, f"Unbalanced delimiter {ch!r}")
            stack.pop()
    if stack:
        return (False, max_depth, f"Unclosed delimiter {stack[-1]!r}")
    return (True, max_depth, "")


def validate_cs1(
    source_code: Any,
    manifest: Any,
    code_format: Any = "cs1",
    expected_kind: Optional[str] = None,
    mutations: Optional[Set[str]] = None,
) -> Tuple[bool, Dict[str, Any], Optional[Dict[str, Any]]]:
    """Genuine sandbox & manifest contract validator matching PROJECT.md:L160-167."""
    muts = mutations or set()
    errors: List[str] = []
    if code_format != "cs1":
        errors.append(f"Unsupported code_format {code_format!r}; expected 'cs1'")
    if not isinstance(source_code, str) or not source_code.strip():
        errors.append("Empty source_code")
        return (False, {"valid": False, "cover_hash": "", "errors": errors, "diagnostics": errors}, None)
    raw_bytes = source_code.encode("utf-8")
    if "MUT_ACCEPT_OVERSIZED" not in muts and len(raw_bytes) > 65_536:
        errors.append(f"max_source_bytes exceeded: {len(raw_bytes)} > 65536")
    if any(ord(c) < 9 or (13 < ord(c) < 32) for c in source_code):
        errors.append("Invalid control bytes in source_code")
    non_comment = "\n".join(ln for ln in source_code.splitlines() if not ln.strip().startswith("//")).strip()
    if not non_comment:
        errors.append("Comment-only source_code")
    balanced, depth, bal_err = _check_balanced_and_depth(source_code)
    if not balanced:
        errors.append(bal_err)
    if depth > 64:
        errors.append(f"max_ast_depth exceeded: {depth} > 64")
    span = _find_on_render_span(source_code)
    if span is None:
        errors.append("Missing required on_render() entrypoint")
    if not isinstance(manifest, dict):
        errors.append("Missing manifest object")
        return (False, {"valid": False, "cover_hash": "", "errors": errors, "diagnostics": errors}, None)
    mkind = manifest.get("kind")
    if mkind not in VALID_ASSET_KINDS:
        errors.append(f"Invalid manifest.kind={mkind!r}")
    if expected_kind is not None and mkind != expected_kind:
        errors.append(f"Kind mismatch: asset.kind={expected_kind!r} != manifest.kind={mkind!r}")
    clean_man = copy.deepcopy(manifest)
    w = clean_man.get("width", 360)
    h = clean_man.get("height", 640)
    fps = clean_man.get("fps", 60)
    if not all(isinstance(v, int) and not isinstance(v, bool) for v in (w, h, fps)):
        errors.append("Manifest width/height/fps must be integers")
    else:
        clean_man["width"] = max(160, min(720, w))
        clean_man["height"] = max(160, min(1280, h))
        clean_man["fps"] = max(15, min(60, fps))
    if mkind == "video":
        dur = clean_man.get("duration_sec")
        if not isinstance(dur, (int, float)) or isinstance(dur, bool) or not math.isfinite(dur) or dur <= 0.0:
            errors.append("Video manifest requires finite duration_sec > 0")
        else:
            clean_man["duration_sec"] = max(5.0, min(120.0, float(dur)))
    seed = clean_man.get("seed", 1)
    if not isinstance(seed, int) or isinstance(seed, bool) or seed < 0:
        errors.append("Manifest seed must be non-negative integer")
    if "MUT_ACCEPT_INFINITE_LOOP" not in muts:
        if re.search(r"while\s*\(\s*(?:true|1|__i\s*>=\s*0)\s*\)", source_code):
            errors.append("Runtime budget exceeded: max_ops_per_frame (100000) / infinite loop")
    for m_rec in re.finditer(r"\b(?:fn|func|def)\s+([A-Za-z_]\w*)\s*\([^)]*\)\s*\{([^}]*)\}", source_code):
        fname, fbody = m_rec.group(1), m_rec.group(2)
        if re.search(rf"\b{re.escape(fname)}\s*\(", fbody) and "if" not in fbody:
            errors.append(f"Runtime budget exceeded: max_call_depth (48) in {fname}")
    m_heap = re.search(r"while\s*\([^)]*<\s*(\d+)\s*\)[^{]*\{[^}]*(?:push|append)\(", source_code)
    if m_heap and int(m_heap.group(1)) > 32_768:
        errors.append(f"Runtime budget exceeded: max_heap_values ({m_heap.group(1)} > 32768)")
    non_comment_lines = sum(1 for ln in source_code.splitlines() if ln.strip() and not ln.strip().startswith("//"))
    stmt_count = max(source_code.count(";"), non_comment_lines)
    if stmt_count > 4_000:
        errors.append(f"Compiler budget exceeded: max_bytecode_ops ({stmt_count * 4} > 16384)")
    m_draw_loop = re.search(r"while\s*\([^)]*<\s*(\d+)\s*\)[^{]*\{[^}]*(?:draw_|fill_|rect\(|circle\()", source_code)
    if m_draw_loop and int(m_draw_loop.group(1)) > 2_048:
        errors.append(f"Runtime budget exceeded: max_draw_cmds ({m_draw_loop.group(1)} > 2048)")
    m_pix = re.search(r"pixel_buffer\s*\(\s*(\d+)\s*,\s*(\d+)\s*\)", source_code)
    if m_pix and int(m_pix.group(1)) * int(m_pix.group(2)) > 240 * 320:
        errors.append("Runtime budget exceeded: max_pixel_area (> 76800)")
    has_nan_expr = bool(re.search(r"0\.0\s*/\s*0\.0|1\.0\s*/\s*0\.0|\bNaN\b|\bInfinity\b", source_code))
    if has_nan_expr and "MUT_ACCEPT_NAN_COORDS" not in muts:
        errors.append("Non-finite NaN/Inf coordinate expression rejected")
    render_body = source_code[span[0]:span[1]] if span else ""
    draw_hits = len(re.findall(r"\b(?:draw_rect|draw_circle|draw_line|set_pixel|pixel_buffer|fill_rect)\s*\(", render_body))
    if draw_hits == 0 and "MUT_ACCEPT_NAN_COORDS" not in muts:
        errors.append("Dry-run failed: on_render emitted 0 visible draw/pixel ops")
    if errors:
        return (False, {"valid": False, "cover_hash": "", "errors": errors, "diagnostics": errors}, None)
    norm_body = re.sub(r"\s+", " ", render_body).strip()
    h_in = f"{norm_body}|{clean_man['kind']}|{clean_man['width']}x{clean_man['height']}@{clean_man['fps']}|seed={clean_man['seed']}|dur={clean_man.get('duration_sec')}"
    cover_hash = hashlib.sha256(h_in.encode("utf-8")).hexdigest()[:32]
    ops_used: Any = min(99_999, max(42, stmt_count * 12 + draw_hits * 25))
    if has_nan_expr and "MUT_ACCEPT_NAN_COORDS" in muts:
        ops_used = float("nan")
    report = {
        "valid": True, "cover_hash": cover_hash, "ops_used": ops_used,
        "draw_cmds_count": max(1, draw_hits), "errors": [], "diagnostics": [],
    }
    return (True, report, clean_man)


class DoubleStore:
    """Thread-safe SQLite-WAL store powering StrictContractDoubleServer."""

    def __init__(self, db_path: Path, mutations: Optional[Set[str]] = None) -> None:
        self.db_path = db_path
        self.mutations: Set[str] = set(mutations or set())
        self._lock = threading.RLock()
        self._cursor_snapshots: Dict[str, Tuple[int, str, List[Dict[str, Any]]]] = {}
        self._conn = sqlite3.connect(str(db_path), check_same_thread=False)
        self._conn.row_factory = sqlite3.Row
        self._conn.execute("PRAGMA journal_mode=WAL;")
        self._conn.execute("PRAGMA foreign_keys=ON;")
        self._conn.execute("PRAGMA busy_timeout=5000;")
        self._conn.executescript(SCHEMA_SQL)
        self._bootstrap_seeds()

    def close(self) -> None:
        with self._lock:
            try:
                self._conn.commit()
                self._conn.close()
            except sqlite3.Error:
                pass

    def _bootstrap_seeds(self) -> None:
        now = _now_ms()
        with self._lock:
            for bot_id, bot_name in OFFICIAL_BOTS:
                self._conn.execute(
                    "INSERT OR IGNORE INTO users(user_id, username, password, first_name, is_bot, created_at_ms) VALUES (?,?,?,?,1,?)",
                    (bot_id, bot_name, "bot_locked", bot_name, now),
                )
            for idx, bp in enumerate(build_seed_blueprints()):
                kind_slot = idx // 6
                pos_in_kind = idx % 6
                ts = now - 60_000 - (pos_in_kind * 3_000 + kind_slot * 500)
                self._conn.execute(
                    """INSERT OR IGNORE INTO code_scenes(
                        scene_id, title, description, kind, tags_json, author_user_id, author_username,
                        creator_bot_id, creator_bot_username, remix_parent_id, source_code, code_format,
                        manifest_json, controls_hint, created_at_ms, updated_at_ms
                    ) VALUES (?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?)""",
                    (
                        bp["scene_id"], bp["title"], bp["description"], bp["kind"], json.dumps(bp["tags"]),
                        bp["author_user_id"], bp["author_username"], bp["creator_bot_id"], bp["creator_bot_username"],
                        bp["remix_parent_id"], bp["source_code"], bp["code_format"],
                        json.dumps(bp["manifest"]), bp["controls_hint"], ts, ts,
                    ),
                )
            self._conn.commit()

    def register(self, body: Dict[str, Any]) -> Tuple[int, Dict[str, Any]]:
        uname = str(body.get("username", "")).strip()
        pwd = str(body.get("password", ""))
        fname = str(body.get("first_name") or "User").strip() or "User"
        if not uname or len(uname) > 128 or not pwd or len(pwd.encode("utf-8")) > 1024:
            return (400, {"error": "invalid_credentials", "message": "Invalid username or password"})
        now = _now_ms()
        with self._lock:
            row = self._conn.execute("SELECT 1 FROM users WHERE username=?", (uname,)).fetchone()
            if row:
                return (409, {"error": "user_already_exists", "message": "Username already taken"})
            max_id = self._conn.execute("SELECT MAX(user_id) FROM users WHERE user_id < 500000").fetchone()[0]
            uid = max(100_001, (max_id or 100_000) + 1)
            self._conn.execute(
                "INSERT INTO users(user_id, username, password, first_name, is_bot, created_at_ms) VALUES (?,?,?,?,0,?)",
                (uid, uname, pwd, fname, now),
            )
            sid = f"sess_{secrets.token_hex(6)}"
            tok = f"{sid}.{secrets.token_hex(16)}"
            self._conn.execute(
                "INSERT INTO sessions(token, session_id, user_id, revoked, created_at_ms) VALUES (?,?,?,0,?)",
                (tok, sid, uid, now),
            )
            self._conn.commit()
        return (201, {"user_id": uid, "username": uname, "first_name": fname, "session_id": sid, "token": tok, "auth_version": 1})

    def login(self, body: Dict[str, Any]) -> Tuple[int, Dict[str, Any]]:
        uname = str(body.get("username", "")).strip()
        pwd = str(body.get("password", ""))
        with self._lock:
            row = self._conn.execute("SELECT * FROM users WHERE username=?", (uname,)).fetchone()
            if not row or row["password"] != pwd:
                return (401, {"error": "invalid_credentials", "message": "Bad username or password"})
            sid = f"sess_{secrets.token_hex(6)}"
            tok = f"{sid}.{secrets.token_hex(16)}"
            self._conn.execute(
                "INSERT INTO sessions(token, session_id, user_id, revoked, created_at_ms) VALUES (?,?,?,0,?)",
                (tok, sid, row["user_id"], _now_ms()),
            )
            self._conn.commit()
            return (200, {"user_id": row["user_id"], "username": row["username"], "first_name": row["first_name"], "session_id": sid, "token": tok, "auth_version": 1})

    def delete_session(self, user: Dict[str, Any], session_id: str) -> Tuple[int, Dict[str, Any]]:
        with self._lock:
            cur = self._conn.execute(
                "DELETE FROM sessions WHERE session_id=? AND user_id=?",
                (session_id, user["user_id"]),
            )
            self._conn.commit()
            if cur.rowcount == 0:
                return (404, {"error": "session_not_found", "message": "Session not found for current user"})
        return (200, {"revoked": True, "session_id": session_id, "account_generation": 1})

    def auth_user(self, token: Optional[str]) -> Tuple[Optional[Dict[str, Any]], Optional[Tuple[int, Dict[str, Any]]]]:
        if not token:
            if "MUT_AUTH_BYPASS" in self.mutations:
                return ({"user_id": 777000, "username": "fabushi_ai_bot", "session_id": "bypass"}, None)
            return (None, (401, {"error": "missing_token", "message": "Bearer token required"}))
        with self._lock:
            row = self._conn.execute(
                "SELECT s.session_id, u.user_id, u.username, u.first_name, u.is_bot "
                "FROM sessions s JOIN users u ON s.user_id=u.user_id WHERE s.token=? AND s.revoked=0",
                (token,),
            ).fetchone()
        if not row:
            if "MUT_AUTH_BYPASS" in self.mutations:
                return ({"user_id": 777000, "username": "fabushi_ai_bot", "session_id": "bypass"}, None)
            return (None, (401, {"error": "invalid_token", "message": "Invalid or revoked token"}))
        return (dict(row), None)

    def create_chat(self, user: Dict[str, Any], body: Dict[str, Any]) -> Tuple[int, Dict[str, Any]]:
        ctype = str(body.get("chat_type") or "group")
        title = str(body.get("title") or f"Chat {secrets.token_hex(2)}").strip() or "Chat"
        members = {int(user["user_id"])}
        for mid in body.get("member_ids") or []:
            if isinstance(mid, int):
                members.add(mid)
        peer = body.get("peer_user_id")
        if isinstance(peer, int):
            members.add(peer)
        now = _now_ms()
        with self._lock:
            cur = self._conn.execute(
                "INSERT INTO chats(title, chat_type, created_by, created_at_ms) VALUES (?,?,?,?)",
                (title, ctype, user["user_id"], now),
            )
            cid = int(cur.lastrowid)
            for mid in members:
                exists = self._conn.execute("SELECT 1 FROM users WHERE user_id=?", (mid,)).fetchone()
                if exists:
                    self._conn.execute("INSERT OR IGNORE INTO chat_members(chat_id, user_id) VALUES (?,?)", (cid, mid))
            self._conn.commit()
        return (201, {"chat_id": cid, "id": cid, "title": title, "chat_type": ctype, "member_count": len(members)})

    def list_chats(self, user: Dict[str, Any]) -> Tuple[int, Dict[str, Any]]:
        with self._lock:
            rows = self._conn.execute(
                "SELECT c.* FROM chats c JOIN chat_members m ON c.chat_id=m.chat_id WHERE m.user_id=? ORDER BY c.chat_id ASC",
                (user["user_id"],),
            ).fetchall()
        chats = [{"chat_id": r["chat_id"], "id": r["chat_id"], "title": r["title"], "chat_type": r["chat_type"]} for r in rows]
        return (200, {"chats": chats, "total": len(chats)})

    def _is_chat_member(self, chat_id: int, user_id: int) -> Tuple[bool, bool]:
        c = self._conn.execute("SELECT 1 FROM chats WHERE chat_id=?", (chat_id,)).fetchone()
        if not c:
            return (False, False)
        m = self._conn.execute("SELECT 1 FROM chat_members WHERE chat_id=? AND user_id=?", (chat_id, user_id)).fetchone()
        return (True, bool(m))

    def send_message(self, user: Dict[str, Any], chat_id: int, body: Dict[str, Any]) -> Tuple[int, Dict[str, Any]]:
        with self._lock:
            exists, is_mem = self._is_chat_member(chat_id, user["user_id"])
            if not exists:
                return (404, {"error": "not_found", "message": "Chat not found"})
            if not is_mem:
                return (403, {"error": "forbidden", "message": "Not a member of this chat"})
            content = body.get("content")
            text = str(body.get("text") or "")
            if isinstance(content, dict) and content.get("type") == "GenerativeCard":
                data = content.get("data") or {}
                if not str(data.get("card_id", "")).strip() or not str(data.get("title", "")).strip():
                    return (400, {"error": "invalid_generative_card", "message": "card_id and title required"})
                text = f"[Card: {html.escape(str(data['title']))}]"
            now = _now_ms()
            cur = self._conn.execute(
                "INSERT INTO chat_messages(chat_id, sender_id, text, content_json, inline_keyboard_json, date_ms) VALUES (?,?,?,?,?,?)",
                (chat_id, user["user_id"], text, json.dumps(content) if content is not None else None,
                 json.dumps(body.get("inline_keyboard")) if body.get("inline_keyboard") is not None else None, now),
            )
            mid = int(cur.lastrowid)
            self._conn.commit()
        return (201, {"message_id": mid, "id": mid, "chat_id": chat_id, "sender_id": user["user_id"], "text": text, "date_ms": now, "status": "sent"})

    def list_messages(self, user: Dict[str, Any], chat_id: int) -> Tuple[int, Dict[str, Any]]:
        with self._lock:
            exists, is_mem = self._is_chat_member(chat_id, user["user_id"])
            if not exists:
                return (404, {"error": "not_found", "message": "Chat not found"})
            if not is_mem:
                return (403, {"error": "forbidden", "message": "Not a member of this chat"})
            rows = self._conn.execute("SELECT * FROM chat_messages WHERE chat_id=? ORDER BY message_id ASC", (chat_id,)).fetchall()
        msgs = []
        for r in rows:
            msgs.append({
                "message_id": r["message_id"], "id": r["message_id"], "chat_id": r["chat_id"],
                "sender_id": r["sender_id"], "text": r["text"],
                "content": json.loads(r["content_json"]) if r["content_json"] else {"type": "Text", "data": {"text": r["text"]}},
                "inline_keyboard": json.loads(r["inline_keyboard_json"]) if r["inline_keyboard_json"] else None,
                "date_ms": r["date_ms"], "is_outgoing": r["sender_id"] == user["user_id"],
            })
        return (200, {"chat_id": chat_id, "messages": msgs, "has_more": False, "pts": len(msgs), "current_pts": len(msgs)})

    def chat_callback(self, user: Dict[str, Any], chat_id: int, body: Dict[str, Any]) -> Tuple[int, Dict[str, Any]]:
        cb = str(body.get("callback_data") or "").strip()
        if not cb:
            return (400, {"error": "invalid_callback", "message": "callback_data required"})
        with self._lock:
            exists, is_mem = self._is_chat_member(chat_id, user["user_id"])
            if not exists:
                return (404, {"error": "not_found", "message": "Chat not found"})
            if not is_mem:
                return (403, {"error": "forbidden", "message": "Forbidden"})
        return (200, {"ok": True, "toast": f"Handled {cb}", "callback_data": cb})

    def _effective_uid(self, viewer_id: int) -> int:
        return 0 if "MUT_USER_LEAKAGE" in self.mutations else viewer_id

    def _row_to_asset(self, row: sqlite3.Row, viewer_id: int) -> Dict[str, Any]:
        eff = self._effective_uid(viewer_id)
        liked = bool(self._conn.execute("SELECT 1 FROM scene_likes WHERE user_id=? AND scene_id=?", (eff, row["scene_id"])).fetchone())
        bmk = bool(self._conn.execute("SELECT 1 FROM scene_bookmarks WHERE user_id=? AND scene_id=?", (eff, row["scene_id"])).fetchone())
        foll = bool(self._conn.execute("SELECT 1 FROM creator_follows WHERE follower_id=? AND creator_id=?", (viewer_id, row["author_user_id"])).fetchone())
        return {
            "scene_id": row["scene_id"], "title": row["title"], "description": row["description"],
            "kind": row["kind"], "tags": json.loads(row["tags_json"]),
            "author_user_id": row["author_user_id"], "author_username": row["author_username"],
            "creator_bot_id": row["creator_bot_id"], "creator_bot_username": row["creator_bot_username"],
            "remix_parent_id": row["remix_parent_id"], "source_code": row["source_code"],
            "code_format": row["code_format"], "manifest": json.loads(row["manifest_json"]),
            "controls_hint": row["controls_hint"], "plays_count": row["plays_count"],
            "likes_count": row["likes_count"], "bookmarks_count": row["bookmarks_count"],
            "comments_count": row["comments_count"], "shares_count": row["shares_count"],
            "remixes_count": row["remixes_count"], "viewer_liked": liked,
            "viewer_bookmarked": bmk, "viewer_following_author": foll,
            "created_at_ms": row["created_at_ms"], "updated_at_ms": row["updated_at_ms"],
        }

    def list_assets(self, viewer_id: int, qs: Dict[str, List[str]]) -> Tuple[int, Dict[str, Any]]:
        kind = (qs.get("kind") or [None])[0]
        q = (qs.get("q") or qs.get("query") or [None])[0]
        tag = (qs.get("tag") or [None])[0]
        with self._lock:
            rows = self._conn.execute("SELECT * FROM code_scenes ORDER BY created_at_ms DESC").fetchall()
            out = []
            for r in rows:
                a = self._row_to_asset(r, viewer_id)
                if kind and a["kind"] != kind:
                    continue
                if tag and tag not in a["tags"]:
                    continue
                if q and q.strip():
                    ql = q.strip().lower()
                    hay = (a["title"] + " " + a["description"] + " " + " ".join(a["tags"])).lower()
                    if ql not in hay:
                        continue
                out.append(a)
        return (200, {"assets": out, "total": len(out)})

    def get_asset(self, viewer_id: int, scene_id: str) -> Tuple[int, Dict[str, Any]]:
        if ".." in scene_id or "/" in scene_id or "\\" in scene_id:
            return (400, {"error": "invalid_scene_id", "message": "Malformed scene_id"})
        with self._lock:
            row = self._conn.execute("SELECT * FROM code_scenes WHERE scene_id=?", (scene_id,)).fetchone()
            if not row:
                return (404, {"error": "not_found", "message": f"Scene {scene_id} not found"})
            return (200, self._row_to_asset(row, viewer_id))

    def _insert_scene(
        self, user: Dict[str, Any], title: str, description: str, kind: str, tags: List[str],
        source_code: str, manifest: Dict[str, Any], controls_hint: str,
        creator_bot_id: Optional[int] = None, creator_bot_username: Optional[str] = None,
        remix_parent_id: Optional[str] = None,
    ) -> Dict[str, Any]:
        sid = f"cs_{secrets.token_hex(6)}"
        now = _now_ms()
        self._conn.execute(
            """INSERT INTO code_scenes(
                scene_id, title, description, kind, tags_json, author_user_id, author_username,
                creator_bot_id, creator_bot_username, remix_parent_id, source_code, code_format,
                manifest_json, controls_hint, created_at_ms, updated_at_ms
            ) VALUES (?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?)""",
            (
                sid, title, description, kind, json.dumps(tags, ensure_ascii=False),
                user["user_id"], user["username"], creator_bot_id, creator_bot_username,
                remix_parent_id, source_code, "cs1", json.dumps(manifest), controls_hint, now, now,
            ),
        )
        if remix_parent_id:
            self._conn.execute(
                "UPDATE code_scenes SET remixes_count = remixes_count + 1, updated_at_ms=? WHERE scene_id=?",
                (now, remix_parent_id),
            )
        prof = self._load_profile(user["user_id"])
        jp = list(prof.get("just_published") or [])
        jp.insert(0, sid)
        prof["just_published"] = jp[:20]
        self._save_profile(user["user_id"], prof)
        self._conn.commit()
        row = self._conn.execute("SELECT * FROM code_scenes WHERE scene_id=?", (sid,)).fetchone()
        return self._row_to_asset(row, user["user_id"])

    def publish_asset(self, user: Dict[str, Any], body: Dict[str, Any]) -> Tuple[int, Dict[str, Any]]:
        title = body.get("title")
        kind = body.get("kind")
        if not isinstance(title, str) or not title.strip():
            return (422, {"error": "invalid_title", "message": "Non-empty title required"})
        if kind not in VALID_ASSET_KINDS:
            return (422, {"error": "invalid_kind", "message": f"Invalid kind {kind!r}"})
        tags = body.get("tags")
        if tags is None:
            tags = []
        if not isinstance(tags, list) or not all(isinstance(t, str) for t in tags):
            return (422, {"error": "invalid_tags", "message": "tags must be list[str]"})
        ok, report, clean_man = validate_cs1(
            body.get("source_code"), body.get("manifest"),
            body.get("code_format", "cs1"), expected_kind=kind, mutations=self.mutations,
        )
        if not ok or clean_man is None:
            return (422, {"error": "validation_failed", "message": "; ".join(report.get("errors") or []), "validation": report})
        with self._lock:
            asset = self._insert_scene(
                user=user, title=title.strip(), description=str(body.get("description") or ""),
                kind=kind, tags=list(tags), source_code=str(body["source_code"]),
                manifest=clean_man, controls_hint=str(body.get("controls_hint") or "Tap / Keys"),
                creator_bot_id=body.get("creator_bot_id"), creator_bot_username=body.get("creator_bot_username"),
                remix_parent_id=body.get("remix_parent_id"),
            )
        return (201, {"asset": asset, "scene_id": asset["scene_id"], "validation": report})

    def toggle_like(self, user: Dict[str, Any], scene_id: str) -> Tuple[int, Dict[str, Any]]:
        eff = self._effective_uid(user["user_id"])
        with self._lock:
            row = self._conn.execute("SELECT likes_count FROM code_scenes WHERE scene_id=?", (scene_id,)).fetchone()
            if not row:
                return (404, {"error": "not_found", "message": "Scene not found"})
            now = _now_ms()
            if "MUT_NON_IDEMPOTENT_LIKE" in self.mutations:
                new_cnt = int(row["likes_count"]) + 1
                self._conn.execute("INSERT OR IGNORE INTO scene_likes(user_id, scene_id) VALUES (?,?)", (eff, scene_id))
                self._conn.execute("UPDATE code_scenes SET likes_count=?, updated_at_ms=? WHERE scene_id=?", (new_cnt, now, scene_id))
                self._conn.commit()
                return (200, {"liked": True, "likes_count": new_cnt, "scene_id": scene_id})
            ex = self._conn.execute("SELECT 1 FROM scene_likes WHERE user_id=? AND scene_id=?", (eff, scene_id)).fetchone()
            if ex:
                self._conn.execute("DELETE FROM scene_likes WHERE user_id=? AND scene_id=?", (eff, scene_id))
                new_cnt = max(0, int(row["likes_count"]) - 1)
                liked = False
            else:
                self._conn.execute("INSERT INTO scene_likes(user_id, scene_id) VALUES (?,?)", (eff, scene_id))
                new_cnt = int(row["likes_count"]) + 1
                liked = True
            self._conn.execute("UPDATE code_scenes SET likes_count=?, updated_at_ms=? WHERE scene_id=?", (new_cnt, now, scene_id))
            self._conn.commit()
            return (200, {"liked": liked, "likes_count": new_cnt, "scene_id": scene_id})

    def toggle_bookmark(self, user: Dict[str, Any], scene_id: str) -> Tuple[int, Dict[str, Any]]:
        eff = self._effective_uid(user["user_id"])
        with self._lock:
            row = self._conn.execute("SELECT bookmarks_count FROM code_scenes WHERE scene_id=?", (scene_id,)).fetchone()
            if not row:
                return (404, {"error": "not_found", "message": "Scene not found"})
            ex = self._conn.execute("SELECT 1 FROM scene_bookmarks WHERE user_id=? AND scene_id=?", (eff, scene_id)).fetchone()
            if ex:
                self._conn.execute("DELETE FROM scene_bookmarks WHERE user_id=? AND scene_id=?", (eff, scene_id))
                new_cnt = max(0, int(row["bookmarks_count"]) - 1)
                bmk = False
            else:
                self._conn.execute("INSERT INTO scene_bookmarks(user_id, scene_id) VALUES (?,?)", (eff, scene_id))
                new_cnt = int(row["bookmarks_count"]) + 1
                bmk = True
            self._conn.execute("UPDATE code_scenes SET bookmarks_count=?, updated_at_ms=? WHERE scene_id=?", (new_cnt, _now_ms(), scene_id))
            self._conn.commit()
            return (200, {"bookmarked": bmk, "bookmarks_count": new_cnt, "scene_id": scene_id})

    def list_comments(self, scene_id: str) -> Tuple[int, Dict[str, Any]]:
        with self._lock:
            if not self._conn.execute("SELECT 1 FROM code_scenes WHERE scene_id=?", (scene_id,)).fetchone():
                return (404, {"error": "not_found", "message": "Scene not found"})
            rows = self._conn.execute("SELECT * FROM scene_comments WHERE scene_id=? ORDER BY created_at_ms ASC", (scene_id,)).fetchall()
        out = [
            {"comment_id": r["comment_id"], "scene_id": r["scene_id"], "user_id": r["user_id"],
             "username": r["username"], "content": r["content"], "created_at_ms": r["created_at_ms"]}
            for r in rows
        ]
        return (200, {"comments": out, "total": len(out)})

    def post_comment(self, user: Dict[str, Any], scene_id: str, body: Dict[str, Any]) -> Tuple[int, Dict[str, Any]]:
        raw = body.get("content") if "content" in body else (body.get("text") if "text" in body else body.get("body"))
        if not isinstance(raw, str) or not raw.strip():
            return (422, {"error": "empty_comment", "message": "Comment content must be non-empty"})
        if len(raw.encode("utf-8")) > 4096:
            return (422, {"error": "comment_too_long", "message": "Comment exceeds 4096 bytes"})
        with self._lock:
            if not self._conn.execute("SELECT 1 FROM code_scenes WHERE scene_id=?", (scene_id,)).fetchone():
                return (404, {"error": "not_found", "message": "Scene not found"})
            cid = f"cmt_{secrets.token_hex(6)}"
            now = _now_ms()
            self._conn.execute(
                "INSERT INTO scene_comments(comment_id, scene_id, user_id, username, content, created_at_ms) VALUES (?,?,?,?,?,?)",
                (cid, scene_id, user["user_id"], user["username"], raw, now),
            )
            self._conn.execute("UPDATE code_scenes SET comments_count = comments_count + 1, updated_at_ms=? WHERE scene_id=?", (now, scene_id))
            cnt = int(self._conn.execute("SELECT comments_count FROM code_scenes WHERE scene_id=?", (scene_id,)).fetchone()[0])
            self._conn.commit()
        cmt = {"comment_id": cid, "scene_id": scene_id, "user_id": user["user_id"], "username": user["username"], "content": raw, "created_at_ms": now}
        return (201, {"comment": cmt, "comments_count": cnt, **cmt})

    def toggle_follow(self, user: Dict[str, Any], creator_id: int) -> Tuple[int, Dict[str, Any]]:
        if creator_id == user["user_id"]:
            return (400, {"error": "self_follow", "message": "Cannot follow yourself"})
        with self._lock:
            if not self._conn.execute("SELECT 1 FROM users WHERE user_id=?", (creator_id,)).fetchone():
                return (404, {"error": "not_found", "message": "Creator not found"})
            ex = self._conn.execute("SELECT 1 FROM creator_follows WHERE follower_id=? AND creator_id=?", (user["user_id"], creator_id)).fetchone()
            prof = self._load_profile(user["user_id"])
            cw = prof["creator_weights"]
            ck = str(creator_id)
            if ex:
                self._conn.execute("DELETE FROM creator_follows WHERE follower_id=? AND creator_id=?", (user["user_id"], creator_id))
                cw[ck] = max(-2.0, float(cw.get(ck, 0.0)) - 1.5)
                following = False
            else:
                self._conn.execute("INSERT INTO creator_follows(follower_id, creator_id) VALUES (?,?)", (user["user_id"], creator_id))
                cw[ck] = min(5.0, float(cw.get(ck, 0.0)) + 1.8)
                following = True
            self._save_profile(user["user_id"], prof)
            fc = int(self._conn.execute("SELECT COUNT(*) FROM creator_follows WHERE creator_id=?", (creator_id,)).fetchone()[0])
            self._conn.commit()
        return (200, {"following": following, "followers_count": fc, "creator_id": creator_id})

    def _load_profile(self, user_id: int) -> Dict[str, Any]:
        eff = self._effective_uid(user_id)
        row = self._conn.execute("SELECT profile_json FROM user_profiles WHERE user_id=?", (eff,)).fetchone()
        if row:
            return json.loads(row["profile_json"])
        return {
            "kind_weights": {"video": 0.0, "animation": 0.0, "game": 0.0},
            "tag_weights": {}, "creator_weights": {},
            "disliked_scenes": [], "hidden_authors": [], "hidden_tags": [],
            "just_published": [], "interaction_count": 0,
        }

    def _save_profile(self, user_id: int, prof: Dict[str, Any]) -> None:
        eff = self._effective_uid(user_id)
        self._conn.execute(
            "INSERT INTO user_profiles(user_id, profile_json) VALUES (?,?) ON CONFLICT(user_id) DO UPDATE SET profile_json=excluded.profile_json",
            (eff, json.dumps(prof, ensure_ascii=False)),
        )

    def record_behavior(self, user: Dict[str, Any], payload: Any) -> Tuple[int, Dict[str, Any]]:
        if isinstance(payload, dict) and "events" in payload:
            events = payload["events"]
        elif isinstance(payload, list):
            events = payload
        elif isinstance(payload, dict):
            events = [payload]
        else:
            return (400, {"error": "invalid_payload", "message": "Expected event object or batch"})
        if not isinstance(events, list):
            return (400, {"error": "invalid_payload", "message": "events must be list"})
        if len(events) == 0:
            return (200, {"accepted": 0, "reward": 0.0, "profile_updated": False})
        total_reward = 0.0
        accepted = 0
        with self._lock:
            prof = self._load_profile(user["user_id"])
            for ev in events:
                if not isinstance(ev, dict):
                    return (400, {"error": "invalid_event", "message": "Event must be dict"})
                ek = ev.get("kind") or ev.get("event_kind")
                if ek not in ALL_BEHAVIOR_EVENT_KINDS:
                    return (422, {"error": "invalid_event_kind", "message": f"Unknown event kind {ek!r}"})
                sid = str(ev.get("scene_id") or "")
                srow = self._conn.execute("SELECT * FROM code_scenes WHERE scene_id=?", (sid,)).fetchone()
                if not srow:
                    continue
                raw_play = (
                    ev.get("game_play_ms")
                    if ev.get("game_play_ms") is not None
                    else (ev.get("dwell_ms") if ev.get("dwell_ms") is not None else ev.get("play_ms"))
                )
                dwell_ms = max(0.0, min(600_000.0, float(raw_play or 0.0)))
                comp = max(0.0, min(3.0, float(ev.get("completion_ratio") or 0.0)))
                raw_inputs = ev.get("game_inputs") if ev.get("game_inputs") is not None else ev.get("inputs")
                inputs = max(0, min(10_000, int(raw_inputs or 0)))
                raw_restarts = ev.get("game_restarts") if ev.get("game_restarts") is not None else ev.get("restarts")
                restarts = max(0, min(10_000, int(raw_restarts or 0)))
                raw_score = ev.get("game_score") if ev.get("game_score") is not None else ev.get("score")
                score = max(0.0, min(1_000_000.0, float(raw_score or 0.0)))
                if ek == "impression":
                    r = 0.05
                    self._conn.execute("UPDATE code_scenes SET plays_count = plays_count + 1 WHERE scene_id=?", (sid,))
                elif ek == "dwell":
                    r = min(1.8, 0.25 + (dwell_ms / 10000.0) + 0.3 * comp)
                elif ek == "complete":
                    r = 2.0 + 0.4 * min(3.0, comp)
                    self._conn.execute("UPDATE code_scenes SET plays_count = plays_count + 1 WHERE scene_id=?", (sid,))
                elif ek == "replay":
                    r = 2.5 + 0.2 * min(5.0, float(ev.get("loop_count") or ev.get("loops") or 1))
                elif ek == "game_interaction":
                    r = 1.6 + min(1.5, (dwell_ms / 25000.0) + (inputs / 80.0) + (restarts * 0.05) + math.log1p(score) * 0.08)
                elif ek in ("like", "double_tap_like"):
                    r = 2.2
                elif ek in ("bookmark", "comment", "share", "follow_author", "inspect_code", "remix"):
                    r = 2.4 if ek != "inspect_code" else 1.7
                    if ek == "share":
                        self._conn.execute("UPDATE code_scenes SET shares_count = shares_count + 1 WHERE scene_id=?", (sid,))
                elif ek in ("unlike", "unbookmark", "unfollow_author"):
                    r = -0.5
                elif ek == "fast_skip":
                    r = -1.8 if dwell_ms < 1500.0 else (-0.7 if dwell_ms < 3000.0 else -0.2)
                elif ek == "not_interested":
                    r = -4.0
                    if sid not in prof["disliked_scenes"]:
                        prof["disliked_scenes"].append(sid)
                elif ek == "hide_author":
                    r = -5.0
                    aid = int(ev.get("author_user_id") or srow["author_user_id"])
                    if aid not in prof["hidden_authors"]:
                        prof["hidden_authors"].append(aid)
                elif ek == "hide_tag":
                    r = -4.5
                    ev_tags = ev.get("tags") if isinstance(ev.get("tags"), list) else []
                    t = (
                        (str(ev["reason"]).strip() if isinstance(ev.get("reason"), str) and ev["reason"].strip() else None)
                        or (str(ev_tags[0]).strip() if ev_tags and isinstance(ev_tags[0], str) and ev_tags[0].strip() else None)
                        or ev.get("target_tag")
                        or ev.get("tag")
                    )
                    if not t:
                        stags = json.loads(srow["tags_json"])
                        t = stags[0] if stags else None
                    if t and str(t) not in prof["hidden_tags"]:
                        prof["hidden_tags"].append(str(t))
                else:
                    r = 0.0
                total_reward += r
                accepted += 1
                if "MUT_NO_PIVOT" not in self.mutations and ek != "impression":
                    kw = prof["kind_weights"]
                    k = srow["kind"]
                    kw[k] = max(-6.0, min(8.0, float(kw.get(k, 0.0)) * 0.88 + 0.35 * r))
                    tw = prof["tag_weights"]
                    for tag in json.loads(srow["tags_json"]):
                        tw[tag] = max(-5.0, min(6.0, float(tw.get(tag, 0.0)) * 0.90 + 0.25 * r))
                    cw = prof["creator_weights"]
                    ck = str(srow["author_user_id"])
                    cw[ck] = max(-5.0, min(6.0, float(cw.get(ck, 0.0)) * 0.92 + 0.20 * r))
                prof["interaction_count"] = int(prof.get("interaction_count", 0)) + 1
            self._save_profile(user["user_id"], prof)
            self._conn.commit()
        return (200, {"accepted": accepted, "reward": round(total_reward, 4), "profile_updated": accepted > 0})

    def recommend_feed(self, user: Dict[str, Any], body: Dict[str, Any]) -> Tuple[int, Dict[str, Any]]:
        tab = body["tab"] if ("tab" in body and body["tab"] is not None) else "for_you"
        if tab not in VALID_FEED_TABS:
            return (422, {"error": "invalid_tab", "message": f"Invalid feed tab {tab!r}"})
        raw_limit = body.get("limit", 10)
        if not isinstance(raw_limit, int) or isinstance(raw_limit, bool):
            return (400, {"error": "invalid_limit", "message": "limit must be int"})
        if raw_limit < 0:
            return (400, {"error": "invalid_limit", "message": "limit must be >= 0"})
        limit = max(1, min(100, raw_limit)) if raw_limit > 0 else 0
        cursor_raw = body.get("cursor")
        uid = user["user_id"]
        if cursor_raw is not None:
            if not isinstance(cursor_raw, str):
                return (400, {"error": "invalid_cursor", "message": f"Malformed cursor {cursor_raw!r}"})
            m_cur = re.fullmatch(r"cur_([0-9a-f]+)_(\d+)", cursor_raw)
            if not m_cur:
                return (400, {"error": "invalid_cursor", "message": f"Malformed cursor {cursor_raw!r}"})
            snap_id = m_cur.group(1)
            offset = int(m_cur.group(2))
            with self._lock:
                total_catalog = int(self._conn.execute("SELECT COUNT(*) FROM code_scenes").fetchone()[0])
                snap = self._cursor_snapshots.get(snap_id)
            if snap is None or snap[0] != uid or snap[1] != tab:
                return (400, {"error": "invalid_cursor", "message": f"Expired or invalid cursor {cursor_raw!r}"})
            snap_items = snap[2]
            if limit == 0 or offset >= len(snap_items):
                return (200, {"items": [], "next_cursor": None, "total_catalog": total_catalog})
            page_items = list(snap_items[offset:offset + limit])
            if "MUT_TAB_KIND_POLLUTION" in self.mutations and offset > 0 and snap_items:
                page_items = [snap_items[0]] + page_items[:-1]
            next_cur = f"cur_{snap_id}_{offset + limit}" if (offset + limit) < len(snap_items) else None
            return (200, {"items": page_items, "next_cursor": next_cur, "total_catalog": total_catalog})
        tag_filter = body.get("tag_filter")
        query = body.get("query")
        with self._lock:
            all_rows = self._conn.execute("SELECT * FROM code_scenes").fetchall()
            total_catalog = len(all_rows)
            prof = self._load_profile(uid)
            followed = {
                int(r["creator_id"])
                for r in self._conn.execute("SELECT creator_id FROM creator_follows WHERE follower_id=?", (uid,)).fetchall()
            }
            assets = [self._row_to_asset(r, uid) for r in all_rows]
        disliked = set(prof.get("disliked_scenes") or [])
        hid_authors = set(int(x) for x in (prof.get("hidden_authors") or []))
        hid_tags = set(prof.get("hidden_tags") or [])
        just_pub = set(prof.get("just_published") or [])
        kw = prof.get("kind_weights") or {}
        tw = prof.get("tag_weights") or {}
        cw = prof.get("creator_weights") or {}
        now = _now_ms()
        filtered: List[Dict[str, Any]] = []
        for a in assets:
            if tab == "videos" and a["kind"] != "video":
                continue
            elif tab == "animations" and a["kind"] != "animation":
                continue
            elif tab == "games" and a["kind"] != "game":
                if not ("MUT_TAB_KIND_POLLUTION" in self.mutations and a["kind"] == "video"):
                    continue
            elif tab == "my_works" and a["author_user_id"] != uid:
                continue
            elif tab == "following" and a["author_user_id"] not in followed:
                continue
            elif tab == "for_you" and "MUT_LEAK_DISLIKED" not in self.mutations:
                if a["scene_id"] in disliked or a["author_user_id"] in hid_authors:
                    continue
                if any(t in hid_tags for t in a["tags"]):
                    continue
            if tag_filter and tag_filter not in a["tags"]:
                continue
            if isinstance(query, str) and query.strip():
                ql = query.strip().lower()
                hay = (a["title"] + " " + a["description"] + " " + " ".join(a["tags"])).lower()
                if ql not in hay:
                    continue
            filtered.append(a)
        scored: List[Tuple[float, Dict[str, Any], Dict[str, Any]]] = []
        for a in filtered:
            k_aff = float(kw.get(a["kind"], 0.0))
            t_aff = sum(float(tw.get(t, 0.0)) for t in a["tags"]) / max(1, len(a["tags"]))
            c_aff = float(cw.get(str(a["author_user_id"]), 0.0)) + (1.5 if a["author_user_id"] in followed else 0.0)
            eng = a["likes_count"] * 1.4 + a["bookmarks_count"] * 1.8 + a["comments_count"] * 1.2 + a["remixes_count"] * 2.5
            q_ecpm = math.log1p(eng + a["plays_count"] * 0.2) * 0.65 + 0.5
            age_sec = max(0.0, (now - a["created_at_ms"]) / 1000.0)
            age_hr = age_sec / 3600.0
            fresh = 1.0 / (1.0 + age_hr * 0.15)
            is_new = age_sec < 30.0 and a["plays_count"] < 5
            is_fast = a["scene_id"] in just_pub
            cold_viewer = int(prof.get("interaction_count", 0)) < 3
            if is_fast:
                expl = 4.2
            elif is_new and cold_viewer:
                expl = 3.6 / (1.0 + age_sec * 2.0) + 0.6 / math.sqrt(1.0 + a["plays_count"])
            elif is_new:
                expl = 1.8 / (1.0 + age_sec * 1.5) + 0.4 / math.sqrt(1.0 + a["plays_count"])
            else:
                expl = 0.35
            emb = round(max(-1.0, min(1.0, (k_aff * 0.35 + t_aff * 0.25) / 3.0)), 4)
            swing = round(min(1.5, eng * 0.05), 4)
            neg = -2.0 if a["scene_id"] in disliked else 0.0
            final = round(emb * 1.5 + swing + k_aff * 1.15 + t_aff * 0.8 + c_aff + q_ecpm + fresh + expl + neg, 4)
            channels = []
            if is_fast:
                channels.append("creator_just_published")
            if c_aff > 0.5:
                channels.append("creator_graph")
            if k_aff > 0.5 or t_aff > 0.3:
                channels.append("tag_kind_affinity")
            if is_new:
                channels.append("cold_start_exploration")
            if not channels:
                channels.append("hot_velocity_pool")
            tier = "creator_fast_track" if is_fast else ("cold_start" if is_new else ("hot_pool" if eng >= 3 else "normal"))
            reason_zh = "刚发布的作品推荐" if is_fast else ("根据你的互动偏好推荐" if k_aff > 0.5 else ("关注创作者精选" if c_aff > 0.5 else "代码场景冷启动探索"))
            reason_en = "Just published spotlight" if is_fast else ("Matched to your interaction taste" if k_aff > 0.5 else ("From creators you follow" if c_aff > 0.5 else "Cold-start discovery pick"))
            bd: Dict[str, Any] = {
                "embedding_sim": emb, "swing_cf": swing, "tag_kind_affinity": round(k_aff + t_aff, 4),
                "creator_affinity": round(c_aff, 4), "quality_ecpm": round(q_ecpm, 4),
                "freshness": round(fresh, 4), "exploration_bonus": round(expl, 4),
                "mmr_penalty": 0.0, "negative_penalty": neg, "final_score": final,
            }
            exp = {"recall_channels": channels, "primary_reason_zh": reason_zh, "primary_reason_en": reason_en, "traffic_tier": tier, "breakdown": bd}
            if "MUT_EMPTY_SCORE_BREAKDOWN" in self.mutations:
                exp["primary_reason_zh"] = ""
                exp["recall_channels"] = []
                bd["final_score"] = float("nan")
            scored.append((final, a, exp))
        if tab == "for_you" and "MUT_NO_SCATTER" in self.mutations:
            author_freq: Dict[int, int] = {}
            for _, a_item, _ in scored:
                aid = int(a_item["author_user_id"])
                author_freq[aid] = author_freq.get(aid, 0) + 1
            kind_order = {"video": 0, "animation": 1, "game": 2}
            scored.sort(
                key=lambda x: (
                    -author_freq[int(x[1]["author_user_id"])],
                    int(x[1]["author_user_id"]),
                    kind_order.get(x[1]["kind"], 9),
                    x[1]["scene_id"],
                )
            )
            ranked = scored
        else:
            scored.sort(key=lambda x: (-x[0], -x[1]["created_at_ms"], x[1]["scene_id"]))
            ranked = self._apply_scatter(scored) if tab == "for_you" else scored
        if limit == 0 or not ranked:
            return (200, {"items": [], "next_cursor": None, "total_catalog": total_catalog})
        all_items = [{"asset": a, "explanation": exp} for (_, a, exp) in ranked]
        page = all_items[0:limit]
        next_cur: Optional[str] = None
        if limit < len(all_items):
            snap_id = secrets.token_hex(6)
            with self._lock:
                if len(self._cursor_snapshots) >= 256:
                    oldest_key = next(iter(self._cursor_snapshots))
                    self._cursor_snapshots.pop(oldest_key, None)
                self._cursor_snapshots[snap_id] = (uid, tab, all_items)
            next_cur = f"cur_{snap_id}_{limit}"
        return (200, {"items": page, "next_cursor": next_cur, "total_catalog": total_catalog})

    def _apply_scatter(self, scored: List[Tuple[float, Dict[str, Any], Dict[str, Any]]]) -> List[Tuple[float, Dict[str, Any], Dict[str, Any]]]:
        pool = list(scored)
        out: List[Tuple[float, Dict[str, Any], Dict[str, Any]]] = []
        while pool:
            chosen_idx = 0
            if len(out) >= 2:
                prev2_authors = [out[-1][1]["author_user_id"], out[-2][1]["author_user_id"]]
                need_diff_author = prev2_authors[0] == prev2_authors[1]
                prev2_kinds = [out[-1][1]["kind"], out[-2][1]["kind"]]
                need_diff_kind = prev2_kinds[0] == prev2_kinds[1]
                if need_diff_author or need_diff_kind:
                    for i, cand in enumerate(pool):
                        ok_a = (not need_diff_author) or (cand[1]["author_user_id"] != prev2_authors[0])
                        ok_k = (not need_diff_kind) or (cand[1]["kind"] != prev2_kinds[0])
                        if ok_a and ok_k:
                            chosen_idx = i
                            break
                    else:
                        if need_diff_author:
                            for i, cand in enumerate(pool):
                                if cand[1]["author_user_id"] != prev2_authors[0]:
                                    chosen_idx = i
                                    break
            elif len(out) == 1 and len(set(x[1]["kind"] for x in pool)) >= 2:
                # Guarantee multi-kind diversity in cold-start top window when scores are close
                top_score = pool[0][0]
                if pool[0][1]["kind"] == out[0][1]["kind"]:
                    for i, cand in enumerate(pool):
                        if cand[1]["kind"] != out[0][1]["kind"] and (top_score - cand[0]) < 1.2:
                            chosen_idx = i
                            break
            item = pool.pop(chosen_idx)
            if chosen_idx > 0:
                item[2]["breakdown"]["mmr_penalty"] = -0.15
            out.append(item)
        return out

    def _infer_kind_from_prompt(self, prompt: str, explicit_kind: Optional[str]) -> str:
        if explicit_kind in VALID_ASSET_KINDS:
            return explicit_kind
        pl = prompt.lower()
        if any(w in pl for w in ("game", "游戏", "打砖块", "弹球", "arcade", "pong", "snake", "brick")):
            return "game"
        if any(w in pl for w in ("video", "视频", "倒计时", "countdown", "clip", "story", "烟花")):
            return "video"
        return "animation"

    def _synthesize_code(self, kind: str, prompt: str, seed: int, iteration: int = 1) -> str:
        base = CANONICAL_VIDEO_CS1 if kind == "video" else (CANONICAL_GAME_CS1 if kind == "game" else CANONICAL_ANIMATION_CS1)
        r = 60 + (seed * 37 + iteration * 29) % 180
        g = 80 + (seed * 53 + iteration * 41) % 165
        b = 100 + (seed * 71 + iteration * 17) % 150
        y_off = 140.0 + float((seed + iteration * 19) % 240)
        safe_tag = re.sub(r"[\r\n]", " ", prompt[:48])
        injected = f"    draw_circle(180.0, {y_off:.1f}, {20 + iteration * 4}.0, {r}, {g}, {b}, 255);\n"
        span = _find_on_render_span(base)
        if span:
            return f"// AI synth iter={iteration} prompt={safe_tag}\n" + base[:span[0]] + "\n" + injected + base[span[0]:]
        return base

    def ai_generate(self, user: Dict[str, Any], body: Dict[str, Any]) -> Tuple[int, Dict[str, Any]]:
        prompt = body.get("prompt")
        if not isinstance(prompt, str) or not prompt.strip():
            return (422, {"error": "empty_prompt", "message": "Prompt must be non-empty"})
        bounded_prompt = prompt.strip()[:2048]
        kind = self._infer_kind_from_prompt(bounded_prompt, body.get("kind"))
        seed = int(body.get("seed") or (int(hashlib.sha256(bounded_prompt.encode("utf-8")).hexdigest()[:6], 16) % 9000 + 1))
        src = self._synthesize_code(kind, bounded_prompt, seed, iteration=1)
        man = {"kind": kind, "width": 360, "height": 640, "fps": 30 if kind == "video" else 60, "duration_sec": 15.0 if kind == "video" else None, "seed": seed}
        ok, report, clean_man = validate_cs1(src, man, "cs1", expected_kind=kind, mutations=self.mutations)
        assert ok and clean_man is not None
        did = f"draft_{secrets.token_hex(6)}"
        title = bounded_prompt[:40]
        tags = [kind, "ai_bot", "neon"]
        controls = "Arrow Keys / Tap" if kind == "game" else ("Space Play/Pause" if kind == "video" else "Move Pointer")
        now = _now_ms()
        with self._lock:
            self._conn.execute(
                """INSERT INTO ai_drafts(draft_id, owner_user_id, title, description, kind, tags_json, source_code, manifest_json, controls_hint, published_scene_id, created_at_ms)
                   VALUES (?,?,?,?,?,?,?,?,?,NULL,?)""",
                (did, user["user_id"], title, f"Generated by @fabushi_ai_bot: {bounded_prompt[:120]}", kind,
                 json.dumps(tags, ensure_ascii=False), src, json.dumps(clean_man), controls, now),
            )
            self._conn.commit()
        return (200, {
            "draft_id": did, "title": title, "description": f"Generated by @fabushi_ai_bot: {bounded_prompt[:120]}",
            "kind": kind, "tags": tags, "source_code": src, "code_format": "cs1",
            "manifest": clean_man, "controls_hint": controls, "validation": report,
            "cover_hash": report["cover_hash"], "engine": "offline_procedural",
        })

    def ai_iterate(self, user: Dict[str, Any], body: Dict[str, Any]) -> Tuple[int, Dict[str, Any]]:
        instr = body.get("instruction") or body.get("prompt") or ""
        if not isinstance(instr, str) or not instr.strip():
            return (422, {"error": "empty_instruction", "message": "Iteration instruction required"})
        did = body.get("draft_id")
        if not isinstance(did, str) or not did.strip():
            return (400, {"error": "missing_draft_id", "message": "draft_id required"})
        with self._lock:
            row = self._conn.execute("SELECT * FROM ai_drafts WHERE draft_id=?", (did,)).fetchone()
            if not row:
                return (404, {"error": "not_found", "message": "Draft not found"})
            if int(row["owner_user_id"]) != int(user["user_id"]):
                return (403, {"error": "forbidden", "message": "Draft belongs to another user"})
            man = json.loads(row["manifest_json"])
            man["seed"] = int(man.get("seed", 1)) + 13
            prev_iter = len(re.findall(r"AI synth iter=", row["source_code"])) + 1
            new_src = self._synthesize_code(row["kind"], instr.strip()[:256], man["seed"], iteration=prev_iter + 1)
            ok, report, clean_man = validate_cs1(new_src, man, "cs1", expected_kind=row["kind"], mutations=self.mutations)
            assert ok and clean_man is not None
            self._conn.execute("UPDATE ai_drafts SET source_code=?, manifest_json=? WHERE draft_id=?", (new_src, json.dumps(clean_man), did))
            self._conn.commit()
        return (200, {
            "draft_id": did, "title": row["title"], "description": row["description"],
            "kind": row["kind"], "tags": json.loads(row["tags_json"]), "source_code": new_src,
            "code_format": "cs1", "manifest": clean_man, "controls_hint": row["controls_hint"],
            "validation": report, "cover_hash": report["cover_hash"], "engine": "offline_procedural",
        })

    def ai_publish(self, user: Dict[str, Any], body: Dict[str, Any]) -> Tuple[int, Dict[str, Any]]:
        did = body.get("draft_id")
        chat_id = body.get("chat_id")
        if chat_id is not None:
            if not isinstance(chat_id, int) or isinstance(chat_id, bool) or chat_id <= 0:
                return (400, {"error": "invalid_chat_id", "message": "chat_id must be positive int"})
        with self._lock:
            if chat_id is not None:
                exists, is_mem = self._is_chat_member(chat_id, user["user_id"])
                if not exists:
                    return (404, {"error": "not_found", "message": "Chat not found"})
                if not is_mem:
                    return (403, {"error": "forbidden", "message": "Not a member of target chat"})
            row = None
            if did is not None:
                row = self._conn.execute("SELECT * FROM ai_drafts WHERE draft_id=?", (did,)).fetchone()
                if not row:
                    return (404, {"error": "not_found", "message": "Draft not found"})
                if int(row["owner_user_id"]) != int(user["user_id"]):
                    return (403, {"error": "forbidden", "message": "Draft belongs to another user"})
                if row["published_scene_id"]:
                    return (409, {"error": "already_published", "message": "Draft already published", "scene_id": row["published_scene_id"]})
            title = str(body.get("title") or (row["title"] if row else "")).strip()
            desc = str(body.get("description") if "description" in body else (row["description"] if row else ""))
            kind = body.get("kind") or (row["kind"] if row else None)
            tags = body.get("tags") if "tags" in body else (json.loads(row["tags_json"]) if row else ["ai_bot"])
            src = body.get("source_code") if "source_code" in body else (row["source_code"] if row else "")
            man = body.get("manifest") if "manifest" in body else (json.loads(row["manifest_json"]) if row else None)
            controls = str(body.get("controls_hint") or (row["controls_hint"] if row else "Tap / Keys"))
            ok, report, clean_man = validate_cs1(src, man, body.get("code_format", "cs1"), expected_kind=kind, mutations=self.mutations)
            if not ok or clean_man is None or not title:
                return (422, {"error": "validation_failed", "message": "; ".join(report.get("errors") or ["Invalid draft"]), "validation": report})
            asset = self._insert_scene(
                user=user, title=title, description=desc, kind=kind, tags=list(tags),
                source_code=src, manifest=clean_man, controls_hint=controls,
                creator_bot_id=777000, creator_bot_username="fabushi_ai_bot",
            )
            if did is not None:
                self._conn.execute("UPDATE ai_drafts SET published_scene_id=? WHERE draft_id=?", (asset["scene_id"], did))
                self._conn.commit()
            msg_id = None
            card = None
            if chat_id is not None:
                sid = asset["scene_id"]
                safe_title = html.escape(asset["title"])
                comps: List[Dict[str, Any]] = [
                    {"type": "Text", "text": html.escape(asset["description"][:140]), "bold": False, "color": None},
                    {"type": "Badge", "label": asset["kind"], "color": "#2AABEE"},
                    {"type": "Button", "id": "open", "label": "打开场景", "action_payload": f"code_scene:open:{sid}"},
                ]
                if "MUT_MISSING_CARD_ACTIONS" not in self.mutations:
                    comps.append({"type": "Button", "id": "remix", "label": "二创 Remix", "action_payload": f"code_scene:remix:{sid}"})
                    comps.append({"type": "Button", "id": "share", "label": "转发分享", "action_payload": f"code_scene:share:{sid}"})
                card = {
                    "card_id": f"cs_card_{sid}", "title": safe_title,
                    "subtitle": f"by @fabushi_ai_bot · {asset['kind']}",
                    "layout": "VerticalStack", "components": comps, "status": "published",
                }
                st, mresp = self.send_message(user, chat_id, {"content": {"type": "GenerativeCard", "data": card}})
                assert st == 201
                msg_id = mresp["message_id"]
        return (201, {"asset": asset, "scene_id": asset["scene_id"], "validation": report, "message_id": msg_id, "card": card})

    def ai_remix(self, user: Dict[str, Any], body: Dict[str, Any]) -> Tuple[int, Dict[str, Any]]:
        parent_id = body.get("parent_scene_id") or body.get("scene_id")
        prompt = body.get("prompt") or ""
        if not isinstance(parent_id, str) or not parent_id.strip():
            return (400, {"error": "missing_scene_id", "message": "parent_scene_id or scene_id required"})
        if not isinstance(prompt, str) or not prompt.strip():
            return (422, {"error": "empty_prompt", "message": "Remix prompt required"})
        should_publish = bool(body["auto_publish"]) if "auto_publish" in body else bool(body.get("publish", True))
        with self._lock:
            prow = self._conn.execute("SELECT * FROM code_scenes WHERE scene_id=?", (parent_id,)).fetchone()
            if not prow:
                return (404, {"error": "not_found", "message": "Parent scene not found"})
            pman = json.loads(prow["manifest_json"])
            pman["seed"] = int(pman.get("seed", 1)) + 31
            src = self._synthesize_code(prow["kind"], prompt.strip()[:120], pman["seed"], iteration=2)
            ok, report, clean_man = validate_cs1(src, pman, "cs1", expected_kind=prow["kind"], mutations=self.mutations)
            assert ok and clean_man is not None
            link_parent = None if "MUT_DROP_REMIX_PARENT" in self.mutations else parent_id
            tags = list(set(json.loads(prow["tags_json"]) + ["remix"]))
            title = f"Remix: {prow['title'][:32]}"
            desc = f"Remixed: {prompt.strip()[:120]}"
            if not should_publish:
                did = f"draft_{secrets.token_hex(6)}"
                now = _now_ms()
                self._conn.execute(
                    """INSERT INTO ai_drafts(draft_id, owner_user_id, title, description, kind, tags_json, source_code, manifest_json, controls_hint, published_scene_id, created_at_ms)
                       VALUES (?,?,?,?,?,?,?,?,?,NULL,?)""",
                    (did, user["user_id"], title, desc, prow["kind"], json.dumps(tags, ensure_ascii=False),
                     src, json.dumps(clean_man), prow["controls_hint"], now),
                )
                self._conn.commit()
                draft_dto = {
                    "draft_id": did, "title": title, "description": desc, "kind": prow["kind"],
                    "tags": tags, "source_code": src, "code_format": "cs1", "manifest": clean_man,
                    "controls_hint": prow["controls_hint"], "validation": report,
                    "remix_parent_id": link_parent,
                }
                return (200, {"draft": draft_dto, "published": None})
            asset = self._insert_scene(
                user=user, title=title, description=desc,
                kind=prow["kind"], tags=tags, source_code=src, manifest=clean_man,
                controls_hint=prow["controls_hint"], creator_bot_id=777000, creator_bot_username="fabushi_ai_bot",
                remix_parent_id=link_parent,
            )
        return (201, {
            "asset": asset, "scene_id": asset["scene_id"], "remix_parent_id": link_parent,
            "validation": report, "published": {"asset": asset, "validation": report},
        })


def _make_handler(store: DoubleStore) -> type[BaseHTTPRequestHandler]:
    class _Handler(BaseHTTPRequestHandler):
        def log_message(self, fmt: str, *args: Any) -> None:
            return

        def _send_json(self, status: int, data: Any) -> None:
            allow_nan = bool(store.mutations & {"MUT_EMPTY_SCORE_BREAKDOWN", "MUT_ACCEPT_NAN_COORDS"})
            raw = json.dumps(data, ensure_ascii=False, allow_nan=allow_nan).encode("utf-8")
            self.send_response(status)
            self.send_header("Content-Type", "application/json; charset=utf-8")
            self.send_header("Content-Length", str(len(raw)))
            self.end_headers()
            self.wfile.write(raw)

        def _send_text(self, status: int, text: str) -> None:
            raw = text.encode("utf-8")
            self.send_response(status)
            self.send_header("Content-Type", "text/plain; charset=utf-8")
            self.send_header("Content-Length", str(len(raw)))
            self.end_headers()
            self.wfile.write(raw)

        def _extract_token(self, qs: Dict[str, List[str]]) -> Optional[str]:
            auth = self.headers.get("Authorization") or ""
            if auth.lower().startswith("bearer "):
                tok = auth[7:].strip()
                return tok or None
            qtok = (qs.get("token") or [None])[0]
            return qtok.strip() if isinstance(qtok, str) and qtok.strip() else None

        def _read_json(self, allow_empty: bool = False, allow_list: bool = False) -> Tuple[Optional[Any], Optional[Tuple[int, Dict[str, Any]]]]:
            cl = int(self.headers.get("Content-Length") or "0")
            raw = self.rfile.read(cl) if cl > 0 else b""
            if not raw:
                return ({} if allow_empty else None), (None if allow_empty else (400, {"error": "empty_body", "message": "Empty JSON body"}))
            try:
                val = json.loads(raw.decode("utf-8"))
            except ValueError as exc:
                return None, (400, {"error": "invalid_json", "message": str(exc)})
            if isinstance(val, dict):
                return val, None
            if allow_list and isinstance(val, list):
                return val, None
            return None, (400, {"error": "invalid_payload", "message": "JSON object required"})

        def _dispatch(self, method: str) -> None:
            parsed = urllib.parse.urlparse(self.path)
            path = parsed.path
            qs = urllib.parse.parse_qs(parsed.query)
            if method == "GET" and path == "/health":
                return self._send_text(200, "OK")
            if method == "GET" and path == "/api/v1/status":
                return self._send_json(200, {
                    "server": "gpui_telegram_server", "status": "healthy", "version": "1.6.11",
                    "capabilities": ["code_scene", "auth", "chats", "generative_cards"],
                })
            if method == "POST" and path == "/api/v1/auth/register":
                body, err = self._read_json()
                return self._send_json(*(err if err else store.register(body)))
            if method == "POST" and path == "/api/v1/auth/login":
                body, err = self._read_json()
                return self._send_json(*(err if err else store.login(body)))
            tok = self._extract_token(qs)
            user, aerr = store.auth_user(tok)
            if aerr or user is None:
                return self._send_json(*(aerr or (401, {"error": "unauthorized", "message": "Unauthorized"})))
            m_sess = re.fullmatch(r"/api/v1/auth/sessions/([^/]+)", path)
            if m_sess and method == "DELETE":
                return self._send_json(*store.delete_session(user, urllib.parse.unquote(m_sess.group(1))))
            if method == "GET" and path == "/api/v1/auth/me":
                return self._send_json(200, user)
            if path == "/api/v1/chats":
                if method == "GET":
                    return self._send_json(*store.list_chats(user))
                if method == "POST":
                    body, err = self._read_json(allow_empty=True)
                    return self._send_json(*(err if err else store.create_chat(user, body)))
            m_msg = re.fullmatch(r"/api/v1/chats/(\d+)/messages", path)
            if m_msg:
                cid = int(m_msg.group(1))
                if method == "GET":
                    return self._send_json(*store.list_messages(user, cid))
                if method == "POST":
                    body, err = self._read_json()
                    return self._send_json(*(err if err else store.send_message(user, cid, body)))
            m_cb = re.fullmatch(r"/api/v1/chats/(\d+)/callback", path)
            if m_cb and method == "POST":
                body, err = self._read_json()
                return self._send_json(*(err if err else store.chat_callback(user, int(m_cb.group(1)), body)))
            # ---- Code Scene API (/api/v1/code-scene/*) ----
            if method == "POST" and path == "/api/v1/code-scene/feed":
                body, err = self._read_json(allow_empty=True)
                return self._send_json(*(err if err else store.recommend_feed(user, body)))
            if path == "/api/v1/code-scene/assets":
                if method == "GET":
                    return self._send_json(*store.list_assets(user["user_id"], qs))
                if method == "POST":
                    body, err = self._read_json()
                    return self._send_json(*(err if err else store.publish_asset(user, body)))
            if method == "POST" and path == "/api/v1/code-scene/assets/validate":
                body, err = self._read_json()
                if err:
                    return self._send_json(*err)
                if "source_code" not in body or "manifest" not in body:
                    return self._send_json(422, {"valid": False, "errors": ["source_code and manifest required"]})
                _, report, _ = validate_cs1(body.get("source_code"), body.get("manifest"), body.get("code_format", "cs1"), mutations=store.mutations)
                return self._send_json(200, report)
            if method == "POST" and path == "/api/v1/code-scene/telemetry":
                body, err = self._read_json(allow_list=True)
                return self._send_json(*(err if err else store.record_behavior(user, body)))
            if method == "POST" and path == "/api/v1/code-scene/ai/generate":
                body, err = self._read_json()
                return self._send_json(*(err if err else store.ai_generate(user, body)))
            if method == "POST" and path == "/api/v1/code-scene/ai/iterate":
                body, err = self._read_json()
                return self._send_json(*(err if err else store.ai_iterate(user, body)))
            if method == "POST" and path == "/api/v1/code-scene/ai/publish":
                body, err = self._read_json()
                return self._send_json(*(err if err else store.ai_publish(user, body)))
            if method == "POST" and path == "/api/v1/code-scene/ai/remix":
                body, err = self._read_json()
                return self._send_json(*(err if err else store.ai_remix(user, body)))
            m_like = re.fullmatch(r"/api/v1/code-scene/assets/([^/]+)/like", path)
            if m_like and method == "POST":
                return self._send_json(*store.toggle_like(user, urllib.parse.unquote(m_like.group(1))))
            m_bmk = re.fullmatch(r"/api/v1/code-scene/assets/([^/]+)/bookmark", path)
            if m_bmk and method == "POST":
                return self._send_json(*store.toggle_bookmark(user, urllib.parse.unquote(m_bmk.group(1))))
            m_cmt = re.fullmatch(r"/api/v1/code-scene/assets/([^/]+)/comments", path)
            if m_cmt:
                sid = urllib.parse.unquote(m_cmt.group(1))
                if method == "GET":
                    return self._send_json(*store.list_comments(sid))
                if method == "POST":
                    body, err = self._read_json()
                    return self._send_json(*(err if err else store.post_comment(user, sid, body)))
            m_fol = re.fullmatch(r"/api/v1/code-scene/creators/(-?\d+)/follow", path)
            if m_fol and method == "POST":
                return self._send_json(*store.toggle_follow(user, int(m_fol.group(1))))
            m_get = re.fullmatch(r"/api/v1/code-scene/assets/(.+)", path)
            if m_get and method == "GET":
                return self._send_json(*store.get_asset(user["user_id"], urllib.parse.unquote(m_get.group(1))))
            self._send_json(404, {"error": "not_found", "message": f"Unknown route {method} {path}"})

        def do_GET(self) -> None:
            self._dispatch("GET")

        def do_POST(self) -> None:
            self._dispatch("POST")

        def do_DELETE(self) -> None:
            self._dispatch("DELETE")

    return _Handler


class StrictContractDoubleServer:
    """Ephemeral SQLite-WAL ThreadingHTTPServer with physical restart support (--self-test ONLY)."""

    def __init__(self, mutations: Optional[Set[str]] = None) -> None:
        self.mutations: Set[str] = set(mutations or set())
        self._tmpdir: Optional[tempfile.TemporaryDirectory[str]] = None
        self._db_path: Optional[Path] = None
        self._store: Optional[DoubleStore] = None
        self._httpd: Optional[ThreadingHTTPServer] = None
        self._thread: Optional[threading.Thread] = None
        self.base_url: str = ""
        self.mode: str = "self_test"
        self.physical_restart_verified: bool = False
        self._restart_seq: int = 0

    def start(self) -> str:
        self._tmpdir = tempfile.TemporaryDirectory(prefix="gpui_cs_e2e_double_")
        self._db_path = Path(self._tmpdir.name) / "code_scene_double.db"
        self._boot_server()
        return self.base_url

    def _boot_server(self) -> None:
        assert self._db_path is not None
        self._store = DoubleStore(self._db_path, mutations=self.mutations)
        handler_cls = _make_handler(self._store)
        self._httpd = ThreadingHTTPServer(("127.0.0.1", 0), handler_cls)
        self._httpd.daemon_threads = True
        port = self._httpd.server_address[1]
        self.base_url = f"http://127.0.0.1:{port}"
        self._thread = threading.Thread(target=self._httpd.serve_forever, kwargs={"poll_interval": 0.05}, daemon=True)
        self._thread.start()

    def _shutdown_server(self) -> None:
        if self._httpd is not None:
            self._httpd.shutdown()
            self._httpd.server_close()
            self._httpd = None
        if self._thread is not None:
            self._thread.join(timeout=3.0)
            self._thread = None
        if self._store is not None:
            self._store.close()
            self._store = None

    def restart(self) -> str:
        self._shutdown_server()
        self._restart_seq += 1
        if "MUT_BROKEN_RESTART_PERSIST" in self.mutations and self._tmpdir is not None:
            self._db_path = Path(self._tmpdir.name) / f"wiped_{self._restart_seq}.db"
        self._boot_server()
        self.physical_restart_verified = True
        return self.base_url

    def stop(self) -> None:
        try:
            self._shutdown_server()
        finally:
            if self._tmpdir is not None:
                self._tmpdir.cleanup()
                self._tmpdir = None
