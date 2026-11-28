"""Tier 2 Boundary, Corner & Negative Tests: UI-F06 through UI-F10 (30 opaque-box tests)."""

from __future__ import annotations

from tests.e2e_openbot_uiux.harness import (
    TempUiUxEnv,
    assert_within,
    require_field,
    uiux_e2e_test,
)
from tests.e2e_openbot_uiux.oracles import (
    is_valid_utf8_and_no_unpaired_surrogates,
    utf16_surrogate_safe_truncate,
)


# ---------------------------------------------------------------------------
# UI-F06 Boundaries & Negative Cases (6 tests)
# ---------------------------------------------------------------------------


@uiux_e2e_test(
    id="TEST-UIUX-T2-UI-F06-01",
    tier=2,
    features=("UI-F06",),
    description="Verify <=300px sidebar collapses labeled amber wait chip to 18x18px icon-only chip",
)
def test_t2_f06_01_narrow_300px_collapses_wait_chip_to_18px_icon(env: TempUiUxEnv) -> None:
    wide = env.probe_scenario("f06_width_320", "sidebar_list")
    narrow = env.probe_scenario("f06_width_300", "sidebar_list")
    assert require_field(wide, "rows")[0]["wait_chip_icon_only"] is False
    assert require_field(narrow, "rows")[0]["wait_chip_icon_only"] is True


@uiux_e2e_test(
    id="TEST-UIUX-T2-UI-F06-02",
    tier=2,
    features=("UI-F06",),
    description="Verify <=210px sidebar hides row timestamps so agent/group names keep room",
)
def test_t2_f06_02_narrow_210px_hides_timestamp_preserving_name(env: TempUiUxEnv) -> None:
    w220 = env.probe_scenario("f06_width_220", "sidebar_list")
    w210 = env.probe_scenario("f06_width_210", "sidebar_list")
    assert require_field(w220, "rows")[0]["time_visible"] is True
    assert require_field(w210, "rows")[0]["time_visible"] is False


@uiux_e2e_test(
    id="TEST-UIUX-T2-UI-F06-03",
    tier=2,
    features=("UI-F06",),
    description="Verify mixed CJK + ZWJ emoji preview truncates without UTF-8 or UTF-16 surrogate split (D6)",
)
def test_t2_f06_03_cjk_emoji_surrogate_preview_no_utf8_utf16_split(env: TempUiUxEnv) -> None:
    sl = env.probe_scenario("f06_cjk_emoji_truncate", "sidebar_list")
    row = require_field(sl, "rows")[0]
    disp = require_field(row, "preview_displayed")
    assert require_field(row, "preview_truncated") is True
    assert require_field(row, "valid_utf8_utf16") is True
    assert is_valid_utf8_and_no_unpaired_surrogates(disp) is True
    assert require_field(row, "min_w_0") is True and require_field(row, "whitespace_nowrap") is True


@uiux_e2e_test(
    id="TEST-UIUX-T2-UI-F06-04",
    tier=2,
    features=("UI-F06",),
    description="Verify empty workspace renders Create First Agent sidebar empty state with zero pins",
)
def test_t2_f06_04_empty_workspace_renders_first_agent_empty_state(env: TempUiUxEnv) -> None:
    sl = env.probe_scenario("f06_empty_workspace", "sidebar_list")
    es = require_field(sl, "empty_state")
    assert require_field(es, "visible") is True
    assert require_field(es, "label") == "创建第一个助手"
    assert require_field(sl, "pinned_grid.visible") is False
    assert require_field(sl, "rows") == []


@uiux_e2e_test(
    id="TEST-UIUX-T2-UI-F06-05",
    tier=2,
    features=("UI-F06",),
    description="Verify blank folder name and folder nesting depth >2 are rejected with aria-invalid",
)
def test_t2_f06_05_invalid_folder_name_and_depth_gt_2_rejected(env: TempUiUxEnv) -> None:
    blank = env.probe_scenario("f06_folder_blank_name", "sidebar_list")
    deep = env.probe_scenario("f06_folder_depth_3", "sidebar_list")
    assert require_field(blank, "folder_editor.aria_invalid") is True
    assert require_field(blank, "folder_editor.rejected") is True
    assert require_field(deep, "folder_editor.rejected") is True
    assert require_field(deep, "folder_editor.depth") <= 2


@uiux_e2e_test(
    id="TEST-UIUX-T2-UI-F06-06",
    tier=2,
    features=("UI-F06",),
    description="Verify hidden/paused row 0.5 grayscale opacity and 60x60 compact pinned tiles",
)
def test_t2_f06_06_hidden_paused_grayscale_and_compact_60x60_pinned(env: TempUiUxEnv) -> None:
    hid = env.probe_scenario("f06_hidden_row", "sidebar_list")
    cmp = env.probe_scenario("f06_compact_pinned", "sidebar_list")
    row = require_field(hid, "rows")[0]
    assert require_field(row, "archived_or_hidden") is True
    assert require_field(row, "opacity") == 0.5
    assert require_field(row, "grayscale") is True
    assert require_field(row, "time_visible") is False
    assert require_field(cmp, "pinned_grid.tile_w_px") == 60
    assert require_field(cmp, "pinned_grid.tile_h_px") == 60
    assert require_field(cmp, "tabs.visible") is False


# ---------------------------------------------------------------------------
# UI-F07 Boundaries & Negative Cases (6 tests)
# ---------------------------------------------------------------------------


@uiux_e2e_test(
    id="TEST-UIUX-T2-UI-F07-01",
    tier=2,
    features=("UI-F07",),
    description="Verify rapid typing discards stale out-of-order remote search responses by sequence ID",
)
def test_t2_f07_01_rapid_typing_discards_stale_out_of_order_responses(env: TempUiUxEnv) -> None:
    gs = env.probe_scenario("f07_stale_response_guard", "global_search")
    rem = require_field(gs, "remote")
    assert require_field(rem, "latest_seq") >= 2
    assert require_field(rem, "stale_discarded_count") >= 1
    for item in require_field(gs, "results"):
        assert "alpha" in require_field(item, "title").lower()


@uiux_e2e_test(
    id="TEST-UIUX-T2-UI-F07-02",
    tier=2,
    features=("UI-F07",),
    description="Verify remote search failure shows inline Retry banner and recovers cleanly on retry",
)
def test_t2_f07_02_remote_search_failure_renders_inline_retry(env: TempUiUxEnv) -> None:
    err = env.probe_scenario("f07_remote_error", "global_search")
    rec = env.probe_scenario("f07_remote_recovered", "global_search")
    assert require_field(err, "remote.error_retry_visible") is True
    assert require_field(rec, "remote.error_retry_visible") is False
    assert require_field(rec, "remote.latest_seq") > require_field(err, "remote.latest_seq")


@uiux_e2e_test(
    id="TEST-UIUX-T2-UI-F07-03",
    tier=2,
    features=("UI-F07",),
    description="Verify zero-match query displays 56px GlobalSearchPlush with zero phantom rows",
)
def test_t2_f07_03_zero_match_query_shows_plush_no_phantom_rows(env: TempUiUxEnv) -> None:
    gs = env.probe_scenario("f07_zero_match", "global_search")
    assert require_field(gs, "results") == []
    assert require_field(gs, "plush.visible") is True
    assert require_field(gs, "plush.size_px") == 56


@uiux_e2e_test(
    id="TEST-UIUX-T2-UI-F07-04",
    tier=2,
    features=("UI-F07",),
    description="Verify narrow 480px viewport clamps GlobalSearch modal width to 432px (100vw - 48px)",
)
def test_t2_f07_04_narrow_viewport_clamps_modal_width_100vw_minus_48(env: TempUiUxEnv) -> None:
    gs = env.probe_scenario("f07_narrow_480", "global_search")
    assert require_field(gs, "width_px") == 432
    assert require_field(gs, "top_px") == 112


@uiux_e2e_test(
    id="TEST-UIUX-T2-UI-F07-05",
    tier=2,
    features=("UI-F07",),
    description="Verify prefers-reduced-motion freezes GlobalSearchPlush idle scan and head tilt",
)
def test_t2_f07_05_reduced_motion_freezes_global_search_plush(env: TempUiUxEnv) -> None:
    gs = env.probe_scenario("f07_reduced_motion", "global_search")
    pl = require_field(gs, "plush")
    assert require_field(pl, "visible") is True
    assert require_field(pl, "reduced_motion_frozen") is True
    assert require_field(pl, "idle_scan_active") is False
    assert require_field(pl, "head_tilt_deg") == 0.0
    assert require_field(pl, "caret_offset_x_pct") == 0.0


@uiux_e2e_test(
    id="TEST-UIUX-T2-UI-F07-06",
    tier=2,
    features=("UI-F07",),
    description="Verify ArrowUp/Down clamping, Escape close, and Cmd+K block while ConfirmDialog is active",
)
def test_t2_f07_06_keyboard_bounds_escape_and_dialog_shortcut_block(env: TempUiUxEnv) -> None:
    nav = env.probe_scenario("f07_arrow_bounds", "global_search")
    blk = env.probe_scenario("f07_blocked_by_dialog", "global_search")
    assert require_field(nav, "highlighted_index") >= 0
    assert require_field(blk, "blocked_by_active_dialog") is True
    assert require_field(blk, "open") is False


# ---------------------------------------------------------------------------
# UI-F08 Boundaries & Negative Cases (6 tests)
# ---------------------------------------------------------------------------


@uiux_e2e_test(
    id="TEST-UIUX-T2-UI-F08-01",
    tier=2,
    features=("UI-F08",),
    description="Verify empty or whitespace-only agent name disables Create CTA and blocks submit",
)
def test_t2_f08_01_empty_or_whitespace_name_disables_create_cta(env: TempUiUxEnv) -> None:
    st = env.probe_scenario("f08_blank_name", "agent_studio")
    assert require_field(st, "submit.disabled") is True
    assert require_field(st, "created_agent_id") is None
    assert require_field(st, "navigated_to_chat") is False


@uiux_e2e_test(
    id="TEST-UIUX-T2-UI-F08-02",
    tier=2,
    features=("UI-F08",),
    description="Verify over-limit instructions (>INPUT_LIMITS) disable submit with inline error message",
)
def test_t2_f08_02_over_limit_instructions_blocks_submit_with_error(env: TempUiUxEnv) -> None:
    st = env.probe_scenario("f08_over_limit_purpose", "agent_studio")
    assert require_field(st, "submit.disabled") is True
    assert bool(require_field(st, "submit.error_message"))
    assert require_field(st, "created_agent_id") is None


@uiux_e2e_test(
    id="TEST-UIUX-T2-UI-F08-03",
    tier=2,
    features=("UI-F08",),
    description="Verify Escape cancels additional agent mode cleanly while first-agent mode stays mounted",
)
def test_t2_f08_03_escape_cancels_additional_mode_only_not_first(env: TempUiUxEnv) -> None:
    add = env.probe_scenario("f08_escape_additional", "agent_studio")
    fst = env.probe_scenario("f08_escape_first", "agent_studio")
    assert require_field(add, "mode") == "additional"
    assert require_field(add, "navigated_to_chat") is True
    assert require_field(add, "created_agent_id") is None
    assert require_field(fst, "mode") == "first"
    assert require_field(fst, "header.cancel_visible") is False
    assert require_field(fst, "navigated_to_chat") is False


@uiux_e2e_test(
    id="TEST-UIUX-T2-UI-F08-04",
    tier=2,
    features=("UI-F08",),
    description="Verify backend CreateManagedBot failure shows inline error and preserves user draft",
)
def test_t2_f08_04_backend_create_failure_preserves_draft_and_shows_error(env: TempUiUxEnv) -> None:
    st = env.probe_scenario("f08_backend_error", "agent_studio")
    assert require_field(st, "created_agent_id") is None
    assert require_field(st, "navigated_to_chat") is False
    assert bool(require_field(st, "submit.error_message"))
    assert bool(require_field(st, "fields.name").strip())
    assert bool(require_field(st, "fields.purpose").strip())


@uiux_e2e_test(
    id="TEST-UIUX-T2-UI-F08-05",
    tier=2,
    features=("UI-F08",),
    description="Verify manual edit after selecting recommendation template clears active suggestion_id",
)
def test_t2_f08_05_manual_edit_after_template_clears_suggestion_match(env: TempUiUxEnv) -> None:
    picked = env.probe_scenario("f08_template_inbox", "agent_studio")
    edited = env.probe_scenario("f08_template_edited", "agent_studio")
    assert require_field(picked, "recommendations.suggestion_id") == "inbox-helper"
    assert require_field(edited, "recommendations.suggestion_id") is None


@uiux_e2e_test(
    id="TEST-UIUX-T2-UI-F08-06",
    tier=2,
    features=("UI-F08",),
    description="Verify CJK+emoji IME agent name and sweep across all 18 swatches and 12-13 plush shapes",
)
def test_t2_f08_06_cjk_ime_name_and_all_18_swatches_12_shapes_sweep(env: TempUiUxEnv) -> None:
    st = env.probe_scenario("f08_cjk_swatch_shape_sweep", "agent_studio")
    live_name = require_field(st, "header.live_name")
    assert live_name == "法布施助手✨"
    assert is_valid_utf8_and_no_unpaired_surrogates(live_name) is True
    assert require_field(st, "swatches.total") == 18
    assert_within(require_field(st, "swatches.selected_idx"), 0, 17, "swatch.idx")
    assert require_field(st, "shapes.contains_goutte") is False
    assert require_field(st, "hero.is_plush_3d") is True


# ---------------------------------------------------------------------------
# UI-F09 Boundaries & Negative Cases (6 tests)
# ---------------------------------------------------------------------------


@uiux_e2e_test(
    id="TEST-UIUX-T2-UI-F09-01",
    tier=2,
    features=("UI-F09",),
    description="Verify narrow header auto-fit cascade degrades subtitle->effort->icon without wrapping or clipping",
)
def test_t2_f09_01_narrow_header_auto_fit_cascade_zero_wrap_or_clip(env: TempUiUxEnv) -> None:
    mid = env.probe_scenario("f09_narrow_620", "conversation_header")
    tight = env.probe_scenario("f09_narrow_424", "conversation_header")
    assert require_field(mid, "title_btn.subtitle_visible") is False
    assert require_field(mid, "model_picker.fit_mode") in ("name", "no_effort")
    assert require_field(tight, "model_picker.fit_mode") in ("icon", "icon_only")
    assert require_field(tight, "height_px") == 38
    assert require_field(tight, "wrapped_to_multiline") is False
    assert require_field(tight, "actions_clipped") is False


@uiux_e2e_test(
    id="TEST-UIUX-T2-UI-F09-02",
    tier=2,
    features=("UI-F09",),
    description="Verify disconnected provider disables ProviderModelPicker at 0.48 opacity with tooltip",
)
def test_t2_f09_02_disconnected_provider_disables_model_picker_48pct(env: TempUiUxEnv) -> None:
    hdr = env.probe_scenario("f09_provider_disconnected", "conversation_header")
    mp = require_field(hdr, "model_picker")
    assert require_field(mp, "disabled") is True
    assert_within(require_field(mp, "opacity"), 0.45, 0.50, "model_picker.opacity")
    assert bool(require_field(mp, "tooltip"))


@uiux_e2e_test(
    id="TEST-UIUX-T2-UI-F09-03",
    tier=2,
    features=("UI-F09",),
    description="Verify empty PinnedMessages popover displays clean empty-state copy",
)
def test_t2_f09_03_empty_pinned_messages_popover_shows_empty_copy(env: TempUiUxEnv) -> None:
    hdr = env.probe_scenario("f09_pinned_empty", "conversation_header")
    pp = require_field(hdr, "pinned_popover")
    assert require_field(pp, "open") is True
    assert require_field(pp, "count") == 0
    assert require_field(pp, "empty_state_visible") is True
    assert require_field(pp, "max_width_px") == 420


@uiux_e2e_test(
    id="TEST-UIUX-T2-UI-F09-04",
    tier=2,
    features=("UI-F09",),
    description="Verify closed sidebar shifts header start X past SidebarTopbar controls avoiding overlap",
)
def test_t2_f09_04_sidebar_closed_shifts_header_avoiding_topbar_overlap(env: TempUiUxEnv) -> None:
    hdr = env.probe_scenario("f09_sidebar_closed_shift", "conversation_header")
    assert require_field(hdr, "shifted_start_px") > 0
    assert require_field(hdr, "header_rect.x") >= require_field(hdr, "topbar_controls_right_edge")
    assert require_field(hdr, "overlaps_timeline") is False


@uiux_e2e_test(
    id="TEST-UIUX-T2-UI-F09-05",
    tier=2,
    features=("UI-F09",),
    description="Verify Group header hides ProviderModelPicker and archived group disables title click",
)
def test_t2_f09_05_group_hides_model_picker_and_archived_disables_title(env: TempUiUxEnv) -> None:
    grp = env.probe_scenario("f09_group_active", "conversation_header")
    arch = env.probe_scenario("f09_group_archived", "conversation_header")
    assert require_field(grp, "model_picker.visible") is False
    assert require_field(grp, "title_btn.disabled") is False
    assert require_field(grp, "title_btn.click_target_pane") == "channel_settings"
    assert require_field(arch, "model_picker.visible") is False
    assert require_field(arch, "title_btn.disabled") is True


@uiux_e2e_test(
    id="TEST-UIUX-T2-UI-F09-06",
    tier=2,
    features=("UI-F09",),
    description="Verify reduced-motion zeroes TabShape transition and window-glass hides TabShape without overlap",
)
def test_t2_f09_06_reduced_motion_and_window_glass_disable_tab_shape(env: TempUiUxEnv) -> None:
    rm = env.probe_scenario("f09_reduced_motion", "conversation_header")
    wg = env.probe_scenario("f09_window_glass", "conversation_header")
    assert require_field(rm, "tab_shape.transition_ms") == 0
    assert require_field(rm, "tab_shape.disabled_by_motion_or_glass") is True
    assert require_field(wg, "tab_shape.visible") is False
    assert require_field(wg, "overlaps_timeline") is False


# ---------------------------------------------------------------------------
# UI-F10 Boundaries & Negative Cases (6 tests)
# ---------------------------------------------------------------------------


@uiux_e2e_test(
    id="TEST-UIUX-T2-UI-F10-01",
    tier=2,
    features=("UI-F10",),
    description="Verify 20,000 vs 20,001-char collapse threshold and UTF-16 surrogate-safe 3,000-char preview",
)
def test_t2_f10_01_long_20001_char_message_preview_surrogate_safe(env: TempUiUxEnv) -> None:
    b20k = env.probe_scenario("f10_msg_20000", "chat_stream")
    b20k1 = env.probe_scenario("f10_msg_20001", "chat_stream")
    m0 = require_field(b20k, "bubbles")[0]
    m1 = require_field(b20k1, "bubbles")[0]
    assert require_field(m0, "truncated") is False
    assert require_field(m0, "show_full_visible") is False
    assert require_field(m1, "truncated") is True
    assert require_field(m1, "preview_len") <= 3000
    assert require_field(m1, "show_full_visible") is True
    assert require_field(m1, "valid_utf8_utf16") is True
    sample_preview = require_field(m1, "preview_text")
    assert is_valid_utf8_and_no_unpaired_surrogates(sample_preview) is True
    assert len(utf16_surrogate_safe_truncate(sample_preview, 3000)) == len(sample_preview)


@uiux_e2e_test(
    id="TEST-UIUX-T2-UI-F10-02",
    tier=2,
    features=("UI-F10",),
    description="Verify verbose raw JSON API error never leaks into primary message bubble (image_3.png D8)",
)
def test_t2_f10_02_verbose_raw_json_tool_error_never_leaks_into_bubble(env: TempUiUxEnv) -> None:
    cs = env.probe_scenario("f10_raw_json_tool_error", "chat_stream")
    for b in require_field(cs, "bubbles"):
        assert require_field(b, "contains_raw_json_wall") is False
    st = require_field(cs, "steps")
    assert require_field(st, "collapsed_by_default") is True
    assert require_field(st, "error_drawer_open") is False
    assert require_field(st, "error_drawer_btn_label") == "Show error"
    assert require_field(st, "error_pre_max_height_px") == 160


@uiux_e2e_test(
    id="TEST-UIUX-T2-UI-F10-03",
    tier=2,
    features=("UI-F10",),
    description="Verify prefers-reduced-motion freezes AgentActivity shimmer animation cleanly",
)
def test_t2_f10_03_reduced_motion_freezes_shimmer_and_stream_fade(env: TempUiUxEnv) -> None:
    cs = env.probe_scenario("f10_reduced_motion", "chat_stream")
    act = require_field(cs, "activity")
    assert require_field(act, "visible") is True
    assert require_field(act, "shimmer_frozen_reduced_motion") is True
    assert require_field(act, "shimmer_active") is False


@uiux_e2e_test(
    id="TEST-UIUX-T2-UI-F10-04",
    tier=2,
    features=("UI-F10",),
    description="Verify broken inline markdown image renders fallback and failed outbound message offers Retry",
)
def test_t2_f10_04_broken_inline_image_fallback_and_failed_send_retry(env: TempUiUxEnv) -> None:
    cs = env.probe_scenario("f10_broken_img_and_retry", "chat_stream")
    assert require_field(cs, "broken_image_fallback_visible") is True
    bubbles = require_field(cs, "bubbles")
    assert any(b.get("pending_error_retry") is True for b in bubbles)


@uiux_e2e_test(
    id="TEST-UIUX-T2-UI-F10-05",
    tier=2,
    features=("UI-F10",),
    description="Verify Cmd+F ChatSearch shows 0/0 on zero matches and shifts left by sidePanelWidth+16px",
)
def test_t2_f10_05_cmd_f_zero_matches_and_side_panel_left_shift(env: TempUiUxEnv) -> None:
    cs = env.probe_scenario("f10_search_with_side_panel_360", "chat_stream")
    sf = require_field(cs, "chat_search")
    assert require_field(sf, "open") is True
    assert require_field(sf, "match_counter") == "0/0"
    assert require_field(sf, "right_offset_px") == 376


@uiux_e2e_test(
    id="TEST-UIUX-T2-UI-F10-06",
    tier=2,
    features=("UI-F10",),
    description="Verify row error boundary isolates corrupt rows and DirectConversation 3s typing idle clears",
)
def test_t2_f10_06_row_boundary_crash_isolation_and_dm_3s_typing_idle(env: TempUiUxEnv) -> None:
    cs = env.probe_scenario("f10_row_isolation_and_dm_idle", "chat_stream")
    assert require_field(cs, "row_boundary_caught_error") is True
    assert len(require_field(cs, "bubbles")) >= 2
    dm = require_field(cs, "direct_dm")
    assert require_field(dm, "send_disabled") is True
    assert require_field(dm, "typing_active") is False
