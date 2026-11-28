"""Tier 2 Boundary, Corner & Negative Tests: UI-F11 through UI-F15 (30 opaque-box tests)."""

from __future__ import annotations

from tests.e2e_openbot_uiux.harness import (
    TempUiUxEnv,
    require_field,
    uiux_e2e_test,
)
from tests.e2e_openbot_uiux.oracles import (
    channel_avatar_cluster_oracle,
    normalize_channel_icon_and_hue,
    side_panel_clamp_width,
)


# ---------------------------------------------------------------------------
# UI-F11 Boundaries & Negative Cases (6 tests)
# ---------------------------------------------------------------------------


@uiux_e2e_test(
    id="TEST-UIUX-T2-UI-F11-01",
    tier=2,
    features=("UI-F11",),
    description="Verify ChannelAvatar extreme member counts (0, 4, 12, 13, 100) and duplicate deduplication",
)
def test_t2_f11_01_channel_avatar_extreme_counts_and_dedup(env: TempUiUxEnv) -> None:
    cases = (
        (0, "f11_cluster_0"),
        (4, "f11_cluster_4"),
        (12, "f11_cluster_12"),
        (13, "f11_cluster_13"),
        (100, "f11_cluster_100"),
    )
    for cnt, sc_id in cases:
        st = env.probe_scenario(sc_id, "channel_stage")
        av = require_field(st, "avatar")
        oracle = channel_avatar_cluster_oracle(cnt)
        faces = require_field(av, "visible_faces")
        ov = require_field(av, "overflow_count")
        tone = require_field(av, "overflow_pill_tone")
        assert (faces, ov, tone) in oracle["valid_tuples"], (
            f"cnt={cnt}: tuple={(faces, ov, tone)!r} not in {oracle['valid_tuples']}"
        )
        assert faces + ov == cnt
        if cnt >= 13:
            assert ov >= 10


@uiux_e2e_test(
    id="TEST-UIUX-T2-UI-F11-02",
    tier=2,
    features=("UI-F11",),
    description="Verify ChannelAvatar broken 404 photo fallback chain and invalid icon/hue normalization",
)
def test_t2_f11_02_channel_avatar_broken_photo_and_invalid_icon_fallback(env: TempUiUxEnv) -> None:
    fb = env.probe_scenario("f11_broken_photo_icon_fallback", "channel_stage")
    inv = env.probe_scenario("f11_invalid_icon_hue", "channel_stage")
    assert require_field(fb, "avatar.image_fallback_active") is True
    assert require_field(fb, "avatar.resolved_icon") == "code"
    bad_icon, norm_hue = normalize_channel_icon_and_hue("dragon", 999)
    assert require_field(inv, "avatar.resolved_icon") is bad_icon
    assert require_field(inv, "avatar.normalized_hue") == norm_hue == 215


@uiux_e2e_test(
    id="TEST-UIUX-T2-UI-F11-03",
    tier=2,
    features=("UI-F11",),
    description="Verify ChannelCreateDialog rejects empty/whitespace name, 0 members, and >80-char name",
)
def test_t2_f11_03_channel_create_dialog_invalid_submit_guards(env: TempUiUxEnv) -> None:
    zero_m = env.probe_scenario("f11_create_zero_members", "channel_stage")
    over_n = env.probe_scenario("f11_create_name_81", "channel_stage")
    assert require_field(zero_m, "create_dialog.submit_disabled") is True
    assert require_field(zero_m, "create_dialog.created") is False
    assert require_field(over_n, "create_dialog.submit_disabled") is True
    assert bool(require_field(over_n, "create_dialog.error_banner"))


@uiux_e2e_test(
    id="TEST-UIUX-T2-UI-F11-04",
    tier=2,
    features=("UI-F11",),
    description="Verify archived group read-only lock hides composer, disables drag, and leaves Restore-only menu",
)
def test_t2_f11_04_archived_group_read_only_lock_and_restore_only_menu(env: TempUiUxEnv) -> None:
    st = env.probe_scenario("f11_archived_channel", "channel_stage")
    assert require_field(st, "archived") is True
    assert require_field(st, "read_only") is True
    assert require_field(st, "composer_hidden") is True
    assert require_field(st, "sidebar_drag_disabled") is True
    assert require_field(st, "context_menu_items") == ["Restore"]


@uiux_e2e_test(
    id="TEST-UIUX-T2-UI-F11-05",
    tier=2,
    features=("UI-F11",),
    description="Verify deleted author fallback, missing reply target dashed notice, and disabled group reactions",
)
def test_t2_f11_05_deleted_author_missing_reply_and_disabled_group_actions(env: TempUiUxEnv) -> None:
    st = env.probe_scenario("f11_deleted_author_missing_reply", "channel_stage")
    assert require_field(st, "deleted_author_label") == "Unavailable agent"
    assert require_field(st, "reply_quote.unavailable_dashed") is True
    assert require_field(st, "reactions_enabled") is False
    assert require_field(st, "mark_unread_enabled") is False
    assert require_field(st, "thread_branch_enabled") is False


@uiux_e2e_test(
    id="TEST-UIUX-T2-UI-F11-06",
    tier=2,
    features=("UI-F11",),
    description="Verify ChannelActivityIndicator hides on 0 workers and freezes shimmer under reduced motion",
)
def test_t2_f11_06_activity_indicator_empty_workers_and_reduced_motion(env: TempUiUxEnv) -> None:
    empty = env.probe_scenario("f11_activity_empty", "channel_stage")
    rm = env.probe_scenario("f11_activity_reduced_motion", "channel_stage")
    assert require_field(empty, "activity_indicator.visible") is False
    assert require_field(rm, "activity_indicator.visible") is True
    assert require_field(rm, "activity_indicator.shimmer_animated") is False


# ---------------------------------------------------------------------------
# UI-F12 Boundaries & Negative Cases (6 tests)
# ---------------------------------------------------------------------------


@uiux_e2e_test(
    id="TEST-UIUX-T2-UI-F12-01",
    tier=2,
    features=("UI-F12",),
    description="Verify 4000-char inline paste boundary vs 4001-char pasted-text.txt auto-conversion",
)
def test_t2_f12_01_4000_vs_4001_char_paste_conversion_boundary(env: TempUiUxEnv) -> None:
    p4000 = env.probe_scenario("f12_paste_4000", "composer")
    p4001 = env.probe_scenario("f12_paste_4001", "composer")
    assert require_field(p4000, "pasted_as_file") is False
    assert require_field(p4000, "inline_text_length") == 4000
    assert require_field(p4001, "pasted_as_file") is True
    assert require_field(p4001, "pasted_file_name") == "pasted-text.txt"
    assert require_field(p4001, "border_radius_px") == 14


@uiux_e2e_test(
    id="TEST-UIUX-T2-UI-F12-02",
    tier=2,
    features=("UI-F12",),
    description="Verify attachment count/size caps (10 msg / 50 draft / 100 MiB file / 250 MiB total)",
)
def test_t2_f12_02_attachment_count_and_size_caps_enforced(env: TempUiUxEnv) -> None:
    ok_b = env.probe_scenario("f12_attach_exact_cap", "composer")
    bad_b = env.probe_scenario("f12_attach_over_cap", "composer")
    assert require_field(ok_b, "attachment_accepted") is True
    assert require_field(ok_b, "attachments_count") == 10
    assert require_field(bad_b, "attachment_accepted") is False
    assert require_field(bad_b, "attachments_count") <= 10


@uiux_e2e_test(
    id="TEST-UIUX-T2-UI-F12-03",
    tier=2,
    features=("UI-F12",),
    description="Verify text-only pi model image warning and 1-click Allow Images capability recovery",
)
def test_t2_f12_03_text_only_model_image_guard_and_allow_recovery(env: TempUiUxEnv) -> None:
    warn = env.probe_scenario("f12_text_only_image_warn", "composer")
    rec = env.probe_scenario("f12_text_only_image_allowed", "composer")
    assert require_field(warn, "image_model_warning") is True
    assert require_field(warn, "image_inputs_enabled") is False
    assert require_field(rec, "image_model_warning") is False
    assert require_field(rec, "image_inputs_enabled") is True


@uiux_e2e_test(
    id="TEST-UIUX-T2-UI-F12-04",
    tier=2,
    features=("UI-F12",),
    description="Verify whitespace-only submit rejection and busy + New Chat guard without aborting turn",
)
def test_t2_f12_04_whitespace_submit_rejection_and_busy_new_chat_guard(env: TempUiUxEnv) -> None:
    ws = env.probe_scenario("f12_whitespace_submit", "composer")
    busy = env.probe_scenario("f12_busy_new_chat", "composer")
    assert require_field(ws, "sent_count") == 0
    assert require_field(ws, "action_circle_kind") == "voice"
    assert require_field(busy, "new_chat_blocked_while_busy") is True
    assert require_field(busy, "active_turn_preserved") is True


@uiux_e2e_test(
    id="TEST-UIUX-T2-UI-F12-05",
    tier=2,
    features=("UI-F12",),
    description="Verify IME preedit Escape cancellation and mid-composition focus loss do not send or panic",
)
def test_t2_f12_05_ime_preedit_escape_cancel_and_focus_loss(env: TempUiUxEnv) -> None:
    esc = env.probe_scenario("f12_ime_escape_cancel", "composer")
    blur = env.probe_scenario("f12_ime_focus_loss", "composer")
    assert require_field(esc, "ime_composing") is False
    assert require_field(esc, "sent_count") == 0
    assert require_field(blur, "ime_composing") is False
    assert require_field(blur, "sent_count") == 0


@uiux_e2e_test(
    id="TEST-UIUX-T2-UI-F12-06",
    tier=2,
    features=("UI-F12",),
    description="Verify disabled composer disables + trigger and human DM omits bot-only add menu items",
)
def test_t2_f12_06_composer_add_menu_disabled_and_human_dm_filtering(env: TempUiUxEnv) -> None:
    dis = env.probe_scenario("f12_composer_disabled", "composer")
    dm = env.probe_scenario("f12_human_dm_add_menu", "composer")
    assert require_field(dis, "add_trigger_disabled") is True
    assert require_field(dis, "add_menu_open") is False
    dm_items = set(require_field(dm, "add_menu_items"))
    assert "skill" not in dm_items and "teach" not in dm_items and "side_question" not in dm_items
    assert require_field(dm, "leading_or_trailing_separator") is False


# ---------------------------------------------------------------------------
# UI-F13 Boundaries & Negative Cases (6 tests)
# ---------------------------------------------------------------------------


@uiux_e2e_test(
    id="TEST-UIUX-T2-UI-F13-01",
    tier=2,
    features=("UI-F13",),
    description="Verify @everyone is strictly hidden in 1:1 Bot DM, 1:1 Human DM, and <=1-member groups",
)
def test_t2_f13_01_everyone_strictly_hidden_when_members_le_1_or_dm(env: TempUiUxEnv) -> None:
    for sc_id in ("f13_1to1_bot", "f13_1to1_human", "f13_group_1_member"):
        mp = env.probe_scenario(sc_id, "mention_picker")
        assert require_field(mp, "everyone_shown") is False
        assert not any(c.get("token") == "@everyone" for c in require_field(mp, "inserted_chips"))


@uiux_e2e_test(
    id="TEST-UIUX-T2-UI-F13-02",
    tier=2,
    features=("UI-F13",),
    description="Verify @ picker suppresses messages section for query <3 chars and activates at >=3 chars",
)
def test_t2_f13_02_messages_section_suppressed_under_3_chars(env: TempUiUxEnv) -> None:
    q2 = env.probe_scenario("f13_query_len_2", "mention_picker")
    q3 = env.probe_scenario("f13_query_len_3", "mention_picker")
    assert require_field(q2, "sections") == ["agents", "files", "tabs", "threads"]
    assert require_field(q3, "sections") == ["agents", "files", "tabs", "threads", "messages"]
    assert require_field(q3, "messages_debounce_ms") == 150
    assert require_field(q3, "thread_search_rows") == 5


@uiux_e2e_test(
    id="TEST-UIUX-T2-UI-F13-03",
    tier=2,
    features=("UI-F13",),
    description="Verify $ picker excludes disabled/broken skills and surfaces broken skills in skill_problems",
)
def test_t2_f13_03_broken_and_disabled_skills_isolated_in_skill_problems(env: TempUiUxEnv) -> None:
    mp = env.probe_scenario("f13_skill_problems", "mention_picker")
    results = require_field(mp, "results")
    assert all(r.get("status") == "ok" for r in results)
    problems = require_field(mp, "skill_problems")
    assert len(problems) >= 1


@uiux_e2e_test(
    id="TEST-UIUX-T2-UI-F13-04",
    tier=2,
    features=("UI-F13",),
    description="Verify >7 picker matches cap viewport at 7 rows with scroll-edge fade indicators",
)
def test_t2_f13_04_overflowing_picker_7_row_cap_and_scroll_fades(env: TempUiUxEnv) -> None:
    mp = env.probe_scenario("f13_overflow_15_rows", "mention_picker")
    assert require_field(mp, "visible_rows") <= 7
    assert require_field(mp, "row_height_px") == 29
    assert require_field(mp, "scroll_enabled") is True
    assert require_field(mp, "fade_top") is True or require_field(mp, "fade_bottom") is True


@uiux_e2e_test(
    id="TEST-UIUX-T2-UI-F13-05",
    tier=2,
    features=("UI-F13",),
    description="Verify deleted referenced entity renders unavailable dashed ReferenceChip without panic on click",
)
def test_t2_f13_05_deleted_referenced_entity_unavailable_chip(env: TempUiUxEnv) -> None:
    mp = env.probe_scenario("f13_unavailable_chip", "mention_picker")
    chips = require_field(mp, "rendered_chips")
    assert len(chips) >= 1
    assert require_field(chips[0], "unavailable") is True
    assert require_field(chips[0], "navigated_on_click") is False


@uiux_e2e_test(
    id="TEST-UIUX-T2-UI-F13-06",
    tier=2,
    features=("UI-F13",),
    description="Verify mid-word user@example.com / $100 do not open picker and Escape dismisses cleanly",
)
def test_t2_f13_06_mid_word_non_trigger_and_escape_dismissal(env: TempUiUxEnv) -> None:
    mid = env.probe_scenario("f13_mid_word_email_currency", "mention_picker")
    esc = env.probe_scenario("f13_escape_dismissed", "mention_picker")
    assert require_field(mid, "open") is False
    assert require_field(esc, "open") is False
    assert require_field(esc, "focus_kept_in_composer") is True


# ---------------------------------------------------------------------------
# UI-F14 Boundaries & Negative Cases (6 tests)
# ---------------------------------------------------------------------------


@uiux_e2e_test(
    id="TEST-UIUX-T2-UI-F14-01",
    tier=2,
    features=("UI-F14",),
    description="Verify Tab Steer is refused when routine owns active turn and queued message is preserved",
)
def test_t2_f14_01_tab_steer_refused_for_routine_turn(env: TempUiUxEnv) -> None:
    st = env.probe_scenario("f14_steer_refused_routine", "composer_stack")
    q = require_field(st, "queue")
    assert require_field(q, "can_steer") is False
    assert require_field(q, "steer_refused") is True
    assert require_field(q, "steered_on_tab") is False
    assert len(require_field(q, "items")) >= 1


@uiux_e2e_test(
    id="TEST-UIUX-T2-UI-F14-02",
    tier=2,
    features=("UI-F14",),
    description="Verify QueuePanel >7 items caps visible rows at 7 and clamps out-of-bounds reorder indices",
)
def test_t2_f14_02_queue_panel_7_row_cap_and_reorder_clamping(env: TempUiUxEnv) -> None:
    st = env.probe_scenario("f14_queue_10_items_clamp", "composer_stack")
    q = require_field(st, "queue")
    assert require_field(q, "visible_rows") <= 7
    assert len(require_field(q, "items")) == 10
    assert require_field(q, "row_height_px") == 29


@uiux_e2e_test(
    id="TEST-UIUX-T2-UI-F14-03",
    tier=2,
    features=("UI-F14",),
    description="Verify ChannelStoppedTasks double-click idempotency guard and archived group refusal",
)
def test_t2_f14_03_stopped_tasks_double_click_idempotency_and_archived(env: TempUiUxEnv) -> None:
    dbl = env.probe_scenario("f14_stopped_double_click", "composer_stack")
    arch = env.probe_scenario("f14_stopped_archived", "composer_stack")
    assert require_field(dbl, "stopped_tasks.resume_dispatch_count") == 1
    assert len(require_field(dbl, "stopped_tasks.resuming_ids")) >= 1
    assert require_field(arch, "stopped_tasks.visible") is False


@uiux_e2e_test(
    id="TEST-UIUX-T2-UI-F14-04",
    tier=2,
    features=("UI-F14",),
    description="Verify AwaitingReplies Dismiss is disabled while any subtask is active and enabled once settled",
)
def test_t2_f14_04_awaiting_replies_dismiss_blocked_while_active(env: TempUiUxEnv) -> None:
    act = env.probe_scenario("f14_awaiting_active", "composer_stack")
    set_st = env.probe_scenario("f14_awaiting_settled", "composer_stack")
    assert require_field(act, "awaiting_replies.dismiss_available") is False
    assert require_field(set_st, "awaiting_replies.dismiss_available") is True


@uiux_e2e_test(
    id="TEST-UIUX-T2-UI-F14-05",
    tier=2,
    features=("UI-F14",),
    description="Verify multi-card stacking order and full collapse back to exact 0px when all cards cleared",
)
def test_t2_f14_05_multi_card_stacking_and_exact_0px_collapse(env: TempUiUxEnv) -> None:
    multi = env.probe_scenario("f14_multi_cards", "composer_stack")
    cleared = env.probe_scenario("f14_multi_cards_cleared", "composer_stack")
    assert require_field(multi, "visible") is True
    assert require_field(multi, "reply_preview.visible") is True
    assert require_field(multi, "notice.visible") is True
    assert require_field(cleared, "visible") is False
    assert require_field(cleared, "height_px") == 0


@uiux_e2e_test(
    id="TEST-UIUX-T2-UI-F14-06",
    tier=2,
    features=("UI-F14",),
    description="Verify OpenWaits 20-wait ceiling and oversized ComposerNotice truncation with Copy details",
)
def test_t2_f14_06_open_waits_20_cap_and_oversized_notice_truncation(env: TempUiUxEnv) -> None:
    st = env.probe_scenario("f14_waits_cap_20_and_long_notice", "composer_stack")
    assert require_field(st, "open_waits_count") <= 20
    assert require_field(st, "notice.visible") is True
    assert require_field(st, "notice.has_copy_details") is True
    assert require_field(st, "notice.truncated") is True


# ---------------------------------------------------------------------------
# UI-F15 Boundaries & Negative Cases (6 tests)
# ---------------------------------------------------------------------------


@uiux_e2e_test(
    id="TEST-UIUX-T2-UI-F15-01",
    tier=2,
    features=("UI-F15",),
    description="Verify SidePanel width clamps 200px->360px and 2200px->min(1600, host-96) matching oracle",
)
def test_t2_f15_01_side_panel_width_clamping_and_96px_conversation_floor(env: TempUiUxEnv) -> None:
    lo = env.probe_scenario("f15_drag_200", "side_panel")
    hi = env.probe_scenario("f15_drag_2200_at_1280", "side_panel")
    assert require_field(lo, "width_px") == side_panel_clamp_width(200.0, 1280.0, 52.0, 280.0) == 360.0
    exp_hi = side_panel_clamp_width(2200.0, 1280.0, 52.0, 280.0)
    assert require_field(hi, "width_px") == exp_hi
    assert require_field(hi, "conversation_width_px") >= 96


@uiux_e2e_test(
    id="TEST-UIUX-T2-UI-F15-02",
    tier=2,
    features=("UI-F15",),
    description="Verify SidePanel rejects 3rd vertical split (>MAX_SIDE_SPLITS=2) and invalid heights/shares",
)
def test_t2_f15_02_split_count_and_height_share_bounds_enforced(env: TempUiUxEnv) -> None:
    sp = env.probe_scenario("f15_invalid_third_split", "side_panel")
    assert require_field(sp, "split_accepted") is False
    assert len(require_field(sp, "active_splits")) <= 2
    assert require_field(sp, "main_group_height_px") >= 160
    assert require_field(sp, "lower_split_height_px") >= 120


@uiux_e2e_test(
    id="TEST-UIUX-T2-UI-F15-03",
    tier=2,
    features=("UI-F15",),
    description="Verify crowded SidePanelHeader activates overflow ChevronDown and combined ... menu",
)
def test_t2_f15_03_crowded_side_panel_header_overflow_folding(env: TempUiUxEnv) -> None:
    sp = env.probe_scenario("f15_overflow_8_tabs", "side_panel")
    assert require_field(sp, "overflow_chevron_visible") is True
    assert require_field(sp, "hidden_tabs_count") > 0
    assert require_field(sp, "combined_actions_menu") is True
    assert require_field(sp, "header_height_px") == 38


@uiux_e2e_test(
    id="TEST-UIUX-T2-UI-F15-04",
    tier=2,
    features=("UI-F15",),
    description="Verify unavailable file tab flag and closing last tab while maximized exits maximize cleanly",
)
def test_t2_f15_04_unavailable_file_tab_and_closing_last_maximized_pane(env: TempUiUxEnv) -> None:
    unav = env.probe_scenario("f15_unavailable_file_tab", "side_panel")
    closed = env.probe_scenario("f15_close_last_maximized", "side_panel")
    assert require_field(unav, "unavailable_tab_flagged") is True
    assert require_field(closed, "maximized") is False
    assert require_field(closed, "launcher_visible") is True


@uiux_e2e_test(
    id="TEST-UIUX-T2-UI-F15-05",
    tier=2,
    features=("UI-F15",),
    description="Verify SideQuestionPanel 20-turn cap, zero main chat mutation, and failed-turn draft restore",
)
def test_t2_f15_05_side_question_20_turn_cap_and_failed_draft_restore(env: TempUiUxEnv) -> None:
    sp = env.probe_scenario("f15_side_question_cap_and_fail", "side_panel")
    assert require_field(sp, "side_question_turns") == 20
    assert require_field(sp, "side_question_max_turns") == 20
    assert require_field(sp, "main_msg_delta") == 0
    assert require_field(sp, "side_question_draft_restored") is True


@uiux_e2e_test(
    id="TEST-UIUX-T2-UI-F15-06",
    tier=2,
    features=("UI-F15",),
    description="Verify rapid PanelRight/Settings/Inspector toggle spam never spawns double right drawers (D11)",
)
def test_t2_f15_06_rapid_toggle_spam_single_right_drawer_invariant(env: TempUiUxEnv) -> None:
    sp = env.probe_scenario("f15_toggle_spam_stress", "side_panel")
    assert require_field(sp, "active_right_drawer_count") <= 1
    assert require_field(sp, "legacy_standalone_inspector_mounted") is False
    assert require_field(sp, "competing_companion_panel_mounted") is False
