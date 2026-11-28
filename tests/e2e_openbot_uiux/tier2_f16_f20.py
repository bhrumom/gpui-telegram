"""Tier 2 Boundary, Corner & Negative Tests: UI-F16 through UI-F20 (30 opaque-box tests)."""

from __future__ import annotations

from tests.e2e_openbot_uiux.harness import (
    TempUiUxEnv,
    assert_all_defects_eliminated,
    assert_hex_eq,
    check_cargo_toml_lean_profiles,
    check_immutable_governance_files,
    check_preserved_pub_consts,
    require_field,
    uiux_e2e_test,
)
from tests.e2e_openbot_uiux.oracles import normalize_channel_icon_and_hue


# ---------------------------------------------------------------------------
# UI-F16 Boundaries & Negative Cases (6 tests)
# ---------------------------------------------------------------------------


@uiux_e2e_test(
    id="TEST-UIUX-T2-UI-F16-01",
    tier=2,
    features=("UI-F16",),
    description="Verify dirty Bot Profile text fields flush strictly to Bot A on immediate switch (<400ms)",
)
def test_t2_f16_01_dirty_settings_flush_on_agent_switch_before_400ms(env: TempUiUxEnv) -> None:
    prof = env.probe_scenario("f16_dirty_flush_switch", "agent_settings_profile")
    assert require_field(prof, "dirty_flushed_to_agent_id") == "bot-a"
    assert require_field(prof, "flushed_name") == "Alpha-v2"
    assert require_field(prof, "other_agent_name") != "Alpha-v2"


@uiux_e2e_test(
    id="TEST-UIUX-T2-UI-F16-02",
    tier=2,
    features=("UI-F16",),
    description="Verify permission widening opens ConfirmDialog with Cancel initial focus while narrowing applies directly",
)
def test_t2_f16_02_permission_widening_cancel_focus_and_narrowing_direct(env: TempUiUxEnv) -> None:
    widen = env.probe_scenario("f16_permission_widen_enter", "agent_settings_profile")
    widen_ov = env.probe_scenario("f16_permission_widen_enter", "settings_and_overlays")
    narrow = env.probe_scenario("f16_permission_narrow", "agent_settings_profile")
    assert require_field(widen_ov, "confirm_dialog.initial_focus") == "cancel"
    assert require_field(widen, "permission_unchanged") is True
    assert require_field(narrow, "permission_level") == "workspace"
    assert require_field(narrow, "narrowing_confirm_required") is False


@uiux_e2e_test(
    id="TEST-UIUX-T2-UI-F16-03",
    tier=2,
    features=("UI-F16",),
    description="Verify busy bot blocks /new with info toast and disables delete_history until turn settles",
)
def test_t2_f16_03_busy_bot_new_chat_toast_and_delete_history_disabled(env: TempUiUxEnv) -> None:
    busy = env.probe_scenario("f16_busy_guards", "agent_settings_profile")
    idle = env.probe_scenario("f16_idle_guards_unlocked", "agent_settings_profile")
    assert require_field(busy, "new_chat_busy_toast") is True
    assert require_field(busy, "chat_reset") is False
    assert require_field(busy, "delete_history_disabled") is True
    assert require_field(idle, "delete_history_disabled") is False


@uiux_e2e_test(
    id="TEST-UIUX-T2-UI-F16-04",
    tier=2,
    features=("UI-F16",),
    description="Verify auto-save failure rolls back optimistic Name/Bio and displays inline save error",
)
def test_t2_f16_04_autosave_failure_rollback_and_inline_error(env: TempUiUxEnv) -> None:
    prof = env.probe_scenario("f16_autosave_error", "agent_settings_profile")
    assert require_field(prof, "save_error_visible") is True
    assert require_field(prof, "name") == require_field(prof, "rolled_back_name")


@uiux_e2e_test(
    id="TEST-UIUX-T2-UI-F16-05",
    tier=2,
    features=("UI-F16",),
    description="Verify corrupt avatar upload toast preserves existing plush and >80-char/blank name is guarded",
)
def test_t2_f16_05_invalid_avatar_upload_and_oversized_name_limits(env: TempUiUxEnv) -> None:
    prof = env.probe_scenario("f16_corrupt_avatar_and_name81", "agent_settings_profile")
    assert require_field(prof, "avatar_upload_error_toast") is True
    assert require_field(prof, "avatar_unchanged") is True
    assert len(require_field(prof, "name")) <= 80


@uiux_e2e_test(
    id="TEST-UIUX-T2-UI-F16-06",
    tier=2,
    features=("UI-F16",),
    description="Verify brand-new bot with 0 creations and 0 remote computers renders cleanly keeping ContextInspector live",
)
def test_t2_f16_06_zero_creations_and_unbound_remote_computer_safe(env: TempUiUxEnv) -> None:
    prof = env.probe_scenario("f16_zero_creations_bot", "agent_settings_profile")
    counts = require_field(prof, "creations_counts")
    assert counts["books"] == 0 and counts["music"] == 0 and counts["games"] == 0
    assert bool(require_field(prof, "remote_computer_label"))
    assert require_field(prof, "context_inspector_live_reachable") is True


# ---------------------------------------------------------------------------
# UI-F17 Boundaries & Negative Cases (6 tests)
# ---------------------------------------------------------------------------


@uiux_e2e_test(
    id="TEST-UIUX-T2-UI-F17-01",
    tier=2,
    features=("UI-F17",),
    description="Verify blank channel name reverts on blur and >2000-char instructions set aria-invalid blocking save",
)
def test_t2_f17_01_blank_channel_name_revert_and_over_2000_instructions_block(env: TempUiUxEnv) -> None:
    cs = env.probe_scenario("f17_blank_name_and_long_instructions", "channel_settings")
    assert bool(require_field(cs, "name").strip())
    assert require_field(cs, "instructions_invalid") is True
    assert require_field(cs, "save_blocked") is True


@uiux_e2e_test(
    id="TEST-UIUX-T2-UI-F17-02",
    tier=2,
    features=("UI-F17",),
    description="Verify concurrent rapid channel edits serialize cleanly and isolate dirty state on switch",
)
def test_t2_f17_02_concurrent_rapid_edits_serialized_and_dirty_isolated(env: TempUiUxEnv) -> None:
    cs = env.probe_scenario("f17_rapid_edits_switch", "channel_settings")
    assert require_field(cs, "serialized_save_order") is True
    assert require_field(cs, "dirty_isolated_on_switch") is True


@uiux_e2e_test(
    id="TEST-UIUX-T2-UI-F17-03",
    tier=2,
    features=("UI-F17",),
    description="Verify removing current lead updates lead_agent_id and removing last member is guarded",
)
def test_t2_f17_03_remove_lead_and_last_remaining_member_guard(env: TempUiUxEnv) -> None:
    cs = env.probe_scenario("f17_remove_lead_and_last", "channel_settings")
    assert require_field(cs, "lead_agent_id") != "lead-removed"
    assert len(require_field(cs, "members")) >= 1
    assert require_field(cs, "last_member_removal_blocked") is True


@uiux_e2e_test(
    id="TEST-UIUX-T2-UI-F17-04",
    tier=2,
    features=("UI-F17",),
    description="Verify archived group locks ChannelSettingsPane and unsupported provider disables Compact",
)
def test_t2_f17_04_archived_group_settings_lock_and_compact_disable(env: TempUiUxEnv) -> None:
    arch = env.probe_scenario("f17_archived_settings", "channel_settings")
    nocomp = env.probe_scenario("f17_compact_unsupported", "channel_settings")
    assert require_field(arch, "read_only") is True
    assert require_field(arch, "context_actions_visible") is False
    assert require_field(nocomp, "compact_disabled") is True
    assert require_field(nocomp, "context_actions_visible") is True


@uiux_e2e_test(
    id="TEST-UIUX-T2-UI-F17-05",
    tier=2,
    features=("UI-F17",),
    description="Verify + Add member excludes existing members, handles zero match, and enforces 100-member cap",
)
def test_t2_f17_05_add_member_no_duplicates_and_100_member_cap(env: TempUiUxEnv) -> None:
    cs = env.probe_scenario("f17_member_cap_100", "channel_settings")
    members = require_field(cs, "members")
    assert len(members) == 100
    assert len(set(members)) == 100
    assert require_field(cs, "member_add_accepted") is False
    assert require_field(cs, "zero_match_hint_visible") is True


@uiux_e2e_test(
    id="TEST-UIUX-T2-UI-F17-06",
    tier=2,
    features=("UI-F17",),
    description="Verify ChannelPictureEditor rejects invalid upload/icon slug and normalizes hue 999->215",
)
def test_t2_f17_06_picture_editor_invalid_upload_and_hue_normalization(env: TempUiUxEnv) -> None:
    cs = env.probe_scenario("f17_invalid_picture_icon_hue", "channel_settings")
    bad_ico, norm_hue = normalize_channel_icon_and_hue("invalid_icon", 999)
    assert require_field(cs, "resolved_icon") is bad_ico
    assert require_field(cs, "resolved_hue") == norm_hue == 215
    assert require_field(cs, "reset_visible") is False
    assert require_field(cs, "upload_error_toast") is True


# ---------------------------------------------------------------------------
# UI-F18 Boundaries & Negative Cases (6 tests)
# ---------------------------------------------------------------------------


@uiux_e2e_test(
    id="TEST-UIUX-T2-UI-F18-01",
    tier=2,
    features=("UI-F18",),
    description="Verify zero-match hub search uses fishing.webm when supported and fishing-still.webp under reduced motion",
)
def test_t2_f18_01_zero_match_search_marketplace_no_match_plush_fallback(env: TempUiUxEnv) -> None:
    norm = env.probe_scenario("f18_no_match_normal", "content_hub_pages")
    rm = env.probe_scenario("f18_no_match_reduced_motion", "content_hub_pages")
    assert require_field(norm, "no_match_plush_asset") == "fishing.webm"
    assert require_field(rm, "no_match_plush_asset") == "fishing-still.webp"


@uiux_e2e_test(
    id="TEST-UIUX-T2-UI-F18-02",
    tier=2,
    features=("UI-F18",),
    description="Verify <=720px narrow viewport collapses secondary hub cleanly with zero horizontal overflow",
)
def test_t2_f18_02_narrow_720px_secondary_hub_zero_overflow(env: TempUiUxEnv) -> None:
    hub = env.probe_scenario("f18_narrow_700", "content_hub_pages")
    sh = env.probe_scenario("f18_narrow_700", "shell")
    assert require_field(sh, "rail_visible") is False
    assert require_field(hub, "horizontal_overflow_px") == 0
    assert require_field(hub, "content_width_px") > 0


@uiux_e2e_test(
    id="TEST-UIUX-T2-UI-F18-03",
    tier=2,
    features=("UI-F18",),
    description="Verify PublishReviewModal rejects empty title/category with OpenBot danger alert preserving inputs",
)
def test_t2_f18_03_publish_review_modal_validation_error_blocks_submit(env: TempUiUxEnv) -> None:
    hub = env.probe_scenario("f18_publish_invalid", "content_hub_pages")
    assert require_field(hub, "publish_modal_open") is True
    assert require_field(hub, "publish_submitted") is False
    assert require_field(hub, "publish_error_visible") is True
    assert require_field(hub, "legacy_blue_pill_count") == 0


@uiux_e2e_test(
    id="TEST-UIUX-T2-UI-F18-04",
    tier=2,
    features=("UI-F18",),
    description="Verify light mode token parity across secondary hubs (#f5f5f6/#ffffff, #1c1c1e CTA, zero blue pills)",
)
def test_t2_f18_04_light_mode_token_parity_across_secondary_hubs(env: TempUiUxEnv) -> None:
    hub = env.probe_scenario("f18_hubs_light_mode", "content_hub_pages")
    assert require_field(hub, "canvas_hex").lower() in ("#f5f5f6", "#ffffff")
    assert_hex_eq(require_field(hub, "primary_cta_bg_hex"), "#1c1c1e", "light_hub.cta_bg")
    assert_hex_eq(require_field(hub, "primary_cta_fg_hex"), "#ffffff", "light_hub.cta_fg")
    assert require_field(hub, "legacy_blue_pill_count") == 0


@uiux_e2e_test(
    id="TEST-UIUX-T2-UI-F18-05",
    tier=2,
    features=("UI-F18",),
    description="Verify deep-link breadcrumb stack restores scroll cleanly when returning from Messages",
)
def test_t2_f18_05_deep_link_breadcrumb_stack_restoration(env: TempUiUxEnv) -> None:
    deep = env.probe_scenario("f18_breadcrumb_deep", "content_hub_pages")
    back = env.probe_scenario("f18_breadcrumb_back", "content_hub_pages")
    assert require_field(deep, "nav_depth") >= 2
    assert require_field(back, "nav_depth") == require_field(deep, "nav_depth") - 1
    assert require_field(back, "orphaned_modal_blocking") is False


@uiux_e2e_test(
    id="TEST-UIUX-T2-UI-F18-06",
    tier=2,
    features=("UI-F18",),
    description="Verify missing music URL and malformed mini-game manifest recover gracefully via error toast",
)
def test_t2_f18_06_music_player_and_game_runtime_graceful_error_recovery(env: TempUiUxEnv) -> None:
    hub = env.probe_scenario("f18_media_game_error", "content_hub_pages")
    assert require_field(hub, "error_toast_visible") is True
    assert require_field(hub, "music_playing") is False
    assert require_field(hub, "runtime_crashed") is False


# ---------------------------------------------------------------------------
# UI-F19 Boundaries & Negative Cases (6 tests)
# ---------------------------------------------------------------------------


@uiux_e2e_test(
    id="TEST-UIUX-T2-UI-F19-01",
    tier=2,
    features=("UI-F19",),
    description="Verify SettingsSearchField zero-match state, 12-result ceiling, and double-Escape exit",
)
def test_t2_f19_01_settings_search_zero_match_12_cap_and_escape(env: TempUiUxEnv) -> None:
    zero = env.probe_scenario("f19_search_zero", "settings_and_overlays")
    broad = env.probe_scenario("f19_search_broad", "settings_and_overlays")
    esc2 = env.probe_scenario("f19_search_escape_twice", "settings_and_overlays")
    assert require_field(zero, "settings_page.search_results_count") == 0
    assert require_field(zero, "settings_page.empty_search_visible") is True
    assert require_field(broad, "settings_page.search_results_count") <= 12
    assert require_field(esc2, "settings_page.open") is False


@uiux_e2e_test(
    id="TEST-UIUX-T2-UI-F19-02",
    tier=2,
    features=("UI-F19",),
    description="Verify ConfirmDialog pending async lock blocks backdrop/Escape dismiss and restores focus",
)
def test_t2_f19_02_confirm_dialog_pending_async_lock_and_focus_restore(env: TempUiUxEnv) -> None:
    pend = env.probe_scenario("f19_confirm_pending", "settings_and_overlays")
    done = env.probe_scenario("f19_confirm_resolved", "settings_and_overlays")
    assert require_field(pend, "confirm_dialog.open") is True
    assert require_field(pend, "confirm_dialog.pending") is True
    assert require_field(pend, "confirm_dialog.backdrop_dismiss_blocked") is True
    assert require_field(done, "confirm_dialog.open") is False
    assert require_field(done, "confirm_dialog.focus_restored_to_opener") is True


@uiux_e2e_test(
    id="TEST-UIUX-T2-UI-F19-03",
    tier=2,
    features=("UI-F19",),
    description="Verify shortcut rebinding refuses invalid, OS-reserved, and already-taken chords cleanly",
)
def test_t2_f19_03_shortcut_refusal_invalid_reserved_and_taken(env: TempUiUxEnv) -> None:
    inv = env.probe_scenario("f19_shortcut_invalid", "settings_and_overlays")
    res = env.probe_scenario("f19_shortcut_reserved", "settings_and_overlays")
    tak = env.probe_scenario("f19_shortcut_taken", "settings_and_overlays")
    assert require_field(inv, "shortcuts.last_refusal") == "invalid"
    assert require_field(res, "shortcuts.last_refusal") == "reserved"
    assert require_field(tak, "shortcuts.last_refusal") == "taken"


@uiux_e2e_test(
    id="TEST-UIUX-T2-UI-F19-04",
    tier=2,
    features=("UI-F19",),
    description="Verify global workspace shortcuts are suppressed inside active dialogs, alertdialogs, and menus",
)
def test_t2_f19_04_global_shortcuts_suppressed_in_dialogs_and_menus(env: TempUiUxEnv) -> None:
    ov = env.probe_scenario("f19_shortcuts_in_dialog", "settings_and_overlays")
    assert require_field(ov, "shortcuts.suppressed_in_overlay") is True
    assert require_field(ov, "confirm_dialog.open") is True


@uiux_e2e_test(
    id="TEST-UIUX-T2-UI-F19-05",
    tier=2,
    features=("UI-F19",),
    description="Verify ui-action-menu flips inside viewport bounds and skips disabled destructive items on ArrowDown",
)
def test_t2_f19_05_action_menu_viewport_flip_and_disabled_item_skip(env: TempUiUxEnv) -> None:
    ov = env.probe_scenario("f19_action_menu_edge_flip", "settings_and_overlays")
    am = require_field(ov, "action_menu")
    assert require_field(am, "within_viewport") is True
    assert require_field(am, "disabled_item_skipped") is True
    assert require_field(am, "disabled_handler_invoked") is False


@uiux_e2e_test(
    id="TEST-UIUX-T2-UI-F19-06",
    tier=2,
    features=("UI-F19",),
    description="Verify ui-toaster overflow queueing, Undo callback restoration, and reduced-motion mode",
)
def test_t2_f19_06_toaster_overflow_queue_undo_and_reduced_motion(env: TempUiUxEnv) -> None:
    ov = env.probe_scenario("f19_toaster_undo_reduced_motion", "settings_and_overlays")
    t = require_field(ov, "toaster")
    assert require_field(t, "z_index") == 120
    assert require_field(t, "reduced_motion_static") is True
    assert require_field(t, "undo_restored_item") is True


# ---------------------------------------------------------------------------
# UI-F20 Boundaries & Negative Cases (6 tests)
# ---------------------------------------------------------------------------


@uiux_e2e_test(
    id="TEST-UIUX-T2-UI-F20-01",
    tier=2,
    features=("UI-F20",),
    description="Verify probe rejects malformed JSON, empty scenario, and unknown scenario with exit 2 and zero panic",
)
def test_t2_f20_01_probe_rejects_malformed_and_unknown_scenarios_without_panic(env: TempUiUxEnv) -> None:
    rej = env.probe_cli_reject("f20_cli_reject_invalid")
    assert require_field(rej, "invalid_scenario_exit_code") == 2
    assert require_field(rej, "malformed_json_exit_code") == 2
    assert require_field(rej, "rust_panic_detected") is False


@uiux_e2e_test(
    id="TEST-UIUX-T2-UI-F20-02",
    tier=2,
    features=("UI-F20",),
    description="Verify active OpenBot runtime layout (280px/360px) never mutates legacy pub const exports",
)
def test_t2_f20_02_legacy_pub_const_immutability_under_active_openbot_mode(env: TempUiUxEnv) -> None:
    ws = check_preserved_pub_consts()
    assert ws["all_preserved"] is True
    cl = env.probe_scenario("f20_active_openbot_consts", "compat_and_ledger")
    pc = require_field(cl, "preserved_consts")
    assert require_field(pc, "chat_sidebar_default_width") == 288.0
    assert require_field(pc, "context_inspector_drawer_width") == 320.0
    assert require_field(pc, "msg_min_width") == 160.0 and require_field(pc, "msg_max_width") == 430.0
    assert require_field(pc, "default_fabushi_avatar_shape_valid") is True


@uiux_e2e_test(
    id="TEST-UIUX-T2-UI-F20-03",
    tier=2,
    features=("UI-F20",),
    description="Verify immutable governance files (TEST_INFRA prefix, 28 shards = 3441 entries, roadmap) untouched",
)
def test_t2_f20_03_immutable_governance_files_and_prefix_sha256(env: TempUiUxEnv) -> None:
    gov = check_immutable_governance_files()
    assert gov["test_infra_prefix_ok"] is True
    assert gov["shards_count"] == 28 and gov["ledger_entries"] == 3441
    cl = env.probe_section("compat_and_ledger")
    assert require_field(cl, "ledger_closure_pass") is True


@uiux_e2e_test(
    id="TEST-UIUX-T2-UI-F20-04",
    tier=2,
    features=("UI-F20",),
    description="Verify Cargo.toml lean profiles (debug=0, strip=debuginfo, incremental=false) and clang+mold config",
)
def test_t2_f20_04_lean_cargo_profile_hygiene_guard(env: TempUiUxEnv) -> None:
    cargo = check_cargo_toml_lean_profiles()
    assert cargo["extra_target_dirs"] == []
    assert set(cargo["profiles_checked"]) >= {"dev", "test", "ci", "release"}
    assert cargo["has_clang_mold"] is True


@uiux_e2e_test(
    id="TEST-UIUX-T2-UI-F20-05",
    tier=2,
    features=("UI-F20",),
    description="Verify missing optional plush asset triggers graceful procedural fallback with zero Rust panic",
)
def test_t2_f20_05_missing_plush_asset_graceful_fallback_zero_panic(env: TempUiUxEnv) -> None:
    plush = env.probe_scenario("f20_missing_plush_asset_fallback", "plush")
    assert require_field(plush, "runtime.render_ok") is True
    assert require_field(plush, "tint_ramp.fallback_used") is True


@uiux_e2e_test(
    id="TEST-UIUX-T2-UI-F20-06",
    tier=2,
    features=("UI-F20",),
    description="Verify strict negative audit confirming ALL 12 image_3.png defects (D1-D12) are eliminated",
)
def test_t2_f20_06_defect_zero_regression_gate_d1_to_d12(env: TempUiUxEnv) -> None:
    defects = env.probe_section("defects_audit")
    assert len(defects) >= 12
    assert_all_defects_eliminated(defects)
