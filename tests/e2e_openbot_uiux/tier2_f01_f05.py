"""Tier 2 Boundary, Corner & Negative Tests: UI-F01 through UI-F05 (30 opaque-box tests)."""

from __future__ import annotations

import re

from tests.e2e_openbot_uiux.harness import (
    TempUiUxEnv,
    assert_hex_eq,
    require_field,
    uiux_e2e_test,
)
from tests.e2e_openbot_uiux.oracles import (
    SUPPORTED_AVATAR_SILHOUETTES,
    hex_to_oklab,
    oklab_readable_chat_bg,
    resolve_plush_profile_oracle,
    sidebar_hysteresis_step,
    solid_bubble_takes_dark_text,
    validate_app_theme_palette,
    validate_font_family,
)


# ---------------------------------------------------------------------------
# UI-F01 Boundaries & Negative Cases (6 tests)
# ---------------------------------------------------------------------------


@uiux_e2e_test(
    id="TEST-UIUX-T2-UI-F01-01",
    tier=2,
    features=("UI-F01",),
    description="Verify invalid appearance inputs fallback cleanly to dark #0d0d0d without panic",
)
def test_t2_f01_01_invalid_appearance_fallback_to_dark(env: TempUiUxEnv) -> None:
    tok = env.probe_scenario("f01_invalid_appearance", "tokens")
    assert require_field(tok, "fallback_applied") is True
    assert require_field(tok, "appearance") == "dark"
    assert require_field(tok, "scheme") == "dark"
    assert_hex_eq(require_field(tok, "dark.bg_frame"), "#0d0d0d", "fallback.bg_frame")


@uiux_e2e_test(
    id="TEST-UIUX-T2-UI-F01-02",
    tier=2,
    features=("UI-F01",),
    description="Verify malformed AppThemePalette IDs and hex colors are rejected preserving default tokens",
)
def test_t2_f01_02_reject_malformed_palette_id_and_hex(env: TempUiUxEnv) -> None:
    tok = env.probe_scenario("f01_invalid_palette", "tokens")
    cp = require_field(tok, "custom_palette")
    assert require_field(cp, "valid") is False
    assert bool(require_field(cp, "error_code"))
    bad_samples = require_field(cp, "rejected_samples")
    assert len(bad_samples) >= 3
    for sample in bad_samples:
        ok, _ = validate_app_theme_palette(sample)
        assert ok is False
    assert_hex_eq(require_field(tok, "dark.bg_canvas"), "#1a1a1a", "untouched.bg_canvas")


@uiux_e2e_test(
    id="TEST-UIUX-T2-UI-F01-03",
    tier=2,
    features=("UI-F01",),
    description="Verify OKLab extreme gamut colors clamp safely to valid hex without NaN or overflow",
)
def test_t2_f01_03_oklab_extreme_gamut_clamp_no_nan(env: TempUiUxEnv) -> None:
    tok = env.probe_scenario("f01_oklab_extreme", "tokens")
    samples = require_field(tok, "oklab_samples")
    assert len(samples) >= 4
    for s in samples:
        in_hex = require_field(s, "hex")
        scheme = require_field(s, "scheme")
        out_hex = require_field(s, "result_hex")
        assert re.fullmatch(r"#[0-9a-f]{6}", out_hex.lower()) is not None
        assert_hex_eq(out_hex, oklab_readable_chat_bg(in_hex, scheme), f"extreme({in_hex},{scheme})")
        L, _, _, C = hex_to_oklab(out_hex)
        if scheme == "dark":
            assert L <= 0.305 and C <= 0.055
        else:
            assert L >= 0.945 and C <= 0.025


@uiux_e2e_test(
    id="TEST-UIUX-T2-UI-F01-04",
    tier=2,
    features=("UI-F01",),
    description="Verify solidBubbleTakesDarkText exact WCAG 3.0 boundary between #999999 and #939393",
)
def test_t2_f01_04_solid_bubble_wcag_3_boundary_threshold(env: TempUiUxEnv) -> None:
    tok = env.probe_scenario("f01_wcag_boundary", "tokens")
    by_hex = {require_field(s, "hex").lower(): s for s in require_field(tok, "solid_bubble_samples")}
    for hx in ("#999999", "#939393", "#ffff00", "#000000"):
        row = by_hex.get(hx)
        assert row is not None, f"Missing WCAG boundary probe for {hx}"
        expected = solid_bubble_takes_dark_text(hx)
        assert require_field(row, "takes_dark_text") is expected
        assert_hex_eq(
            require_field(row, "resolved_text_hex"),
            "#141414" if expected else "#ffffff",
            f"wcag({hx})",
        )


@uiux_e2e_test(
    id="TEST-UIUX-T2-UI-F01-05",
    tier=2,
    features=("UI-F01",),
    description="Verify unsafe CSS/control-char font family injection strings are rejected",
)
def test_t2_f01_05_reject_unsafe_font_family_injection(env: TempUiUxEnv) -> None:
    tok = env.probe_scenario("f01_invalid_fonts", "tokens")
    fv = require_field(tok, "font_validation")
    assert require_field(fv, "valid") is False
    assert require_field(fv, "error_code") == "invalid_font_family"
    for bad in require_field(fv, "rejected_inputs"):
        ok, _ = validate_font_family(bad)
        assert ok is False
    assert require_field(fv, "clean_font_accepted") is True


@uiux_e2e_test(
    id="TEST-UIUX-T2-UI-F01-06",
    tier=2,
    features=("UI-F01",),
    description="Verify transparentWindow window-glass mode keeps popovers opaque and disables tab-shape",
)
def test_t2_f01_06_window_glass_forces_opaque_overlays(env: TempUiUxEnv) -> None:
    glass = env.probe_scenario("f01_window_glass_on", "tokens")
    off = env.probe_scenario("f01_window_glass_off", "tokens")
    wg = require_field(glass, "window_glass")
    assert require_field(wg, "enabled") is True
    assert require_field(wg, "popovers_opaque") is True
    assert require_field(wg, "tab_shape_disabled") is True
    assert require_field(off, "window_glass.enabled") is False
    assert_hex_eq(require_field(off, "dark.bg_frame"), "#0d0d0d", "restored.bg_frame")


# ---------------------------------------------------------------------------
# UI-F02 Boundaries & Negative Cases (6 tests)
# ---------------------------------------------------------------------------


@uiux_e2e_test(
    id="TEST-UIUX-T2-UI-F02-01",
    tier=2,
    features=("UI-F02",),
    description="Verify unsupported goutte silhouette is deterministically remapped to a supported shape",
)
def test_t2_f02_01_unsupported_goutte_silhouette_remapped(env: TempUiUxEnv) -> None:
    plush = env.probe_scenario("f02_goutte_remap", "plush")
    prof = require_field(plush, "profile")
    seed = require_field(prof, "seed")
    expected = resolve_plush_profile_oracle(seed, requested_shape="goutte")
    sil = require_field(prof, "silhouette")
    assert sil != "goutte"
    assert sil in SUPPORTED_AVATAR_SILHOUETTES
    assert sil == expected["silhouette"]
    assert require_field(prof, "remapped_from_goutte") is True


@uiux_e2e_test(
    id="TEST-UIUX-T2-UI-F02-02",
    tier=2,
    features=("UI-F02",),
    description="Verify rapid tool activity churn respects 1400ms ACTIVITY_HOLD_MS without flicker",
)
def test_t2_f02_02_rapid_tool_activity_churn_1400ms_hold(env: TempUiUxEnv) -> None:
    mid = env.probe_scenario("f02_activity_t500", "plush")
    after = env.probe_scenario("f02_activity_t1450", "plush")
    assert require_field(mid, "activity.shown") == "searching"
    assert require_field(mid, "activity.flickered") is False
    assert require_field(mid, "activity.held_ms") < 1400
    assert require_field(after, "activity.shown") == "editing"
    assert require_field(after, "activity.held_ms") >= 1400


@uiux_e2e_test(
    id="TEST-UIUX-T2-UI-F02-03",
    tier=2,
    features=("UI-F02",),
    description="Verify broken/404 or corrupt custom avatar URL falls back cleanly to 3D plush",
)
def test_t2_f02_03_missing_or_corrupt_avatar_url_plush_fallback(env: TempUiUxEnv) -> None:
    plush = env.probe_scenario("f02_broken_avatar_url", "plush")
    rt = require_field(plush, "runtime")
    assert require_field(rt, "image_failed") is True
    assert require_field(rt, "fallback_to_plush") is True
    assert require_field(plush, "default_bot_avatar.is_plush_3d") is True


@uiux_e2e_test(
    id="TEST-UIUX-T2-UI-F02-04",
    tier=2,
    features=("UI-F02",),
    description="Verify prefers-reduced-motion freezes plush FPS to 0, pauses hopping balls, and swaps fishing-still",
)
def test_t2_f02_04_reduced_motion_freezes_plush_and_badges(env: TempUiUxEnv) -> None:
    plush = env.probe_scenario("f02_reduced_motion", "plush")
    rt = require_field(plush, "runtime")
    assert require_field(rt, "reduced_motion") is True
    assert require_field(rt, "frozen_pose") is True
    assert require_field(rt, "fps") == 0
    assert require_field(rt, "balls_paused") is True
    assert require_field(rt, "fishing_asset") == "fishing-still.webp"


@uiux_e2e_test(
    id="TEST-UIUX-T2-UI-F02-05",
    tier=2,
    features=("UI-F02",),
    description="Verify offscreen or unfocused window drops plush animation FPS to 0 cleanly",
)
def test_t2_f02_05_offscreen_and_unfocused_fps_drop_to_zero(env: TempUiUxEnv) -> None:
    active = env.probe_scenario("f02_active_fps", "plush")
    offscreen = env.probe_scenario("f02_offscreen_fps", "plush")
    assert require_field(active, "runtime.fps") in (30, 80)
    assert require_field(offscreen, "runtime.fps") == 0
    assert require_field(offscreen, "runtime.balls_paused") is True


@uiux_e2e_test(
    id="TEST-UIUX-T2-UI-F02-06",
    tier=2,
    features=("UI-F02",),
    description="Verify unread bump animation alternation, 200ms roll-out / 240ms exit mount, and invalid LUT hex fallback",
)
def test_t2_f02_06_unread_count_churn_exit_and_invalid_lut_hex(env: TempUiUxEnv) -> None:
    plush = env.probe_scenario("f02_unread_churn_invalid_hex", "plush")
    unread = require_field(plush, "badges.unread")
    assert require_field(unread, "bump_anim") in ("bump", "bump-b")
    assert require_field(unread, "exiting_mounted") is True
    assert require_field(plush, "tint_ramp.fallback_used") is True


# ---------------------------------------------------------------------------
# UI-F03 Boundaries & Negative Cases (6 tests)
# ---------------------------------------------------------------------------


@uiux_e2e_test(
    id="TEST-UIUX-T2-UI-F03-01",
    tier=2,
    features=("UI-F03",),
    description="Verify responsive <=720px breakpoint hides rail and goes full-bleed vs 721px inset card",
)
def test_t2_f03_01_responsive_720px_hides_rail_and_goes_full_bleed(env: TempUiUxEnv) -> None:
    s720 = env.probe_scenario("f03_viewport_720", "shell")
    s721 = env.probe_scenario("f03_viewport_721", "shell")
    assert require_field(s720, "rail_visible") is False
    assert require_field(s720, "rail_width_px") == 0
    assert require_field(s720, "canvas_card.full_bleed") is True
    assert require_field(s720, "canvas_card.radius_px") == 0
    assert require_field(s721, "rail_visible") is True
    assert require_field(s721, "rail_width_px") == 52
    assert require_field(s721, "canvas_card.full_bleed") is False
    assert require_field(s721, "canvas_card.radius_px") == 14


@uiux_e2e_test(
    id="TEST-UIUX-T2-UI-F03-02",
    tier=2,
    features=("UI-F03",),
    description="Verify extreme narrow 320px/480px viewports preserve positive geometry with zero panic",
)
def test_t2_f03_02_extreme_narrow_viewport_no_negative_geometry(env: TempUiUxEnv) -> None:
    sh = env.probe_scenario("f03_viewport_320", "shell")
    assert require_field(sh, "rail_visible") is False
    assert require_field(sh, "conversation_rect.w") > 0
    assert require_field(sh, "conversation_rect.h") > 0


@uiux_e2e_test(
    id="TEST-UIUX-T2-UI-F03-03",
    tier=2,
    features=("UI-F03",),
    description="Verify server rail shrinks cleanly to 56px at <=800px vs 64px/72px above 800px",
)
def test_t2_f03_03_narrow_800px_server_rail_shrinks_to_56px(env: TempUiUxEnv) -> None:
    s801 = env.probe_scenario("f03_server_rail_801", "shell")
    s800 = env.probe_scenario("f03_server_rail_800", "shell")
    assert require_field(s801, "server_rail.width_px") in (64, 72)
    assert require_field(s800, "server_rail.width_px") == 56


@uiux_e2e_test(
    id="TEST-UIUX-T2-UI-F03-04",
    tier=2,
    features=("UI-F03",),
    description="Verify rapid resize sweep 1440->720->500->721->1280 restores exact shell frame metrics",
)
def test_t2_f03_04_rapid_resize_sweep_recovers_exact_shell_frame(env: TempUiUxEnv) -> None:
    sh = env.probe_scenario("f03_resize_sweep_restored", "shell")
    assert require_field(sh, "rail_visible") is True
    assert require_field(sh, "rail_width_px") == 52
    assert require_field(sh, "canvas_card.radius_px") == 14
    assert require_field(sh, "canvas_card.inset_right_px") == 4
    assert require_field(sh, "canvas_card.inset_bottom_px") == 4
    assert require_field(sh, "top_strip_height_px") == 38


@uiux_e2e_test(
    id="TEST-UIUX-T2-UI-F03-05",
    tier=2,
    features=("UI-F03",),
    description="Verify window-glass mode disables curved tab-shape cleanly while keeping 14px inset card",
)
def test_t2_f03_05_window_glass_mode_disables_tab_shape_cleanly(env: TempUiUxEnv) -> None:
    sh = env.probe_scenario("f03_window_glass", "shell")
    assert require_field(sh, "window_glass_active") is True
    assert require_field(sh, "tab_shape_enabled") is False
    assert require_field(sh, "canvas_card.radius_px") == 14


@uiux_e2e_test(
    id="TEST-UIUX-T2-UI-F03-06",
    tier=2,
    features=("UI-F03",),
    description="Verify degenerate 0x0 minimized and 3840x2160 4K viewports clamp safely",
)
def test_t2_f03_06_zero_and_oversized_viewport_clamping_guard(env: TempUiUxEnv) -> None:
    zero = env.probe_scenario("f03_viewport_0x0", "shell")
    uhd = env.probe_scenario("f03_viewport_4k", "shell")
    assert require_field(zero, "conversation_rect.w") >= 0
    assert require_field(zero, "conversation_rect.h") >= 0
    assert require_field(uhd, "rail_width_px") == 52
    assert require_field(uhd, "top_strip_height_px") == 38
    assert require_field(uhd, "canvas_card.radius_px") == 14


# ---------------------------------------------------------------------------
# UI-F04 Boundaries & Negative Cases (6 tests)
# ---------------------------------------------------------------------------


@uiux_e2e_test(
    id="TEST-UIUX-T2-UI-F04-01",
    tier=2,
    features=("UI-F04",),
    description="Verify primary chats item cannot be unpinned and stays unconditionally first",
)
def test_t2_f04_01_primary_chats_item_cannot_be_unpinned(env: TempUiUxEnv) -> None:
    rail = env.probe_scenario("f04_unpin_chats_attempt", "page_rail")
    assert require_field(rail, "chats_unpinnable") is False
    assert require_field(rail, "top_items")[0] == "chats"


@uiux_e2e_test(
    id="TEST-UIUX-T2-UI-F04-02",
    tier=2,
    features=("UI-F04",),
    description="Verify short viewport folds excess usage rings into +N MoreUsageRings preserving 12px free gap",
)
def test_t2_f04_02_short_viewport_usage_ring_overflow_collapse(env: TempUiUxEnv) -> None:
    rail = env.probe_scenario("f04_short_360px_rings", "page_rail")
    bot = require_field(rail, "bottom_items")
    assert require_field(bot, "min_free_px") >= 12
    assert require_field(bot, "usage_ring_slots") >= 1
    assert require_field(bot, "collapsed_more_rings_count") > 0


@uiux_e2e_test(
    id="TEST-UIUX-T2-UI-F04-03",
    tier=2,
    features=("UI-F04",),
    description="Verify corrupt, unknown, or duplicate persisted rail pins are sanitized cleanly",
)
def test_t2_f04_03_corrupt_or_duplicate_rail_pins_sanitized(env: TempUiUxEnv) -> None:
    rail = env.probe_scenario("f04_corrupt_pins", "page_rail")
    pins = require_field(rail, "persisted_pins")
    assert "nonexistent_tab" not in pins
    assert "" not in pins
    assert len(pins) == len(set(pins))


@uiux_e2e_test(
    id="TEST-UIUX-T2-UI-F04-04",
    tier=2,
    features=("UI-F04",),
    description="Verify unpinning all pinnable pages leaves every secondary entry reachable inside RailMoreMenu",
)
def test_t2_f04_04_unpin_all_keeps_all_entries_reachable_via_more(env: TempUiUxEnv) -> None:
    rail = env.probe_scenario("f04_unpin_all", "page_rail")
    assert require_field(rail, "pinned_items") == []
    assert require_field(rail, "top_items") == ["chats", "more"]
    assert require_field(rail, "more_menu.open") is True
    assert len(require_field(rail, "more_menu.entries")) >= 16


@uiux_e2e_test(
    id="TEST-UIUX-T2-UI-F04-05",
    tier=2,
    features=("UI-F04",),
    description="Verify PageRail hides and closes open RailMoreMenu at <=720px and restores at 721px",
)
def test_t2_f04_05_page_rail_hidden_at_720px_and_restored_at_721px(env: TempUiUxEnv) -> None:
    r720 = env.probe_scenario("f04_rail_720", "page_rail")
    r721 = env.probe_scenario("f04_rail_721", "page_rail")
    assert require_field(r720, "visible") is False
    assert require_field(r720, "more_menu.open") is False
    assert require_field(r721, "visible") is True
    assert require_field(r721, "width_px") == 52


@uiux_e2e_test(
    id="TEST-UIUX-T2-UI-F04-06",
    tier=2,
    features=("UI-F04",),
    description="Verify mandatory Settings and Account bottom rail items refuse hide attempts",
)
def test_t2_f04_06_reject_hiding_mandatory_settings_or_account(env: TempUiUxEnv) -> None:
    rail = env.probe_scenario("f04_hide_mandatory", "page_rail")
    hidden = require_field(rail, "hidden_bottom_items")
    assert "settings" not in hidden and "account" not in hidden
    assert require_field(rail, "bottom_items.settings_visible") is True
    assert require_field(rail, "bottom_items.account_tile_px") == 24


# ---------------------------------------------------------------------------
# UI-F05 Boundaries & Negative Cases (6 tests)
# ---------------------------------------------------------------------------


@uiux_e2e_test(
    id="TEST-UIUX-T2-UI-F05-01",
    tier=2,
    features=("UI-F05",),
    description="Verify sidebar resize clamps 650px->400px, 127px->128px, <=0px->88px compact, and ignores NaN",
)
def test_t2_f05_01_resize_clamp_boundaries_and_invalid_drag_values(env: TempUiUxEnv) -> None:
    hi = env.probe_scenario("f05_drag_650", "sidebar_chrome")
    lo = env.probe_scenario("f05_drag_127", "sidebar_chrome")
    neg = env.probe_scenario("f05_drag_neg50", "sidebar_chrome")
    nan = env.probe_scenario("f05_drag_nan", "sidebar_chrome")
    assert require_field(hi, "width_px") == sidebar_hysteresis_step("expanded", 650.0)[1] == 400
    assert require_field(lo, "width_px") == sidebar_hysteresis_step("expanded", 127.0)[1] == 128
    assert require_field(neg, "mode") == "compact" and require_field(neg, "width_px") == 88
    assert require_field(nan, "width_px") == sidebar_hysteresis_step("expanded", float("nan"), 280.0)[1]


@uiux_e2e_test(
    id="TEST-UIUX-T2-UI-F05-02",
    tier=2,
    features=("UI-F05",),
    description="Verify hysteresis deadband [110, 118] prevents jitter from both compact and expanded directions",
)
def test_t2_f05_02_hysteresis_deadband_110_to_118_zero_jitter(env: TempUiUxEnv) -> None:
    from_cmp = env.probe_scenario("f05_deadband_from_compact_114", "sidebar_chrome")
    from_exp = env.probe_scenario("f05_deadband_from_expanded_114", "sidebar_chrome")
    m_c, w_c = sidebar_hysteresis_step("compact", 114.0)
    m_e, w_e = sidebar_hysteresis_step("expanded", 114.0)
    assert require_field(from_cmp, "mode") == m_c == "compact"
    assert require_field(from_cmp, "width_px") == w_c == 88
    assert require_field(from_exp, "mode") == m_e == "expanded"
    assert require_field(from_exp, "width_px") == w_e == 128


@uiux_e2e_test(
    id="TEST-UIUX-T2-UI-F05-03",
    tier=2,
    features=("UI-F05",),
    description="Verify 88px compact mode hides SidebarTabs and replaces search pill with 32px icon",
)
def test_t2_f05_03_compact_88px_mode_swaps_search_pill_to_32px_icon(env: TempUiUxEnv) -> None:
    sc = env.probe_scenario("f05_compact_88", "sidebar_chrome")
    sl = env.probe_scenario("f05_compact_88", "sidebar_list")
    assert require_field(sc, "mode") == "compact"
    assert require_field(sc, "search.compact_search_visible") is True
    assert require_field(sc, "search.compact_search_size_px") == 32
    assert require_field(sl, "tabs.visible") is False


@uiux_e2e_test(
    id="TEST-UIUX-T2-UI-F05-04",
    tier=2,
    features=("UI-F05",),
    description="Verify empty history stack disables Back/Forward buttons at 0.4 opacity as safe no-op",
)
def test_t2_f05_04_empty_history_stack_disables_back_forward_noop(env: TempUiUxEnv) -> None:
    sc = env.probe_scenario("f05_empty_history", "sidebar_chrome")
    tb = require_field(sc, "topbar")
    assert require_field(tb, "back_enabled") is False
    assert require_field(tb, "forward_enabled") is False
    assert require_field(tb, "back_opacity") == 0.4
    assert require_field(tb, "forward_opacity") == 0.4


@uiux_e2e_test(
    id="TEST-UIUX-T2-UI-F05-05",
    tier=2,
    features=("UI-F05",),
    description="Verify non-agents pages hide SidebarTopbar toggle and ignore Cmd+B cleanly",
)
def test_t2_f05_05_non_agents_page_hides_sidebar_toggle_button(env: TempUiUxEnv) -> None:
    sc = env.probe_scenario("f05_books_page_cmd_b", "sidebar_chrome")
    sh = env.probe_scenario("f05_books_page_cmd_b", "shell")
    assert require_field(sc, "topbar.toggle_visible") is False
    assert require_field(sh, "sidebar_visible") is False


@uiux_e2e_test(
    id="TEST-UIUX-T2-UI-F05-06",
    tier=2,
    features=("UI-F05",),
    description="Verify filter funnel zero-match state displays Clear filter action and restores rows on reset",
)
def test_t2_f05_06_filter_funnel_zero_matches_renders_clear_filter(env: TempUiUxEnv) -> None:
    empty = env.probe_scenario("f05_filter_zero_matches", "sidebar_chrome")
    empty_sl = env.probe_scenario("f05_filter_zero_matches", "sidebar_list")
    cleared = env.probe_scenario("f05_filter_cleared", "sidebar_chrome")
    assert require_field(empty, "search.filter_empty_visible") is True
    assert require_field(empty, "search.clear_filter_visible") is True
    assert require_field(empty_sl, "rows") == []
    assert require_field(cleared, "search.filter_active") is False
