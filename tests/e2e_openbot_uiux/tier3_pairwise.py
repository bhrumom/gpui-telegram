"""Tier 3 Cross-Feature Pairwise Combination Tests (24 tests covering every UI-F01..UI-F20 >=2x)."""

from __future__ import annotations

from tests.e2e_openbot_uiux.harness import (
    TempUiUxEnv,
    assert_defect_eliminated,
    assert_hex_eq,
    check_capability_coverage_json,
    check_preserved_pub_consts,
    require_field,
    uiux_e2e_test,
)
from tests.e2e_openbot_uiux.oracles import (
    channel_avatar_cluster_oracle,
    hex_to_oklab,
    oklab_readable_chat_bg,
    solid_bubble_takes_dark_text,
)


@uiux_e2e_test(
    id="TEST-UIUX-T3-PAIR-01",
    tier=3,
    features=("UI-F01", "UI-F03"),
    description="Verify dark/light token propagation across inset edge-framed root shell without seams",
)
def test_t3_pair_01_dark_light_tokens_across_inset_shell(env: TempUiUxEnv) -> None:
    dk_tok = env.probe_scenario("pair01_dark", "tokens")
    dk_sh = env.probe_scenario("pair01_dark", "shell")
    lt_tok = env.probe_scenario("pair01_light", "tokens")
    lt_sh = env.probe_scenario("pair01_light", "shell")
    assert_hex_eq(require_field(dk_tok, "dark.bg_frame"), "#0d0d0d", "dk.frame")
    assert_hex_eq(require_field(dk_sh, "outer_frame_bg_hex"), "#0d0d0d", "dk.shell")
    assert_hex_eq(require_field(dk_sh, "canvas_card.bg_hex"), "#1a1a1a", "dk.card")
    assert_hex_eq(require_field(lt_tok, "light.bg_frame"), "#e8e8eb", "lt.frame")
    assert_hex_eq(require_field(lt_sh, "outer_frame_bg_hex"), "#e8e8eb", "lt.shell")
    assert_hex_eq(require_field(lt_sh, "canvas_card.bg_hex"), "#f5f5f6", "lt.card")
    assert require_field(dk_sh, "canvas_card.radius_px") == 14


@uiux_e2e_test(
    id="TEST-UIUX-T3-PAIR-02",
    tier=3,
    features=("UI-F01", "UI-F10"),
    description="Verify OKLab readable chat background and bright bubble text contrast on 1:1 stream",
)
def test_t3_pair_02_oklab_chat_bg_and_bubble_contrast_on_stream(env: TempUiUxEnv) -> None:
    tok = env.probe_scenario("pair02_yellow_stream", "tokens")
    cs = env.probe_scenario("pair02_yellow_stream", "chat_stream")
    clamped = require_field(tok, "oklab_samples")[0]
    out_hex = require_field(clamped, "result_hex")
    assert_hex_eq(out_hex, oklab_readable_chat_bg(require_field(clamped, "hex"), require_field(clamped, "scheme")))
    L, _, _, C = hex_to_oklab(out_hex)
    assert L <= 0.305 and C <= 0.055
    user_b = next(b for b in require_field(cs, "bubbles") if b.get("author") == "user")
    assert solid_bubble_takes_dark_text("#e5a50a") is True
    assert_hex_eq(require_field(user_b, "resolved_text_hex"), "#141414", "yellow.bubble_text")


@uiux_e2e_test(
    id="TEST-UIUX-T3-PAIR-03",
    tier=3,
    features=("UI-F02", "UI-F06"),
    description="Verify 3D plush avatars and felt status badges inside 62px CJK-safe sidebar rows (D6/D7)",
)
def test_t3_pair_03_plush_avatars_and_felt_badges_in_62px_rows(env: TempUiUxEnv) -> None:
    plush = env.probe_scenario("pair03_sidebar_plush_badges", "plush")
    sl = env.probe_scenario("pair03_sidebar_plush_badges", "sidebar_list")
    assert require_field(plush, "default_bot_avatar.is_plush_3d") is True
    assert require_field(plush, "badges.working.width_px") == 30
    row = require_field(sl, "rows")[0]
    assert require_field(row, "height_px") == 62
    assert require_field(row, "avatar_px") == 46
    assert require_field(row, "min_w_0") is True and require_field(row, "whitespace_nowrap") is True


@uiux_e2e_test(
    id="TEST-UIUX-T3-PAIR-04",
    tier=3,
    features=("UI-F02", "UI-F08"),
    description="Verify FirstAgentSetup swatch/shape selection updates live 72px plush hero and persists plush:*",
)
def test_t3_pair_04_agent_studio_selection_persists_plush_avatar(env: TempUiUxEnv) -> None:
    st = env.probe_scenario("pair04_studio_to_plush", "agent_studio")
    plush = env.probe_scenario("pair04_studio_to_plush", "plush")
    assert require_field(st, "hero.size_px") == 72
    assert require_field(st, "hero.shape") == "triangle"
    assert bool(require_field(st, "created_agent_id"))
    assert require_field(plush, "default_bot_avatar.shape_prefix") == "plush:"
    assert require_field(plush, "profile.silhouette") == "triangle"


@uiux_e2e_test(
    id="TEST-UIUX-T3-PAIR-05",
    tier=3,
    features=("UI-F03", "UI-F04"),
    description="Verify responsive <=720px frame collapse hides 52px PageRail and restores cleanly at 1280px",
)
def test_t3_pair_05_responsive_720px_frame_and_rail_collapse_restore(env: TempUiUxEnv) -> None:
    sh700 = env.probe_scenario("pair05_viewport_700", "shell")
    rl700 = env.probe_scenario("pair05_viewport_700", "page_rail")
    sh1280 = env.probe_scenario("pair05_viewport_1280", "shell")
    rl1280 = env.probe_scenario("pair05_viewport_1280", "page_rail")
    assert require_field(sh700, "canvas_card.full_bleed") is True
    assert require_field(rl700, "visible") is False
    assert require_field(sh1280, "canvas_card.radius_px") == 14
    assert require_field(rl1280, "visible") is True and require_field(rl1280, "width_px") == 52


@uiux_e2e_test(
    id="TEST-UIUX-T3-PAIR-06",
    tier=3,
    features=("UI-F04", "UI-F06", "UI-F20"),
    description="Verify sole PageRail footer suppresses duplicate ChatSidebar footer while preserving legacy pub consts",
)
def test_t3_pair_06_sole_rail_footer_no_dup_sidebar_footer_and_compat_consts(env: TempUiUxEnv) -> None:
    ws = check_preserved_pub_consts()
    assert ws["all_preserved"] is True
    rail = env.probe_scenario("pair06_sole_footer_compat", "page_rail")
    sl = env.probe_scenario("pair06_sole_footer_compat", "sidebar_list")
    defects = env.probe_scenario("pair06_sole_footer_compat", "defects_audit")
    assert require_field(rail, "width_px") == 52
    assert require_field(rail, "total_account_footers_in_shell") == 1
    assert require_field(sl, "has_duplicate_account_footer") is False
    assert_defect_eliminated(defects, "d1_wide_apple_sidebar")
    assert_defect_eliminated(defects, "d2_duplicate_account_footer")


@uiux_e2e_test(
    id="TEST-UIUX-T3-PAIR-07",
    tier=3,
    features=("UI-F04", "UI-F18"),
    description="Verify PageRail RailMoreMenu Pin/Unpin routes cleanly into OpenBot-aligned secondary hubs",
)
def test_t3_pair_07_page_rail_pins_route_to_openbot_secondary_hubs(env: TempUiUxEnv) -> None:
    rail = env.probe_scenario("pair07_rail_to_hubs", "page_rail")
    hub = env.probe_scenario("pair07_rail_to_hubs", "content_hub_pages")
    assert "my_creations" in require_field(rail, "persisted_pins")
    assert "games" in require_field(rail, "more_menu.entries")
    assert require_field(hub, "mounted_inside_inset_card") is True
    assert require_field(hub, "page_header_height_px") == 38
    assert require_field(hub, "legacy_blue_pill_count") == 0


@uiux_e2e_test(
    id="TEST-UIUX-T3-PAIR-08",
    tier=3,
    features=("UI-F05", "UI-F07"),
    description="Verify SidebarSearch pill trigger opens 640px GlobalSearch with GlobalSearchPlush navigation",
)
def test_t3_pair_08_sidebar_search_pill_to_global_search_navigation(env: TempUiUxEnv) -> None:
    sc = env.probe_scenario("pair08_pill_to_global_search", "sidebar_chrome")
    gs = env.probe_scenario("pair08_pill_to_global_search", "global_search")
    assert require_field(sc, "search.height_px") == 32
    assert require_field(gs, "open") is True
    assert require_field(gs, "width_px") == 640
    assert require_field(gs, "plush.size_px") == 56
    assert len(require_field(gs, "tabs")) == 6


@uiux_e2e_test(
    id="TEST-UIUX-T3-PAIR-09",
    tier=3,
    features=("UI-F05", "UI-F08"),
    description="Verify circular #f0f0f0 + menu launches FirstAgentSetup studio and appends new plush bot row",
)
def test_t3_pair_09_plus_menu_new_agent_to_studio_and_sidebar_row(env: TempUiUxEnv) -> None:
    sc = env.probe_scenario("pair09_plus_to_studio", "sidebar_chrome")
    st = env.probe_scenario("pair09_plus_to_studio", "agent_studio")
    assert_hex_eq(require_field(sc, "add_button.bg_hex"), "#f0f0f0", "plus.bg")
    assert require_field(st, "form_width_px") == 420
    assert require_field(st, "recommendations.hydrated") is True
    assert bool(require_field(st, "created_agent_id"))


@uiux_e2e_test(
    id="TEST-UIUX-T3-PAIR-10",
    tier=3,
    features=("UI-F05", "UI-F09"),
    description="Verify Cmd+B sidebar close keeps SidebarTopbar controls visible and shifts ConversationHeader (D3)",
)
def test_t3_pair_10_cmd_b_sidebar_close_shifts_conversation_header(env: TempUiUxEnv) -> None:
    sc = env.probe_scenario("pair10_cmd_b_header_shift", "sidebar_chrome")
    hdr = env.probe_scenario("pair10_cmd_b_header_shift", "conversation_header")
    assert require_field(sc, "mode") == "closed"
    assert require_field(sc, "topbar.toggle_visible") is True
    assert require_field(hdr, "shifted_start_px") > 0
    assert require_field(hdr, "overlaps_timeline") is False


@uiux_e2e_test(
    id="TEST-UIUX-T3-PAIR-11",
    tier=3,
    features=("UI-F06", "UI-F11"),
    description="Verify 5-member group sidebar row cluster opens ChannelConversation multi-author stage (D12)",
)
def test_t3_pair_11_group_sidebar_row_to_channel_stage_multi_author(env: TempUiUxEnv) -> None:
    sl = env.probe_scenario("pair11_group_row_to_stage", "sidebar_list")
    ch = env.probe_scenario("pair11_group_row_to_stage", "channel_stage")
    hdr = env.probe_scenario("pair11_group_row_to_stage", "conversation_header")
    oracle5 = channel_avatar_cluster_oracle(5)
    assert require_field(sl, "rows")[0]["height_px"] == 62
    av5 = require_field(ch, "avatar")
    assert (
        require_field(av5, "visible_faces"),
        require_field(av5, "overflow_count"),
        require_field(av5, "overflow_pill_tone"),
    ) in oracle5["valid_tuples"]
    assert require_field(ch, "author_gutter_px") == 36
    assert require_field(hdr, "model_picker.visible") is False


@uiux_e2e_test(
    id="TEST-UIUX-T3-PAIR-12",
    tier=3,
    features=("UI-F07", "UI-F19"),
    description="Verify GlobalSearch Cmd+K hit jumps directly into 2-column SettingsPage and Escape returns",
)
def test_t3_pair_12_global_search_hit_to_settings_page_and_escape(env: TempUiUxEnv) -> None:
    gs = env.probe_scenario("pair12_search_to_settings", "global_search")
    ov = env.probe_scenario("pair12_search_to_settings", "settings_and_overlays")
    assert require_field(gs, "open") is False
    assert require_field(ov, "settings_page.open") is True
    assert require_field(ov, "settings_page.left_nav_px") == 220
    assert require_field(ov, "settings_page.content_max_px") == 960


@uiux_e2e_test(
    id="TEST-UIUX-T3-PAIR-13",
    tier=3,
    features=("UI-F09", "UI-F10"),
    description="Verify solid 38px header + TabShape above scrolled 1:1 stream with collapsed AgentSteps (D3/D8)",
)
def test_t3_pair_13_header_tab_shape_above_scrolled_stream_and_steps(env: TempUiUxEnv) -> None:
    hdr = env.probe_scenario("pair13_header_above_stream", "conversation_header")
    cs = env.probe_scenario("pair13_header_above_stream", "chat_stream")
    assert require_field(hdr, "height_px") == 38
    assert require_field(hdr, "overlaps_timeline") is False
    assert require_field(hdr, "tab_shape.visible") is True
    assert require_field(cs, "steps.error_drawer_open") is False
    assert require_field(cs, "activity.bubble_less") is True


@uiux_e2e_test(
    id="TEST-UIUX-T3-PAIR-14",
    tier=3,
    features=("UI-F09", "UI-F15"),
    description="Verify header title click opens SidePanel and widening triggers header fit cascade + Cmd+F shift",
)
def test_t3_pair_14_header_title_opens_side_panel_and_widening_cascade(env: TempUiUxEnv) -> None:
    hdr = env.probe_scenario("pair14_side_panel_640_cascade", "conversation_header")
    sp = env.probe_scenario("pair14_side_panel_640_cascade", "side_panel")
    cs = env.probe_scenario("pair14_side_panel_640_cascade", "chat_stream")
    assert require_field(sp, "open") is True
    assert require_field(sp, "width_px") == 640
    assert require_field(hdr, "wrapped_to_multiline") is False
    assert require_field(hdr, "actions_clipped") is False
    assert require_field(cs, "chat_search.right_offset_px") == 656


@uiux_e2e_test(
    id="TEST-UIUX-T3-PAIR-15",
    tier=3,
    features=("UI-F10", "UI-F12"),
    description="Verify 48px pill composer submit triggers neutral bubble, shimmer, and Send->Stop circle (D9/D10)",
)
def test_t3_pair_15_pill_composer_submit_to_shimmer_and_stop_circle(env: TempUiUxEnv) -> None:
    comp = env.probe_scenario("pair15_submit_turn_active", "composer")
    cs = env.probe_scenario("pair15_submit_turn_active", "chat_stream")
    assert require_field(comp, "legacy_chip_strip_present") is False
    assert require_field(comp, "focus_border_is_telegram_blue") is False
    assert require_field(comp, "action_circle_kind") == "stop"
    assert require_field(cs, "activity.visible") is True
    assert require_field(cs, "activity.bubble_less") is True
    assert_hex_eq(require_field(cs, "bubbles")[0]["bg_hex"], "#242424", "outgoing.bg")


@uiux_e2e_test(
    id="TEST-UIUX-T3-PAIR-16",
    tier=3,
    features=("UI-F11", "UI-F13"),
    description="Verify group @mention/@everyone inserts atomic ReferenceChips and emits routing divider",
)
def test_t3_pair_16_group_mention_chips_and_routing_action_marker(env: TempUiUxEnv) -> None:
    mp = env.probe_scenario("pair16_group_mention_routing", "mention_picker")
    ch = env.probe_scenario("pair16_group_mention_routing", "channel_stage")
    assert require_field(mp, "everyone_shown") is True
    assert len(require_field(mp, "rendered_chips")) >= 2
    markers = require_field(ch, "action_markers")
    assert any(m.get("kind") == "channel-routing" and m.get("clickable_pills") is True for m in markers)


@uiux_e2e_test(
    id="TEST-UIUX-T3-PAIR-17",
    tier=3,
    features=("UI-F11", "UI-F14"),
    description="Verify multi-worker group turn syncs ChannelActivityIndicator, AwaitingReplies, and ChannelStoppedTasks",
)
def test_t3_pair_17_group_workers_sync_activity_subtasks_and_stopped(env: TempUiUxEnv) -> None:
    ch = env.probe_scenario("pair17_group_workers_stack", "channel_stage")
    st = env.probe_scenario("pair17_group_workers_stack", "composer_stack")
    assert require_field(ch, "activity_indicator.visible") is True
    assert require_field(st, "awaiting_replies.visible") is True
    assert require_field(st, "stopped_tasks.visible") is True
    assert len(require_field(st, "stopped_tasks.tasks")) >= 1


@uiux_e2e_test(
    id="TEST-UIUX-T3-PAIR-18",
    tier=3,
    features=("UI-F12", "UI-F13"),
    description="Verify + ComposerAddMenu -> $ Skill opens picker and preserves pill-to-card multiline morphing",
)
def test_t3_pair_18_add_menu_skill_picker_and_multiline_morph(env: TempUiUxEnv) -> None:
    comp = env.probe_scenario("pair18_add_skill_multiline", "composer")
    mp = env.probe_scenario("pair18_add_skill_multiline", "mention_picker")
    assert require_field(comp, "compact") is False
    assert require_field(comp, "border_radius_px") == 14
    assert require_field(comp, "min_height_px") >= 96
    chips = require_field(mp, "inserted_chips")
    assert any(c.get("kind") == "skill" and c.get("height_px") == 24 for c in chips)


@uiux_e2e_test(
    id="TEST-UIUX-T3-PAIR-19",
    tier=3,
    features=("UI-F12", "UI-F14"),
    description="Verify busy follow-up queueing into .composer-stack, Tab Steer, and collapse back to 0px pill",
)
def test_t3_pair_19_busy_queueing_tab_steer_and_zero_px_collapse(env: TempUiUxEnv) -> None:
    queued = env.probe_scenario("pair19_queued_steered", "composer_stack")
    done = env.probe_scenario("pair19_collapsed_idle", "composer_stack")
    comp = env.probe_scenario("pair19_collapsed_idle", "composer")
    assert require_field(queued, "queue.steered_on_tab") is True
    assert require_field(done, "visible") is False
    assert require_field(done, "height_px") == 0
    assert require_field(comp, "compact") is True and require_field(comp, "height_px") == 48


@uiux_e2e_test(
    id="TEST-UIUX-T3-PAIR-20",
    tier=3,
    features=("UI-F14", "UI-F15"),
    description="Verify + Side Question and AwaitingReplies wait action dock cleanly inside right SidePanel",
)
def test_t3_pair_20_side_question_and_awaiting_wait_dock_in_side_panel(env: TempUiUxEnv) -> None:
    st = env.probe_scenario("pair20_stack_to_side_panel", "composer_stack")
    sp = env.probe_scenario("pair20_stack_to_side_panel", "side_panel")
    assert require_field(st, "awaiting_replies.visible") is True
    assert require_field(sp, "open") is True
    assert require_field(sp, "active_right_drawer_count") == 1
    assert len(require_field(sp, "active_splits")) <= 2


@uiux_e2e_test(
    id="TEST-UIUX-T3-PAIR-21",
    tier=3,
    features=("UI-F15", "UI-F16"),
    description="Verify 4-button SidePanelLauncher -> 打开 助手设置 hosts full Bot Profile and ContextInspector (D11)",
)
def test_t3_pair_21_launcher_to_assistant_settings_hosts_bot_profile(env: TempUiUxEnv) -> None:
    sp = env.probe_scenario("pair21_launcher_to_bot_profile", "side_panel")
    prof = env.probe_scenario("pair21_launcher_to_bot_profile", "agent_settings_profile")
    assert require_field(sp, "active_right_drawer_count") == 1
    assert require_field(sp, "legacy_standalone_inspector_mounted") is False
    assert require_field(prof, "mounted") is True
    assert require_field(prof, "host") == "side_panel_assistant_settings"
    assert require_field(prof, "context_inspector_live_reachable") is True


@uiux_e2e_test(
    id="TEST-UIUX-T3-PAIR-22",
    tier=3,
    features=("UI-F15", "UI-F17"),
    description="Verify Bot->Group->Human peer switch adapts 4th launcher button and flushes dirty settings",
)
def test_t3_pair_22_peer_switch_adapts_launcher_and_flushes_dirty_settings(env: TempUiUxEnv) -> None:
    grp_sp = env.probe_scenario("pair22_switch_to_group", "side_panel")
    grp_cs = env.probe_scenario("pair22_switch_to_group", "channel_settings")
    hum_sp = env.probe_scenario("pair22_switch_to_human", "side_panel")
    prof = env.probe_scenario("pair22_switch_to_group", "agent_settings_profile")
    assert require_field(grp_sp, "launcher_buttons")[3] == "打开 群聊设置"
    assert require_field(grp_cs, "mounted") is True
    assert require_field(prof, "dirty_flushed_to_agent_id") == "bot-a"
    assert require_field(hum_sp, "launcher_buttons")[3] == "打开 联系人资料"


@uiux_e2e_test(
    id="TEST-UIUX-T3-PAIR-23",
    tier=3,
    features=("UI-F16", "UI-F18"),
    description="Verify Bot Profile 资料与作品 deep-links cleanly into OpenBot-aligned Books/Music/Games hub",
)
def test_t3_pair_23_bot_profile_works_deep_link_to_content_hub(env: TempUiUxEnv) -> None:
    prof = env.probe_scenario("pair23_works_to_hub", "agent_settings_profile")
    hub = env.probe_scenario("pair23_works_to_hub", "content_hub_pages")
    assert require_field(prof, "creations_counts.books") >= 1
    assert require_field(hub, "active_page") in ("books", "music", "games")
    assert require_field(hub, "mounted_inside_inset_card") is True
    assert require_field(hub, "legacy_blue_pill_count") == 0


@uiux_e2e_test(
    id="TEST-UIUX-T3-PAIR-24",
    tier=3,
    features=("UI-F16", "UI-F17", "UI-F19", "UI-F20"),
    description="Verify ConfirmDialog + toaster across Bot & Channel settings while keeping ContextInspector & ledger intact",
)
def test_t3_pair_24_confirm_dialog_across_bot_channel_and_ledger_compat(env: TempUiUxEnv) -> None:
    cap = check_capability_coverage_json()
    assert cap["context_inspector_found"] is True
    ov = env.probe_scenario("pair24_confirm_and_ledger", "settings_and_overlays")
    prof = env.probe_scenario("pair24_confirm_and_ledger", "agent_settings_profile")
    cs = env.probe_scenario("pair24_confirm_and_ledger", "channel_settings")
    cl = env.probe_scenario("pair24_confirm_and_ledger", "compat_and_ledger")
    assert require_field(ov, "confirm_dialog.max_width_px") == 460
    assert require_field(ov, "confirm_dialog.initial_focus") == "cancel"
    assert require_field(prof, "context_inspector_live_reachable") is True
    assert require_field(cs, "context_actions_visible") is True
    assert require_field(cl, "preserved_consts.context_inspector_drawer_width") == 320.0
    assert require_field(cl, "ledger_closure_pass") is True
