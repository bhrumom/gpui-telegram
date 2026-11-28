"""Closed-form upstream `nightly-labs/openbot@383a35d2` math, color, avatar, and layout oracles.

Pure Python 3.10+ standard library. Used by all 4 E2E tiers to cross-check live runtime
UI probe telemetry without hardcoding mock results.
"""

from __future__ import annotations

import math
import re
import unicodedata
from typing import Any, Dict, List, Optional, Tuple

CANONICAL_DARK_TOKENS: Dict[str, str] = {
    "bg_frame": "#0d0d0d",
    "bg_canvas": "#1a1a1a",
    "bg_sidebar": "#1a1a1a",
    "bg_surface": "#212121",
    "bg_surface_raised": "#242424",
    "bg_control_active": "#2a2a2a",
    "surface_light": "#f0f0f0",
    "text_on_light": "#141414",
    "text_primary": "#ffffff",
    "text_secondary": "#dcdcdc",
    "text_muted": "#979797",
    "text_dim": "#6a6a6a",
    "border": "rgba(255, 255, 255, 0.035)",
    "border_strong": "rgba(255, 255, 255, 0.045)",
    "accent": "#007cf7",
    "accent_text": "#42a0ff",
    "success": "#31cf76",
    "warning": "#ff9412",
    "danger": "#fd2f3b",
    "unread_surface": "#74b9ff",
    "unread_text": "#07131f",
    "badge_new": "#ff1e5e",
}

CANONICAL_LIGHT_TOKENS: Dict[str, str] = {
    "bg_frame": "#e8e8eb",
    "bg_canvas": "#f5f5f6",
    "bg_sidebar": "#f5f5f6",
    "bg_surface": "#ffffff",
    "bg_surface_raised": "#fcfcfc",
    "bg_control_hover": "#f0f0f2",
    "bg_control_active": "#e8e8eb",
    "surface_light": "#1c1c1e",
    "text_on_light": "#ffffff",
    "text_primary": "#141414",
    "text_secondary": "#3a3a3c",
    "text_muted": "#6b6b70",
    "text_dim": "#8e8e93",
    "border": "rgba(20, 20, 20, 0.07)",
    "border_strong": "rgba(20, 20, 20, 0.1)",
    "success": "#1fa85a",
    "warning": "#f08a00",
    "danger": "#e8222d",
    "badge_new": "#e0004a",
}

CHAT_COLOR_PRESETS: Dict[str, str] = {
    "red": "#f75d59",
    "orange": "#eb7007",
    "yellow": "#e5a50a",
    "green": "#07b655",
    "cyan": "#0eafa2",
    "sky": "#06a8d3",
    "blue": "#4699fe",
    "indigo": "#8688ff",
    "violet": "#b474f4",
    "magenta": "#dc63c6",
    "pink": "#f05b93",
}

CANONICAL_CHANNEL_ICONS_20: Tuple[str, ...] = (
    "megaphone",
    "chart",
    "lifebuoy",
    "search",
    "briefcase",
    "code",
    "globe",
    "mail",
    "calendar",
    "gift",
    "palette",
    "brain",
    "book",
    "sparkles",
    "star",
    "zap",
    "shield",
    "store",
    "workflow",
    "card",
)

CANONICAL_AVATAR_HUES_10: Tuple[int, ...] = (
    0, 30, 55, 100, 150, 185, 215, 245, 280, 320
)
DEFAULT_CHANNEL_ICON_HUE: int = 215

SEEDED_AVATAR_SILHOUETTES: Tuple[str, ...] = (
    "cercle", "galet", "squircle", "capsule", "triangle",
    "hexagone", "nuage", "goutte", "carre", "losange", "pentagone",
)
SEEDED_SUPPORTED_SILHOUETTES: Tuple[str, ...] = (
    "cercle", "galet", "squircle", "capsule", "triangle",
    "hexagone", "nuage", "carre", "losange", "pentagone",
)
SUPPORTED_AVATAR_SILHOUETTES: Tuple[str, ...] = SEEDED_SUPPORTED_SILHOUETTES + (
    "fantome", "meduse", "poulpe",
)
AVATAR_EXPRESSIONS: Tuple[str, ...] = (
    "neutre", "attentif", "surpris", "excite", "heureux", "hilare",
    "colere", "triste", "effraye", "mefiant", "confus", "curieux",
    "fier", "timide", "blase", "somnolent",
)
AVATAR_MOODS_PRIORITY: Tuple[str, ...] = (
    "failed", "waiting", "working", "responded", "idle", "connecting", "asleep",
)
TOOL_ACTIVITY_EXPRESSIONS: Dict[str, str] = {
    "searching": "curieux",
    "browsing": "wide",
    "reading": "attentif",
    "running": "excite",
    "editing": "fier",
    "delegating": "wink",
    "working": "attentif",
    "reviewing": "thinking",
}
AUTOMATIC_COLORS: Tuple[str, ...] = (
    "rouge", "orange", "ambre", "vert", "turquoise", "bleu", "violet", "rose",
)
COLOR_BY_ID: Dict[str, str] = {
    "rouge": "#e8483f", "orange": "#f08a24", "ambre": "#f0b429", "vert": "#3ecf8e",
    "turquoise": "#2fbfa0", "bleu": "#3b93f0", "violet": "#8b5cf6", "rose": "#e152b0",
    "encre": "#0a0a0c", "brun": "#8b5e3c", "gris": "#a3a3a3", "creme": "#f1efe9",
}
HUE_COLORS: Dict[int, str] = {
    0: "#e8483f", 30: "#f08a24", 55: "#f0b429", 100: "#8ece34", 150: "#3ecf8e",
    185: "#2fbfa0", 215: "#3b93f0", 245: "#5d67ee", 280: "#8b5cf6", 320: "#e152b0",
}

DEFECT_KEYS_D1_TO_D12: Tuple[str, ...] = (
    "d1_wide_apple_sidebar",
    "d2_duplicate_account_footer",
    "d3_header_overlaps_messages",
    "d4_oversized_pinned_circles",
    "d5_blue_pill_folder_tabs",
    "d6_cjk_preview_clipped",
    "d7_flat_2d_bot_default",
    "d8_blue_bubble_or_raw_json_wall",
    "d9_two_row_composer_chip_strip",
    "d10_telegram_blue_composer",
    "d11_double_right_drawers",
    "d12_legacy_group_stage",
)

APP_THEME_PALETTE_ID_RE = re.compile(r"\A[a-z0-9][a-z0-9-]{0,63}\Z")
APP_THEME_HEX_RE = re.compile(r"\A#[0-9a-fA-F]{6}(?:[0-9a-fA-F]{2})?\Z")
_FONT_FAMILY_FORBIDDEN_CHARS = frozenset({'"', "\\", ";", "{", "}", "<", ">"})
APP_THEME_REQUIRED_KEYS: Tuple[str, ...] = (
    "canvas", "frame", "sidebar", "surface", "text", "textMuted",
    "border", "accent", "accentForeground", "success", "warning", "danger",
)


def _js_round(val: float) -> int:
    return math.floor(val + 0.5)


def hex_to_rgb(hex_str: str) -> Tuple[int, int, int]:
    cleaned = hex_str.strip().lower()
    if not re.fullmatch(r"#[0-9a-f]{6}", cleaned):
        raise ValueError(f"Expected 6-digit hex color, got {hex_str!r}")
    return (int(cleaned[1:3], 16), int(cleaned[3:5], 16), int(cleaned[5:7], 16))


def srgb_to_linear(v: float) -> float:
    return v / 12.92 if v <= 0.04045 else ((v + 0.055) / 1.055) ** 2.4


def linear_to_srgb(v: float) -> float:
    clamped = max(0.0, min(1.0, v))
    if clamped <= 0.0031308:
        return clamped * 12.92
    return 1.055 * (clamped ** (1.0 / 2.4)) - 0.055


def hex_to_oklab(hex_str: str) -> Tuple[float, float, float, float]:
    """Return `(L, a, b, C)` in OKLab space for `#rrggbb`."""
    r_i, g_i, b_i = hex_to_rgb(hex_str)
    r, g, b = srgb_to_linear(r_i / 255.0), srgb_to_linear(g_i / 255.0), srgb_to_linear(b_i / 255.0)
    cbrt = lambda x: math.copysign(abs(x) ** (1.0 / 3.0), x)
    l = cbrt(0.4122214708 * r + 0.5363325363 * g + 0.0514459929 * b)
    m = cbrt(0.2119034982 * r + 0.6806995451 * g + 0.1073969566 * b)
    s = cbrt(0.0883024619 * r + 0.2817188376 * g + 0.6299787005 * b)
    L = 0.2104542553 * l + 0.7936177850 * m - 0.0040720468 * s
    a = 1.9779984951 * l - 2.4285922050 * m + 0.4505937099 * s
    bb = 0.0259040371 * l + 0.7827717662 * m - 0.8086757660 * s
    return (L, a, bb, math.hypot(a, bb))


def oklab_readable_chat_bg(hex_str: str, scheme: str) -> str:
    """Exact `packages/brand/src/chat-colors.ts` OKLab readable chat background clamp."""
    L, a, bb, chroma = hex_to_oklab(hex_str)
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


def relative_luminance(hex_str: str) -> float:
    r_i, g_i, b_i = hex_to_rgb(hex_str)
    r = srgb_to_linear(r_i / 255.0)
    g = srgb_to_linear(g_i / 255.0)
    b = srgb_to_linear(b_i / 255.0)
    return 0.2126 * r + 0.7152 * g + 0.0722 * b


def solid_bubble_takes_dark_text(hex_str: str) -> bool:
    """Exact `own-bubble.ts:43-50` WCAG contrast against white `< 3.0`."""
    lum = relative_luminance(hex_str)
    return (1.05 / (lum + 0.05)) < 3.0


def fnv1a_32_utf16(seed: str) -> int:
    """Exact 32-bit FNV-1a over UTF-16 code units (`bloub-avatar.ts:274`)."""
    h = 0x811C9DC5
    raw = seed.encode("utf-16-le")
    for idx in range(0, len(raw), 2):
        unit = raw[idx] | (raw[idx + 1] << 8)
        h ^= unit
        h = (h * 0x01000193) & 0xFFFFFFFF
    return h


def resolve_plush_profile_oracle(
    seed: str,
    requested_shape: Optional[str] = None,
    hue: Optional[int] = None,
    color: Optional[str] = None,
) -> Dict[str, Any]:
    """Exact `bloubAvatarProfile` resolver with 13 supported silhouettes & `goutte` remap."""
    eff_seed = seed or "openbot"
    style = "flat" if re.search(r":flat:\d+$", eff_seed) else "plush"
    remapped_from_goutte = False
    if requested_shape is not None:
        norm = requested_shape.strip().lower()
        if norm == "goutte":
            remapped_from_goutte = True
            shape = SEEDED_SUPPORTED_SILHOUETTES[
                fnv1a_32_utf16(f"{eff_seed}:shape:replacement") % 10
            ]
        elif norm in SUPPORTED_AVATAR_SILHOUETTES:
            shape = norm
        else:
            shape = SEEDED_SUPPORTED_SILHOUETTES[fnv1a_32_utf16(f"{eff_seed}:shape") % 10]
    else:
        m_named = re.search(r":(fantome|meduse|poulpe):(?:face|flat):\d+$", eff_seed)
        if m_named:
            shape = m_named.group(1)
        else:
            raw_shape = SEEDED_AVATAR_SILHOUETTES[fnv1a_32_utf16(f"{eff_seed}:shape") % 11]
            if raw_shape == "goutte":
                remapped_from_goutte = True
                shape = SEEDED_SUPPORTED_SILHOUETTES[
                    fnv1a_32_utf16(f"{eff_seed}:shape:replacement") % 10
                ]
            else:
                shape = raw_shape
    expression = AVATAR_EXPRESSIONS[fnv1a_32_utf16(f"{eff_seed}:expression") % 16]
    if color is not None:
        resolved_color = color.strip().lower()
    elif hue is not None and hue in HUE_COLORS:
        resolved_color = HUE_COLORS[hue]
    else:
        cid = AUTOMATIC_COLORS[fnv1a_32_utf16(f"{eff_seed}:color") % 8]
        resolved_color = COLOR_BY_ID[cid]
    return {
        "style": style,
        "silhouette": shape,
        "expression": expression,
        "color_hex": resolved_color,
        "remapped_from_goutte": remapped_from_goutte,
    }


def plush_tint_ramp_pixel(hex_str: str, grey_byte: int) -> Tuple[int, int, int, int]:
    """Exact `plush-felt.ts` RGBA tint ramp entry for a single greyscale byte `0..255`."""
    r_i, g_i, b_i = hex_to_rgb(hex_str)
    ink = (srgb_to_linear(r_i / 255.0), srgb_to_linear(g_i / 255.0), srgb_to_linear(b_i / 255.0))
    g_norm = max(0, min(255, grey_byte)) / 255.0
    light = (srgb_to_linear(g_norm) / srgb_to_linear(0.55)) ** 0.6
    sheen = 0.08 * (max(0.0, (g_norm - 0.55) / 0.45) ** 2)
    out: List[int] = []
    for ch in ink:
        tinted = min(1.0, ch * light)
        mixed = tinted + (1.0 - tinted) * sheen
        out.append(max(0, min(255, _js_round(255.0 * linear_to_srgb(mixed)))))
    return (out[0], out[1], out[2], 255)


def plush_tint_ramp_lut(hex_str: str) -> List[Tuple[int, int, int, int]]:
    """Return full 256-entry RGBA tint ramp LUT for `#rrggbb`."""
    return [plush_tint_ramp_pixel(hex_str, i) for i in range(256)]


def sidebar_hysteresis_step(
    current_mode: str,
    drag_width_px: float,
    last_valid_width: float = 280.0,
) -> Tuple[str, float]:
    """Exact OpenBot `110/118px` sidebar hysteresis (`[128, 400]` expanded, `88px` compact)."""
    if math.isnan(drag_width_px) or math.isinf(drag_width_px):
        return (current_mode, last_valid_width)
    if current_mode == "compact":
        if drag_width_px >= 118.0:
            return ("expanded", max(128.0, min(400.0, drag_width_px)))
        return ("compact", 88.0)
    if drag_width_px < 110.0:
        return ("compact", 88.0)
    return ("expanded", max(128.0, min(400.0, drag_width_px)))


def side_panel_clamp_width(
    requested_px: float,
    viewport_w: float,
    rail_w: float,
    sidebar_w: float,
) -> float:
    """Exact SidePanel clamp (`[360, 1600]` & `CONVERSATION_PANEL_MIN = 96`)."""
    host_available = max(0.0, viewport_w - rail_w - sidebar_w)
    effective_max = min(1600.0, max(360.0, host_available - 96.0))
    return max(360.0, min(effective_max, requested_px))


def channel_avatar_cluster_oracle(member_count: int) -> Dict[str, Any]:
    """Conservation-safe `ChannelAvatar` cluster oracle with coupled (faces, overflow, tone) tuples."""
    if member_count <= 0:
        valid_tuples: Tuple[Tuple[int, int, str], ...] = ((0, 0, "none"),)
    elif member_count in (1, 2, 3):
        valid_tuples = ((member_count, 0, "none"),)
    elif member_count == 4:
        valid_tuples = ((4, 0, "none"), (3, 1, "lilac"))
    elif member_count <= 12:
        valid_tuples = ((3, member_count - 3, "lilac"),)
    else:
        valid_tuples = (
            (3, member_count - 3, "amber"),
            (3, member_count - 3, "double"),
            (2, member_count - 2, "amber"),
            (2, member_count - 2, "double"),
        )
    valid_faces = tuple(sorted({t[0] for t in valid_tuples}))
    overflows = [t[1] for t in valid_tuples]
    valid_tones = tuple(dict.fromkeys(t[2] for t in valid_tuples))
    return {
        "valid_faces": valid_faces,
        "overflow_range": (min(overflows), max(overflows)),
        "valid_tones": valid_tones,
        "valid_tuples": valid_tuples,
    }


def is_valid_utf8_and_no_unpaired_surrogates(text: str) -> bool:
    if "\ufffd" in text:
        return False
    for ch in text:
        cp = ord(ch)
        if 0xD800 <= cp <= 0xDFFF:
            return False
    try:
        text.encode("utf-8", errors="strict")
    except UnicodeEncodeError:
        return False
    return True


def utf16_surrogate_safe_truncate(text: str, max_chars: int = 3000) -> str:
    """Truncate text at <= `max_chars` UTF-16 code units without splitting surrogate pairs."""
    units = 0
    out_chars: List[str] = []
    for ch in text:
        cost = 2 if ord(ch) > 0xFFFF else 1
        if units + cost > max_chars:
            break
        out_chars.append(ch)
        units += cost
    return "".join(out_chars)


def validate_app_theme_palette(palette: Dict[str, Any]) -> Tuple[bool, Optional[str]]:
    if not isinstance(palette, dict):
        return (False, "invalid_palette_object")
    pid = palette.get("id")
    if not isinstance(pid, str) or not APP_THEME_PALETTE_ID_RE.fullmatch(pid):
        return (False, "invalid_palette_id")
    if not isinstance(palette.get("name"), str):
        return (False, "invalid_palette_name")
    if palette.get("scheme") not in ("dark", "light"):
        return (False, "invalid_palette_scheme")
    colors = palette.get("colors")
    if not isinstance(colors, dict):
        return (False, "missing_palette_colors")
    for req in APP_THEME_REQUIRED_KEYS:
        val = colors.get(req)
        if not isinstance(val, str) or not APP_THEME_HEX_RE.fullmatch(val):
            return (False, f"invalid_color:{req}")
    return (True, None)


def validate_font_family(font: Any) -> Tuple[bool, Optional[str]]:
    if not isinstance(font, str) or not (1 <= len(font) <= 128) or font.strip() != font:
        return (False, "invalid_font_family")
    if any(
        unicodedata.category(ch) == "Cc" or ch in _FONT_FAMILY_FORBIDDEN_CHARS
        for ch in font
    ):
        return (False, "invalid_font_family")
    return (True, None)


def normalize_channel_icon_and_hue(
    name: Optional[str], hue: Optional[int]
) -> Tuple[Optional[str], int]:
    if not isinstance(name, str):
        return (None, DEFAULT_CHANNEL_ICON_HUE)
    norm = name.strip().lower()
    if norm not in CANONICAL_CHANNEL_ICONS_20:
        return (None, DEFAULT_CHANNEL_ICON_HUE)
    eff_hue = hue if (isinstance(hue, int) and hue in CANONICAL_AVATAR_HUES_10) else DEFAULT_CHANNEL_ICON_HUE
    return (norm, eff_hue)
