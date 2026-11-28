//! OpenBot 1:1 Browser UI/UX & Logic Comprehensive Verification Suite.
//!
//! Covers:
//! 1. Smart Omnibox (`browser_address_url`, `browser_display_url`, `browser_search_query`,
//!    `browser_completion`, `browser_suggestions`, max-8 ranking & tab switch pills).
//! 2. Multi-tab Session Engine (default Google tab, placement, duplicate, reorder,
//!    close/close-left/close-right/close-others, 25-tab cap, closed-tab stack restore,
//!    unseen-tab notice tracking).
//! 3. Platform Keyboard Shortcuts (`Mac` & `Pc` matrices + shortcut glyphs).
//! 4. Floating `BrowserDock` & `BrowserIsland` priority ordering, auto-tuck on takeover,
//!    reply 6s auto-dismiss & hover pause, and tucked pill labels.
//! 5. `BrowserTeachBar` recording lifecycle, elapsed `m:ss` formatting, and chat teach prompt.
//! 6. Exact OpenBot Browser Chrome geometry constants & `IconName::ALL.len() == 64` preservation.
//! 7. Full Headless GPUI `AppShell` <-> `BrowserWorkspace` <-> Right `SidePanel` E2E roundtrip.

use std::sync::Arc;

use bot_browser::BrowserControlMode;
use gpui::{
    bounds, point, px, size, App, AppContext, Application, Entity, Modifiers, WindowBounds,
    WindowOptions,
};
use gpui_telegram_client::{ClientConfig, TelegramClient};
use gpui_telegram_desktop::theme::openbot_palette::OpenBotPalette;
use gpui_telegram_desktop::theme::{IconName, TelegramAssetSource};
use gpui_telegram_desktop::views::browser::{
    browser_action_label, browser_address_url, browser_completion, browser_display_url,
    browser_profile_hue_color, browser_search_query, browser_search_url, browser_shortcut_glyph,
    browser_suggestions, format_teach_message_for_chat, is_recordable_browser_url,
    resolve_browser_view_command, ActiveBrowserInput, BrowserDockUiState, BrowserIslandView,
    BrowserShortcutName, BrowserShortcutPlatform, BrowserSuggestionKind, BrowserTabIndexTarget,
    BrowserTeachUiState, BrowserViewCommand, OpenBotBrowserSession, TabPlacement,
    BROWSER_ADDRESS_ROW_HEIGHT_PX, BROWSER_CLOSED_LIMIT, BROWSER_DOCK_MAX_WIDTH_PX,
    BROWSER_DOCK_REPLY_CLOSE_MS, BROWSER_FULL_TOP_HEIGHT_PX, BROWSER_HISTORY_LIMIT,
    BROWSER_MAC_TRAFFIC_LIGHTS_INSET_PX, BROWSER_MAX_SUGGESTIONS, BROWSER_MAX_TABS,
    BROWSER_NEW_TAB_URL, BROWSER_PREVIEW_ASPECT_RATIO, BROWSER_TAB_MAX_WIDTH_PX,
    BROWSER_TAB_MIN_WIDTH_PX,
};
use gpui_telegram_desktop::views::openbot::side_panel::SidePanelPaneKind;
use gpui_telegram_desktop::views::shell::{ActiveWorkspacePanel, AppShell};
use tempfile::TempDir;

// ============================================================================
// 1. Smart Omnibox URL Resolution, Completion & Suggestions
// ============================================================================

#[test]
fn openbot_browser_omnibox_url_search_display_and_completion() {
    assert_eq!(browser_address_url(""), BROWSER_NEW_TAB_URL);
    assert_eq!(browser_address_url("about:blank"), "about:blank");
    assert_eq!(browser_address_url("google.com"), "https://google.com");
    assert_eq!(
        browser_address_url("github.com/nightly-labs/openbot"),
        "https://github.com/nightly-labs/openbot"
    );
    assert_eq!(
        browser_address_url("http://localhost:3000/api?v=1"),
        "http://localhost:3000/api?v=1"
    );
    assert_eq!(
        browser_address_url("localhost:5173"),
        "https://localhost:5173"
    );
    assert_eq!(
        browser_address_url("openbot browser dock"),
        browser_search_url("openbot browser dock")
    );
    assert_eq!(
        browser_search_query("https://www.google.com/search?q=rust+gpui%20browser&hl=zh-CN"),
        Some("rust gpui browser".to_string())
    );
    assert_eq!(browser_display_url("https://www.google.com/"), "google.com");
    assert_eq!(
        browser_display_url("https://docs.rs/gpui/"),
        "docs.rs/gpui"
    );
    assert!(is_recordable_browser_url("https://example.com"));
    assert!(is_recordable_browser_url("http://127.0.0.1:8080"));
    assert!(!is_recordable_browser_url("about:blank"));
    assert!(!is_recordable_browser_url("javascript:alert(1)"));

    // Suggestions ranking: completed open tab > history > URL > search + cap at 8
    let mut session = OpenBotBrowserSession::new_empty();
    let t1 = session
        .open_tab(
            "https://www.google.com",
            None,
            None,
            "default",
            TabPlacement::default(),
            true,
        )
        .unwrap();
    let t2 = session
        .open_tab(
            "https://github.com/nightly-labs/openbot",
            None,
            None,
            "default",
            TabPlacement::default(),
            true,
        )
        .unwrap();
    for i in 0..12 {
        session.record_visit(
            &format!("https://docs-{i}.openbot.dev/guide"),
            &format!("OpenBot Guide {i}"),
        );
    }
    // Boost one history entry so inline completion picks highest visits
    session.record_visit("https://docs-3.openbot.dev/guide", "OpenBot Guide 3");
    session.record_visit("https://docs-3.openbot.dev/guide", "OpenBot Guide 3");

    let comp = browser_completion("docs-", &session.history);
    assert_eq!(
        comp.map(|c| c.url.as_str()),
        Some("https://docs-3.openbot.dev/guide")
    );

    // Query matching another open tab produces Tab row with tab_id + Search row
    let rows = browser_suggestions("github", None, &session.tabs, Some(&t1), &session.history);
    assert!(rows
        .iter()
        .any(|r| r.kind == BrowserSuggestionKind::Tab && r.tab_id.as_deref() == Some(t2.as_str())));
    assert!(rows.iter().any(|r| r.kind == BrowserSuggestionKind::Search));

    // Many matches capped at BROWSER_MAX_SUGGESTIONS (8)
    let capped = browser_suggestions(
        "openbot",
        comp,
        &session.tabs,
        Some(&t1),
        &session.history,
    );
    assert!(capped.len() <= BROWSER_MAX_SUGGESTIONS);

    // Empty query returns up to 6 recent history entries excluding currently open tabs
    let empty_rows = browser_suggestions("", None, &session.tabs, Some(&t1), &session.history);
    assert_eq!(empty_rows.len(), 6);
    assert!(empty_rows.iter().all(|r| r.url != "https://www.google.com"));
}

// ============================================================================
// 2. Multi-Tab Session Engine (Order, Duplicate, Close L/R/Others, Reopen, Cap)
// ============================================================================

#[test]
fn openbot_browser_multi_tab_session_crud_order_close_reopen_and_cap() {
    let mut session = OpenBotBrowserSession::new_with_default_tab();
    assert_eq!(session.tabs.len(), 1);
    assert_eq!(session.tabs[0].url, BROWSER_NEW_TAB_URL);
    assert_eq!(session.tabs[0].title, "Google");

    let t1 = session.tabs[0].id.clone();
    let t2 = session
        .open_tab(
            "https://example.com",
            Some("agent-1".into()),
            None,
            "default",
            TabPlacement::default(),
            false, // unseen!
        )
        .unwrap();
    let t3 = session
        .open_tab(
            "https://rust-lang.org",
            Some("agent-1".into()),
            None,
            "default",
            TabPlacement::default(),
            true,
        )
        .unwrap();

    // Unseen tabs tracking for ConversationBrowserTabNotice
    assert_eq!(session.unseen_tabs().len(), 1);
    assert_eq!(session.unseen_tabs()[0].id, t2);
    session.mark_all_tabs_noticed();
    assert!(session.unseen_tabs().is_empty());

    // Duplicate places immediately to the right of target tab
    let t1_dup = session.duplicate_tab(&t1).unwrap();
    let ids: Vec<&str> = session.tabs.iter().map(|t| t.id.as_str()).collect();
    assert_eq!(ids, vec![t1.as_str(), t1_dup.as_str(), t2.as_str(), t3.as_str()]);

    // Reorder + cyclic select_offset + select_index
    assert!(session.move_tab(&t3, 1));
    assert_eq!(session.tabs[1].id, t3);
    assert!(session.activate_tab(&t1));
    assert_eq!(session.select_offset(-1).as_deref(), Some(t2.as_str()));
    assert_eq!(
        session.select_index(BrowserTabIndexTarget::Index(0)).as_deref(),
        Some(t1.as_str())
    );
    assert_eq!(
        session.select_index(BrowserTabIndexTarget::Last).as_deref(),
        Some(t2.as_str())
    );

    // Close left / right / others + reopen stack
    let closed_r = session.close_right(&t3);
    assert_eq!(closed_r, 2);
    assert_eq!(session.tabs.len(), 2);
    assert!(session.can_reopen());

    let reopened = session
        .reopen_closed_tab(None, None, "default")
        .unwrap()
        .expect("reopen tab");
    assert_eq!(session.tabs.len(), 3);
    assert!(session.tab_by_id(&reopened).is_some());

    let closed_others = session.close_others(&t1);
    assert_eq!(closed_others, 2);
    assert_eq!(session.tabs.len(), 1);
    assert_eq!(session.tabs[0].id, t1);

    // Enforce BROWSER_MAX_TABS (25) cap
    while session.tabs.len() < BROWSER_MAX_TABS {
        session
            .open_tab(
                "https://example.org",
                None,
                None,
                "default",
                TabPlacement::default(),
                true,
            )
            .unwrap();
    }
    assert!(session
        .open_tab(
            "https://overflow.example",
            None,
            None,
            "default",
            TabPlacement::default(),
            true,
        )
        .is_err());

    // Enforce BROWSER_CLOSED_LIMIT (25) and BROWSER_HISTORY_LIMIT (200)
    for i in 0..(BROWSER_HISTORY_LIMIT + 25) {
        session.record_visit(&format!("https://h{i}.example.com"), "Hist");
    }
    assert_eq!(session.history.len(), BROWSER_HISTORY_LIMIT);

    let all_ids: Vec<String> = session.tabs.iter().map(|t| t.id.clone()).collect();
    session.close_tabs(&all_ids);
    assert!(session.tabs.is_empty());
    assert!(session.closed_tabs.len() <= BROWSER_CLOSED_LIMIT);
}

// ============================================================================
// 3. Platform Keyboard Shortcuts Matrix & Glyphs
// ============================================================================

#[test]
fn openbot_browser_shortcuts_platform_matrix_and_glyphs() {
    let mac_cmd = Modifiers {
        platform: true,
        ..Default::default()
    };
    let pc_ctrl = Modifiers {
        control: true,
        ..Default::default()
    };

    assert_eq!(
        resolve_browser_view_command("t", mac_cmd, BrowserShortcutPlatform::Mac),
        Some(BrowserViewCommand::NewTab)
    );
    assert_eq!(
        resolve_browser_view_command(
            "t",
            Modifiers {
                platform: true,
                shift: true,
                ..Default::default()
            },
            BrowserShortcutPlatform::Mac
        ),
        Some(BrowserViewCommand::Reopen)
    );
    assert_eq!(
        resolve_browser_view_command("w", mac_cmd, BrowserShortcutPlatform::Mac),
        Some(BrowserViewCommand::Close)
    );
    assert_eq!(
        resolve_browser_view_command("l", mac_cmd, BrowserShortcutPlatform::Mac),
        Some(BrowserViewCommand::FocusAddress)
    );
    assert_eq!(
        resolve_browser_view_command("r", mac_cmd, BrowserShortcutPlatform::Mac),
        Some(BrowserViewCommand::Reload)
    );
    assert_eq!(
        resolve_browser_view_command("[", mac_cmd, BrowserShortcutPlatform::Mac),
        Some(BrowserViewCommand::Back)
    );
    assert_eq!(
        resolve_browser_view_command("]", mac_cmd, BrowserShortcutPlatform::Mac),
        Some(BrowserViewCommand::Forward)
    );
    assert_eq!(
        resolve_browser_view_command(
            "right",
            Modifiers {
                platform: true,
                alt: true,
                ..Default::default()
            },
            BrowserShortcutPlatform::Mac
        ),
        Some(BrowserViewCommand::Select(1))
    );
    assert_eq!(
        resolve_browser_view_command(
            "tab",
            Modifiers {
                control: true,
                shift: true,
                ..Default::default()
            },
            BrowserShortcutPlatform::Mac
        ),
        Some(BrowserViewCommand::Select(-1))
    );
    assert_eq!(
        resolve_browser_view_command(
            "pagedown",
            Modifiers {
                control: true,
                shift: true,
                ..Default::default()
            },
            BrowserShortcutPlatform::Mac
        ),
        Some(BrowserViewCommand::Move(1))
    );

    // PC bindings
    assert_eq!(
        resolve_browser_view_command("t", pc_ctrl, BrowserShortcutPlatform::Pc),
        Some(BrowserViewCommand::NewTab)
    );
    assert_eq!(
        resolve_browser_view_command("1", pc_ctrl, BrowserShortcutPlatform::Pc),
        Some(BrowserViewCommand::SelectIndex(BrowserTabIndexTarget::Index(0)))
    );
    assert_eq!(
        resolve_browser_view_command("9", pc_ctrl, BrowserShortcutPlatform::Pc),
        Some(BrowserViewCommand::SelectIndex(BrowserTabIndexTarget::Last))
    );

    assert_eq!(
        browser_shortcut_glyph(BrowserShortcutName::NewTab, BrowserShortcutPlatform::Mac),
        "⌘T"
    );
    assert_eq!(
        browser_shortcut_glyph(BrowserShortcutName::Reopen, BrowserShortcutPlatform::Mac),
        "⇧⌘T"
    );
    assert_eq!(
        browser_shortcut_glyph(BrowserShortcutName::NewTab, BrowserShortcutPlatform::Pc),
        "Ctrl+T"
    );
}

// ============================================================================
// 4. BrowserDock & BrowserIsland Priority, Auto-Tuck & Reply TTL
// ============================================================================

#[test]
fn openbot_browser_dock_island_priority_auto_tuck_and_reply_ttl() {
    let mut dock = BrowserDockUiState::new();
    dock.active_agent.name = "New agent".to_string();

    // Non-question reply auto-dismisses after BROWSER_DOCK_REPLY_CLOSE_MS (6000ms) unless hovered
    dock.latest_reply = Some(("r1".into(), "已打开 Google 首页".into(), false, 10_000));
    assert!(matches!(
        dock.resolve_island(&BrowserControlMode::Idle, 12_000, true),
        Some(BrowserIslandView::Reply { .. })
    ));
    assert!(dock
        .resolve_island(&BrowserControlMode::Idle, 10_000 + BROWSER_DOCK_REPLY_CLOSE_MS, true)
        .is_none());

    // Hovering pauses auto-dismiss
    dock.reply_hovered = true;
    assert!(matches!(
        dock.resolve_island(
            &BrowserControlMode::Idle,
            10_000 + BROWSER_DOCK_REPLY_CLOSE_MS + 5_000,
            true
        ),
        Some(BrowserIslandView::Reply { .. })
    ));
    dock.reply_hovered = false;
    dock.dismiss_reply("r1");
    assert!(dock
        .resolve_island(&BrowserControlMode::Idle, 12_000, true)
        .is_none());

    // Question reply never auto-dismisses and forces dock untucked
    dock.tucked = true;
    dock.latest_reply = Some(("r2".into(), "要继续提交订单吗？".into(), true, 10_000));
    let q_island = dock.resolve_island(&BrowserControlMode::Idle, 99_999, true);
    assert!(matches!(
        q_island,
        Some(BrowserIslandView::Reply {
            is_question: true,
            ..
        })
    ));
    dock.reconcile_auto_tuck(q_island.as_ref());
    assert!(!dock.tucked, "Question reply must auto-expand tucked dock");

    // Takeover request overrides reply & auto-tucks only when draft is empty
    let req_mode = BrowserControlMode::WaitingForTakeover {
        reason: "Complete captcha".into(),
    };
    let req_island = dock.resolve_island(&req_mode, 12_000, true);
    assert!(matches!(
        req_island,
        Some(BrowserIslandView::ActivityRequest { .. })
    ));
    dock.draft = "keep open".into();
    dock.reconcile_auto_tuck(req_island.as_ref());
    assert!(!dock.tucked, "Non-empty draft prevents auto-tuck");
    dock.draft.clear();
    dock.reconcile_auto_tuck(req_island.as_ref());
    assert!(dock.tucked && dock.folded_for_takeover);

    // Tucked label localization
    assert!(dock
        .tucked_label(req_island.as_ref(), true)
        .contains("需要你在该页面操作"));
    assert!(dock
        .tucked_label(req_island.as_ref(), false)
        .contains("needs you on this page"));

    // Leaving takeover request automatically re-expands dock
    dock.reconcile_auto_tuck(None);
    assert!(!dock.tucked && !dock.folded_for_takeover);
}

// ============================================================================
// 5. BrowserTeachBar & Localized Action Labels
// ============================================================================

#[test]
fn openbot_browser_teach_bar_lifecycle_and_chat_prompt() {
    let mut teach = BrowserTeachUiState::Inactive;
    assert!(!teach.is_active());
    teach = BrowserTeachUiState::Ready;
    assert!(teach.is_active() && !teach.is_recording());

    teach = BrowserTeachUiState::Recording {
        started_at_ms: 1_000,
        step_count: 2,
    };
    assert!(teach.is_recording());
    assert_eq!(teach.elapsed_label(1_000 + 65_000), "1:05");
    assert_eq!(teach.elapsed_label(1_000 + 5_000), "0:05");

    let zh_prompt = format_teach_message_for_chat(&["click #login".into()], true);
    assert!(zh_prompt.contains("我在浏览器中向你演示了一项任务"));
    assert!(zh_prompt.contains("1. click #login"));

    let en_prompt = format_teach_message_for_chat(&["click #login".into()], false);
    assert!(en_prompt.contains("I demonstrated a task in the browser"));

    assert_eq!(browser_action_label("click", true), "正在点击…");
    assert_eq!(browser_action_label("snapshot", false), "Reading page…");
}

// ============================================================================
// 6. OpenBot Chrome Geometry Constants & IconName Invariant
// ============================================================================

#[test]
fn openbot_browser_chrome_geometry_and_icon_invariants() {
    assert_eq!(BROWSER_FULL_TOP_HEIGHT_PX, 38.0);
    assert_eq!(BROWSER_ADDRESS_ROW_HEIGHT_PX, 44.0);
    assert_eq!(BROWSER_MAC_TRAFFIC_LIGHTS_INSET_PX, 82.0);
    assert_eq!(BROWSER_DOCK_MAX_WIDTH_PX, 640.0);
    assert_eq!(BROWSER_TAB_MIN_WIDTH_PX, 120.0);
    assert_eq!(BROWSER_TAB_MAX_WIDTH_PX, 200.0);
    assert!((BROWSER_PREVIEW_ASPECT_RATIO - 1.6).abs() < 1e-6);
    assert_eq!(
        IconName::ALL.len(),
        64,
        "OpenBot browser vector icons must never mutate IconName::ALL.len()"
    );

    let dark = OpenBotPalette::dark();
    let light = OpenBotPalette::light();
    let c_dark = browser_profile_hue_color(210, &dark);
    let c_light = browser_profile_hue_color(210, &light);
    assert_ne!(c_dark, c_light);
}

// ============================================================================
// 7. Full Cross-Platform Headless GPUI AppShell <-> SidePanel <-> BrowserFullView E2E
// ============================================================================

const HEADLESS_CHILD_ENV: &str = "GPUI_OPENBOT_BROWSER_HEADLESS_CHILD";
const HEADLESS_TARGET_ENV: &str = "GPUI_OPENBOT_BROWSER_HEADLESS_TARGET";

fn dispatch_headless_browser_target(target: &str) -> ! {
    let target_owned = target.to_string();
    let outer = std::panic::catch_unwind(std::panic::AssertUnwindSafe(move || {
        Application::headless()
            .with_assets(TelegramAssetSource::new())
            .run(move |cx: &mut App| {
                let inner = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    match target_owned.as_str() {
                        "test_openbot_browser_headless_appshell_sidepanel_fullview_dock_teach_e2e" => {
                            body_headless_appshell_openbot_browser_e2e(cx);
                        }
                        other => panic!("unknown headless browser test target: {other}"),
                    }
                }));
                match inner {
                    Ok(()) => std::process::exit(0),
                    Err(payload) => {
                        let msg = payload
                            .downcast_ref::<&str>()
                            .copied()
                            .or_else(|| payload.downcast_ref::<String>().map(String::as_str))
                            .unwrap_or("panic");
                        eprintln!("[headless openbot browser panic] {target_owned}: {msg}");
                        std::process::exit(101);
                    }
                }
            });
    }));
    if let Err(payload) = outer {
        let msg = payload
            .downcast_ref::<&str>()
            .copied()
            .or_else(|| payload.downcast_ref::<String>().map(String::as_str))
            .unwrap_or("panic");
        eprintln!("[headless outer panic] {target}: {msg}");
        std::process::exit(101);
    }
    std::process::exit(0);
}

#[cfg(target_os = "macos")]
#[used]
#[unsafe(link_section = "__DATA,__mod_init_func")]
static MACOS_OPENBOT_BROWSER_HEADLESS_ENTRY: extern "C" fn() =
    macos_openbot_browser_headless_entry;

#[cfg(target_os = "macos")]
extern "C" fn macos_openbot_browser_headless_entry() {
    if std::env::var(HEADLESS_CHILD_ENV).as_deref() == Ok("1") {
        if let Ok(target) = std::env::var(HEADLESS_TARGET_ENV) {
            dispatch_headless_browser_target(&target);
        }
    }
}

fn run_headless_browser_subprocess(test_fn_name: &str) {
    if std::env::var(HEADLESS_CHILD_ENV).as_deref() == Ok("1") {
        dispatch_headless_browser_target(test_fn_name);
    }
    let cfg_dir = TempDir::new().expect("headless config tempdir");
    let exe = std::env::current_exe().expect("current_exe");
    let output = std::process::Command::new(exe)
        .env(HEADLESS_CHILD_ENV, "1")
        .env(HEADLESS_TARGET_ENV, test_fn_name)
        .env("GPUI_TELEGRAM_CONFIG_DIR", cfg_dir.path())
        .env("GPUI_OFFLINE_TEST", "1")
        .arg("--exact")
        .arg(test_fn_name)
        .arg("--nocapture")
        .output()
        .expect("spawn headless gpui child");
    assert!(
        output.status.success(),
        "headless gpui child failed for {test_fn_name}:\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn test_openbot_browser_headless_appshell_sidepanel_fullview_dock_teach_e2e() {
    run_headless_browser_subprocess(
        "test_openbot_browser_headless_appshell_sidepanel_fullview_dock_teach_e2e",
    );
}

fn body_headless_appshell_openbot_browser_e2e(cx: &mut App) {
    let mut cfg = ClientConfig::default();
    cfg.server_url = "http://127.0.0.1:9".to_string();
    let client = Arc::new(TelegramClient::new(cfg));
    let _rt = client.runtime().enter();

    let shell: Entity<AppShell> = cx.new(|cx| {
        let mut s = AppShell::new_with_client(client.clone(), cx);
        s.is_authenticated = true;
        s
    });

    // 1. Opening right SidePanel -> Browser creates background BrowserWorkspace with Google tab (`image.png`)
    shell.update(cx, |s, scx| {
        s.open_unified_side_panel_pane(SidePanelPaneKind::Browser, scx);
    });
    {
        let s = shell.read(cx);
        assert_eq!(s.active_panel, ActiveWorkspacePanel::Chats);
        assert_eq!(
            s.bot_fusion.side_panel.active_top_tab,
            Some(SidePanelPaneKind::Browser)
        );
        let bw = s
            .browser_workspace()
            .expect("background BrowserWorkspace must exist when Browser SidePanel opens")
            .read(cx);
        assert_eq!(bw.session.tabs.len(), 1);
        assert_eq!(bw.session.tabs[0].url, BROWSER_NEW_TAB_URL);
        assert_eq!(bw.session.tabs[0].title, "Google");
        assert!(
            bw.is_workspace_occluded(),
            "background BrowserWorkspace stays hidden/occluded while in SidePanel preview mode"
        );
        assert_eq!(s.bot_fusion.side_panel.state.browser.tabs.len(), 1);
    }

    // 2. Toggle SidePanel `"以标签页显示页面"` syncs to BrowserWorkspace::session.page_tabs_shown
    shell.update(cx, |s, scx| {
        s.toggle_side_panel_browser_page_tabs(scx);
    });
    {
        let s = shell.read(cx);
        assert!(s.bot_fusion.side_panel.browser_page_tabs_shown);
        assert!(s.browser_workspace().unwrap().read(cx).session.page_tabs_shown);
    }
    shell.update(cx, |s, scx| {
        s.toggle_side_panel_browser_page_tabs(scx);
    });
    assert!(!shell.read(cx).bot_fusion.side_panel.browser_page_tabs_shown);

    // 3. Clicking preview card (`open_browser_full_view_for_tab`) enters BrowserFullView (`image_2.png`)
    let first_tab_id = shell
        .read(cx)
        .browser_workspace()
        .unwrap()
        .read(cx)
        .session
        .tabs[0]
        .id
        .clone();
    shell.update(cx, |s, scx| {
        s.open_browser_full_view_for_tab(Some(&first_tab_id), scx);
    });
    {
        let s = shell.read(cx);
        assert_eq!(s.active_panel, ActiveWorkspacePanel::Browser);
        let bw = s.browser_workspace().unwrap().read(cx);
        assert!(!bw.is_workspace_occluded());
        assert_eq!(bw.session.active_tab_id.as_deref(), Some(first_tab_id.as_str()));
    }

    // 4. Multi-tab commands + popover occlusion guard (separate from workspace occlusion)
    let bw_ent = shell.read(cx).browser_workspace().unwrap().clone();
    bw_ent.update(cx, |w, wcx| {
        assert!(w.handle_browser_view_command(BrowserViewCommand::NewTab, None, wcx));
        assert_eq!(w.session.tabs.len(), 2);
        w.navigate("github.com/nightly-labs/openbot", wcx);
        assert_eq!(w.url_input, "https://github.com/nightly-labs/openbot");
        w.page_menu_open = true;
        assert!(
            w.has_covering_popover(),
            "page_menu_open must trigger macOS WKWebView popover cover guard"
        );
        assert!(
            !w.is_workspace_occluded(),
            "popover guard must not mark workspace itself occluded"
        );
        assert!(w.handle_non_char_key_input("escape", false, wcx));
        assert!(!w.page_menu_open);

        // Focus address bar via Cmd+L command & cycle suggestions
        assert!(w.handle_browser_view_command(BrowserViewCommand::FocusAddress, None, wcx));
        assert_eq!(w.active_input, ActiveBrowserInput::AddressBar);
        assert!(w.address_suggestions_open);
    });

    // 5. Floating BrowserDock message submission routes into AppShell chat & TeachBar lifecycle
    bw_ent.update(cx, |w, wcx| {
        w.set_active_input(ActiveBrowserInput::BrowserDockInput, None, wcx);
        assert!(w.insert_text_to_active_input("帮我总结这个页面", wcx));
        assert_eq!(w.dock.draft, "帮我总结这个页面");
        assert!(w.handle_non_char_key_input("enter", false, wcx));
        assert!(w.dock.draft.is_empty());

        // Takeover -> request_back_to_chat releases UserTakeover and returns to Chat + Browser SidePanel
        w.user_take_over(wcx);
        assert!(matches!(
            w.control_mode(),
            BrowserControlMode::UserTakeover { .. }
        ));
        w.request_back_to_chat(true, wcx);
    });
    {
        let s = shell.read(cx);
        assert_eq!(s.active_panel, ActiveWorkspacePanel::Chats);
        assert_eq!(
            s.bot_fusion.side_panel.active_top_tab,
            Some(SidePanelPaneKind::Browser)
        );
        let bw = s.browser_workspace().unwrap().read(cx);
        assert!(
            !matches!(bw.control_mode(), BrowserControlMode::UserTakeover { .. }),
            "Back to chat must cleanly release user control"
        );
    }

    // 6. Background unseen tab surfaces for ConversationBrowserTabNotice when Browser SidePanel closed
    shell.update(cx, |s, scx| {
        s.close_unified_side_panel(scx);
    });
    bw_ent.update(cx, |w, _| {
        let _ = w.session.open_tab(
            "https://news.ycombinator.com",
            Some("agent-1".into()),
            None,
            "default",
            TabPlacement::default(),
            false,
        );
    });
    assert_eq!(
        bw_ent.read(cx).session.unseen_tabs().len(),
        1,
        "Unseen background tab must trigger ConversationBrowserTabNotice"
    );

    // 7. Re-enter BrowserFullView, run TeachBar save, then close all tabs -> auto BackToChat
    shell.update(cx, |s, scx| {
        s.open_browser_full_view_for_tab(None, scx);
    });
    bw_ent.update(cx, |w, wcx| {
        w.open_teach_bar(wcx);
        assert_eq!(w.teach, BrowserTeachUiState::Ready);
        w.start_teach_recording(wcx);
        assert!(w.teach.is_recording());
        assert!(w.session.active_tab().unwrap().recording);
        w.save_teach_recording(wcx);
        assert_eq!(w.teach, BrowserTeachUiState::Inactive);
    });
    assert_eq!(
        shell.read(cx).active_panel,
        ActiveWorkspacePanel::Chats,
        "Saving teach recording sends prompt and returns to Chat"
    );

    // Closing last tab automatically returns to Chat
    shell.update(cx, |s, scx| {
        s.open_browser_full_view_for_tab(None, scx);
    });
    assert_eq!(shell.read(cx).active_panel, ActiveWorkspacePanel::Browser);
    bw_ent.update(cx, |w, wcx| {
        let all_ids: Vec<String> = w.session.tabs.iter().map(|t| t.id.clone()).collect();
        w.close_tabs(&all_ids, wcx);
    });
    assert_eq!(
        shell.read(cx).active_panel,
        ActiveWorkspacePanel::Chats,
        "Closing the last browser tab must emit BackToChat and return to Chats"
    );
    assert!(bw_ent.read(cx).session.tabs.is_empty());

    // 8. On macOS, also execute full offscreen GPUI window.draw(cx) in BrowserFullView
    if cfg!(target_os = "macos") {
        shell.update(cx, |s, scx| {
            s.open_browser_full_view_for_tab(None, scx);
        });
        let win = cx
            .open_window(
                WindowOptions {
                    window_bounds: Some(WindowBounds::Windowed(bounds(
                        point(px(0.0), px(0.0)),
                        size(px(1280.0), px(800.0)),
                    ))),
                    show: false,
                    ..Default::default()
                },
                |_window, _cx| shell.clone(),
            )
            .expect("open headless AppShell window");
        cx.update_window(win.into(), |_, window, cx| {
            let _ = window.draw(cx);
        })
        .expect("draw OpenBot BrowserFullView frame");
    }
}
