"""Tier 4 Real-World End-to-End User Journeys (12 journeys covering UI-F01..UI-F20 & D1..D12)."""

from __future__ import annotations

from tests.e2e_openbot_uiux.harness import (
    TempUiUxEnv,
    assert_all_defects_eliminated,
    assert_defect_eliminated,
    assert_hex_eq,
    check_capability_coverage_json,
    check_cargo_toml_lean_profiles,
    check_immutable_governance_files,
    check_no_allow_dead_code,
    check_preserved_pub_consts,
    check_workspace_plush_assets,
    require_field,
    run_ledger_closure_baseline,
    uiux_e2e_test,
)
from tests.e2e_openbot_uiux.oracles import (
    channel_avatar_cluster_oracle,
    hex_to_oklab,
    oklab_readable_chat_bg,
    sidebar_hysteresis_step,
    side_panel_clamp_width,
    solid_bubble_takes_dark_text,
)


@uiux_e2e_test(
    id="TEST-UIUX-T4-JOURNEY-01",
    tier=4,
    features=("UI-F01", "UI-F02", "UI-F03", "UI-F04", "UI-F05", "UI-F06", "UI-F08"),
    description="Journey 1: Cold launch -> OpenBot onboarding & FirstAgentSetup studio (image.png; closes D1/D2/D4/D5/D7)",
)
def test_t4_journey_01_cold_launch_onboarding_and_first_agent_setup(env: TempUiUxEnv) -> None:
    sh = env.probe_scenario("j01_onboarding", "shell")
    rail = env.probe_scenario("j01_onboarding", "page_rail")
    sc = env.probe_scenario("j01_onboarding", "sidebar_chrome")
    sl = env.probe_scenario("j01_onboarding", "sidebar_list")
    st = env.probe_scenario("j01_onboarding", "agent_studio")
    plush = env.probe_scenario("j01_onboarding", "plush")
    defects = env.probe_scenario("j01_onboarding", "defects_audit")
    assert_hex_eq(require_field(sh, "outer_frame_bg_hex"), "#0d0d0d", "j01.frame")
    assert require_field(sh, "canvas_card.radius_px") == 14
    assert require_field(rail, "width_px") == 52 and require_field(rail, "total_account_footers_in_shell") == 1
    assert require_field(sl, "has_duplicate_account_footer") is False
    assert require_field(sl, "tabs.uses_blue_pill") is False
    assert require_field(sl, "pinned_grid.legacy_oversized_circles") is False
    assert_hex_eq(require_field(sc, "add_button.bg_hex"), "#f0f0f0", "j01.plus")
    assert require_field(st, "form_width_px") == 420 and require_field(st, "hero.size_px") == 72
    assert bool(require_field(st, "created_agent_id")) and require_field(st, "navigated_to_chat") is True
    assert require_field(plush, "default_bot_avatar.is_plush_3d") is True
    for d in ("d1_wide_apple_sidebar", "d2_duplicate_account_footer", "d4_oversized_pinned_circles", "d5_blue_pill_folder_tabs", "d7_flat_2d_bot_default"):
        assert_defect_eliminated(defects, d)


@uiux_e2e_test(
    id="TEST-UIUX-T4-JOURNEY-02",
    tier=4,
    features=("UI-F01", "UI-F02", "UI-F09", "UI-F10", "UI-F12"),
    description="Journey 2: Active 1:1 bot turn -> working felt badge, shimmer, collapsed error & pill composer (image_2.png; closes D3/D8/D9/D10)",
)
def test_t4_journey_02_active_bot_turn_shimmer_error_steps_and_pill_composer(env: TempUiUxEnv) -> None:
    hdr = env.probe_scenario("j02_active_bot_turn", "conversation_header")
    cs = env.probe_scenario("j02_active_bot_turn", "chat_stream")
    comp = env.probe_scenario("j02_active_bot_turn", "composer")
    plush = env.probe_scenario("j02_active_bot_turn", "plush")
    defects = env.probe_scenario("j02_active_bot_turn", "defects_audit")
    assert require_field(hdr, "height_px") == 38 and require_field(hdr, "overlaps_timeline") is False
    assert require_field(comp, "height_px") == 48 and require_field(comp, "border_radius_px") == 24
    assert require_field(comp, "legacy_chip_strip_present") is False
    assert require_field(comp, "focus_border_is_telegram_blue") is False
    assert require_field(comp, "action_circle_kind") == "stop"
    assert require_field(plush, "badges.working.width_px") == 30 and require_field(plush, "badges.working.ball_count") == 3
    assert require_field(cs, "activity.bubble_less") is True and require_field(cs, "activity.avatar_px") == 32
    assert require_field(cs, "steps.error_drawer_open") is False
    assert all(b.get("contains_raw_json_wall") is False for b in require_field(cs, "bubbles"))
    for d in ("d3_header_overlaps_messages", "d8_blue_bubble_or_raw_json_wall", "d9_two_row_composer_chip_strip", "d10_telegram_blue_composer"):
        assert_defect_eliminated(defects, d)


@uiux_e2e_test(
    id="TEST-UIUX-T4-JOURNEY-03",
    tier=4,
    features=("UI-F15", "UI-F16", "UI-F19"),
    description="Journey 3: Right SidePanel 4-button launcher -> complete Bot Profile inside 打开 助手设置 (image_2.png; closes D11)",
)
def test_t4_journey_03_side_panel_launcher_to_complete_bot_profile(env: TempUiUxEnv) -> None:
    sp = env.probe_scenario("j03_bot_profile_flow", "side_panel")
    prof = env.probe_scenario("j03_bot_profile_flow", "agent_settings_profile")
    ov = env.probe_scenario("j03_bot_profile_flow", "settings_and_overlays")
    defects = env.probe_scenario("j03_bot_profile_flow", "defects_audit")
    assert require_field(sp, "active_right_drawer_count") == 1
    assert require_field(sp, "launcher_buttons") == ["打开 浏览器", "打开 文件", "打开 话题", "打开 助手设置"]
    assert require_field(prof, "mounted") is True and require_field(prof, "host") == "side_panel_assistant_settings"
    assert require_field(prof, "avatar_px") == 62 and require_field(prof, "avatar_popover_width_px") == 284
    assert require_field(prof, "handle_copied") is True and require_field(prof, "autosave_debounce_ms") == 400
    assert require_field(prof, "context_inspector_live_reachable") is True
    assert require_field(ov, "confirm_dialog.initial_focus") == "cancel"
    assert_defect_eliminated(defects, "d11_double_right_drawers")


@uiux_e2e_test(
    id="TEST-UIUX-T4-JOURNEY-04",
    tier=4,
    features=("UI-F05", "UI-F06", "UI-F11", "UI-F13", "UI-F14"),
    description="Journey 4: Multi-bot Group Chat creation -> @mention/@everyone -> routing, subtasks & resume (closes D12)",
)
def test_t4_journey_04_group_chat_creation_mentions_routing_and_resume(env: TempUiUxEnv) -> None:
    ch = env.probe_scenario("j04_group_lifecycle", "channel_stage")
    mp = env.probe_scenario("j04_group_lifecycle", "mention_picker")
    stack = env.probe_scenario("j04_group_lifecycle", "composer_stack")
    defects = env.probe_scenario("j04_group_lifecycle", "defects_audit")
    assert require_field(ch, "create_dialog.width_px") == 440
    o4 = channel_avatar_cluster_oracle(4)
    av4 = require_field(ch, "avatar")
    assert (
        require_field(av4, "visible_faces"),
        require_field(av4, "overflow_count"),
        require_field(av4, "overflow_pill_tone"),
    ) in o4["valid_tuples"]
    assert require_field(ch, "author_gutter_px") == 36
    assert require_field(mp, "everyone_shown") is True and len(require_field(mp, "rendered_chips")) >= 2
    assert len(require_field(ch, "action_markers")) >= 1
    assert require_field(ch, "activity_indicator.visible") is True
    assert require_field(stack, "awaiting_replies.visible") is True
    assert require_field(stack, "stopped_tasks.resume_dispatch_count") >= 1
    assert require_field(ch, "reply_quote.flash_duration_ms") == 1600
    assert_defect_eliminated(defects, "d12_legacy_group_stage")


@uiux_e2e_test(
    id="TEST-UIUX-T4-JOURNEY-05",
    tier=4,
    features=("UI-F09", "UI-F11", "UI-F15", "UI-F17", "UI-F19"),
    description="Journey 5: Group Settings (ChannelSettingsPane) lifecycle -> icon, crown, isolation, confirm & archive lock",
)
def test_t4_journey_05_group_settings_picture_crown_isolation_and_archive(env: TempUiUxEnv) -> None:
    hdr = env.probe_scenario("j05_group_settings_lifecycle", "conversation_header")
    sp = env.probe_scenario("j05_group_settings_lifecycle", "side_panel")
    cs = env.probe_scenario("j05_group_settings_lifecycle", "channel_settings")
    arch = env.probe_scenario("j05_group_archived", "channel_stage")
    defects = env.probe_scenario("j05_group_settings_lifecycle", "defects_audit")
    assert require_field(sp, "active_right_drawer_count") == 1
    assert require_field(cs, "picture_popover_width_px") == 284 and require_field(cs, "icon_catalog_count") >= 20
    assert require_field(cs, "isolated") is True and require_field(hdr, "isolation_lock_visible") is True
    assert require_field(cs, "lead_agent_id") in require_field(cs, "members")
    assert require_field(arch, "read_only") is True and require_field(arch, "composer_hidden") is True
    assert_defect_eliminated(defects, "d11_double_right_drawers")
    assert_defect_eliminated(defects, "d12_legacy_group_stage")


@uiux_e2e_test(
    id="TEST-UIUX-T4-JOURNEY-06",
    tier=4,
    features=("UI-F12", "UI-F13", "UI-F14", "UI-F15"),
    description="Journey 6: Active-turn queueing, Tab steer, 4001-char paste morph & + Side Question docking (closes D9/D10)",
)
def test_t4_journey_06_queueing_tab_steer_paste_morph_and_side_question(env: TempUiUxEnv) -> None:
    comp = env.probe_scenario("j06_queue_paste_sideq", "composer")
    mp = env.probe_scenario("j06_queue_paste_sideq", "mention_picker")
    stack = env.probe_scenario("j06_queue_paste_sideq", "composer_stack")
    sp = env.probe_scenario("j06_queue_paste_sideq", "side_panel")
    defects = env.probe_scenario("j06_queue_paste_sideq", "defects_audit")
    assert require_field(comp, "pasted_as_file") is True and require_field(comp, "pasted_file_name") == "pasted-text.txt"
    assert require_field(stack, "queue.steered_on_tab") is True
    assert require_field(stack, "height_px") == 0
    assert any(c.get("kind") == "skill" for c in require_field(mp, "inserted_chips"))
    assert require_field(sp, "main_msg_delta") == 0 and require_field(sp, "active_right_drawer_count") == 1
    assert_defect_eliminated(defects, "d9_two_row_composer_chip_strip")
    assert_defect_eliminated(defects, "d10_telegram_blue_composer")


@uiux_e2e_test(
    id="TEST-UIUX-T4-JOURNEY-07",
    tier=4,
    features=("UI-F09", "UI-F10", "UI-F15"),
    description="Journey 7: Multi-split SidePanel, width clamping, maximize & narrow header fit cascade (closes D3/D11)",
)
def test_t4_journey_07_multi_split_side_panel_clamp_maximize_and_header_cascade(env: TempUiUxEnv) -> None:
    sp = env.probe_scenario("j07_side_panel_splits", "side_panel")
    hdr = env.probe_scenario("j07_side_panel_splits", "conversation_header")
    cs = env.probe_scenario("j07_side_panel_splits", "chat_stream")
    defects = env.probe_scenario("j07_side_panel_splits", "defects_audit")
    assert require_field(sp, "width_px") == side_panel_clamp_width(720.0, 1440.0, 52.0, 280.0)
    assert len(require_field(sp, "active_splits")) == 2 and require_field(sp, "max_splits") == 2
    assert require_field(sp, "overflow_chevron_visible") is True
    assert require_field(sp, "active_right_drawer_count") == 1
    assert require_field(hdr, "wrapped_to_multiline") is False and require_field(hdr, "overlaps_timeline") is False
    assert require_field(cs, "chat_search.right_offset_px") == require_field(sp, "width_px") + 16
    assert_defect_eliminated(defects, "d3_header_overlaps_messages")
    assert_defect_eliminated(defects, "d11_double_right_drawers")


@uiux_e2e_test(
    id="TEST-UIUX-T4-JOURNEY-08",
    tier=4,
    features=("UI-F03", "UI-F04", "UI-F18"),
    description="Journey 8: Other entries in 52px PageRail -> Books, Music, Games, App Store, Creations & Community",
)
def test_t4_journey_08_other_entries_in_52px_rail_all_six_hubs_flow(env: TempUiUxEnv) -> None:
    rail = env.probe_scenario("j08_all_hubs_flow", "page_rail")
    hub = env.probe_scenario("j08_all_hubs_flow", "content_hub_pages")
    sh = env.probe_scenario("j08_all_hubs_flow", "shell")
    defects = env.probe_scenario("j08_all_hubs_flow", "defects_audit")
    assert require_field(rail, "width_px") == 52 and "my_creations" in require_field(rail, "persisted_pins")
    assert require_field(sh, "canvas_card.radius_px") == 14
    assert require_field(hub, "mounted_inside_inset_card") is True
    assert require_field(hub, "page_header_height_px") == 38 and require_field(hub, "left_nav_width_px") == 280
    assert require_field(hub, "content_max_width_px") == 880 and require_field(hub, "legacy_blue_pill_count") == 0
    assert require_field(hub, "reader_page_index") >= 1 and require_field(hub, "music_playing") is True
    assert require_field(hub, "game_runtime_active") is True and require_field(hub, "publish_submitted") is True
    assert require_field(hub, "community_interactions_ok") is True
    for d in ("d1_wide_apple_sidebar", "d2_duplicate_account_footer", "d5_blue_pill_folder_tabs"):
        assert_defect_eliminated(defects, d)


@uiux_e2e_test(
    id="TEST-UIUX-T4-JOURNEY-09",
    tier=4,
    features=("UI-F05", "UI-F06", "UI-F07", "UI-F09", "UI-F19"),
    description="Journey 9: Power-user navigation -> Cmd+K, Cmd+B, 88px hysteresis, history & shortcuts (closes D1/D4/D5/D6)",
)
def test_t4_journey_09_power_user_navigation_search_hysteresis_and_shortcuts(env: TempUiUxEnv) -> None:
    sc = env.probe_scenario("j09_power_nav", "sidebar_chrome")
    sl = env.probe_scenario("j09_power_nav", "sidebar_list")
    gs = env.probe_scenario("j09_power_nav", "global_search")
    hdr = env.probe_scenario("j09_power_nav", "conversation_header")
    ov = env.probe_scenario("j09_power_nav", "settings_and_overlays")
    defects = env.probe_scenario("j09_power_nav", "defects_audit")
    assert sidebar_hysteresis_step("compact", 114.0) == ("compact", 88.0)
    assert require_field(sc, "collapse_threshold_px") == 110 and require_field(sc, "expand_threshold_px") == 118
    assert require_field(sl, "rows")[0]["min_w_0"] is True
    assert require_field(hdr, "shifted_start_px") > 0 and require_field(hdr, "overlaps_timeline") is False
    assert require_field(gs, "plush.size_px") == 56 and require_field(ov, "shortcuts.last_refusal") in ("invalid", "reserved", "taken")
    for d in ("d1_wide_apple_sidebar", "d4_oversized_pinned_circles", "d5_blue_pill_folder_tabs", "d6_cjk_preview_clipped"):
        assert_defect_eliminated(defects, d)


@uiux_e2e_test(
    id="TEST-UIUX-T4-JOURNEY-10",
    tier=4,
    features=("UI-F01", "UI-F03", "UI-F10", "UI-F18", "UI-F19"),
    description="Journey 10: Appearance personalization -> Dark<->Light, 12-color palette, OKLab & SettingsPage (closes D5/D8/D10)",
)
def test_t4_journey_10_appearance_personalization_dark_light_oklab_and_settings(env: TempUiUxEnv) -> None:
    tok = env.probe_scenario("j10_appearance_light_oklab", "tokens")
    sh = env.probe_scenario("j10_appearance_light_oklab", "shell")
    cs = env.probe_scenario("j10_appearance_light_oklab", "chat_stream")
    hub = env.probe_scenario("j10_appearance_light_oklab", "content_hub_pages")
    ov = env.probe_scenario("j10_appearance_light_oklab", "settings_and_overlays")
    defects = env.probe_scenario("j10_appearance_light_oklab", "defects_audit")
    assert require_field(ov, "settings_page.left_nav_px") == 220 and require_field(ov, "settings_page.content_max_px") == 960
    assert_hex_eq(require_field(sh, "outer_frame_bg_hex"), "#e8e8eb", "j10.light_frame")
    s0 = require_field(tok, "oklab_samples")[0]
    out_hex = require_field(s0, "result_hex")
    assert_hex_eq(out_hex, oklab_readable_chat_bg(require_field(s0, "hex"), require_field(s0, "scheme")))
    L, _, _, C = hex_to_oklab(out_hex)
    assert (L >= 0.945 and C <= 0.025) if s0["scheme"] == "light" else (L <= 0.305 and C <= 0.055)
    assert solid_bubble_takes_dark_text("#e5a50a") is True
    assert require_field(cs, "activity.shimmer_frozen_reduced_motion") is True
    assert require_field(hub, "legacy_blue_pill_count") == 0
    for d in ("d5_blue_pill_folder_tabs", "d8_blue_bubble_or_raw_json_wall", "d10_telegram_blue_composer"):
        assert_defect_eliminated(defects, d)


@uiux_e2e_test(
    id="TEST-UIUX-T4-JOURNEY-11",
    tier=4,
    features=("UI-F10", "UI-F15", "UI-F16", "UI-F17"),
    description="Journey 11: 1:1 Human DirectConversation + 打开 联系人资料 + cross-peer dirty settings flush (closes D11)",
)
def test_t4_journey_11_human_direct_dm_contact_profile_and_dirty_flush(env: TempUiUxEnv) -> None:
    cs = env.probe_scenario("j11_human_dm_and_flush", "chat_stream")
    sp = env.probe_scenario("j11_human_dm_and_flush", "side_panel")
    prof = env.probe_scenario("j11_human_dm_and_flush", "agent_settings_profile")
    cp = env.probe_scenario("j11_human_dm_and_flush", "contact_profile")
    defects = env.probe_scenario("j11_human_dm_and_flush", "defects_audit")
    assert require_field(cs, "direct_dm.stream_width_px") == 760
    assert require_field(sp, "launcher_buttons")[3] == "打开 联系人资料"
    assert require_field(sp, "active_right_drawer_count") == 1
    assert require_field(cp, "mounted") is True and require_field(cp, "host") == "side_panel_contact_profile"
    assert require_field(prof, "dirty_flushed_to_agent_id") == "bot-a"
    assert require_field(prof, "flushed_name") == "BotA-Edited"
    assert_defect_eliminated(defects, "d11_double_right_drawers")


@uiux_e2e_test(
    id="TEST-UIUX-T4-JOURNEY-12",
    tier=4,
    features=("UI-F02", "UI-F16", "UI-F20"),
    description="Journey 12: Full compatibility, plush assets, ledger closure & D1-D12 zero-regression certification",
)
def test_t4_journey_12_full_compat_ledger_closure_and_d1_to_d12_certification(env: TempUiUxEnv) -> None:
    root = env.probe_full()
    cl = require_field(root, "compat_and_ledger")
    assert require_field(cl, "headless_gpui_render_ok") is True
    assert require_field(cl, "ledger_closure_pass") is True
    assert_all_defects_eliminated(require_field(root, "defects_audit"))
    assert check_preserved_pub_consts()["all_preserved"] is True
    assert check_no_allow_dead_code()["offenders_count"] == 0
    assert check_workspace_plush_assets()["total_files"] == 74
    assert check_capability_coverage_json()["context_inspector_found"] is True
    assert check_cargo_toml_lean_profiles()["extra_target_dirs"] == []
    assert check_immutable_governance_files()["ledger_entries"] == 3441
    assert run_ledger_closure_baseline(strict_dist=False)["passed"] is True
