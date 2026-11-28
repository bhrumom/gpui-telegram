"""Tier 1 Isolated Feature Tests: UI-F11 through UI-F15 (30 opaque-box tests)."""

from __future__ import annotations

from tests.e2e_openbot_uiux.harness import (
    TempUiUxEnv,
    assert_hex_eq,
    assert_within,
    require_field,
    uiux_e2e_test,
)
from tests.e2e_openbot_uiux.oracles import channel_avatar_cluster_oracle


# ---------------------------------------------------------------------------
# UI-F11: OpenBot Group Chat (Channel) Stage (6 tests)
# ---------------------------------------------------------------------------


@uiux_e2e_test(
    id="TEST-UIUX-T1-UI-F11-01",
    tier=1,
    features=("UI-F11",),
    description="Verify ChannelAvatar cluster composition for 1, 2, 3, and 5 members",
)
def test_t1_f11_01_channel_avatar_cluster_1_2_3_5_members(env: TempUiUxEnv) -> None:
    for cnt, sc_id in ((1, "f11_cluster_1"), (2, "f11_cluster_2"), (3, "f11_cluster_3"), (5, "f11_cluster_5")):
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
        if cnt == 1:
            assert require_field(av, "face_scale_pct") == 100
        elif cnt in (2, 3):
            assert require_field(av, "face_scale_pct") == 54
        else:
            assert require_field(av, "overflow_label") == "+2"


@uiux_e2e_test(
    id="TEST-UIUX-T1-UI-F11-02",
    tier=1,
    features=("UI-F11",),
    description="Verify ChannelAvatar row layout, custom icon catalog >=20, and custom photo precedence",
)
def test_t1_f11_02_channel_avatar_row_icon_and_photo_priority(env: TempUiUxEnv) -> None:
    row_st = env.probe_scenario("f11_avatar_row", "channel_stage")
    ico_st = env.probe_scenario("f11_avatar_icon", "channel_stage")
    pho_st = env.probe_scenario("f11_avatar_photo", "channel_stage")
    assert require_field(row_st, "avatar.layout") == "row"
    assert require_field(row_st, "avatar.visible_faces") <= 3
    assert require_field(row_st, "avatar.overflow_count") == 0
    assert require_field(ico_st, "avatar.render_kind") == "icon"
    assert require_field(ico_st, "avatar.resolved_icon") == "sparkles"
    assert require_field(pho_st, "avatar.render_kind") == "photo"


@uiux_e2e_test(
    id="TEST-UIUX-T1-UI-F11-03",
    tier=1,
    features=("UI-F11",),
    description="Verify group 36px author gutter, 32px clickable avatar, colored author name, and classic grouping",
)
def test_t1_f11_03_multi_author_gutter_avatar_name_and_grouping(env: TempUiUxEnv) -> None:
    st = env.probe_scenario("f11_multi_author_stream", "channel_stage")
    assert require_field(st, "show_author") is True
    assert require_field(st, "bubble_style") == "classic"
    assert require_field(st, "author_gutter_px") == 36
    assert require_field(st, "author_avatar_px") == 32
    assert require_field(st, "author_avatar_clickable") is True
    assert require_field(st, "author_name_clickable") is True
    assert_hex_eq(require_field(st, "author_name_color"), "#8b5cf6", "author_name.color")
    assert require_field(st, "consecutive_header_collapsed") is True


@uiux_e2e_test(
    id="TEST-UIUX-T1-UI-F11-04",
    tier=1,
    features=("UI-F11",),
    description="Verify group ReplyQuote 2px start border, 60px max height, click jump, and 1.6s flash",
)
def test_t1_f11_04_reply_quote_click_jump_and_1600ms_flash(env: TempUiUxEnv) -> None:
    st = env.probe_scenario("f11_reply_quote_jump", "channel_stage")
    rq = require_field(st, "reply_quote")
    assert require_field(rq, "border_px") == 2
    assert require_field(rq, "max_height_px") == 60
    assert bool(require_field(rq, "jumped_to_id"))
    assert require_field(rq, "flash_duration_ms") == 1600


@uiux_e2e_test(
    id="TEST-UIUX-T1-UI-F11-05",
    tier=1,
    features=("UI-F11",),
    description="Verify centered ChatActionMarker clickable pills and ChannelActivityIndicator multi-worker shimmer",
)
def test_t1_f11_05_action_markers_and_multi_worker_activity_indicator(env: TempUiUxEnv) -> None:
    st = env.probe_scenario("f11_routing_and_activity", "channel_stage")
    markers = require_field(st, "action_markers")
    assert len(markers) >= 1
    assert require_field(markers[0], "clickable_pills") is True
    act = require_field(st, "activity_indicator")
    assert require_field(act, "visible") is True
    assert require_field(act, "worker_count") == 2
    assert require_field(act, "overlap_px") in (-8, -10)
    assert require_field(act, "shimmer_animated") is True
    assert ":" in require_field(act, "elapsed_text")


@uiux_e2e_test(
    id="TEST-UIUX-T1-UI-F11-06",
    tier=1,
    features=("UI-F11",),
    description="Verify 440px ChannelCreateDialog searchable checklist, >=1 member guard, and auto-lead creation",
)
def test_t1_f11_06_channel_create_dialog_440px_checklist_and_auto_lead(env: TempUiUxEnv) -> None:
    st = env.probe_scenario("f11_create_dialog_ok", "channel_stage")
    dlg = require_field(st, "create_dialog")
    assert require_field(dlg, "width_px") == 440
    assert require_field(dlg, "radius_px") == 14
    assert_hex_eq(require_field(dlg, "surface_hex"), "#212121", "dialog.surface")
    assert require_field(dlg, "selected_members") == 2
    assert require_field(dlg, "submit_disabled") is False
    assert bool(require_field(st, "channel_id"))


# ---------------------------------------------------------------------------
# UI-F12: Sleek 48px Morphing Pill Composer & + ComposerAddMenu (6 tests)
# ---------------------------------------------------------------------------


@uiux_e2e_test(
    id="TEST-UIUX-T1-UI-F12-01",
    tier=1,
    features=("UI-F12",),
    description="Verify idle 48px 24px-radius #212121 pill composer and #f0f0f0 right action circle (image_2.png)",
)
def test_t1_f12_01_idle_48px_pill_geometry_and_tokens(env: TempUiUxEnv) -> None:
    c = env.probe_scenario("f12_idle_pill", "composer")
    stack = env.probe_scenario("f12_idle_pill", "composer_stack")
    assert require_field(c, "compact") is True
    assert require_field(c, "height_px") == 48
    assert require_field(c, "border_radius_px") == 24
    assert_hex_eq(require_field(c, "surface_hex"), "#212121", "composer.surface")
    assert require_field(c, "add_btn_px") == 28
    assert require_field(c, "action_circle_px") == 28
    assert_hex_eq(require_field(c, "action_circle_bg_hex"), "#f0f0f0", "composer.send_bg")
    assert require_field(stack, "height_px") == 0


@uiux_e2e_test(
    id="TEST-UIUX-T1-UI-F12-02",
    tier=1,
    features=("UI-F12",),
    description="Verify pill morphs to >=96px 14px-radius card on multiline input or attachment",
)
def test_t1_f12_02_pill_to_card_morph_on_multiline_and_attachment(env: TempUiUxEnv) -> None:
    multi = env.probe_scenario("f12_multiline_card", "composer")
    att = env.probe_scenario("f12_attachment_card", "composer")
    assert require_field(multi, "compact") is False
    assert require_field(multi, "min_height_px") >= 96
    assert require_field(multi, "border_radius_px") == 14
    assert require_field(att, "compact") is False
    assert require_field(att, "border_radius_px") == 14
    assert require_field(att, "attachments_count") >= 1


@uiux_e2e_test(
    id="TEST-UIUX-T1-UI-F12-03",
    tier=1,
    features=("UI-F12",),
    description="Verify + ComposerAddMenu exposes all 6 canonical OpenBot actions",
)
def test_t1_f12_03_composer_add_menu_six_canonical_actions(env: TempUiUxEnv) -> None:
    c = env.probe_scenario("f12_add_menu", "composer")
    assert require_field(c, "add_menu_open") is True
    items = require_field(c, "add_menu_items")
    for expected in ("attach", "skill", "teach", "side_question", "voice", "new_chat"):
        assert expected in items, f"Missing ComposerAddMenu action {expected!r}"
    assert len(items) == 6


@uiux_e2e_test(
    id="TEST-UIUX-T1-UI-F12-04",
    tier=1,
    features=("UI-F12",),
    description="Verify right 28x28 #f0f0f0 circle transitions cleanly across Voice, Send, and Stop states",
)
def test_t1_f12_04_tri_state_right_action_circle_voice_send_stop(env: TempUiUxEnv) -> None:
    idle = env.probe_scenario("f12_idle_pill", "composer")
    draft = env.probe_scenario("f12_draft_send", "composer")
    busy = env.probe_scenario("f12_active_stop", "composer")
    assert require_field(idle, "action_circle_kind") == "voice"
    assert require_field(draft, "action_circle_kind") == "send"
    assert require_field(busy, "action_circle_kind") == "stop"
    assert_hex_eq(require_field(busy, "action_circle_bg_hex"), "#f0f0f0", "stop.bg")


@uiux_e2e_test(
    id="TEST-UIUX-T1-UI-F12-05",
    tier=1,
    features=("UI-F12",),
    description="Verify IME-safe CJK Enter commit fence and dictation mic 12-bar waveform morph",
)
def test_t1_f12_05_ime_safe_editor_and_dictation_waveform(env: TempUiUxEnv) -> None:
    ime_commit = env.probe_scenario("f12_ime_commit", "composer")
    ime_send = env.probe_scenario("f12_ime_send", "composer")
    dictation = env.probe_scenario("f12_dictation_mic", "composer")
    assert require_field(ime_commit, "sent_count") == 0
    assert require_field(ime_commit, "draft_text") == "你好"
    assert require_field(ime_send, "sent_count") == 1
    assert require_field(dictation, "dictation_active") is True
    assert_within(require_field(dictation, "dictation_width_px"), 152, 188, "dictation.width")
    assert require_field(dictation, "dictation_bars") == 12


@uiux_e2e_test(
    id="TEST-UIUX-T1-UI-F12-06",
    tier=1,
    features=("UI-F12",),
    description="Verify elimination of legacy 2-row chip bar (D9) and Telegram blue focus border (D10)",
)
def test_t1_f12_06_no_legacy_two_row_chip_strip_or_blue_composer(env: TempUiUxEnv) -> None:
    c = env.probe_scenario("f12_idle_pill", "composer")
    assert require_field(c, "legacy_chip_strip_present") is False
    assert require_field(c, "detached_slash_present") is False
    assert require_field(c, "legacy_aa_emoji_row_present") is False
    assert require_field(c, "focus_border_is_telegram_blue") is False
    assert require_field(c, "send_btn_is_telegram_blue") is False


# ---------------------------------------------------------------------------
# UI-F13: @mention / @everyone / $ Skill Picker & ReferenceChip Tokens (6 tests)
# ---------------------------------------------------------------------------


@uiux_e2e_test(
    id="TEST-UIUX-T1-UI-F13-01",
    tier=1,
    features=("UI-F13",),
    description="Verify @ picker #121212 surface, 14px radius, 29px rows, max 7 rows, and 5 ordered sections",
)
def test_t1_f13_01_at_picker_tokens_sections_and_max7_cap(env: TempUiUxEnv) -> None:
    mp = env.probe_scenario("f13_at_picker_rep", "mention_picker")
    assert require_field(mp, "open") is True
    assert require_field(mp, "trigger") == "@"
    assert_hex_eq(require_field(mp, "bg_hex"), "#121212", "picker.bg")
    assert_hex_eq(require_field(mp, "border_hex"), "#222222", "picker.border")
    assert require_field(mp, "radius_px") == 14
    assert require_field(mp, "row_height_px") == 29
    assert require_field(mp, "max_visible_rows") == 7
    assert require_field(mp, "sections") == ["agents", "files", "tabs", "threads", "messages"]
    assert require_field(mp, "messages_debounce_ms") == 150
    assert require_field(mp, "messages_min_query") == 3


@uiux_e2e_test(
    id="TEST-UIUX-T1-UI-F13-02",
    tier=1,
    features=("UI-F13",),
    description="Verify @everyone appears in multi-member groups (>1) and inserts atomic 24px chip",
)
def test_t1_f13_02_everyone_visible_in_multi_member_group_and_chip(env: TempUiUxEnv) -> None:
    mp = env.probe_scenario("f13_group_everyone", "mention_picker")
    assert require_field(mp, "everyone_shown") is True
    chips = require_field(mp, "inserted_chips")
    ev_chip = next(ch for ch in chips if ch.get("token") == "@everyone")
    assert require_field(ev_chip, "height_px") == 24
    assert require_field(ev_chip, "radius_px") == 6
    assert_hex_eq(require_field(ev_chip, "bg_hex"), "#262626", "chip.bg")


@uiux_e2e_test(
    id="TEST-UIUX-T1-UI-F13-03",
    tier=1,
    features=("UI-F13",),
    description="Verify $ skill and MCP picker disambiguates duplicate names by slug and inserts chip",
)
def test_t1_f13_03_dollar_skill_picker_and_slug_disambiguation(env: TempUiUxEnv) -> None:
    mp = env.probe_scenario("f13_dollar_skills", "mention_picker")
    assert require_field(mp, "trigger") == "$"
    results = require_field(mp, "results")
    slugs = {r.get("slug") for r in results}
    assert {"web-search", "doc-search"}.issubset(slugs)
    chips = require_field(mp, "inserted_chips")
    assert any(ch.get("kind") == "skill" and ch.get("height_px") == 24 for ch in chips)


@uiux_e2e_test(
    id="TEST-UIUX-T1-UI-F13-04",
    tier=1,
    features=("UI-F13",),
    description="Verify arrow navigation, Tab accept, and single Backspace atomic chip deletion",
)
def test_t1_f13_04_keyboard_navigation_and_atomic_backspace(env: TempUiUxEnv) -> None:
    picked = env.probe_scenario("f13_tab_accepted", "mention_picker")
    deleted = env.probe_scenario("f13_atomic_backspace", "mention_picker")
    assert require_field(picked, "open") is False
    assert len(require_field(picked, "inserted_chips")) == 1
    assert len(require_field(deleted, "inserted_chips")) == 0
    assert require_field(deleted, "leftover_partial_mention") is False


@uiux_e2e_test(
    id="TEST-UIUX-T1-UI-F13-05",
    tier=1,
    features=("UI-F13",),
    description="Verify sent messages render clickable 24px ReferenceChip pills with 16px icons",
)
def test_t1_f13_05_rendered_message_reference_chips_clickable(env: TempUiUxEnv) -> None:
    mp = env.probe_scenario("f13_rendered_chips", "mention_picker")
    chips = require_field(mp, "rendered_chips")
    assert len(chips) >= 2
    for chip in chips:
        assert require_field(chip, "height_px") == 24
        assert require_field(chip, "radius_px") == 6
        assert require_field(chip, "clickable") is True


@uiux_e2e_test(
    id="TEST-UIUX-T1-UI-F13-06",
    tier=1,
    features=("UI-F13",),
    description="Verify NFD-folded fuzzy agent mention ranking puts prefix diacritic match first",
)
def test_t1_f13_06_nfd_folded_fuzzy_agent_mention_ranking(env: TempUiUxEnv) -> None:
    mp = env.probe_scenario("f13_nfd_eclair", "mention_picker")
    results = require_field(mp, "results")
    assert len(results) >= 1
    assert require_field(results[0], "name") == "Éclair"


# ---------------------------------------------------------------------------
# UI-F14: Tucked .composer-stack (6 tests)
# ---------------------------------------------------------------------------


@uiux_e2e_test(
    id="TEST-UIUX-T1-UI-F14-01",
    tier=1,
    features=("UI-F14",),
    description="Verify idle .composer-stack collapses to 0px and tucks -24px behind pill when active",
)
def test_t1_f14_01_idle_zero_height_and_tucked_overlap_when_active(env: TempUiUxEnv) -> None:
    idle = env.probe_scenario("f14_stack_idle", "composer_stack")
    active = env.probe_scenario("f14_stack_active", "composer_stack")
    assert require_field(idle, "visible") is False
    assert require_field(idle, "height_px") == 0
    assert require_field(active, "visible") is True
    assert require_field(active, "radius_px") == 14
    assert require_field(active, "overlap_bottom_px") == 24
    assert require_field(active, "z_index") < require_field(active, "composer_z_index")


@uiux_e2e_test(
    id="TEST-UIUX-T1-UI-F14-02",
    tier=1,
    features=("UI-F14",),
    description="Verify QueuePanel 29px rows, reorder, edit, delete, and clean collapse",
)
def test_t1_f14_02_queue_panel_reorder_edit_delete_lifecycle(env: TempUiUxEnv) -> None:
    st = env.probe_scenario("f14_queue_mutations", "composer_stack")
    q = require_field(st, "queue")
    assert require_field(q, "row_height_px") == 29
    assert require_field(q, "items") == ["M3", "M2-edited"]
    assert require_field(q, "visible_rows") <= 7


@uiux_e2e_test(
    id="TEST-UIUX-T1-UI-F14-03",
    tier=1,
    features=("UI-F14",),
    description="Verify QueuePanel Tab Steer promotion during an active steerable agent turn",
)
def test_t1_f14_03_queue_panel_tab_steer_promotion(env: TempUiUxEnv) -> None:
    st = env.probe_scenario("f14_queue_tab_steer", "composer_stack")
    q = require_field(st, "queue")
    assert require_field(q, "can_steer") is True
    assert require_field(q, "steered_on_tab") is True
    assert require_field(q, "steer_refused") is False


@uiux_e2e_test(
    id="TEST-UIUX-T1-UI-F14-04",
    tier=1,
    features=("UI-F14",),
    description="Verify ComposerReplyPreview 42px min-height card and X dismiss collapse",
)
def test_t1_f14_04_composer_reply_preview_and_dismiss(env: TempUiUxEnv) -> None:
    shown = env.probe_scenario("f14_reply_shown", "composer_stack")
    cleared = env.probe_scenario("f14_reply_dismissed", "composer_stack")
    rp = require_field(shown, "reply_preview")
    assert require_field(rp, "visible") is True
    assert require_field(rp, "min_height_px") == 42
    assert require_field(rp, "sender") == "BotA"
    assert require_field(cleared, "reply_preview.visible") is False
    assert require_field(cleared, "height_px") == 0


@uiux_e2e_test(
    id="TEST-UIUX-T1-UI-F14-05",
    tier=1,
    features=("UI-F14",),
    description="Verify group AwaitingReplies Subtasks collapsible card and settled dismiss",
)
def test_t1_f14_05_group_awaiting_replies_subtasks_card(env: TempUiUxEnv) -> None:
    st = env.probe_scenario("f14_awaiting_subtasks", "composer_stack")
    aw = require_field(st, "awaiting_replies")
    assert require_field(aw, "visible") is True
    assert require_field(aw, "title") == "Subtasks"
    assert len(require_field(aw, "tasks")) == 3
    assert require_field(st, "z_index") < require_field(st, "composer_z_index")


@uiux_e2e_test(
    id="TEST-UIUX-T1-UI-F14-06",
    tier=1,
    features=("UI-F14",),
    description="Verify ChannelStoppedTasks Resume/Resume all, OpenWaits, and dismissible ComposerNotice",
)
def test_t1_f14_06_stopped_tasks_open_waits_and_composer_notice(env: TempUiUxEnv) -> None:
    st = env.probe_scenario("f14_stopped_and_notice", "composer_stack")
    stopped = require_field(st, "stopped_tasks")
    assert require_field(stopped, "visible") is True
    assert len(require_field(stopped, "tasks")) == 2
    assert require_field(st, "open_waits_count") >= 1
    notice = require_field(st, "notice")
    assert require_field(notice, "visible") is True
    assert require_field(notice, "has_copy_details") is True


# ---------------------------------------------------------------------------
# UI-F15: Unified Dockable OpenBot Right SidePanel & 4-Button Launcher (6 tests)
# ---------------------------------------------------------------------------


@uiux_e2e_test(
    id="TEST-UIUX-T1-UI-F15-01",
    tier=1,
    features=("UI-F15",),
    description="Verify exact 4-button centered SidePanelLauncher for bot chat (image_2.png)",
)
def test_t1_f15_01_four_button_launcher_empty_state_bot_chat(env: TempUiUxEnv) -> None:
    sp = env.probe_scenario("f15_launcher_bot", "side_panel")
    assert require_field(sp, "open") is True
    assert require_field(sp, "launcher_visible") is True
    assert require_field(sp, "launcher_max_width_px") == 320
    assert require_field(sp, "launcher_button_height_px") == 36
    assert_hex_eq(require_field(sp, "launcher_button_bg_hex"), "#242424", "launcher.btn_bg")
    assert require_field(sp, "launcher_buttons") == [
        "打开 浏览器", "打开 文件", "打开 话题", "打开 助手设置"
    ]


@uiux_e2e_test(
    id="TEST-UIUX-T1-UI-F15-02",
    tier=1,
    features=("UI-F15",),
    description="Verify 4th launcher button adapts across Bot, Group, and 1:1 Human peer kinds",
)
def test_t1_f15_02_context_adaptive_fourth_launcher_button(env: TempUiUxEnv) -> None:
    bot = env.probe_scenario("f15_launcher_bot", "side_panel")
    grp = env.probe_scenario("f15_launcher_group", "side_panel")
    hum = env.probe_scenario("f15_launcher_human", "side_panel")
    assert require_field(bot, "launcher_buttons")[3] == "打开 助手设置"
    assert require_field(grp, "launcher_buttons")[3] == "打开 群聊设置"
    assert require_field(hum, "launcher_buttons")[3] == "打开 联系人资料"


@uiux_e2e_test(
    id="TEST-UIUX-T1-UI-F15-03",
    tier=1,
    features=("UI-F15",),
    description="Verify single unified right drawer invariant eliminates D11 double drawers",
)
def test_t1_f15_03_single_unified_right_drawer_eliminates_d11(env: TempUiUxEnv) -> None:
    sp = env.probe_scenario("f15_unified_drawer", "side_panel")
    assert require_field(sp, "active_right_drawer_count") == 1
    assert require_field(sp, "legacy_standalone_inspector_mounted") is False
    assert require_field(sp, "competing_companion_panel_mounted") is False
    assert require_field(sp, "width_px") == 360


@uiux_e2e_test(
    id="TEST-UIUX-T1-UI-F15-04",
    tier=1,
    features=("UI-F15",),
    description="Verify 38px tabbed SidePanelHeader, + add pane menu, and launcher fallback on last tab close",
)
def test_t1_f15_04_tabbed_38px_header_add_menu_and_last_tab_close(env: TempUiUxEnv) -> None:
    tabs = env.probe_scenario("f15_tabs_open", "side_panel")
    closed = env.probe_scenario("f15_tabs_all_closed", "side_panel")
    assert require_field(tabs, "header_height_px") == 38
    assert len(require_field(tabs, "open_tabs")) >= 2
    assert len(require_field(tabs, "add_pane_menu_items")) >= 1
    assert require_field(closed, "open_tabs") == []
    assert require_field(closed, "launcher_visible") is True


@uiux_e2e_test(
    id="TEST-UIUX-T1-UI-F15-05",
    tier=1,
    features=("UI-F15",),
    description="Verify SidePanel [360, 1600] resize persistence and 100% Maximize/Minimize toggle",
)
def test_t1_f15_05_resize_persistence_and_maximize_toggle(env: TempUiUxEnv) -> None:
    res = env.probe_scenario("f15_resized_520", "side_panel")
    mx = env.probe_scenario("f15_maximized", "side_panel")
    assert require_field(res, "width_px") == 520
    assert require_field(res, "persisted_width_px") == 520
    assert require_field(mx, "maximized") is True
    assert require_field(mx, "effective_width_pct") == 100


@uiux_e2e_test(
    id="TEST-UIUX-T1-UI-F15-06",
    tier=1,
    features=("UI-F15",),
    description="Verify vertical split (MAX_SIDE_SPLITS=2) and SideQuestion/Subagent pane docking",
)
def test_t1_f15_06_vertical_split_and_side_question_subagent_docking(env: TempUiUxEnv) -> None:
    sp = env.probe_scenario("f15_split_and_side_question", "side_panel")
    assert len(require_field(sp, "active_splits")) == 2
    assert require_field(sp, "max_splits") == 2
    assert require_field(sp, "main_group_height_px") >= 160
    assert require_field(sp, "lower_split_height_px") >= 120
    assert 0.0 < require_field(sp, "split_share") < 1.0
    assert require_field(sp, "main_msg_delta") == 0
