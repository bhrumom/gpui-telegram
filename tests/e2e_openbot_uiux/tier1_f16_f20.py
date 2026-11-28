"""Tier 1 Isolated Feature Tests: UI-F16 through UI-F20 (30 opaque-box tests)."""

from __future__ import annotations

from tests.e2e_openbot_uiux.harness import (
    PROBE_SCHEMA_VERSION,
    PROBE_SUITE_NAME,
    TempUiUxEnv,
    assert_hex_eq,
    assert_within,
    check_capability_coverage_json,
    check_no_allow_dead_code,
    check_preserved_pub_consts,
    check_workspace_plush_assets,
    require_field,
    run_ledger_closure_baseline,
    uiux_e2e_test,
)
from tests.e2e_openbot_uiux.oracles import CANONICAL_CHANNEL_ICONS_20


# ---------------------------------------------------------------------------
# UI-F16: Bot Profile Page Embedded Inside 打开 助手设置 (6 tests)
# ---------------------------------------------------------------------------


@uiux_e2e_test(
    id="TEST-UIUX-T1-UI-F16-01",
    tier=1,
    features=("UI-F16",),
    description="Verify Bot Profile inside 打开 助手设置 with 62px plush hero and 284px Avatar Editor popover",
)
def test_t1_f16_01_embedded_bot_profile_hero_and_avatar_editor_popover(env: TempUiUxEnv) -> None:
    prof = env.probe_scenario("f16_avatar_popover", "agent_settings_profile")
    assert require_field(prof, "mounted") is True
    assert require_field(prof, "host") == "side_panel_assistant_settings"
    assert require_field(prof, "avatar_px") == 62
    assert require_field(prof, "avatar_popover_open") is True
    assert require_field(prof, "avatar_popover_width_px") == 284
    assert require_field(prof, "avatar_shapes_count") == 13
    assert require_field(prof, "avatar_eyes_count") == 16
    assert require_field(prof, "avatar_colors_count") == 18


@uiux_e2e_test(
    id="TEST-UIUX-T1-UI-F16-02",
    tier=1,
    features=("UI-F16",),
    description="Verify Bot identity block (Name, Title, copyable @handle, online badge, Bio) and 400ms auto-save",
)
def test_t1_f16_02_bot_identity_block_handle_copy_and_400ms_autosave(env: TempUiUxEnv) -> None:
    prof = env.probe_scenario("f16_identity_autosave", "agent_settings_profile")
    assert require_field(prof, "online") is True
    assert require_field(prof, "handle").startswith("@")
    assert require_field(prof, "handle_copied") is True
    assert require_field(prof, "autosave_debounce_ms") == 400
    assert bool(require_field(prof, "name").strip())
    assert bool(require_field(prof, "bio").strip())


@uiux_e2e_test(
    id="TEST-UIUX-T1-UI-F16-03",
    tier=1,
    features=("UI-F16",),
    description="Verify SettingsLinkGroup inset card spec (#212121, 8px, 40px rows) and 资料与作品 section",
)
def test_t1_f16_03_inset_card_spec_and_profile_works_section(env: TempUiUxEnv) -> None:
    prof = env.probe_scenario("f16_profile_works", "agent_settings_profile")
    card = require_field(prof, "inset_card_style")
    assert_hex_eq(require_field(card, "bg_hex"), "#212121", "inset_card.bg")
    assert require_field(card, "radius_px") == 8
    assert require_field(card, "row_height_px") == 40
    fields = set(require_field(prof, "profile_fields"))
    for req in ("creations_books_music_games", "add_to_group", "shared_media", "notify_me"):
        assert req in fields, f"Missing Bot Profile field {req!r}"
    counts = require_field(prof, "creations_counts")
    assert require_field(counts, "books") >= 0 and require_field(counts, "music") >= 0
    assert require_field(counts, "games") >= 0


@uiux_e2e_test(
    id="TEST-UIUX-T1-UI-F16-04",
    tier=1,
    features=("UI-F16",),
    description="Verify 核心 (Brain) and 已知 (Knows) inset groups and in-panel subpage drill-down",
)
def test_t1_f16_04_brain_and_knows_groups_with_subpage_drilldown(env: TempUiUxEnv) -> None:
    prof = env.probe_scenario("f16_brain_knows_drill", "agent_settings_profile")
    groups = set(require_field(prof, "groups_present"))
    assert {"brain", "knows"}.issubset(groups)
    assert require_field(prof, "active_subpage") == "memories"
    assert require_field(prof, "subpage_back_restores_root") is True


@uiux_e2e_test(
    id="TEST-UIUX-T1-UI-F16-05",
    tier=1,
    features=("UI-F16",),
    description="Verify 行为 (Does) and 规则 (Rules) groups including Permissions and destructive ConfirmDialog actions",
)
def test_t1_f16_05_does_and_rules_groups_and_advanced_actions(env: TempUiUxEnv) -> None:
    prof = env.probe_scenario("f16_does_rules", "agent_settings_profile")
    groups = set(require_field(prof, "groups_present"))
    assert {"does", "rules"}.issubset(groups)
    assert require_field(prof, "permission_level") in ("workspace", "ask", "full")
    destructive = set(require_field(prof, "destructive_confirm_dialogs"))
    assert {"new_chat", "delete_history", "delete_agent"}.issubset(destructive)


@uiux_e2e_test(
    id="TEST-UIUX-T1-UI-F16-06",
    tier=1,
    features=("UI-F16",),
    description="Verify ContextInspector stays live-reachable inside 打开 助手设置 and verified in capability-coverage.json",
)
def test_t1_f16_06_context_inspector_live_reachable_in_assistant_settings(env: TempUiUxEnv) -> None:
    cap = check_capability_coverage_json()
    assert cap["context_inspector_found"] is True
    prof = env.probe_scenario("f16_context_inspector", "agent_settings_profile")
    sp = env.probe_scenario("f16_context_inspector", "side_panel")
    assert require_field(prof, "context_inspector_live_reachable") is True
    assert require_field(prof, "host") == "side_panel_assistant_settings"
    assert require_field(sp, "active_right_drawer_count") == 1


# ---------------------------------------------------------------------------
# UI-F17: ChannelSettingsPane & Human ContactProfile (6 tests)
# ---------------------------------------------------------------------------


@uiux_e2e_test(
    id="TEST-UIUX-T1-UI-F17-01",
    tier=1,
    features=("UI-F17",),
    description="Verify 62px ChannelPictureEditor with 284px popover, >=20 canonical icons, >=10 colors, and Reset",
)
def test_t1_f17_01_channel_picture_editor_icons_colors_and_reset(env: TempUiUxEnv) -> None:
    cs = env.probe_scenario("f17_picture_editor", "channel_settings")
    assert require_field(cs, "mounted") is True
    assert require_field(cs, "picture_trigger_px") == 62
    assert require_field(cs, "picture_popover_width_px") == 284
    assert require_field(cs, "icon_catalog_count") >= 20
    assert set(CANONICAL_CHANNEL_ICONS_20).issubset(set(require_field(cs, "canonical_icons_present")))
    assert require_field(cs, "color_palette_count") >= 10
    assert require_field(cs, "resolved_icon") == "code"
    assert require_field(cs, "reset_restores_cluster") is True


@uiux_e2e_test(
    id="TEST-UIUX-T1-UI-F17-02",
    tier=1,
    features=("UI-F17",),
    description="Verify group Name, Title, Instructions auto-save, Browser access selector, and Isolation Lock",
)
def test_t1_f17_02_group_fields_browser_access_and_isolation_switch(env: TempUiUxEnv) -> None:
    cs = env.probe_scenario("f17_isolation_on", "channel_settings")
    hdr = env.probe_scenario("f17_isolation_on", "conversation_header")
    assert bool(require_field(cs, "name").strip())
    assert require_field(cs, "browser_access") not in ("", None)
    assert require_field(cs, "isolated") is True
    assert require_field(hdr, "isolation_lock_visible") is True


@uiux_e2e_test(
    id="TEST-UIUX-T1-UI-F17-03",
    tier=1,
    features=("UI-F17",),
    description="Verify ChannelSettingsPane Memories, Routines, and Folders drill-down subpages",
)
def test_t1_f17_03_group_subpages_memories_routines_folders(env: TempUiUxEnv) -> None:
    cs = env.probe_scenario("f17_subpages", "channel_settings")
    subpages = require_field(cs, "subpages_present")
    assert set(subpages) >= {"memories", "routines", "folders"}
    counts = require_field(cs, "subpage_counts")
    assert require_field(counts, "memories") >= 0
    assert require_field(counts, "routines") >= 0
    assert require_field(counts, "folders") >= 0


@uiux_e2e_test(
    id="TEST-UIUX-T1-UI-F17-04",
    tier=1,
    features=("UI-F17",),
    description="Verify ChannelContextActions (Compact, New, Delete history) gated by ConfirmDialog",
)
def test_t1_f17_04_channel_context_actions_with_confirm_dialog(env: TempUiUxEnv) -> None:
    cs = env.probe_scenario("f17_context_actions", "channel_settings")
    ov = env.probe_scenario("f17_context_actions", "settings_and_overlays")
    assert require_field(cs, "context_actions_visible") is True
    assert set(require_field(cs, "context_actions")) >= {"compact", "new_conversation", "delete_history"}
    assert require_field(ov, "confirm_dialog.open") is True


@uiux_e2e_test(
    id="TEST-UIUX-T1-UI-F17-05",
    tier=1,
    features=("UI-F17",),
    description="Verify Members roster Crown lead toggle, Remove, + Add searchable popover, and no drag reorder",
)
def test_t1_f17_05_members_roster_crown_lead_add_and_remove(env: TempUiUxEnv) -> None:
    cs = env.probe_scenario("f17_members_roster", "channel_settings")
    members = require_field(cs, "members")
    assert len(members) >= 2
    assert require_field(cs, "lead_agent_id") in members
    assert require_field(cs, "member_drag_reorder_supported") is False
    assert require_field(cs, "member_add_accepted") is True


@uiux_e2e_test(
    id="TEST-UIUX-T1-UI-F17-06",
    tier=1,
    features=("UI-F17",),
    description="Verify 1:1 human 打开 联系人资料 pane mounts inside unified right SidePanel",
)
def test_t1_f17_06_human_contact_profile_inside_side_panel(env: TempUiUxEnv) -> None:
    cp = env.probe_scenario("f17_human_contact", "contact_profile")
    sp = env.probe_scenario("f17_human_contact", "side_panel")
    assert require_field(cp, "mounted") is True
    assert require_field(cp, "host") == "side_panel_contact_profile"
    assert require_field(sp, "active_right_drawer_count") == 1
    assert require_field(cp, "handle").startswith("@")
    assert_hex_eq(require_field(cp, "online_dot_hex"), "#31cf76", "contact.online_dot")
    assert require_field(cp, "destructive_confirm_gated") is True


# ---------------------------------------------------------------------------
# UI-F18: OpenBot Visual Alignment of Secondary Hubs (6 tests)
# ---------------------------------------------------------------------------


@uiux_e2e_test(
    id="TEST-UIUX-T1-UI-F18-01",
    tier=1,
    features=("UI-F18",),
    description="Verify 2-column OpenBot browse geometry across Books, Music, Games, App Store, Creations, Community",
)
def test_t1_f18_01_openbot_two_column_browse_shell_all_six_hubs(env: TempUiUxEnv) -> None:
    hub = env.probe_scenario("f18_hubs_geometry", "content_hub_pages")
    assert require_field(hub, "mounted_inside_inset_card") is True
    assert require_field(hub, "page_header_height_px") == 38
    assert require_field(hub, "left_nav_width_px") == 280
    assert require_field(hub, "content_max_width_px") == 880
    assert_hex_eq(require_field(hub, "card_surface_hex"), "#212121", "hub.card")
    assert_hex_eq(require_field(hub, "hairline_hex"), "#262626", "hub.hairline")
    assert_hex_eq(require_field(hub, "primary_cta_bg_hex"), "#f0f0f0", "hub.cta_bg")
    assert_hex_eq(require_field(hub, "primary_cta_fg_hex"), "#141414", "hub.cta_fg")
    assert require_field(hub, "legacy_blue_pill_count") == 0


@uiux_e2e_test(
    id="TEST-UIUX-T1-UI-F18-02",
    tier=1,
    features=("UI-F18",),
    description="Verify Books hub OpenBot restyle and interactive BookReader page navigation",
)
def test_t1_f18_02_books_hub_restyle_and_book_reader(env: TempUiUxEnv) -> None:
    hub = env.probe_scenario("f18_books_reader", "content_hub_pages")
    assert require_field(hub, "active_page") == "books"
    assert require_field(hub, "legacy_blue_pill_count") == 0
    assert require_field(hub, "reader_page_index") >= 1
    assert_hex_eq(require_field(hub, "card_surface_hex"), "#212121", "books.card")


@uiux_e2e_test(
    id="TEST-UIUX-T1-UI-F18-03",
    tier=1,
    features=("UI-F18",),
    description="Verify Music hub OpenBot restyle and persistent MusicPlayerBar playback state",
)
def test_t1_f18_03_music_hub_restyle_and_music_player_bar(env: TempUiUxEnv) -> None:
    hub = env.probe_scenario("f18_music_player", "content_hub_pages")
    assert require_field(hub, "active_page") == "music"
    assert require_field(hub, "music_playing") is True
    assert bool(require_field(hub, "music_track_id"))
    assert require_field(hub, "music_position_sec") >= 0
    assert require_field(hub, "legacy_blue_pill_count") == 0


@uiux_e2e_test(
    id="TEST-UIUX-T1-UI-F18-04",
    tier=1,
    features=("UI-F18",),
    description="Verify Games hub OpenBot restyle and interactive GameRuntimeSheet execution",
)
def test_t1_f18_04_games_hub_restyle_and_game_runtime(env: TempUiUxEnv) -> None:
    hub = env.probe_scenario("f18_games_runtime", "content_hub_pages")
    assert require_field(hub, "active_page") == "games"
    assert require_field(hub, "game_runtime_active") is True
    assert require_field(hub, "game_score") >= 1
    assert require_field(hub, "legacy_blue_pill_count") == 0


@uiux_e2e_test(
    id="TEST-UIUX-T1-UI-F18-05",
    tier=1,
    features=("UI-F18",),
    description="Verify App Store and Cloud Creations OpenBot restyle plus PublishReviewModal flow",
)
def test_t1_f18_05_app_store_and_creations_publish_review(env: TempUiUxEnv) -> None:
    hub = env.probe_scenario("f18_creations_publish", "content_hub_pages")
    assert require_field(hub, "active_page") in ("app_store", "my_creations")
    assert require_field(hub, "publish_status") in ("pending_review", "published")
    assert require_field(hub, "publish_submitted") is True
    assert require_field(hub, "legacy_blue_pill_count") == 0


@uiux_e2e_test(
    id="TEST-UIUX-T1-UI-F18-06",
    tier=1,
    features=("UI-F18",),
    description="Verify Community workspace OpenBot visual alignment with zero blue pills",
)
def test_t1_f18_06_community_workspace_openbot_alignment(env: TempUiUxEnv) -> None:
    hub = env.probe_scenario("f18_community", "content_hub_pages")
    assert require_field(hub, "active_page") == "community"
    assert require_field(hub, "page_header_height_px") == 38
    assert_hex_eq(require_field(hub, "card_surface_hex"), "#212121", "community.card")
    assert require_field(hub, "legacy_blue_pill_count") == 0
    assert require_field(hub, "community_interactions_ok") is True


# ---------------------------------------------------------------------------
# UI-F19: OpenBot 2-Column SettingsPage, ConfirmDialog, Menus, Tooltips & Shortcuts (6 tests)
# ---------------------------------------------------------------------------


@uiux_e2e_test(
    id="TEST-UIUX-T1-UI-F19-01",
    tier=1,
    features=("UI-F19",),
    description="Verify 2-column SettingsPage (220px nav, 960px content, #1e1e1e 12px cards) and live search",
)
def test_t1_f19_01_two_column_settings_page_and_cross_tab_search(env: TempUiUxEnv) -> None:
    ov = env.probe_scenario("f19_settings_search", "settings_and_overlays")
    sp = require_field(ov, "settings_page")
    assert require_field(sp, "open") is True
    assert require_field(sp, "left_nav_px") == 220
    assert require_field(sp, "content_max_px") == 960
    assert_hex_eq(require_field(sp, "card_bg_hex"), "#1e1e1e", "settings.card")
    assert require_field(sp, "card_radius_px") == 12
    assert require_field(sp, "row_min_height_px") == 64
    assert 1 <= require_field(sp, "search_results_count") <= 12


@uiux_e2e_test(
    id="TEST-UIUX-T1-UI-F19-02",
    tier=1,
    features=("UI-F19",),
    description="Verify SettingsPage SaveBarDock appears on dirty edit, discards cleanly, and saves with toast",
)
def test_t1_f19_02_save_bar_dock_dirty_discard_and_save(env: TempUiUxEnv) -> None:
    dirty = env.probe_scenario("f19_save_bar_dirty", "settings_and_overlays")
    saved = env.probe_scenario("f19_save_bar_saved", "settings_and_overlays")
    assert require_field(dirty, "settings_page.save_bar_dock_visible") is True
    assert require_field(saved, "settings_page.save_bar_dock_visible") is False
    toasts = require_field(saved, "toaster.toasts")
    assert any(t.get("tone") == "success" for t in toasts)


@uiux_e2e_test(
    id="TEST-UIUX-T1-UI-F19-03",
    tier=1,
    features=("UI-F19",),
    description="Verify unified ConfirmDialog 460px max width, 16px radius, #161616 bg, and Cancel default focus",
)
def test_t1_f19_03_confirm_dialog_visual_spec_and_destructive_focus(env: TempUiUxEnv) -> None:
    ov = env.probe_scenario("f19_confirm_destructive", "settings_and_overlays")
    cd = require_field(ov, "confirm_dialog")
    assert require_field(cd, "open") is True
    assert require_field(cd, "max_width_px") == 460
    assert require_field(cd, "radius_px") == 16
    assert_hex_eq(require_field(cd, "bg_hex"), "#161616", "confirm.bg")
    assert require_field(cd, "initial_focus") == "cancel"


@uiux_e2e_test(
    id="TEST-UIUX-T1-UI-F19-04",
    tier=1,
    features=("UI-F19",),
    description="Verify unified ui-action-menu 14px radius, #212121 surface, 32px rows, and z-index 40",
)
def test_t1_f19_04_action_menu_spec_radius_14_bg_212121_rows_32(env: TempUiUxEnv) -> None:
    ov = env.probe_scenario("f19_action_menu", "settings_and_overlays")
    am = require_field(ov, "action_menu")
    assert require_field(am, "open") is True
    assert_within(require_field(am, "width_px"), 192, 288, "action_menu.width")
    assert require_field(am, "radius_px") == 14
    assert_hex_eq(require_field(am, "bg_hex"), "#212121", "action_menu.bg")
    assert require_field(am, "row_height_px") == 32
    assert require_field(am, "z_index") == 40


@uiux_e2e_test(
    id="TEST-UIUX-T1-UI-F19-05",
    tier=1,
    features=("UI-F19",),
    description="Verify unified ui-tooltip (z=240), ui-toaster (z=120), and OpenBot MainMenuDrawer",
)
def test_t1_f19_05_tooltip_toaster_and_main_menu_drawer(env: TempUiUxEnv) -> None:
    ov = env.probe_scenario("f19_overlays_all", "settings_and_overlays")
    tip = require_field(ov, "tooltip")
    assert require_field(tip, "visible") is True
    assert require_field(tip, "radius_px") in (6, 8)
    assert require_field(tip, "z_index") == 240
    assert require_field(ov, "toaster.z_index") == 120
    assert require_field(ov, "main_menu_drawer.uses_legacy_blue_header") is False


@uiux_e2e_test(
    id="TEST-UIUX-T1-UI-F19-06",
    tier=1,
    features=("UI-F19",),
    description="Verify complete keyboard shortcut matrix exposes 11 configurable + 4 fixed commands",
)
def test_t1_f19_06_keyboard_shortcut_matrix_11_configurable_4_fixed(env: TempUiUxEnv) -> None:
    ov = env.probe_scenario("f19_shortcuts_matrix", "settings_and_overlays")
    sc = require_field(ov, "shortcuts")
    assert require_field(sc, "configurable_count") == 11
    assert require_field(sc, "fixed_count") == 4
    assert require_field(sc, "all_defaults_dispatched") is True


# ---------------------------------------------------------------------------
# UI-F20: Test Alignment, Headless GPUI UI E2E & Ledger Closure (6 tests)
# ---------------------------------------------------------------------------


@uiux_e2e_test(
    id="TEST-UIUX-T1-UI-F20-01",
    tier=1,
    features=("UI-F20",),
    description="Verify preserved backward-compatible pub consts on disk and in live runtime probe",
)
def test_t1_f20_01_preserved_backward_compatible_pub_consts(env: TempUiUxEnv) -> None:
    ws = check_preserved_pub_consts()
    assert ws["all_preserved"] is True
    cl = env.probe_section("compat_and_ledger")
    pc = require_field(cl, "preserved_consts")
    assert require_field(pc, "nav_rail_width") == 52.0
    assert require_field(pc, "chat_sidebar_default_width") == 288.0
    assert require_field(pc, "chat_sidebar_collapsed_width") == 64.0
    assert require_field(pc, "chat_sidebar_row_height") == 62.0
    assert require_field(pc, "context_inspector_drawer_width") == 320.0
    assert require_field(pc, "msg_min_width") == 160.0
    assert require_field(pc, "msg_max_width") == 430.0


@uiux_e2e_test(
    id="TEST-UIUX-T1-UI-F20-02",
    tier=1,
    features=("UI-F20",),
    description="Verify zero #![allow(dead_code)] or #![allow(unused)] attributes across desktop views",
)
def test_t1_f20_02_zero_allow_dead_code_across_desktop_views(env: TempUiUxEnv) -> None:
    ws = check_no_allow_dead_code()
    assert ws["scanned_files"] > 100
    assert ws["offenders_count"] == 0
    cl = env.probe_section("compat_and_ledger")
    assert require_field(cl, "allow_dead_code_count") == 0


@uiux_e2e_test(
    id="TEST-UIUX-T1-UI-F20-03",
    tier=1,
    features=("UI-F20",),
    description="Verify verify_ledger_closure.py core gates PASS and >=1090 targets verified",
)
def test_t1_f20_03_ledger_closure_1090_targets_pass(env: TempUiUxEnv) -> None:
    cl = env.probe_section("compat_and_ledger")
    assert require_field(cl, "ledger_targets_verified") >= 1090
    assert require_field(cl, "ledger_closure_pass") is True
    res = run_ledger_closure_baseline(strict_dist=False)
    assert res["passed"] is True
    assert res["non_dist_gate67_violations"] == 0


@uiux_e2e_test(
    id="TEST-UIUX-T1-UI-F20-04",
    tier=1,
    features=("UI-F20",),
    description="Verify capability-coverage.json 58 capabilities, 127 entrypoints, and ContextInspector integrity",
)
def test_t1_f20_04_capability_coverage_entrypoint_integrity(env: TempUiUxEnv) -> None:
    cap = check_capability_coverage_json()
    assert cap["capabilities_count"] == 58
    assert cap["entrypoints_count"] == 127
    assert cap["context_inspector_found"] is True
    cl = env.probe_section("compat_and_ledger")
    assert require_field(cl, "context_inspector_capability_verified") is True


@uiux_e2e_test(
    id="TEST-UIUX-T1-UI-F20-05",
    tier=1,
    features=("UI-F20",),
    description="Verify 73 .webp + 1 .webm bundled plush assets and package_release.py bundling rule",
)
def test_t1_f20_05_bundled_plush_assets_inventory_and_packaging(env: TempUiUxEnv) -> None:
    ws = check_workspace_plush_assets()
    assert ws["total_files"] == 74
    assert ws["pkg_bundles_assets"] is True
    cl = env.probe_section("compat_and_ledger")
    assert require_field(cl, "plush_asset_count") == 74


@uiux_e2e_test(
    id="TEST-UIUX-T1-UI-F20-06",
    tier=1,
    features=("UI-F20",),
    description="Verify headless GPUI runtime probe schema completeness and deterministic replay",
)
def test_t1_f20_06_headless_gpui_probe_schema_completeness(env: TempUiUxEnv) -> None:
    root = env.probe_full()
    assert require_field(root, "schema_version") == PROBE_SCHEMA_VERSION
    assert require_field(root, "suite") == PROBE_SUITE_NAME
    cl = require_field(root, "compat_and_ledger")
    assert require_field(cl, "headless_gpui_render_ok") is True
    assert require_field(cl, "replay_deterministic") is True
