"""3-Layer opaque-box CodeSceneScript ('cs1') corpus & 18-seed blueprints.

Layer A: Live-harvested valid Video/Animation/Game cs1 scripts (with canonical
         fallback templates for self-test / bootstrap).
Layer B: Grammar-independent hostile payloads (empty, exact 65_536B vs 65_537B+,
         syntax/garbage/XSS/SQLi, AST nesting > 64, invalid format & manifest).
Layer C: Grammar-introspected differential hostile splices (benign control check
         followed by infinite loop, recursion > 48, heap bomb > 32_768,
         bytecode bomb > 16_384, draw flood > 2_048, pixel overflow > 76_800,
         zero-draw on_render, and NaN/Inf coordinate expressions).

Zero imports of _contract_double.py. Pure Python 3 stdlib only.
"""

from __future__ import annotations

import copy
import re
from dataclasses import dataclass, field
from typing import Any, Dict, List, Optional, Tuple


CANONICAL_VIDEO_CS1 = """// Canonical Code Video (cs1)
fn on_init() {
    let phase = 0.0;
    let accent = 180.0;
}
fn on_update(dt) {
    phase = phase + dt * 1.25;
}
fn on_render() {
    draw_rect(0.0, 0.0, 360.0, 640.0, 14, 18, 34, 255);
    let y = 240.0 + sin(phase) * 90.0;
    draw_circle(180.0, y, 56.0, 42, 171, 238, 255);
    draw_line(40.0, 520.0, 320.0, 520.0, 255, 196, 61, 220);
}
"""

CANONICAL_ANIMATION_CS1 = """// Canonical Code Animation (cs1)
fn on_init() {
    let t = 0.0;
    let amp = 48.0;
}
fn on_update(dt) {
    t = t + dt * 2.0;
}
fn on_render() {
    draw_rect(0.0, 0.0, 360.0, 640.0, 10, 12, 24, 255);
    let cx = 180.0 + cos(t) * amp;
    let cy = 320.0 + sin(t * 1.5) * amp;
    draw_circle(cx, cy, 42.0, 255, 78, 162, 235);
    draw_circle(360.0 - cx, 640.0 - cy, 28.0, 74, 222, 128, 210);
}
"""

CANONICAL_GAME_CS1 = """// Canonical Playable Code Game (cs1)
fn on_init() {
    let px = 180.0;
    let py = 540.0;
    let score = 0;
}
fn on_update(dt) {
    px = clamp(px + input_x() * dt * 220.0, 32.0, 328.0);
    score = score + 1;
}
fn on_render() {
    draw_rect(0.0, 0.0, 360.0, 640.0, 12, 16, 28, 255);
    draw_rect(px - 44.0, py, 88.0, 16.0, 56, 189, 248, 255);
    draw_circle(180.0, 260.0, 14.0, 250, 204, 21, 255);
    set_hud(score, 0);
}
"""

CANONICAL_PIXEL_SHADER_CS1 = """// Canonical Pixel-Shader Animation (cs1)
fn on_init() {
    let tick = 0.0;
}
fn on_update(dt) {
    tick = tick + dt;
}
fn on_render() {
    pixel_buffer(160, 240);
    set_pixel(80, 120, 64, 224, 208, 255);
    draw_circle(180.0, 320.0, 32.0, 64, 224, 208, 200);
}
"""


def default_manifest(kind: str, seed: int = 42) -> Dict[str, Any]:
    return {
        "kind": kind,
        "width": 360,
        "height": 640,
        "fps": 60 if kind != "video" else 30,
        "duration_sec": 15.0 if kind == "video" else None,
        "seed": seed,
    }


def _make_seed_code(kind: str, variant: int, color_r: int, color_g: int, color_b: int) -> str:
    if kind == "video":
        return (
            f"// Seed Video #{variant}\n"
            f"fn on_init() {{\n    let t = {variant}.0;\n    let speed = 1.{variant};\n}}\n"
            f"fn on_update(dt) {{\n    t = t + dt * speed;\n}}\n"
            f"fn on_render() {{\n"
            f"    draw_rect(0.0, 0.0, 360.0, 640.0, 12, 14, 26, 255);\n"
            f"    let cy = 180.0 + sin(t) * {40 + variant * 10}.0;\n"
            f"    draw_circle(180.0, cy, {30 + variant * 4}.0, {color_r}, {color_g}, {color_b}, 255);\n"
            f"    draw_line(24.0, 560.0, 336.0, 560.0, {color_r}, {color_g}, {color_b}, 220);\n"
            f"}}\n"
        )
    if kind == "animation":
        pix_lines = (
            f"    pixel_buffer(160, 240);\n"
            f"    set_pixel(80, 120, {color_r}, {color_g}, {color_b}, 255);\n"
            if variant == 7
            else ""
        )
        return (
            f"// Seed Animation #{variant}\n"
            f"fn on_init() {{\n    let phase = {variant}.5;\n    let radius = {24 + variant * 5}.0;\n}}\n"
            f"fn on_update(dt) {{\n    phase = phase + dt * 2.{variant};\n}}\n"
            f"fn on_render() {{\n"
            f"{pix_lines}"
            f"    draw_rect(0.0, 0.0, 360.0, 640.0, 8, 10, 20, 255);\n"
            f"    let x = 180.0 + cos(phase) * 90.0;\n"
            f"    let y = 320.0 + sin(phase * 1.3) * 110.0;\n"
            f"    draw_circle(x, y, radius, {color_r}, {color_g}, {color_b}, 240);\n"
            f"}}\n"
        )
    return (
        f"// Seed Game #{variant}\n"
        f"fn on_init() {{\n    let px = 180.0;\n    let score = {variant};\n}}\n"
        f"fn on_update(dt) {{\n    px = clamp(px + input_x() * dt * 240.0, 24.0, 336.0);\n"
        f"    score = score + 1;\n}}\n"
        f"fn on_render() {{\n"
        f"    draw_rect(0.0, 0.0, 360.0, 640.0, 10, 16, 28, 255);\n"
        f"    draw_rect(px - 40.0, 550.0, 80.0, 16.0, {color_r}, {color_g}, {color_b}, 255);\n"
        f"    draw_circle(180.0, 220.0, {14 + variant}.0, 255, 220, 80, 255);\n"
        f"    set_hud(score, 0);\n}}\n"
    )


OFFICIAL_BOTS: List[Tuple[int, str]] = [
    (777000, "fabushi_ai_bot"),
    (777101, "cine_code_bot"),
    (777102, "shader_loom_bot"),
    (777103, "arcade_forge_bot"),
    (777104, "pixel_pulse_bot"),
    (777105, "synth_wave_bot"),
]


def build_seed_blueprints() -> List[Dict[str, Any]]:
    """Build 18 distinct seeded assets (6 Video, 6 Animation, 6 Game)."""
    specs = [
        # 6 Videos
        ("seed_vid_01", "Cyberpunk Countdown", "Neon procedural countdown video", "video", ["neon", "cyberpunk", "video"], 0, 12.0, (42, 171, 238)),
        ("seed_vid_02", "Orbital Solar Story", "Kepler orbit telemetry clip", "video", ["space", "math", "video"], 1, 18.0, (250, 204, 21)),
        ("seed_vid_03", "Synthwave Equalizer", "Audio pulse spectrum video", "video", ["music", "neon", "video"], 2, 15.0, (244, 114, 182)),
        ("seed_vid_04", "Matrix Rain Reel", "Falling glyph cascade clip", "video", ["retro", "cyberpunk", "video"], 3, 20.0, (74, 222, 128)),
        ("seed_vid_05", "Fractal Zoom Journey", "Procedural fractal keyframe video", "video", ["math", "shader", "video"], 4, 24.0, (168, 85, 247)),
        ("seed_vid_06", "Aurora Borealis Loop", "Ribbon wave timelapse video", "video", ["particles", "space", "video"], 5, 16.0, (56, 189, 248)),
        # 6 Animations
        ("seed_ani_01", "Plasma Wave Field", "Pointer-reactive plasma shader", "animation", ["shader", "neon", "animation"], 0, None, (236, 72, 153)),
        ("seed_ani_02", "Boids Swarm Bloom", "60fps flocking particle field", "animation", ["particles", "physics", "animation"], 1, None, (52, 211, 153)),
        ("seed_ani_03", "Lissajous Neon Loom", "Harmonic parametric ribbon art", "animation", ["math", "neon", "animation"], 2, None, (96, 165, 250)),
        ("seed_ani_04", "Cyber Starfield Warp", "Hyperspace depth stars animation", "animation", ["space", "cyberpunk", "animation"], 3, None, (251, 191, 36)),
        ("seed_ani_05", "Fluid Ripple Pond", "Interactive wave ripple shader", "animation", ["shader", "physics", "animation"], 4, None, (45, 212, 191)),
        ("seed_ani_06", "Chromatic Kaleidoscope", "Rotating symmetry mandala art", "animation", ["retro", "particles", "animation"], 5, None, (192, 132, 252)),
        # 6 Games
        ("seed_gam_01", "Neon Brick Breaker", "Playable paddle brick breaker", "game", ["arcade", "neon", "game"], 0, None, (56, 189, 248)),
        ("seed_gam_02", "Starlight Dodge", "Dodge falling meteors arcade", "game", ["space", "arcade", "game"], 1, None, (251, 146, 60)),
        ("seed_gam_03", "Cyber Snake 60fps", "Classic grid snake playable game", "game", ["retro", "arcade", "game"], 2, None, (74, 222, 128)),
        ("seed_gam_04", "Gravity Bounce", "Physics slingshot rebound game", "game", ["physics", "game", "neon"], 3, None, (244, 114, 182)),
        ("seed_gam_05", "Rhythm Tap Pulse", "Beat-sync lane tapper game", "game", ["music", "arcade", "game"], 4, None, (168, 85, 247)),
        ("seed_gam_06", "Laser Pong Arena", "Fast reactive paddle duel", "game", ["retro", "physics", "game"], 5, None, (250, 204, 21)),
    ]
    out: List[Dict[str, Any]] = []
    for idx, (sid, title, desc, kind, tags, bot_idx, dur, rgb) in enumerate(specs, start=1):
        bot_id, bot_user = OFFICIAL_BOTS[bot_idx]
        controls = (
            "Space Play/Pause · Drag Timeline" if kind == "video"
            else ("Move Pointer / Tap to Disturb" if kind == "animation" else "Arrow Keys / Tap to Play · R Restart")
        )
        out.append({
            "scene_id": sid,
            "title": title,
            "description": desc,
            "kind": kind,
            "tags": list(tags),
            "author_user_id": bot_id,
            "author_username": bot_user,
            "creator_bot_id": bot_id,
            "creator_bot_username": bot_user,
            "remix_parent_id": None,
            "source_code": _make_seed_code(kind, idx, rgb[0], rgb[1], rgb[2]),
            "code_format": "cs1",
            "manifest": {
                "kind": kind,
                "width": 360,
                "height": 640,
                "fps": 30 if kind == "video" else 60,
                "duration_sec": dur,
                "seed": 1000 + idx,
            },
            "controls_hint": controls,
        })
    return out


@dataclass
class Cs1Dialect:
    fn_kw: str = "fn"
    var_kw: str = "let"
    semi: str = ";"
    draw_stmt: str = "draw_circle(180.0, 320.0, 24.0, 42, 171, 238, 255);"
    benign_control_verified: bool = False


def _find_on_render_span(source: str) -> Optional[Tuple[int, int]]:
    """Return (body_start, body_end) inside on_render(...) [-> RetType] { ... }.

    Brace scanning skips // line comments, /* ... */ block comments, and
    single/double-quoted string literals with backslash escapes.
    """
    m = re.search(r"\b(?:fn|func|def)\s+on_render\s*\([^)]*\)\s*(?:->\s*[^{]+?)?\{", source)
    if not m:
        return None
    start = m.end()
    depth = 1
    i = start
    n = len(source)
    while i < n and depth > 0:
        ch = source[i]
        if ch == "/" and i + 1 < n and source[i + 1] == "/":
            nl = source.find("\n", i + 2)
            i = n if nl == -1 else nl + 1
            continue
        if ch == "/" and i + 1 < n and source[i + 1] == "*":
            end_blk = source.find("*/", i + 2)
            i = n if end_blk == -1 else end_blk + 2
            continue
        if ch in ('"', "'"):
            quote = ch
            i += 1
            while i < n:
                if source[i] == "\\" and i + 1 < n:
                    i += 2
                    continue
                if source[i] == quote:
                    i += 1
                    break
                i += 1
            continue
        if ch == "{":
            depth += 1
        elif ch == "}":
            depth -= 1
            if depth == 0:
                return (start, i)
        i += 1
    return None


def _strip_line_comment(line: str) -> str:
    """Strip trailing // comment outside quotes."""
    in_q: Optional[str] = None
    i = 0
    n = len(line)
    while i < n:
        ch = line[i]
        if in_q:
            if ch == "\\" and i + 1 < n:
                i += 2
                continue
            if ch == in_q:
                in_q = None
        else:
            if ch in ('"', "'"):
                in_q = ch
            elif ch == "/" and i + 1 < n and line[i + 1] == "/":
                return line[:i].strip()
        i += 1
    return line.strip()


def _is_balanced_expr(stmt: str) -> bool:
    stack: List[str] = []
    pairs = {")": "(", "}": "{", "]": "["}
    in_q: Optional[str] = None
    i = 0
    while i < len(stmt):
        ch = stmt[i]
        if in_q:
            if ch == "\\" and i + 1 < len(stmt):
                i += 2
                continue
            if ch == in_q:
                in_q = None
        else:
            if ch in ('"', "'"):
                in_q = ch
            elif ch in "({[":
                stack.append(ch)
            elif ch in ")}]":
                if not stack or stack[-1] != pairs[ch]:
                    return False
                stack.pop()
        i += 1
    return in_q is None and not stack


def _introspect_dialect(source: str) -> Cs1Dialect:
    d = Cs1Dialect()
    m_fn = re.search(r"\b(fn|func|def)\s+on_render\b", source)
    if m_fn:
        d.fn_kw = m_fn.group(1)
    if re.search(r"\blet\s+mut\s+[A-Za-z_]", source):
        d.var_kw = "let mut"
    elif re.search(r"\blet\s+[A-Za-z_]", source):
        d.var_kw = "let"
    elif re.search(r"\bvar\s+[A-Za-z_]", source):
        d.var_kw = "var"
    else:
        d.var_kw = ""
    code_lines = [_strip_line_comment(ln) for ln in source.splitlines()]
    d.semi = ";" if any(";" in ln for ln in code_lines if ln and not ln.startswith("/*")) else ""
    d.draw_stmt = f"draw_circle(180.0, 320.0, 24.0, 42, 171, 238, 255){d.semi}"
    span = _find_on_render_span(source)
    if span:
        body = source[span[0]:span[1]]
        fallback_pixel_stmt: Optional[str] = None
        for line in body.splitlines():
            raw_s = line.strip()
            if not raw_s or raw_s.startswith(("//", "/*", "*", "#")):
                continue
            cleaned = _strip_line_comment(raw_s)
            if not cleaned:
                continue
            if re.match(r"^(?:let\s+mut|let|var|const|if|else|while|for|loop|return|fn|func|def)\b", cleaned):
                continue
            if not _is_balanced_expr(cleaned) or "(" not in cleaned:
                continue
            if re.search(r"\b(?:draw_\w+|fill_\w+|stroke_\w+|rect|circle|line|plot)\s*\(", cleaned):
                d.draw_stmt = cleaned
                break
            if fallback_pixel_stmt is None and re.search(r"\bset_pixel\s*\(", cleaned):
                fallback_pixel_stmt = cleaned
        else:
            if fallback_pixel_stmt is not None:
                d.draw_stmt = fallback_pixel_stmt
    return d


@dataclass
class Cs1CorpusBuilder:
    """3-Layer adaptive cs1 corpus harvested from a live server."""

    samples: Dict[str, Tuple[str, Dict[str, Any]]] = field(default_factory=dict)
    pixel_sample: Optional[Tuple[str, Dict[str, Any]]] = None
    dialect: Cs1Dialect = field(default_factory=Cs1Dialect)
    harvested_live: bool = False

    def __post_init__(self) -> None:
        if not self.samples:
            self.samples = {
                "video": (CANONICAL_VIDEO_CS1, default_manifest("video", 11)),
                "animation": (CANONICAL_ANIMATION_CS1, default_manifest("animation", 22)),
                "game": (CANONICAL_GAME_CS1, default_manifest("game", 33)),
            }

    def harvest(self, env: Any, user: Any) -> None:
        """Layer A: Harvest real valid scripts from GET /assets & POST /ai/generate."""
        resp, assets = env.list_assets(user, {"limit": 100})
        found_kinds = set()
        shader_candidate: Optional[Tuple[str, Dict[str, Any]]] = None
        if resp.status == 200 and isinstance(assets, list):
            for a in assets:
                kind = a.get("kind")
                src = a.get("source_code")
                man = a.get("manifest")
                if kind in ("video", "animation", "game") and src and isinstance(man, dict):
                    if kind not in found_kinds:
                        self.samples[kind] = (src, copy.deepcopy(man))
                        found_kinds.add(kind)
                    src_l = src.lower()
                    tags_l = [str(t).lower() for t in (a.get("tags") or [])]
                    meta_l = (str(a.get("title") or "") + " " + str(a.get("description") or "")).lower()
                    if "pixel" in src_l:
                        self.pixel_sample = (src, copy.deepcopy(man))
                    elif shader_candidate is None and (
                        "shader" in tags_l or "pixel" in tags_l or "shader" in meta_l or "pixel" in meta_l
                    ):
                        shader_candidate = (src, copy.deepcopy(man))
        if self.pixel_sample is None and shader_candidate is not None:
            self.pixel_sample = shader_candidate
        # Supplement any missing kind via POST /ai/generate
        for kind in ("video", "animation", "game"):
            if kind not in found_kinds:
                rg = env.ai_generate(user, f"Create a neon procedural {kind} scene", kind=kind)
                if rg.status in (200, 201) and isinstance(rg.body, dict):
                    sub = rg.body.get("draft") if isinstance(rg.body.get("draft"), dict) else rg.body
                    src = sub.get("source_code")
                    man = sub.get("manifest")
                    if isinstance(src, str) and src.strip() and isinstance(man, dict):
                        self.samples[kind] = (src, copy.deepcopy(man))
                        found_kinds.add(kind)
        # Supplement pixel/shader sample via POST /ai/generate if still missing
        if self.pixel_sample is None:
            rp = env.ai_generate(user, "Create a pixel shader animation", kind="animation")
            if rp.status in (200, 201) and isinstance(rp.body, dict):
                sub = rp.body.get("draft") if isinstance(rp.body.get("draft"), dict) else rp.body
                src = sub.get("source_code")
                man = sub.get("manifest")
                if isinstance(src, str) and src.strip() and isinstance(man, dict):
                    self.pixel_sample = (src, copy.deepcopy(man))
        if len(found_kinds) == 3:
            self.harvested_live = True
        base_src = self.samples["animation"][0]
        self.dialect = _introspect_dialect(base_src)
        benign_src, benign_man = self.splice_benign_noop("animation")
        vresp = env.validate_script(user, benign_src, benign_man)
        ok, _, _ = env.extract_validation(vresp)
        self.dialect.benign_control_verified = ok

    def valid_script(self, kind: str = "animation") -> Tuple[str, Dict[str, Any]]:
        src, man = self.samples.get(kind, self.samples["animation"])
        return src, copy.deepcopy(man)

    def mutated_valid_script(self, kind: str = "animation", variant: int = 7) -> Tuple[str, Dict[str, Any]]:
        src, man = self.valid_script(kind)
        man["seed"] = int(man.get("seed", 1)) + variant * 17
        # Modify a numeric literal (float OR integer call argument) inside on_render so rendered cover_hash changes
        span = _find_on_render_span(src)
        if span:
            body = src[span[0]:span[1]]
            new_body, count = re.subn(
                r"\b(\d+\.\d+)\b",
                lambda m: f"{float(m.group(1)) + variant * 1.5:.1f}",
                body,
                count=1,
            )
            if count == 0:
                new_body, count = re.subn(
                    r"([(,]\s*)(\d+)\b",
                    lambda m: f"{m.group(1)}{((int(m.group(2)) + variant * 7) % 220) + 12}",
                    body,
                    count=1,
                )
            if count > 0:
                src = src[:span[0]] + new_body + src[span[1]:]
            else:
                src = src[:span[0]] + f"\n    // variant {variant}\n" + body + src[span[1]:]
        return src, man

    def pixel_shader_script(self) -> Tuple[str, Dict[str, Any]]:
        if self.pixel_sample is not None:
            return self.pixel_sample[0], copy.deepcopy(self.pixel_sample[1])
        # Last-resort fallback adapted to introspected dialect
        kw = f"{self.dialect.var_kw} " if self.dialect.var_kw else ""
        fn_kw = self.dialect.fn_kw
        s = self.dialect.semi
        if fn_kw == "fn" and self.dialect.var_kw == "let" and s == ";":
            return CANONICAL_PIXEL_SHADER_CS1, default_manifest("animation", 77)
        adapted = (
            f"// Adapted Pixel-Shader Animation (cs1)\n"
            f"{fn_kw} on_init() {{\n    {kw}tick = 0.0{s}\n}}\n"
            f"{fn_kw} on_update(dt) {{\n    tick = tick + dt{s}\n}}\n"
            f"{fn_kw} on_render() {{\n"
            f"    pixel_buffer(160, 240){s}\n"
            f"    set_pixel(80, 120, 64, 224, 208, 255){s}\n"
            f"    {self.dialect.draw_stmt}\n"
            f"}}\n"
        )
        return adapted, default_manifest("animation", 77)

    def _splice_into_render(self, snippet: str, kind: str = "animation", at_end: bool = False) -> Tuple[str, Dict[str, Any]]:
        src, man = self.valid_script(kind)
        span = _find_on_render_span(src)
        if span:
            pos = span[1] if at_end else span[0]
            injected = src[:pos] + "\n" + snippet + "\n" + src[pos:]
            return injected, man
        return src + "\n" + snippet + "\n", man

    def splice_benign_noop(self, kind: str = "animation") -> Tuple[str, Dict[str, Any]]:
        kw = f"{self.dialect.var_kw} " if self.dialect.var_kw else ""
        s = self.dialect.semi
        return self._splice_into_render(f"    {kw}__e2e_ok = 1 + 1{s}", kind)

    # ---- Layer B: Grammar-Independent Hostile Payloads ----
    def exact_64k_script(self, kind: str = "animation") -> Tuple[str, Dict[str, Any]]:
        src, man = self.valid_script(kind)
        raw = src.encode("utf-8")
        assert len(raw) < 65_536
        pad_len = 65_536 - len(raw)
        padded = src + (" " * (pad_len - 1)) + "\n"
        assert len(padded.encode("utf-8")) == 65_536
        return padded, man

    def oversized_script(self, target_bytes: int = 70_000, kind: str = "animation") -> Tuple[str, Dict[str, Any]]:
        src, man = self.valid_script(kind)
        raw = src.encode("utf-8")
        pad_len = max(1, target_bytes - len(raw))
        padded = src + (" " * (pad_len - 1)) + "\n"
        assert len(padded.encode("utf-8")) == target_bytes
        return padded, man

    def hostile_empty_scripts(self) -> List[str]:
        return ["", "   \n\t\r\n  ", "// nothing here"]

    def hostile_syntax_scripts(self) -> List[str]:
        return [
            "fn on_render( { {{{ ]]] )",
            "\x00\x01\x02@@@###$$$",
            "<script>alert('xss')</script>",
            "'; DROP TABLE code_scenes; --",
        ]

    def hostile_ast_depth_script(self, depth: int = 80) -> Tuple[str, Dict[str, Any]]:
        kw = f"{self.dialect.var_kw} " if self.dialect.var_kw else ""
        s = self.dialect.semi
        expr = ("1+(" * depth) + "1" + (")" * depth)
        return self._splice_into_render(f"    {kw}__ast = {expr}{s}")

    # ---- Layer C: Differential Runtime / Semantic Hostile Splices ----
    def hostile_infinite_loop_script(self) -> Tuple[str, Dict[str, Any]]:
        kw = f"{self.dialect.var_kw} " if self.dialect.var_kw else ""
        s = self.dialect.semi
        snippet = (
            f"    {kw}__i = 0{s}\n"
            f"    while (true) {{\n"
            f"        __i = __i + 1{s}\n"
            f"    }}"
        )
        return self._splice_into_render(snippet)

    def hostile_deep_recursion_script(self) -> Tuple[str, Dict[str, Any]]:
        src, man = self.valid_script("animation")
        fn_kw = self.dialect.fn_kw
        s = self.dialect.semi
        helper = f"{fn_kw} __e2e_bomb(n) {{\n    return __e2e_bomb(n + 1){s}\n}}\n"
        spliced, _ = self._splice_into_render(f"    __e2e_bomb(0){s}")
        return helper + spliced, man

    def hostile_heap_bomb_script(self) -> Tuple[str, Dict[str, Any]]:
        kw = f"{self.dialect.var_kw} " if self.dialect.var_kw else ""
        s = self.dialect.semi
        snippet = (
            f"    {kw}__arr = []{s}\n"
            f"    {kw}__k = 0{s}\n"
            f"    while (__k < 40000) {{\n"
            f"        push(__arr, __k){s}\n"
            f"        __k = __k + 1{s}\n"
            f"    }}"
        )
        return self._splice_into_render(snippet)

    def hostile_bytecode_bomb_script(self) -> Tuple[str, Dict[str, Any]]:
        kw = f"{self.dialect.var_kw} " if self.dialect.var_kw else ""
        s = self.dialect.semi
        if s:
            stmt = f"__b=__b+1{s}"
            chunks = [f"    {kw}__b=0{s}"]
            for _ in range(450):
                chunks.append("    " + (stmt * 10))
            snippet = "\n".join(chunks)
        else:
            # Semicolon-free dialect: strictly newline-separate every statement (<65_536B)
            chunks = [f"{kw}__b=0"] + ["__b=__b+1"] * 4500
            snippet = "\n".join(chunks)
        src, man = self._splice_into_render(snippet)
        assert len(src.encode("utf-8")) < 65_536, f"Bytecode bomb too large: {len(src.encode('utf-8'))}B"
        return src, man

    def hostile_draw_flood_script(self) -> Tuple[str, Dict[str, Any]]:
        kw = f"{self.dialect.var_kw} " if self.dialect.var_kw else ""
        s = self.dialect.semi
        draw_line = self.dialect.draw_stmt.rstrip()
        if s and not draw_line.endswith(";"):
            draw_line = draw_line + s
        loop_snippet = (
            f"    {kw}__d = 0{s}\n"
            f"    while (__d < 2500) {{\n"
            f"        {draw_line}\n"
            f"        __d = __d + 1{s}\n"
            f"    }}"
        )
        # Inject at end of on_render (after all local declarations) so referenced locals are in scope
        return self._splice_into_render(loop_snippet, at_end=True)

    def hostile_pixel_overflow_script(self) -> Tuple[str, Dict[str, Any]]:
        s = self.dialect.semi
        return self._splice_into_render(f"    pixel_buffer(400, 400){s}")

    def hostile_zero_draw_script(self) -> Tuple[str, Dict[str, Any]]:
        src, man = self.valid_script("animation")
        span = _find_on_render_span(src)
        kw = f"{self.dialect.var_kw} " if self.dialect.var_kw else ""
        fn_kw = self.dialect.fn_kw
        s = self.dialect.semi
        if span:
            return src[:span[0]] + f"\n    {kw}__z = 1 + 2{s}\n" + src[span[1]:], man
        return f"{fn_kw} on_init() {{}}\n{fn_kw} on_update(dt) {{}}\n{fn_kw} on_render() {{\n    {kw}__z = 1 + 2{s}\n}}\n", man

    def _adapt_draw_for_nan_inf(self) -> str:
        s = self.dialect.semi
        stmt = self.dialect.draw_stmt.strip().rstrip(";")
        m = re.match(r"^([A-Za-z_]\w*)\s*\((.*)\)$", stmt)
        if m:
            fname, raw_args = m.group(1), m.group(2)
            # Split top-level comma-separated arguments
            args: List[str] = []
            buf: List[str] = []
            depth = 0
            for ch in raw_args:
                if ch in "([{":
                    depth += 1
                elif ch in ")]}":
                    depth -= 1
                if ch == "," and depth == 0:
                    args.append("".join(buf).strip())
                    buf = []
                else:
                    buf.append(ch)
            if buf:
                args.append("".join(buf).strip())
            if len(args) >= 2:
                args[0] = "__nan"
                args[1] = "__inf"
                return f"{fname}({', '.join(args)}){s}"
        return f"draw_circle(__nan, __inf, 20.0, 255, 0, 0, 255){s}"

    def hostile_nan_inf_script(self) -> Tuple[str, Dict[str, Any]]:
        kw = f"{self.dialect.var_kw} " if self.dialect.var_kw else ""
        s = self.dialect.semi
        adapted_call = self._adapt_draw_for_nan_inf()
        snippet = (
            f"    {kw}__nan = 0.0 / 0.0{s}\n"
            f"    {kw}__inf = 1.0 / 0.0{s}\n"
            f"    {adapted_call}"
        )
        return self._splice_into_render(snippet, at_end=True)
