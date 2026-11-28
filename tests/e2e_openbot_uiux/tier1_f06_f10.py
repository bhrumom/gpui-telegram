"""Tier 1 Isolated Feature Tests: UI-F06 through UI-F10 (30 opaque-box tests)."""

from __future__ import annotations

from tests.e2e_openbot_uiux.harness import (
    TempUiUxEnv,
    assert_hex_eq,
    assert_within,
    require_field,
    uiux_e2e_test,
)
from tests.e2e_openbot_uiux.oracles import is_valid_utf8_and_no_unpaired_surrogates


# ---------------------------------------------------------------------------
# UI-F06: Underline SidebarTabs, Compact Pinned Grid & Clean 62px Rows (6 tests)
# ---------------------------------------------------------------------------


@uiux_e2e_test(
    id="TEST-UIUX-T1-UI-F06-01",
    tier=1,
    features=("UI-F06",),
    description="Verify 32px flat underline SidebarTabs with zero Telegram blue pills (D5)",
)
def test_t1_f06_01_flat_32px_underline_tabs_zero_blue_pills(env: TempUiUxEnv) -> None:
    sl = env.probe_scenario("f06_tabs", "sidebar_list")
    tabs = require_field(sl, "tabs")
    assert require_field(tabs, "visible") is True
    assert require_field(tabs, "height_px") == 32
    assert require_field(tabs, "active_id") == "all"
    assert require_field(tabs, "active_underline_px") == 2
    assert_hex_eq(require_field(tabs, "active_underline_hex"), "#ffffff", "tabs.underline")
    assert require_field(tabs, "uses_blue_pill") is False
    assert require_field(tabs, "unread_badge_height_px") == 18
    assert require_field(tabs, "sliders_menu_open") is True


@uiux_e2e_test(
    id="TEST-UIUX-T1-UI-F06-02",
    tier=1,
    features=("UI-F06",),
    description="Verify compact 3-col 72x102px pinned grid appears only when pinned (D4)",
)
def test_t1_f06_02_compact_72x102_pinned_grid_only_when_pinned(env: TempUiUxEnv) -> None:
    zero_pins = env.probe_scenario("f06_zero_pins", "sidebar_list")
    three_pins = env.probe_scenario("f06_three_pins", "sidebar_list")
    pg0 = require_field(zero_pins, "pinned_grid")
    pg3 = require_field(three_pins, "pinned_grid")
    assert require_field(pg0, "visible") is False
    assert require_field(pg0, "legacy_oversized_circles") is False
    assert require_field(pg3, "visible") is True
    assert require_field(pg3, "columns") == 3
    assert require_field(pg3, "max_width_px") == 236
    assert require_field(pg3, "tile_w_px") == 72 and require_field(pg3, "tile_h_px") == 102
    assert require_field(pg3, "avatar_px") == 56 and require_field(pg3, "gap_px") == 8


@uiux_e2e_test(
    id="TEST-UIUX-T1-UI-F06-03",
    tier=1,
    features=("UI-F06",),
    description="Verify 62px chat rows with 46px plush avatar, CJK ellipsis, and right timestamp (D6)",
)
def test_t1_f06_03_clean_62px_rows_46px_avatar_timestamp_and_preview(env: TempUiUxEnv) -> None:
    sl = env.probe_scenario("f06_cjk_rows", "sidebar_list")
    rows = require_field(sl, "rows")
    assert len(rows) >= 1
    row = rows[0]
    assert require_field(row, "height_px") == 62
    assert require_field(row, "avatar_px") == 46
    assert require_field(row, "radius_px") == 8
    assert require_field(row, "min_w_0") is True
    assert require_field(row, "whitespace_nowrap") is True
    assert require_field(row, "time_visible") is True
    preview = require_field(row, "preview_displayed")
    assert is_valid_utf8_and_no_unpaired_surrogates(preview)
    assert require_field(row, "preview_truncated") is True


@uiux_e2e_test(
    id="TEST-UIUX-T1-UI-F06-04",
    tier=1,
    features=("UI-F06",),
    description="Verify hover thread chevron, 30px inline thread sub-rows, and row context menu",
)
def test_t1_f06_04_inline_30px_thread_subrows_and_context_menu(env: TempUiUxEnv) -> None:
    sl = env.probe_scenario("f06_threads_and_menu", "sidebar_list")
    row = require_field(sl, "rows")[0]
    assert require_field(row, "thread_chevron_hover") is True
    assert require_field(row, "thread_subrow_height_px") == 30
    ctx = set(require_field(sl, "context_menu_items"))
    for action in ("pin", "move_to_folder", "hide", "mute", "settings", "delete"):
        assert action in ctx, f"Missing sidebar row context action {action!r}"


@uiux_e2e_test(
    id="TEST-UIUX-T1-UI-F06-05",
    tier=1,
    features=("UI-F06",),
    description="Verify canonical top-to-bottom sidebar group ordering",
)
def test_t1_f06_05_canonical_sidebar_group_ordering(env: TempUiUxEnv) -> None:
    sl = env.probe_scenario("f06_group_order", "sidebar_list")
    order = require_field(sl, "group_order")
    assert order == ["pinned", "waiting", "unread", "sections", "hidden", "archived"]
    assert len(order) == 6


@uiux_e2e_test(
    id="TEST-UIUX-T1-UI-F06-06",
    tier=1,
    features=("UI-F06",),
    description="Verify zero duplicate bottom account footer rendered inside ChatSidebar (D2)",
)
def test_t1_f06_06_zero_duplicate_bottom_account_footer_in_chat_sidebar(env: TempUiUxEnv) -> None:
    sl = env.probe_scenario("f06_no_dup_footer", "sidebar_list")
    rail = env.probe_scenario("f06_no_dup_footer", "page_rail")
    assert require_field(sl, "has_duplicate_account_footer") is False
    assert require_field(rail, "total_account_footers_in_shell") == 1


# ---------------------------------------------------------------------------
# UI-F07: OpenBot GlobalSearch (Cmd+K) Command Palette & GlobalSearchPlush (6 tests)
# ---------------------------------------------------------------------------


@uiux_e2e_test(
    id="TEST-UIUX-T1-UI-F07-01",
    tier=1,
    features=("UI-F07",),
    description="Verify 640px centered GlobalSearch modal at top 112px with #1e1e1e surface",
)
def test_t1_f07_01_global_search_640px_top112_modal_geometry(env: TempUiUxEnv) -> None:
    gs = env.probe_scenario("f07_modal_open", "global_search")
    assert require_field(gs, "open") is True
    assert require_field(gs, "width_px") == 640
    assert require_field(gs, "top_px") == 112
    assert_hex_eq(require_field(gs, "bg_hex"), "#1e1e1e", "global_search.bg")
    assert require_field(gs, "ring_border") is True
    assert require_field(gs, "input_height_px") == 48
    assert require_field(gs, "footer.height_px") == 36


@uiux_e2e_test(
    id="TEST-UIUX-T1-UI-F07-02",
    tier=1,
    features=("UI-F07",),
    description="Verify 6 sliding-pill GlobalSearch filter tabs and deterministic Tab wrap cycle",
)
def test_t1_f07_02_six_sliding_pill_filter_tabs_and_tab_cycle(env: TempUiUxEnv) -> None:
    gs = env.probe_scenario("f07_tab_cycle", "global_search")
    tabs = require_field(gs, "tabs")
    assert tabs == ["all", "agents", "channels", "messages", "files", "routines"]
    assert require_field(gs, "active_tab") == "all"
    assert require_field(gs, "tab_cycle_trace") == [
        "agents", "channels", "messages", "files", "routines", "all"
    ]


@uiux_e2e_test(
    id="TEST-UIUX-T1-UI-F07-03",
    tier=1,
    features=("UI-F07",),
    description="Verify 36px virtualized rows, 28px section headers, highlight marks, and Enter Kbd",
)
def test_t1_f07_03_virtualized_36px_rows_marks_and_enter_kbd(env: TempUiUxEnv) -> None:
    gs = env.probe_scenario("f07_query_planner", "global_search")
    assert require_field(gs, "row_height_px") == 36
    assert require_field(gs, "section_header_height_px") == 28
    idx = require_field(gs, "highlighted_index")
    results = require_field(gs, "results")
    assert 0 <= idx < len(results)
    row = results[idx]
    assert require_field(row, "kbd_enter_visible") is True
    assert require_field(row, "avatar_px") == 20
    assert len(require_field(row, "highlighted_marks")) >= 1


@uiux_e2e_test(
    id="TEST-UIUX-T1-UI-F07-04",
    tier=1,
    features=("UI-F07",),
    description="Verify 56px GlobalSearchPlush caret gaze tracking bounds and 500ms idle scan tilt",
)
def test_t1_f07_04_global_search_plush_56px_caret_tracking_and_idle(env: TempUiUxEnv) -> None:
    gs = env.probe_scenario("f07_plush_idle", "global_search")
    pl = require_field(gs, "plush")
    assert require_field(pl, "visible") is True
    assert require_field(pl, "size_px") == 56
    assert_within(require_field(pl, "caret_offset_x_pct"), -7.0, 7.0, "plush.caret_x")
    assert_within(require_field(pl, "caret_offset_y_pct"), -5.0, 5.0, "plush.caret_y")
    assert require_field(pl, "idle_scan_active") is True
    assert_within(require_field(pl, "head_tilt_deg"), -12.0, -8.0, "plush.head_tilt")


@uiux_e2e_test(
    id="TEST-UIUX-T1-UI-F07-05",
    tier=1,
    features=("UI-F07",),
    description="Verify 36px keyboard footer hints and Enter executing selected search hit",
)
def test_t1_f07_05_keyboard_footer_hints_and_enter_executes_result(env: TempUiUxEnv) -> None:
    open_gs = env.probe_scenario("f07_modal_open", "global_search")
    done_gs = env.probe_scenario("f07_enter_executed", "global_search")
    assert require_field(open_gs, "footer.height_px") == 36
    hints = require_field(open_gs, "footer.hints")
    assert len(hints) >= 4
    assert require_field(done_gs, "open") is False
    assert bool(require_field(done_gs, "navigated_target_id"))


@uiux_e2e_test(
    id="TEST-UIUX-T1-UI-F07-06",
    tier=1,
    features=("UI-F07",),
    description="Verify 150ms debounced remote search across messages and files tabs",
)
def test_t1_f07_06_paged_remote_search_150ms_debounce_messages_files(env: TempUiUxEnv) -> None:
    gs = env.probe_scenario("f07_remote_invoice", "global_search")
    assert require_field(gs, "remote.debounce_ms") == 150
    results = require_field(gs, "results")
    assert len(results) >= 1
    kinds = {require_field(r, "kind") for r in results}
    assert kinds.issubset({"message", "file"})


# ---------------------------------------------------------------------------
# UI-F08: OpenBot FirstAgentSetup Create-Agent Studio (image.png) (6 tests)
# ---------------------------------------------------------------------------


@uiux_e2e_test(
    id="TEST-UIUX-T1-UI-F08-01",
    tier=1,
    features=("UI-F08",),
    description="Verify 420px FirstAgentSetup studio inside #1a1a1a canvas with 38px live preview header",
)
def test_t1_f08_01_create_studio_420px_canvas_and_38px_live_header(env: TempUiUxEnv) -> None:
    st = env.probe_scenario("f08_first_mode", "agent_studio")
    assert require_field(st, "form_width_px") == 420
    assert_hex_eq(require_field(st, "canvas_bg_hex"), "#1a1a1a", "studio.canvas")
    hdr = require_field(st, "header")
    assert require_field(hdr, "height_px") == 38
    assert require_field(hdr, "avatar_px") == 18
    assert require_field(hdr, "live_name") == "Research Bot"
    assert require_field(hdr, "cancel_visible") is False


@uiux_e2e_test(
    id="TEST-UIUX-T1-UI-F08-02",
    tier=1,
    features=("UI-F08",),
    description="Verify 72px 3D plush hero and 9x2 (18) color swatches with 1px solid #42a0ff ring",
)
def test_t1_f08_02_hero_72px_plush_and_9x2_18_swatches_42a0ff_ring(env: TempUiUxEnv) -> None:
    st = env.probe_scenario("f08_swatch_pick", "agent_studio")
    hero = require_field(st, "hero")
    assert require_field(hero, "size_px") == 72
    assert require_field(hero, "is_plush_3d") is True
    assert require_field(hero, "pointer_follow") is True
    sw = require_field(st, "swatches")
    assert require_field(sw, "cols") == 9 and require_field(sw, "rows") == 2
    assert require_field(sw, "total") == 18 and require_field(sw, "swatch_px") == 18
    assert require_field(sw, "selected_idx") == 4
    assert "#42a0ff" in require_field(sw, "selection_ring_css").lower()


@uiux_e2e_test(
    id="TEST-UIUX-T1-UI-F08-03",
    tier=1,
    features=("UI-F08",),
    description="Verify 12-13 plush face/shape buttons update hero silhouette and omit goutte",
)
def test_t1_f08_03_plush_12_to_13_shape_buttons_update_hero_silhouette(env: TempUiUxEnv) -> None:
    st = env.probe_scenario("f08_shape_pick", "agent_studio")
    sh = require_field(st, "shapes")
    assert_within(require_field(sh, "count"), 12, 13, "shapes.count")
    assert require_field(sh, "button_px") == 33
    assert require_field(sh, "preview_px") == 24
    assert require_field(sh, "eye_scale") == 1.25
    assert require_field(sh, "contains_goutte") is False
    assert require_field(st, "hero.shape") == "triangle"


@uiux_e2e_test(
    id="TEST-UIUX-T1-UI-F08-04",
    tier=1,
    features=("UI-F08",),
    description="Verify Name, Purpose textarea, Model selector, and #f0f0f0 Create Agent CTA styling",
)
def test_t1_f08_04_name_purpose_model_fields_and_f0f0f0_create_cta(env: TempUiUxEnv) -> None:
    st = env.probe_scenario("f08_filled_form", "agent_studio")
    fields = require_field(st, "fields")
    assert require_field(fields, "purpose_min_height_px") == 64
    assert require_field(fields, "model_row_height_px") == 44
    sub = require_field(st, "submit")
    assert require_field(sub, "label") == "创建助手"
    assert_hex_eq(require_field(sub, "bg_hex"), "#f0f0f0", "cta.bg")
    assert_hex_eq(require_field(sub, "fg_hex"), "#141414", "cta.fg")
    assert require_field(sub, "min_width_px") == 132
    assert require_field(sub, "disabled") is False


@uiux_e2e_test(
    id="TEST-UIUX-T1-UI-F08-05",
    tier=1,
    features=("UI-F08",),
    description="Verify >=24 recommendation templates and card click hydrating studio draft",
)
def test_t1_f08_05_recommendations_24_templates_hydrate_studio_draft(env: TempUiUxEnv) -> None:
    st = env.probe_scenario("f08_template_trip_planner", "agent_studio")
    rec = require_field(st, "recommendations")
    assert require_field(rec, "total_templates") >= 24
    assert require_field(rec, "visible_cards") >= 3
    assert require_field(rec, "card_w_px") == 256
    assert require_field(rec, "card_h_px") == 112
    assert require_field(rec, "card_avatar_px") == 42
    assert require_field(rec, "suggestion_id") == "trip-planner"
    assert require_field(rec, "hydrated") is True
    assert bool(require_field(st, "fields.purpose").strip())


@uiux_e2e_test(
    id="TEST-UIUX-T1-UI-F08-06",
    tier=1,
    features=("UI-F08",),
    description="Verify studio submit invokes CreateManagedBot and navigates into new plush bot chat",
)
def test_t1_f08_06_submit_wires_create_managed_bot_and_opens_chat(env: TempUiUxEnv) -> None:
    st = env.probe_scenario("f08_submit_created", "agent_studio")
    assert bool(require_field(st, "created_agent_id"))
    assert require_field(st, "navigated_to_chat") is True
    assert require_field(st, "wired_to_create_managed_bot") is True


# ---------------------------------------------------------------------------
# UI-F09: Non-Overlapping 38px ConversationHeader & Seamless Active TabShape (6 tests)
# ---------------------------------------------------------------------------


@uiux_e2e_test(
    id="TEST-UIUX-T1-UI-F09-01",
    tier=1,
    features=("UI-F09",),
    description="Verify solid 38px flex-flow ConversationHeader never overlaps scrolled message timeline (D3)",
)
def test_t1_f09_01_solid_38px_flex_header_zero_absolute_collision(env: TempUiUxEnv) -> None:
    hdr = env.probe_scenario("f09_scrolled_chat", "conversation_header")
    assert require_field(hdr, "height_px") == 38
    assert require_field(hdr, "position_mode") == "flex_flow"
    assert_hex_eq(require_field(hdr, "bg_strip_hex"), "#0d0d0d", "header.strip")
    hrect = require_field(hdr, "header_rect")
    trect = require_field(hdr, "timeline_rect")
    assert require_field(hrect, "y") + require_field(hrect, "h") <= require_field(trect, "y")
    assert require_field(hdr, "overlaps_timeline") is False


@uiux_e2e_test(
    id="TEST-UIUX-T1-UI-F09-02",
    tier=1,
    features=("UI-F09",),
    description="Verify seamless curved TabShape with 8px fillet radius joining #1a1a1a canvas",
)
def test_t1_f09_02_seamless_curved_tab_shape_joins_canvas(env: TempUiUxEnv) -> None:
    hdr = env.probe_scenario("f09_tab_shape", "conversation_header")
    ts = require_field(hdr, "tab_shape")
    assert require_field(ts, "visible") is True
    assert require_field(ts, "fillet_radius_px") == 8
    assert_hex_eq(require_field(ts, "fill_hex"), "#1a1a1a", "tab_shape.fill")
    assert require_field(ts, "transition_ms") == 240
    assert require_field(ts, "disabled_by_motion_or_glass") is False


@uiux_e2e_test(
    id="TEST-UIUX-T1-UI-F09-03",
    tier=1,
    features=("UI-F09",),
    description="Verify clickable header title routes Bot->AssistantSettings, Group->ChannelSettings, Human->ContactProfile",
)
def test_t1_f09_03_clickable_title_routes_to_settings_or_main_chat(env: TempUiUxEnv) -> None:
    bot = env.probe_scenario("f09_title_bot", "conversation_header")
    grp = env.probe_scenario("f09_title_group", "conversation_header")
    hum = env.probe_scenario("f09_title_human", "conversation_header")
    assert require_field(bot, "title_btn.height_px") == 28
    assert require_field(bot, "title_btn.avatar_px") == 18
    assert require_field(bot, "title_btn.click_target_pane") == "assistant_settings"
    assert require_field(grp, "title_btn.click_target_pane") == "channel_settings"
    assert require_field(hum, "title_btn.click_target_pane") == "contact_profile"


@uiux_e2e_test(
    id="TEST-UIUX-T1-UI-F09-04",
    tier=1,
    features=("UI-F09",),
    description="Verify + new thread button and Cmd+Shift+O append and focus new thread tab",
)
def test_t1_f09_04_new_thread_plus_button_and_shortcut_cmd_shift_o(env: TempUiUxEnv) -> None:
    hdr = env.probe_scenario("f09_new_thread", "conversation_header")
    assert require_field(hdr, "new_thread_btn_visible") is True
    assert require_field(hdr, "thread_tabs_count") >= 2
    assert require_field(hdr, "tab_shape.visible") is True


@uiux_e2e_test(
    id="TEST-UIUX-T1-UI-F09-05",
    tier=1,
    features=("UI-F09",),
    description="Verify 28px ProviderModelPicker full fit mode and mid-turn pending badge",
)
def test_t1_f09_05_provider_model_picker_and_pending_model_badge(env: TempUiUxEnv) -> None:
    hdr = env.probe_scenario("f09_model_picker", "conversation_header")
    mp = require_field(hdr, "model_picker")
    assert require_field(mp, "visible") is True
    assert require_field(mp, "height_px") == 28
    assert require_field(mp, "disabled") is False
    assert require_field(mp, "fit_mode") == "full"
    assert require_field(mp, "pending_badge_visible") is True


@uiux_e2e_test(
    id="TEST-UIUX-T1-UI-F09-06",
    tier=1,
    features=("UI-F09",),
    description="Verify 420px PinnedMessages popover and 28px PanelRight toggle with hairline",
)
def test_t1_f09_06_pinned_messages_popover_and_side_panel_toggle(env: TempUiUxEnv) -> None:
    hdr = env.probe_scenario("f09_pinned_and_toggle", "conversation_header")
    pp = require_field(hdr, "pinned_popover")
    assert require_field(pp, "open") is True
    assert require_field(pp, "max_width_px") == 420
    assert require_field(pp, "count") == 2
    assert require_field(pp, "empty_state_visible") is False
    tog = require_field(hdr, "side_panel_toggle")
    assert require_field(tog, "size_px") == 28
    assert require_field(tog, "hairline_visible") is True


# ---------------------------------------------------------------------------
# UI-F10: OpenBot 1:1 Chat Stream, Neutral Bubbles, AgentActivity & AgentSteps (6 tests)
# ---------------------------------------------------------------------------


@uiux_e2e_test(
    id="TEST-UIUX-T1-UI-F10-01",
    tier=1,
    features=("UI-F10",),
    description="Verify monochrome #242424/#212121 bubbles, 22px/6px corners, and hover-only actions (D8)",
)
def test_t1_f10_01_monochrome_242424_212121_bubbles_22px_6px_hover_only(env: TempUiUxEnv) -> None:
    cs = env.probe_scenario("f10_bubbles_dark", "chat_stream")
    assert require_field(cs, "entry_gap_px") == 12
    assert require_field(cs, "grouped_gap_px") == 4
    bubbles = require_field(cs, "bubbles")
    assert len(bubbles) >= 2
    user_b = next(b for b in bubbles if b.get("author") == "user")
    asst_b = next(b for b in bubbles if b.get("author") == "assistant")
    assert_hex_eq(require_field(user_b, "bg_hex"), "#242424", "user.bg")
    assert require_field(user_b, "uses_legacy_blue") is False
    assert require_field(user_b, "radius_outer_px") == 22
    assert require_field(user_b, "radius_grouped_px") == 6
    assert_hex_eq(require_field(asst_b, "bg_hex"), "#212121", "assistant.bg")
    assert require_field(user_b, "actions_opacity_rest") == 0.0
    assert require_field(user_b, "time_opacity_rest") == 0.0
    assert require_field(user_b, "actions_opacity_hover") == 1.0


@uiux_e2e_test(
    id="TEST-UIUX-T1-UI-F10-02",
    tier=1,
    features=("UI-F10",),
    description="Verify bubble-less 32px plush thinking shimmer and 5s/60s elapsed timer thresholds",
)
def test_t1_f10_02_bubbleless_32px_plush_thinking_shimmer_and_5s_timer(env: TempUiUxEnv) -> None:
    t2 = env.probe_scenario("f10_thinking_2s", "chat_stream")
    t6 = env.probe_scenario("f10_thinking_6s", "chat_stream")
    t61 = env.probe_scenario("f10_thinking_plain_61s", "chat_stream")
    act6 = require_field(t6, "activity")
    assert require_field(act6, "visible") is True
    assert require_field(act6, "bubble_less") is True
    assert require_field(act6, "avatar_px") == 32
    assert require_field(act6, "shimmer_active") is True
    assert require_field(act6, "steps_max_height_px") == 220
    assert require_field(t2, "activity.elapsed_visible") is False
    assert require_field(act6, "elapsed_visible") is True
    assert require_field(t61, "activity.elapsed_visible") is True


@uiux_e2e_test(
    id="TEST-UIUX-T1-UI-F10-03",
    tier=1,
    features=("UI-F10",),
    description="Verify collapsible AgentSteps quarantines tool errors behind Show error 160px pre drawer",
)
def test_t1_f10_03_collapsible_agent_steps_and_show_error_pre_drawer(env: TempUiUxEnv) -> None:
    closed = env.probe_scenario("f10_steps_error_default", "chat_stream")
    opened = env.probe_scenario("f10_steps_error_opened", "chat_stream")
    st0 = require_field(closed, "steps")
    assert require_field(st0, "collapsed_by_default") is True
    assert require_field(st0, "error_drawer_open") is False
    assert require_field(st0, "error_drawer_btn_label") == "Show error"
    assert require_field(st0, "trace_waterfall_available") is True
    for b in require_field(closed, "bubbles"):
        assert require_field(b, "contains_raw_json_wall") is False
    assert require_field(opened, "steps.error_drawer_open") is True
    assert require_field(opened, "steps.error_pre_max_height_px") == 160


@uiux_e2e_test(
    id="TEST-UIUX-T1-UI-F10-04",
    tier=1,
    features=("UI-F10",),
    description="Verify wide Markdown table/CodeBlock expansion and interactive UiBlockCard answered state",
)
def test_t1_f10_04_rich_markdown_codeblock_wide_tables_and_uiblock(env: TempUiUxEnv) -> None:
    cs = env.probe_scenario("f10_markdown_uiblock", "chat_stream")
    wide_b = next(b for b in require_field(cs, "bubbles") if b.get("wide_table_or_code"))
    assert require_field(wide_b, "wide_table_or_code") is True
    ui_block = require_field(cs, "ui_block")
    assert require_field(ui_block, "width_expr") == "min(540px, 100%)"
    assert require_field(ui_block, "status") == "answered"
    assert_hex_eq(require_field(ui_block, "badge_hex"), "#31cf76", "uiblock.badge")


@uiux_e2e_test(
    id="TEST-UIUX-T1-UI-F10-05",
    tier=1,
    features=("UI-F10",),
    description="Verify AgentChatEmptyState, ChatScrollRail minimap wave ticks, and Cmd+F ChatSearch",
)
def test_t1_f10_05_empty_state_scroll_rail_minimap_and_cmd_f_search(env: TempUiUxEnv) -> None:
    empty = env.probe_scenario("f10_empty_state", "chat_stream")
    pop = env.probe_scenario("f10_scroll_and_search", "chat_stream")
    assert require_field(empty, "empty_state.visible") is True
    assert require_field(empty, "empty_state.avatar_px") == 64
    assert require_field(empty, "empty_state.heading_px") == 32
    sr = require_field(pop, "scroll_rail")
    assert require_field(sr, "visible") is True
    assert require_field(sr, "tick_px") == 2
    assert require_field(sr, "wave_widths_px") == [4, 8, 10, 12, 16]
    sf = require_field(pop, "chat_search")
    assert require_field(sf, "open") is True
    assert require_field(sf, "width_px") == 308 and require_field(sf, "height_px") == 34
    assert require_field(sf, "right_offset_px") == 16


@uiux_e2e_test(
    id="TEST-UIUX-T1-UI-F10-06",
    tier=1,
    features=("UI-F10",),
    description="Verify 1:1 human DirectConversation 760px stream, 5px online dot, and 3s typing idle",
)
def test_t1_f10_06_direct_human_dm_760px_stream_online_dot_and_typing(env: TempUiUxEnv) -> None:
    cs = env.probe_scenario("f10_direct_dm", "chat_stream")
    dm = require_field(cs, "direct_dm")
    assert require_field(dm, "stream_width_px") == 760
    assert require_field(dm, "online_dot_px") == 5
    assert require_field(dm, "typing_active") is True
    assert require_field(dm, "typing_idle_timeout_ms") == 3000
    assert require_field(dm, "send_disabled") is False
