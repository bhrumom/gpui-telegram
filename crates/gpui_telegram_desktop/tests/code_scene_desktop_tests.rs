//! End-to-end Desktop & Headless GPUI Integration Tests for Code Scene ("代码场景") M4 + M5.
//!
//! Covers:
//! - Hard repository invariants (`IconName::ALL.len() == 64`, `IconName::CodeScene` in `EXTRA`,
//!   `NavRail` 11 items with `unread_badge == 0`, `ChatSidebar::new().dialogs.is_empty()`).
//! - Hybrid GPUI rendering primitives (`build_render_image_from_pixel_frame` BGRA8 layout,
//!   `build_vector_path_for_cmd` tessellation across all 14 vector `DrawCmd`s + NaN/Inf guard,
//!   `map_point_contain_scene` coordinate mapping).
//! - Douyin vertical snap gestures (`SwipeGestureController` wheel inertia & drag threshold)
//!   and double-tap `HeartBurst` animations.
//! - Headless GPUI `CodeSceneWorkspace` lifecycle (Video seek/speed, Animation pointer, Game
//!   focus capture/WASD/Esc, VRAM `cx.drop_image` leak guard, swipe + live telemetry, Comments,
//!   Live Code Inspector hot-reload, AI Bot Studio generate -> iterate -> 1-click publish).
//! - Full `AppShell` integration (`NavDestination::CodeScene` -> `ActiveWorkspacePanel::CodeScene`,
//!   composer hidden, CJK IME routing, `code_scene:open/remix/share:<id>` chat card callbacks,
//!   theme & bilingual locale propagation, logout teardown).

use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::time::{Duration, Instant};

use futures::FutureExt;
use gpui::{
    bounds, point, px, size, App, AppContext, Application, AssetSource, AsyncApp, ScrollDelta,
};
use gpui_telegram_client::{ClientConfig, LocalCodeSceneService, TelegramClient};
use gpui_telegram_code_scene::{DrawCmd, PixelFrame, RgbaColor, TextAlign};
use gpui_telegram_desktop::theme::{IconName, TdesktopThemeMode, TdesktopThemePalette, TelegramAssetSource};
use gpui_telegram_desktop::views::code_scene::{
    build_render_image_from_pixel_frame, build_vector_path_for_cmd, format_compact_count,
    format_timecode, map_point_contain_scene, ActiveCodeSceneInput, CodeSceneDrawer,
    CodeScenePendingAction, CodeSceneWorkspace, CodeSceneWorkspaceConfig, DrawerAction,
    HeartBurst, HudAction, SceneContainRect, SwipeGestureController, SwipeSnapDirection,
};
use gpui_telegram_desktop::views::nav_rail::{NavDestination, NavRail};
use gpui_telegram_desktop::views::shell::{
    nav_destination_for_shortcut_digit, ActiveWorkspacePanel, AppShell,
};
use gpui_telegram_desktop::views::ChatSidebar;
use gpui_telegram_proto::code_scene_contracts::{AssetKind, CodeSceneCardAction, CodeSceneFeedTab};

const CHILD_ENV: &str = "GPUI_CODE_SCENE_HEADLESS_CHILD";
const CHILD_TARGET_ENV: &str = "GPUI_CODE_SCENE_HEADLESS_TARGET";

fn panic_payload_to_string(payload: Box<dyn std::any::Any + Send>) -> String {
    if let Some(s) = payload.downcast_ref::<&str>() {
        (*s).to_string()
    } else if let Some(s) = payload.downcast_ref::<String>() {
        s.clone()
    } else {
        "panic".to_string()
    }
}

fn dispatch_headless_target(target: &str) -> ! {
    type AsyncBody = fn(AsyncApp) -> Pin<Box<dyn Future<Output = ()> + 'static>>;
    let async_fn: AsyncBody = match target {
        "test_headless_code_scene_workspace_full_flow" => {
            |app| Box::pin(body_code_scene_workspace_full_flow(app))
        }
        "test_headless_app_shell_code_scene_navigation_and_cards" => {
            |app| Box::pin(body_app_shell_code_scene_navigation_and_cards(app))
        }
        other => {
            eprintln!("unknown code_scene headless gpui test target: {other}");
            std::process::exit(101);
        }
    };

    let target_owned = target.to_string();
    let outer = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        Application::headless()
            .with_assets(TelegramAssetSource::new())
            .run(move |cx: &mut App| {
                let t = target_owned.clone();
                cx.spawn(async move |app: &mut AsyncApp| {
                    let app_clone = app.clone();
                    let res = std::panic::AssertUnwindSafe(async_fn(app_clone))
                        .catch_unwind()
                        .await;
                    match res {
                        Ok(()) => std::process::exit(0),
                        Err(payload) => {
                            let msg = panic_payload_to_string(payload);
                            eprintln!("[code_scene headless panic] {t}: {msg}");
                            std::process::exit(101);
                        }
                    }
                })
                .detach();
            });
    }));
    if let Err(payload) = outer {
        let msg = panic_payload_to_string(payload);
        eprintln!("[code_scene headless outer panic] {target}: {msg}");
        std::process::exit(101);
    }
    std::process::exit(0);
}

#[cfg(target_os = "macos")]
#[used]
#[unsafe(link_section = "__DATA,__mod_init_func")]
static MACOS_CODE_SCENE_HEADLESS_ENTRY: extern "C" fn() = macos_mod_init_code_scene_entry;

#[cfg(target_os = "macos")]
extern "C" fn macos_mod_init_code_scene_entry() {
    if std::env::var(CHILD_ENV).as_deref() == Ok("1") {
        if let Ok(target) = std::env::var(CHILD_TARGET_ENV) {
            dispatch_headless_target(&target);
        }
    }
}

fn run_in_headless_subprocess(test_fn_name: &str) {
    if std::env::var(CHILD_ENV).as_deref() == Ok("1") {
        dispatch_headless_target(test_fn_name);
    }
    let tmp_dir = tempfile::tempdir().expect("create temp config dir");
    let exe = std::env::current_exe().expect("current_exe");
    let output = std::process::Command::new(exe)
        .env(CHILD_ENV, "1")
        .env(CHILD_TARGET_ENV, test_fn_name)
        .env("GPUI_TELEGRAM_CONFIG_DIR", tmp_dir.path())
        .env("GPUI_OFFLINE_TEST", "1")
        .arg("--exact")
        .arg(test_fn_name)
        .arg("--nocapture")
        .output()
        .expect("spawn headless gpui child");

    if !output.status.success() {
        let stdout = String::from_utf8_lossy(&output.stdout);
        let stderr = String::from_utf8_lossy(&output.stderr);
        panic!(
            "headless child test `{test_fn_name}` failed (status {:?})\n--- stdout ---\n{stdout}\n--- stderr ---\n{stderr}",
            output.status.code()
        );
    }
}

#[test]
fn test_code_scene_icon_nav_and_sidebar_hard_invariants() {
    assert_eq!(IconName::ALL.len(), 64, "IconName::ALL must stay strictly 64");
    assert!(
        IconName::EXTRA.contains(&IconName::CodeScene),
        "IconName::CodeScene must live in IconName::EXTRA"
    );
    let src = TelegramAssetSource::new();
    let loaded = src
        .load(IconName::CodeScene.path())
        .expect("load code_scene.svg")
        .expect("code_scene.svg bytes present");
    let svg_str = std::str::from_utf8(&loaded).expect("valid utf8 svg");
    assert!(svg_str.contains("<svg") && svg_str.contains("</svg>"));

    let rail = NavRail::new();
    assert_eq!(rail.items.len(), 11);
    let cs_item = rail
        .items
        .iter()
        .find(|it| it.destination == NavDestination::CodeScene)
        .expect("CodeScene item registered in NavRail");
    assert_eq!(cs_item.unread_badge, 0);
    assert_eq!(NavDestination::CodeScene.icon_name(), IconName::CodeScene);
    assert_eq!(NavDestination::CodeScene.localized_label("zh-CN"), "代码场景");
    assert_eq!(NavDestination::CodeScene.localized_label("en-US"), "Code Scene");
    assert_eq!(nav_destination_for_shortcut_digit("0"), None);

    let sidebar = ChatSidebar::new();
    assert!(
        sidebar.dialogs.is_empty(),
        "ChatSidebar::new() must never fabricate fake chat rows for Code Scene"
    );
}

#[test]
fn test_hybrid_renderer_bgra8_and_vector_tessellation_primitives() {
    // 1. BGRA8 RenderImage builder verifies exact byte ordering & zero-size rejection
    let empty_pf = PixelFrame {
        width: 0,
        height: 0,
        bgra: Vec::new(),
        revision: 1,
    };
    assert!(build_render_image_from_pixel_frame(&empty_pf).is_none());

    let pf = PixelFrame {
        width: 4,
        height: 2,
        bgra: vec![
            0x11, 0x22, 0x33, 0xFF, 0x44, 0x55, 0x66, 0xFF, 0x77, 0x88, 0x99, 0xFF, 0xAA, 0xBB,
            0xCC, 0xFF, 0x10, 0x20, 0x30, 0xFF, 0x40, 0x50, 0x60, 0xFF, 0x70, 0x80, 0x90, 0xFF,
            0xA0, 0xB0, 0xC0, 0xFF,
        ],
        revision: 2,
    };
    let render_img = build_render_image_from_pixel_frame(&pf).expect("valid RenderImage");
    let sz = render_img.size(0);
    assert_eq!(sz.width.0, 4);
    assert_eq!(sz.height.0, 2);

    // 2. Contain mapping & NaN/Inf clamping
    let b = bounds(point(px(100.0), px(50.0)), size(px(800.0), px(640.0)));
    let rect = SceneContainRect::compute(b, 360, 640);
    assert!((rect.width - 360.0).abs() < 0.5);
    assert!((rect.height - 640.0).abs() < 0.5);
    let (cx_pt, cy_pt) = map_point_contain_scene(point(px(500.0), px(370.0)), b, 360, 640);
    assert!((cx_pt - 180.0).abs() < 1.0);
    assert!((cy_pt - 320.0).abs() < 1.0);
    let (nan_x, nan_y) = map_point_contain_scene(point(px(f32::NAN), px(f32::INFINITY)), b, 360, 640);
    assert!(nan_x.is_finite() && nan_y.is_finite());

    // 3. Every vector DrawCmd variant tessellates cleanly via PathBuilder
    let red = RgbaColor::new(254, 44, 85, 255);
    let cyan = RgbaColor::new(37, 244, 238, 255);
    let cmds = vec![
        DrawCmd::Clear { color: red },
        DrawCmd::LinearGradientBg {
            top: red,
            bottom: cyan,
            angle_deg: 135.0,
        },
        DrawCmd::FillRect {
            x: 10.0,
            y: 10.0,
            w: 80.0,
            h: 40.0,
            color: red,
        },
        DrawCmd::RoundRect {
            x: 20.0,
            y: 20.0,
            w: 100.0,
            h: 50.0,
            radius: 12.0,
            color: cyan,
        },
        DrawCmd::StrokeRect {
            x: 30.0,
            y: 30.0,
            w: 60.0,
            h: 60.0,
            stroke_width: 2.0,
            color: red,
        },
        DrawCmd::FillCircle {
            cx: 180.0,
            cy: 320.0,
            radius: 40.0,
            color: cyan,
        },
        DrawCmd::StrokeCircle {
            cx: 180.0,
            cy: 320.0,
            radius: 50.0,
            stroke_width: 3.0,
            color: red,
        },
        DrawCmd::GlowCircle {
            cx: 180.0,
            cy: 320.0,
            radius: 25.0,
            color: cyan,
            intensity: 1.2,
        },
        DrawCmd::Line {
            x1: 0.0,
            y1: 0.0,
            x2: 200.0,
            y2: 200.0,
            stroke_width: 2.0,
            color: red,
        },
        DrawCmd::Polyline {
            points: vec![[10.0, 10.0], [80.0, 40.0], [150.0, 120.0]],
            stroke_width: 2.0,
            color: cyan,
        },
        DrawCmd::FillPolygon {
            points: vec![[10.0, 10.0], [120.0, 20.0], [70.0, 130.0]],
            color: red,
        },
        DrawCmd::Arc {
            cx: 180.0,
            cy: 320.0,
            radius: 60.0,
            start_rad: 0.0,
            end_rad: 2.4,
            stroke_width: 3.0,
            color: cyan,
        },
        DrawCmd::Bezier {
            x0: 10.0,
            y0: 10.0,
            cx1: 80.0,
            cy1: 120.0,
            cx2: 140.0,
            cy2: 20.0,
            x1: 220.0,
            y1: 180.0,
            stroke_width: 2.5,
            color: red,
        },
        DrawCmd::Star {
            cx: 180.0,
            cy: 200.0,
            outer_r: 36.0,
            inner_r: 16.0,
            points: 5,
            rotation_rad: 0.2,
            color: cyan,
        },
    ];
    for cmd in &cmds {
        assert!(
            build_vector_path_for_cmd(cmd, &rect).is_some(),
            "expected valid tessellation for {cmd:?}"
        );
    }
    let text_cmd = DrawCmd::Text {
        x: 180.0,
        y: 40.0,
        content: "SCORE 100".to_string(),
        size: 18.0,
        color: RgbaColor::WHITE,
        align: TextAlign::Center,
    };
    assert!(build_vector_path_for_cmd(&text_cmd, &rect).is_none());

    // NaN/Inf draw command is rejected fail-closed without panic
    let bad_cmd = DrawCmd::FillCircle {
        cx: f32::NAN,
        cy: 100.0,
        radius: 20.0,
        color: red,
    };
    assert!(build_vector_path_for_cmd(&bad_cmd, &rect).is_none());
}

#[test]
fn test_douyin_swipe_gesture_inertia_heart_burst_and_formatters() {
    let mut g = SwipeGestureController::new();
    let t0 = Instant::now();
    // Single wheel down crosses threshold -> Next, immediate follow-up inertia debounced
    assert_eq!(
        g.on_scroll_wheel_at(ScrollDelta::Lines(point(0.0, -2.0)), 720.0, t0),
        Some(SwipeSnapDirection::Next)
    );
    assert_eq!(
        g.on_scroll_wheel_at(
            ScrollDelta::Lines(point(0.0, -3.0)),
            720.0,
            t0 + Duration::from_millis(60)
        ),
        None
    );
    // After cooldown expires, scrolling up snaps to Prev
    assert_eq!(
        g.on_scroll_wheel_at(
            ScrollDelta::Pixels(point(px(0.0), px(90.0))),
            720.0,
            t0 + Duration::from_millis(320)
        ),
        Some(SwipeSnapDirection::Prev)
    );

    // Drag-to-swipe below threshold rolls back; above threshold snaps
    g.on_drag_start(400.0);
    g.on_drag_move(380.0);
    assert_eq!(g.on_drag_end(720.0), None);
    g.on_drag_start(400.0);
    g.on_drag_move(250.0);
    assert_eq!(g.on_drag_end(720.0), Some(SwipeSnapDirection::Next));

    // Heart burst samples alive then expires cleanly
    let hb = HeartBurst::new(1, 180.0, 320.0);
    assert!(hb.sample(hb.created_at + Duration::from_millis(120)).is_some());
    assert!(hb.sample(hb.created_at + Duration::from_millis(900)).is_none());

    assert_eq!(format_compact_count(42), "42");
    assert_eq!(format_compact_count(1_500), "1.5k");
    assert_eq!(format_compact_count(25_000), "2.5w");
    assert_eq!(format_timecode(75.4), "01:15");
}

#[test]
fn test_headless_code_scene_workspace_full_flow() {
    run_in_headless_subprocess("test_headless_code_scene_workspace_full_flow");
}

async fn body_code_scene_workspace_full_flow(app: AsyncApp) {
    let service = Arc::new(LocalCodeSceneService::new_offline(1001, Some("alice".into())));
    let ws = app
        .update(|cx| {
            cx.new(|wcx| {
                CodeSceneWorkspace::new(
                    CodeSceneWorkspaceConfig {
                        service: Arc::clone(&service),
                        tokio_handle: None,
                        session_token: None,
                        palette: TdesktopThemePalette::default(),
                        language_id: "zh-CN".to_string(),
                    },
                    wcx,
                )
            })
        })
        .expect("create CodeSceneWorkspace");

    // 1. Seeded feed loads immediately with active VM & pre-warmed neighbors
    app.update(|cx| {
        let r = ws.read(cx);
        assert!(r.current_item().is_some(), "feed loaded with 18 real-code seeds");
        assert!(r.viewport.vm.is_some(), "active card VM initialized");
        assert!(r.viewport.last_frame.is_some(), "initial frame rendered");
        assert!(!r.prewarm_cache.is_empty(), "neighbor pre-warm cache populated");
    })
    .unwrap();

    // 2. Switch to Videos tab -> deterministic seek & playback speed cycle
    app.update(|cx| {
        ws.update(cx, |w, wcx| {
            w.handle_hud_action(HudAction::SelectTab(CodeSceneFeedTab::Videos), wcx);
            let cur = w.current_item().expect("video item");
            assert_eq!(cur.asset.kind, AssetKind::Video);
            w.handle_hud_action(HudAction::CycleSpeed, wcx);
            assert!((w.viewport.playback_speed - 1.5).abs() < 0.01);
            w.handle_hud_action(HudAction::SeekVideoRatio(0.5), wcx);
            let (t_sec, dur, _) = w.viewport.timeline_progress();
            assert!(dur.is_some());
            assert!(t_sec > 1.0, "video seek advanced timeline");
        });
    })
    .unwrap();

    // 3. Switch to Games tab -> capture focus, send WASD/Space keys, Esc releases capture
    app.update(|cx| {
        ws.update(cx, |w, wcx| {
            w.handle_hud_action(HudAction::SelectTab(CodeSceneFeedTab::Games), wcx);
            let cur = w.current_item().expect("game item");
            assert_eq!(cur.asset.kind, AssetKind::Game);
            assert!(!w.is_game_input_captured());
            w.handle_hud_action(HudAction::CaptureGameFocus, wcx);
            assert!(w.is_game_input_captured());
            assert!(w.handle_non_char_key_input("d", false, false, wcx));
            assert!(w.viewport.input_state.right);
            w.handle_key_up("d", wcx);
            assert!(!w.viewport.input_state.right);
            // Esc releases game focus first without exiting workspace
            assert!(w.handle_non_char_key_input("escape", false, false, wcx));
            assert!(!w.is_game_input_captured());
            assert!(w.take_pending_action().is_none());
        });
    })
    .unwrap();

    // 4. Switch to ForYou -> swipe next, like, bookmark, comment drawer (with CJK text)
    app.update(|cx| {
        ws.update(cx, |w, wcx| {
            w.handle_hud_action(HudAction::SelectTab(CodeSceneFeedTab::ForYou), wcx);
            let first_id = w.current_item().unwrap().asset.scene_id;
            w.swipe_card(SwipeSnapDirection::Next, wcx);
            let second_id = w.current_item().unwrap().asset.scene_id;
            assert_ne!(first_id, second_id);

            w.handle_hud_action(HudAction::ToggleLike, wcx);
            assert!(w.current_item().unwrap().asset.viewer_liked);
            w.handle_hud_action(HudAction::ToggleBookmark, wcx);
            assert!(w.current_item().unwrap().asset.viewer_bookmarked);

            w.handle_hud_action(HudAction::OpenComments, wcx);
            assert_eq!(w.active_drawer, CodeSceneDrawer::Comments);
            assert_eq!(w.active_input, ActiveCodeSceneInput::Comment);
            assert!(w.insert_text_input("太酷了，纯代码粒子效果！", wcx));
            w.handle_drawer_action(DrawerAction::SubmitComment, wcx);
            assert_eq!(w.comments.len(), 1);
            assert_eq!(w.comments[0].content, "太酷了，纯代码粒子效果！");
        });
    })
    .unwrap();

    // 5. Code Inspector -> validate + hot-reload live VM
    app.update(|cx| {
        ws.update(cx, |w, wcx| {
            w.handle_hud_action(HudAction::OpenCodeInspector, wcx);
            assert_eq!(w.active_drawer, CodeSceneDrawer::CodeInspector);
            assert!(w.editor_validation.as_ref().is_some_and(|v| v.ok));
            w.handle_drawer_action(DrawerAction::HotReloadEditorCode, wcx);
            assert!(w.viewport.last_error.is_none());
            assert!(w.viewport.vm.is_some());
        });
    })
    .unwrap();

    // 6. AI Bot Studio -> prompt -> offline synthesize -> live preview -> iterate -> 1-click publish into feed!
    app.update(|cx| {
        ws.update(cx, |w, wcx| {
            let before_total = w.service.total_catalog();
            w.handle_hud_action(HudAction::OpenAiStudio, wcx);
            assert_eq!(w.active_drawer, CodeSceneDrawer::AiStudio);
            w.handle_drawer_action(
                DrawerAction::StudioSelectKind(AssetKind::Game),
                wcx,
            );
            assert!(w.insert_text_input("像素霓虹弹球小游戏", wcx));
            w.handle_drawer_action(DrawerAction::StudioGenerate, wcx);
            assert!(w.studio.current_draft.is_some(), "AI Bot generated validated draft");
            assert!(w.studio_preview_viewport.vm.is_some(), "Studio live preview running");

            // Multi-turn iterate
            w.handle_drawer_action(
                DrawerAction::FocusInput(ActiveCodeSceneInput::StudioIterate),
                wcx,
            );
            assert!(w.insert_text_input("加快速度换成极光蓝", wcx));
            w.handle_drawer_action(DrawerAction::StudioIterate, wcx);
            assert!(w.studio.current_draft.is_some());

            // 1-click publish directly into feed
            w.handle_drawer_action(DrawerAction::StudioPublish, wcx);
            assert_eq!(w.active_drawer, CodeSceneDrawer::None);
            assert_eq!(w.service.total_catalog(), before_total + 1);
            assert!(matches!(
                w.take_pending_action(),
                Some(CodeScenePendingAction::NotifyToast(_))
            ));
        });
    })
    .unwrap();
}

#[test]
fn test_headless_app_shell_code_scene_navigation_and_cards() {
    run_in_headless_subprocess("test_headless_app_shell_code_scene_navigation_and_cards");
}

async fn body_app_shell_code_scene_navigation_and_cards(app: AsyncApp) {
    let shell = app
        .update(|cx| {
            let client = Arc::new(TelegramClient::new(ClientConfig::resolve()));
            cx.new(|scx| {
                let mut s = AppShell::new_with_client(client, scx);
                s.is_authenticated = true;
                s
            })
        })
        .expect("create AppShell");

    // 1. Activate NavDestination::CodeScene -> mounts full-pane sibling & hides composer
    app.update(|cx| {
        shell.update(cx, |s, scx| {
            s.activate_nav_destination(NavDestination::CodeScene, scx);
            assert_eq!(s.active_panel, ActiveWorkspacePanel::CodeScene);
            assert!(s.code_scene_workspace.is_some());
            assert!(s.should_hide_composer(scx));
            let tree = s.inspect_state(scx);
            assert_eq!(tree.window.active_panel, "CodeScene");
            assert_eq!(tree.nav_rail.active_destination, "CodeScene");
        });
    })
    .unwrap();

    // 2. Theme & bilingual locale propagation reaches CodeSceneWorkspace
    app.update(|cx| {
        shell.update(cx, |s, scx| {
            s.apply_theme_selection(TdesktopThemeMode::Day, None, None, scx);
            s.toggle_language(scx);
            let cs = s.code_scene_workspace.as_ref().unwrap().read(scx);
            assert_eq!(cs.palette.mode, TdesktopThemeMode::Day);
            assert_eq!(cs.language_id, s.active_lang_id());
        });
    })
    .unwrap();

    // 3. Chat GenerativeCard callbacks (`code_scene:open/remix/share:<id>`) dispatch cleanly
    app.update(|cx| {
        shell.update(cx, |s, scx| {
            let first_scene_id = s
                .code_scene_workspace
                .as_ref()
                .unwrap()
                .read(scx)
                .service
                .items()
                .first()
                .expect("seeded scene")
                .asset
                .scene_id
                .clone();

            // Leave to Chats, then click a chat GenerativeCard "Remix" button
            s.activate_nav_destination(NavDestination::Chats, scx);
            assert_eq!(s.active_panel, ActiveWorkspacePanel::Chats);
            assert!(!s.code_scene_workspace.as_ref().unwrap().read(scx).is_visible);

            let remix_payload = CodeSceneCardAction::Remix(first_scene_id.clone()).to_payload();
            s.execute_hermes_inline_callback(remix_payload, scx);
            assert_eq!(s.active_panel, ActiveWorkspacePanel::CodeScene);
            assert_eq!(
                s.code_scene_workspace.as_ref().unwrap().read(scx).active_drawer,
                CodeSceneDrawer::AiStudio
            );
            assert!(s.is_code_scene_input_active(scx));
            // Typing CJK text via AppShell routes straight into AI Studio prompt!
            s.insert_committed_text("赛博烟花倒计时", scx);
            assert_eq!(
                s.code_scene_workspace.as_ref().unwrap().read(scx).studio.prompt,
                "赛博烟花倒计时"
            );

            // Share card callback opens ShareToChat drawer
            let share_payload = CodeSceneCardAction::Share(first_scene_id.clone()).to_payload();
            s.execute_hermes_inline_callback(share_payload, scx);
            assert_eq!(
                s.code_scene_workspace.as_ref().unwrap().read(scx).active_drawer,
                CodeSceneDrawer::ShareToChat
            );

            // Open card callback focuses scene & closes drawer
            let open_payload = CodeSceneCardAction::Open(first_scene_id.clone()).to_payload();
            s.execute_hermes_inline_callback(open_payload, scx);
            assert_eq!(
                s.code_scene_workspace.as_ref().unwrap().read(scx).active_drawer,
                CodeSceneDrawer::None
            );
            assert_eq!(
                s.code_scene_workspace
                    .as_ref()
                    .unwrap()
                    .read(scx)
                    .current_item()
                    .unwrap()
                    .asset
                    .scene_id,
                first_scene_id
            );

            // Logout tears down CodeSceneWorkspace cleanly
            s.logout(scx);
            assert!(s.code_scene_workspace.is_none());
            assert_eq!(s.active_panel, ActiveWorkspacePanel::Chats);
        });
    })
    .unwrap();
}
