use gpui_telegram_desktop::platform::mac::macos_permissions_drag::{
    MacPermissionsSnapshot, PermissionKind, ScreenPermissionState, cached_permissions_snapshot,
    has_accessibility_permission, has_screen_recording_permission, hide_drag_hud,
    open_permission_with_drag_hud, reset_stale_permission_and_reopen,
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
