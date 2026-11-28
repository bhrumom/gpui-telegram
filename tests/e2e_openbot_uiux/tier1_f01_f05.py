"""Tier 1 Isolated Feature Tests: UI-F01 through UI-F05 (30 opaque-box tests)."""

from __future__ import annotations

from tests.e2e_openbot_uiux.harness import (
    TempUiUxEnv,
    assert_hex_eq,
    assert_token_map,
    assert_within,
    check_workspace_plush_assets,
    require_field,
    uiux_e2e_test,
)
from tests.e2e_openbot_uiux.oracles import (
    CANONICAL_DARK_TOKENS,
    CANONICAL_LIGHT_TOKENS,
    CHAT_COLOR_PRESETS,
    SUPPORTED_AVATAR_SILHOUETTES,
    TOOL_ACTIVITY_EXPRESSIONS,
    hex_to_oklab,
    oklab_readable_chat_bg,
    plush_tint_ramp_lut,
    sidebar_hysteresis_step,
    solid_bubble_takes_dark_text,
    validate_app_theme_palette,
)


# ---------------------------------------------------------------------------
# UI-F01: OpenBot 383a35d2 Dark/Light Tokens & OKLab Chat Colors (6 tests)
# ---------------------------------------------------------------------------


@uiux_e2e_test(
    id="TEST-UIUX-T1-UI-F01-01",
    tier=1,
    features=("UI-F01",),
    description="Verify exact OpenBot 383a35d2 dark mode semantic token values",
)
def test_t1_f01_01_dark_semantic_tokens_parity(env: TempUiUxEnv) -> None:
    tok = env.probe_scenario("f01_dark_tokens", "tokens")
    assert require_field(tok, "scheme") == "dark"
    assert_token_map(require_field(tok, "dark"), CANONICAL_DARK_TOKENS, "tokens.dark")


@uiux_e2e_test(
    id="TEST-UIUX-T1-UI-F01-02",
    tier=1,
    features=("UI-F01",),
    description="Verify exact OpenBot 383a35d2 light mode semantic token values",
)
def test_t1_f01_02_light_semantic_tokens_parity(env: TempUiUxEnv) -> None:
    tok = env.probe_scenario("f01_light_tokens", "tokens")
    assert require_field(tok, "scheme") == "light"
    assert_token_map(require_field(tok, "light"), CANONICAL_LIGHT_TOKENS, "tokens.light")


@uiux_e2e_test(
    id="TEST-UIUX-T1-UI-F01-03",
    tier=1,
    features=("UI-F01",),
    description="Verify OpenBotPalette and TdesktopThemePalette sync eliminates Telegram blue chrome",
)
def test_t1_f01_03_tdesktop_palette_sync_no_legacy_blue(env: TempUiUxEnv) -> None:
    tok = env.probe_scenario("f01_tdesktop_sync", "tokens")
    sync = require_field(tok, "tdesktop_sync")
    assert require_field(sync, "uses_legacy_telegram_blue_chrome") is False
    assert_hex_eq(require_field(sync, "primary_cta_hex"), "#f0f0f0", "dark.primary_cta")
    assert_hex_eq(require_field(sync, "user_bubble_hex"), "#242424", "dark.user_bubble")
    assert require_field(sync, "active_tab_bg_hex").lower() != "#007aff"
    assert set(require_field(sync, "supported_modes")) >= {"Night", "Day", "Tinted", "Classic"}


@uiux_e2e_test(
    id="TEST-UIUX-T1-UI-F01-04",
    tier=1,
    features=("UI-F01",),
    description="Verify 11 chat color presets and OKLab readableChatBackground clamping",
)
def test_t1_f01_04_oklab_chat_presets_and_readable_bg(env: TempUiUxEnv) -> None:
    tok = env.probe_scenario("f01_oklab_presets", "tokens")
    presets = require_field(tok, "chat_color_presets")
    assert len(presets) == 11
    for name, expected_hex in CHAT_COLOR_PRESETS.items():
        assert_hex_eq(presets.get(name), expected_hex, f"preset.{name}")
    samples = require_field(tok, "oklab_samples")
    assert len(samples) >= 2
    for sample in samples:
        in_hex = require_field(sample, "hex")
        scheme = require_field(sample, "scheme")
        out_hex = require_field(sample, "result_hex")
        expected = oklab_readable_chat_bg(in_hex, scheme)
        assert_hex_eq(out_hex, expected, f"oklab({in_hex},{scheme})")
        L, _, _, C = hex_to_oklab(out_hex)
        if scheme == "dark":
            assert L <= 0.305 and C <= 0.055
        else:
            assert L >= 0.945 and C <= 0.025


@uiux_e2e_test(
    id="TEST-UIUX-T1-UI-F01-05",
    tier=1,
    features=("UI-F01",),
    description="Verify solidBubbleTakesDarkText WCAG contrast rule across bright vs dark bubbles",
)
def test_t1_f01_05_solid_bubble_takes_dark_text_contrast(env: TempUiUxEnv) -> None:
    tok = env.probe_scenario("f01_solid_bubble_contrast", "tokens")
    samples = require_field(tok, "solid_bubble_samples")
    assert len(samples) >= 5
    by_hex = {require_field(s, "hex").lower(): s for s in samples}
    for test_hex in ("#e5a50a", "#07b655", "#06a8d3", "#b474f4", "#242424"):
        entry = by_hex.get(test_hex)
        assert entry is not None, f"Missing solid bubble sample for {test_hex}"
        expected_dark = solid_bubble_takes_dark_text(test_hex)
        assert require_field(entry, "takes_dark_text") is expected_dark
        assert_hex_eq(
            require_field(entry, "resolved_text_hex"),
            "#141414" if expected_dark else "#ffffff",
            f"text({test_hex})",
        )


@uiux_e2e_test(
    id="TEST-UIUX-T1-UI-F01-06",
    tier=1,
    features=("UI-F01",),
    description="Verify custom 12-color AppThemePalette derivation and clean reset to default",
)
def test_t1_f01_06_custom_12color_palette_derivation_and_reset(env: TempUiUxEnv) -> None:
    tok = env.probe_scenario("f01_custom_palette_valid", "tokens")
    cp = require_field(tok, "custom_palette")
    assert require_field(cp, "valid") is True
    payload = require_field(cp, "palette")
    ok_oracle, err = validate_app_theme_palette(payload)
    assert ok_oracle is True and err is None
    assert require_field(cp, "reset_restores_defaults") is True
    assert_hex_eq(require_field(cp, "reset_bg_frame"), "#0d0d0d", "reset.bg_frame")


# ---------------------------------------------------------------------------
# UI-F02: 3D Felt PlushBot Default Avatars & Felt Status Badges (6 tests)
# ---------------------------------------------------------------------------


@uiux_e2e_test(
    id="TEST-UIUX-T1-UI-F02-01",
    tier=1,
    features=("UI-F02",),
    description="Verify 73 .webp + 1 .webm plush assets on disk and loaded cleanly by runtime probe",
)
def test_t1_f02_01_workspace_73_webp_plush_assets_integrity(env: TempUiUxEnv) -> None:
    ws = check_workspace_plush_assets()
    assert ws["webp_count"] == 73 and ws["webm_count"] == 1
    assert ws["has_boule"] is True and ws["has_fishing_still"] is True
    plush = env.probe_scenario("f02_assets", "plush")
    assets = require_field(plush, "assets")
    assert require_field(assets, "webp_count") == 73
    assert require_field(assets, "webm_count") == 1
    assert require_field(assets, "all_valid_webp_headers") is True


@uiux_e2e_test(
    id="TEST-UIUX-T1-UI-F02-02",
    tier=1,
    features=("UI-F02",),
    description="Verify Fabushi bots without custom photo default to 3D PlushFeltRenderer not 2D blobs",
)
def test_t1_f02_02_fabushi_bots_default_to_3d_plush_not_2d(env: TempUiUxEnv) -> None:
    plush = env.probe_scenario("f02_default_bot_avatar", "plush")
    av = require_field(plush, "default_bot_avatar")
    assert require_field(av, "is_plush_3d") is True
    assert require_field(av, "renderer") == "PlushFeltRenderer"
    assert require_field(av, "shape_prefix") == "plush:"
    assert require_field(av, "legacy_2d_fallback_used") is False


@uiux_e2e_test(
    id="TEST-UIUX-T1-UI-F02-03",
    tier=1,
    features=("UI-F02",),
    description="Verify 13 silhouettes, 18 color choices, and 256-entry tintRamp LUT accuracy & caching",
)
def test_t1_f02_03_fnv1a_13_silhouettes_18_colors_lut_cache(env: TempUiUxEnv) -> None:
    plush = env.probe_scenario("f02_lut_and_catalog", "plush")
    silhouettes = require_field(plush, "supported_silhouettes")
    assert len(silhouettes) == 13
    assert set(silhouettes) == set(SUPPORTED_AVATAR_SILHOUETTES)
    assert "goutte" not in silhouettes
    assert require_field(plush, "color_choices_count") == 18
    lut = require_field(plush, "tint_ramp")
    assert require_field(lut, "lut_len") == 256
    assert require_field(lut, "cache_hit") is True
    sample_hex = require_field(lut, "hex")
    grey = require_field(lut, "sample_grey_byte")
    actual_rgba = tuple(require_field(lut, "sample_rgba"))
    expected_rgba = plush_tint_ramp_lut(sample_hex)[grey]
    for a, e in zip(actual_rgba, expected_rgba):
        assert abs(a - e) <= 1, f"LUT mismatch for {sample_hex}@{grey}: {actual_rgba} vs {expected_rgba}"


@uiux_e2e_test(
    id="TEST-UIUX-T1-UI-F02-04",
    tier=1,
    features=("UI-F02",),
    description="Verify 7 avatar moods priority order and 8 tool activities with 1400ms hold",
)
def test_t1_f02_04_seven_moods_priority_and_eight_activities(env: TempUiUxEnv) -> None:
    plush = env.probe_scenario("f02_moods_activities", "plush")
    mood = require_field(plush, "mood")
    assert require_field(mood, "active") == "failed"
    assert len(require_field(mood, "supported_moods")) == 7
    act = require_field(plush, "activity")
    assert require_field(act, "hold_threshold_ms") == 1400
    act_map = require_field(act, "activity_expression_map")
    for k, exp in TOOL_ACTIVITY_EXPRESSIONS.items():
        assert act_map.get(k) == exp, f"Tool activity {k!r} mapped to {act_map.get(k)!r}, expected {exp!r}"


@uiux_e2e_test(
    id="TEST-UIUX-T1-UI-F02-05",
    tier=1,
    features=("UI-F02",),
    description="Verify 30x16px 3-hopping-ball felt working badge geometry, assets, and stagger",
)
def test_t1_f02_05_working_30x16_three_hopping_balls_badge(env: TempUiUxEnv) -> None:
    plush = env.probe_scenario("f02_working_badge", "plush")
    wb = require_field(plush, "badges.working")
    assert require_field(wb, "width_px") == 30
    assert require_field(wb, "height_px") == 16
    assert_hex_eq(require_field(wb, "bg_hex"), "#212121", "working.bg")
    assert require_field(wb, "ball_count") == 3
    assert require_field(wb, "ball_size_px") == 7
    assert require_field(wb, "ball_assets") == ["ball-lilac.webp", "ball-mint.webp", "ball-pink.webp"]
    assert require_field(wb, "stagger_ms") == [0, 130, 260]
    assert require_field(wb, "loop_ms") == 960


@uiux_e2e_test(
    id="TEST-UIUX-T1-UI-F02-06",
    tier=1,
    features=("UI-F02",),
    description="Verify plum unread, amber waiting, green responded, and raspberry error felt badges",
)
def test_t1_f02_06_felt_status_badges_plum_amber_green_raspberry(env: TempUiUxEnv) -> None:
    plush = env.probe_scenario("f02_status_badges", "plush")
    badges = require_field(plush, "badges")
    unread = require_field(badges, "unread")
    assert require_field(unread, "top_rgb") == [112, 80, 131]
    assert require_field(unread, "bottom_rgb") == [74, 40, 92]
    assert_hex_eq(require_field(unread, "text_hex"), "#f6efe2", "unread.text")
    waiting = require_field(badges, "waiting")
    assert require_field(waiting, "size_px") == 18
    assert require_field(waiting, "top_rgb") == [255, 149, 17]
    responded = require_field(badges, "responded")
    assert require_field(responded, "size_px") == 16
    assert_hex_eq(require_field(responded, "color_hex"), "#31cf76", "responded.color")
    err = require_field(badges, "error")
    assert require_field(err, "size_px") == 16
    assert require_field(err, "asset") == "felt-x-ball.webp"


# ---------------------------------------------------------------------------
# UI-F03: OpenBot Inset Edge-Framed Root Shell (6 tests)
# ---------------------------------------------------------------------------


@uiux_e2e_test(
    id="TEST-UIUX-T1-UI-F03-01",
    tier=1,
    features=("UI-F03",),
    description="Verify dark mode #0d0d0d outer frame and 14px-rounded #1a1a1a inset card geometry",
)
def test_t1_f03_01_dark_outer_frame_and_inset_14px_card(env: TempUiUxEnv) -> None:
    sh = env.probe_scenario("f03_dark_shell", "shell")
    assert_hex_eq(require_field(sh, "outer_frame_bg_hex"), "#0d0d0d", "shell.frame")
    assert require_field(sh, "top_strip_height_px") == 38
    card = require_field(sh, "canvas_card")
    assert_hex_eq(require_field(card, "bg_hex"), "#1a1a1a", "card.bg")
    assert require_field(card, "radius_px") == 14
    assert require_field(card, "border_px") == 1
    assert require_field(card, "inset_right_px") == 4
    assert require_field(card, "inset_bottom_px") == 4
    assert require_field(card, "full_bleed") is False


@uiux_e2e_test(
    id="TEST-UIUX-T1-UI-F03-02",
    tier=1,
    features=("UI-F03",),
    description="Verify light mode #e8e8eb outer frame and #f5f5f6 14px-rounded inset card",
)
def test_t1_f03_02_light_outer_frame_and_inset_14px_card(env: TempUiUxEnv) -> None:
    sh = env.probe_scenario("f03_light_shell", "shell")
    assert_hex_eq(require_field(sh, "outer_frame_bg_hex"), "#e8e8eb", "light.frame")
    card = require_field(sh, "canvas_card")
    assert_hex_eq(require_field(card, "bg_hex"), "#f5f5f6", "light.card")
    assert require_field(card, "radius_px") == 14
    assert require_field(card, "inset_right_px") == 4
    assert require_field(card, "inset_bottom_px") == 4
    assert require_field(sh, "top_strip_height_px") == 38


@uiux_e2e_test(
    id="TEST-UIUX-T1-UI-F03-03",
    tier=1,
    features=("UI-F03",),
    description="Verify 3-zone single card eliminates 5-column layout crush from image_3.png",
)
def test_t1_f03_03_three_zone_single_card_eliminates_5col_crush(env: TempUiUxEnv) -> None:
    sh = env.probe_scenario("f03_active_chat_1440", "shell")
    assert require_field(sh, "rail_visible") is True
    assert require_field(sh, "rail_width_px") == 52
    assert require_field(sh, "sidebar_visible") is True
    assert require_field(sh, "sidebar_width_px") == 280
    assert require_field(sh, "conversation_min_width_px") == 424
    assert require_field(sh, "conversation_rect.w") >= 424
    assert require_field(sh, "right_panel_count") <= 1


@uiux_e2e_test(
    id="TEST-UIUX-T1-UI-F03-04",
    tier=1,
    features=("UI-F03",),
    description="Verify windowPageKind sidebar visibility across agents, titled hubs, and settings",
)
def test_t1_f03_04_window_page_kind_sidebar_visibility_rules(env: TempUiUxEnv) -> None:
    agents = env.probe_scenario("f03_page_agents", "shell")
    titled = env.probe_scenario("f03_page_titled", "shell")
    settings = env.probe_scenario("f03_page_settings", "shell")
    assert require_field(agents, "page_kind") == "agents"
    assert require_field(agents, "sidebar_visible") is True
    assert require_field(titled, "page_kind") == "titled"
    assert require_field(titled, "sidebar_visible") is False
    assert require_field(titled, "canvas_card.radius_px") == 14
    assert require_field(settings, "page_kind") == "settings"


@uiux_e2e_test(
    id="TEST-UIUX-T1-UI-F03-05",
    tier=1,
    features=("UI-F03",),
    description="Verify server rail defaults to menu mode and adapts darwin/linux widths in rail mode",
)
def test_t1_f03_05_server_rail_hidden_default_and_platform_widths(env: TempUiUxEnv) -> None:
    default_sh = env.probe_scenario("f03_server_menu", "shell")
    darwin_rail = env.probe_scenario("f03_server_rail_darwin", "shell")
    linux_rail = env.probe_scenario("f03_server_rail_linux", "shell")
    assert require_field(default_sh, "server_rail.view_mode") == "menu"
    assert require_field(default_sh, "server_rail.visible") is False
    assert require_field(darwin_rail, "server_rail.width_px") == 72
    assert require_field(linux_rail, "server_rail.width_px") == 64


@uiux_e2e_test(
    id="TEST-UIUX-T1-UI-F03-06",
    tier=1,
    features=("UI-F03",),
    description="Verify Cmd+B closing sidebar preserves 38px strip and shifts header tabs start offset",
)
def test_t1_f03_06_sidebar_closed_preserves_38px_strip_and_shifts_header(env: TempUiUxEnv) -> None:
    sh = env.probe_scenario("f03_sidebar_closed", "shell")
    assert require_field(sh, "sidebar_visible") is False
    assert require_field(sh, "sidebar_width_px") == 0
    assert require_field(sh, "top_strip_height_px") == 38
    assert require_field(sh, "top_row_tabs_start_px") > 0
    assert require_field(sh, "canvas_card.radius_px") == 14


# ---------------------------------------------------------------------------
# UI-F04: Unified 52px Left Icon PageRail + ... RailMoreMenu (6 tests)
# ---------------------------------------------------------------------------


@uiux_e2e_test(
    id="TEST-UIUX-T1-UI-F04-01",
    tier=1,
    features=("UI-F04",),
    description="Verify 52px icon-only PageRail button size 36px, radius 12px, icon 20px geometry",
)
def test_t1_f04_01_page_rail_52px_button_36px_radius_12px_geometry(env: TempUiUxEnv) -> None:
    rail = env.probe_scenario("f04_rail_geometry", "page_rail")
    assert require_field(rail, "width_px") == 52
    assert require_field(rail, "padding_top_px") == 42
    assert require_field(rail, "padding_bottom_px") == 12
    assert require_field(rail, "gap_px") == 6
    assert require_field(rail, "button_size_px") == 36
    assert require_field(rail, "button_radius_px") == 12
    assert require_field(rail, "icon_size_px") == 20
    assert require_field(rail, "icon_stroke_px") == 1.5


@uiux_e2e_test(
    id="TEST-UIUX-T1-UI-F04-02",
    tier=1,
    features=("UI-F04",),
    description="Verify top PageRail houses chats first plus pinned community/books/music/games/app_store",
)
def test_t1_f04_02_top_pinned_other_entries_and_chats_first(env: TempUiUxEnv) -> None:
    rail = env.probe_scenario("f04_top_items", "page_rail")
    top = require_field(rail, "top_items")
    assert top[0] == "chats"
    assert top[-1] == "more"
    for entry in ("community", "books", "music", "games", "app_store"):
        assert entry in top, f"Expected default pinned rail entry {entry!r} in {top!r}"
    assert_hex_eq(require_field(rail, "active_btn_bg_hex"), "#2a2a2a", "rail.active_btn")


@uiux_e2e_test(
    id="TEST-UIUX-T1-UI-F04-03",
    tier=1,
    features=("UI-F04",),
    description="Verify RailMoreMenu lists all secondary entries and persists Pin/Unpin toggles",
)
def test_t1_f04_03_rail_more_menu_lists_all_secondary_entries_and_pins(env: TempUiUxEnv) -> None:
    rail = env.probe_scenario("f04_more_menu_pin", "page_rail")
    menu = require_field(rail, "more_menu")
    assert require_field(menu, "open") is True
    assert require_field(menu, "placement") == "right-start"
    assert require_field(menu, "gutter_px") == 10
    entries = set(require_field(menu, "entries"))
    expected = {
        "my_creations", "contacts", "calls", "workflows", "tasks",
        "schedule", "library", "usage", "mcp_tools", "remote_computer", "browser",
    }
    assert expected.issubset(entries), f"Missing RailMoreMenu entries: {expected - entries}"
    persisted = require_field(rail, "persisted_pins")
    assert "my_creations" in persisted and "workflows" in persisted


@uiux_e2e_test(
    id="TEST-UIUX-T1-UI-F04-04",
    tier=1,
    features=("UI-F04",),
    description="Verify PageRail bottom utilities include usage ring, help, settings with dot, divider, and 24px plush tile",
)
def test_t1_f04_04_bottom_utility_group_single_24px_account_tile(env: TempUiUxEnv) -> None:
    rail = env.probe_scenario("f04_bottom_utilities", "page_rail")
    bot = require_field(rail, "bottom_items")
    assert require_field(bot, "usage_ring_slots") >= 1
    assert require_field(bot, "help_visible") is True
    assert require_field(bot, "settings_visible") is True
    assert require_field(bot, "update_dot_px") == 8
    assert_hex_eq(require_field(bot, "update_dot_hex"), "#ff1e5e", "rail.update_dot")
    assert require_field(bot, "divider_px") == [24, 1]
    assert require_field(bot, "account_tile_px") == 24
    assert require_field(bot, "account_tile_is_plush") is True


@uiux_e2e_test(
    id="TEST-UIUX-T1-UI-F04-05",
    tier=1,
    features=("UI-F04",),
    description="Verify PageRail hosts the sole account footer with zero duplicate footer in ChatSidebar (D2)",
)
def test_t1_f04_05_sole_account_footer_zero_duplicate_in_sidebar(env: TempUiUxEnv) -> None:
    rail = env.probe_scenario("f04_sole_footer", "page_rail")
    slist = env.probe_scenario("f04_sole_footer", "sidebar_list")
    assert require_field(rail, "total_account_footers_in_shell") == 1
    assert require_field(slist, "has_duplicate_account_footer") is False


@uiux_e2e_test(
    id="TEST-UIUX-T1-UI-F04-06",
    tier=1,
    features=("UI-F04",),
    description="Verify hiding optional bottom rail item shows Undo toast and persists/restores cleanly",
)
def test_t1_f04_06_hide_optional_bottom_items_with_undo_and_persist(env: TempUiUxEnv) -> None:
    hidden = env.probe_scenario("f04_hide_help", "page_rail")
    restored = env.probe_scenario("f04_undo_hide_help", "page_rail")
    assert "help" in require_field(hidden, "hidden_bottom_items")
    assert "help" in require_field(hidden, "persisted_hidden")
    assert require_field(hidden, "bottom_items.help_visible") is False
    assert require_field(hidden, "undo_toast_visible") is True
    assert require_field(restored, "bottom_items.help_visible") is True
    assert "help" not in require_field(restored, "persisted_hidden")


# ---------------------------------------------------------------------------
# UI-F05: OpenBot SidebarTopbar, Pill Search + Filter Funnel + White + Menu (6 tests)
# ---------------------------------------------------------------------------


@uiux_e2e_test(
    id="TEST-UIUX-T1-UI-F05-01",
    tier=1,
    features=("UI-F05",),
    description="Verify 280px default sidebar width, [128, 400] bounds, resize persistence, and 288px compat const",
)
def test_t1_f05_01_sidebar_280px_default_and_128_400_resize_persist(env: TempUiUxEnv) -> None:
    init = env.probe_scenario("f05_default_width", "sidebar_chrome")
    resized = env.probe_scenario("f05_resized_340", "sidebar_chrome")
    assert require_field(init, "default_width_px") == 280
    assert require_field(init, "width_px") == 280
    assert require_field(init, "min_width_px") == 128
    assert require_field(init, "max_width_px") == 400
    assert require_field(init, "compat_const_default_width_px") == 288
    assert require_field(resized, "width_px") == 340
    assert require_field(resized, "persisted_width_px") == 340


@uiux_e2e_test(
    id="TEST-UIUX-T1-UI-F05-02",
    tier=1,
    features=("UI-F05",),
    description="Verify 88px compact snap below 110px and re-expand above 118px matching hysteresis oracle",
)
def test_t1_f05_02_sidebar_88px_compact_snap_hysteresis_110_118(env: TempUiUxEnv) -> None:
    compact = env.probe_scenario("f05_drag_105_compact", "sidebar_chrome")
    expanded = env.probe_scenario("f05_drag_124_expand", "sidebar_chrome")
    exp_mode_c, exp_w_c = sidebar_hysteresis_step("expanded", 105.0)
    exp_mode_e, exp_w_e = sidebar_hysteresis_step("compact", 124.0)
    assert require_field(compact, "mode") == exp_mode_c == "compact"
    assert require_field(compact, "width_px") == exp_w_c == 88
    assert require_field(compact, "collapse_threshold_px") == 110
    assert require_field(compact, "expand_threshold_px") == 118
    assert require_field(expanded, "mode") == exp_mode_e == "expanded"
    assert require_field(expanded, "width_px") >= exp_w_e


@uiux_e2e_test(
    id="TEST-UIUX-T1-UI-F05-03",
    tier=1,
    features=("UI-F05",),
    description="Verify Cmd+B closes sidebar to 0px while PanelLeft toggle restores 280px cleanly",
)
def test_t1_f05_03_cmd_b_and_panel_left_toggle_close_restore(env: TempUiUxEnv) -> None:
    closed = env.probe_scenario("f05_cmd_b_closed", "sidebar_chrome")
    restored = env.probe_scenario("f05_panel_left_restored", "sidebar_chrome")
    assert require_field(closed, "mode") == "closed"
    assert require_field(closed, "width_px") == 0
    assert require_field(closed, "topbar.toggle_visible") is True
    assert require_field(restored, "mode") == "expanded"
    assert require_field(restored, "width_px") == 280


@uiux_e2e_test(
    id="TEST-UIUX-T1-UI-F05-04",
    tier=1,
    features=("UI-F05",),
    description="Verify 38px SidebarTopbar back/forward history states and macOS traffic-light clearance",
)
def test_t1_f05_04_sidebar_38px_topbar_history_nav_and_mac_clearance(env: TempUiUxEnv) -> None:
    nav_b = env.probe_scenario("f05_history_at_b", "sidebar_chrome")
    nav_back = env.probe_scenario("f05_history_after_back", "sidebar_chrome")
    assert require_field(nav_b, "topbar.height_px") == 38
    assert require_field(nav_b, "topbar.back_enabled") is True
    assert require_field(nav_b, "topbar.forward_enabled") is False
    assert_within(require_field(nav_b, "topbar.forward_opacity"), 0.35, 0.45, "forward_opacity")
    assert require_field(nav_back, "topbar.forward_enabled") is True
    assert require_field(nav_b, "topbar.mac_clearance_px") in (24, 78)


@uiux_e2e_test(
    id="TEST-UIUX-T1-UI-F05-05",
    tier=1,
    features=("UI-F05",),
    description="Verify 32px pill search input, inset 28px funnel trigger, and Cmd+K search launch",
)
def test_t1_f05_05_search_32px_pill_and_inset_funnel_filter(env: TempUiUxEnv) -> None:
    sc = env.probe_scenario("f05_search_pill", "sidebar_chrome")
    search = require_field(sc, "search")
    assert require_field(search, "height_px") == 32
    assert require_field(search, "radius_px") >= 999
    assert_hex_eq(require_field(search, "bg_hex"), "#212121", "search.bg")
    assert require_field(search, "funnel_size_px") == 28
    assert require_field(search, "filter_active") is True


@uiux_e2e_test(
    id="TEST-UIUX-T1-UI-F05-06",
    tier=1,
    features=("UI-F05",),
    description="Verify 32x32 circular #f0f0f0 + dropdown exposing New Agent, New Channel, and New Folder",
)
def test_t1_f05_06_circular_32px_f0f0f0_plus_dropdown_three_actions(env: TempUiUxEnv) -> None:
    sc = env.probe_scenario("f05_plus_menu", "sidebar_chrome")
    btn = require_field(sc, "add_button")
    assert require_field(btn, "size_px") == 32
    assert_hex_eq(require_field(btn, "bg_hex"), "#f0f0f0", "plus.bg")
    assert_hex_eq(require_field(btn, "fg_hex"), "#141414", "plus.fg")
    assert require_field(btn, "menu_open") is True
    items = require_field(btn, "menu_items")
    assert len(items) == 3
    assert items == ["新建助手", "新建群聊", "新建分组"]
