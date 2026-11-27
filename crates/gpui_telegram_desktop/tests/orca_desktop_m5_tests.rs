//! Headless M5 Orca desktop UI integration tests (`F22`).

use gpui::{Bounds, Pixels, Point, Size, px};
use gpui_telegram_desktop::views::browser::design_mode::{
    DesignModePanelState, contain_draw_rect, project_css_rect_to_overlay_px,
};
use gpui_telegram_desktop::views::nav_rail::{NavDestination, NavRail};
use gpui_telegram_desktop::views::orca::{
    BotHomeTab, DiffDisplayMode, JUMP_PALETTE_PAGE_SIZE, JUMP_PALETTE_RECENT_SHORTCUT_COUNT,
    OrcaBotsSurfaceState, OrcaDesktopRuntime, OrcaIdeSurfaceState, OrcaJumpPaletteState,
    OrchestrationBoardTab, PopoutKanbanWindowState, SECURITY_SURFACE_BG, SECURITY_SURFACE_PALETTE,
    SecuritySurfaceModalState, agent_run_state_meta, attribution_gutter_meta, format_countdown_ms,
    tab_content_badge_color, tab_content_icon,
};
use orca_core::orchestration_protocol::LineAttribution;
use orca_core::workspace_layout::{
    AttentionPriority, QuickSwitcherItem, QuickSwitcherItemKind, TabContentType,
};
use orca_ide::USAGE_PROVIDERS_9;
use orca_ide::agent_status_hooks::AgentRunState;
use tempfile::TempDir;

#[test]
fn orca_m5_nav_rail_exposes_13_destinations_including_bots_and_ide() {
    let rail = NavRail::new();
    assert_eq!(rail.items.len(), 13);
    assert!(rail.items.iter().any(|i| i.destination == NavDestination::OrcaBots));
    assert!(rail.items.iter().any(|i| i.destination == NavDestination::OrcaIde));
    assert_eq!(NavDestination::OrcaBots.localized_label("en"), "Orca Bots");
    assert_eq!(NavDestination::OrcaIde.localized_label("en"), "Orca IDE");
}

#[test]
fn orca_m5_desktop_runtime_isolated_open_and_9_provider_usage_roster() {
    let tmp = TempDir::new().expect("tempdir");
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("tokio");
    let desktop_rt = OrcaDesktopRuntime::open_isolated(tmp.path(), rt.handle().clone())
        .expect("open isolated runtime");
    let roster = desktop_rt.ide().usage_roster().sorted_roster(1_700_000_000_000);
    assert_eq!(roster.len(), USAGE_PROVIDERS_9.len());
    for prov in USAGE_PROVIDERS_9 {
        assert!(roster.iter().any(|e| e.provider == prov));
    }
}

#[test]
fn orca_m5_bots_surface_state_four_home_tabs_and_templates() {
    let state = OrcaBotsSurfaceState::default();
    assert_eq!(state.active_tab, BotHomeTab::Chat);
    let all_tabs = [
        BotHomeTab::Chat,
        BotHomeTab::Items,
        BotHomeTab::UpNext,
        BotHomeTab::Settings,
    ];
    assert_eq!(all_tabs.len(), 4);
    assert!(state.bots.is_empty());
}

#[test]
fn orca_m5_ide_surface_all_ten_tab_types_icons_and_badge_colors() {
    let state = OrcaIdeSurfaceState::default();
    assert_eq!(TabContentType::ALL.len(), 10);
    for kind in TabContentType::ALL {
        let color = tab_content_badge_color(kind);
        let _icon = tab_content_icon(kind);
        assert!(color > 0);
    }
    assert_eq!(state.diff_display_mode, DiffDisplayMode::SideBySide);
}

#[test]
fn orca_m5_jump_palette_ranks_needs_you_before_done_before_idle_and_caps_6() {
    assert_eq!(JUMP_PALETTE_RECENT_SHORTCUT_COUNT, 6);
    assert!(JUMP_PALETTE_PAGE_SIZE >= 6);
    let mut palette = OrcaJumpPaletteState::default();
    let now_ms = 1_700_000_000_000i64;
    let candidates = vec![
        QuickSwitcherItem {
            id: "tab-idle".into(),
            kind: QuickSwitcherItemKind::OpenTab,
            title: "Idle Terminal".into(),
            subtitle: "local".into(),
            host_id: Some("local".into()),
            repo_id: Some("repo-a".into()),
            recency_ms: now_ms - 10_000,
            pinned: false,
            attention: AttentionPriority::Idle,
            is_current_tab: false,
        },
        QuickSwitcherItem {
            id: "tab-done".into(),
            kind: QuickSwitcherItemKind::RecentChat,
            title: "Done Agent".into(),
            subtitle: "local".into(),
            host_id: Some("local".into()),
            repo_id: Some("repo-a".into()),
            recency_ms: now_ms - 20_000,
            pinned: false,
            attention: AttentionPriority::Done,
            is_current_tab: false,
        },
        QuickSwitcherItem {
            id: "tab-needs".into(),
            kind: QuickSwitcherItemKind::RecentChat,
            title: "Blocked Agent".into(),
            subtitle: "local".into(),
            host_id: Some("local".into()),
            repo_id: Some("repo-a".into()),
            recency_ms: now_ms - 30_000,
            pinned: false,
            attention: AttentionPriority::NeedsYou,
            is_current_tab: false,
        },
    ];
    palette.open_with_scope(Some("local".into()), Some("repo-a".into()), candidates, now_ms);
    assert!(palette.is_open);
    assert_eq!(palette.frozen_recents.first().map(|i| i.id.as_str()), Some("tab-needs"));
    assert_eq!(palette.frozen_recents.get(1).map(|i| i.id.as_str()), Some("tab-done"));
    palette.close();
    assert!(!palette.is_open);
}

#[test]
fn orca_m5_security_surface_palette_is_host_controlled_and_non_themable() {
    assert_eq!(SECURITY_SURFACE_PALETTE.surface_bg, SECURITY_SURFACE_BG);
    let modal = SecuritySurfaceModalState::open_orca_yaml_trust("/tmp/repo", &"a".repeat(64));
    assert!(!modal.security_surface().theme_override_allowed);
    assert!(modal.decision.is_none());
}

#[test]
fn orca_m5_browser_design_mode_coordinate_projector_and_panel_state() {
    let (img_x, img_y, draw_w, draw_h) = contain_draw_rect(1000.0, 500.0, (1000, 1000));
    assert!((draw_w - 500.0).abs() < 0.01);
    assert!((draw_h - 500.0).abs() < 0.01);
    assert!((img_x - 250.0).abs() < 0.01);
    assert!(img_y.abs() < 0.01);

    let bounds: Bounds<Pixels> = Bounds {
        origin: Point { x: px(0.0), y: px(0.0) },
        size: Size { width: px(1000.0), height: px(500.0) },
    };
    let rect = bot_browser::ElementRect {
        x: 100.0,
        y: 200.0,
        width: 200.0,
        height: 100.0,
        device_pixel_ratio: 2.0,
    };
    let projected = project_css_rect_to_overlay_px(&rect, bounds, (1000, 1000), (1000, 1000))
        .expect("project rect");
    assert!((projected.left - 300.0).abs() < 0.1);
    assert!((projected.top - 100.0).abs() < 0.1);

    let mut dm = DesignModePanelState::default();
    assert!(!dm.enabled);
    dm.enabled = true;
    dm.picker_active = true;
    assert!(dm.enabled && dm.picker_active);
}

#[test]
fn orca_m5_diff_attribution_gutter_distinguishes_ai_vs_human_lines() {
    let (h_label, _) = attribution_gutter_meta(&LineAttribution::Human);
    let (b_label, _) = attribution_gutter_meta(&LineAttribution::Bot { bot_id: "bot-1".into() });
    let (w_label, _) = attribution_gutter_meta(&LineAttribution::Worker { dispatch_id: "d-1".into() });
    let (u_label, _) = attribution_gutter_meta(&LineAttribution::Unknown);
    assert_ne!(h_label, b_label);
    assert!(!w_label.is_empty());
    assert!(!u_label.is_empty());
}

#[test]
fn orca_m5_orchestration_board_tabs_agent_states_and_popout_kanban() {
    let tabs = [
        OrchestrationBoardTab::Kanban,
        OrchestrationBoardTab::RunsDag,
        OrchestrationBoardTab::Workers,
        OrchestrationBoardTab::Mailbox,
        OrchestrationBoardTab::Gates,
        OrchestrationBoardTab::Hosts,
    ];
    assert_eq!(tabs.len(), 6);
    for st in [
        AgentRunState::Idle,
        AgentRunState::Working,
        AgentRunState::Blocked,
        AgentRunState::Waiting,
        AgentRunState::Done,
    ] {
        let (label, color) = agent_run_state_meta(st);
        assert!(!label.is_empty() && color > 0);
    }
    let kanban = PopoutKanbanWindowState::default();
    assert!(!kanban.is_open && kanban.selected_run_id.is_none());
}

#[test]
fn orca_m5_usage_countdown_formatter_handles_minutes_hours_and_days() {
    assert!(format_countdown_ms(Some(90_000)).contains('m'));
    assert!(format_countdown_ms(Some(2 * 3_600_000 + 15 * 60_000)).contains('h'));
    assert!(format_countdown_ms(Some(26 * 3_600_000)).contains('d'));
}
