use std::sync::Arc;

use base64::Engine as _;
use fabushi_remote_engine::control_lease::RemoteControlHub;
use fabushi_remote_engine::ipc::{LocalIpcClient, LocalIpcServer};
use gpui_telegram_client::{ClientConfig, TelegramClient};
use gpui_telegram_desktop::app::cli_installer::{
    inspect_cli_status_with, install_cli_to_target, locate_bundled_fabushi_cli_from_exe,
};
use gpui_telegram_desktop::platform::mac::tray_mac::{TrayMacMac, tray_action_ids};
use gpui_telegram_desktop::platform::win::tray_win::TrayWinWin;
use gpui_telegram_desktop::theme::{TdesktopThemeMode, TdesktopThemePalette};
use gpui_telegram_desktop::views::app_shell::desktop_ipc_host::DesktopLocalIpcHandler;
use gpui_telegram_desktop::views::computer_workbench::{
    ComputerWorkbench, MouseControlMode, WorkbenchCommand, WorkbenchHotkeyPreset,
};
use gpui_telegram_desktop::views::remote_controlled_window::{
    ControlledBannerVariant, RemoteControlledWindow, RemoteControlledWindowAction,
    format_elapsed_hms, group_controlled_sessions, resolve_banner_variant,
};
use gpui_telegram_desktop::views::remote_terminal_view::{
    RemoteTerminalAction, RemoteTerminalView, TerminalScreen, encode_terminal_key,
};
use gpui_telegram_desktop::views::settings::sections::{
    AssistInputField, McpClientPreset, SettingsComputersEvent, SettingsComputersSections,
    SettingsComputersTab, build_mcp_client_config_json, format_assist_id_display,
    is_valid_custom_assist_code,
};
use gpui_telegram_proto::remote_control_contracts::{
    AssistCodeMode, ColorSubsampling, DisplayInfoDto, FileConflictPolicy, FileTransferDirection,
    FileTransferStatus, FileTransferTaskDto, McpOAuthGrantSummaryDto, PowerActionKind,
    RemoteAssistConfigDto, RemoteControlMode, RemoteControllerKind, RemoteSessionDto, StreamQuality,
};
use gpui_telegram_proto::tailnet_contracts::TailnetRelayMode;

fn sample_session(
    id: &str,
    kind: RemoteControllerKind,
    name: &str,
    started_at_ms: i64,
    tool: Option<&str>,
) -> RemoteSessionDto {
    RemoteSessionDto {
        session_id: id.to_string(),
        controller_kind: kind,
        controller_name: name.to_string(),
        controller_platform: if kind == RemoteControllerKind::AiMcp {
            "MCP".to_string()
        } else {
            "macOS".to_string()
        },
        controller_id: format!("ctrl-{id}"),
        target_node_id: "local:1:host".to_string(),
        target_device_id: "dev-host".to_string(),
        mode: match kind {
            RemoteControllerKind::AiMcp => RemoteControlMode::AiControl,
            _ => RemoteControlMode::Desktop,
        },
        started_at_ms,
        last_activity_ms: started_at_ms + 1_000,
        active_tool: tool.map(str::to_string),
        relay_mode: TailnetRelayMode::DirectUdp,
        rtt_ms: Some(11),
        fps: Some(60),
    }
}

#[test]
fn test_remote_controlled_window_grouping_timer_banner_and_kill_switch() {
    let hub = Arc::new(RemoteControlHub::new());
    let s1 = hub.register_session(sample_session(
        "s-own",
        RemoteControllerKind::OwnDevice,
        "MacBook Pro",
        1_000_000,
        None,
    ));
    let s2 = hub.register_session(sample_session(
        "s-assist",
        RemoteControllerKind::AssistGuest,
        "Support Helper",
        1_002_000,
        None,
    ));
    let s3 = hub.register_session(sample_session(
        "s-ai",
        RemoteControllerKind::AiMcp,
        "Antigravity Agent",
        1_004_000,
        Some("screen.capture"),
    ));

    assert_eq!(resolve_banner_variant(&[]), ControlledBannerVariant::Idle);
    assert_eq!(
        resolve_banner_variant(&[s1.clone()]),
        ControlledBannerVariant::HumanBlue
    );
    assert_eq!(
        resolve_banner_variant(&[s3.clone()]),
        ControlledBannerVariant::AiTeal
    );
    assert_eq!(
        resolve_banner_variant(&[s1.clone(), s3.clone()]),
        ControlledBannerVariant::MixedIndigo
    );

    let groups = group_controlled_sessions(&[s3, s2, s1]);
    assert_eq!(groups.len(), 3);
    assert_eq!(groups[0].title_zh, "本账号设备");
    assert_eq!(groups[1].title_zh, "远程协助设备");
    assert_eq!(groups[2].title_zh, "AI Agent (MCP)");

    let mut win = RemoteControlledWindow::new_offline(hub.clone(), "zh-CN");
    win.now_ms_override = Some(1_006_000);
    assert_eq!(win.earliest_elapsed_hms(), "00:00:06");
    assert_eq!(format_elapsed_hms(3_661_000), "01:01:01");

    win.toggle_collapse();
    assert!(win.collapsed_to_pill);

    let token = hub.preemption_token();
    assert!(!token.is_cancelled());

    win.disconnect_session("s-assist");
    assert_eq!(hub.active_count(), 2);
    assert!(token.is_cancelled());

    let disconnected = win.disconnect_all();
    assert_eq!(disconnected, 2);
    assert_eq!(hub.active_count(), 0);
    assert!(win.sessions.is_empty());

    let actions = win.take_actions();
    assert!(actions.iter().any(|a| matches!(
        a,
        RemoteControlledWindowAction::DisconnectAll {
            disconnected_local_count: 2,
            ..
        }
    )));
}

#[test]
fn test_remote_terminal_view_ansi_sgr_scrollback_osc52_and_tabs() {
    let mut screen = TerminalScreen::new(40, 6);
    screen.feed_str("\x1b[38;5;196mRed256\x1b[38;2;10;200;150m TrueClr\x1b[0m");
    let row0 = screen.visible_row(0).unwrap();
    assert_eq!(row0[0].ch, 'R');
    assert_eq!(row0[7].ch, 'T');
    assert_eq!(row0[7].fg, 0x0AC896);

    // Push scrollback + test OSC 52 + OSC title
    for i in 0..20 {
        screen.feed_str(&format!("\r\nline-{i:02}"));
    }
    assert!(screen.scrollback_len() >= 14);
    screen.scroll_lines(5);
    assert!(screen.scroll_offset > 0);
    screen.scroll_lines(-5);
    assert_eq!(screen.scroll_offset, 0);

    // OSC 52: base64("hello-osc52") = "aGVsbG8tb3NjNTI="
    screen.feed_str("\x1b]0;fabushi-zsh\x07\x1b]52;c;aGVsbG8tb3NjNTI=\x07");
    assert_eq!(screen.title.as_deref(), Some("fabushi-zsh"));
    assert_eq!(
        screen.pending_osc52_clipboard,
        vec!["hello-osc52".to_string()]
    );

    // Special key encoding
    assert_eq!(
        encode_terminal_key("c", false, true, false, None),
        Some(vec![0x03])
    );
    assert_eq!(
        encode_terminal_key("up", false, false, false, None),
        Some(b"\x1b[A".to_vec())
    );
    assert_eq!(
        encode_terminal_key("enter", false, false, false, None),
        Some(b"\r".to_vec())
    );

    let pal = TdesktopThemePalette::build(TdesktopThemeMode::Night, 0x1E6BFF, 14, 12);
    let mut term = RemoteTerminalView::new(pal, "zh-CN");
    let tab1 = term.tabs[0].session_id.clone();
    let tab2 = term.open_new_tab(Some("build".to_string()));
    assert_eq!(term.tabs.len(), 2);

    term.begin_rename_tab(1);
    term.rename_draft = "deploy-shell".to_string();
    assert!(term.commit_rename());
    assert_eq!(term.tabs[1].name, "deploy-shell");

    // Default keep_alive_on_exit=true keeps exited tab open for inspection
    assert!(term.tabs[1].keep_alive_on_exit);
    term.mark_session_exited(&tab2, 0);
    assert_eq!(term.tabs.len(), 2);
    assert!(!term.tabs[1].alive);

    // Toggling keep_alive_on_exit=false on a third tab auto-closes it when process exits
    let tab3 = term.open_new_tab(Some("auto-close".to_string()));
    assert_eq!(term.tabs.len(), 3);
    term.toggle_exit_keep_alive(2);
    assert!(!term.tabs[2].keep_alive_on_exit);
    term.mark_session_exited(&tab3, 0);
    assert_eq!(term.tabs.len(), 2);

    // Kill tab2 explicitly
    term.close_or_kill_tab(1, true);
    assert_eq!(term.tabs.len(), 1);
    assert_eq!(term.tabs[0].session_id, tab1);

    let acts = term.take_actions();
    assert!(acts
        .iter()
        .any(|a| matches!(a, RemoteTerminalAction::NewSession { .. })));
    assert!(acts
        .iter()
        .any(|a| matches!(a, RemoteTerminalAction::KillSession { .. })));
}

#[test]
fn test_tray_mac_and_win_remote_menu_and_assist_formatting() {
    let mut mac_tray = TrayMacMac::new();
    mac_tray.create_icon().unwrap();
    mac_tray.set_remote_control_status(
        true,
        2,
        true,
        false,
        "482910375".to_string(),
        Some("Xy99Ab12".to_string()),
        "zh-CN",
    );
    let clip_text = mac_tray.formatted_assist_clipboard_text();
    assert!(
        clip_text.contains("482 910 375") && clip_text.contains("Xy99Ab12"),
        "unexpected assist clipboard text: {clip_text}"
    );
    assert!(mac_tray
        .actions
        .iter()
        .any(|a| a.id == tray_action_ids::DISCONNECT_ALL && a.is_enabled));
    assert!(mac_tray
        .actions
        .iter()
        .any(|a| a.id == tray_action_ids::TOGGLE_ALLOW_REMOTE && !a.title.contains('✓')));

    let mut win_tray = TrayWinWin::new();
    win_tray.tray().unwrap();
    win_tray.set_remote_control_status(
        true,
        0,
        false,
        true,
        "123456789".to_string(),
        None,
        "zh-CN",
    );
    assert!(win_tray
        .actions
        .iter()
        .any(|a| a.id == tray_action_ids::DISCONNECT_ALL && !a.is_enabled));
    assert!(win_tray
        .actions
        .iter()
        .any(|a| a.id == tray_action_ids::COPY_ASSIST));
}

#[test]
fn test_cli_installer_symlink_and_status_in_tempdir() {
    let dir = tempfile::TempDir::new().unwrap();
    let app_macos = dir.path().join("Fabushi.app/Contents/MacOS");
    let app_helpers = dir.path().join("Fabushi.app/Contents/Helpers");
    std::fs::create_dir_all(&app_macos).unwrap();
    std::fs::create_dir_all(&app_helpers).unwrap();

    let exe_path = app_macos.join("gpui_telegram_desktop");
    std::fs::write(&exe_path, b"stub").unwrap();
    let bin_name = if cfg!(windows) {
        "fabushi.exe"
    } else {
        "fabushi"
    };
    let bundled_cli = app_helpers.join(bin_name);
    std::fs::write(&bundled_cli, b"#!/bin/sh\necho ok\n").unwrap();

    let located = locate_bundled_fabushi_cli_from_exe(&exe_path).unwrap();
    assert_eq!(located, bundled_cli.canonicalize().unwrap());

    let target_bin_dir = dir.path().join("user_bin");
    let target_link = target_bin_dir.join(bin_name);
    let installed_path = install_cli_to_target(&located, &target_link).unwrap();
    assert_eq!(installed_path, target_link);

    let status = inspect_cli_status_with(
        Some(located),
        &[target_link],
        &target_bin_dir.display().to_string(),
    );
    assert!(status.installed);
    assert!(status.on_path);
}

#[test]
fn test_computer_workbench_hud_controls_mouse_modes_power_and_file_transfers() {
    let pal = TdesktopThemePalette::build(TdesktopThemeMode::Night, 0x1E6BFF, 14, 12);
    let mut wb = ComputerWorkbench::new_live("Studio-Mac".to_string(), pal, "zh-CN");
    wb.device.os_name = "macOS 15.3".to_string();

    wb.set_stream_fps(144);
    wb.set_stream_quality(StreamQuality::Ultra4K);
    wb.toggle_truecolor_444();
    wb.toggle_hdr();
    assert_eq!(wb.stream_settings.target_fps, 144);
    assert_eq!(wb.stream_settings.quality, StreamQuality::Ultra4K);
    assert_eq!(
        wb.stream_settings.color_mode,
        ColorSubsampling::TrueColor444
    );
    assert!(wb.stream_settings.hdr);

    wb.set_mouse_mode(MouseControlMode::CursorLock);
    assert!(wb.stream_settings.relative_mouse);
    assert!(wb.stream_settings.lock_cursor_in_window);

    wb.set_displays(vec![
        DisplayInfoDto {
            display_id: 0,
            name: "Main".into(),
            width: 2560,
            height: 1440,
            scale_factor_x100: 200,
            is_primary: true,
            is_virtual: false,
            refresh_hz: 120,
        },
        DisplayInfoDto {
            display_id: 1,
            name: "Virtual 1080p".into(),
            width: 1920,
            height: 1080,
            scale_factor_x100: 100,
            is_primary: false,
            is_virtual: true,
            refresh_hz: 60,
        },
    ]);
    wb.switch_display(1);
    wb.toggle_virtual_display();
    wb.toggle_privacy_screen();
    wb.trigger_hotkey(WorkbenchHotkeyPreset::Copy);
    wb.trigger_power_action(PowerActionKind::Lock);
    wb.trigger_wake_on_lan();
    wb.trigger_screenshot();

    wb.upsert_file_transfer(FileTransferTaskDto {
        transfer_id: "xf-1".into(),
        direction: FileTransferDirection::Upload,
        local_path: "/tmp/a.bin".into(),
        remote_path: "/home/a.bin".into(),
        total_bytes: 1000,
        transferred_bytes: 400,
        chunk_size: 200,
        sha256: None,
        status: FileTransferStatus::Running,
        conflict_policy: FileConflictPolicy::Overwrite,
        error: None,
        created_at_ms: 1,
        updated_at_ms: 2,
    });
    wb.set_file_conflict_policy(FileConflictPolicy::Rename);
    wb.pause_file_transfer("xf-1");
    assert_eq!(wb.file_transfers[0].status, FileTransferStatus::Paused);
    wb.resume_file_transfer("xf-1");
    assert_eq!(wb.file_transfers[0].status, FileTransferStatus::Running);

    let cmds = wb.take_commands();
    assert!(cmds
        .iter()
        .any(|c| matches!(c, WorkbenchCommand::SwitchDisplay { display_index: 1 })));
    assert!(cmds.iter().any(|c| matches!(
        c,
        WorkbenchCommand::ToggleVirtualDisplay { enabled: true, .. }
    )));
    assert!(cmds.iter().any(|c| matches!(
        c,
        WorkbenchCommand::TogglePrivacyScreen { enabled: true, .. }
    )));
    assert!(cmds.iter().any(|c| matches!(
        c,
        WorkbenchCommand::SendHotkey { key, modifiers } if key == "c" && modifiers == &vec!["Meta".to_string()]
    )));
    assert!(cmds.iter().any(|c| matches!(
        c,
        WorkbenchCommand::PowerAction(PowerActionKind::WakeOnLan { .. })
    )));
    assert!(cmds
        .iter()
        .any(|c| matches!(c, WorkbenchCommand::TakeScreenshot)));
}

#[test]
fn test_settings_computers_four_tabs_assist_mcp_and_cli() {
    assert_eq!(format_assist_id_display("482910375"), "482 910 375");
    assert!(is_valid_custom_assist_code("Abc12345"));
    assert!(!is_valid_custom_assist_code("short"));
    assert!(!is_valid_custom_assist_code("invalid-char!"));

    let pal = TdesktopThemePalette::build(TdesktopThemeMode::Night, 0x1E6BFF, 14, 12);
    let mut sc = SettingsComputersSections::new_live(pal, "zh-CN");
    assert_eq!(sc.mcp_endpoint_url, "https://mcp.ombhrum.com/mcp");
    sc.sync_assist_config(Some(RemoteAssistConfigDto {
        assist_id: "482910375".into(),
        allow_assist: true,
        code_mode: AssistCodeMode::Temporary,
        current_code: Some("99887766".into()),
        expires_at_ms: None,
    }));
    sc.sync_mcp_grants(
        None,
        vec![McpOAuthGrantSummaryDto {
            family_id: "fam-1".into(),
            client_id: "antigravity-mcp".into(),
            client_name: "Antigravity".into(),
            scopes: vec!["devices:read".into(), "screen:capture".into()],
            allowed_node_ids: vec![],
            created_at_ms: 100,
            last_used_ms: Some(200),
            expires_at_ms: 999999,
        }],
    );
    assert_eq!(sc.mcp_endpoint_url, "https://mcp.ombhrum.com/mcp");

    sc.set_overview_tab(SettingsComputersTab::RemoteAssist);
    sc.assist_focus = AssistInputField::CustomCode;
    assert!(sc.is_input_active());
    sc.assist_custom_code_draft = "MyPass2026".to_string();
    assert!(sc.submit_assist_custom_code());
    assert!(matches!(
        sc.take_event(),
        Some(SettingsComputersEvent::SetAssistCustomCode(ref c)) if c == "MyPass2026"
    ));

    sc.assist_target_id_draft = "123 456 789".to_string();
    sc.assist_target_code_draft = "SecRet88".to_string();
    assert!(sc.submit_assist_connect());
    assert!(matches!(
        sc.take_event(),
        Some(SettingsComputersEvent::StartAssistConnect(ref req))
            if req.assist_id == "123456789" && req.access_code == "SecRet88"
    ));

    sc.set_overview_tab(SettingsComputersTab::AiMcpHub);
    sc.trigger_copy_mcp_preset(McpClientPreset::Antigravity);
    match sc.take_event() {
        Some(SettingsComputersEvent::CopyMcpClientConfig {
            client_name,
            config_json,
        }) => {
            assert_eq!(client_name, "Antigravity");
            assert!(config_json.contains("serverUrl"));
            assert!(config_json.contains("https://mcp.ombhrum.com/mcp"));
        }
        other => panic!("unexpected event: {other:?}"),
    }

    let vs_json = build_mcp_client_config_json(McpClientPreset::VsCode, "");
    assert!(vs_json.contains("\"servers\""));
    assert!(vs_json.contains("https://mcp.ombhrum.com/mcp"));
}

#[test]
fn test_desktop_local_ipc_handler_roundtrip() {
    let temp = tempfile::TempDir::new().unwrap();
    let sock_path = temp.path().join("desktop-ipc-test.sock");
    let cfg = ClientConfig::default();
    let client = Arc::new(TelegramClient::new(cfg));
    let handler = Arc::new(DesktopLocalIpcHandler::new(client.clone()));
    handler.set_local_node_id(Some("local:1:test".to_string()));

    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();

    rt.block_on(async move {
        let server = LocalIpcServer::bind(sock_path.clone(), handler)
            .await
            .expect("bind LocalIpcServer in TempDir");
        let ipc = LocalIpcClient::new(sock_path);

        let echo = ipc
            .call("echo", serde_json::json!({ "ping": 42 }))
            .await
            .unwrap();
        assert!(echo.success);
        assert_eq!(echo.data.unwrap()["ping"], 42);

        let status = ipc.call("status", serde_json::json!({})).await.unwrap();
        assert!(status.success);
        assert_eq!(status.data.unwrap()["running"], true);

        let new_term = ipc
            .call(
                "lterm.new",
                serde_json::json!({ "name": "ipc-smoke", "cols": 80, "rows": 24 }),
            )
            .await
            .unwrap();
        assert!(new_term.success, "lterm.new failed: {:?}", new_term.error);

        let ls_term = ipc.call("lterm.ls", serde_json::json!({})).await.unwrap();
        assert!(ls_term.success);
        let sessions = ls_term.data.unwrap()["sessions"].as_array().unwrap().clone();
        assert!(sessions.iter().any(|s| s["name"] == "ipc-smoke"));

        let kill_term = ipc
            .call("lterm.kill", serde_json::json!({ "name": "ipc-smoke" }))
            .await
            .unwrap();
        assert!(kill_term.success);

        let clip_set = ipc
            .call(
                "clipboard.set",
                serde_json::json!({ "text": "fabushi-ipc-clip" }),
            )
            .await
            .unwrap();
        assert!(clip_set.success);

        let mcp_tools = ipc.call("mcp.tools", serde_json::json!({})).await.unwrap();
        assert!(mcp_tools.success);
        assert!(mcp_tools.data.unwrap()["tools"]
            .as_array()
            .unwrap()
            .iter()
            .any(|v| v == "screen.capture"));

        server.shutdown();
    });
}

#[test]
fn test_desktop_local_ipc_cli_contract_and_fallback() {
    let temp = tempfile::TempDir::new().unwrap();
    let sock_path = temp.path().join("desktop-ipc-contract.sock");
    let pushed_file = temp.path().join("sub").join("pushed.bin");
    let cfg = ClientConfig::default();
    let client = Arc::new(TelegramClient::new(cfg));
    let handler = Arc::new(DesktopLocalIpcHandler::new(client));
    handler.set_local_node_id(Some("local:1:contract".to_string()));

    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();

    rt.block_on(async move {
        let server = LocalIpcServer::bind(sock_path.clone(), handler)
            .await
            .expect("bind LocalIpcServer in TempDir");
        let ipc = LocalIpcClient::new(sock_path);

        // 1. term.open -> term.new and term.exit -> term.kill (supporting both `name` & `session_name`)
        let open_term = ipc
            .call(
                "term.open",
                serde_json::json!({
                    "node_id": "local:1:contract",
                    "session_name": "cli-term-contract",
                    "cols": 90,
                    "rows": 30
                }),
            )
            .await
            .unwrap();
        assert!(open_term.success, "term.open failed: {:?}", open_term.error);

        let has_open = ipc
            .call("lterm.has", serde_json::json!({ "name": "cli-term-contract" }))
            .await
            .unwrap();
        assert!(has_open.success);
        assert_eq!(has_open.data.unwrap()["exists"], true);

        let exit_term = ipc
            .call(
                "term.exit",
                serde_json::json!({
                    "node_id": "local:1:contract",
                    "name": "cli-term-contract"
                }),
            )
            .await
            .unwrap();
        assert!(exit_term.success, "term.exit failed: {:?}", exit_term.error);

        let has_after_exit = ipc
            .call("lterm.has", serde_json::json!({ "name": "cli-term-contract" }))
            .await
            .unwrap();
        assert!(has_after_exit.success);
        assert_eq!(has_after_exit.data.unwrap()["exists"], false);

        // 2. input.drag & input.scroll (CLI exact param names)
        let drag = ipc
            .call(
                "input.drag",
                serde_json::json!({
                    "node_id": "local",
                    "x": 100,
                    "y": 150,
                    "dx": 40,
                    "dy": -20,
                    "button": "left"
                }),
            )
            .await
            .unwrap();
        assert!(drag.success, "input.drag failed: {:?}", drag.error);
        assert_eq!(drag.data.unwrap()["ok"], true);

        let scroll = ipc
            .call(
                "input.scroll",
                serde_json::json!({
                    "node_id": "local",
                    "x": 200,
                    "y": 240,
                    "scroll_x": 0,
                    "scroll_y": -120
                }),
            )
            .await
            .unwrap();
        assert!(scroll.success, "input.scroll failed: {:?}", scroll.error);
        assert_eq!(scroll.data.unwrap()["ok"], true);

        // 3. file.push & file.pull (CLI remote_path + data_b64 -> binary roundtrip)
        let raw_payload = b"\x00\x01fabushi-binary-test\xff\n".to_vec();
        let encoded_payload = base64::engine::general_purpose::STANDARD.encode(&raw_payload);
        let push = ipc
            .call(
                "file.push",
                serde_json::json!({
                    "node_id": "local:1:contract",
                    "local_path": "/tmp/source.bin",
                    "remote_path": pushed_file.to_string_lossy().to_string(),
                    "data_b64": encoded_payload,
                    "size": raw_payload.len(),
                }),
            )
            .await
            .unwrap();
        assert!(push.success, "file.push failed: {:?}", push.error);
        assert_eq!(push.data.unwrap()["bytes_written"], raw_payload.len() as u64);

        let pull = ipc
            .call(
                "file.pull",
                serde_json::json!({
                    "node_id": "local:1:contract",
                    "remote_path": pushed_file.to_string_lossy().to_string(),
                    "local_path": "/tmp/dest.bin",
                }),
            )
            .await
            .unwrap();
        assert!(pull.success, "file.pull failed: {:?}", pull.error);
        let pull_data = pull.data.unwrap();
        let pulled_b64 = pull_data["data_b64"].as_str().expect("data_b64 in file.pull");
        let decoded = base64::engine::general_purpose::STANDARD
            .decode(pulled_b64)
            .unwrap();
        assert_eq!(decoded, raw_payload);

        // 4. user.info, msg.history, assist.id dispatch check (do NOT return method_not_found when handled)
        let user_info_self = ipc.call("user.info", serde_json::json!({})).await.unwrap();
        assert!(!user_info_self.success);
        assert_eq!(
            user_info_self.error.as_ref().map(|e| e.code.as_str()),
            Some("unauthenticated")
        );

        let msg_hist = ipc
            .call(
                "msg.history",
                serde_json::json!({ "chat_id": 1, "limit": 20 }),
            )
            .await
            .unwrap();
        assert!(!msg_hist.success);
        assert_eq!(
            msg_hist.error.as_ref().map(|e| e.code.as_str()),
            Some("unauthenticated")
        );

        let assist_id = ipc.call("assist.id", serde_json::json!({})).await.unwrap();
        assert!(!assist_id.success);
        assert_ne!(
            assist_id.error.as_ref().map(|e| e.code.as_str()),
            Some("method_not_found")
        );

        // 5. All unhandled / fallback-delegated methods MUST return "method_not_found"
        for (method, params) in [
            ("lterm.unknown", serde_json::json!({})),
            ("user.wallet", serde_json::json!({})),
            ("user.info", serde_json::json!({ "user_id": 42 })),
            ("device.disconnect", serde_json::json!({})),
            ("cloudpc.launch", serde_json::json!({ "node_id": "n1" })),
            ("assist.unknown", serde_json::json!({})),
            ("screen.unknown", serde_json::json!({})),
            ("input.unknown", serde_json::json!({})),
            ("file.unknown", serde_json::json!({})),
            ("clipboard.unknown", serde_json::json!({})),
            (
                "msg.history",
                serde_json::json!({ "chat_id": 1, "limit": 20, "offset_id": 99 }),
            ),
            ("msg.search", serde_json::json!({ "query": "hi" })),
            ("mcp.status", serde_json::json!({})),
            ("vault.status", serde_json::json!({})),
        ] {
            let resp = ipc.call(method, params).await.unwrap();
            assert!(!resp.success, "{method} unexpectedly succeeded");
            assert_eq!(
                resp.error.as_ref().map(|e| e.code.as_str()),
                Some("method_not_found"),
                "expected method_not_found for {method}, got {:?}",
                resp.error
            );
        }

        server.shutdown();
    });
}
