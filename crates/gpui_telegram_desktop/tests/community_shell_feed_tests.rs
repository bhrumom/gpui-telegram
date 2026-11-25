//! M4 Headless GPUI Integration Tests for F15–F16:
//! Desktop Left-Sidebar Entry Points (All 6), `AppShell` Hosting, Theme/Locale/Teardown,
//! Centralized English + CJK IME (80ms trailing-Enter fence + 3-step Escape),
//! 3/2/1-Column Adaptive Community Workspace, Human/Bot Identity Switcher, Multi-Tab Feed,
//! Algo Tuner, Fused PostCard Interactions, Realtime WS Events & Cross-App Telegram Bridges.

use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::time::{Duration, Instant};

use futures::FutureExt;
use gpui::{
    bounds, point, px, size, App, AppContext, Application, AsyncApp, Entity, Keystroke,
    WindowBounds, WindowOptions,
};
use gpui_telegram_client::{AuthResponse, ClientConfig, ClientDomainEvent, TelegramClient};
use gpui_telegram_core::types::ChatId;
use gpui_telegram_desktop::theme::{IconName, TelegramAssetSource};
use gpui_telegram_desktop::views::boxes::modal_types::{ModalActionOutcome, ModalDialogKind};
use gpui_telegram_desktop::views::chat_sidebar::{
    AddContactKind, DialogDeliveryStatus, DialogItemModel,
};
use gpui_telegram_desktop::views::community::post_card::compact_post_preview_text;
use gpui_telegram_desktop::views::community::shared_widgets::{
    format_badge_count, linkify_community_slugs,
};
use gpui_telegram_desktop::views::community::{
    sanitize_algo_prefs, sanitize_unit_f64, ActiveCommunityInput, CommunityLayoutMode,
    CommunityPopover, CommunityRoute, CommunityWorkspaceAction, CommunityWorkspaceState,
    FeedDensity, PollBreakdownMode,
};
use gpui_telegram_desktop::views::canonical_composer::BotSlashCommandItem;
use gpui_telegram_desktop::views::nav_rail::{NavDestination, NavFolderItem, NavRail};
use gpui_telegram_desktop::views::shell::{
    nav_destination_for_shortcut_digit, ActiveWorkspacePanel, AppShell,
};
use gpui_telegram_desktop::views::sidebar::gemini_unified_sidebar::UnifiedSidebarAction;
use gpui_telegram_desktop::views::stage::entities_render::RichTextAction;
use gpui_telegram_desktop::views::apple_sidebar::AppleSidebarSection;
use gpui_telegram_desktop::views::content_hub::{ContentHubInputTarget, ContentHubSection};
use gpui_telegram_desktop::views::{MainMenuCommand, TdesktopThemeMode};
use gpui_telegram_proto::community_contracts::{
    ActorFilter, ActorKind, ActorProfileResponse, AlgoPreferencesDto, AlgoPreset,
    CommunityActorDto, CommunityPostDto, CommunitySearchResultDto, CreatePollInputDto,
    CreatePostRequest, EmbeddedPostSummaryDto, EngagementSignalKind, FeedExplanationDto,
    FeedItemDto, FeedSort, FeedTab, LinkPreviewDto, PostFormat, PostMediaAttachmentDto, PostStatus,
    SignalContributionDto, TimeWindow,
};
use gpui_telegram_proto::sync::UpdateEvent;
use tempfile::TempDir;

const CHILD_ENV: &str = "GPUI_COMMUNITY_M4_CHILD";
const CHILD_TARGET_ENV: &str = "GPUI_COMMUNITY_M4_TARGET";
#[cfg(target_os = "macos")]
const CHILD_AFTER_CTORS_ENV: &str = "GPUI_COMMUNITY_M4_AFTER_CTORS";

fn exit_headless_child(code: i32, use_underscore_exit: bool) -> ! {
    use std::io::Write as _;
    let _ = std::io::stdout().flush();
    let _ = std::io::stderr().flush();
    if use_underscore_exit {
        #[cfg(unix)]
        unsafe {
            unsafe extern "C" {
                fn _exit(status: i32) -> !;
            }
            _exit(code);
        }
    }
    std::process::exit(code);
}

fn panic_payload_to_string(payload: Box<dyn std::any::Any + Send>) -> String {
    if let Some(s) = payload.downcast_ref::<&str>() {
        (*s).to_string()
    } else if let Some(s) = payload.downcast_ref::<String>() {
        s.clone()
    } else {
        "panic".to_string()
    }
}

fn dispatch_headless_target(target: &str, use_underscore_exit: bool) -> ! {
    type AsyncBody = fn(AsyncApp) -> Pin<Box<dyn Future<Output = ()> + 'static>>;
    let async_fn: AsyncBody = match target {
        "test_community_six_entry_points_composer_hide_and_chat_return" => {
            |app| Box::pin(body_six_entry_points_composer_hide_and_chat_return(app))
        }
        "test_community_responsive_layouts_sidebar_toggle_theme_locale_and_teardown" => {
            |app| Box::pin(body_responsive_layouts_sidebar_toggle_theme_locale_and_teardown(app))
        }
        "test_community_centralized_cjk_ime_80ms_fence_multiline_and_3step_escape" => {
            |app| Box::pin(body_centralized_cjk_ime_80ms_fence_multiline_and_3step_escape(app))
        }
        "test_community_identity_switcher_0bot_fallback_and_bot_posting" => {
            |app| Box::pin(body_identity_switcher_0bot_fallback_and_bot_posting(app))
        }
        "test_community_feed_controls_algo_tuner_post_interactions_polls_and_signals" => {
            |app| Box::pin(body_feed_controls_algo_tuner_post_interactions_polls_and_signals(app))
        }
        "test_community_ws_events_unmounted_badge_malformed_json_and_cross_app_bridges" => {
            |app| Box::pin(body_ws_events_unmounted_badge_malformed_json_and_cross_app_bridges(app))
        }
        "test_community_secondary_routes_live_data_and_owned_bot_sync" => {
            |app| Box::pin(body_secondary_routes_live_data_and_owned_bot_sync(app))
        }
        "test_community_f15_remediation_regressions" => {
            |app| Box::pin(body_f15_remediation_regressions(app))
        }
        "test_community_f16_remediation_regressions" => {
            |app| Box::pin(body_f16_remediation_regressions(app))
        }
        "test_community_r3_second_order_f15_regressions" => {
            |app| Box::pin(body_r3_second_order_f15_regressions(app))
        }
        "test_community_r4_bidirectional_focus_and_compose_cleanup" => {
            |app| Box::pin(body_r4_bidirectional_focus_and_compose_cleanup(app))
        }
        "test_community_r5_shell_occlusion_compose_modal_and_remote_hub_cleanup" => {
            |app| Box::pin(body_r5_shell_occlusion_compose_modal_and_remote_hub_cleanup(app))
        }
        "test_community_r6_theme_plus_focus_search_unfocused_ime_switch_and_talkmode" => {
            |app| {
                Box::pin(
                    body_r6_theme_plus_focus_search_unfocused_ime_switch_and_talkmode(app),
                )
            }
        }
        "test_community_r7_composer_ime_carryover_nav_sync_and_load_more_supersede" => {
            |app| {
                Box::pin(
                    body_r7_composer_ime_carryover_nav_sync_and_load_more_supersede(app),
                )
            }
        }
        "test_community_apple_sidebar_content_hub_coordination" => {
            |app| Box::pin(body_apple_sidebar_content_hub_coordination(app))
        }
        other => {
            eprintln!("unknown community M4 headless gpui test target: {other}");
            exit_headless_child(101, use_underscore_exit);
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
                        Ok(()) => exit_headless_child(0, use_underscore_exit),
                        Err(payload) => {
                            let msg = panic_payload_to_string(payload);
                            eprintln!("[community headless child panic] {t}: {msg}");
                            exit_headless_child(101, use_underscore_exit);
                        }
                    }
                })
                .detach();
            });
    }));
    if let Err(payload) = outer {
        let msg = panic_payload_to_string(payload);
        eprintln!("[community headless outer panic] {target}: {msg}");
        exit_headless_child(101, use_underscore_exit);
    }
    eprintln!(
        "[community headless error] {target}: Application::headless().run returned before async test body completed"
    );
    exit_headless_child(102, use_underscore_exit);
}

#[cfg(target_os = "macos")]
#[used]
#[unsafe(link_section = "__DATA,__mod_init_func")]
static MACOS_HEADLESS_MAIN_THREAD_ENTRY: extern "C" fn() = macos_mod_init_headless_entry;

#[cfg(target_os = "macos")]
extern "C" fn macos_atexit_headless_entry() {
    if let Ok(target) = std::env::var(CHILD_TARGET_ENV) {
        dispatch_headless_target(&target, true);
    }
}

#[cfg(target_os = "macos")]
extern "C" fn macos_mod_init_headless_entry() {
    if std::env::var(CHILD_ENV).as_deref() == Ok("1") {
        if std::env::var(CHILD_AFTER_CTORS_ENV).as_deref() == Ok("1") {
            unsafe extern "C" {
                fn atexit(func: extern "C" fn()) -> i32;
            }
            unsafe {
                atexit(macos_atexit_headless_entry);
            }
            return;
        }
        if let Ok(target) = std::env::var(CHILD_TARGET_ENV) {
            dispatch_headless_target(&target, false);
        }
    }
}

fn run_in_headless_subprocess(test_fn_name: &str) {
    if std::env::var(CHILD_ENV).as_deref() == Ok("1") {
        dispatch_headless_target(test_fn_name, false);
    }
    let cfg_dir = TempDir::new().expect("create TempDir for GPUI_TELEGRAM_CONFIG_DIR");
    let exe = std::env::current_exe().expect("current_exe");
    let mut cmd = std::process::Command::new(exe);
    cmd.env(CHILD_ENV, "1")
        .env(CHILD_TARGET_ENV, test_fn_name)
        .env("GPUI_TELEGRAM_CONFIG_DIR", cfg_dir.path())
        .env("GPUI_OFFLINE_TEST", "1");
    #[cfg(target_os = "macos")]
    {
        cmd.env(CHILD_AFTER_CTORS_ENV, "1")
            .arg("--exact")
            .arg("__community_m4_headless_child_noop__");
    }
    #[cfg(not(target_os = "macos"))]
    {
        cmd.arg("--exact").arg(test_fn_name);
    }
    let output = cmd
        .arg("--nocapture")
        .output()
        .expect("spawn community headless gpui child");
    if !output.status.success() {
        let stdout = String::from_utf8_lossy(&output.stdout);
        let stderr = String::from_utf8_lossy(&output.stderr);
        panic!(
            "community headless test `{test_fn_name}` failed (status {:?})\n--- stdout ---\n{stdout}\n--- stderr ---\n{stderr}",
            output.status.code()
        );
    }
}

fn test_auth(user_id: u64, username: &str) -> AuthResponse {
    AuthResponse {
        user_id,
        username: username.to_string(),
        session_id: format!("sess-{user_id}"),
        token: format!("test-token-{user_id}"),
        first_name: Some(username.to_string()),
        created_at: None,
        expires_at: None,
    }
}

struct HeadlessShellHost {
    _dir: TempDir,
    client: Arc<TelegramClient>,
    win: Option<gpui::WindowHandle<AppShell>>,
    shell: Entity<AppShell>,
    width: f32,
    height: f32,
}

impl HeadlessShellHost {
    fn draw(&self, app: &mut AsyncApp) {
        if let Some(win) = self.win {
            let _ = app.update_window(win.into(), |_, window, cx| {
                let _ = window.draw(cx);
            });
        } else {
            let w = self.width;
            let h = self.height;
            let _ = self.shell.update(app, |s, cx| {
                s.app.sync_viewport_size(w, h);
                let sb_w = s.sidebar.read(cx).effective_width();
                if let Some(cw) = s.community_workspace.clone() {
                    cw.update(cx, |cw, wcx| {
                        cw.set_host_sidebar_width(sb_w);
                        let _el = cw.render_for_viewport_width(w, wcx);
                    });
                }
            });
        }
    }

    fn key(&self, app: &mut AsyncApp, spec: &str) {
        if let Some(win) = self.win {
            let _ = app.update_window(win.into(), |_, window, cx| {
                let _ = window.dispatch_keystroke(Keystroke::parse(spec).unwrap(), cx);
            });
            return;
        }
        let _ = self.shell.update(app, |s, cx| match spec {
            "escape" => {
                let _ = s.handle_non_char_key("escape", false, cx);
            }
            "cmd-shift-c" => {
                let _ = s.handle_shortcut_key("c", true, cx);
            }
            "cmd-c" => {
                let _ = s.handle_shortcut_key("c", false, cx);
            }
            "cmd-b" => {
                let _ = s.handle_shortcut_key("b", false, cx);
            }
            "cmd-d" => {
                let _ = s.handle_shortcut_key("d", false, cx);
            }
            "cmd-l" => {
                let _ = s.handle_shortcut_key("l", false, cx);
            }
            other => panic!("unsupported headless key spec: {other}"),
        });
    }
}

fn open_authenticated_shell(
    app: &mut AsyncApp,
    width: f32,
    height: f32,
    user_id: u64,
    username: &str,
) -> HeadlessShellHost {
    let dir = TempDir::new().expect("create community test tempdir");
    let cfg = ClientConfig {
        server_url: "http://127.0.0.1:9".to_string(),
        max_reconnect_attempts: 0,
        ..Default::default()
    };
    let client = Arc::new(
        TelegramClient::open_with_dir(cfg, dir.path()).expect("open isolated TelegramClient"),
    );
    client.community().set_force_local(true);
    let auth = test_auth(user_id, username);
    client.set_auth_session(auth.clone());

    let client_for_win = Arc::clone(&client);
    let auth_for_win = auth.clone();
    let (win, shell) = app
        .update(|cx| {
            let _rt_guard = client_for_win.runtime().enter();
            let skip_macos_ci_window = cfg!(target_os = "macos")
                && (std::env::var("CI").as_deref() == Ok("true")
                    || std::env::var("GITHUB_ACTIONS").as_deref() == Ok("true")
                    || std::env::var("GPUI_HEADLESS_NO_WINDOW").as_deref() == Ok("1"));
            if skip_macos_ci_window {
                // GitHub Actions macOS Apple Silicon VMs lack a Metal GPU device, causing
                // `gpui::MacWindow::open` -> `MetalRenderer::new` to abort with `exit(1)`.
                // Construct the real `Entity<AppShell>` on `Application::headless()` directly.
                let ent = cx.new(|cx| {
                    let mut shell = AppShell::new_with_client(client_for_win.clone(), cx);
                    shell.apply_authenticated_session(auth_for_win.clone(), cx);
                    shell
                });
                (None, ent)
            } else {
                let opened = cx.open_window(
                    WindowOptions {
                        window_bounds: Some(WindowBounds::Windowed(bounds(
                            point(px(0.0), px(0.0)),
                            size(px(width), px(height)),
                        ))),
                        focus: true,
                        show: false,
                        ..Default::default()
                    },
                    |_window, cx| {
                        cx.new(|cx| {
                            let mut shell = AppShell::new_with_client(client_for_win.clone(), cx);
                            shell.apply_authenticated_session(auth_for_win.clone(), cx);
                            shell
                        })
                    },
                );
                match opened {
                    Ok(w) => {
                        let ent = w.entity(cx).expect("AppShell entity from window");
                        (Some(w), ent)
                    }
                    Err(err)
                        if cfg!(target_os = "linux")
                            && err.to_string().contains("headless mode") =>
                    {
                        // GPUI 0.2.2's Linux `Application::headless()` uses `HeadlessClient`,
                        // whose `open_window` unconditionally returns this error; on local macOS
                        // it opens a real offscreen window and MUST succeed.
                        let ent = cx.new(|cx| {
                            let mut shell = AppShell::new_with_client(client_for_win.clone(), cx);
                            shell.apply_authenticated_session(auth_for_win.clone(), cx);
                            shell
                        });
                        (None, ent)
                    }
                    Err(err) => {
                        panic!("open headless GPUI window failed unexpectedly: {err:#}");
                    }
                }
            }
        })
        .expect("app update open_authenticated_shell");

    HeadlessShellHost {
        _dir: dir,
        client,
        win,
        shell,
        width,
        height,
    }
}

async fn pump_until(
    app: &mut AsyncApp,
    timeout: Duration,
    mut pred: impl FnMut(&mut App) -> bool,
) -> bool {
    let deadline = Instant::now() + timeout;
    while Instant::now() < deadline {
        if app.update(|cx| pred(cx)).unwrap_or(false) {
            return true;
        }
        app.background_executor()
            .timer(Duration::from_millis(15))
            .await;
    }
    app.update(|cx| pred(cx)).unwrap_or(false)
}

// ============================================================================
// Pure Unit Tests (Fast, No Subprocess Required)
// ============================================================================

#[test]
fn test_community_icons_extra_invariant_and_svg_load() {
    assert_eq!(IconName::ALL.len(), 64, "IconName::ALL MUST remain 64");
    let expected = [
        IconName::CommunityGlobe,
        IconName::UpvoteArrow,
        IconName::DownvoteArrow,
        IconName::Repost,
        IconName::Bookmark,
        IconName::FlameTrending,
        IconName::ShieldMod,
        IconName::SparkleAlgo,
        IconName::CommunityAward,
        IconName::CommunityThread,
    ];
    let source = TelegramAssetSource::new();
    for icon in expected {
        assert!(
            !IconName::ALL.contains(&icon),
            "{icon:?} MUST NOT be in IconName::ALL"
        );
        assert!(
            IconName::EXTRA.contains(&icon),
            "{icon:?} MUST be in IconName::EXTRA"
        );
        let bytes = gpui::AssetSource::load(&source, icon.path())
            .expect("asset source load")
            .unwrap_or_else(|| panic!("missing SVG bytes for {}", icon.path()));
        let svg_text = std::str::from_utf8(&bytes).expect("valid utf8 svg");
        assert!(svg_text.contains("<svg"), "invalid svg for {}", icon.path());
        assert!(
            svg_text.contains("viewBox=\"0 0 24 24\""),
            "expected 24x24 viewBox for {}",
            icon.path()
        );
        assert!(
            svg_text.contains("currentColor"),
            "expected currentColor in {}",
            icon.path()
        );
    }
}

#[test]
fn test_nav_rail_community_entry_badges_and_shortcut_digits() {
    let mut rail = NavRail::new();
    assert_eq!(rail.items.len(), 11);
    let comm_item = rail
        .items
        .iter()
        .find(|i| i.destination == NavDestination::Community)
        .expect("NavDestination::Community item present");
    assert_eq!(comm_item.unread_badge, 0);
    assert_eq!(NavDestination::Community.icon_name(), IconName::CommunityGlobe);
    assert_eq!(NavDestination::Community.localized_label("zh-CN"), "社区");
    assert_eq!(NavDestination::Community.localized_label("en"), "Community");

    rail.set_unread(NavDestination::Community, 42);
    assert_eq!(
        rail.items
            .iter()
            .find(|i| i.destination == NavDestination::Community)
            .unwrap()
            .unread_badge,
        42
    );
    rail.set_unread(NavDestination::Community, 1200);
    assert_eq!(format_badge_count(1200), "999+");
    assert_eq!(format_badge_count(42), "42");

    // F15-D4: NavRail::unread_total() MUST saturate at u32::MAX without overflow panic
    rail.set_unread(NavDestination::Chats, 1);
    rail.set_unread(NavDestination::Community, u32::MAX);
    assert_eq!(rail.unread_total(), u32::MAX);

    // Cmd+1..9 and "0" => None MUST remain intact!
    assert_eq!(nav_destination_for_shortcut_digit("1"), Some(NavDestination::Chats));
    assert_eq!(nav_destination_for_shortcut_digit("9"), Some(NavDestination::Browser));
    assert_eq!(nav_destination_for_shortcut_digit("0"), None);

    // Responsive layout resolution across all 10 AppShell & standalone breakpoints (C2-D5)
    assert_eq!(CommunityLayoutMode::resolve(1440.0, 288.0), CommunityLayoutMode::ThreeColumn);
    assert_eq!(CommunityLayoutMode::resolve(1440.0, 64.0), CommunityLayoutMode::ThreeColumn);
    assert_eq!(CommunityLayoutMode::resolve(1040.0, 288.0), CommunityLayoutMode::TwoColumn);
    assert_eq!(CommunityLayoutMode::resolve(1040.0, 64.0), CommunityLayoutMode::TwoColumn);
    assert_eq!(CommunityLayoutMode::resolve(820.0, 288.0), CommunityLayoutMode::OneColumn);
    assert_eq!(CommunityLayoutMode::resolve(820.0, 64.0), CommunityLayoutMode::OneColumn);
    assert_eq!(CommunityLayoutMode::resolve(640.0, 288.0), CommunityLayoutMode::OneColumn);
    assert_eq!(CommunityLayoutMode::resolve(1080.0, 0.0), CommunityLayoutMode::ThreeColumn);
    assert_eq!(CommunityLayoutMode::resolve(780.0, 0.0), CommunityLayoutMode::TwoColumn);
    assert_eq!(CommunityLayoutMode::resolve(779.0, 0.0), CommunityLayoutMode::OneColumn);

    // c/<slug> linkifier ignores inline/fenced code spans
    let linked = linkify_community_slugs("Visit c/rust_dev and `c/raw_code` plus c/ai-agents!");
    assert!(linked.contains("[c/rust_dev](tg://community/rust_dev)"));
    assert!(linked.contains("`c/raw_code`"));
    assert!(linked.contains("[c/ai-agents](tg://community/ai-agents)"));
}

#[test]
fn test_community_linkify_slugs_commonmark_urls_and_boundaries() {
    // R2-BLOCKER-5 / C2-D1: CommonMark double-backtick, tilde & 4-backtick fences
    assert_eq!(linkify_community_slugs("``c/rust``"), "``c/rust``");
    assert_eq!(linkify_community_slugs("`` `c/rust` ``"), "`` `c/rust` ``");
    assert_eq!(
        linkify_community_slugs("~~~rust\nc/rust\n~~~\nc/ai"),
        "~~~rust\nc/rust\n~~~\n[c/ai](tg://community/ai)"
    );
    assert_eq!(
        linkify_community_slugs("````md\nc/rust\n````\nc/rust"),
        "````md\nc/rust\n````\n[c/rust](tg://community/rust)"
    );

    // Existing Markdown links & bare URLs MUST NOT be corrupted or double-linkified
    assert_eq!(
        linkify_community_slugs("[check c/rust now](https://example.com)"),
        "[check c/rust now](https://example.com)"
    );
    assert_eq!(
        linkify_community_slugs("[community](https://example.com/?sub=c/rust)"),
        "[community](https://example.com/?sub=c/rust)"
    );
    assert_eq!(
        linkify_community_slugs("See https://example.com/?q=c/rust and https://example.com/#c/rust"),
        "See https://example.com/?q=c/rust and https://example.com/#c/rust"
    );
    assert_eq!(
        linkify_community_slugs("Already [c/rust](tg://community/rust) + c/gpui"),
        "Already [c/rust](tg://community/rust) + [c/gpui](tg://community/gpui)"
    );

    // Invalid short/punctuation-only/overlong/path/boundary-violating tokens stay untouched
    for unchanged in [
        "c/a",
        "c/---",
        "c/__",
        "c/foo/bar",
        "a-c/rust",
        "foo.c/rust",
        "user@c/rust",
        "path:c/rust",
        "?q=c/rust",
        "#c/rust",
        "c/aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
    ] {
        assert_eq!(
            linkify_community_slugs(unchanged),
            unchanged,
            "negative input must not be linkified: {unchanged}"
        );
    }

    // Positive boundaries: start, parens, comma, CJK, emoji, trailing punctuation
    assert_eq!(linkify_community_slugs("(c/rust)"), "([c/rust](tg://community/rust))");
    assert_eq!(
        linkify_community_slugs("看c/rust_lang社区"),
        "看[c/rust_lang](tg://community/rust_lang)社区"
    );
    assert_eq!(
        linkify_community_slugs("🔥c/rust_2026🚀"),
        "🔥[c/rust_2026](tg://community/rust_2026)🚀"
    );
    assert_eq!(
        linkify_community_slugs("c/ai_agents."),
        "[c/ai_agents](tg://community/ai_agents)."
    );
    assert_eq!(
        linkify_community_slugs("c/ab,c/rust"),
        "[c/ab](tg://community/ab),[c/rust](tg://community/rust)"
    );
}

#[test]
fn test_community_linkify_r2_c2_d1_stray_backtick_and_crlf_regressions() {
    // 1. Stray single backtick in paragraph 1 MUST NOT bleed across \n\n into paragraph 2 code span
    let p1 = "Don`t worry about it.\n\nVisit c/rust and also ` c/python ` in code.";
    let p1_exp = "Don`t worry about it.\n\nVisit [c/rust](tg://community/rust) and also ` c/python ` in code.";
    assert_eq!(linkify_community_slugs(p1), p1_exp);
    assert_eq!(linkify_community_slugs(p1_exp), p1_exp, "idempotent after linkify");

    // 2. Stray single backtick before line-start fenced code block MUST NOT bypass ``` fence opener
    let p2 = "It`s a typo\n```rust\nlet s = \"` c/rust `\";\n```";
    assert_eq!(linkify_community_slugs(p2), p2);

    // 3. Stray backtick + CRLF blank line (\r\n\r\n) MUST NOT bleed into paragraph 2 code span
    let p3 = "Don`t\r\n\r\nCheck c/rust and ` c/ai `";
    let p3_exp = "Don`t\r\n\r\nCheck [c/rust](tg://community/rust) and ` c/ai `";
    assert_eq!(linkify_community_slugs(p3), p3_exp);
    assert_eq!(linkify_community_slugs(p3_exp), p3_exp);

    // 4. Stray double backticks MUST NOT bleed across blank line into paragraph 2 double-backtick code span
    let p4 = "Weird `` typo\n\nSee c/rust and `` c/python ``";
    let p4_exp = "Weird `` typo\n\nSee [c/rust](tg://community/rust) and `` c/python ``";
    assert_eq!(linkify_community_slugs(p4), p4_exp);

    // 5. Unclosed Markdown link across CRLF blank line MUST have exact parity with LF blank line
    let p5_lf = "[unclosed\n\nSee c/rust](https://example.com)";
    let p5_crlf = "[unclosed\r\n\r\nSee c/rust](https://example.com)";
    assert_eq!(
        linkify_community_slugs(p5_lf),
        "[unclosed\n\nSee [c/rust](tg://community/rust)](https://example.com)"
    );
    assert_eq!(
        linkify_community_slugs(p5_crlf),
        "[unclosed\r\n\r\nSee [c/rust](tg://community/rust)](https://example.com)"
    );

    // 6. Extra edge cases: whitespace-only blank line + tilde fence opener after stray backtick
    let p6 = "Don`t\n   \t \nSee c/rust and ` c/python `";
    assert_eq!(
        linkify_community_slugs(p6),
        "Don`t\n   \t \nSee [c/rust](tg://community/rust) and ` c/python `"
    );
    let p7 = "It`s\n~~~\n` c/rust `\n~~~";
    assert_eq!(linkify_community_slugs(p7), p7);
}

#[test]
fn test_community_state_six_caches_vote_machine_compact_veil_and_algo_sanitize() {
    // C2-D4: sanitize_unit_f64 & sanitize_algo_prefs replace NaN/Inf and clamp bounds
    assert!((sanitize_unit_f64(f64::NAN, 0.35) - 0.35).abs() < 1e-9);
    assert!((sanitize_unit_f64(f64::INFINITY, 0.25) - 0.25).abs() < 1e-9);
    assert!((sanitize_unit_f64(f64::NEG_INFINITY, 0.45) - 0.45).abs() < 1e-9);
    assert_eq!(sanitize_unit_f64(-5.0, 0.5), 0.0);
    assert_eq!(sanitize_unit_f64(99.0, 0.5), 1.0);
    let dirty = AlgoPreferencesDto {
        preset: AlgoPreset::Balanced,
        recency_weight: f64::NAN,
        affinity_weight: f64::INFINITY,
        quality_weight: f64::NEG_INFINITY,
        discovery_ratio: -3.0,
        bot_content_ratio: 42.0,
        show_explanations: true,
        allow_nsfw: false,
    };
    let clean = sanitize_algo_prefs(&dirty);
    let defs = AlgoPreferencesDto::default();
    assert_eq!(clean.recency_weight, defs.recency_weight);
    assert_eq!(clean.affinity_weight, defs.affinity_weight);
    assert_eq!(clean.quality_weight, defs.quality_weight);
    assert_eq!(clean.discovery_ratio, 0.0);
    assert_eq!(clean.bot_content_ratio, 1.0);

    let make_post = |post_id: &str, body: &str| -> CommunityPostDto {
        serde_json::from_value(serde_json::json!({
            "post_id": post_id,
            "author_actor_id": "u:1",
            "author_handle": "tester",
            "author_display_name": "Tester",
            "body": body,
            "created_at_ms": 1_700_000_000_000i64
        }))
        .expect("deserialize test CommunityPostDto")
    };
    let make_actor = || -> CommunityActorDto {
        serde_json::from_value(serde_json::json!({
            "actor_id": "u:1",
            "handle": "tester",
            "display_name": "Tester",
            "created_at_ms": 1_700_000_000_000i64
        }))
        .expect("deserialize test CommunityActorDto")
    };

    // R2-BLOCKER-4 / C2-D6: Compact preview text masks NSFW/Spoiler body when veil_locked
    let mut nsfw_post = make_post("p_nsfw", "TOP_SECRET_NSFW_RAW_BODY_12345");
    nsfw_post.is_nsfw = true;
    let locked_en = compact_post_preview_text(&nsfw_post, true, "en");
    assert!(!locked_en.contains("TOP_SECRET_NSFW_RAW_BODY_12345"));
    assert!(locked_en.contains("NSFW"));
    let unlocked_en = compact_post_preview_text(&nsfw_post, false, "en");
    assert!(unlocked_en.contains("TOP_SECRET_NSFW_RAW_BODY_12345"));

    let mut spoiler_post = make_post("p_spoiler", "DARTH_VADER_SPOILER_SECRET");
    spoiler_post.is_spoiler = true;
    let locked_sp = compact_post_preview_text(&spoiler_post, true, "en");
    assert!(!locked_sp.contains("DARTH_VADER_SPOILER_SECRET"));
    assert!(locked_sp.contains("SPOILER"));

    // R2-BLOCKER-3 / C2-D2: find_post searches actor_profile.recent_posts & search_result.posts
    let mut state = CommunityWorkspaceState::default();
    let mut prof_post = make_post("p_only_prof", "Profile post");
    prof_post.upvotes = 10;
    prof_post.downvotes = 2;
    prof_post.score = 8;
    state.actor_profile = Some(ActorProfileResponse {
        actor: make_actor(),
        recent_posts: vec![prof_post],
        recent_comments: Vec::new(),
        moderated_communities: Vec::new(),
    });
    let mut search_post = make_post("p_only_search", "Search post");
    search_post.upvotes = 5;
    search_post.downvotes = 1;
    search_post.score = 4;
    state.search_result = Some(CommunitySearchResultDto {
        query: "rust".to_string(),
        posts: vec![search_post],
        ..Default::default()
    });

    assert!(state.find_post("p_only_prof").is_some());
    assert!(state.find_post("p_only_search").is_some());

    // Vote flip (+1 -> -1 delta -2), toggle-off (-1 -> 0 exact score), and error rollback
    let (d1, snap1) = state.apply_optimistic_vote("p_only_prof", 1);
    assert_eq!(d1, 1);
    assert!(snap1.is_some());
    assert_eq!(state.find_post("p_only_prof").unwrap().score, 9);
    let (d2, snap2) = state.apply_optimistic_vote("p_only_prof", -1);
    assert_eq!(d2, -1);
    assert!(snap2.is_some());
    assert_eq!(state.find_post("p_only_prof").unwrap().score, 7);
    let (d3, _snap3) = state.apply_optimistic_vote("p_only_prof", -1);
    assert_eq!(d3, 0);
    assert_eq!(state.find_post("p_only_prof").unwrap().score, 8);
    // Rollback on search-only post
    let (_, snap_s) = state.apply_optimistic_vote("p_only_search", 1);
    assert_eq!(state.find_post("p_only_search").unwrap().score, 5);
    state.rollback_vote("p_only_search", snap_s.expect("search post rollback snapshot"));
    assert_eq!(state.find_post("p_only_search").unwrap().score, 4);
    let like_snap = state.apply_optimistic_like("p_only_search").expect("search like snap");
    assert!(state.find_post("p_only_search").unwrap().viewer_liked);
    state.rollback_like("p_only_search", like_snap);
    assert!(!state.find_post("p_only_search").unwrap().viewer_liked);
}

// ============================================================================
// Headless GPUI Tests (Subprocess Re-Exec with Real AppShell + window.draw)
// ============================================================================

#[test]
fn test_community_six_entry_points_composer_hide_and_chat_return() {
    run_in_headless_subprocess("test_community_six_entry_points_composer_hide_and_chat_return");
}

async fn body_six_entry_points_composer_hide_and_chat_return(mut app: AsyncApp) {
    let host = open_authenticated_shell(&mut app, 1440.0, 900.0, 1001, "alice");
    let shell = host.shell.clone();

    // 1. Expanded ChatSidebar (#sidebar-nav-community / #top-action-community)
    let _ = shell.update(&mut app, |s, cx| {
        s.sidebar.update(cx, |sb, scx| {
            sb.set_collapsed(false);
            sb.handle_unified_action(
                UnifiedSidebarAction::SelectDestination(NavDestination::Community),
                scx,
            );
        });
    });
    assert!(
        pump_until(&mut app, Duration::from_secs(2), |cx| {
            let s = shell.read(cx);
            s.active_panel == ActiveWorkspacePanel::Community
                && s.should_hide_composer(cx)
                && s.community_workspace.is_some()
        })
        .await,
        "Expanded sidebar entry must open Community and hide composer"
    );
    host.draw(&mut app);

    // Return to Chats first
    let _ = shell.update(&mut app, |s, cx| {
        s.activate_nav_destination(NavDestination::Chats, cx);
    });

    // 2. Collapsed 64px rail (#rail-community-btn)
    let _ = shell.update(&mut app, |s, cx| {
        s.sidebar.update(cx, |sb, scx| {
            sb.set_collapsed(true);
            sb.handle_unified_action(
                UnifiedSidebarAction::SelectDestination(NavDestination::Community),
                scx,
            );
        });
    });
    assert!(
        pump_until(&mut app, Duration::from_secs(2), |cx| {
            shell.read(cx).active_panel == ActiveWorkspacePanel::Community
        })
        .await,
        "Collapsed rail entry must open Community"
    );
    host.draw(&mut app);

    // 3. NavRail direct selection
    let _ = shell.update(&mut app, |s, cx| {
        s.activate_nav_destination(NavDestination::Chats, cx);
        s.nav_rail.update(cx, |r, rcx| {
            r.select(NavDestination::Community);
            rcx.notify();
        });
    });
    assert!(
        pump_until(&mut app, Duration::from_secs(2), |cx| {
            shell.read(cx).active_panel == ActiveWorkspacePanel::Community
        })
        .await,
        "NavRail selection must open Community"
    );

    // 4 & 5. AccountPopover & MainMenuDrawer (MainMenuCommand::Community)
    let _ = shell.update(&mut app, |s, cx| {
        s.activate_nav_destination(NavDestination::Chats, cx);
        s.handle_main_menu_command(MainMenuCommand::Community, cx);
    });
    assert_eq!(
        shell.read_with(&app, |s, _| s.active_panel).unwrap(),
        ActiveWorkspacePanel::Community
    );

    // 6. Keyboard shortcut Cmd+Shift+C toggles Community <-> Chats; plain Cmd+C MUST NOT open Community!
    host.draw(&mut app);
    host.key(&mut app, "cmd-shift-c");
    assert_eq!(
        shell.read_with(&app, |s, _| s.active_panel).unwrap(),
        ActiveWorkspacePanel::Chats,
        "Cmd+Shift+C from Community must toggle back to Chats"
    );

    // Plain Cmd+C while on Chats MUST NOT open Community
    host.key(&mut app, "cmd-c");
    assert_eq!(
        shell.read_with(&app, |s, _| s.active_panel).unwrap(),
        ActiveWorkspacePanel::Chats,
        "Plain Cmd+C must NOT trigger Community"
    );

    // Cmd+Shift+C from Chats opens Community
    host.key(&mut app, "cmd-shift-c");
    assert_eq!(
        shell.read_with(&app, |s, _| s.active_panel).unwrap(),
        ActiveWorkspacePanel::Community,
        "Cmd+Shift+C from Chats must open Community"
    );

    // Clicking a chat dialog row in ChatSidebar MUST auto-return to ActiveWorkspacePanel::Chats
    let _ = shell.update(&mut app, |s, cx| {
        s.sidebar.update(cx, |sb, scx| {
            sb.set_collapsed(false);
            sb.dialogs.push(DialogItemModel {
                id: 7701,
                peer_name: "Bob".to_string(),
                avatar_initials: "BO".to_string(),
                avatar_color: 0x2AABEE,
                last_message: "hi".to_string(),
                timestamp: "10:00".to_string(),
                unread_count: 0,
                is_muted: false,
                is_pinned: false,
                is_online: true,
                typing_indicator: None,
                delivery_status: DialogDeliveryStatus::Read,
                folder: "All".to_string(),
            });
            sb.select_dialog_from_user(7701);
            scx.notify();
        });
    });
    assert!(
        pump_until(&mut app, Duration::from_secs(2), |cx| {
            let s = shell.read(cx);
            s.active_panel == ActiveWorkspacePanel::Chats && !s.should_hide_composer(cx)
        })
        .await,
        "Selecting a chat row must auto-return from Community to Chats"
    );
}

#[test]
fn test_community_responsive_layouts_sidebar_toggle_theme_locale_and_teardown() {
    run_in_headless_subprocess(
        "test_community_responsive_layouts_sidebar_toggle_theme_locale_and_teardown",
    );
}

async fn body_responsive_layouts_sidebar_toggle_theme_locale_and_teardown(mut app: AsyncApp) {
    // Verify real window.draw across all 4 responsive breakpoints
    for (w, h, expected) in [
        (1440.0, 900.0, CommunityLayoutMode::ThreeColumn),
        (1040.0, 800.0, CommunityLayoutMode::TwoColumn),
        (820.0, 720.0, CommunityLayoutMode::OneColumn),
        (640.0, 480.0, CommunityLayoutMode::OneColumn),
    ] {
        let host = open_authenticated_shell(&mut app, w, h, 1002, "bob");
        let shell = host.shell.clone();
        let _ = shell.update(&mut app, |s, cx| {
            s.open_community_workspace(cx);
        });
        host.draw(&mut app);
        let actual = shell
            .read_with(&app, |s, cx| {
                s.community_workspace
                    .as_ref()
                    .expect("cw mounted")
                    .read(cx)
                    .last_layout_mode
            })
            .unwrap();
        assert_eq!(actual, expected, "viewport {w}x{h} layout mismatch");
    }

    // Rapid Cmd+B sidebar collapse/expand, Cmd+D theme cycle, Cmd+L locale switch, user-switch & logout teardown
    let host = open_authenticated_shell(&mut app, 1440.0, 900.0, 1003, "carol");
    let shell = host.shell.clone();
    let _ = shell.update(&mut app, |s, cx| {
        s.open_community_workspace(cx);
        s.set_community_unread_badge(15, cx);
    });
    for _ in 0..4 {
        host.key(&mut app, "cmd-b");
        host.draw(&mut app);
    }

    let (mode_before, lang_before) = shell
        .read_with(&app, |s, cx| {
            let cw = s.community_workspace.as_ref().unwrap().read(cx);
            (cw.palette.mode, cw.language_id.clone())
        })
        .unwrap();
    host.key(&mut app, "cmd-d");
    host.key(&mut app, "cmd-l");
    host.draw(&mut app);
    let (mode_after, lang_after) = shell
        .read_with(&app, |s, cx| {
            let cw = s.community_workspace.as_ref().unwrap().read(cx);
            (cw.palette.mode, cw.language_id.clone())
        })
        .unwrap();
    assert_ne!(mode_before, mode_after, "Cmd+D must propagate theme to CommunityWorkspace");
    assert_ne!(lang_before, lang_after, "Cmd+L must propagate locale to CommunityWorkspace");
    assert!(matches!(
        mode_after,
        TdesktopThemeMode::Night
            | TdesktopThemeMode::Tinted
            | TdesktopThemeMode::Day
            | TdesktopThemeMode::Classic
    ));

    // User switch tears down CommunityWorkspace & resets badge to 0
    let _ = shell.update(&mut app, |s, cx| {
        s.apply_authenticated_session(test_auth(2004, "dave"), cx);
    });
    shell
        .read_with(&app, |s, cx| {
            assert!(s.community_workspace.is_none(), "user switch must tear down community_workspace");
            assert_eq!(s.active_panel, ActiveWorkspacePanel::Chats);
            let badge = s
                .nav_rail
                .read(cx)
                .items
                .iter()
                .find(|i| i.destination == NavDestination::Community)
                .unwrap()
                .unread_badge;
            assert_eq!(badge, 0, "user switch must clear Community unread badge");
        })
        .unwrap();

    // Re-open Community, then logout -> must tear down cleanly
    let _ = shell.update(&mut app, |s, cx| {
        s.open_community_workspace(cx);
        s.set_community_unread_badge(7, cx);
        s.logout(cx);
    });
    shell
        .read_with(&app, |s, cx| {
            assert!(s.community_workspace.is_none(), "logout must tear down community_workspace");
            assert_eq!(s.active_panel, ActiveWorkspacePanel::Chats);
            let badge = s
                .nav_rail
                .read(cx)
                .items
                .iter()
                .find(|i| i.destination == NavDestination::Community)
                .unwrap()
                .unread_badge;
            assert_eq!(badge, 0, "logout must reset Community unread badge");
        })
        .unwrap();
}

#[test]
fn test_community_centralized_cjk_ime_80ms_fence_multiline_and_3step_escape() {
    run_in_headless_subprocess(
        "test_community_centralized_cjk_ime_80ms_fence_multiline_and_3step_escape",
    );
}

async fn body_centralized_cjk_ime_80ms_fence_multiline_and_3step_escape(mut app: AsyncApp) {
    let host = open_authenticated_shell(&mut app, 1440.0, 900.0, 1005, "ime_user");
    let shell = host.shell.clone();
    let cw = shell.update(&mut app, |s, cx| s.open_community_workspace(cx)).unwrap();

    // Wait for initial seeded feed to load
    assert!(
        pump_until(&mut app, Duration::from_secs(3), |cx| {
            !cw.read(cx).state.feed_loading && !cw.read(cx).state.feed_items.is_empty()
        })
        .await,
        "initial seeded feed must load"
    );
    let initial_count = cw.read_with(&app, |w, _| w.state.feed_items.len()).unwrap();

    // Focus Quick-Post body
    let _ = cw.update(&mut app, |w, cx| w.open_quick_post_composer(cx));
    assert!(shell.read_with(&app, |s, cx| s.is_community_input_active(cx)).unwrap());

    // English commit + CJK preedit canceled by Backspace (must NOT erase committed English!)
    let _ = shell.update(&mut app, |s, cx| {
        s.insert_committed_text("Hello ", cx);
        s.update_ime_marked_text("社区", cx);
        let (committed, pre) = s.active_text_and_preedit(cx);
        assert_eq!(committed, "Hello ");
        assert_eq!(pre.as_deref(), Some("社区"));
        assert!(s.handle_non_char_key("backspace", false, cx));
        let (committed2, pre2) = s.active_text_and_preedit(cx);
        assert_eq!(committed2, "Hello ");
        assert!(pre2.is_none(), "Backspace must cancel preedit without deleting committed text");
    });

    // CJK preedit -> unmark commit -> immediate Enter MUST be swallowed by 80ms trailing-Enter fence!
    let _ = shell.update(&mut app, |s, cx| {
        s.update_ime_marked_text("中文测试", cx);
        s.commit_ime_unmark_text(cx);
        assert_eq!(s.active_text_and_preedit(cx).0, "Hello 中文测试");
        assert!(
            s.handle_non_char_key("enter", false, cx),
            "trailing Enter within 80ms fence must be consumed"
        );
    });
    assert_eq!(
        cw.read_with(&app, |w, _| w.state.quick_post_body.clone()).unwrap(),
        "Hello 中文测试",
        "80ms fence must prevent premature submission"
    );
    assert_eq!(
        cw.read_with(&app, |w, _| w.state.feed_items.len()).unwrap(),
        initial_count
    );

    // Multiline paste preserves '\n' in QuickPostBody
    let _ = shell.update(&mut app, |s, cx| {
        s.insert_committed_text("\n第二行 Markdown", cx);
    });
    assert_eq!(
        cw.read_with(&app, |w, _| w.state.quick_post_body.clone()).unwrap(),
        "Hello 中文测试\n第二行 Markdown"
    );

    // After 95ms (> 80ms fence), Enter submits Quick-Post genuinely into SQLite!
    app.background_executor().timer(Duration::from_millis(95)).await;
    let _ = shell.update(&mut app, |s, cx| {
        assert!(s.handle_non_char_key("enter", false, cx));
    });
    assert!(
        pump_until(&mut app, Duration::from_secs(3), |cx| {
            let w = cw.read(cx);
            !w.state.quick_post_submitting
                && w.state.feed_items.len() == initial_count + 1
                && w.state.feed_items[0].post.body.contains("中文测试")
                && w.state.feed_items[0].post.body.contains("第二行 Markdown")
        })
        .await,
        "Enter after 80ms fence must submit multiline CJK Quick-Post"
    );

    // 3-step progressive Escape cascade:
    // Setup: focus Quick-Post, open AlgoTuner popover, set IME preedit
    let _ = cw.update(&mut app, |w, cx| {
        w.open_quick_post_composer(cx);
        w.state.active_popover = Some(CommunityPopover::AlgoTuner);
        cx.notify();
    });
    let _ = shell.update(&mut app, |s, cx| {
        s.update_ime_marked_text("拼音", cx);
    });
    host.draw(&mut app);

    // Esc #1: cancels IME preedit ONLY (popover + input + Community stay open)
    host.key(&mut app, "escape");
    shell
        .read_with(&app, |s, cx| {
            assert!(!s.has_active_ime_preedit(cx));
            assert_eq!(cw.read(cx).state.active_popover, Some(CommunityPopover::AlgoTuner));
            assert_eq!(s.active_panel, ActiveWorkspacePanel::Community);
        })
        .unwrap();

    // Esc #2: closes Community popover (input + Community stay open)
    host.key(&mut app, "escape");
    shell
        .read_with(&app, |s, cx| {
            assert!(cw.read(cx).state.active_popover.is_none());
            assert!(cw.read(cx).has_focused_text_input());
            assert_eq!(s.active_panel, ActiveWorkspacePanel::Community);
        })
        .unwrap();

    // Esc #3: blurs focused Community input (Community panel stays open)
    host.key(&mut app, "escape");
    shell
        .read_with(&app, |s, cx| {
            assert!(!cw.read(cx).has_focused_text_input());
            assert_eq!(s.active_panel, ActiveWorkspacePanel::Community);
        })
        .unwrap();

    // Esc #4: returns from Community panel cleanly to Chats!
    host.key(&mut app, "escape");
    assert_eq!(
        shell.read_with(&app, |s, _| s.active_panel).unwrap(),
        ActiveWorkspacePanel::Chats,
        "Final Escape must exit Community to Chats"
    );
}

#[test]
fn test_community_identity_switcher_0bot_fallback_and_bot_posting() {
    run_in_headless_subprocess("test_community_identity_switcher_0bot_fallback_and_bot_posting");
}

async fn body_identity_switcher_0bot_fallback_and_bot_posting(mut app: AsyncApp) {
    let host = open_authenticated_shell(&mut app, 1440.0, 900.0, 1006, "human_zero_bots");
    let shell = host.shell.clone();
    let cw = shell.update(&mut app, |s, cx| s.open_community_workspace(cx)).unwrap();

    assert!(
        pump_until(&mut app, Duration::from_secs(3), |cx| {
            let w = cw.read(cx);
            w.state.identities.is_some() && !w.state.feed_loading
        })
        .await
    );

    // 0-bot fallback: open IdentitySwitcher popover and draw without panic!
    let _ = cw.update(&mut app, |w, cx| {
        assert!(
            w.state.identities.as_ref().unwrap().bot_actors.is_empty(),
            "fresh user starts with 0 owned bots"
        );
        w.toggle_popover(CommunityPopover::IdentitySwitcher, cx);
    });
    host.draw(&mut app);

    // Navigate to BotStudio and register a real first-class Bot citizen
    let _ = cw.update(&mut app, |w, cx| {
        w.navigate_to(CommunityRoute::BotStudio, cx);
        w.state.bot_studio_name = "Rust Critic Bot".to_string();
        w.state.bot_studio_handle = "rust_critic_bot".to_string();
        w.state.bot_studio_persona = "Reviews Rust RFCs and systems benchmarks.".to_string();
        w.submit_register_bot(cx);
    });
    assert!(
        pump_until(&mut app, Duration::from_secs(3), |cx| {
            cw.read(cx)
                .state
                .identities
                .as_ref()
                .is_some_and(|i| !i.bot_actors.is_empty())
        })
        .await,
        "BotStudio registration must add owned bot to identities"
    );
    host.draw(&mut app);

    let bot_actor_id = cw
        .read_with(&app, |w, _| {
            w.state.identities.as_ref().unwrap().bot_actors[0]
                .actor_id
                .clone()
        })
        .unwrap();

    // Switch active identity to the newly registered Bot!
    let _ = cw.update(&mut app, |w, cx| {
        w.replace_route(CommunityRoute::Feed, cx);
        w.switch_active_actor(bot_actor_id.clone(), cx);
    });
    assert_eq!(
        cw.read_with(&app, |w, _| w.facade.active_actor_id()).unwrap(),
        Some(bot_actor_id.clone())
    );
    assert!(
        pump_until(&mut app, Duration::from_secs(3), |cx| !cw.read(cx).state.feed_loading).await
    );
    let before_count = cw.read_with(&app, |w, _| w.state.feed_items.len()).unwrap();

    // Whitespace-only Quick-Post MUST be rejected with validation error
    let _ = cw.update(&mut app, |w, cx| {
        w.state.quick_post_body = "   \t  \n ".to_string();
        w.submit_quick_post(cx);
    });
    assert!(
        cw.read_with(&app, |w, _| w.state.quick_post_validation_error.is_some())
            .unwrap(),
        "whitespace-only Quick-Post must trigger validation error"
    );
    assert_eq!(cw.read_with(&app, |w, _| w.state.feed_items.len()).unwrap(), before_count);

    // Genuine Bot Quick-Post must create a post authored by the Bot citizen!
    let _ = cw.update(&mut app, |w, cx| {
        w.state.quick_post_body = "Zero-copy parsing benchmark results in c/rust #rustlang".to_string();
        w.submit_quick_post(cx);
    });
    assert!(
        pump_until(&mut app, Duration::from_secs(3), |cx| {
            let w = cw.read(cx);
            !w.state.quick_post_submitting
                && w.state.feed_items.len() == before_count + 1
                && w.state.feed_items[0].post.author_actor_id == bot_actor_id
                && w.state.feed_items[0].post.author_kind == ActorKind::Bot
        })
        .await,
        "Quick-Post while acting as Bot must create a first-class Bot post"
    );
}

#[test]
fn test_community_feed_controls_algo_tuner_post_interactions_polls_and_signals() {
    run_in_headless_subprocess(
        "test_community_feed_controls_algo_tuner_post_interactions_polls_and_signals",
    );
}

async fn body_feed_controls_algo_tuner_post_interactions_polls_and_signals(mut app: AsyncApp) {
    let host = open_authenticated_shell(&mut app, 1440.0, 900.0, 1007, "feed_tester");
    let client = Arc::clone(&host.client);
    let shell = host.shell.clone();
    let cw = shell.update(&mut app, |s, cx| s.open_community_workspace(cx)).unwrap();

    assert!(
        pump_until(&mut app, Duration::from_secs(3), |cx| {
            let w = cw.read(cx);
            !w.state.feed_loading && w.state.feed_items.len() >= 5 && !w.state.trending.is_empty()
        })
        .await,
        "seeded feed and trending must load"
    );

    // Draw all 3 densities (Card, Compact, Classic) via real window.draw
    for expected_density in [FeedDensity::Card, FeedDensity::Compact, FeedDensity::Classic] {
        assert_eq!(cw.read_with(&app, |w, _| w.state.density).unwrap(), expected_density);
        host.draw(&mut app);
        let _ = cw.update(&mut app, |w, cx| w.cycle_density(cx));
    }
    assert_eq!(cw.read_with(&app, |w, _| w.state.density).unwrap(), FeedDensity::Card);

    // Exercise Feed Tabs + ActorFilter + Sort/TimeWindow
    let _ = cw.update(&mut app, |w, cx| {
        w.set_actor_filter(ActorFilter::BotsOnly, cx);
    });
    assert!(
        pump_until(&mut app, Duration::from_secs(3), |cx| {
            let w = cw.read(cx);
            !w.state.feed_loading
                && !w.state.feed_items.is_empty()
                && w.state.feed_items.iter().all(|it| it.post.author_kind.is_bot_like())
        })
        .await,
        "BotsOnly filter must return only Bot-authored posts"
    );

    let _ = cw.update(&mut app, |w, cx| {
        w.set_actor_filter(ActorFilter::AllCitizens, cx);
        w.select_feed_tab(FeedTab::Popular, cx);
        w.set_feed_sort(FeedSort::Top, cx);
        w.set_time_window(TimeWindow::Week, cx);
    });
    assert!(
        pump_until(&mut app, Duration::from_secs(3), |cx| !cw.read(cx).state.feed_loading).await
    );

    // Algo Tuner: switch to RedditDeepDive preset via default_prefs_for_preset & save_algo_prefs
    let _ = cw.update(&mut app, |w, cx| {
        w.state.algo_draft = gpui_telegram_community::default_prefs_for_preset(AlgoPreset::RedditDeepDive);
        w.state.algo_draft.bot_content_ratio = 0.65;
        w.save_algo_prefs(cx);
    });
    assert!(
        pump_until(&mut app, Duration::from_secs(3), |cx| {
            let w = cw.read(cx);
            w.state.algo_prefs.preset == AlgoPreset::RedditDeepDive
                && !w.state.feed_loading
                && client
                    .community()
                    .block_on(client.community().get_algo_prefs())
                    .is_ok_and(|p| p.preset == AlgoPreset::RedditDeepDive)
        })
        .await,
        "RedditDeepDive algo prefs must persist to SQLite and finish reloading feed"
    );
    let persisted_prefs = client
        .community()
        .block_on(client.community().get_algo_prefs())
        .expect("read persisted algo prefs");
    assert_eq!(persisted_prefs.preset, AlgoPreset::RedditDeepDive);

    // Switch back to All feed so we have all seeded posts including Polls
    let _ = cw.update(&mut app, |w, cx| {
        w.select_feed_tab(FeedTab::All, cx);
        w.set_feed_sort(FeedSort::New, cx);
    });
    assert!(
        pump_until(&mut app, Duration::from_secs(3), |cx| {
            !cw.read(cx).state.feed_loading && !cw.read(cx).state.feed_items.is_empty()
        })
        .await
    );

    let target_pid = cw.read_with(&app, |w, _| w.state.feed_items[0].post.post_id.clone()).unwrap();
    let score_before = cw.read_with(&app, |w, _| w.state.feed_items[0].post.score).unwrap();

    // Upvote -> Flip to Downvote (-2) -> Toggle-off (0) -> Like -> Repost -> Bookmark -> Award
    let _ = cw.update(&mut app, |w, cx| w.vote_post(target_pid.clone(), 1, cx));
    assert!(
        pump_until(&mut app, Duration::from_secs(3), |cx| {
            cw.read(cx)
                .state
                .find_post(&target_pid)
                .is_some_and(|p| p.viewer_vote == 1 && p.score == score_before + 1)
        })
        .await
    );
    let _ = cw.update(&mut app, |w, cx| w.vote_post(target_pid.clone(), -1, cx));
    assert!(
        pump_until(&mut app, Duration::from_secs(3), |cx| {
            cw.read(cx)
                .state
                .find_post(&target_pid)
                .is_some_and(|p| p.viewer_vote == -1 && p.score == score_before - 1)
        })
        .await
    );
    let _ = cw.update(&mut app, |w, cx| w.vote_post(target_pid.clone(), -1, cx));
    assert!(
        pump_until(&mut app, Duration::from_secs(3), |cx| {
            cw.read(cx)
                .state
                .find_post(&target_pid)
                .is_some_and(|p| p.viewer_vote == 0 && p.score == score_before)
        })
        .await
    );
    let _ = cw.update(&mut app, |w, cx| {
        w.toggle_like(target_pid.clone(), cx);
        w.repost_post(target_pid.clone(), cx);
        w.toggle_bookmark(target_pid.clone(), cx);
        w.give_award(target_pid.clone(), "insightful".to_string(), cx);
    });
    assert!(
        pump_until(&mut app, Duration::from_secs(3), |cx| {
            cw.read(cx).state.find_post(&target_pid).is_some_and(|p| {
                p.viewer_liked && p.viewer_reposted && p.viewer_bookmarked && !p.awards.is_empty()
            })
        })
        .await,
        "Like, Repost, Bookmark, and Award must all reconcile from local runtime"
    );

    // Poll vote + All | Humans | Bots breakdown toggle (non-optional assertion!)
    let (poll_pid, opt_id) = cw
        .read_with(&app, |w, _| {
            w.state
                .feed_items
                .iter()
                .find_map(|it| {
                    it.post.poll.as_ref().and_then(|poll| {
                        poll.options
                            .first()
                            .map(|opt| (it.post.post_id.clone(), opt.option_id.clone()))
                    })
                })
        })
        .unwrap()
        .expect("seeded feed must contain a poll post");
    let _ = cw.update(&mut app, |w, cx| {
        w.vote_poll(poll_pid.clone(), opt_id.clone(), cx);
        w.state
            .poll_breakdown_by_post
            .insert(poll_pid.clone(), PollBreakdownMode::Humans);
        cx.notify();
    });
    assert!(
        pump_until(&mut app, Duration::from_secs(3), |cx| {
            cw.read(cx)
                .state
                .find_post(&poll_pid)
                .and_then(|p| p.poll.as_ref())
                .is_some_and(|poll| poll.viewer_voted_option_ids.contains(&opt_id))
        })
        .await,
        "Poll vote must persist and reconcile"
    );
    let _ = cw.update(&mut app, |w, cx| {
        w.state
            .poll_breakdown_by_post
            .insert(poll_pid.clone(), PollBreakdownMode::Bots);
        cx.notify();
    });
    host.draw(&mut app);

    // Explainability popover + ShowMore & NotInterested signals
    let _ = cw.update(&mut app, |w, cx| {
        w.toggle_popover(CommunityPopover::ExplainPost(target_pid.clone()), cx);
    });
    host.draw(&mut app);
    let _ = cw.update(&mut app, |w, cx| {
        w.record_signal(
            target_pid.clone(),
            EngagementSignalKind::ShowMore,
            None,
            None,
            cx,
        );
        w.record_signal(
            target_pid.clone(),
            EngagementSignalKind::NotInterested,
            None,
            None,
            cx,
        );
    });
    assert!(
        cw.read_with(&app, |w, _| w.state.hidden_post_ids.contains(&target_pid))
            .unwrap(),
        "NotInterested must hide post from visible stream"
    );
}

#[test]
fn test_community_ws_events_unmounted_badge_malformed_json_and_cross_app_bridges() {
    run_in_headless_subprocess(
        "test_community_ws_events_unmounted_badge_malformed_json_and_cross_app_bridges",
    );
}

async fn body_ws_events_unmounted_badge_malformed_json_and_cross_app_bridges(mut app: AsyncApp) {
    let host = open_authenticated_shell(&mut app, 1440.0, 900.0, 1008, "bridge_user");
    let shell = host.shell.clone();

    // 1. Unmounted CommunityWorkspace: WS UpdateEvent::CommunityEvent still increments unread badge
    //    AND malformed JSON never panics!
    let _ = shell.update(&mut app, |s, cx| {
        assert!(s.community_workspace.is_none());
        s.apply_update_event_with_pts(
            ChatId(0),
            None,
            UpdateEvent::CommunityEvent {
                event_kind: "mention".to_string(),
                community_id: Some("c_rust".to_string()),
                post_id: Some("p_1".to_string()),
                comment_id: None,
                actor_id: Some("b:500".to_string()),
                payload_json: "{malformed-json!!!".to_string(),
                timestamp_ms: 1_700_000_000_000,
            },
            cx,
        );
    });
    let unmounted_badge = shell
        .read_with(&app, |s, cx| {
            s.nav_rail
                .read(cx)
                .items
                .iter()
                .find(|i| i.destination == NavDestination::Community)
                .unwrap()
                .unread_badge
        })
        .unwrap();
    assert_eq!(unmounted_badge, 1, "unmounted WS mention must increment Community badge");

    // 2. Mount Community and send ClientDomainEvent::CommunityEvent("post_created") -> live banner
    let cw = shell.update(&mut app, |s, cx| s.open_community_workspace(cx)).unwrap();
    let _ = shell.update(&mut app, |s, cx| {
        s.handle_community_domain_event(
            ClientDomainEvent::CommunityEvent {
                event_kind: "post_created".to_string(),
                community_id: Some("c_rust".to_string()),
                post_id: Some("p_live_new".to_string()),
                comment_id: None,
                actor_id: Some("b:501".to_string()),
                payload_json: "{\"title\":\"Breaking\"}".to_string(),
                timestamp_ms: 1_700_000_001_000,
            },
            cx,
        );
    });
    assert_eq!(
        cw.read_with(&app, |w, _| w.state.pending_ws_new_posts).unwrap(),
        1,
        "WS post_created must increment pending_ws_new_posts banner counter"
    );
    host.draw(&mut app);
    let _ = cw.update(&mut app, |w, cx| w.refresh_feed_from_ws_banner(cx));
    assert_eq!(cw.read_with(&app, |w, _| w.state.pending_ws_new_posts).unwrap(), 0);

    // 3. Cross-App Bridge: OpenMediaLightbox -> resolves bare media_id to media_stream_url + Esc keeps Community open!
    let _ = shell.update(&mut app, |s, cx| {
        s.handle_community_action(
            CommunityWorkspaceAction::OpenMediaLightbox("media_test_42".to_string()),
            cx,
        );
    });
    shell
        .read_with(&app, |s, cx| {
            let mv = s
                .overlays
                .media_viewer_overlay
                .as_ref()
                .expect("MediaViewer overlay must open")
                .read(cx);
            assert!(
                mv.items[0].url.contains("/api/v1/media/stream/media_test_42"),
                "unexpected media url: {}",
                mv.items[0].url
            );
        })
        .unwrap();
    host.key(&mut app, "escape");
    shell
        .read_with(&app, |s, _| {
            assert!(s.overlays.media_viewer_overlay.is_none(), "Esc must close MediaViewer");
            assert_eq!(
                s.active_panel,
                ActiveWorkspacePanel::Community,
                "Closing MediaViewer via Esc must keep Community open"
            );
        })
        .unwrap();

    // 4. Cross-App Bridge: ShowToast (is_error: true) & UnreadCountChanged(1250)
    let _ = shell.update(&mut app, |s, cx| {
        s.handle_community_action(
            CommunityWorkspaceAction::ShowToast {
                text: "Rate limited".to_string(),
                is_error: true,
            },
            cx,
        );
        s.handle_community_action(CommunityWorkspaceAction::UnreadCountChanged(1250), cx);
    });
    shell
        .read_with(&app, |s, cx| {
            let toast = s
                .window_runtime
                .controller
                .current_toast
                .as_ref()
                .expect("toast must be shown");
            assert!(AppShell::is_error_toast_text(&toast.text));
            let badge = s
                .nav_rail
                .read(cx)
                .items
                .iter()
                .find(|i| i.destination == NavDestination::Community)
                .unwrap()
                .unread_badge;
            assert_eq!(badge, 1250);
        })
        .unwrap();

    // 5. Cross-App Bridge: SharePostToChat -> ForwardMessages modal + empty message_ids send path
    let _ = shell.update(&mut app, |s, cx| {
        s.handle_community_action(
            CommunityWorkspaceAction::SharePostToChat {
                post_id: "p_share_1".to_string(),
                summary: "[c/rust] @alice — Shared from Community".to_string(),
            },
            cx,
        );
    });
    shell
        .read_with(&app, |s, cx| {
            assert!(s.community_pending_share.is_some());
            assert!(matches!(
                s.modal_container.read(cx).dialog.kind,
                Some(ModalDialogKind::ForwardMessages { ref message_ids, .. }) if message_ids.is_empty()
            ));
        })
        .unwrap();
    let _ = shell.update(&mut app, |s, cx| {
        s.handle_modal_outcome(
            ModalActionOutcome::ForwardToChat {
                target_chat_id: 5501,
                message_ids: Vec::new(),
                extra_caption: "Check this out!".to_string(),
                show_sender: true,
            },
            cx,
        );
    });
    assert!(
        shell.read_with(&app, |s, _| s.community_pending_share.is_none()).unwrap(),
        "ForwardToChat with empty message_ids must consume community_pending_share"
    );

    // 6. Cross-App Bridge: OpenDmWithActor safety guards (<=0 and self) + valid cached peer DM navigation
    let _ = shell.update(&mut app, |s, cx| {
        s.modal_container.update(cx, |m, _| m.hide_all());
        s.handle_community_action(
            CommunityWorkspaceAction::OpenDmWithActor { linked_user_id: 0 },
            cx,
        );
        s.handle_community_action(
            CommunityWorkspaceAction::OpenDmWithActor { linked_user_id: -7 },
            cx,
        );
        s.handle_community_action(
            CommunityWorkspaceAction::OpenDmWithActor { linked_user_id: 1008 },
            cx,
        );
    });
    assert_eq!(
        shell.read_with(&app, |s, _| s.active_panel).unwrap(),
        ActiveWorkspacePanel::Community,
        "Invalid (<=0) or self OpenDmWithActor must stay safely in Community"
    );

    let _ = shell.update(&mut app, |s, cx| {
        s.session_sync.chat_peer_user_ids.insert(5501, 9901);
        s.handle_community_action(
            CommunityWorkspaceAction::OpenDmWithActor { linked_user_id: 9901 },
            cx,
        );
    });
    shell
        .read_with(&app, |s, cx| {
            assert_eq!(s.active_panel, ActiveWorkspacePanel::Chats);
            assert_eq!(s.stage.read(cx).active_chat_id, Some(5501));
        })
        .unwrap();
}

#[test]
fn test_community_secondary_routes_live_data_and_owned_bot_sync() {
    run_in_headless_subprocess("test_community_secondary_routes_live_data_and_owned_bot_sync");
}

async fn body_secondary_routes_live_data_and_owned_bot_sync(mut app: AsyncApp) {
    let host = open_authenticated_shell(&mut app, 1440.0, 900.0, 1009, "route_tester");
    let shell = host.shell.clone();

    // Seed an owned Fabushi Bot before opening Community -> sync_community_identities must sync it!
    let _ = shell.update(&mut app, |s, _cx| {
        s.miniapps.fabushi_bots.push(
            gpui_telegram_proto::miniapp_contracts::FabushiBotSummaryDto {
                bot_user_id: 90090,
                owner_user_id: 1009,
                agent_id: "agent-90090".to_string(),
                username: "fabushi_helper_bot".to_string(),
                display_name: "Fabushi Helper".to_string(),
                description: "Systems engineering helper bot".to_string(),
                avatar_shape: "hex".to_string(),
                avatar_color: "violet".to_string(),
                preferred_model: "Fabushi-Pro".to_string(),
                ..Default::default()
            },
        );
    });

    let cw = shell.update(&mut app, |s, cx| s.open_community_workspace(cx)).unwrap();
    assert!(
        pump_until(&mut app, Duration::from_secs(3), |cx| {
            let w = cw.read(cx);
            !w.state.feed_loading
                && !w.state.feed_items.is_empty()
                && !w.state.communities.is_empty()
                && w.state
                    .identities
                    .as_ref()
                    .is_some_and(|i| i.bot_actors.iter().any(|b| b.linked_user_id == Some(90090)))
        })
        .await,
        "Owned Fabushi Bot must sync into Community identities automatically"
    );
    host.draw(&mut app);

    // 1. PostDetail route: load seeded post, submit inline reply, Summon Bot, and render
    let (sample_pid, sample_author, sample_comm) = cw
        .read_with(&app, |w, _| {
            let p = &w.state.feed_items[0].post;
            (
                p.post_id.clone(),
                p.author_handle.clone(),
                w.state.communities[0].slug.clone(),
            )
        })
        .unwrap();

    let _ = cw.update(&mut app, |w, cx| w.open_post_detail(sample_pid.clone(), cx));
    assert!(
        pump_until(&mut app, Duration::from_secs(3), |cx| {
            cw.read(cx)
                .state
                .detail_post
                .as_ref()
                .is_some_and(|d| d.post.post_id == sample_pid)
        })
        .await
    );
    let _ = cw.update(&mut app, |w, cx| {
        w.state.detail_reply_draft = "Great synthesis of Reddit trees + X speed!".to_string();
        w.submit_detail_reply(cx);
    });
    assert!(
        pump_until(&mut app, Duration::from_secs(3), |cx| {
            cw.read(cx)
                .state
                .detail_post
                .as_ref()
                .is_some_and(|d| {
                    d.comments
                        .iter()
                        .any(|c| c.body.contains("Great synthesis of Reddit trees"))
                })
        })
        .await,
        "Inline PostDetail reply must persist and reload into comment tree"
    );
    let _ = cw.update(&mut app, |w, cx| w.summon_bot_on_post(sample_pid.clone(), cx));
    host.draw(&mut app);

    // 2. CommunityPage route: load c/<slug>, toggle join, and render
    let _ = cw.update(&mut app, |w, cx| {
        w.navigate_to(CommunityRoute::CommunityPage(sample_comm.clone()), cx);
    });
    assert!(
        pump_until(&mut app, Duration::from_secs(3), |cx| {
            cw.read(cx)
                .state
                .community_detail
                .as_ref()
                .is_some_and(|d| d.community.slug == sample_comm)
        })
        .await
    );
    host.draw(&mut app);

    // 3. ActorProfile route: load @handle and render
    let _ = cw.update(&mut app, |w, cx| w.open_actor_profile(sample_author.clone(), cx));
    assert!(
        pump_until(&mut app, Duration::from_secs(3), |cx| {
            cw.read(cx)
                .state
                .actor_profile
                .as_ref()
                .is_some_and(|p| p.actor.handle == sample_author)
        })
        .await
    );
    host.draw(&mut app);

    // 4. Explore route: live search and render
    let _ = cw.update(&mut app, |w, cx| w.open_explore_with_query("rust".to_string(), cx));
    assert!(
        pump_until(&mut app, Duration::from_secs(3), |cx| {
            cw.read(cx).state.search_result.is_some()
        })
        .await
    );
    host.draw(&mut app);

    // 5. Notifications, BookmarksLists, ModQueue, BotStudio + Back navigation
    for route in [
        CommunityRoute::Notifications,
        CommunityRoute::BookmarksLists,
        CommunityRoute::ModQueue(Some(sample_comm.clone())),
        CommunityRoute::BotStudio,
    ] {
        let _ = cw.update(&mut app, |w, cx| w.navigate_to(route.clone(), cx));
        app.background_executor().timer(Duration::from_millis(60)).await;
        host.draw(&mut app);
    }
    let _ = cw.update(&mut app, |w, cx| w.go_back(cx));
    assert_eq!(
        cw.read_with(&app, |w, _| w.state.route.clone()).unwrap(),
        CommunityRoute::ModQueue(Some(sample_comm))
    );
}

#[test]
fn test_community_f15_remediation_regressions() {
    run_in_headless_subprocess("test_community_f15_remediation_regressions");
}

async fn body_f15_remediation_regressions(mut app: AsyncApp) {
    let host = open_authenticated_shell(&mut app, 1440.0, 900.0, 4501, "f15_rem_user");
    let shell = host.shell.clone();

    // Seed an owned Fabushi Bot (b:95001) BEFORE mounting Community
    let _ = shell.update(&mut app, |s, _cx| {
        s.miniapps.fabushi_bots.push(
            gpui_telegram_proto::miniapp_contracts::FabushiBotSummaryDto {
                bot_user_id: 95001,
                owner_user_id: 4501,
                agent_id: "agent-95001".to_string(),
                username: "my_owned_bot".to_string(),
                display_name: "My Owned Bot".to_string(),
                ..Default::default()
            },
        );
    });

    // F15-D5 (unmounted): owned bot WS event MUST NOT inflate badge, foreign bot MUST inflate
    let _ = shell.update(&mut app, |s, cx| {
        s.handle_community_domain_event(
            ClientDomainEvent::CommunityEvent {
                event_kind: "comment_created".to_string(),
                community_id: None,
                post_id: Some("p_own_unmounted".to_string()),
                comment_id: None,
                actor_id: Some("b:95001".to_string()),
                payload_json: "{}".to_string(),
                timestamp_ms: 1_700_000_000_000,
            },
            cx,
        );
    });
    let badge_after_own_bot = shell
        .read_with(&app, |s, cx| {
            s.nav_rail
                .read(cx)
                .items
                .iter()
                .find(|i| i.destination == NavDestination::Community)
                .unwrap()
                .unread_badge
        })
        .unwrap();
    assert_eq!(badge_after_own_bot, 0, "F15-D5: unmounted own-bot WS event must not inflate badge");

    let cw = shell.update(&mut app, |s, cx| s.open_community_workspace(cx)).unwrap();
    assert!(
        pump_until(&mut app, Duration::from_secs(3), |cx| {
            cw.read(cx)
                .state
                .identities
                .as_ref()
                .is_some_and(|i| i.bot_actors.iter().any(|b| b.actor_id == "b:95001"))
        })
        .await,
        "owned bot b:95001 must sync without bootstrap clobber (R2-BLOCKER-2)"
    );

    // F15-D5 (mounted): switch active actor to owned bot b:95001, then human self event u:4501 MUST NOT inflate unread
    let _ = cw.update(&mut app, |w, cx| {
        w.state.unread_notifications = 0;
        w.switch_active_actor("b:95001".to_string(), cx);
    });
    assert!(pump_until(&mut app, Duration::from_secs(3), |cx| !cw.read(cx).state.notifications_loading).await);
    let unread_before = cw.read_with(&app, |w, _| w.state.unread_notifications).unwrap();
    let _ = shell.update(&mut app, |s, cx| {
        s.handle_community_domain_event(
            ClientDomainEvent::CommunityEvent {
                event_kind: "comment_created".to_string(),
                community_id: None,
                post_id: Some("p_human_self".to_string()),
                comment_id: None,
                actor_id: Some("u:4501".to_string()),
                payload_json: "{}".to_string(),
                timestamp_ms: 1_700_000_001_000,
            },
            cx,
        );
    });
    assert_eq!(
        cw.read_with(&app, |w, _| w.state.unread_notifications).unwrap(),
        unread_before,
        "F15-D5: human self WS event while acting as owned bot must not inflate unread"
    );

    // F15-D1: Cmd+K focuses sidebar search; Escape exits search while staying in Community (composer unfocused!)
    let _ = shell.update(&mut app, |s, cx| {
        assert!(s.handle_shortcut_key("k", false, cx));
        assert!(s.sidebar.read(cx).search_focused);
        s.insert_committed_text("alice", cx);
        assert_eq!(s.sidebar.read(cx).search_query, "alice");
        assert!(s.handle_non_char_key("escape", false, cx));
        assert_eq!(s.active_panel, ActiveWorkspacePanel::Community, "F15-D1: Escape over sidebar search must stay in Community");
        assert!(!s.sidebar.read(cx).search_focused, "F15-D1: Escape must blur sidebar search");
        assert!(s.sidebar.read(cx).search_query.is_empty(), "F15-D1: Escape must clear sidebar search query");
        assert!(!s.composer.read(cx).is_input_focused, "F15-D1: Escape must NOT focus hidden chat composer");
    });

    // F15-D3: Single-line paste into RightRailSearch flattens \n / \r\n to ' ' and strips control chars
    let _ = cw.update(&mut app, |w, cx| {
        w.state.right_rail_search_draft.clear();
        w.state.focus_input(ActiveCommunityInput::RightRailSearch);
        cx.notify();
    });
    let _ = shell.update(&mut app, |s, cx| {
        s.insert_committed_text("Alpha\nBeta\r\nGamma\x00\x07Delta", cx);
    });
    assert_eq!(
        cw.read_with(&app, |w, _| w.state.right_rail_search_draft.clone()).unwrap(),
        "Alpha Beta GammaDelta",
        "F15-D3: single-line paste must flatten newlines to spaces"
    );

    // F15-D2 & F15-D8: Stale IME preedit cleared immediately on MediaViewer/Modal/AccountPopover open,
    // first Escape closes lightbox directly, and .mp4?query + HTTPS:// handled accurately
    let _ = cw.update(&mut app, |w, cx| w.open_quick_post_composer(cx));
    let _ = shell.update(&mut app, |s, cx| {
        s.update_ime_marked_text("旧拼音", cx);
        assert_eq!(s.ime.ime_preedit.as_deref(), Some("旧拼音"));
        assert_eq!(cw.read(cx).state.ime_preedit.as_deref(), Some("旧拼音"));
        s.handle_community_action(
            CommunityWorkspaceAction::OpenMediaLightbox("https://example.com/a.mp4?token=xyz".to_string()),
            cx,
        );
        assert!(s.ime.ime_preedit.is_none(), "F15-D2: show_media_viewer must clear shell preedit");
        assert!(cw.read(cx).state.ime_preedit.is_none(), "F15-D2: show_media_viewer must clear cw preedit");
        let mv = s.overlays.media_viewer_overlay.as_ref().expect("MediaViewer open").read(cx);
        assert_eq!(mv.items[0].url, "https://example.com/a.mp4?token=xyz");
        assert!(mv.items[0].is_video, "F15-D8: .mp4?token=xyz must detect is_video=true");
        assert!(s.handle_non_char_key("escape", false, cx));
        assert!(s.overlays.media_viewer_overlay.is_none(), "F15-D2: first Escape must close MediaViewer");
        assert_eq!(s.active_panel, ActiveWorkspacePanel::Community);
    });
    let _ = shell.update(&mut app, |s, cx| {
        s.handle_community_action(
            CommunityWorkspaceAction::OpenMediaLightbox("HTTPS://example.com/a.MP4".to_string()),
            cx,
        );
        let mv = s.overlays.media_viewer_overlay.as_ref().expect("MediaViewer open").read(cx);
        assert_eq!(mv.items[0].url, "HTTPS://example.com/a.MP4", "F15-D8: uppercase HTTPS:// must not be prefixed");
        assert!(mv.items[0].is_video, "F15-D8: uppercase .MP4 must detect is_video=true");
        s.hide_media_viewer();
    });

    // F15-D2 (AccountPopover + synthetic occluded Escape preedit clear)
    let _ = cw.update(&mut app, |w, cx| w.open_quick_post_composer(cx));
    let _ = shell.update(&mut app, |s, cx| {
        s.update_ime_marked_text("弹窗拼音", cx);
        s.toggle_account_popover(cx);
        assert!(s.ime.ime_preedit.is_none());
        assert!(cw.read(cx).state.ime_preedit.is_none());
        // Synthetic occluded preedit: dismiss_pre_auth_escape clears cw.state.ime_preedit unconditionally
        s.ime.ime_preedit = Some("合成".to_string());
        cw.update(cx, |w, wcx| w.set_ime_preedit(Some("合成"), wcx));
        assert!(s.handle_non_char_key("escape", false, cx));
        assert!(s.ime.ime_preedit.is_none());
        assert!(cw.read(cx).state.ime_preedit.is_none(), "Reviewer 1 §2.6: preedit clear must be unconditional");
        s.toggle_account_popover(cx);
    });

    // F15-D6 (same-turn Escape) & F15-D7 (modal replacement) clear community_pending_share cleanly
    let _ = shell.update(&mut app, |s, cx| {
        s.handle_community_action(
            CommunityWorkspaceAction::SharePostToChat {
                post_id: "p_fast_cancel".to_string(),
                summary: "Fast Cancel Summary".to_string(),
            },
            cx,
        );
        assert!(s.community_pending_share.is_some());
        assert!(s.handle_non_char_key("escape", false, cx));
        assert!(s.community_pending_share.is_none(), "F15-D6: immediate Escape must clear community_pending_share");

        s.handle_community_action(
            CommunityWorkspaceAction::SharePostToChat {
                post_id: "p_replaced".to_string(),
                summary: "Replaced Summary".to_string(),
            },
            cx,
        );
        assert!(s.community_pending_share.is_some());
        s.open_modal_dialog(
            "Other Forward",
            ModalDialogKind::ForwardMessages {
                message_ids: vec![1],
                preview_snippet: "Chat msg".to_string(),
            },
            420.0,
            460.0,
            cx,
        );
        assert!(s.community_pending_share.is_none(), "F15-D7: open_modal_dialog must clear stale community_pending_share");
    });
}

#[test]
fn test_community_f16_remediation_regressions() {
    run_in_headless_subprocess("test_community_f16_remediation_regressions");
}

async fn body_f16_remediation_regressions(mut app: AsyncApp) {
    let host = open_authenticated_shell(&mut app, 1440.0, 900.0, 4601, "f16_rem_user");
    let client = Arc::clone(&host.client);
    let shell = host.shell.clone();
    let cw = shell.update(&mut app, |s, cx| s.open_community_workspace(cx)).unwrap();

    assert!(
        pump_until(&mut app, Duration::from_secs(3), |cx| {
            let w = cw.read(cx);
            !w.state.feed_loading && w.state.feed_items.len() >= 3 && !w.state.communities.is_empty()
        })
        .await
    );

    // R2-BLOCKER-1: rich-text action sets card_child_click_consumed so parent PostCard on_click skips open_post_detail
    let _ = cw.update(&mut app, |w, cx| {
        w.state.card_child_click_consumed = false;
        w.handle_rich_text_action(RichTextAction::OpenUrl("tg://community/rust".to_string()), cx);
        assert!(w.state.card_child_click_consumed, "R2-BLOCKER-1: handle_rich_text_action must mark child click consumed");
        assert_eq!(w.state.route, CommunityRoute::CommunityPage("rust".to_string()));
        assert!(std::mem::take(&mut w.state.card_child_click_consumed));
        w.replace_route(CommunityRoute::Feed, cx);
    });

    // R2-MAJOR-6: multi-choice poll allows toggling multiple options and toggling off down to >= 1
    let multi_created = client
        .community()
        .block_on(client.community().create_post(&CreatePostRequest {
            format: PostFormat::Poll,
            title: Some("Multi-select poll regression".to_string()),
            body: "Pick multiple engines".to_string(),
            status: PostStatus::Published,
            poll: Some(CreatePollInputDto {
                question: "Favorite tools?".to_string(),
                options: vec!["Rust".to_string(), "GPUI".to_string(), "SQLite".to_string()],
                multi_select: true,
                duration_secs: 3600,
            }),
            ..Default::default()
        }))
        .expect("create multi-select poll post");
    let mp_id = multi_created.post.post_id.clone();
    let (opt1, opt2) = {
        let p = multi_created.post.poll.as_ref().expect("poll dto");
        assert!(p.multi_select);
        (p.options[0].option_id.clone(), p.options[1].option_id.clone())
    };
    let _ = cw.update(&mut app, |w, cx| {
        w.state.feed_items.insert(0, FeedItemDto {
            post: multi_created.post,
            explanation: None,
            rank_score: 99.0,
            is_exploration_slot: false,
        });
        w.vote_poll(mp_id.clone(), opt1.clone(), cx);
    });
    assert!(
        pump_until(&mut app, Duration::from_secs(3), |cx| {
            cw.read(cx).state.find_post(&mp_id).and_then(|p| p.poll.as_ref())
                .is_some_and(|p| p.viewer_voted_option_ids == vec![opt1.clone()])
        }).await,
        "first multi-select option must be recorded"
    );
    let _ = cw.update(&mut app, |w, cx| w.vote_poll(mp_id.clone(), opt2.clone(), cx));
    assert!(
        pump_until(&mut app, Duration::from_secs(3), |cx| {
            cw.read(cx).state.find_post(&mp_id).and_then(|p| p.poll.as_ref())
                .is_some_and(|p| p.viewer_voted_option_ids.contains(&opt1) && p.viewer_voted_option_ids.contains(&opt2))
        }).await,
        "R2-MAJOR-6: second click on multi-select poll must add opt2 while keeping opt1"
    );
    let _ = cw.update(&mut app, |w, cx| w.vote_poll(mp_id.clone(), opt1.clone(), cx));
    assert!(
        pump_until(&mut app, Duration::from_secs(3), |cx| {
            cw.read(cx).state.find_post(&mp_id).and_then(|p| p.poll.as_ref())
                .is_some_and(|p| !p.viewer_voted_option_ids.contains(&opt1) && p.viewer_voted_option_ids == vec![opt2.clone()])
        }).await,
        "R2-MAJOR-6: re-clicking opt1 on multi-select poll must toggle opt1 off while keeping opt2"
    );

    // R2-BLOCKER-3 / C2-D2 (live): bookmark + unbookmark a post present ONLY in actor_profile.recent_posts
    let (live_pid, live_author, live_comm) = cw
        .read_with(&app, |w, _| {
            let p = &w.state.feed_items[1].post;
            (p.post_id.clone(), p.author_handle.clone(), w.state.communities[0].slug.clone())
        })
        .unwrap();
    let _ = cw.update(&mut app, |w, cx| {
        w.open_actor_profile(live_author.clone(), cx);
    });
    assert!(
        pump_until(&mut app, Duration::from_secs(3), |cx| {
            cw.read(cx).state.actor_profile.as_ref()
                .is_some_and(|p| p.recent_posts.iter().any(|x| x.post_id == live_pid))
        }).await
    );
    let _ = cw.update(&mut app, |w, cx| {
        w.state.feed_items.clear();
        w.state.community_posts.clear();
        w.state.saved_items.clear();
        w.state.detail_post = None;
        w.toggle_bookmark(live_pid.clone(), cx);
    });
    assert!(
        pump_until(&mut app, Duration::from_secs(3), |cx| {
            cw.read(cx).state.find_post(&live_pid).is_some_and(|p| p.viewer_bookmarked)
        }).await,
        "bookmark on profile-only post must reconcile"
    );
    let _ = cw.update(&mut app, |w, cx| w.toggle_bookmark(live_pid.clone(), cx));
    assert!(
        pump_until(&mut app, Duration::from_secs(3), |cx| {
            cw.read(cx).state.find_post(&live_pid).is_some_and(|p| !p.viewer_bookmarked)
        }).await,
        "R2-BLOCKER-3 / C2-D2: unbookmark on profile-only post must reconcile"
    );

    // C2-D3 / R2-MINOR-7: warm & cold nonexistent secondary routes MUST clear stale caches & set errors
    let _ = cw.update(&mut app, |w, cx| {
        w.open_post_detail(live_pid.clone(), cx);
    });
    assert!(pump_until(&mut app, Duration::from_secs(3), |cx| cw.read(cx).state.detail_post.is_some()).await);
    let _ = cw.update(&mut app, |w, cx| w.open_post_detail("no_such_post_xyz".to_string(), cx));
    assert!(
        pump_until(&mut app, Duration::from_secs(3), |cx| {
            let s = &cw.read(cx).state;
            !s.detail_loading && s.detail_post.is_none() && s.detail_error.is_some()
        }).await,
        "C2-D3: nonexistent PostDetail must clear stale detail_post and set detail_error"
    );
    host.draw(&mut app);

    let _ = cw.update(&mut app, |w, cx| w.navigate_to(CommunityRoute::CommunityPage(live_comm.clone()), cx));
    assert!(pump_until(&mut app, Duration::from_secs(3), |cx| cw.read(cx).state.community_detail.is_some()).await);
    let _ = cw.update(&mut app, |w, cx| w.navigate_to(CommunityRoute::CommunityPage("no_such_comm_xyz".to_string()), cx));
    assert!(
        pump_until(&mut app, Duration::from_secs(3), |cx| {
            let s = &cw.read(cx).state;
            !s.community_loading && s.community_detail.is_none() && s.community_posts.is_empty() && s.community_error.is_some()
        }).await,
        "C2-D3: nonexistent CommunityPage must clear stale community_detail & posts"
    );
    host.draw(&mut app);

    let _ = cw.update(&mut app, |w, cx| w.open_actor_profile("nobody_xyz_404".to_string(), cx));
    assert!(
        pump_until(&mut app, Duration::from_secs(3), |cx| {
            let s = &cw.read(cx).state;
            !s.profile_loading && s.actor_profile.is_none() && s.profile_error.is_some()
        }).await,
        "C2-D3: nonexistent ActorProfile must clear stale actor_profile"
    );
    host.draw(&mut app);

    // C2-D4: NaN/Inf algo_draft renders cleanly in AlgoTuner and is sanitized synchronously before load_feed
    let _ = cw.update(&mut app, |w, cx| {
        w.replace_route(CommunityRoute::Feed, cx);
        w.state.algo_draft.recency_weight = f64::NAN;
        w.state.algo_draft.affinity_weight = f64::INFINITY;
        w.state.algo_draft.quality_weight = f64::NEG_INFINITY;
        w.state.algo_draft.discovery_ratio = -5.0;
        w.state.algo_draft.bot_content_ratio = 99.0;
        w.state.active_popover = Some(CommunityPopover::AlgoTuner);
        cx.notify();
    });
    host.draw(&mut app);
    let _ = cw.update(&mut app, |w, cx| {
        w.save_algo_prefs(cx);
        assert!(w.state.algo_prefs.recency_weight.is_finite());
        assert!(w.state.algo_prefs.affinity_weight.is_finite());
        assert!(w.state.algo_prefs.quality_weight.is_finite());
        assert_eq!(w.state.algo_prefs.discovery_ratio, 0.0);
        assert_eq!(w.state.algo_prefs.bot_content_ratio, 1.0);
        assert!(w.state.algo_draft.recency_weight.is_finite());
    });
    assert!(pump_until(&mut app, Duration::from_secs(3), |cx| !cw.read(cx).state.feed_loading).await);

    // C2-D7 (duplicate post + dual embeds), R2-BLOCKER-4 (Compact NSFW veil), C2-D8 (Quote toggle-off blur)
    let _ = cw.update(&mut app, |w, cx| {
        let mut p = w.state.feed_items[0].post.clone();
        p.is_nsfw = true;
        p.is_spoiler = true;
        p.media = vec![PostMediaAttachmentDto {
            media_id: "m1".to_string(),
            kind: "image".to_string(),
            url: Some("https://example.com/1.png".to_string()),
            thumbnail_url: None,
            alt_text: None,
            width: None,
            height: None,
            duration_secs: None,
        }];
        p.link_preview = Some(LinkPreviewDto {
            url: "https://example.com".to_string(),
            domain: "example.com".to_string(),
            title: "Example".to_string(),
            description: None,
            image_url: None,
        });
        let emb = EmbeddedPostSummaryDto {
            post_id: "same_emb_id".to_string(),
            community_slug: None,
            author_actor_id: "b:777".to_string(),
            author_handle: "bot_x".to_string(),
            author_display_name: "Bot X".to_string(),
            author_kind: ActorKind::Bot,
            author_avatar_shape: "blob".to_string(),
            author_avatar_color: "blue".to_string(),
            title: None,
            body_preview: "Embedded".to_string(),
            format: PostFormat::Micro,
            score: 1,
            comments_count: 0,
            created_at_ms: 1_700_000_000_000,
        };
        p.quoted_post = Some(Box::new(emb.clone()));
        p.crosspost_origin = Some(Box::new(emb.clone()));
        p.reposted_post = Some(Box::new(emb));
        let item = FeedItemDto {
            post: p.clone(),
            explanation: Some(FeedExplanationDto {
                primary_reason: "NaN test".to_string(),
                signal_breakdown: vec![SignalContributionDto {
                    label: "Affinity".to_string(),
                    contribution_pct: f64::NAN,
                }],
            }),
            rank_score: 1.0,
            is_exploration_slot: false,
        };
        // Two copies of the exact same post_id in feed_items + dual embeds (tests C2-D7 real window.draw!)
        w.state.feed_items = vec![item.clone(), item];
        w.state.revealed_post_veils.insert(p.post_id.clone());
        w.state.active_popover = Some(CommunityPopover::ExplainPost(p.post_id.clone()));
        cx.notify();
    });
    host.draw(&mut app);
    let _ = cw.update(&mut app, |w, cx| {
        // Test Compact NSFW veil draw + Quote popover toggle-off focus blur (C2-D8)
        let pid = w.state.feed_items[0].post.post_id.clone();
        w.state.revealed_post_veils.remove(&pid);
        w.state.density = FeedDensity::Compact;
        w.state.active_popover = None;
        w.toggle_popover(CommunityPopover::QuotePost(pid.clone()), cx);
        w.state.focus_input(ActiveCommunityInput::QuotePostNote);
        assert_eq!(w.state.active_input, ActiveCommunityInput::QuotePostNote);
        w.toggle_popover(CommunityPopover::QuotePost(pid), cx);
        assert!(w.state.active_popover.is_none());
        assert_eq!(w.state.active_input, ActiveCommunityInput::None, "C2-D8: Quote toggle-off must blur QuotePostNote");
        cx.notify();
    });
    host.draw(&mut app);
}

#[test]
fn test_community_r3_second_order_f15_regressions() {
    run_in_headless_subprocess("test_community_r3_second_order_f15_regressions");
}

async fn body_r3_second_order_f15_regressions(mut app: AsyncApp) {
    let host = open_authenticated_shell(&mut app, 1440.0, 900.0, 4701, "r3_second_order_user");
    let shell = host.shell.clone();

    // Seed owned Fabushi Bot (b:97001), owned my_bot (b:97002), and foreign bot (b:97999)
    let _ = shell.update(&mut app, |s, _cx| {
        s.miniapps.fabushi_bots.push(
            gpui_telegram_proto::miniapp_contracts::FabushiBotSummaryDto {
                bot_user_id: 97001,
                owner_user_id: 4701,
                agent_id: "agent-97001".to_string(),
                username: "r3_owned_fabushi".to_string(),
                display_name: "R3 Owned Fabushi".to_string(),
                ..Default::default()
            },
        );
        s.miniapps.my_bots.push(
            gpui_telegram_proto::miniapp_contracts::BotRegistryEntry {
                bot_user_id: 97002,
                owner_user_id: 4701,
                username: "r3_owned_my_bot".to_string(),
                display_name: "R3 Owned MyBot".to_string(),
                ..Default::default()
            },
        );
        s.miniapps.fabushi_bots.push(
            gpui_telegram_proto::miniapp_contracts::FabushiBotSummaryDto {
                bot_user_id: 97999,
                owner_user_id: 8888,
                agent_id: "agent-97999".to_string(),
                username: "foreign_bot".to_string(),
                display_name: "Foreign Bot".to_string(),
                ..Default::default()
            },
        );
    });

    // F15-R2-D1 setup: open in-chat stage search in Chats BEFORE switching to Community
    let _ = shell.update(&mut app, |s, cx| {
        s.stage.update(cx, |st, stcx| {
            st.search_open = true;
            st.search_query = "needle".to_string();
            stcx.notify();
        });
        assert!(s.is_stage_search_active(cx), "stage search is active while in Chats");
    });

    // F15-R2-D4: Cold mount race — open Community AND immediately dispatch own-Bot & own-Human WS events
    // on the SAME turn BEFORE async sync_identities resolves (cw.state.identities == None)!
    let cw = shell
        .update(&mut app, |s, cx| {
            let cw = s.open_community_workspace(cx);
            assert!(
                cw.read(cx).state.identities.is_none(),
                "identities must still be None synchronously right after cold mount"
            );
            assert!(
                !s.is_stage_search_active(cx),
                "F15-R2-D1: stage search must NOT be active while in Community"
            );
            for self_actor in ["u:4701", "b:97001", "b:97002", "  b:97001  "] {
                s.handle_community_domain_event(
                    ClientDomainEvent::CommunityEvent {
                        event_kind: "comment_created".to_string(),
                        community_id: None,
                        post_id: Some("p_cold_self".to_string()),
                        comment_id: None,
                        actor_id: Some(self_actor.to_string()),
                        payload_json: "{}".to_string(),
                        timestamp_ms: 1_700_000_010_000,
                    },
                    cx,
                );
            }
            assert_eq!(
                cw.read(cx).state.unread_notifications,
                0,
                "F15-R2-D4: cold-mount own Human & own Bots must NOT inflate unread_notifications"
            );
            // Foreign-owned bot MUST still increment unread_notifications
            s.handle_community_domain_event(
                ClientDomainEvent::CommunityEvent {
                    event_kind: "comment_created".to_string(),
                    community_id: None,
                    post_id: Some("p_cold_foreign".to_string()),
                    comment_id: None,
                    actor_id: Some("b:97999".to_string()),
                    payload_json: "{}".to_string(),
                    timestamp_ms: 1_700_000_011_000,
                },
                cx,
            );
            assert_eq!(
                cw.read(cx).state.unread_notifications,
                1,
                "F15-R2-D4: cold-mount foreign bot must still increment unread_notifications"
            );
            cw
        })
        .unwrap();

    // F15-R2-D1: Hidden stage.search_open MUST NOT hijack Backspace, Shift+Enter, Enter,
    // clear_active_field, or unfocused typing while in Community!
    let _ = cw.update(&mut app, |w, cx| w.open_quick_post_composer(cx));
    let _ = shell.update(&mut app, |s, cx| {
        s.insert_committed_text("hello", cx);
        assert_eq!(cw.read(cx).state.quick_post_body, "hello");
        assert_eq!(s.stage.read(cx).search_query, "needle");

        assert!(s.handle_non_char_key("backspace", false, cx));
        assert_eq!(
            cw.read(cx).state.quick_post_body,
            "hell",
            "F15-R2-D1: Backspace must edit focused Community QuickPostBody"
        );
        assert_eq!(
            s.stage.read(cx).search_query,
            "needle",
            "F15-R2-D1: hidden stage.search_query must NOT be mutated by Backspace in Community"
        );

        assert!(s.handle_non_char_key("enter", true, cx));
        assert_eq!(
            cw.read(cx).state.quick_post_body,
            "hell\n",
            "F15-R2-D1: Shift+Enter must insert newline in Community QuickPostBody"
        );
        assert_eq!(s.stage.read(cx).search_query, "needle");

        // Blur Community input -> unfocused typing & clear_active_field must NOT leak to hidden stage search!
        cw.update(cx, |w, wcx| w.blur_active_input(wcx));
        s.insert_committed_text("unfocused_leak", cx);
        assert_eq!(
            s.stage.read(cx).search_query,
            "needle",
            "F15-R2-D1: unfocused typing in Community must NOT leak to hidden stage.search_query"
        );
        s.clear_active_field(cx);
        assert_eq!(
            s.stage.read(cx).search_query,
            "needle",
            "F15-R2-D1: clear_active_field in unfocused Community must NOT clear hidden stage.search_query"
        );
    });

    // F15-R2-D2: Pressing Tab from focused Sidebar Search (Cmd+K) in Community must NOT focus hidden composer
    let _ = shell.update(&mut app, |s, cx| {
        assert!(s.handle_shortcut_key("k", false, cx));
        assert!(s.sidebar.read(cx).search_focused);
        assert!(s.handle_non_char_key("tab", false, cx));
        assert!(!s.sidebar.read(cx).search_focused, "Tab must exit focused sidebar search");
        assert!(
            !s.composer.read(cx).is_input_focused,
            "F15-R2-D2: Tab from sidebar search in Community must NOT focus hidden composer"
        );
        assert_eq!(s.active_panel, ActiveWorkspacePanel::Community);
    });

    // F15-R2-D3: Unfocused residual sidebar.search_query after Enter must NOT steal Escape from
    // active foreground Community popovers & inputs; subsequent Escape clears sidebar query before exit!
    let _ = shell.update(&mut app, |s, cx| {
        assert!(s.handle_shortcut_key("k", false, cx));
        s.insert_committed_text("rust", cx);
        assert!(s.handle_non_char_key("enter", false, cx));
        assert!(!s.sidebar.read(cx).search_focused);
        assert_eq!(s.sidebar.read(cx).search_query, "rust");
        assert!(!s.composer.read(cx).is_input_focused);

        cw.update(cx, |w, wcx| {
            w.open_quick_post_composer(wcx);
            w.state.active_popover = Some(CommunityPopover::AlgoTuner);
            wcx.notify();
        });

        // Esc #1: closes foreground Community popover (NOT residual unfocused sidebar search!)
        assert!(s.handle_non_char_key("escape", false, cx));
        assert!(
            cw.read(cx).state.active_popover.is_none(),
            "F15-R2-D3: Esc #1 must close foreground Community popover"
        );
        assert_eq!(
            s.sidebar.read(cx).search_query,
            "rust",
            "F15-R2-D3: residual sidebar query must stay intact until Community foreground closes"
        );
        assert_eq!(s.active_panel, ActiveWorkspacePanel::Community);

        // Esc #2: blurs focused Community input
        assert!(s.handle_non_char_key("escape", false, cx));
        assert!(
            !cw.read(cx).has_focused_text_input(),
            "F15-R2-D3: Esc #2 must blur foreground Community input"
        );
        assert_eq!(s.sidebar.read(cx).search_query, "rust");
        assert_eq!(s.active_panel, ActiveWorkspacePanel::Community);

        // Esc #3: now clears unfocused residual sidebar.search_query while staying in Community
        assert!(s.handle_non_char_key("escape", false, cx));
        assert!(
            s.sidebar.read(cx).search_query.is_empty(),
            "F15-R2-D3: Esc #3 must clear residual sidebar search query"
        );
        assert_eq!(s.active_panel, ActiveWorkspacePanel::Community);

        // Esc #4: exits Community cleanly to Chats
        assert!(s.handle_non_char_key("escape", false, cx));
        assert_eq!(s.active_panel, ActiveWorkspacePanel::Chats);
        // Back in Chats, stage search is active again and preserved untouched!
        assert!(s.is_stage_search_active(cx));
        assert_eq!(s.stage.read(cx).search_query, "needle");
    });

    // F15-R2-D5: Whitespace-only SharePostToChat summary + empty extra_caption MUST NOT emit
    // success toast or dispatch blank message!
    let _ = shell.update(&mut app, |s, cx| {
        s.open_community_workspace(cx);
        s.window_runtime.controller.current_toast = None;
        s.handle_community_action(
            CommunityWorkspaceAction::SharePostToChat {
                post_id: "p_blank_summary".to_string(),
                summary: "   \n\t  ".to_string(),
            },
            cx,
        );
        assert!(s.community_pending_share.is_some());
        s.handle_modal_outcome(
            ModalActionOutcome::ForwardToChat {
                target_chat_id: 9001,
                message_ids: Vec::new(),
                extra_caption: "   \t ".to_string(),
                show_sender: true,
            },
            cx,
        );
        assert!(
            s.community_pending_share.is_none(),
            "F15-R2-D5: pending share must still be consumed"
        );
        assert!(
            s.window_runtime.controller.current_toast.is_none(),
            "F15-R2-D5: whitespace-only summary + blank caption must NOT display success toast"
        );

        // With non-empty extra_caption, whitespace-only summary falls back cleanly to caption & shows toast
        s.handle_community_action(
            CommunityWorkspaceAction::SharePostToChat {
                post_id: "p_blank_with_cap".to_string(),
                summary: "   \n\t  ".to_string(),
            },
            cx,
        );
        s.handle_modal_outcome(
            ModalActionOutcome::ForwardToChat {
                target_chat_id: 9001,
                message_ids: Vec::new(),
                extra_caption: "Worth reading!".to_string(),
                show_sender: true,
            },
            cx,
        );
        assert!(s.community_pending_share.is_none());
        assert!(
            s.window_runtime.controller.current_toast.is_some(),
            "F15-R2-D5: non-empty extra_caption with blank summary must succeed with toast"
        );
    });

    // Verify monotonic saved_gen and mod_gen generation guards
    let _ = cw.update(&mut app, |w, cx| {
        let sg0 = w.state.saved_gen;
        w.load_saved_posts(cx);
        assert_eq!(w.state.saved_gen, sg0 + 1);
        w.load_saved_posts(cx);
        assert_eq!(w.state.saved_gen, sg0 + 2);

        let mg0 = w.state.mod_gen;
        w.load_mod_queue("rust".to_string(), cx);
        assert_eq!(w.state.mod_gen, mg0 + 1);
        w.load_mod_queue("ai".to_string(), cx);
        assert_eq!(w.state.mod_gen, mg0 + 2);
    });
}

#[test]
fn test_community_r4_bidirectional_focus_and_compose_cleanup() {
    run_in_headless_subprocess("test_community_r4_bidirectional_focus_and_compose_cleanup");
}

async fn body_r4_bidirectional_focus_and_compose_cleanup(mut app: AsyncApp) {
    let host = open_authenticated_shell(&mut app, 1440.0, 900.0, 4801, "r4_focus_compose_user");
    let shell = host.shell.clone();

    let cw = shell
        .update(&mut app, |s, cx| s.open_community_workspace(cx))
        .unwrap();
    host.draw(&mut app);

    // -------------------------------------------------------------------------
    // F15-R3-D1: Bidirectional Focus Handoff (Sidebar Search Cmd+K <-> Community Inputs)
    // -------------------------------------------------------------------------
    // 1a. Cmd+K + CJK preedit "ni" -> focus QuickPostBody -> sidebar search blurs,
    //     preedit clears, and typing routes exclusively to QuickPostBody.
    let _ = shell.update(&mut app, |s, cx| {
        assert!(s.handle_shortcut_key("k", false, cx));
        s.update_ime_marked_text("ni", cx);
        assert!(s.sidebar.read(cx).search_focused);
        assert_eq!(s.ime.ime_preedit.as_deref(), Some("ni"));
        assert!(!s.is_community_input_active(cx));
    });
    let _ = cw.update(&mut app, |w, cx| {
        w.state.focus_input(ActiveCommunityInput::QuickPostBody);
        cx.notify();
    });
    host.draw(&mut app);
    let _ = shell.update(&mut app, |s, cx| {
        assert!(
            !s.sidebar.read(cx).search_focused,
            "F15-R3-D1: clicking QuickPostBody must blur sidebar.search_focused"
        );
        assert!(
            s.ime.ime_preedit.is_none(),
            "F15-R3-D1: clicking QuickPostBody must clear stale sidebar search IME preedit"
        );
        assert!(
            s.is_community_input_active(cx),
            "F15-R3-D1: is_community_input_active must be true after focusing QuickPostBody"
        );
        s.insert_committed_text("hello community", cx);
        assert_eq!(
            cw.read(cx).state.quick_post_body,
            "hello community",
            "F15-R3-D1: typing after focusing QuickPostBody must land in QuickPostBody"
        );
        assert!(
            s.sidebar.read(cx).search_query.is_empty(),
            "F15-R3-D1: typing after focusing QuickPostBody must NOT leak into sidebar.search_query"
        );
    });

    // 1b. Repeat for RightRailSearch ("rust"): Cmd+K + preedit "wo" -> focus RightRailSearch
    let _ = shell.update(&mut app, |s, cx| {
        assert!(s.handle_shortcut_key("k", false, cx));
        s.update_ime_marked_text("wo", cx);
        assert!(s.sidebar.read(cx).search_focused);
        assert_eq!(cw.read(cx).state.active_input, ActiveCommunityInput::None);
        assert_eq!(s.ime.ime_preedit.as_deref(), Some("wo"));
    });
    let _ = cw.update(&mut app, |w, cx| {
        w.state.focus_input(ActiveCommunityInput::RightRailSearch);
        cx.notify();
    });
    host.draw(&mut app);
    let _ = shell.update(&mut app, |s, cx| {
        assert!(
            !s.sidebar.read(cx).search_focused,
            "F15-R3-D1: clicking RightRailSearch must blur sidebar.search_focused"
        );
        assert!(
            s.ime.ime_preedit.is_none(),
            "F15-R3-D1: clicking RightRailSearch must clear stale sidebar search IME preedit"
        );
        assert!(s.is_community_input_active(cx));
        s.insert_committed_text("rust", cx);
        assert_eq!(
            cw.read(cx).state.right_rail_search_draft,
            "rust",
            "F15-R3-D1: typing after focusing RightRailSearch must land in right_rail_search_draft"
        );
        assert!(
            s.sidebar.read(cx).search_query.is_empty(),
            "F15-R3-D1: sidebar.search_query must remain empty"
        );
    });

    // 1c. Mouse-style sidebar search focus -> "New Post" CTA (open_quick_post_composer)
    let _ = shell.update(&mut app, |s, cx| {
        s.sidebar.update(cx, |sb, scx| {
            sb.focus_search();
            scx.notify();
        });
    });
    host.draw(&mut app);
    let _ = shell.update(&mut app, |s, cx| {
        assert!(s.sidebar.read(cx).search_focused);
        assert_eq!(cw.read(cx).state.active_input, ActiveCommunityInput::None);
        s.update_ime_marked_text("hao", cx);
    });
    let _ = cw.update(&mut app, |w, cx| w.open_quick_post_composer(cx));
    host.draw(&mut app);
    let _ = shell.update(&mut app, |s, cx| {
        assert!(!s.sidebar.read(cx).search_focused);
        assert!(s.ime.ime_preedit.is_none());
        assert!(s.is_community_input_active(cx));
        s.insert_committed_text("!", cx);
        assert_eq!(cw.read(cx).state.quick_post_body, "hello community!");
        assert!(s.sidebar.read(cx).search_query.is_empty());
    });

    // 1d. Same-turn Cmd+K + immediate CJK preedit while QuickPostBody was focused must NOT wipe
    //     sidebar search's brand-new preedit when CommunityWorkspace's blur observer flushes.
    let _ = shell.update(&mut app, |s, cx| {
        assert!(s.handle_shortcut_key("k", false, cx));
        s.update_ime_marked_text("zhong", cx);
    });
    host.draw(&mut app);
    let _ = shell.update(&mut app, |s, cx| {
        assert!(s.sidebar.read(cx).search_focused);
        assert_eq!(
            s.ime.ime_preedit.as_deref(),
            Some("zhong"),
            "delayed Community blur observer must not wipe brand-new sidebar search preedit"
        );
        assert!(s.handle_non_char_key("escape", false, cx));
        assert!(s.ime.ime_preedit.is_none());
        assert!(s.handle_non_char_key("escape", false, cx));
        assert!(!s.sidebar.read(cx).search_focused);
    });

    // -------------------------------------------------------------------------
    // F15-R3-D2: Chats Cmd+N (Plus Menu + Compose Isolation) -> Cmd+Shift+C cleanup
    // Verify BOTH Apple style (Night) AND Tdesktop style (Classic)!
    // -------------------------------------------------------------------------
    for mode in [TdesktopThemeMode::Night, TdesktopThemeMode::Classic] {
        let _ = shell.update(&mut app, |s, cx| {
            s.apply_theme_selection(mode, None, None, cx);
            s.activate_nav_destination(NavDestination::Chats, cx);
            assert_eq!(s.active_panel, ActiveWorkspacePanel::Chats);

            // Enter Cmd+N compose / plus-menu mode in Chats
            assert!(s.handle_shortcut_key("n", false, cx));
            assert!(s.sidebar.read(cx).is_plus_menu_open);
            assert_eq!(
                s.composer.read(cx).is_compose_isolated,
                s.theme_state.palette.is_apple_style(),
                "F15-R3-D2 ({mode:?}): Apple Cmd+N isolates composer; Classic preserves C1-I7-6 non-isolation"
            );
            if s.theme_state.palette.is_apple_style() {
                s.sidebar.update(cx, |sb, scx| {
                    sb.compose_recipient_query = "alice".to_string();
                    scx.notify();
                });
            }
        });
        host.draw(&mut app);

        // Press Cmd+Shift+C -> opens Community AND cancels compose mode + exits compose isolation
        let _ = shell.update(&mut app, |s, cx| {
            assert!(s.handle_shortcut_key("c", true, cx));
            assert_eq!(s.active_panel, ActiveWorkspacePanel::Community);
            assert!(
                !s.sidebar.read(cx).is_plus_menu_open,
                "F15-R3-D2 ({mode:?}): Cmd+Shift+C must close sidebar.is_plus_menu_open"
            );
            assert!(
                s.sidebar.read(cx).compose_recipient_query.is_empty(),
                "F15-R3-D2 ({mode:?}): Cmd+Shift+C must clear compose_recipient_query"
            );
            assert!(
                !s.composer.read(cx).is_compose_isolated,
                "F15-R3-D2 ({mode:?}): Cmd+Shift+C must exit composer.is_compose_isolated"
            );
            assert!(
                !s.composer.read(cx).is_input_focused,
                "F15-R3-D2 ({mode:?}): Cmd+Shift+C must keep hidden composer unfocused"
            );
        });
        host.draw(&mut app);

        // Post-observer-flush invariant check: hidden composer stays unfocused!
        let _ = shell.update(&mut app, |s, cx| {
            assert!(!s.sidebar.read(cx).is_plus_menu_open);
            assert!(!s.composer.read(cx).is_compose_isolated);
            assert!(
                !s.composer.read(cx).is_input_focused,
                "F15-R3-D2 ({mode:?}): sidebar observer flush must NOT re-focus hidden composer"
            );
        });

        // Open AlgoTuner -> Escape #1 MUST close AlgoTuner (not stale plus menu!) while staying in Community
        let _ = cw.update(&mut app, |w, cx| {
            w.state.active_popover = Some(CommunityPopover::AlgoTuner);
            cx.notify();
        });
        host.draw(&mut app);
        let _ = shell.update(&mut app, |s, cx| {
            assert!(s.handle_non_char_key("escape", false, cx));
            assert!(
                cw.read(cx).state.active_popover.is_none(),
                "F15-R3-D2 ({mode:?}): Esc #1 must close AlgoTuner cleanly"
            );
            assert_eq!(
                s.active_panel,
                ActiveWorkspacePanel::Community,
                "F15-R3-D2 ({mode:?}): Esc #1 must stay in Community"
            );
            assert!(!s.composer.read(cx).is_input_focused);
        });
    }
}

#[test]
fn test_community_r5_shell_occlusion_compose_modal_and_remote_hub_cleanup() {
    run_in_headless_subprocess(
        "test_community_r5_shell_occlusion_compose_modal_and_remote_hub_cleanup",
    );
}

async fn body_r5_shell_occlusion_compose_modal_and_remote_hub_cleanup(mut app: AsyncApp) {
    let host = open_authenticated_shell(&mut app, 1440.0, 900.0, 4901, "r5_shell_occlusion_user");
    let shell = host.shell.clone();

    let cw = shell
        .update(&mut app, |s, cx| s.open_community_workspace(cx))
        .unwrap();
    host.draw(&mut app);

    // -------------------------------------------------------------------------
    // F15-R4-D1: Persistent Sidebar "New Chat" (TogglePlusMenu) while in Community
    // -------------------------------------------------------------------------
    // 1a. Apple Night style: switches cleanly to Chats, clears preedit, blurs Community & closes AlgoTuner.
    let _ = shell.update(&mut app, |s, cx| {
        s.apply_theme_selection(TdesktopThemeMode::Night, None, None, cx);
        assert_eq!(s.active_panel, ActiveWorkspacePanel::Community);
    });
    let _ = cw.update(&mut app, |w, cx| {
        w.state.focus_input(ActiveCommunityInput::QuickPostBody);
        w.state.active_popover = Some(CommunityPopover::AlgoTuner);
        cx.notify();
    });
    host.draw(&mut app);
    let _ = shell.update(&mut app, |s, cx| {
        s.update_ime_marked_text("ni", cx);
        assert_eq!(s.ime.ime_preedit.as_deref(), Some("ni"));
        assert!(s.is_community_input_active(cx));
        assert_eq!(cw.read(cx).state.active_popover, Some(CommunityPopover::AlgoTuner));
        s.sidebar.update(cx, |sb, scx| {
            sb.handle_unified_action(UnifiedSidebarAction::TogglePlusMenu, scx);
        });
    });
    host.draw(&mut app);
    let _ = shell.update(&mut app, |s, cx| {
        assert_eq!(
            s.active_panel,
            ActiveWorkspacePanel::Chats,
            "F15-R4-D1 (Night): TogglePlusMenu in Community must switch active_panel to Chats"
        );
        assert_eq!(s.nav_rail.read(cx).active_destination, NavDestination::Chats);
        assert!(s.sidebar.read(cx).is_plus_menu_open);
        assert!(s.composer.read(cx).is_compose_isolated);
        assert!(!s.composer.read(cx).is_input_focused);
        assert!(
            s.ime.ime_preedit.is_none(),
            "F15-R4-D1 (Night): TogglePlusMenu must clear Community IME preedit"
        );
        assert!(
            !cw.read(cx).has_focused_text_input(),
            "F15-R4-D1 (Night): TogglePlusMenu must blur Community input"
        );
        assert!(
            cw.read(cx).state.active_popover.is_none(),
            "F15-R4-D1 (Night): TogglePlusMenu must close open Community AlgoTuner popover"
        );
    });

    // Return to Community -> Escape #1 must close AlgoTuner without any invisible compose hijack
    let _ = shell.update(&mut app, |s, cx| {
        assert!(s.handle_shortcut_key("c", true, cx));
        assert_eq!(s.active_panel, ActiveWorkspacePanel::Community);
    });
    host.draw(&mut app);
    let _ = cw.update(&mut app, |w, cx| {
        w.state.active_popover = Some(CommunityPopover::AlgoTuner);
        cx.notify();
    });
    host.draw(&mut app);
    let _ = shell.update(&mut app, |s, cx| {
        assert!(s.handle_non_char_key("escape", false, cx));
        assert!(cw.read(cx).state.active_popover.is_none());
        assert_eq!(s.active_panel, ActiveWorkspacePanel::Community);
        assert!(!s.composer.read(cx).is_input_focused);
    });

    // 1b. Tdesktop Classic style: floating popup dropdown blurs Community & clears preedit cleanly.
    let _ = shell.update(&mut app, |s, cx| {
        s.apply_theme_selection(TdesktopThemeMode::Classic, None, None, cx);
        assert_eq!(s.active_panel, ActiveWorkspacePanel::Community);
    });
    let _ = cw.update(&mut app, |w, cx| {
        w.state.focus_input(ActiveCommunityInput::QuickPostBody);
        cx.notify();
    });
    host.draw(&mut app);
    let _ = shell.update(&mut app, |s, cx| {
        s.update_ime_marked_text("ni", cx);
        assert_eq!(s.ime.ime_preedit.as_deref(), Some("ni"));
        assert!(s.is_community_input_active(cx));
        s.sidebar.update(cx, |sb, scx| {
            sb.handle_unified_action(UnifiedSidebarAction::TogglePlusMenu, scx);
        });
    });
    host.draw(&mut app);
    let _ = shell.update(&mut app, |s, cx| {
        assert_eq!(s.active_panel, ActiveWorkspacePanel::Community);
        assert!(s.sidebar.read(cx).is_plus_menu_open);
        assert!(
            s.ime.ime_preedit.is_none(),
            "F15-R4-D1 (Classic): Classic plus-menu dropdown must clear Community preedit"
        );
        assert!(
            !cw.read(cx).has_focused_text_input(),
            "F15-R4-D1 (Classic): Classic plus-menu dropdown must blur Community input"
        );
        assert!(s.has_community_visual_occlusion_overlay(cx));
        assert!(!s.is_community_input_active(cx));
        assert!(s.handle_non_char_key("escape", false, cx));
        assert!(!s.sidebar.read(cx).is_plus_menu_open);
        assert!(!s.composer.read(cx).is_input_focused);
        assert!(!s.has_community_visual_occlusion_overlay(cx));
    });
    host.draw(&mut app);

    // -------------------------------------------------------------------------
    // F15-R4-D2: Dismissing Creation / Contacts modals via Escape & Mouse Close
    //            in BOTH Night and Classic must NEVER focus hidden Chats composer
    // -------------------------------------------------------------------------
    for mode in [TdesktopThemeMode::Night, TdesktopThemeMode::Classic] {
        let _ = shell.update(&mut app, |s, cx| {
            s.apply_theme_selection(mode, None, None, cx);
            assert_eq!(s.active_panel, ActiveWorkspacePanel::Community);
            // 2a. Cmd+J Contacts modal -> Escape
            assert!(s.handle_shortcut_key("j", false, cx));
            assert!(s.modal_container.read(cx).is_modal_active());
            assert!(s.composer.read(cx).is_compose_isolated);
        });
        host.draw(&mut app);
        let _ = shell.update(&mut app, |s, cx| {
            assert!(s.handle_non_char_key("escape", false, cx));
            assert!(!s.modal_container.read(cx).is_modal_active());
            assert!(!s.composer.read(cx).is_compose_isolated);
            assert!(
                !s.composer.read(cx).is_input_focused,
                "F15-R4-D2 ({mode:?}): Cmd+J Esc must NOT focus hidden composer synchronously"
            );
        });
        host.draw(&mut app);
        let _ = shell.update(&mut app, |s, cx| {
            assert!(
                !s.composer.read(cx).is_input_focused,
                "F15-R4-D2 ({mode:?}): Cmd+J Esc must NOT focus hidden composer post-flush"
            );
            // Clicking sidebar search immediately after must NOT be clobbered by composer observer!
            s.sidebar.update(cx, |sb, scx| {
                sb.handle_unified_action(UnifiedSidebarAction::FocusSearch, scx);
            });
        });
        host.draw(&mut app);
        let _ = shell.update(&mut app, |s, cx| {
            assert!(
                s.sidebar.read(cx).search_focused,
                "F15-R4-D2 ({mode:?}): FocusSearch after Cmd+J Esc must stay focused"
            );
            assert!(!s.composer.read(cx).is_input_focused);
            s.sidebar.update(cx, |sb, scx| {
                sb.exit_search();
                scx.notify();
            });
        });
        host.draw(&mut app);

        // 2b. Cmd+J Contacts modal -> Mouse close (dismiss_top)
        let _ = shell.update(&mut app, |s, cx| {
            assert!(s.handle_shortcut_key("j", false, cx));
        });
        host.draw(&mut app);
        let _ = shell.update(&mut app, |s, cx| {
            s.modal_container.update(cx, |mc, mcx| {
                mc.dismiss_top();
                mcx.notify();
            });
        });
        host.draw(&mut app);
        let _ = shell.update(&mut app, |s, cx| {
            assert!(!s.modal_container.read(cx).is_modal_active());
            assert!(!s.composer.read(cx).is_compose_isolated);
            assert!(
                !s.composer.read(cx).is_input_focused,
                "F15-R4-D2 ({mode:?}): Cmd+J mouse close must NOT focus hidden composer"
            );
        });

        // 2c. Sidebar creation modal (open_add_contact_modal) -> Escape (isolated + unisolated) & Mouse close
        let _ = shell.update(&mut app, |s, cx| {
            s.sidebar.update(cx, |sb, scx| {
                sb.open_add_contact_modal(AddContactKind::Friend);
                scx.notify();
            });
        });
        host.draw(&mut app);
        let _ = shell.update(&mut app, |s, cx| {
            assert!(s.handle_non_char_key("escape", false, cx));
            assert!(
                !s.composer.read(cx).is_input_focused,
                "F15-R4-D2 ({mode:?}): creation modal Esc (isolated) must NOT focus hidden composer"
            );
        });
        host.draw(&mut app);
        let _ = shell.update(&mut app, |s, cx| {
            assert!(!s.composer.read(cx).is_input_focused);
            // Same-turn open + Esc before observer flush hits shell_input_ime.rs:624 else-branch
            s.sidebar.update(cx, |sb, scx| {
                sb.open_add_contact_modal(AddContactKind::Friend);
                scx.notify();
            });
            assert!(s.handle_non_char_key("escape", false, cx));
            assert!(
                !s.composer.read(cx).is_input_focused,
                "F15-R4-D2 ({mode:?}): same-turn creation modal Esc must NOT focus hidden composer"
            );
        });
        host.draw(&mut app);
        let _ = shell.update(&mut app, |s, cx| {
            assert!(!s.composer.read(cx).is_input_focused);
            s.sidebar.update(cx, |sb, scx| {
                sb.open_add_contact_modal(AddContactKind::Friend);
                scx.notify();
            });
        });
        host.draw(&mut app);
        let _ = shell.update(&mut app, |s, cx| {
            s.sidebar.update(cx, |sb, scx| {
                sb.close_creation_modal();
                scx.notify();
            });
        });
        host.draw(&mut app);
        let _ = shell.update(&mut app, |s, cx| {
            assert!(
                !s.composer.read(cx).is_input_focused,
                "F15-R4-D2 ({mode:?}): creation modal mouse close must NOT focus hidden composer"
            );
        });
    }

    // -------------------------------------------------------------------------
    // F15-R4-D3: open_remote_computers_hub leaves Community cleanly
    // -------------------------------------------------------------------------
    let _ = shell.update(&mut app, |s, cx| {
        s.apply_theme_selection(TdesktopThemeMode::Night, None, None, cx);
        assert_eq!(s.active_panel, ActiveWorkspacePanel::Community);
    });
    let _ = cw.update(&mut app, |w, cx| {
        w.state.focus_input(ActiveCommunityInput::QuickPostBody);
        w.state.active_popover = Some(CommunityPopover::AlgoTuner);
        cx.notify();
    });
    host.draw(&mut app);
    let _ = shell.update(&mut app, |s, cx| {
        s.update_ime_marked_text("remote_pre", cx);
        assert_eq!(s.ime.ime_preedit.as_deref(), Some("remote_pre"));
        assert!(cw.read(cx).has_focused_text_input());
        assert_eq!(cw.read(cx).state.active_popover, Some(CommunityPopover::AlgoTuner));
        s.open_remote_computers_hub(cx);
    });
    host.draw(&mut app);
    let _ = shell.update(&mut app, |s, cx| {
        assert_eq!(s.active_panel, ActiveWorkspacePanel::Settings);
        assert!(
            s.ime.ime_preedit.is_none(),
            "F15-R4-D3: open_remote_computers_hub must clear Community IME preedit"
        );
        assert!(
            !cw.read(cx).has_focused_text_input(),
            "F15-R4-D3: open_remote_computers_hub must blur Community text input"
        );
        assert!(
            cw.read(cx).state.active_popover.is_none(),
            "F15-R4-D3: open_remote_computers_hub must close open Community popovers"
        );
    });

    // -------------------------------------------------------------------------
    // F16-R4-D1: Occluded None -> QuickPostBody under ModalContainer & MediaViewer
    //            must preserve overlay preedit/search AND blur Community immediately
    // -------------------------------------------------------------------------
    let _ = shell.update(&mut app, |s, cx| {
        s.open_community_workspace(cx);
        assert_eq!(s.active_panel, ActiveWorkspacePanel::Community);
    });
    host.draw(&mut app);

    // 4a. ModalContainer with live CJK preedit "modal_preedit" (community_ime_preedit_input_gen == None)
    let _ = shell.update(&mut app, |s, cx| {
        s.open_modal_dialog("New Channel", ModalDialogKind::NewChannel, 420.0, 380.0, cx);
        assert!(s.modal_container.read(cx).is_modal_active());
        s.update_ime_marked_text("modal_preedit", cx);
        assert_eq!(s.ime.ime_preedit.as_deref(), Some("modal_preedit"));
        assert!(s.community_ime_preedit_input_gen.is_none());
        assert_eq!(cw.read(cx).state.active_input, ActiveCommunityInput::None);
    });
    let _ = cw.update(&mut app, |w, cx| {
        w.state.focus_input(ActiveCommunityInput::QuickPostBody);
        cx.notify();
    });
    host.draw(&mut app);
    let _ = shell.update(&mut app, |s, cx| {
        assert_eq!(
            s.ime.ime_preedit.as_deref(),
            Some("modal_preedit"),
            "F16-R4-D1: occluded QuickPostBody focus must preserve ModalContainer CJK preedit"
        );
        assert_eq!(
            cw.read(cx).state.active_input,
            ActiveCommunityInput::None,
            "F16-R4-D1: occluded QuickPostBody focus must be immediately blurred back to None"
        );
        assert!(!s.is_community_input_active(cx));
        // Esc #1 clears modal preedit, Esc #2 dismisses ModalContainer
        assert!(s.handle_non_char_key("escape", false, cx));
        assert!(s.ime.ime_preedit.is_none());
        assert!(s.modal_container.read(cx).is_modal_active());
        assert!(s.handle_non_char_key("escape", false, cx));
        assert!(!s.modal_container.read(cx).is_modal_active());
        assert_eq!(
            cw.read(cx).state.active_input,
            ActiveCommunityInput::None,
            "F16-R4-D1: dismissing ModalContainer must NOT trigger delayed QuickPostBody focus hijack"
        );
        assert!(!s.is_community_input_active(cx));
        assert!(!s.composer.read(cx).is_input_focused);
    });
    host.draw(&mut app);

    // 4b. Fullscreen Z #3 MediaViewer is ShellInputTarget::Absorbed (upstream C1-I6-3:
    //     zero phantom ime_preedit, occluded QuickPostBody blurs immediately, 1-Esc dismiss)
    let _ = shell.update(&mut app, |s, cx| {
        s.show_media_viewer("https://cdn.example/r5_occlusion.png", false, cx);
        assert!(s.overlays.media_viewer_overlay.is_some());
        s.sidebar.update(cx, |sb, scx| {
            sb.handle_unified_action(UnifiedSidebarAction::FocusSearch, scx);
        });
    });
    host.draw(&mut app);
    let _ = shell.update(&mut app, |s, cx| {
        s.update_ime_marked_text("mv_search_pre", cx);
        assert!(s.sidebar.read(cx).search_focused);
        assert!(
            s.ime.ime_preedit.is_none(),
            "C1-I6-3: Z #3 MediaViewer absorbs IME with zero phantom preedit"
        );
        assert!(s.community_ime_preedit_input_gen.is_none());
    });
    let _ = cw.update(&mut app, |w, cx| {
        w.state.focus_input(ActiveCommunityInput::QuickPostBody);
        cx.notify();
    });
    host.draw(&mut app);
    let _ = shell.update(&mut app, |s, cx| {
        assert!(
            s.sidebar.read(cx).search_focused,
            "F16-R4-D1: occluded QuickPostBody focus must preserve sidebar.search_focused under MediaViewer"
        );
        assert!(s.ime.ime_preedit.is_none());
        assert_eq!(
            cw.read(cx).state.active_input,
            ActiveCommunityInput::None,
            "F16-R4-D1: occluded QuickPostBody focus under MediaViewer must blur immediately"
        );
        assert!(!s.is_community_input_active(cx));
        // 1st Escape immediately dismisses MediaViewer (no phantom preedit step)
        assert!(s.handle_non_char_key("escape", false, cx));
        assert!(s.overlays.media_viewer_overlay.is_none());
        assert_eq!(
            cw.read(cx).state.active_input,
            ActiveCommunityInput::None,
            "F16-R4-D1: dismissing MediaViewer must NOT trigger delayed QuickPostBody focus hijack"
        );
        assert!(!s.is_community_input_active(cx));
    });
    host.draw(&mut app);

    // 4c. Z #14 PublishReviewModal over Community preserves live modal CJK preedit & blurs Community
    let _ = shell.update(&mut app, |s, cx| {
        s.sidebar.update(cx, |sb, scx| {
            sb.exit_search();
            scx.notify();
        });
        s.open_reviewer_queue_modal(cx);
        assert!(s.is_publish_review_modal_active(cx));
    });
    host.draw(&mut app);
    let _ = shell.update(&mut app, |s, cx| {
        s.content_hub.update(cx, |h, hcx| {
            h.publish_modal.focused_field = Some(
                gpui_telegram_desktop::views::content_hub::publish_review_modal::PublishModalField::ReviewNotes,
            );
            hcx.notify();
        });
        s.update_ime_marked_text("pub_pre", cx);
        assert_eq!(s.ime.ime_preedit.as_deref(), Some("pub_pre"));
        assert!(s.community_ime_preedit_input_gen.is_none());
    });
    let _ = cw.update(&mut app, |w, cx| {
        w.state.focus_input(ActiveCommunityInput::QuickPostBody);
        cx.notify();
    });
    host.draw(&mut app);
    let _ = shell.update(&mut app, |s, cx| {
        assert_eq!(
            s.ime.ime_preedit.as_deref(),
            Some("pub_pre"),
            "F16-R4-D1: occluded QuickPostBody focus must preserve PublishReviewModal CJK preedit"
        );
        assert_eq!(cw.read(cx).state.active_input, ActiveCommunityInput::None);
        assert!(!s.is_community_input_active(cx));
        // Esc #1 clears modal preedit, Esc #2 closes PublishReviewModal
        assert!(s.handle_non_char_key("escape", false, cx));
        assert!(s.ime.ime_preedit.is_none());
        assert!(s.is_publish_review_modal_active(cx));
        assert!(s.handle_non_char_key("escape", false, cx));
        assert!(!s.is_publish_review_modal_active(cx));
        assert_eq!(cw.read(cx).state.active_input, ActiveCommunityInput::None);
    });
}

#[test]
fn test_community_r6_theme_plus_focus_search_unfocused_ime_switch_and_talkmode() {
    run_in_headless_subprocess(
        "test_community_r6_theme_plus_focus_search_unfocused_ime_switch_and_talkmode",
    );
}

async fn body_r6_theme_plus_focus_search_unfocused_ime_switch_and_talkmode(mut app: AsyncApp) {
    let host = open_authenticated_shell(&mut app, 1440.0, 900.0, 5001, "r6_user_one");
    let shell = host.shell.clone();

    let cw = shell
        .update(&mut app, |s, cx| s.open_community_workspace(cx))
        .unwrap();
    host.draw(&mut app);

    // -------------------------------------------------------------------------
    // F15-R5-D1: Classic '+' dropdown in Community -> Cmd+D & ToggleTheme to Night
    //            must dismiss '+' cleanly without invisible Apple Compose hijacking Escape
    // -------------------------------------------------------------------------
    for via_toggle_theme in [false, true] {
        let label = if via_toggle_theme { "ToggleTheme" } else { "Cmd+D" };
        let _ = shell.update(&mut app, |s, cx| {
            s.apply_theme_selection(TdesktopThemeMode::Classic, None, None, cx);
            assert_eq!(s.active_panel, ActiveWorkspacePanel::Community);
            assert!(!s.theme_state.palette.is_apple_style());
            s.sidebar.update(cx, |sb, scx| {
                sb.handle_unified_action(UnifiedSidebarAction::TogglePlusMenu, scx);
            });
        });
        host.draw(&mut app);
        let _ = cw.update(&mut app, |w, cx| {
            w.state.active_popover = Some(CommunityPopover::AlgoTuner);
            cx.notify();
        });
        host.draw(&mut app);
        let _ = shell.update(&mut app, |s, cx| {
            assert!(s.sidebar.read(cx).is_plus_menu_open);
            assert!(s.has_community_visual_occlusion_overlay(cx));
            if via_toggle_theme {
                s.sidebar.update(cx, |sb, scx| {
                    sb.handle_unified_action(UnifiedSidebarAction::ToggleTheme, scx);
                });
            } else {
                assert!(s.handle_shortcut_key("d", false, cx));
            }
        });
        host.draw(&mut app);
        let _ = shell.update(&mut app, |s, cx| {
            assert_eq!(
                s.active_panel,
                ActiveWorkspacePanel::Community,
                "F15-R5-D1 ({label}): switching Classic->Night must stay in Community"
            );
            assert!(
                s.theme_state.palette.is_apple_style(),
                "F15-R5-D1 ({label}): theme must now be Apple style"
            );
            assert!(
                !s.sidebar.read(cx).is_plus_menu_open,
                "F15-R5-D1 ({label}): Classic plus-menu must be dismissed on Apple theme switch"
            );
            assert!(
                !s.composer.read(cx).is_compose_isolated,
                "F15-R5-D1 ({label}): hidden composer must NOT enter compose isolation"
            );
            assert!(
                !s.composer.read(cx).is_input_focused,
                "F15-R5-D1 ({label}): hidden composer must remain unfocused"
            );
            assert!(!s.has_community_visual_occlusion_overlay(cx));
            // Escape #1 MUST close AlgoTuner directly (NOT hijacked by invisible Apple Compose)
            assert!(s.handle_non_char_key("escape", false, cx));
            assert!(
                cw.read(cx).state.active_popover.is_none(),
                "F15-R5-D1 ({label}): Esc #1 must close AlgoTuner cleanly"
            );
            assert_eq!(s.active_panel, ActiveWorkspacePanel::Community);
            assert!(!s.composer.read(cx).is_input_focused);
        });
        host.draw(&mut app);
    }

    // -------------------------------------------------------------------------
    // F15-R5-D2: Mouse-clicking #sidebar-search-box (FocusSearch) from focused Community
    //            input + same-turn CJK preedit "zhong" must preserve "zhong" while blurring
    //            Community; without same-turn preedit, stale Community preedit "ni" clears.
    // -------------------------------------------------------------------------
    let _ = cw.update(&mut app, |w, cx| {
        w.state.focus_input(ActiveCommunityInput::QuickPostBody);
        cx.notify();
    });
    host.draw(&mut app);
    let _ = shell.update(&mut app, |s, cx| {
        s.update_ime_marked_text("ni", cx);
        assert_eq!(s.ime.ime_preedit.as_deref(), Some("ni"));
        assert!(s.community_ime_preedit_input_gen.is_some());
        s.sidebar.update(cx, |sb, scx| {
            sb.handle_unified_action(UnifiedSidebarAction::FocusSearch, scx);
        });
        s.update_ime_marked_text("zhong", cx);
    });
    host.draw(&mut app);
    let _ = shell.update(&mut app, |s, cx| {
        assert!(s.sidebar.read(cx).search_focused);
        assert!(
            !cw.read(cx).has_focused_text_input(),
            "F15-R5-D2: FocusSearch must blur Community input"
        );
        assert_eq!(
            s.ime.ime_preedit.as_deref(),
            Some("zhong"),
            "F15-R5-D2: FocusSearch + same-turn preedit must preserve fresh Sidebar Search preedit"
        );
        assert!(cw.read(cx).state.ime_preedit.is_none());
        assert!(s.handle_non_char_key("escape", false, cx));
        assert!(s.ime.ime_preedit.is_none());
        assert!(s.handle_non_char_key("escape", false, cx));
        assert!(!s.sidebar.read(cx).search_focused);
    });
    host.draw(&mut app);

    // Without same-turn preedit, stale Community preedit "ni" MUST still clear on FocusSearch
    let _ = cw.update(&mut app, |w, cx| {
        w.state.focus_input(ActiveCommunityInput::QuickPostBody);
        cx.notify();
    });
    host.draw(&mut app);
    let _ = shell.update(&mut app, |s, cx| {
        s.update_ime_marked_text("ni", cx);
        assert!(s.community_ime_preedit_input_gen.is_some());
        s.sidebar.update(cx, |sb, scx| {
            sb.handle_unified_action(UnifiedSidebarAction::FocusSearch, scx);
        });
    });
    host.draw(&mut app);
    let _ = shell.update(&mut app, |s, cx| {
        assert!(s.sidebar.read(cx).search_focused);
        assert!(!cw.read(cx).has_focused_text_input());
        assert!(
            s.ime.ime_preedit.is_none(),
            "F15-R5-D2: stale Community preedit without new search preedit must clear"
        );
        assert!(cw.read(cx).state.ime_preedit.is_none());
        assert!(s.handle_non_char_key("escape", false, cx));
        assert!(!s.sidebar.read(cx).search_focused);
    });
    host.draw(&mut app);

    // -------------------------------------------------------------------------
    // F15-R5-D3: Unfocused Community must NOT leak hidden Chats composer draft in
    //            active_text_and_preedit, and update_ime_marked_text("nihao") must NOT
    //            write an orphaned ime_preedit that swallows Escape over AlgoTuner.
    // -------------------------------------------------------------------------
    let _ = cw.update(&mut app, |w, cx| {
        w.blur_active_input(cx);
        w.state.active_popover = Some(CommunityPopover::AlgoTuner);
        cx.notify();
    });
    host.draw(&mut app);
    let _ = shell.update(&mut app, |s, cx| {
        s.composer.update(cx, |c, ccx| {
            c.set_text("secret_chat_draft");
            ccx.notify();
        });
        assert_eq!(s.active_panel, ActiveWorkspacePanel::Community);
        assert!(s.should_hide_composer(cx));
        assert!(!cw.read(cx).has_focused_text_input());
        assert!(!s.sidebar.read(cx).search_focused);
        assert_eq!(
            s.active_text_and_preedit(cx),
            (String::new(), None),
            "F15-R5-D3: unfocused Community must NOT leak hidden Chats composer draft"
        );
        s.update_ime_marked_text("nihao", cx);
        assert!(
            s.ime.ime_preedit.is_none(),
            "F15-R5-D3: unfocused Community must NOT write orphaned ime_preedit"
        );
        assert_eq!(s.active_text_and_preedit(cx), (String::new(), None));
        assert_eq!(s.composer.read(cx).text(), "secret_chat_draft");
        assert!(!s.composer.read(cx).ime_machine.is_composing());
        assert!(s.handle_non_char_key("escape", false, cx));
        assert!(
            cw.read(cx).state.active_popover.is_none(),
            "F15-R5-D3: Esc #1 must close AlgoTuner directly instead of being swallowed"
        );
        assert_eq!(s.active_panel, ActiveWorkspacePanel::Community);
    });
    host.draw(&mut app);

    // -------------------------------------------------------------------------
    // F15-R5-D5: Cmd+6 TalkMode from Community dismissed via BOTH Escape (hangup_active_call)
    //            and overlay close button (hide_talk_mode) restores nav_rail == Community.
    // -------------------------------------------------------------------------
    // 5a. Escape path (hangup_active_call)
    let _ = shell.update(&mut app, |s, cx| {
        assert_eq!(s.active_panel, ActiveWorkspacePanel::Community);
        assert_eq!(s.nav_rail.read(cx).active_destination, NavDestination::Community);
        assert!(s.handle_shortcut_key("6", false, cx));
        assert!(s.rtc_talk.talk_mode_overlay.is_some());
        assert_eq!(s.nav_rail.read(cx).active_destination, NavDestination::TalkMode);
        assert_eq!(s.active_panel, ActiveWorkspacePanel::Community);
    });
    host.draw(&mut app);
    let _ = shell.update(&mut app, |s, cx| {
        assert!(s.handle_non_char_key("escape", false, cx));
        assert!(s.rtc_talk.talk_mode_overlay.is_none());
        assert_eq!(s.active_panel, ActiveWorkspacePanel::Community);
        assert_eq!(
            s.nav_rail.read(cx).active_destination,
            NavDestination::Community,
            "F15-R5-D5: TalkMode Escape from Community must restore nav_rail to Community"
        );
        assert_eq!(s.last_handled_nav_dest, Some(NavDestination::Community));
    });
    host.draw(&mut app);

    // 5b. Overlay close button path (should_close -> hide_talk_mode)
    let _ = shell.update(&mut app, |s, cx| {
        assert!(s.handle_shortcut_key("6", false, cx));
        assert!(s.rtc_talk.talk_mode_overlay.is_some());
        assert_eq!(s.nav_rail.read(cx).active_destination, NavDestination::TalkMode);
        if let Some(ov) = s.rtc_talk.talk_mode_overlay.clone() {
            ov.update(cx, |o, ocx| {
                o.should_close = true;
                ocx.notify();
            });
        }
    });
    host.draw(&mut app);
    let _ = shell.update(&mut app, |s, cx| {
        assert!(s.rtc_talk.talk_mode_overlay.is_none());
        assert_eq!(s.active_panel, ActiveWorkspacePanel::Community);
        assert_eq!(
            s.nav_rail.read(cx).active_destination,
            NavDestination::Community,
            "F15-R5-D5: TalkMode close button from Community must restore nav_rail to Community"
        );
        assert_eq!(s.last_handled_nav_dest, Some(NavDestination::Community));
    });
    host.draw(&mut app);

    // -------------------------------------------------------------------------
    // F15-R5-D4: User switch while in Community with a queued nav_rail notification
    //            (set_community_unread_badge) must reset nav_rail to Chats synchronously
    //            and NEVER resurrect community_workspace on observer flush.
    // -------------------------------------------------------------------------
    let _ = shell.update(&mut app, |s, cx| {
        assert_eq!(s.active_panel, ActiveWorkspacePanel::Community);
        assert_eq!(s.nav_rail.read(cx).active_destination, NavDestination::Community);
        s.set_community_unread_badge(5, cx);
        s.apply_authenticated_session_with_vault(
            test_auth(5002, "r6_user_two"),
            None,
            cx,
        );
        assert!(
            s.community_workspace.is_none(),
            "F15-R5-D4: user switch must clear community_workspace synchronously"
        );
        assert_eq!(s.active_panel, ActiveWorkspacePanel::Chats);
        assert_eq!(
            s.nav_rail.read(cx).active_destination,
            NavDestination::Chats,
            "F15-R5-D4: user switch must reset nav_rail to Chats synchronously"
        );
    });
    host.draw(&mut app);
    let _ = shell.update(&mut app, |s, cx| {
        assert!(
            s.community_workspace.is_none(),
            "F15-R5-D4: queued nav_rail notification must NOT resurrect community_workspace post-flush"
        );
        assert_eq!(s.active_panel, ActiveWorkspacePanel::Chats);
        assert_eq!(s.nav_rail.read(cx).active_destination, NavDestination::Chats);
        let comm_badge = s
            .nav_rail
            .read(cx)
            .items
            .iter()
            .find(|i| i.destination == NavDestination::Community)
            .unwrap()
            .unread_badge;
        assert_eq!(comm_badge, 0, "F15-R5-D4: Community unread badge must reset to 0 on user switch");
    });
}



#[test]
fn test_community_r7_composer_ime_carryover_nav_sync_and_load_more_supersede() {
    run_in_headless_subprocess(
        "test_community_r7_composer_ime_carryover_nav_sync_and_load_more_supersede",
    );
}

async fn body_r7_composer_ime_carryover_nav_sync_and_load_more_supersede(mut app: AsyncApp) {
    let host = open_authenticated_shell(&mut app, 1440.0, 900.0, 5101, "r7_remediation_user");
    let shell = host.shell.clone();

    // 1. F15-R6-D1 / R6-REV1-D2: Chats composer active CJK IME preedit + bot_ac_visible
    //    -> Cmd+Shift+C Community cancels preedit & dismisses autocomplete;
    //    even if hidden composer ime_machine/autocomplete are re-armed, 1st Escape closes
    //    AlgoTuner and plain ASCII replace_text_in_range + immediate Enter is NOT swallowed.
    let _ = shell.update(&mut app, |s, cx| {
        assert_eq!(s.active_panel, ActiveWorkspacePanel::Chats);
        s.update_ime_marked_text("zhong", cx);
        s.composer.update(cx, |c, ccx| {
            c.bot_ac_matches = vec![BotSlashCommandItem {
                command: "/help".to_string(),
                description: "Help".to_string(),
                bot_username: "helper_bot".to_string(),
                avatar_initials: "HB".to_string(),
                avatar_color: 0x3B82F6,
            }];
            c.bot_ac_visible = true;
            ccx.notify();
        });
        assert!(s.composer.read(cx).ime_machine.is_composing());
        assert!(s.composer.read(cx).bot_ac_visible);

        assert!(s.handle_shortcut_key("c", true, cx));
        assert_eq!(s.active_panel, ActiveWorkspacePanel::Community);
        assert!(
            !s.composer.read(cx).ime_machine.is_composing(),
            "F15-R6-D1: switching Chats -> Community must cancel composer CJK IME composition"
        );
        assert!(
            !s.composer.read(cx).bot_ac_visible,
            "R6-REV1-D2: switching Chats -> Community must dismiss composer bot autocomplete"
        );
    });
    host.draw(&mut app);

    let cw = shell
        .read_with(&app, |s, _| s.community_workspace.clone().expect("community mounted"))
        .unwrap();

    // Defense-in-depth gate in shell_input_ime.rs
    let _ = shell.update(&mut app, |s, cx| {
        s.composer.update(cx, |c, ccx| {
            c.ime_machine.on_ime_update("stale", 5);
            c.bot_ac_matches = vec![BotSlashCommandItem {
                command: "/ping".to_string(),
                description: "Ping".to_string(),
                bot_username: "helper_bot".to_string(),
                avatar_initials: "HB".to_string(),
                avatar_color: 0x3B82F6,
            }];
            c.bot_ac_visible = true;
            ccx.notify();
        });
        assert!(s.should_hide_composer(cx));
        assert!(s.composer.read(cx).ime_machine.is_composing());
        cw.update(cx, |w, wcx| {
            w.state.active_popover = Some(CommunityPopover::AlgoTuner);
            wcx.notify();
        });
        assert!(s.handle_non_char_key("escape", false, cx));
        assert!(
            cw.read(cx).state.active_popover.is_none(),
            "F15-R6-D1: 1st Escape in Community must close AlgoTuner even if hidden composer is composing"
        );
        assert_eq!(s.active_panel, ActiveWorkspacePanel::Community);

        cw.update(cx, |w, wcx| {
            w.state.focus_input(ActiveCommunityInput::RightRailSearch);
            w.state.right_rail_search_draft.clear();
            wcx.notify();
        });
        s.ime.just_committed_ime = false;
        s.commit_replace_text_in_range("rust", cx);
        assert!(
            !s.ime.just_committed_ime,
            "F15-R6-D1/R6-REV1-D2: plain ASCII replace_text_in_range in Community must NOT arm 80ms IME fence"
        );
        assert!(s.handle_non_char_key("enter", false, cx));
        assert_eq!(
            cw.read(cx).state.route,
            CommunityRoute::Explore {
                initial_query: Some("rust".to_string()),
            },
            "F15-R6-D1: immediate Enter after plain ASCII replace_text_in_range must submit search"
        );
        assert_eq!(cw.read(cx).state.explore_search_draft, "rust");

        s.composer.update(cx, |c, ccx| {
            c.ime_machine.on_ime_cancel();
            c.bot_ac_visible = false;
            c.bot_ac_matches.clear();
            ccx.notify();
        });
        cw.update(cx, |w, wcx| w.replace_route(CommunityRoute::Feed, wcx));
    });
    host.draw(&mut app);

    // 2. R6-REV1-D1: reset_nav_destination_if preserves last_handled_nav_dest on non-matching
    //    SavedMessages/Folders, AND restores RemoteComputer in standalone_computers_mode!
    let _ = shell.update(&mut app, |s, cx| {
        s.sidebar.update(cx, |sb, scx| {
            sb.folders = vec![("All".to_string(), 0), ("Work".to_string(), 0)];
            scx.notify();
        });
        assert!(s.handle_shortcut_key("7", false, cx));
        assert_eq!(s.nav_rail.read(cx).active_destination, NavDestination::SavedMessages);
        assert_eq!(s.last_handled_nav_dest, Some(NavDestination::SavedMessages));
        s.hide_talk_mode(cx);
        assert_eq!(s.nav_rail.read(cx).active_destination, NavDestination::SavedMessages);
        assert_eq!(
            s.last_handled_nav_dest,
            Some(NavDestination::SavedMessages),
            "R6-REV1-D1: non-matching reset_nav_destination_if must NOT overwrite last_handled_nav_dest"
        );

        assert!(s.handle_shortcut_key("2", false, cx));
        assert_eq!(s.nav_rail.read(cx).active_destination, NavDestination::Folders);
        assert_eq!(s.last_handled_nav_dest, Some(NavDestination::Folders));
        s.sidebar.update(cx, |sb, scx| {
            sb.select_folder("All");
            scx.notify();
        });
        s.hide_talk_mode(cx);
        assert_eq!(
            s.last_handled_nav_dest,
            Some(NavDestination::Folders),
            "R6-REV1-D1: non-matching reset_nav_destination_if must preserve Folders"
        );
        s.set_community_unread_badge(3, cx);
    });
    host.draw(&mut app);
    let _ = shell.update(&mut app, |s, cx| {
        assert_eq!(
            s.sidebar.read(cx).active_folder,
            "All",
            "R6-REV1-D1: badge notification must NOT spuriously re-run activate_nav_destination(Folders)"
        );

        s.open_remote_computers_hub(cx);
        assert_eq!(s.active_panel, ActiveWorkspacePanel::Settings);
        assert!(s.settings_router.read(cx).standalone_computers_mode);
        assert_eq!(s.nav_rail.read(cx).active_destination, NavDestination::RemoteComputer);
        assert!(s.handle_shortcut_key("6", false, cx));
        assert!(s.rtc_talk.talk_mode_overlay.is_some());
        assert_eq!(s.nav_rail.read(cx).active_destination, NavDestination::TalkMode);
        // Closing TalkMode directly (e.g. hangup/close button) while standalone RemoteComputer hub
        // remains active must restore NavDestination::RemoteComputer (R6-REV1-D1).
        s.hide_talk_mode(cx);
        assert!(s.rtc_talk.talk_mode_overlay.is_none());
        assert_eq!(
            s.nav_rail.read(cx).active_destination,
            NavDestination::RemoteComputer,
            "R6-REV1-D1: dismissing TalkMode from standalone RemoteComputer hub must restore RemoteComputer"
        );
        assert_eq!(s.last_handled_nav_dest, Some(NavDestination::RemoteComputer));
        // Upstream Z-order: when Settings (Z #9) sits visually above TalkMode (Z #2),
        // Esc #1 closes Settings (Z #9) and Esc #2 closes TalkMode (Z #2).
        assert!(s.handle_shortcut_key("6", false, cx));
        assert!(s.rtc_talk.talk_mode_overlay.is_some());
        assert!(s.handle_non_char_key("escape", false, cx));
        assert_eq!(s.active_panel, ActiveWorkspacePanel::Chats);
        assert!(s.rtc_talk.talk_mode_overlay.is_some());
        assert!(s.handle_non_char_key("escape", false, cx));
        assert!(s.rtc_talk.talk_mode_overlay.is_none());
        assert_eq!(s.nav_rail.read(cx).active_destination, NavDestination::Chats);
    });
    host.draw(&mut app);
    body_r7_part2_call_failure_and_nav_sync(&host, &shell, &mut app).await;
    body_r7_part3_load_more_and_notif_supersede(&shell, &cw, &mut app).await;
}

async fn body_r7_part2_call_failure_and_nav_sync(
    host: &HeadlessShellHost,
    shell: &Entity<AppShell>,
    app: &mut AsyncApp,
) {
    // 3. F15-R6-D2: Failed AI TalkMode invite AND failed WebRTC call invite from Community
    //    must both restore nav_rail == Community and last_handled_nav_dest == Some(Community).
    let make_dialog = |id: u64, title: &str, initials: &str, is_bot: bool| DialogItemModel {
        id,
        peer_name: title.to_string(),
        avatar_initials: initials.to_string(),
        avatar_color: 0x3B82F6,
        last_message: "hi".to_string(),
        timestamp: "now".to_string(),
        unread_count: 0,
        is_pinned: false,
        is_muted: false,
        is_online: true,
        typing_indicator: None,
        delivery_status: DialogDeliveryStatus::Read,
        folder: if is_bot { "Bots" } else { "All" }.to_string(),
    };

    let _ = shell.update(app, |s, cx| {
        s.sidebar.update(cx, |sb, scx| {
            sb.dialogs = vec![
                make_dialog(7701, "Voice AI Bot", "VB", true),
                make_dialog(7702, "Human Peer", "HP", false),
            ];
            sb.bot_ids.insert(7701);
            sb.select_dialog(7701);
            scx.notify();
        });
        s.stage.update(cx, |st, stcx| {
            st.active_chat_id = Some(7701);
            st.chat_title = "Voice AI Bot".to_string();
            stcx.notify();
        });
        s.session_sync.chat_peer_user_ids.insert(7702, 8802);
        s.open_community_workspace(cx);
        assert!(s.is_active_chat_bot(cx));
        assert!(s.handle_shortcut_key("6", false, cx));
        assert!(s.rtc_talk.talk_mode_overlay.is_some());
        assert_eq!(s.nav_rail.read(cx).active_destination, NavDestination::TalkMode);
    });
    assert!(
        pump_until(app, Duration::from_secs(5), |cx| {
            shell.read(cx).rtc_talk.talk_mode_overlay.is_none()
        })
        .await,
        "F15-R6-D2: AI TalkMode invite against unreachable endpoint must fail and clear overlay"
    );
    let _ = shell.update(app, |s, cx| {
        assert_eq!(s.active_panel, ActiveWorkspacePanel::Community);
        assert_eq!(
            s.nav_rail.read(cx).active_destination,
            NavDestination::Community,
            "F15-R6-D2: failed AI TalkMode invite from Community must restore nav_rail == Community"
        );
        assert_eq!(s.last_handled_nav_dest, Some(NavDestination::Community));

        s.sidebar.update(cx, |sb, scx| {
            sb.select_dialog(7702);
            scx.notify();
        });
        s.stage.update(cx, |st, stcx| {
            st.active_chat_id = Some(7702);
            st.chat_title = "Human Peer".to_string();
            stcx.notify();
        });
        assert!(!s.is_active_chat_bot(cx));
        assert_eq!(s.current_peer_user_id(cx), Some(8802));
        assert!(s.handle_shortcut_key("6", false, cx));
        assert!(s.rtc_talk.talk_mode_overlay.is_some());
        assert_eq!(s.nav_rail.read(cx).active_destination, NavDestination::TalkMode);
    });
    assert!(
        pump_until(app, Duration::from_secs(5), |cx| {
            shell.read(cx).rtc_talk.talk_mode_overlay.is_none()
        })
        .await,
        "F15-R6-D2: WebRTC call invite against unreachable endpoint must fail and clear overlay"
    );
    let _ = shell.update(app, |s, cx| {
        assert_eq!(s.active_panel, ActiveWorkspacePanel::Community);
        assert_eq!(
            s.nav_rail.read(cx).active_destination,
            NavDestination::Community,
            "F15-R6-D2: failed WebRTC call invite from Community must restore nav_rail == Community"
        );
        assert_eq!(s.last_handled_nav_dest, Some(NavDestination::Community));
    });

    // 4. F15-R6-D3: Leaving Community via MainMenuCommand::{TaskCenter, McpTools, Calls,
    //    SavedMessages}, CreateGroupSubmitted, and NavRail Archive/Folder MUST sync
    //    nav_rail.active_destination & last_handled_nav_dest away from Community so
    //    sidebar Community toggles re-open Community cleanly!
    for (cmd, expected_panel, expected_dest) in [
        (MainMenuCommand::TaskCenter, ActiveWorkspacePanel::TaskCenter, NavDestination::TaskCenter),
        (MainMenuCommand::McpTools, ActiveWorkspacePanel::McpTools, NavDestination::McpTools),
        (MainMenuCommand::Calls, ActiveWorkspacePanel::Calls, NavDestination::Calls),
        (MainMenuCommand::SavedMessages, ActiveWorkspacePanel::Chats, NavDestination::SavedMessages),
    ] {
        let _ = shell.update(app, |s, cx| {
            s.open_community_workspace(cx);
            s.handle_main_menu_command(cmd, cx);
            assert_eq!(s.active_panel, expected_panel);
            assert_eq!(
                s.nav_rail.read(cx).active_destination,
                expected_dest,
                "F15-R6-D3: MainMenuCommand must sync nav_rail away from Community"
            );
            assert_eq!(s.last_handled_nav_dest, Some(expected_dest));
            let toggle_target = if s.nav_rail.read(cx).active_destination == NavDestination::Community {
                NavDestination::Chats
            } else {
                NavDestination::Community
            };
            assert_eq!(toggle_target, NavDestination::Community);
            s.sidebar.update(cx, |sb, scx| {
                sb.handle_unified_action(UnifiedSidebarAction::SelectDestination(toggle_target), scx);
            });
        });
        host.draw(app);
        let _ = shell.update(app, |s, cx| {
            assert_eq!(
                s.active_panel,
                ActiveWorkspacePanel::Community,
                "F15-R6-D3: sidebar Community toggle must cleanly re-open Community after leaving"
            );
            assert_eq!(s.nav_rail.read(cx).active_destination, NavDestination::Community);
        });
    }

    let _ = shell.update(app, |s, cx| {
        s.apply_theme_selection(TdesktopThemeMode::Night, None, None, cx);
        s.open_community_workspace(cx);
        s.handle_modal_outcome(
            ModalActionOutcome::CreateGroupSubmitted {
                title: "R7 Squad".to_string(),
                members_count: 0,
                enable_forum_topics: false,
            },
            cx,
        );
        assert_eq!(s.active_panel, ActiveWorkspacePanel::Chats);
        assert_eq!(s.nav_rail.read(cx).active_destination, NavDestination::Chats);
        assert_eq!(s.last_handled_nav_dest, Some(NavDestination::Chats));
    });
    host.draw(app);

    let _ = shell.update(app, |s, cx| {
        s.open_community_workspace(cx);
        s.nav_rail.update(cx, |r, rcx| {
            r.archive_open_requested = true;
            rcx.notify();
        });
    });
    host.draw(app);
    let _ = shell.update(app, |s, cx| {
        assert_eq!(s.active_panel, ActiveWorkspacePanel::Chats);
        assert_eq!(s.nav_rail.read(cx).active_destination, NavDestination::Chats);
        assert_eq!(s.last_handled_nav_dest, Some(NavDestination::Chats));
    });

    for via_select_folder in [false, true] {
        let _ = shell.update(app, |s, cx| {
            s.open_community_workspace(cx);
            s.nav_rail.update(cx, |r, rcx| {
                r.custom_folders = vec![NavFolderItem {
                    folder_id: 42,
                    title: "Work".to_string(),
                    icon_hint: "work".to_string(),
                    unread_badge: 0,
                    is_muted: false,
                }];
                if via_select_folder {
                    r.select_folder(Some(42));
                } else {
                    r.selected_folder_requested = Some(42);
                }
                rcx.notify();
            });
        });
        host.draw(app);
        let _ = shell.update(app, |s, cx| {
            assert_eq!(s.active_panel, ActiveWorkspacePanel::Chats);
            assert_eq!(s.nav_rail.read(cx).active_destination, NavDestination::Chats);
            assert_eq!(s.last_handled_nav_dest, Some(NavDestination::Chats));
            assert_eq!(
                s.sidebar.read(cx).active_folder,
                "Work",
                "F15-R6-D3: NavRail custom folder must preserve selected folder"
            );
        });
    }
}

async fn body_r7_part3_load_more_and_notif_supersede(
    shell: &Entity<AppShell>,
    cw: &Entity<gpui_telegram_desktop::views::community::CommunityWorkspace>,
    app: &mut AsyncApp,
) {
    // 5. F16-R6-D1: load_more_feed superseded by select_feed_tab(Popular) MUST NOT
    //    deadlock feed_loading_more == true; subsequent load_more_feed MUST advance
    //    pagination cleanly. Also load_notifications superseded by mark_all_notifications_read
    //    or bootstrap_initial_load MUST NOT wedge notifications_loading == true!
    let _ = shell.update(app, |s, cx| {
        s.open_community_workspace(cx);
    });
    assert!(
        pump_until(app, Duration::from_secs(5), |cx| {
            !cw.read(cx).state.feed_loading && !cw.read(cx).state.communities.is_empty()
        })
        .await
    );

    let (facade, comm_slug) = cw
        .read_with(app, |w, _| {
            (w.facade.clone(), w.state.communities[0].slug.clone())
        })
        .unwrap();
    let rt_handle = facade.runtime_handle().clone();
    let seed_fut = rt_handle.spawn(async move {
        for idx in 0..28u32 {
            let _ = facade
                .create_post(&CreatePostRequest {
                    community_slug: Some(comm_slug.clone()),
                    format: PostFormat::Micro,
                    title: Some(format!("R7 Pagination Post #{idx}")),
                    body: format!("Pagination stress body #{idx} for F16-R6-D1."),
                    status: PostStatus::Published,
                    ..Default::default()
                })
                .await;
        }
    });
    let _ = seed_fut.await;

    let _ = cw.update(app, |w, cx| {
        w.select_feed_tab(FeedTab::All, cx);
    });
    assert!(
        pump_until(app, Duration::from_secs(5), |cx| {
            let s = &cw.read(cx).state;
            !s.feed_loading && s.feed_items.len() == 25 && s.next_cursor.is_some()
        })
        .await,
        "F16-R6-D1 setup: FeedTab::All must load page 1 (25 items) with next_cursor == Some"
    );

    // Start load_more_feed and IMMEDIATELY supersede with select_feed_tab(FeedTab::Popular)
    let _ = cw.update(app, |w, cx| {
        w.load_more_feed(cx);
        assert!(w.state.feed_loading_more);
        w.select_feed_tab(FeedTab::Popular, cx);
        assert!(
            !w.state.feed_loading_more,
            "F16-R6-D1: load_feed must reset feed_loading_more synchronously when superseding load_more_feed"
        );
    });
    assert!(
        pump_until(app, Duration::from_secs(5), |cx| {
            let s = &cw.read(cx).state;
            !s.feed_loading && s.feed_items.len() == 25 && s.next_cursor.is_some()
        })
        .await
    );
    let settle_until = Instant::now() + Duration::from_millis(120);
    while Instant::now() < settle_until {
        let _ = app.background_executor().timer(Duration::from_millis(10)).await;
    }
    let _ = cw.update(app, |w, cx| {
        assert!(
            !w.state.feed_loading_more,
            "F16-R6-D1: feed_loading_more must remain false after superseded load_more_feed settles"
        );
        assert!(w.state.next_cursor.is_some());
        w.load_more_feed(cx);
        assert!(w.state.feed_loading_more);
    });
    assert!(
        pump_until(app, Duration::from_secs(5), |cx| {
            let s = &cw.read(cx).state;
            !s.feed_loading_more && s.feed_items.len() > 25
        })
        .await,
        "F16-R6-D1: subsequent load_more_feed MUST advance pagination past 25 items"
    );

    // Notifications supersede: load_notifications -> mark_all_notifications_read
    let _ = cw.update(app, |w, cx| {
        w.load_notifications(cx);
        assert!(w.state.notifications_loading);
        w.mark_all_notifications_read(cx);
        assert!(
            !w.state.notifications_loading,
            "F16-R6-D1: mark_all_notifications_read must reset notifications_loading"
        );
    });
    let settle_notif = Instant::now() + Duration::from_millis(120);
    while Instant::now() < settle_notif {
        let _ = app.background_executor().timer(Duration::from_millis(10)).await;
    }
    assert!(
        !cw.read_with(app, |w, _| w.state.notifications_loading).unwrap(),
        "F16-R6-D1: notifications_loading must stay false after superseded load_notifications settles"
    );

    // Notifications supersede: load_notifications -> bootstrap_initial_load
    let _ = cw.update(app, |w, cx| {
        w.load_notifications(cx);
        assert!(w.state.notifications_loading);
        w.bootstrap_initial_load(cx);
        assert!(
            !w.state.notifications_loading,
            "F16-R6-D1: bootstrap_initial_load must reset notifications_loading"
        );
    });
    assert!(
        pump_until(app, Duration::from_secs(5), |cx| {
            let s = &cw.read(cx).state;
            !s.feed_loading && !s.notifications_loading
        })
        .await
    );
}

#[test]
fn test_community_apple_sidebar_content_hub_coordination() {
    run_in_headless_subprocess("test_community_apple_sidebar_content_hub_coordination");
}

async fn body_apple_sidebar_content_hub_coordination(mut app: AsyncApp) {
    let host = open_authenticated_shell(&mut app, 1440.0, 900.0, 701, "hub_user");
    let shell = host.shell.clone();

    // 1. Switch Chats -> ContentHub(Books) -> AppleSidebarSection::Community -> ContentHub(MyCreations) -> Chats
    let _ = shell.update(&mut app, |s, cx| {
        s.select_apple_sidebar_section(AppleSidebarSection::Books, cx);
        assert_eq!(s.active_panel, ActiveWorkspacePanel::ContentHub(ContentHubSection::Books));
        assert_eq!(s.apple_sidebar.read(cx).active_section, AppleSidebarSection::Books);
        assert_eq!(s.inspect_state(cx).window.active_panel, "ContentHub(Books)");

        // Focus CatalogSearch and verify plain Cmd+C stays in ContentHub while Cmd+Shift+C opens Community
        s.content_hub.update(cx, |h, hcx| {
            h.focus_input(ContentHubInputTarget::CatalogSearch, hcx);
        });
        s.insert_committed_text("rustbook", cx);
        assert!(s.is_content_hub_input_active(cx));
        assert!(s.handle_shortcut_key("c", false, cx));
        assert_eq!(s.active_panel, ActiveWorkspacePanel::ContentHub(ContentHubSection::Books));

        assert!(s.handle_shortcut_key("c", true, cx));
        assert_eq!(s.active_panel, ActiveWorkspacePanel::Community);
        assert_eq!(s.apple_sidebar.read(cx).active_section, AppleSidebarSection::Community);
        assert_eq!(s.nav_rail.read(cx).active_destination, NavDestination::Community);
        assert_eq!(s.inspect_state(cx).window.active_panel, "Community");
        assert_eq!(s.content_hub.read(cx).active_input, ContentHubInputTarget::None);
    });
    host.draw(&mut app);

    // 2. Focus Community input, then switch via AppleSidebarSection::Books -> blurs Community cleanly
    let _ = shell.update(&mut app, |s, cx| {
        let cw = s.community_workspace().cloned().expect("mounted");
        cw.update(cx, |w, wcx| {
            w.state.focus_input(ActiveCommunityInput::QuickPostBody);
            wcx.notify();
        });
        assert!(s.is_community_input_active(cx));

        s.apple_sidebar.update(cx, |sb, scx| {
            sb.select_section(AppleSidebarSection::Books, scx);
        });
    });
    host.draw(&mut app);

    let _ = shell.update(&mut app, |s, cx| {
        assert_eq!(s.active_panel, ActiveWorkspacePanel::ContentHub(ContentHubSection::Books));
        assert_eq!(s.apple_sidebar.read(cx).active_section, AppleSidebarSection::Books);
        let cw = s.community_workspace().cloned().expect("mounted");
        assert_eq!(cw.read(cx).state.active_input, ActiveCommunityInput::None);

        // Click AppleSidebarSection::Community directly from Books
        s.apple_sidebar.update(cx, |sb, scx| {
            sb.select_section(AppleSidebarSection::Community, scx);
        });
    });
    host.draw(&mut app);

    // 3. Unread badge sync onto AppleAppSidebar + MyCreations + logout reset
    let _ = shell.update(&mut app, |s, cx| {
        assert_eq!(s.active_panel, ActiveWorkspacePanel::Community);
        assert_eq!(s.apple_sidebar.read(cx).active_section, AppleSidebarSection::Community);

        s.set_community_unread_badge(7, cx);
        assert_eq!(s.apple_sidebar.read(cx).community_unread_count, 7);

        s.select_apple_sidebar_section(AppleSidebarSection::MyCreations, cx);
        assert_eq!(
            s.active_panel,
            ActiveWorkspacePanel::ContentHub(ContentHubSection::MyCreations)
        );
        assert_eq!(s.apple_sidebar.read(cx).active_section, AppleSidebarSection::MyCreations);
        assert_eq!(s.inspect_state(cx).window.active_panel, "ContentHub(MyCreations)");

        s.select_apple_sidebar_section(AppleSidebarSection::Chats, cx);
        assert_eq!(s.active_panel, ActiveWorkspacePanel::Chats);
        assert_eq!(s.apple_sidebar.read(cx).active_section, AppleSidebarSection::Chats);
        assert_eq!(s.apple_sidebar.read(cx).community_unread_count, 7);

        s.logout(cx);
        assert_eq!(s.apple_sidebar.read(cx).community_unread_count, 0);
        assert_eq!(s.apple_sidebar.read(cx).active_section, AppleSidebarSection::Chats);
        assert!(s.community_workspace().is_none());
    });
}

