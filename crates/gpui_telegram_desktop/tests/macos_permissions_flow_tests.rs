use gpui_telegram_desktop::platform::mac::macos_permissions_drag::{
    MacPermissionsSnapshot, PermissionKind, ScreenPermissionState, cached_permissions_snapshot,
    compute_screen_state_from_probe, has_accessibility_permission, has_screen_recording_permission,
    hide_drag_hud, is_stale_tcc_entry_suspected, open_permission_with_drag_hud,
    reset_stale_permission_and_reopen, should_auto_reset_tcc_service,
    should_defer_local_capture_for_permissions, should_probe_sck_live,
};
use gpui_telegram_desktop::theme::palette::{TdesktopThemeMode, TdesktopThemePalette};
use gpui_telegram_desktop::views::computer_workbench::ComputerWorkbench;
use gpui_telegram_desktop::views::settings::sections::SettingsComputersSections;
use gpui_telegram_proto::{ComputerNodeDto, ComputerNodeKind, ComputerRunPolicy};

#[test]
fn test_permissions_snapshot_state_helpers_and_offline_safety() {
    fabushi_remote_engine::set_headless_test_mode(true);
    assert!(has_screen_recording_permission());
    assert!(has_accessibility_permission());

    let snap = cached_permissions_snapshot();
    assert_eq!(snap.screen_state, ScreenPermissionState::GrantedActive);
    assert!(snap.accessibility_granted);
    assert!(snap.all_ready());
    assert!(!snap.any_missing());

    // Offline/test calls must never spawn OS settings or native HUD panels.
    open_permission_with_drag_hud(PermissionKind::ScreenRecording, "zh");
    open_permission_with_drag_hud(PermissionKind::Accessibility, "en");
    reset_stale_permission_and_reopen(PermissionKind::ScreenRecording, "zh");
    hide_drag_hud();

    #[cfg(target_os = "macos")]
    {
        let live_id = fabushi_remote_engine::macos_permissions::current_signing_identity();
        assert!(
            !live_id.executable_path.is_empty(),
            "native SecCode/dyld introspection must resolve current executable path"
        );
        assert!(
            !live_id.cdhash_hex.is_empty(),
            "native SecCode introspection must extract non-empty CDHash on macOS"
        );
    }

    let needs_restart_snap = MacPermissionsSnapshot {
        screen_state: ScreenPermissionState::GrantedNeedsRestart,
        accessibility_granted: true,
        preflight_screen: true,
        fresh_probe_screen: Some(true),
        signing: Default::default(),
        stale_screen_suspected: false,
        stale_ax_suspected: false,
        screen_settings_opened: true,
        ax_settings_opened: false,
        hud_visible: false,
        hud_target: None,
        last_drag_seq: 1,
        last_probe_detail: Some("SCStreamErrorDomain -3801".to_string()),
    };
    assert!(needs_restart_snap.screen_state.needs_restart());
    assert!(!needs_restart_snap.screen_state.is_active());
    assert!(!needs_restart_snap.all_ready());
    assert!(needs_restart_snap.any_missing());
}

#[test]
fn test_signing_identity_drag_target_resolution() {
    use fabushi_remote_engine::macos_permissions::MacSigningIdentity;

    let app_id = MacSigningIdentity {
        is_adhoc: true,
        is_app_bundle: true,
        cdhash_hex: "deadbeef".to_string(),
        bundle_id: "com.ombhrum.fabushi".to_string(),
        bundle_path: "/Applications/Fabushi.app".to_string(),
        executable_path: "/Applications/Fabushi.app/Contents/MacOS/gpui_telegram_desktop"
            .to_string(),
        team_id: String::new(),
    };
    assert_eq!(app_id.drag_target_path(), "/Applications/Fabushi.app");
    assert_eq!(app_id.display_app_name(), "Fabushi");

    let bare_bin = MacSigningIdentity {
        is_adhoc: true,
        is_app_bundle: false,
        cdhash_hex: "cafebabe".to_string(),
        bundle_id: String::new(),
        bundle_path: "/tmp/gpui_telegram_desktop".to_string(),
        executable_path: "/tmp/gpui_telegram_desktop".to_string(),
        team_id: String::new(),
    };
    assert_eq!(bare_bin.drag_target_path(), "/tmp/gpui_telegram_desktop");
    assert_eq!(bare_bin.display_app_name(), "Fabushi");
}

#[test]
fn test_workbench_distinguishes_local_vs_remote_host_permissions() {
    let pal = TdesktopThemePalette::build(TdesktopThemeMode::Night, 0x5288C1, 14, 12);
    let mut wb = ComputerWorkbench::new_live("My Mac".to_string(), pal, "zh");

    let local_node = ComputerNodeDto {
        node_id: "local:1:dev-a".to_string(),
        kind: ComputerNodeKind::LocalDevice,
        display_name: "My MacBook".to_string(),
        device_id: Some("dev-a".to_string()),
        os_name: "macOS arm64".to_string(),
        ip_address: "192.168.1.7".to_string(),
        resolution: "1440x900".to_string(),
        online: true,
        is_current_device: true,
        run_policy: ComputerRunPolicy::AlwaysAllowAuthorized,
        ..ComputerNodeDto::default()
    };
    wb.set_target_node(&local_node);
    assert!(wb.is_local_device);

    let mut remote_node = local_node.clone();
    remote_node.node_id = "local:1:dev-b".to_string();
    remote_node.display_name = "Studio Mac".to_string();
    remote_node.is_current_device = false;
    wb.set_target_node(&remote_node);
    assert!(!wb.is_local_device);

    let mut sc = SettingsComputersSections::new_live(pal, "zh");
    assert!(!sc.permission_modal_open);
    sc.permission_modal_open = true;
    assert!(sc.permission_modal_open);
}

#[test]
fn tier5_workbench_set_target_node_clears_permission_flags_and_error() {
    let pal = TdesktopThemePalette::build(TdesktopThemeMode::Night, 0x5288C1, 14, 12);
    let mut wb = ComputerWorkbench::new_live("Node A".to_string(), pal, "en");

    let node_a = ComputerNodeDto {
        node_id: "node-a".to_string(),
        kind: ComputerNodeKind::LocalDevice,
        display_name: "Node A".to_string(),
        os_name: "macOS arm64".to_string(),
        ip_address: "192.168.1.10".to_string(),
        online: true,
        is_current_device: true,
        run_policy: ComputerRunPolicy::AlwaysAllowAuthorized,
        ..ComputerNodeDto::default()
    };
    wb.set_target_node(&node_a);
    wb.needs_screen_permission_guide = true;
    wb.needs_accessibility_permission_guide = true;
    wb.force_local_permission_modal = true;
    wb.remote_permission_error = Some("SCStreamErrorDomain -3801".to_string());

    // Same-node refresh preserves flags
    wb.set_target_node(&node_a);
    assert!(wb.needs_screen_permission_guide);
    assert!(wb.remote_permission_error.is_some());

    // Switching to another node clears all permission flags and error
    let mut node_b = node_a.clone();
    node_b.node_id = "node-b".to_string();
    node_b.display_name = "Node B".to_string();
    node_b.is_current_device = false;
    wb.set_target_node(&node_b);

    assert!(!wb.needs_screen_permission_guide);
    assert!(!wb.needs_accessibility_permission_guide);
    assert!(!wb.force_local_permission_modal);
    assert_eq!(wb.remote_permission_error, None);
}

#[test]
fn tier5_compute_screen_state_revocation_not_masked_by_cached_preflight() {
    // Launched with TCC granted (`preflight_now = true`), later revoked at runtime
    // (`fresh_probe_screen = Some(false)` + SCK denied): MUST return NotGranted, never GrantedNeedsRestart.
    let state = compute_screen_state_from_probe(
        true,
        Some(false),
        false,
        true,
        false,
        true,
        false,
        true,
    );
    assert_eq!(state, ScreenPermissionState::NotGranted);

    // Same even before SCK probe completes (`sck_probed = false`)
    let state_unprobed = compute_screen_state_from_probe(
        true,
        Some(false),
        false,
        false,
        false,
        false,
        true,
        true,
    );
    assert_eq!(state_unprobed, ScreenPermissionState::NotGranted);

    // Granted in fresh probe after launching without permission -> GrantedNeedsRestart
    let state_needs_restart = compute_screen_state_from_probe(
        false,
        Some(true),
        false,
        false,
        false,
        true,
        false,
        false,
    );
    assert_eq!(state_needs_restart, ScreenPermissionState::GrantedNeedsRestart);
}

#[test]
fn test_auto_reset_allows_dev_id_stale_adhoc_migration_and_never_wipes_working_grant() {
    // Developer ID build denied at launch & denied by fresh probe (stale ad-hoc csreq in TCC.db):
    // MUST trigger one-time reset regardless of is_adhoc!
    assert!(should_auto_reset_tcc_service(
        "com.ombhrum.fabushi",
        false,
        false,
        false,
        Some(false),
    ));
    // Also eligible before first fresh probe has returned when denied at launch & preflight.
    assert!(should_auto_reset_tcc_service(
        "com.ombhrum.fabushi",
        false,
        false,
        false,
        None,
    ));
    // Never reset a second time in the same process.
    assert!(!should_auto_reset_tcc_service(
        "com.ombhrum.fabushi",
        false,
        true,
        false,
        Some(false),
    ));
    // NEVER wipe a grant that was active at launch, active via preflight, or newly granted in TCC!
    assert!(!should_auto_reset_tcc_service(
        "com.ombhrum.fabushi",
        true,
        false,
        false,
        Some(false),
    ));
    assert!(!should_auto_reset_tcc_service(
        "com.ombhrum.fabushi",
        false,
        false,
        true,
        Some(false),
    ));
    assert!(!should_auto_reset_tcc_service(
        "com.ombhrum.fabushi",
        false,
        false,
        false,
        Some(true),
    ));
    // Never invoke tccutil with an empty or non-bundle identifier.
    assert!(!should_auto_reset_tcc_service("", false, false, false, Some(false)));
    assert!(!should_auto_reset_tcc_service("gpui_telegram_desktop", false, false, false, Some(false)));
}

#[test]
fn test_stale_tcc_suspected_covers_dev_id_and_adhoc_transitions() {
    // Upgrading from ad-hoc v1.6.7 (old cdhash in tcc_signature_state.json) to Developer ID v1.6.8:
    // MUST suspect stale TCC even though current_is_adhoc == false!
    assert!(is_stale_tcc_entry_suspected(
        true,
        "ec7b5905bdd765cf99bd2d72ea9471abe94795ee",
        "M4Q99K4UR4",
        false,
        Some("b26606351cec2e10dda2f797eb127248aba7487e"),
        None,
        None,
        false,
        false,
        None,
    ));

    // Switching signing mode (adhoc -> DevID or DevID -> adhoc) when denied -> suspected.
    assert!(is_stale_tcc_entry_suspected(
        true,
        "aaaa",
        "M4Q99K4UR4",
        false,
        Some("aaaa"),
        Some("M4Q99K4UR4"),
        Some(true),
        false,
        false,
        None,
    ));

    // Team ID change when denied -> suspected.
    assert!(is_stale_tcc_entry_suspected(
        true,
        "aaaa",
        "NEWTEAM123",
        false,
        Some("aaaa"),
        Some("OLDTEAM999"),
        Some(false),
        false,
        false,
        None,
    ));

    // Post-drag probe still denied -> suspected even on Developer ID build.
    assert!(is_stale_tcc_entry_suspected(
        true,
        "",
        "M4Q99K4UR4",
        false,
        None,
        None,
        None,
        true,
        false,
        Some(false),
    ));

    // When NOT denied (GrantedActive or GrantedNeedsRestart) -> never suspected.
    assert!(!is_stale_tcc_entry_suspected(
        false,
        "new_hash",
        "M4Q99K4UR4",
        false,
        Some("old_hash"),
        Some("OLDTEAM"),
        Some(true),
        true,
        true,
        Some(false),
    ));
}

#[test]
fn test_sck_probe_and_initial_capture_guards_prevent_os_popups() {
    // Launched without permission, newly enabled in System Settings (fresh=Some(true)):
    // MUST NOT call SCShareableContent before restart (zero OS popup!).
    assert!(!should_probe_sck_live(false, false, Some(true), false));
    // Denied -> MUST NOT call SCShareableContent.
    assert!(!should_probe_sck_live(false, false, Some(false), false));
    // Revoked at runtime despite cached launch preflight -> MUST NOT call SCShareableContent.
    assert!(!should_probe_sck_live(true, true, Some(false), true));
    // Granted at launch and preflight true and not revoked -> safe to probe SCK live!
    assert!(should_probe_sck_live(true, true, Some(true), false));
    assert!(should_probe_sck_live(true, true, None, false));

    // Opening local macOS workbench without active Screen Recording defers CaptureScreen.
    assert!(should_defer_local_capture_for_permissions(
        true,
        true,
        ScreenPermissionState::NotGranted
    ));
    assert!(should_defer_local_capture_for_permissions(
        true,
        true,
        ScreenPermissionState::GrantedNeedsRestart
    ));
    // Active local Screen Recording, remote devices, and non-macOS hosts capture immediately.
    assert!(!should_defer_local_capture_for_permissions(
        true,
        true,
        ScreenPermissionState::GrantedActive
    ));
    assert!(!should_defer_local_capture_for_permissions(
        false,
        true,
        ScreenPermissionState::NotGranted
    ));
    assert!(!should_defer_local_capture_for_permissions(
        true,
        false,
        ScreenPermissionState::NotGranted
    ));
}

#[cfg(target_os = "macos")]
#[test]
fn test_basic_bundle_identity_resolves_without_security_framework() {
    let basic = fabushi_remote_engine::macos_permissions::current_bundle_identity();
    assert!(
        !basic.executable_path.is_empty(),
        "basic bundle identity must resolve executable path without SecCode"
    );
}
