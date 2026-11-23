#!/usr/bin/env python3
"""Live E2E suite for Telegram @GrokAI & Bot/Mini App alignment and UI/State bug fixes.

Two-phase architecture:
  Phase A (Always runs): Live `gpui_telegram_server` child process (GPUI_OFFLINE_TEST=1)
    - Deterministic seeding: Alice, Bob, @fabushi_bot (external Mini App + MenuButtonWebApp
      + commands + bio), @GrokAI (chat_type=4, is_bot=True), Group "AA 记账分账助手、团队投票助手"
      (3 members: Alice, Bob, @fabushi_bot; empty description/about), and 1v1 Bob chat.
    - Tier 1/2/3/4 server & bridge verification:
      1. Profile/chat metadata isolation (Group has empty about/bio, is_group=True, is_bot=False;
         zero @fabushi_bot bio contamination; @GrokAI resolved as bot).
      2. Mini App HTML iframe wrapper (/miniapps/fabushi_bot/fabushi?init_data=...) +
         <iframe id="tg-app-frame"> + tgWebAppData + /js/telegram-web-app.js +
         postMessage relay including WebMCP gpui_webmcp_invoke / gpui_webmcp_response /
         gpui_webmcp_ready relay, plus launch, HMAC initData validate/tamper, CloudStorage CRUD.
      3. Live WebMCP WS RPC via WsClient: disconnected webmcp_call_tool (fabushi_send_dharma)
         cleanly fails without hanging ("未注册的 WebMCP 工具"), AND connected WS receives
         UpdateEvent::MiniAppWebMcpCommand and resolves MiniAppWebMcpResult (ok + error trace).
      4. Hermes PROVIDER_ERROR & SSE: 503 provider_not_configured + action hermes:open_ai_providers;
         streaming SSE error omits data: [DONE]; DM bot PROVIDER_ERROR attaches inline button
         hermes:open_ai_providers; callback returns ok=True; configuring provider recovers turn
         with clean SSE event: hermes + data: [DONE].
      5. @fabushi_bot DM OpenAI tool_calls (webmcp_call_tool fabushi_send_dharma, disconnected WS):
         verifies server-side StepTrace / StreamConsumerState formatting (`🔧 **执行步骤`,
         zero chat_id=/bot_username= leakage via format_tool_args_preview),
         build_webmcp_failure_guard_notice (`⚠️ **小程序操作未完成**` with fabricated LLM
         success text stripped), and inline_keyboard `gpui-miniapp://fabushi_bot/fabushi` CTA.

  Phase B (macOS GUI — auto-detected TCC or --require-desktop, skipped on --skip-desktop):
    - Compiles MacGuiDriver (swiftc -O in TemporaryDirectory), verifies TCC preflight.
    - Launches isolated `gpui_telegram_desktop --server-url ...` with pre-seeded session.json
      (mode 0o600), executes interactive UI flows + /usr/sbin/screencapture PNG validation:
      1. @fabushi_bot inspector copy -> 01-bot-inspector-copied.png -> 2.8s auto-dismiss ->
         02-bot-copy-autodismiss.png.
      2. Bot -> Group switch isolation & immediate copy clear -> 03-group-inspector-isolated.png.
      3. + compose (Cmd+N) suppression & Esc restore -> 04-plus-compose-no-residual-composer.png,
         05-compose-exit-restored.png.
      4. @GrokAI / composer roundtrip persistence & non-overlapping toast check ->
         06-bot-chat-composer-clear.png.
"""

from __future__ import annotations

import argparse
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
import json
import os
from pathlib import Path
import socket
import subprocess
import sys
import tempfile
import threading
import time
from typing import Any
import urllib.parse

THIS_DIR = Path(__file__).resolve().parent
if str(THIS_DIR) not in sys.path:
    sys.path.insert(0, str(THIS_DIR))

from _e2e_http_ws import HttpClient, WsClient  # noqa: E402

REPO_ROOT = THIS_DIR.parent.parent
GROUP_TITLE = "AA 记账分账助手、团队投票助手"
FABUSHI_BIO = "全球法布施与净心禅室：支持每日经文诵读、禅修计时与功德回向。"
FABUSHI_TOOL_CALL_MARKER = "[E2E_FABUSHI_WEBMCP_TOOL_CALL_GUARD]"
FABUSHI_FABRICATED_SUCCESS_TEXT = "已在「全球法布施」小程序中为您发起《金刚经》法布施。"
PNG_MAGIC = b"\x89PNG\r\n\x1a\n"
MIN_PNG_BYTES = 10_240
WINDOW_CONTENT_WIDTH = 1280.0
WINDOW_CONTENT_HEIGHT = 820.0

# macOS Carbon virtual keycodes matching handle_shortcut_key / handle_non_char_key
VK = {
    "d": 2,
    "e": 14,
    "i": 34,
    "j": 38,
    "k": 40,
    "l": 37,
    "n": 45,
    "p": 35,
    "1": 18,
    "return": 36,
    "escape": 53,
}

SWIFT_DRIVER_SOURCE = r'''
import AppKit
import ApplicationServices
import Carbon
import CoreGraphics
import Foundation

func jsonOut(_ obj: [String: Any]) {
    if let data = try? JSONSerialization.data(withJSONObject: obj, options: []),
       let s = String(data: data, encoding: .utf8) {
        print(s)
    }
}

func findMainWindow(pid: Int32) -> [String: Any]? {
    let list = CGWindowListCopyWindowInfo([.optionAll], kCGNullWindowID) as? [[String: Any]] ?? []
    var best: [String: Any]? = nil
    var bestArea: Double = 0.0
    for w in list {
        guard let ownerPid = w[kCGWindowOwnerPID as String] as? Int32, ownerPid == pid else { continue }
        let layer = (w[kCGWindowLayer as String] as? Int) ?? -1
        if layer != 0 { continue }
        guard let bounds = w[kCGWindowBounds as String] as? [String: Any],
              let width = bounds["Width"] as? Double,
              let height = bounds["Height"] as? Double,
              let x = bounds["X"] as? Double,
              let y = bounds["Y"] as? Double,
              let wid = w[kCGWindowNumber as String] as? Int else { continue }
        if width >= 800.0 && height >= 560.0 {
            let area = width * height
            if area > bestArea {
                bestArea = area
                best = ["wid": wid, "x": x, "y": y, "width": width, "height": height]
            }
        }
    }
    return best
}

func currentInputSourceId() -> String? {
    let src = TISCopyCurrentKeyboardInputSource().takeRetainedValue()
    guard let raw = TISGetInputSourceProperty(src, kTISPropertyInputSourceID) else { return nil }
    return Unmanaged<CFString>.fromOpaque(raw).takeUnretainedValue() as String
}

func selectInputSource(id targetId: String) -> Bool {
    let filter = [kTISPropertyInputSourceID as String: targetId] as CFDictionary
    guard let arr = TISCreateInputSourceList(filter, false)?.takeRetainedValue() as? [TISInputSource],
          let first = arr.first else {
        return false
    }
    return TISSelectInputSource(first) == noErr
}

func asciiKeySpec(_ ch: Character) -> (CGKeyCode, Bool)? {
    switch ch {
    case "a": return (0, false)
    case "s": return (1, false)
    case "d": return (2, false)
    case "f": return (3, false)
    case "h": return (4, false)
    case "g": return (5, false)
    case "z": return (6, false)
    case "x": return (7, false)
    case "c": return (8, false)
    case "v": return (9, false)
    case "b": return (11, false)
    case "q": return (12, false)
    case "w": return (13, false)
    case "e": return (14, false)
    case "r": return (15, false)
    case "y": return (16, false)
    case "t": return (17, false)
    case "1": return (18, false)
    case "2": return (19, false)
    case "3": return (20, false)
    case "4": return (21, false)
    case "6": return (22, false)
    case "5": return (23, false)
    case "=": return (24, false)
    case "9": return (25, false)
    case "7": return (26, false)
    case "-": return (27, false)
    case "8": return (28, false)
    case "0": return (29, false)
    case "]": return (30, false)
    case "o": return (31, false)
    case "u": return (32, false)
    case "[": return (33, false)
    case "i": return (34, false)
    case "p": return (35, false)
    case "l": return (37, false)
    case "j": return (38, false)
    case "'": return (39, false)
    case "k": return (40, false)
    case ";": return (41, false)
    case "\\": return (42, false)
    case ",": return (43, false)
    case "/": return (44, false)
    case "n": return (45, false)
    case "m": return (46, false)
    case ".": return (47, false)
    case " ": return (49, false)
    case "`": return (50, false)
    case "A": return (0, true)
    case "S": return (1, true)
    case "D": return (2, true)
    case "F": return (3, true)
    case "H": return (4, true)
    case "G": return (5, true)
    case "Z": return (6, true)
    case "X": return (7, true)
    case "C": return (8, true)
    case "V": return (9, true)
    case "B": return (11, true)
    case "Q": return (12, true)
    case "W": return (13, true)
    case "E": return (14, true)
    case "R": return (15, true)
    case "Y": return (16, true)
    case "T": return (17, true)
    case "!": return (18, true)
    case "@": return (19, true)
    case "#": return (20, true)
    case "$": return (21, true)
    case "^": return (22, true)
    case "%": return (23, true)
    case "+": return (24, true)
    case "(": return (25, true)
    case "&": return (26, true)
    case "_": return (27, true)
    case "*": return (28, true)
    case ")": return (29, true)
    case "}": return (30, true)
    case "O": return (31, true)
    case "U": return (32, true)
    case "{": return (33, true)
    case "I": return (34, true)
    case "P": return (35, true)
    case "L": return (37, true)
    case "J": return (38, true)
    case "\"": return (39, true)
    case "K": return (40, true)
    case ":": return (41, true)
    case "|": return (42, true)
    case "<": return (43, true)
    case "?": return (44, true)
    case "N": return (45, true)
    case "M": return (46, true)
    case ">": return (47, true)
    case "~": return (50, true)
    default: return nil
    }
}

func postKey(code: CGKeyCode, flags: CGEventFlags) {
    let src = CGEventSource(stateID: .privateState)
    guard let down = CGEvent(keyboardEventSource: src, virtualKey: code, keyDown: true),
          let up = CGEvent(keyboardEventSource: src, virtualKey: code, keyDown: false) else {
        fputs("Failed to create CGEvent for key \(code)\n", stderr)
        exit(2)
    }
    down.flags = flags
    up.flags = flags
    down.post(tap: .cghidEventTap)
    usleep(35_000)
    up.post(tap: .cghidEventTap)
    usleep(35_000)
}

func parseFlags(_ raw: String) -> CGEventFlags {
    var flags: CGEventFlags = []
    for part in raw.split(separator: ",") {
        switch part.lowercased() {
        case "cmd": flags.insert(.maskCommand)
        case "shift": flags.insert(.maskShift)
        case "ctrl": flags.insert(.maskControl)
        case "alt": flags.insert(.maskAlternate)
        default: break
        }
    }
    return flags
}

let args = CommandLine.arguments
if args.count < 2 {
    fputs("Usage: helper <cmd> ...\n", stderr)
    exit(1)
}

switch args[1] {
case "preflight":
    jsonOut([
        "screen_capture": CGPreflightScreenCaptureAccess(),
        "post_event": CGPreflightPostEventAccess(),
        "ax_trusted": AXIsProcessTrusted(),
    ])
case "find-window":
    guard args.count >= 3, let pid = Int32(args[2]) else { exit(1) }
    if let w = findMainWindow(pid: pid) {
        jsonOut(w)
    } else {
        exit(1)
    }
case "activate":
    guard args.count >= 3, let pid = Int32(args[2]) else { exit(1) }
    if let app = NSRunningApplication(processIdentifier: pid) {
        _ = app.activate(options: [.activateAllWindows])
    }
    let axApp = AXUIElementCreateApplication(pid)
    var winsRef: CFTypeRef?
    if AXUIElementCopyAttributeValue(axApp, kAXWindowsAttribute as CFString, &winsRef) == .success,
       let wins = winsRef as? [AXUIElement], let first = wins.first {
        _ = AXUIElementPerformAction(first, kAXRaiseAction as CFString)
        _ = AXUIElementSetAttributeValue(first, kAXMainAttribute as CFString, kCFBooleanTrue)
    }
case "key":
    guard args.count >= 4, let code = UInt16(args[3]) else { exit(1) }
    let flags = parseFlags(args[2])
    postKey(code: code, flags: flags)
case "click":
    guard args.count >= 4, let x = Double(args[2]), let y = Double(args[3]) else { exit(1) }
    let pt = CGPoint(x: x, y: y)
    let src = CGEventSource(stateID: .privateState)
    if let mv = CGEvent(mouseEventSource: src, mouseType: .mouseMoved, mouseCursorPosition: pt, mouseButton: .left) {
        mv.post(tap: .cghidEventTap)
        usleep(25_000)
    }
    guard let dn = CGEvent(mouseEventSource: src, mouseType: .leftMouseDown, mouseCursorPosition: pt, mouseButton: .left),
          let up = CGEvent(mouseEventSource: src, mouseType: .leftMouseUp, mouseCursorPosition: pt, mouseButton: .left) else {
        exit(2)
    }
    dn.post(tap: .cghidEventTap)
    usleep(40_000)
    up.post(tap: .cghidEventTap)
    usleep(40_000)
case "type":
    guard args.count >= 3 else { exit(1) }
    for ch in args[2] {
        if let (code, shift) = asciiKeySpec(ch) {
            postKey(code: code, flags: shift ? .maskShift : [])
        } else {
            var chars = Array(String(ch).utf16)
            let src = CGEventSource(stateID: .privateState)
            if let dn = CGEvent(keyboardEventSource: src, virtualKey: 0, keyDown: true),
               let up = CGEvent(keyboardEventSource: src, virtualKey: 0, keyDown: false) {
                dn.keyboardSetUnicodeString(stringLength: chars.count, unicodeString: &chars)
                up.keyboardSetUnicodeString(stringLength: chars.count, unicodeString: &chars)
                dn.post(tap: .cghidEventTap)
                usleep(25_000)
                up.post(tap: .cghidEventTap)
                usleep(25_000)
            }
        }
    }
case "ensure-ascii":
    let prev = currentInputSourceId() ?? ""
    let ascii = TISCopyCurrentASCIICapableKeyboardInputSource().takeRetainedValue()
    let asciiId: String
    if let raw = TISGetInputSourceProperty(ascii, kTISPropertyInputSourceID) {
        asciiId = Unmanaged<CFString>.fromOpaque(raw).takeUnretainedValue() as String
    } else {
        asciiId = "com.apple.keylayout.ABC"
    }
    let ok = (prev == asciiId) || (TISSelectInputSource(ascii) == noErr)
    jsonOut(["previous": prev, "ascii": asciiId, "switched": ok])
case "select-source":
    guard args.count >= 3 else { exit(1) }
    let ok = selectInputSource(id: args[2])
    jsonOut(["selected": args[2], "ok": ok])
default:
    fputs("Unknown command: \(args[1])\n", stderr)
    exit(1)
}
'''


class MacGuiDriver:
    """Compiled Swift CGEvent + WindowServer helper wrapper."""

    def __init__(self, tmp_dir: Path) -> None:
        src_path = tmp_dir / "gui_driver.swift"
        self.bin_path = tmp_dir / "gui_driver"
        src_path.write_text(SWIFT_DRIVER_SOURCE, encoding="utf-8")
        subprocess.run(
            ["swiftc", "-O", str(src_path), "-o", str(self.bin_path)],
            check=True,
        )

    def _run(self, *args: str) -> str:
        res = subprocess.run(
            [str(self.bin_path), *args],
            check=True,
            text=True,
            capture_output=True,
        )
        return res.stdout.strip()

    def check_permissions(self) -> dict[str, bool]:
        return json.loads(self._run("preflight"))

    def ensure_ascii_input_source(self) -> str:
        info = json.loads(self._run("ensure-ascii"))
        return str(info.get("previous") or "")

    def restore_input_source(self, source_id: str) -> None:
        if source_id:
            self._run("select-source", source_id)

    def wait_for_window(self, pid: int, timeout_secs: float = 20.0) -> dict[str, Any]:
        deadline = time.time() + timeout_secs
        while time.time() < deadline:
            res = subprocess.run(
                [str(self.bin_path), "find-window", str(pid)],
                text=True,
                capture_output=True,
            )
            if res.returncode == 0 and res.stdout.strip():
                return json.loads(res.stdout.strip())
            time.sleep(0.25)
        raise TimeoutError(f"Timed out waiting for main window of pid={pid}")

    def activate(self, pid: int) -> None:
        self._run("activate", str(pid))

    def send_key(self, name: str, flags: str = "none") -> None:
        self._run("key", flags, str(VK[name]))

    def click_viewport(self, win: dict[str, Any], vx: float, vy: float) -> None:
        titlebar_h = max(0.0, float(win["height"]) - WINDOW_CONTENT_HEIGHT)
        sx = float(win["x"]) + vx
        sy = float(win["y"]) + titlebar_h + vy
        self._run("click", f"{sx:.1f}", f"{sy:.1f}")

    def type_text(self, text: str) -> None:
        self._run("type", text)


def capture_and_validate(wid: int, out_path: Path) -> int:
    out_path.parent.mkdir(parents=True, exist_ok=True)
    if out_path.exists():
        out_path.unlink()
    subprocess.run(
        ["/usr/sbin/screencapture", "-x", "-o", "-l", str(wid), str(out_path)],
        check=True,
    )
    if not out_path.is_file():
        raise RuntimeError(f"screencapture did not create file: {out_path}")
    data = out_path.read_bytes()
    if len(data) <= MIN_PNG_BYTES:
        raise RuntimeError(f"PNG too small ({len(data)} bytes <= {MIN_PNG_BYTES}): {out_path}")
    if not data.startswith(PNG_MAGIC):
        raise RuntimeError(f"Invalid PNG header in {out_path}")
    print(f"[CAPTURED] {out_path.name} ({len(data)} bytes)")
    return len(data)


def find_free_port() -> int:
    with socket.socket(socket.AF_INET, socket.SOCK_STREAM) as s:
        s.bind(("127.0.0.1", 0))
        return int(s.getsockname()[1])


def resolve_server_binary(explicit: Path | None = None) -> Path:
    if explicit is not None:
        assert explicit.is_file(), f"Missing --server-bin: {explicit}"
        return explicit
    env_bin = os.environ.get("GPUI_SERVER_BIN")
    if env_bin:
        p = Path(env_bin)
        if p.is_file():
            return p
    target_bin = REPO_ROOT / "target" / "debug" / "gpui_telegram_server"
    if not target_bin.is_file():
        subprocess.run(
            ["cargo", "build", "-p", "gpui_telegram_server", "--bin", "gpui_telegram_server"],
            cwd=str(REPO_ROOT),
            check=True,
        )
        subprocess.run(
            [sys.executable, str(REPO_ROOT / "scripts" / "clean_build_garbage.py")],
            cwd=str(REPO_ROOT),
            check=False,
        )
    assert target_bin.is_file(), f"Missing compiled server binary: {target_bin}"
    return target_bin


def resolve_desktop_binary(explicit: Path | None = None) -> Path | None:
    if explicit is not None:
        return explicit if explicit.is_file() else None
    env_bin = os.environ.get("GPUI_DESKTOP_BIN")
    if env_bin:
        p = Path(env_bin)
        if p.is_file():
            return p
    candidates = [
        REPO_ROOT / "target" / "debug" / "gpui_telegram_desktop",
        REPO_ROOT / "dist" / "GPUI Telegram.app" / "Contents" / "MacOS" / "gpui_telegram_desktop",
        REPO_ROOT / "target" / "ci" / "gpui_telegram_desktop",
    ]
    for c in candidates:
        if c.is_file():
            return c
    return None


def wait_until_healthy(http: HttpClient, proc: subprocess.Popen[bytes], timeout_s: float = 20.0) -> None:
    deadline = time.time() + timeout_s
    last_err = ""
    while time.time() < deadline:
        if proc.poll() is not None:
            out = proc.stdout.read().decode("utf-8", errors="replace") if proc.stdout else ""
            err = proc.stderr.read().decode("utf-8", errors="replace") if proc.stderr else ""
            raise RuntimeError(f"Server exited prematurely ({proc.returncode}):\nSTDOUT:{out}\nSTDERR:{err}")
        try:
            st, _, _ = http.request("GET", "/health")
            if st == 200:
                return
        except Exception as exc:  # noqa: BLE001
            last_err = str(exc)
        time.sleep(0.15)
    raise TimeoutError(f"Server /health did not respond within {timeout_s}s: {last_err}")


def start_fixture_http_server(
    server_base_url: str,
    fixture_state: dict[str, Any] | None = None,
) -> tuple[ThreadingHTTPServer, threading.Thread, str]:
    """Starts a local HTTP fixture serving:
    - GET /external-fabushi-app: external Mini App HTML registering WebMCP tool `fabushi_send_dharma`
    - POST /v1/chat/completions: deterministic OpenAI-compatible mock for provider recovery and
      multi-turn `tool_calls` (`webmcp_call_tool` -> fabricated success) verification
    """
    state_lock = threading.Lock()
    state = fixture_state if fixture_state is not None else {}
    state.setdefault("fabushi_cid", None)
    state.setdefault("tool_calls_turn_requests", 0)
    state.setdefault("tool_calls_tool_results", [])

    def extract_openai_msg_text(msg: dict[str, Any]) -> str:
        content = msg.get("content")
        if isinstance(content, str):
            return content
        if isinstance(content, list):
            parts: list[str] = []
            for item in content:
                if isinstance(item, dict) and isinstance(item.get("text"), str):
                    parts.append(item["text"])
            return "".join(parts)
        return ""

    class FixtureHandler(BaseHTTPRequestHandler):
        def log_message(self, format: str, *args: Any) -> None:  # noqa: A002
            return

        def do_GET(self) -> None:  # noqa: N802
            if self.path.startswith("/external-fabushi-app"):
                html = f"""<!DOCTYPE html>
<html lang="zh-CN">
<head>
  <meta charset="utf-8" />
  <title>全球法布施 External Mini App</title>
  <script src="{server_base_url}/js/telegram-web-app.js"></script>
</head>
<body>
  <h1 id="app-title">全球法布施</h1>
  <script>
    if (window.navigator && window.navigator.modelContext && window.navigator.modelContext.registerTool) {{
      window.navigator.modelContext.registerTool({{
        name: "fabushi_send_dharma",
        description: "Send dharma reading",
        inputSchema: {{ type: "object", properties: {{ sutra: {{ type: "string" }} }} }},
        execute: async function(args) {{
          return {{ ok: true, sutra: (args && args.sutra) || "金刚经" }};
        }}
      }});
    }}
  </script>
</body>
</html>"""
                raw = html.encode("utf-8")
                self.send_response(200)
                self.send_header("Content-Type", "text/html; charset=utf-8")
                self.send_header("Content-Length", str(len(raw)))
                self.end_headers()
                self.wfile.write(raw)
                return
            self.send_response(404)
            self.end_headers()

        def do_POST(self) -> None:  # noqa: N802
            length = int(self.headers.get("Content-Length", "0") or 0)
            raw_req = self.rfile.read(length) if length > 0 else b"{}"
            try:
                req_json = json.loads(raw_req.decode("utf-8"))
            except Exception:
                req_json = {}

            if self.path.rstrip("/") == "/v1/chat/completions":
                is_stream = bool(req_json.get("stream"))
                model = str(req_json.get("model") or "grok-mock-1")
                messages = req_json.get("messages")
                marker_user_idx: int | None = None
                if isinstance(messages, list):
                    for idx, m in enumerate(messages):
                        if (
                            isinstance(m, dict)
                            and m.get("role") == "user"
                            and FABUSHI_TOOL_CALL_MARKER in extract_openai_msg_text(m)
                        ):
                            marker_user_idx = idx

                if marker_user_idx is not None:
                    assert isinstance(messages, list)
                    tail_msgs = [m for m in messages[marker_user_idx + 1 :] if isinstance(m, dict)]
                    tool_msgs = [m for m in tail_msgs if m.get("role") == "tool"]
                    with state_lock:
                        state["tool_calls_turn_requests"] = int(state.get("tool_calls_turn_requests", 0)) + 1
                        target_cid = state.get("fabushi_cid")
                        if tool_msgs:
                            results_list = state.setdefault("tool_calls_tool_results", [])
                            for tm in tool_msgs:
                                results_list.append(extract_openai_msg_text(tm))

                    if not tool_msgs:
                        tool_args_obj: dict[str, Any] = {
                            "tool_name": "fabushi_send_dharma",
                            "bot_username": "fabushi_bot",
                            "short_name": "fabushi",
                            "arguments": {"sutra": "金刚经"},
                        }
                        if target_cid is not None:
                            tool_args_obj["chat_id"] = int(target_cid)
                        tool_args_json = json.dumps(tool_args_obj, ensure_ascii=False)

                        if is_stream:
                            chunks = [
                                {
                                    "id": "chatcmpl-grok-mock-tc",
                                    "object": "chat.completion.chunk",
                                    "model": model,
                                    "choices": [
                                        {
                                            "index": 0,
                                            "delta": {
                                                "role": "assistant",
                                                "tool_calls": [
                                                    {
                                                        "index": 0,
                                                        "id": "call_fabushi_dharma_1",
                                                        "type": "function",
                                                        "function": {
                                                            "name": "webmcp_call_tool",
                                                            "arguments": tool_args_json,
                                                        },
                                                    }
                                                ],
                                            },
                                            "finish_reason": None,
                                        }
                                    ],
                                },
                                {
                                    "id": "chatcmpl-grok-mock-tc",
                                    "object": "chat.completion.chunk",
                                    "model": model,
                                    "choices": [{"index": 0, "delta": {}, "finish_reason": "tool_calls"}],
                                },
                            ]
                            sse_bytes = (
                                "".join(f"data: {json.dumps(c, ensure_ascii=False)}\n\n" for c in chunks)
                                + "data: [DONE]\n\n"
                            ).encode("utf-8")
                            self.send_response(200)
                            self.send_header("Content-Type", "text/event-stream; charset=utf-8")
                            self.send_header("Content-Length", str(len(sse_bytes)))
                            self.end_headers()
                            self.wfile.write(sse_bytes)
                        else:
                            body = json.dumps(
                                {
                                    "id": "chatcmpl-grok-mock-tc",
                                    "object": "chat.completion",
                                    "model": model,
                                    "choices": [
                                        {
                                            "index": 0,
                                            "message": {
                                                "role": "assistant",
                                                "content": None,
                                                "tool_calls": [
                                                    {
                                                        "id": "call_fabushi_dharma_1",
                                                        "type": "function",
                                                        "function": {
                                                            "name": "webmcp_call_tool",
                                                            "arguments": tool_args_json,
                                                        },
                                                    }
                                                ],
                                            },
                                            "finish_reason": "tool_calls",
                                        }
                                    ],
                                    "usage": {"prompt_tokens": 12, "completion_tokens": 18, "total_tokens": 30},
                                },
                                ensure_ascii=False,
                            ).encode("utf-8")
                            self.send_response(200)
                            self.send_header("Content-Type", "application/json")
                            self.send_header("Content-Length", str(len(body)))
                            self.end_headers()
                            self.wfile.write(body)
                        return

                    # Turn 2 (tool result present): emit fabricated LLM success text so server
                    # `build_webmcp_failure_guard_notice` must strip and replace it.
                    if is_stream:
                        chunks = [
                            {
                                "id": "chatcmpl-grok-mock-tc-2",
                                "object": "chat.completion.chunk",
                                "model": model,
                                "choices": [
                                    {
                                        "index": 0,
                                        "delta": {
                                            "role": "assistant",
                                            "content": FABUSHI_FABRICATED_SUCCESS_TEXT,
                                        },
                                        "finish_reason": None,
                                    }
                                ],
                            },
                            {
                                "id": "chatcmpl-grok-mock-tc-2",
                                "object": "chat.completion.chunk",
                                "model": model,
                                "choices": [{"index": 0, "delta": {}, "finish_reason": "stop"}],
                            },
                        ]
                        sse_bytes = (
                            "".join(f"data: {json.dumps(c, ensure_ascii=False)}\n\n" for c in chunks)
                            + "data: [DONE]\n\n"
                        ).encode("utf-8")
                        self.send_response(200)
                        self.send_header("Content-Type", "text/event-stream; charset=utf-8")
                        self.send_header("Content-Length", str(len(sse_bytes)))
                        self.end_headers()
                        self.wfile.write(sse_bytes)
                    else:
                        body = json.dumps(
                            {
                                "id": "chatcmpl-grok-mock-tc-2",
                                "object": "chat.completion",
                                "model": model,
                                "choices": [
                                    {
                                        "index": 0,
                                        "message": {
                                            "role": "assistant",
                                            "content": FABUSHI_FABRICATED_SUCCESS_TEXT,
                                        },
                                        "finish_reason": "stop",
                                    }
                                ],
                                "usage": {"prompt_tokens": 20, "completion_tokens": 16, "total_tokens": 36},
                            },
                            ensure_ascii=False,
                        ).encode("utf-8")
                        self.send_response(200)
                        self.send_header("Content-Type", "application/json")
                        self.send_header("Content-Length", str(len(body)))
                        self.end_headers()
                        self.wfile.write(body)
                    return

                if is_stream:
                    chunks = [
                        {
                            "id": "chatcmpl-grok-mock",
                            "object": "chat.completion.chunk",
                            "model": model,
                            "choices": [{"index": 0, "delta": {"role": "assistant", "content": "Grok mock ok."}, "finish_reason": None}],
                        },
                        {
                            "id": "chatcmpl-grok-mock",
                            "object": "chat.completion.chunk",
                            "model": model,
                            "choices": [{"index": 0, "delta": {}, "finish_reason": "stop"}],
                        },
                    ]
                    sse_bytes = (
                        "".join(f"data: {json.dumps(c)}\n\n" for c in chunks) + "data: [DONE]\n\n"
                    ).encode("utf-8")
                    self.send_response(200)
                    self.send_header("Content-Type", "text/event-stream; charset=utf-8")
                    self.send_header("Content-Length", str(len(sse_bytes)))
                    self.end_headers()
                    self.wfile.write(sse_bytes)
                else:
                    body = json.dumps(
                        {
                            "id": "chatcmpl-grok-mock",
                            "object": "chat.completion",
                            "model": model,
                            "choices": [
                                {
                                    "index": 0,
                                    "message": {"role": "assistant", "content": "Grok mock reply ready."},
                                    "finish_reason": "stop",
                                }
                            ],
                            "usage": {"prompt_tokens": 5, "completion_tokens": 4, "total_tokens": 9},
                        }
                    ).encode("utf-8")
                    self.send_response(200)
                    self.send_header("Content-Type", "application/json")
                    self.send_header("Content-Length", str(len(body)))
                    self.end_headers()
                    self.wfile.write(body)
                return

            self.send_response(404)
            self.end_headers()

    httpd = ThreadingHTTPServer(("127.0.0.1", 0), FixtureHandler)
    port = int(httpd.server_address[1])
    thread = threading.Thread(target=httpd.serve_forever, daemon=True)
    thread.start()
    return httpd, thread, f"http://127.0.0.1:{port}"


def register_user(http: HttpClient, username: str) -> dict[str, Any]:
    st, data, _ = http.json(
        "POST",
        "/api/v1/auth/register",
        {
            "username": username,
            "password": "GrokAlignE2e!2026",
            "first_name": username,
            "last_name": "E2E",
            "device_name": "GrokAlignPyE2E",
        },
    )
    assert st in (200, 201), f"register {username} failed ({st}): {data}"
    uid = int(data["user_id"])
    assert 100_001 <= uid < 500_000, f"human user_id {uid} collided with bot range!"
    return data


def list_messages(http: HttpClient, token: str, chat_id: int) -> list[dict[str, Any]]:
    st, data, _ = http.json("GET", f"/api/v1/chats/{chat_id}/messages?limit=100", token=token)
    assert st == 200, f"list_messages({chat_id}) failed ({st}): {data}"
    return list(data.get("messages", []))


def _extract_msg_text(msg: dict[str, Any]) -> str:
    text = msg.get("text") or ""
    if not text and isinstance(msg.get("content"), dict):
        content = msg["content"]
        if isinstance(content.get("data"), dict):
            text = content["data"].get("text") or ""
        if not text:
            text = content.get("text") or ""
    return str(text)


def wait_for_cloud_message(
    http: HttpClient,
    token: str,
    chat_ids: list[int],
    needle: str,
    timeout_secs: float = 10.0,
) -> tuple[int, int]:
    deadline = time.time() + timeout_secs
    while time.time() < deadline:
        for cid in chat_ids:
            st, body, _ = http.json("GET", f"/api/v1/chats/{cid}/messages?limit=50", token=token)
            if st != 200 or not isinstance(body, dict):
                continue
            msgs = body.get("messages", [])
            exact = [m for m in msgs if _extract_msg_text(m) == needle]
            if len(exact) == 1:
                m = exact[0]
                return cid, int(m.get("message_id", m.get("id", 0)))
            if len(exact) > 1:
                raise AssertionError(f"Duplicate roundtrip messages for {needle!r} in chat {cid}")
        time.sleep(0.3)
    raise AssertionError(f"Roundtrip message {needle!r} not persisted in chats {chat_ids}")


def seed_grok_alignment_world(
    http: HttpClient,
    fixture_base_url: str,
) -> dict[str, Any]:
    """Seeds Alice, Bob, @fabushi_bot, @GrokAI, Group, and 1v1 Bob chat."""
    suffix = str(int(time.time() * 1000))
    alice = register_user(http, f"alice_grok_{suffix}")
    bob = register_user(http, f"bob_grok_{suffix}")

    external_app_url = f"{fixture_base_url}/external-fabushi-app"

    # 1. Seed @fabushi_bot with external Mini App + MenuButtonWebApp + commands + bio
    st, bot_res, _ = http.json(
        "POST",
        "/api/v1/bots",
        {
            "display_name": "全球法布施",
            "username": "fabushi_bot",
            "description": "全球法布施与净心禅室 AI 机器人",
            "about": FABUSHI_BIO,
            "ai_persona": "你是全球法布施禅室助手。",
            "menu_button_text": "全球法布施",
            "menu_button_url": external_app_url,
            "allow_attach_menu": True,
            "attach_peer_types": ["users", "bots", "groups"],
            "commands": [
                {"command": "start", "description": "开始使用法布施"},
                {"command": "dharma", "description": "发送每日经文"},
                {"command": "help", "description": "查看使用帮助"},
            ],
        },
        token=alice["token"],
    )
    assert st == 201, f"create @fabushi_bot failed ({st}): {bot_res}"
    fabushi_token = bot_res["bot_token"]
    fabushi_bot_id = int(bot_res["bot"]["bot_user_id"])
    fabushi_cid = int(bot_res["companion_chat_id"])

    st, cmds_res, _ = http.json(
        "POST",
        f"/bot{fabushi_token}/setMyCommands",
        {
            "commands": [
                {"command": "start", "description": "开始使用法布施"},
                {"command": "dharma", "description": "发送每日经文"},
                {"command": "help", "description": "查看使用帮助"},
            ]
        },
    )
    assert st == 200 and cmds_res.get("ok") is True, f"setMyCommands failed: {cmds_res}"

    st, pub_res, _ = http.json(
        "POST",
        f"/bot{fabushi_token}/publishMiniApp",
        {
            "short_name": "fabushi",
            "title": "全球法布施",
            "description": "全球法布施外部 Mini App",
            "category": "lifestyle",
            "icon_emoji": "🪷",
            "web_url": external_app_url,
            "is_main_app": True,
            "is_attach_enabled": True,
        },
    )
    assert st == 200 and pub_res.get("ok") is True, f"publishMiniApp failed: {pub_res}"

    st, menu_res, _ = http.json(
        "POST",
        f"/bot{fabushi_token}/setChatMenuButton",
        {
            "menu_button": {
                "type": "web_app",
                "text": "全球法布施",
                "web_app": {"url": external_app_url},
            }
        },
    )
    assert st == 200 and menu_res.get("ok") is True, f"setChatMenuButton failed: {menu_res}"

    st, inst_res, _ = http.json(
        "POST",
        "/api/v1/miniapps/fabushi_bot/fabushi/install",
        {"allow_write_access": True},
        token=alice["token"],
    )
    assert st == 200 and inst_res.get("ok") is True, f"install fabushi miniapp failed: {inst_res}"

    # 2. Seed @GrokAI (chat_type == 4, is_bot == True, persona)
    st, grok_res, _ = http.json(
        "POST",
        "/api/v1/contacts/add",
        {
            "first_name": "Grok",
            "username": "GrokAI",
            "is_bot": True,
            "persona": "Official Telegram @GrokAI conversational assistant",
            "system_prompt": "You are @GrokAI, concise and aligned with Telegram Bot API.",
            "welcome_message": "Hello! I am @GrokAI. Ask anything.",
        },
        token=alice["token"],
    )
    assert st in (200, 201), f"add @GrokAI failed ({st}): {grok_res}"
    grok_cid = int(grok_res["chat_id"])
    grok_uid = int(grok_res["user_id"])
    assert int(grok_res["chat_type"]) == 4 and grok_res["is_bot"] is True, grok_res

    # 3. Seed Group `AA 记账分账助手、团队投票助手` (3 members: Alice, Bob, @fabushi_bot; empty description/about)
    st, grp_res, _ = http.json(
        "POST",
        "/api/v1/chats",
        {
            "title": GROUP_TITLE,
            "chat_type": "group",
            "member_usernames": [bob["username"], "fabushi_bot"],
        },
        token=alice["token"],
    )
    assert st == 201, f"create group failed ({st}): {grp_res}"
    group_cid = int(grp_res["chat_id"])

    # 4. Seed 1v1 chat with Bob
    st, bob_dm_res, _ = http.json(
        "POST",
        "/api/v1/chats",
        {
            "peer_user_id": int(bob["user_id"]),
            "chat_type": "direct",
        },
        token=alice["token"],
    )
    assert st in (200, 201), f"create 1v1 Bob DM failed ({st}): {bob_dm_res}"
    bob_dm_cid = int(bob_dm_res["chat_id"])

    # Touch last_message_date_ms deterministically so chat list order is unambiguous:
    # Bob DM (oldest) -> Group -> @GrokAI -> @fabushi_bot (top/newest)
    for cid, text in [
        (bob_dm_cid, "Hey Bob, checking 1v1 isolation."),
        (group_cid, "Welcome to AA 记账分账助手、团队投票助手!"),
        (grok_cid, "/help"),
        (fabushi_cid, "/start"),
    ]:
        st_m, _, _ = http.json(
            "POST",
            f"/api/v1/chats/{cid}/messages",
            {"text": text},
            token=alice["token"],
        )
        assert st_m in (200, 201), f"seed message failed for chat {cid}: {st_m}"
        time.sleep(0.05)

    st, chats_data, _ = http.json("GET", "/api/v1/chats", token=alice["token"])
    assert st == 200 and isinstance(chats_data, dict), chats_data
    chats = chats_data.get("chats", [])

    def row_idx_for(target_cid: int) -> int:
        for idx, ch in enumerate(chats):
            if int(ch.get("chat_id", ch.get("id", -1))) == target_cid:
                return idx
        raise AssertionError(f"Chat {target_cid} not found in Alice /api/v1/chats: {chats}")

    return {
        "suffix": suffix,
        "alice": alice,
        "bob": bob,
        "external_app_url": external_app_url,
        "fabushi_bot_id": fabushi_bot_id,
        "fabushi_token": fabushi_token,
        "fabushi_cid": fabushi_cid,
        "grok_cid": grok_cid,
        "grok_uid": grok_uid,
        "group_cid": group_cid,
        "bob_dm_cid": bob_dm_cid,
        "row_indices": {
            "fabushi": row_idx_for(fabushi_cid),
            "grok": row_idx_for(grok_cid),
            "group": row_idx_for(group_cid),
            "bob": row_idx_for(bob_dm_cid),
        },
        "chat_ids": [int(c.get("chat_id", c.get("id", 0))) for c in chats],
    }


def verify_phase_a_server(
    http: HttpClient,
    ws_base: str,
    fixture_base_url: str,
    seeded: dict[str, Any],
    fixture_state: dict[str, Any] | None = None,
) -> None:
    """Runs all Tier 1-4 live gpui_telegram_server HTTP/WS/MiniApp/WebMCP/Hermes assertions.

    Collects per-section failures so every baseline check runs and reports exact diagnostics
    without weakening any assertion.
    """
    alice = seeded["alice"]
    alice_tok = alice["token"]
    failures: list[str] = []

    def run_check(label: str, fn: Any) -> None:
        try:
            fn()
            print(f"[PASS] {label}")
        except Exception as exc:  # noqa: BLE001
            msg = f"[FAIL] {label}: {exc}"
            print(msg, file=sys.stderr)
            failures.append(msg)

    # ------------------------------------------------------------------
    # Tier 1/2/3 — Check 1: Profile & Chat Metadata Isolation
    # ------------------------------------------------------------------
    def check_profile_and_chat_isolation() -> None:
        # 1a. @fabushi_bot public profile
        st, bot_prof, _ = http.json("GET", "/api/v1/bots/profile/fabushi_bot", token=alice_tok)
        assert st == 200, f"GET /api/v1/bots/profile/fabushi_bot -> {st} {bot_prof}"
        assert bot_prof["bot"]["username"] == "fabushi_bot", bot_prof
        assert FABUSHI_BIO in (bot_prof["bot"].get("about") or ""), bot_prof
        assert bot_prof["bot"]["menu_button_type"] == "web_app", bot_prof
        assert bot_prof["main_app"]["short_name"] == "fabushi", bot_prof
        assert bot_prof["main_app"]["web_url"] == seeded["external_app_url"], bot_prof
        cmd_names = {c["command"] for c in bot_prof["bot"].get("commands", [])}
        assert {"start", "dharma", "help"}.issubset(cmd_names), cmd_names

        # 1b. @GrokAI peer resolve & chat list entry
        st, grok_peer, _ = http.json("GET", "/api/v1/peers/resolve?username=GrokAI", token=alice_tok)
        assert st == 200, f"peers/resolve GrokAI -> {st} {grok_peer}"
        assert grok_peer["kind"] == "user" and grok_peer["user"]["is_bot"] is True, grok_peer
        assert grok_peer["user"]["username"] == "GrokAI", grok_peer

        st, chats_list, _ = http.json("GET", "/api/v1/chats", token=alice_tok)
        assert st == 200
        grok_entry = next(
            (c for c in chats_list["chats"] if int(c["chat_id"]) == seeded["grok_cid"]),
            None,
        )
        assert grok_entry is not None, chats_list
        assert int(grok_entry["chat_type"]) == 4 and grok_entry["is_bot"] is True, grok_entry

        # 1c. Group `AA 记账分账助手、团队投票助手` metadata isolation (zero bot bio contamination)
        gid = seeded["group_cid"]
        st, grp_detail, _ = http.json("GET", f"/api/v1/chats/{gid}", token=alice_tok)
        assert st == 200, f"GET /api/v1/chats/{gid} -> {st} {grp_detail}"
        assert grp_detail["title"] == GROUP_TITLE, grp_detail
        assert grp_detail["is_group"] is True and grp_detail["is_bot"] is False, grp_detail
        assert int(grp_detail["member_count"]) == 3, grp_detail
        assert not (grp_detail.get("about") or "").strip(), f"Group must have empty about: {grp_detail}"
        assert not (grp_detail.get("peer_bio") or "").strip(), f"Group must not have peer_bio: {grp_detail}"
        assert FABUSHI_BIO not in json.dumps(grp_detail, ensure_ascii=False), grp_detail

        # 1d. 1v1 Bob chat isolation
        st, bob_detail, _ = http.json("GET", f"/api/v1/chats/{seeded['bob_dm_cid']}", token=alice_tok)
        assert st == 200, bob_detail
        assert bob_detail["is_group"] is False and bob_detail["is_bot"] is False, bob_detail
        assert bob_detail["peer_username"] == seeded["bob"]["username"], bob_detail

    run_check("PhaseA.ProfileAndChatMetadataIsolation", check_profile_and_chat_isolation)

    # ------------------------------------------------------------------
    # Tier 1/2/3 — Check 2: Mini App HTML Iframe Wrapper & WebMCP Relay
    # ------------------------------------------------------------------
    def check_miniapp_iframe_and_sdk() -> None:
        st, _, sdk_raw = http.request("GET", "/js/telegram-web-app.js")
        assert st == 200, f"GET /js/telegram-web-app.js -> {st}"
        sdk_js = sdk_raw.decode("utf-8", errors="replace")
        for token_str in ("navigator.modelContext", "__GPUI_WEBMCP__", "webmcp_response"):
            assert token_str in sdk_js, f"Missing {token_str!r} in /js/telegram-web-app.js"

        st, _, html_raw = http.request("GET", "/miniapps/fabushi_bot/fabushi?init_data=grok_e2e_init")
        assert st == 200, f"GET /miniapps/fabushi_bot/fabushi -> {st}"
        html_str = html_raw.decode("utf-8", errors="replace")

        required_iframe_markers = [
            '<iframe id="tg-app-frame"',
            f"{seeded['external_app_url']}#tgWebAppData=grok_e2e_init",
            "window.__TG_INIT_DATA__",
            "/js/telegram-web-app.js",
            "frame.contentWindow.postMessage",
            # M3 cross-frame WebMCP postMessage relay markers:
            "gpui_webmcp_invoke",
            "gpui_webmcp_response",
            "gpui_webmcp_ready",
        ]
        missing = [m for m in required_iframe_markers if m not in html_str]
        assert not missing, f"Mini App iframe wrapper missing required bridge markers {missing}"

    run_check("PhaseA.MiniAppIframeWrapperAndWebMcpRelay", check_miniapp_iframe_and_sdk)

    # ------------------------------------------------------------------
    # Tier 1/2 — Check 3: Mini App Launch, HMAC initData & CloudStorage
    # ------------------------------------------------------------------
    def check_miniapp_launch_hmac_storage() -> None:
        st, launch_ctx, _ = http.json(
            "POST",
            "/api/v1/miniapps/fabushi_bot/fabushi/launch",
            {"chat_id": seeded["fabushi_cid"], "launch_mode": "MenuButton", "start_param": "grok_test"},
            token=alice_tok,
        )
        assert st == 200 and launch_ctx["init_data"]["hmac_verified"] is True, launch_ctx
        raw_init = launch_ctx["init_data"]["raw_query_string"]

        st, val_ok, _ = http.json(
            "POST",
            "/api/v1/miniapps/fabushi_bot/fabushi/validate-init-data",
            {"init_data": raw_init},
            token=alice_tok,
        )
        assert st == 200 and val_ok["valid"] is True and val_ok["user_id"] == int(alice["user_id"]), val_ok

        st, val_bad, _ = http.json(
            "POST",
            "/api/v1/miniapps/fabushi_bot/fabushi/validate-init-data",
            {"init_data": f"{raw_init}&tamper=1"},
            token=alice_tok,
        )
        assert st == 200 and val_bad["valid"] is False, val_bad

        st, _, _ = http.json(
            "PUT",
            "/api/v1/miniapps/fabushi_bot/fabushi/storage",
            {"key": "sutra_pref", "value": "金刚经"},
            token=alice_tok,
        )
        assert st == 200
        st, st_get, _ = http.json("GET", "/api/v1/miniapps/fabushi_bot/fabushi/storage", token=alice_tok)
        assert st == 200 and st_get["entries"]["sutra_pref"] == "金刚经", st_get

    run_check("PhaseA.MiniAppLaunchHmacAndStorage", check_miniapp_launch_hmac_storage)

    # ------------------------------------------------------------------
    # Tier 1/2/3/4 — Check 4: WebMCP WS RPC & Disconnected Failure Guard
    # ------------------------------------------------------------------
    def check_webmcp_ws_rpc_and_disconnected_guard() -> None:
        # 4a. Disconnected WS: custom tool `fabushi_send_dharma` MUST fail fast without hanging
        t0 = time.monotonic()
        st, acp_disc, _ = http.json(
            "POST",
            "/api/v1/hermes/acp",
            {
                "jsonrpc": "2.0",
                "id": 101,
                "method": "tools/call",
                "params": {
                    "name": "webmcp_call_tool",
                    "chat_id": seeded["fabushi_cid"],
                    "arguments": {
                        "bot_username": "fabushi_bot",
                        "short_name": "fabushi",
                        "tool_name": "fabushi_send_dharma",
                        "arguments": {"sutra": "心经"},
                    },
                },
            },
            token=alice_tok,
        )
        elapsed = time.monotonic() - t0
        assert elapsed < 5.0, f"Disconnected webmcp_call_tool hung for {elapsed:.2f}s"
        assert st == 200, acp_disc
        tool_res = acp_disc["result"]
        assert tool_res["success"] is False, acp_disc
        assert "未注册的 WebMCP 工具" in (tool_res.get("error") or ""), acp_disc

        # 4b. Connected WS: live WebMCP discover + call_tool (ok) + call_tool (error trace)
        ws = WsClient(f"{ws_base}/api/v1/ws?token={alice_tok}", timeout=10.0)
        try:
            ws.send_json({"type": "SubscribeChat", "payload": {"chat_id": seeded["fabushi_cid"]}})

            def call_acp_rpc(
                rpc_id: int, tool_name: str, arguments: dict[str, Any]
            ) -> tuple[dict[str, Any], threading.Thread]:
                out_holder: dict[str, Any] = {}

                def worker() -> None:
                    st_w, resp_w, _ = http.json(
                        "POST",
                        "/api/v1/hermes/acp",
                        {
                            "jsonrpc": "2.0",
                            "id": rpc_id,
                            "method": "tools/call",
                            "params": {
                                "name": tool_name,
                                "chat_id": seeded["fabushi_cid"],
                                "arguments": arguments,
                            },
                        },
                        token=alice_tok,
                    )
                    out_holder["status"] = st_w
                    out_holder["body"] = resp_w

                t = threading.Thread(target=worker, daemon=True)
                t.start()

                frame = ws.recv_until(
                    lambda m: (
                        m.get("type") == "Update"
                        and isinstance(m.get("payload"), dict)
                        and m["payload"].get("type") == "MiniAppWebMcpCommand"
                    ),
                    timeout=8.0,
                )
                cmd = frame["payload"]["data"]
                out_holder["cmd"] = cmd
                return out_holder, t

            # Step 1: webmcp_discover
            holder1, t1 = call_acp_rpc(
                102,
                "webmcp_discover",
                {"bot_username": "fabushi_bot", "short_name": "fabushi"},
            )
            cmd1 = holder1["cmd"]
            assert cmd1["operation"] == "discover" and cmd1["bot_username"] == "fabushi_bot", cmd1
            ws.send_json(
                {
                    "type": "MiniAppWebMcpResult",
                    "payload": {
                        "request_id": cmd1["request_id"],
                        "ok": True,
                        "result": {"discovered": 1},
                        "error": None,
                        "tools": [
                            {
                                "name": "fabushi_send_dharma",
                                "description": "Send dharma sutra",
                                "input_schema": {"type": "object"},
                                "read_only": False,
                            }
                        ],
                        "state_summary": {"ready": True},
                        "control_mode": "ai_controlling",
                    },
                }
            )
            t1.join(timeout=8.0)
            assert holder1.get("status") == 200, holder1
            disc_res = holder1["body"]["result"]
            assert disc_res["success"] is True, disc_res
            disc_out = json.loads(disc_res["output"])
            assert disc_out["mode"] == "live_webmcp", disc_out
            assert any(tool["name"] == "fabushi_send_dharma" for tool in disc_out["tools"]), disc_out

            # Step 2: webmcp_call_tool (ok: true)
            holder2, t2 = call_acp_rpc(
                103,
                "webmcp_call_tool",
                {
                    "bot_username": "fabushi_bot",
                    "short_name": "fabushi",
                    "tool_name": "fabushi_send_dharma",
                    "arguments": {"sutra": "金刚经"},
                },
            )
            cmd2 = holder2["cmd"]
            assert cmd2["operation"] == "call_tool" and cmd2["tool_name"] == "fabushi_send_dharma", cmd2
            ws.send_json(
                {
                    "type": "MiniAppWebMcpResult",
                    "payload": {
                        "request_id": cmd2["request_id"],
                        "ok": True,
                        "result": {"dispatched": True, "sutra": "金刚经"},
                        "error": None,
                        "tools": [],
                        "state_summary": {"last_sutra": "金刚经"},
                        "control_mode": "ai_controlling",
                    },
                }
            )
            t2.join(timeout=8.0)
            assert holder2.get("status") == 200, holder2
            call_ok_res = holder2["body"]["result"]
            assert call_ok_res["success"] is True, call_ok_res
            call_ok_out = json.loads(call_ok_res["output"])
            assert call_ok_out["mode"] == "live_webmcp" and call_ok_out["result"]["dispatched"] is True, call_ok_out

            # Step 3: webmcp_call_tool (ok: false + page error trace)
            holder3, t3 = call_acp_rpc(
                104,
                "webmcp_call_tool",
                {
                    "bot_username": "fabushi_bot",
                    "short_name": "fabushi",
                    "tool_name": "fabushi_send_dharma",
                    "arguments": {"sutra": "心经"},
                },
            )
            cmd3 = holder3["cmd"]
            ws.send_json(
                {
                    "type": "MiniAppWebMcpResult",
                    "payload": {
                        "request_id": cmd3["request_id"],
                        "ok": False,
                        "result": {},
                        "error": "页面工具执行失败",
                        "tools": [],
                        "state_summary": {
                            "boot_error": "WASM bridge timeout",
                            "page_errors": ["TypeError: cannot invoke"],
                        },
                        "control_mode": "idle",
                    },
                }
            )
            t3.join(timeout=8.0)
            assert holder3.get("status") == 200, holder3
            call_err_res = holder3["body"]["result"]
            assert call_err_res["success"] is False, call_err_res
            err_text = call_err_res.get("error") or ""
            assert "页面工具执行失败" in err_text and "WASM bridge timeout" in err_text, call_err_res
        finally:
            ws.close()

    run_check("PhaseA.WebMcpWsRpcAndDisconnectedFailureGuard", check_webmcp_ws_rpc_and_disconnected_guard)

    # ------------------------------------------------------------------
    # Tier 1/2/3/4 — Check 5: Hermes PROVIDER_ERROR, SSE [DONE] Guard & Callback
    # ------------------------------------------------------------------
    def check_hermes_provider_error_sse_and_callback() -> None:
        # 5a. Disable local fallback so Alice has zero configured providers
        st, prov_off, _ = http.json(
            "PUT",
            "/api/v1/hermes/providers",
            {"provider_id": "local_hermes", "allow_local_fallback": False},
            token=alice_tok,
        )
        assert st == 200 and prov_off.get("allow_local_fallback") is False, prov_off

        # 5b. POST /api/v1/hermes/turns -> 503 provider_not_configured + hermes:open_ai_providers
        st, turn_err, _ = http.json(
            "POST",
            "/api/v1/hermes/turns",
            {"prompt": "Hello @GrokAI without provider"},
            token=alice_tok,
        )
        assert st == 503, f"Expected 503 provider_not_configured, got {st}: {turn_err}"
        assert turn_err.get("error") == "provider_not_configured", turn_err
        assert turn_err.get("needs_provider_selection") is True, turn_err
        assert turn_err.get("action") == "hermes:open_ai_providers", turn_err

        # 5c. Streaming SSE error on /v1/chat/completions must emit PROVIDER_ERROR without data: [DONE]
        st_sse, _, sse_raw = http.request(
            "POST",
            "/v1/chat/completions",
            body=json.dumps(
                {
                    "model": "auto",
                    "stream": True,
                    "messages": [{"role": "user", "content": "Trigger PROVIDER_ERROR SSE"}],
                }
            ).encode("utf-8"),
            headers={"Content-Type": "application/json", "Accept": "text/event-stream"},
            token=alice_tok,
        )
        sse_err_str = sse_raw.decode("utf-8", errors="replace")
        assert st_sse == 200, f"SSE stream status {st_sse}: {sse_err_str}"
        assert "PROVIDER_ERROR" in sse_err_str, f"Missing PROVIDER_ERROR in SSE stream: {sse_err_str}"
        assert "data: [DONE]" not in sse_err_str, f"Errored SSE stream must NOT emit [DONE]: {sse_err_str}"

        # 5d. DM to @GrokAI while unconfigured -> bot emits PROVIDER_ERROR + hermes:open_ai_providers button
        st_dm, _, _ = http.json(
            "POST",
            f"/api/v1/chats/{seeded['grok_cid']}/messages",
            {"text": "Explain Telegram Bot MenuButton"},
            token=alice_tok,
        )
        assert st_dm in (200, 201), st_dm

        err_msg_id: int | None = None
        has_provider_btn = False
        deadline = time.time() + 6.0
        while time.time() < deadline:
            msgs = list_messages(http, alice_tok, seeded["grok_cid"])
            for m in reversed(msgs):
                if "PROVIDER_ERROR" in (m.get("text") or ""):
                    err_msg_id = int(m["id"])
                    for row in m.get("inline_keyboard") or []:
                        for btn in row:
                            if btn.get("callback_data") == "hermes:open_ai_providers":
                                has_provider_btn = True
                    break
            if err_msg_id is not None and has_provider_btn:
                break
            time.sleep(0.1)
        assert err_msg_id is not None and has_provider_btn, (
            f"Expected PROVIDER_ERROR message with hermes:open_ai_providers button in @GrokAI DM"
        )

        # 5e. Callback `hermes:open_ai_providers` returns structured guidance toast
        st_cb, cb_res, _ = http.json(
            "POST",
            f"/api/v1/chats/{seeded['grok_cid']}/callback",
            {"message_id": err_msg_id, "callback_data": "hermes:open_ai_providers"},
            token=alice_tok,
        )
        assert st_cb == 200 and cb_res.get("ok") is True and (cb_res.get("toast") or "").strip(), cb_res

        # 5f. Configure mock OpenAI-compatible provider -> clean turn + SSE emits type:done & data: [DONE]
        st_cfg, cfg_res, _ = http.json(
            "PUT",
            "/api/v1/hermes/providers",
            {
                "provider_id": "mock_grok_e2e",
                "name": "Mock Grok Provider",
                "api_mode": "openai_completions",
                "base_url": f"{fixture_base_url}/v1",
                "default_model": "grok-mock-1",
                "api_key": "sk-mock-grok-e2e",
                "enabled": True,
                "set_active": True,
            },
            token=alice_tok,
        )
        assert st_cfg == 200, f"Configure mock provider failed: {cfg_res}"

        st_ok, turn_ok, _ = http.json(
            "POST",
            "/api/v1/hermes/turns",
            {"prompt": "Confirm recovery", "session_id": f"grok_ok_{seeded['suffix']}"},
            token=alice_tok,
        )
        assert st_ok == 200, f"Turn after provider config failed ({st_ok}): {turn_ok}"
        ok_turn_id = turn_ok["turn_id"]

        st_ev, _, ev_raw = http.request(
            "GET",
            f"/api/v1/hermes/turns/{ok_turn_id}/events?sse=true",
            token=alice_tok,
        )
        ev_sse = ev_raw.decode("utf-8", errors="replace")
        assert st_ev == 200, ev_sse
        assert "event: hermes" in ev_sse and '"type":"done"' in ev_sse, ev_sse
        assert "data: [DONE]" in ev_sse, ev_sse

    run_check("PhaseA.HermesProviderErrorSseAndCallback", check_hermes_provider_error_sse_and_callback)

    # ------------------------------------------------------------------
    # Tier 1/2/3/4 — Check 6: @fabushi_bot DM tool_calls -> StepTrace,
    # StreamConsumerState, format_tool_args_preview & Failure Guard
    # ------------------------------------------------------------------
    def check_fabushi_bot_dm_step_trace_and_failure_guard() -> None:
        assert fixture_state is not None, "fixture_state must be provided for tool_calls verification"
        fixture_state["tool_calls_turn_requests"] = 0
        fixture_state["tool_calls_tool_results"] = []

        fabushi_cid = int(seeded["fabushi_cid"])
        fabushi_bot_id = int(seeded["fabushi_bot_id"])

        st_dm, dm_res, _ = http.json(
            "POST",
            f"/api/v1/chats/{fabushi_cid}/messages",
            {"text": f"{FABUSHI_TOOL_CALL_MARKER} 请通过全球法布施小程序为我发送《金刚经》法布施。"},
            token=alice_tok,
        )
        assert st_dm in (200, 201), f"DM to @fabushi_bot failed ({st_dm}): {dm_res}"
        user_msg_id = int(dm_res.get("message_id") or dm_res.get("id") or 0)

        bot_reply: dict[str, Any] | None = None
        bot_reply_text = ""
        last_msgs: list[dict[str, Any]] = []
        deadline = time.time() + 15.0
        while time.time() < deadline:
            last_msgs = list_messages(http, alice_tok, fabushi_cid)
            for m in reversed(last_msgs):
                mid = int(m.get("id") or m.get("message_id") or 0)
                sender_id = int(m.get("sender_id") or 0)
                txt = _extract_msg_text(m)
                if (
                    mid > user_msg_id
                    and sender_id == fabushi_bot_id
                    and "🔧 **执行步骤" in txt
                    and "⚠️ **小程序操作未完成**" in txt
                ):
                    bot_reply = m
                    bot_reply_text = txt
                    break
            if bot_reply is not None:
                break
            time.sleep(0.15)

        assert bot_reply is not None, (
            f"Timed out waiting for finalized @fabushi_bot StepTrace + failure-guard reply "
            f"(openai_calls={fixture_state.get('tool_calls_turn_requests')}, "
            f"msgs={[(_extract_msg_text(m), m.get('sender_id')) for m in last_msgs[-4:]]})"
        )
        assert int(fixture_state.get("tool_calls_turn_requests", 0)) >= 2, (
            f"Expected >= 2 /v1/chat/completions calls (tool_calls + tool result), got {fixture_state}"
        )
        tool_results = list(fixture_state.get("tool_calls_tool_results") or [])
        assert any("未注册的 WebMCP 工具" in tr for tr in tool_results), (
            f"Expected disconnected webmcp_call_tool error in tool_results, got {tool_results}"
        )

        # Verify StepTrace / StreamConsumerState::render_steps_block formatting
        assert "🔧 **执行步骤" in bot_reply_text, bot_reply_text
        assert "webmcp_call_tool" in bot_reply_text, bot_reply_text
        assert "fabushi_send_dharma" in bot_reply_text, bot_reply_text

        # Verify format_tool_args_preview strips internal IDs (chat_id, bot_username, short_name)
        assert "chat_id=" not in bot_reply_text, f"Internal chat_id leaked in reply: {bot_reply_text}"
        assert "bot_username=" not in bot_reply_text, f"Internal bot_username leaked in reply: {bot_reply_text}"
        assert "short_name=" not in bot_reply_text, f"Internal short_name leaked in reply: {bot_reply_text}"

        # Verify build_webmcp_failure_guard_notice & stripping of fabricated LLM success text
        assert "⚠️ **小程序操作未完成**" in bot_reply_text, bot_reply_text
        assert "已在「全球法布施」小程序中为您发起《金刚经》法布施" not in bot_reply_text, (
            f"Fabricated LLM success text was NOT stripped by failure guard: {bot_reply_text}"
        )

        # Verify companion Mini App inline_keyboard button is attached
        kb_rows = bot_reply.get("inline_keyboard") or []
        webapp_urls = [
            btn.get("web_app_url")
            for row in kb_rows
            if isinstance(row, list)
            for btn in row
            if isinstance(btn, dict) and btn.get("web_app_url")
        ]
        assert "gpui-miniapp://fabushi_bot/fabushi" in webapp_urls, (
            f"Expected gpui-miniapp://fabushi_bot/fabushi in inline_keyboard, got {kb_rows}"
        )

    run_check(
        "PhaseA.FabushiBotDmStepTraceAndFailureGuard",
        check_fabushi_bot_dm_step_trace_and_failure_guard,
    )

    if failures:
        raise AssertionError(
            f"Phase A live server suite had {len(failures)} failing check(s):\n"
            + "\n".join(failures)
        )


def run_phase_b_desktop(
    app_bin: Path,
    server_url: str,
    http: HttpClient,
    seeded: dict[str, Any],
    evidence_dir: Path,
    require_desktop: bool,
) -> None:
    """Phase B: Launches real `gpui_telegram_desktop`, exercises interactive UI flows via MacGuiDriver,
    captures PNG window evidence via /usr/sbin/screencapture, and verifies live composer roundtrip.
    """
    if sys.platform != "darwin":
        msg = "[SKIP] macOS GUI TCC unavailable (non-macOS host)"
        if require_desktop:
            raise RuntimeError(msg)
        print(msg)
        return

    with tempfile.TemporaryDirectory(prefix="gpui_grok_desktop_e2e_") as tmp_str:
        tmp_root = Path(tmp_str)
        driver_dir = tmp_root / "driver"
        auth_cfg = tmp_root / "auth_cfg"
        auth_xdg = tmp_root / "auth_xdg"
        for d in (driver_dir, auth_cfg, auth_xdg):
            d.mkdir(parents=True, exist_ok=True)

        driver = MacGuiDriver(driver_dir)
        perms = driver.check_permissions()
        if not (perms.get("screen_capture") and perms.get("post_event") and perms.get("ax_trusted")):
            msg = f"[SKIP] macOS GUI TCC unavailable: {perms}"
            if require_desktop:
                raise RuntimeError(msg)
            print(msg)
            return

        evidence_dir.mkdir(parents=True, exist_ok=True)
        alice = seeded["alice"]
        session_payload = {
            "server_url": server_url.rstrip("/"),
            "user_id": int(alice["user_id"]),
            "username": alice["username"],
            "token": alice["token"],
            "session_id": alice["session_id"],
            "created_at_ms": int(time.time() * 1000),
        }
        session_path = auth_cfg / "session.json"
        session_path.write_text(json.dumps(session_payload, indent=2), encoding="utf-8")
        os.chmod(session_path, 0o600)

        env = os.environ.copy()
        env["GPUI_TELEGRAM_SERVER_URL"] = server_url
        env["GPUI_TELEGRAM_CONFIG_DIR"] = str(auth_cfg)
        env["XDG_DATA_HOME"] = str(auth_xdg)
        env["GPUI_OFFLINE_TEST"] = "1"
        log_path = tmp_root / "desktop.log"
        log_fh = open(log_path, "w", encoding="utf-8")
        proc: subprocess.Popen[str] | None = None
        prev_input_source = ""

        def row_vy(row_idx: int) -> float:
            # Default Apple/Grok Night sidebar: top_bar(52) + search_bar(39) + py(4) = 95,
            # row height 54 + 1px divider = 55.
            return 95.0 + float(row_idx) * 55.0 + 27.0

        try:
            subprocess.run(["caffeinate", "-u", "-t", "5"], check=False)
            prev_input_source = driver.ensure_ascii_input_source()

            proc = subprocess.Popen(
                [str(app_bin), "--server-url", server_url],
                env=env,
                stdout=log_fh,
                stderr=subprocess.STDOUT,
                text=True,
            )
            win = driver.wait_for_window(proc.pid, timeout_secs=25.0)
            driver.activate(proc.pid)
            time.sleep(2.5)
            win = driver.wait_for_window(proc.pid, timeout_secs=5.0)

            rows = seeded["row_indices"]

            # Flow 1: Select @fabushi_bot -> Cmd+I open ContextInspector -> click copyable username row
            # -> capture copied feedback -> wait 2.8s -> capture auto-dismissed state
            driver.click_viewport(win, 144.0, row_vy(rows["fabushi"]))
            time.sleep(0.8)
            driver.send_key("i", "cmd")
            time.sleep(0.6)
            # Right drawer x=960..1280 (center=1120); first info row (~y=294)
            driver.click_viewport(win, 1120.0, 294.0)
            time.sleep(0.25)
            capture_and_validate(int(win["wid"]), evidence_dir / "01-bot-inspector-copied.png")

            time.sleep(2.8)
            capture_and_validate(int(win["wid"]), evidence_dir / "02-bot-copy-autodismiss.png")

            # Flow 2: Click copy row again -> immediately switch to Group `AA 记账分账助手、团队投票助手`
            driver.click_viewport(win, 1120.0, 294.0)
            time.sleep(0.15)
            driver.click_viewport(win, 144.0, row_vy(rows["group"]))
            time.sleep(0.75)
            capture_and_validate(int(win["wid"]), evidence_dir / "03-group-inspector-isolated.png")

            # Flow 3: Switch back to @fabushi_bot -> close inspector (Esc) -> Cmd+N (+ compose) -> Esc restore
            driver.click_viewport(win, 144.0, row_vy(rows["fabushi"]))
            time.sleep(0.6)
            driver.send_key("escape")
            time.sleep(0.25)
            driver.send_key("n", "cmd")
            time.sleep(0.55)
            capture_and_validate(int(win["wid"]), evidence_dir / "04-plus-compose-no-residual-composer.png")

            driver.send_key("escape")
            time.sleep(0.5)
            capture_and_validate(int(win["wid"]), evidence_dir / "05-compose-exit-restored.png")

            # Flow 4: Select @GrokAI -> click composer -> type & send message -> verify HTTP persistence
            driver.click_viewport(win, 144.0, row_vy(rows["grok"]))
            time.sleep(0.7)
            driver.click_viewport(win, 720.0, 780.0)
            time.sleep(0.3)
            roundtrip_msg = f"Grok Alignment Live UI {seeded['suffix']}"
            driver.type_text(roundtrip_msg)
            time.sleep(0.3)
            driver.send_key("return")

            cid_persisted, mid_persisted = wait_for_cloud_message(
                http, alice["token"], seeded["chat_ids"], roundtrip_msg, timeout_secs=10.0
            )
            time.sleep(0.7)
            capture_and_validate(int(win["wid"]), evidence_dir / "06-bot-chat-composer-clear.png")
            print(
                f"[PASS] PhaseB.DesktopLiveUI: chat_id={cid_persisted} message_id={mid_persisted} "
                f"evidence_dir={evidence_dir}"
            )
        finally:
            if proc is not None and proc.poll() is None:
                proc.terminate()
                try:
                    proc.wait(timeout=5.0)
                except subprocess.TimeoutExpired:
                    proc.kill()
                    proc.wait(timeout=3.0)
            log_fh.close()
            if prev_input_source:
                try:
                    driver.restore_input_source(prev_input_source)
                except Exception as exc:  # noqa: BLE001
                    print(f"[WARN] Failed to restore input source {prev_input_source}: {exc}", file=sys.stderr)


def run_live_suite(
    server_bin: Path | None = None,
    app_bin: Path | None = None,
    skip_desktop: bool = False,
    require_desktop: bool = False,
    evidence_dir: Path | None = None,
) -> None:
    if skip_desktop and require_desktop:
        raise ValueError("Cannot pass both --skip-desktop and --require-desktop")

    resolved_server = resolve_server_binary(server_bin)
    resolved_app = resolve_desktop_binary(app_bin)
    port = find_free_port()
    base_url = f"http://127.0.0.1:{port}"
    ws_base = f"ws://127.0.0.1:{port}"

    fixture_state: dict[str, Any] = {
        "fabushi_cid": None,
        "tool_calls_turn_requests": 0,
        "tool_calls_tool_results": [],
    }
    httpd, http_thread, fixture_base_url = start_fixture_http_server(base_url, fixture_state=fixture_state)

    with tempfile.TemporaryDirectory(prefix="gpui_grok_live_e2e_") as tmpdir:
        tmp_path = Path(tmpdir)
        db_path = tmp_path / "grok_alignment_live.db"
        media_dir = tmp_path / "media_cas"
        media_dir.mkdir(parents=True, exist_ok=True)
        ev_dir = evidence_dir if evidence_dir is not None else (tmp_path / "ui_evidence")

        env = os.environ.copy()
        env["HOST"] = "127.0.0.1"
        env["PORT"] = str(port)
        env["DATABASE_URL"] = f"sqlite://{db_path}"
        env["MEDIA_STORAGE_PATH"] = str(media_dir)
        env["GPUI_OFFLINE_TEST"] = "1"
        env["RUST_LOG"] = "warn"

        proc = subprocess.Popen(
            [str(resolved_server)],
            cwd=str(REPO_ROOT),
            env=env,
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
        )
        try:
            http = HttpClient(base_url, timeout=15.0)
            wait_until_healthy(http, proc)

            seeded = seed_grok_alignment_world(http, fixture_base_url)
            fixture_state["fabushi_cid"] = int(seeded["fabushi_cid"])
            verify_phase_a_server(http, ws_base, fixture_base_url, seeded, fixture_state=fixture_state)

            if skip_desktop:
                print("[SKIP] Phase B desktop verification skipped (--skip-desktop)")
            elif resolved_app is None:
                msg = "[SKIP] Phase B desktop binary not found"
                if require_desktop:
                    raise FileNotFoundError(msg)
                print(msg)
            else:
                run_phase_b_desktop(
                    app_bin=resolved_app,
                    server_url=base_url,
                    http=http,
                    seeded=seeded,
                    evidence_dir=ev_dir,
                    require_desktop=require_desktop,
                )
        finally:
            httpd.shutdown()
            httpd.server_close()
            http_thread.join(timeout=3.0)
            if proc.poll() is None:
                proc.terminate()
                try:
                    proc.wait(timeout=5.0)
                except subprocess.TimeoutExpired:
                    proc.kill()
                    proc.wait(timeout=5.0)


def test_grok_bot_alignment_live_e2e() -> None:
    run_live_suite(skip_desktop=False, require_desktop=False)


def main() -> int:
    parser = argparse.ArgumentParser(
        description="Live E2E suite for @GrokAI & Bot/Mini App alignment and UI/State bug fixes"
    )
    parser.add_argument("--server-bin", type=Path, default=None)
    parser.add_argument("--app-bin", type=Path, default=None)
    parser.add_argument("--skip-desktop", action="store_true")
    parser.add_argument("--require-desktop", action="store_true")
    parser.add_argument("--evidence-dir", type=Path, default=None)
    args = parser.parse_args()
    run_live_suite(
        server_bin=args.server_bin,
        app_bin=args.app_bin,
        skip_desktop=args.skip_desktop,
        require_desktop=args.require_desktop,
        evidence_dir=args.evidence_dir,
    )
    return 0


if __name__ == "__main__":
    sys.exit(main())
