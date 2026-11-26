//! M5 Headless GPUI Integration & Unit Test Suite for F17–F21:
//! - F17: `PostDetailView` (Reddit nested tree, 6 sorts, inline reply, edit/delete tombstones,
//!        X Thread & Quote chain, 1-click Summon Bot, locked-post guard)
//! - F18: `ComposerModal` (all 8 formats, formatting toolbar, `@/#`/`c/` autocomplete,
//!        Drafts save/resume/publish, Scheduled posts, Ask Bot Co-Author, Escape cascade)
//! - F19: `CommunityPageView` (banner/stats/pinned, Join/Bell, Flairs, Rules, Versioned Wiki,
//!        Mods roster, non-mod guard, Create Community modal)
//! - F20: `ActorProfileView` (Human+Bot, dual karma, DM bridge, Mute/Block, Edit Profile),
//!        `NotificationsView` (all 13 kinds, mark read/all read badge sync, jump-to-comment),
//!        `BookmarksListsView` (custom folders, List CRUD, membership, live List Timeline)
//! - F21: `ExploreSearchView` (all 7 tabs, operator normalization, Discovery),
//!        `ModQueueView` (role guard, queue actions, AutoMod CRUD + live dry-run, Mod Log),
//!        `BotStudioView` (register/edit bot, 8×11 Grok avatar clamp, snippet copy, playground)
//! - Multi-theme (`Night`/`Tinted`/`Day`/`Classic`), bilingual (`zh-CN`/`en`), 3/2/1-col responsive smoke.

use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::time::{Duration, Instant};

use futures::FutureExt;
use gpui::{
    bounds, point, px, size, App, AppContext, Application, AsyncApp, Entity, Keystroke,
    WindowBounds, WindowOptions,
};
use gpui_telegram_client::{AuthResponse, ClientConfig, TelegramClient};
use gpui_telegram_desktop::theme::{IconName, TelegramAssetSource};
use gpui_telegram_desktop::views::community::bot_studio_view::{
    clamp_grok_avatar_indices, format_bot_api_snippet, format_bot_api_snippet_for_display,
    resolve_bot_studio_token_for_actor, resolve_grok_avatar_pair, validate_bot_studio_registration,
};
use gpui_telegram_desktop::views::community::composer_modal::{
    compute_composer_autocomplete, detect_autocomplete_trigger, validate_composer_link_url,
    AutocompleteKind,
};
use gpui_telegram_desktop::views::community::explore_search_view::{
    build_search_request, filter_search_result_by_operators, normalize_search_query,
};
use gpui_telegram_desktop::views::community::mod_queue_view::validate_automod_regex_syntax;
use gpui_telegram_desktop::views::community::{
    ActiveCommunityInput, ActorProfileTab, BookmarksListsSection, BotStudioEditDraft,
    CommunityLayoutMode, CommunityModal, CommunityPageTab, CommunityPopover, CommunityRoute,
    CommunityWorkspace, CommunityWorkspaceAction, ComposerFormatTab, ComposerMediaDraft,
    ModConsoleTab, NotificationCategoryTab, PostDetailMode,
};
use gpui_telegram_desktop::views::composer::formatting_bar::TextStyleKind;
use gpui_telegram_desktop::views::shell::{ActiveWorkspacePanel, AppShell};
use gpui_telegram_desktop::views::TdesktopThemeMode;
use gpui_telegram_proto::community_contracts::{
    ActorKind, AlgoPreferencesDto, AutoModActionKind, BotAutonomyMode, CommentSort,
    CommunityActorDto, CommunityDetailResponse, CommunityIdentitiesResponse, CommunityListDto,
    CommunityListKind, CommunityListsResponse, CommunityMemberRole, CommunityNotificationDto,
    CommunityNotificationKind, CommunitySearchResultDto, CommunitySearchSort, CommunitySearchTab,
    CreatePostRequest, EngagementSignalKind, FeedResponseDto, ListCommunitiesResponse,
    ModActionKind, ModLogsResponse, ModQueueResponse, NotificationsListResponse, PostFormat,
    PostStatus, SubCommunityDto, TrendingResponse, VerifiedTier,
};
use tempfile::TempDir;

const CHILD_ENV: &str = "GPUI_COMMUNITY_M5_CHILD";
const CHILD_TARGET_ENV: &str = "GPUI_COMMUNITY_M5_TARGET";
#[cfg(target_os = "macos")]
const CHILD_AFTER_CTORS_ENV: &str = "GPUI_COMMUNITY_M5_AFTER_CTORS";

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
        "test_m5_f17_post_detail_nested_tree_sorts_reply_edit_delete_thread_summon_and_lock" => {
            |app| Box::pin(body_f17_post_detail(app))
        }
        "test_m5_f18_composer_modal_all_formats_autocomplete_drafts_schedule_coauthor_and_escape" => {
            |app| Box::pin(body_f18_composer_modal(app))
        }
        "test_m5_f19_community_page_create_flairs_rules_wiki_revisions_and_mod_guards" => {
            |app| Box::pin(body_f19_community_page(app))
        }
        "test_m5_f20_profile_dm_bridge_notifications_bookmarks_folders_and_lists_timeline" => {
            |app| Box::pin(body_f20_profile_notifs_bookmarks_lists(app))
        }
        "test_m5_f21_explore_search_mod_console_automod_dryrun_and_bot_studio" => {
            |app| Box::pin(body_f21_explore_mod_bot_studio(app))
        }
        "test_m5_multi_theme_bilingual_and_responsive_layout_smoke_all_routes" => {
            |app| Box::pin(body_multi_theme_bilingual_responsive_smoke(app))
        }
        "test_m5_f21_mod_race_actor_switch_fences_online_delayed_rtt" => {
            |app| Box::pin(body_f21_mod_race_actor_switch_fences_online_delayed_rtt(app))
        }
        "test_m5_f20_lists_rapid_toggle_single_flight_sqlite_stress" => {
            |app| Box::pin(body_f20_lists_rapid_toggle_single_flight_sqlite_stress(app))
        }
        other => {
            eprintln!("unknown community M5 headless gpui test target: {other}");
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
                            eprintln!("[community M5 headless child panic] {t}: {msg}");
                            exit_headless_child(101, use_underscore_exit);
                        }
                    }
                })
                .detach();
            });
    }));
    if let Err(payload) = outer {
        let msg = panic_payload_to_string(payload);
        eprintln!("[community M5 headless outer panic] {target}: {msg}");
        exit_headless_child(101, use_underscore_exit);
    }
    eprintln!(
        "[community M5 headless error] {target}: Application::headless().run returned before async test body completed"
    );
    exit_headless_child(102, use_underscore_exit);
}

#[cfg(target_os = "macos")]
#[used]
#[unsafe(link_section = "__DATA,__mod_init_func")]
static MACOS_M5_HEADLESS_ENTRY: extern "C" fn() = macos_mod_init_headless_entry;

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
            .arg("__community_m5_headless_child_noop__");
    }
    #[cfg(not(target_os = "macos"))]
    {
        cmd.arg("--exact").arg(test_fn_name);
    }
    let output = cmd
        .arg("--nocapture")
        .output()
        .expect("spawn M5 community headless gpui child");
    if !output.status.success() {
        let stdout = String::from_utf8_lossy(&output.stdout);
        let stderr = String::from_utf8_lossy(&output.stderr);
        panic!(
            "community M5 headless test `{test_fn_name}` failed (status {:?})\n--- stdout ---\n{stdout}\n--- stderr ---\n{stderr}",
            output.status.code()
        );
    }
}

fn test_auth(user_id: u64, username: &str) -> AuthResponse {
    AuthResponse {
        user_id,
        username: username.to_string(),
        session_id: format!("sess-m5-{user_id}"),
        token: format!("test-m5-token-{user_id}"),
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
    cw: Entity<CommunityWorkspace>,
    width: f32,
    height: f32,
}

impl HeadlessShellHost {
    fn draw(&self, app: &mut AsyncApp) {
        self.draw_at(app, self.width, self.height);
    }

    fn draw_at(&self, app: &mut AsyncApp, width: f32, height: f32) {
        if let Some(win) = self.win {
            let _ = app.update_window(win.into(), |_, window, cx| {
                let _ = window.draw(cx);
            });
        }
        let _ = self.shell.update(app, |s, cx| {
            s.app.sync_viewport_size(width, height);
            let sb_w = s.sidebar.read(cx).effective_width();
            if let Some(cw) = s.community_workspace.clone() {
                cw.update(cx, |cw, wcx| {
                    cw.set_host_sidebar_width(sb_w);
                    let _el = cw.render_for_viewport_width(width, wcx);
                });
            }
        });
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
            "cmd-l" => {
                let _ = s.handle_shortcut_key("l", false, cx);
            }
            other => panic!("unsupported M5 headless key spec: {other}"),
        });
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

async fn open_community_host(
    app: &mut AsyncApp,
    width: f32,
    height: f32,
    user_id: u64,
    username: &str,
) -> HeadlessShellHost {
    let dir = TempDir::new().expect("create community M5 test tempdir");
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
                        let ent = cx.new(|cx| {
                            let mut shell = AppShell::new_with_client(client_for_win.clone(), cx);
                            shell.apply_authenticated_session(auth_for_win.clone(), cx);
                            shell
                        });
                        (None, ent)
                    }
                    Err(err) => panic!("open headless GPUI window failed unexpectedly: {err:#}"),
                }
            }
        })
        .expect("app update open_community_host");

    let _ = shell.update(app, |s, cx| {
        s.open_community_workspace(cx);
    });
    let cw = shell
        .read_with(app, |s, _| {
            assert_eq!(s.active_panel, ActiveWorkspacePanel::Community);
            s.community_workspace
                .clone()
                .expect("community workspace mounted")
        })
        .unwrap();

    assert!(
        pump_until(app, Duration::from_secs(5), |cx| {
            let st = &cw.read(cx).state;
            !st.feed_loading
                && !st.feed_items.is_empty()
                && !st.communities.is_empty()
                && st.identities.is_some()
        })
        .await,
        "CommunityWorkspace must complete initial local SQLite bootstrap"
    );

    HeadlessShellHost {
        _dir: dir,
        client,
        win,
        shell,
        cw,
        width,
        height,
    }
}

// ============================================================================
// Pure Unit Tests (Fast, Deterministic)
// ============================================================================

#[test]
fn test_m5_icon_all_invariant_and_search_operator_normalization() {
    assert_eq!(IconName::ALL.len(), 64, "IconName::ALL MUST remain 64");
    assert_eq!(
        normalize_search_query("is:bot from:@rust_bot c/rust flair:\"deep dive\" has:poll"),
        "kind:bot author:rust_bot c:rust flair:\"deep dive\" has:poll"
    );
    assert_eq!(
        normalize_search_query("IS:HUMAN from:alice_dev"),
        "kind:human author:alice_dev"
    );
    assert_eq!(normalize_search_query("((("), "(((");

    let req_people = build_search_request("rust", CommunitySearchTab::People, CommunitySearchSort::Relevance);
    assert_eq!(req_people.actor_kind, Some(ActorKind::Human));
    let req_bots = build_search_request("rust", CommunitySearchTab::Bots, CommunitySearchSort::Top);
    assert_eq!(req_bots.actor_kind, Some(ActorKind::Bot));
    let req_polls = build_search_request("", CommunitySearchTab::Polls, CommunitySearchSort::Latest);
    assert_eq!(req_polls.has_poll, Some(true));
    let req_media = build_search_request("", CommunitySearchTab::Media, CommunitySearchSort::Relevance);
    assert_eq!(req_media.has_media, Some(true));
}

#[test]
fn test_m5_notification_category_exhaustive_mapping_and_automod_regex_validation() {
    let all_kinds = [
        CommunityNotificationKind::Reply,
        CommunityNotificationKind::Mention,
        CommunityNotificationKind::Like,
        CommunityNotificationKind::UpvoteMilestone,
        CommunityNotificationKind::Quote,
        CommunityNotificationKind::Repost,
        CommunityNotificationKind::Follow,
        CommunityNotificationKind::Award,
        CommunityNotificationKind::ModAction,
        CommunityNotificationKind::BotSummon,
        CommunityNotificationKind::BotReply,
        CommunityNotificationKind::PollEnded,
        CommunityNotificationKind::System,
    ];
    for kind in all_kinds {
        assert!(NotificationCategoryTab::All.matches(kind));
        let specific_matches = NotificationCategoryTab::ALL_TABS
            .iter()
            .filter(|t| **t != NotificationCategoryTab::All && t.matches(kind))
            .count();
        assert_eq!(
            specific_matches, 1,
            "every CommunityNotificationKind ({kind:?}) must belong to exactly one category tab"
        );
    }

    assert!(validate_automod_regex_syntax("(spam|airdrop)\\s+free").is_ok());
    assert!(validate_automod_regex_syntax("(?i)(?:scam|phish){1,3}?").is_ok());
    assert!(validate_automod_regex_syntax("(?P<word>foo)").is_ok());
    assert!(validate_automod_regex_syntax("[]a]").is_ok());
    assert!(validate_automod_regex_syntax("[^]a]").is_ok());
    assert!(validate_automod_regex_syntax("\\(?").is_ok());
    assert!(validate_automod_regex_syntax("").is_ok());
    for ok_pat in [
        "foo]bar",
        "]",
        "\\[SPAM]",
        "foo}bar",
        "}",
        "\\b{start}foo\\b{end}",
        "\\b{start-half}x\\b{end-half}",
        "a{1,2}??",
        "a**",
        "a+*",
        "[[:alpha:]-z]",
        "[[:bogus:]]",
        "\\p{L}",
        "\\pL",
        "\\p{Han}",
        "\\p{Emoji}",
        "\\p{gc=Letter}",
        "\\p{sc=Latin}",
        "\\p{sc!=Greek}",
        "(?-u:[\\x00-\\x7f])",
        "(?-u:foo)\\p{L}",
        "(?:spam|scam|phish){1,5}",
        "(?x) a { 1 , 2 } # comment",
        "(?x) a { 1 2 }",
        "^[\\p{Han}\\w\\s]+$",
        "\\w+",
        "\\d{10}",
        "\\s{80}",
        ".{60}",
        "(?-u:\\w{500})",
        // Wave r3 Challenger differential parity patterns (Unicode/dot/bracket capture names & within-64-KiB)
        "(?<标签>测试)",
        "(?<α>x)",
        "(?P<α>x)",
        "(?<a.b>x)",
        "(?P<a.b>x)",
        "(?<a[0]>x)",
        "(?P<a[0]>x)",
        "(?:a|b){400}",
        "(?:a|b){600}",
        "(?:a|b){800}",
        "\\p{Latin}{20}",
        "(a{10000}){0}",
        "(\\d{100}){0}",
    ] {
        assert!(
            validate_automod_regex_syntax(ok_pat).is_ok(),
            "valid Rust regex `{ok_pat}` must be accepted, got: {:?}",
            validate_automod_regex_syntax(ok_pat)
        );
    }
    for bad in [
        "(unclosed",
        "[unclosed",
        "trailing\\",
        "*leading",
        "?leading",
        "a{5,2}",
        "a{abc}",
        "a{}",
        "a{,3}",
        "(?)",
        "(?z)",
        "foo|?bar",
        "(?P<bad)",
        "(?P<>foo)",
        "(?P<1bad>foo)",
        "(?iz)foo",
        "(?i-z)foo",
        "(?-)foo",
        "(?i)*",
        "[]",
        "[^]",
        "[z-a]",
        "a{99999999}",
        "(?P=foo)",
        "(a{50}){50}",
        "((a{10}){10}){10}",
        "(?:(?:a{10}){10}){10}",
        "\\p{NotARealProperty}",
        "\\p{123}",
        "\\pX",
        "[\\p{NotAReal}]",
        "\\p{Cs}",
        "(?-u:\\xff)",
        "(?-u:.)",
        "(?-u:[^a])",
        "(?-u:\\p{L})",
        "(?-u:\\D)",
        "(?-u:[é])",
        "\\字",
        "\\🚀",
        "\\b{bogus}",
        "[\\d-z]",
        "[\\w-a]",
        "[a-\\w]",
        "[\\b]",
        "^[\\p{Han}\\w\\s]{1,50}$",
        "\\w{2}",
        "\\w{10}",
        "\\d{20}",
        "\\s{100}",
        ".{100}",
        "\\p{L}{2}",
        "(?-u:\\w{700})",
        // Wave r3 Challenger Unicode case-folded char-class 64 KiB overflows
        "[a-z]{250}",
        "[a-z]{300}",
        "[a-z]{350}",
        "[a-z]{400}",
        "[a-z]{600}",
        "[a-z]{800}",
        "[a-z]{1000}",
        "[a-z0-9_-]{350}",
        "[a-z0-9._%+-]{350}",
    ] {
        assert!(
            validate_automod_regex_syntax(bad).is_err(),
            "invalid regex `{bad}` must be rejected"
        );
    }

    // Strict Link URL validation (ASCII + multi-byte UTF-8 / CJK / emoji panic-free)
    for bad_url in [
        "",
        "not-a-url",
        "https://",
        "http://   ",
        "https:///no-host",
        "https://bad host.com/x",
        "https://example.com/a b",
        "ftp://example.com",
        "https://example.com/\x00",
        "你好世界测试",
        "123456é",
    ] {
        assert!(
            validate_composer_link_url(bad_url).is_err(),
            "invalid Link URL `{bad_url:?}` must be rejected"
        );
    }
    assert!(
        validate_composer_link_url("http://例子.测试").is_ok(),
        "UTF-8 IDN http:// URL must not panic and should parse host cleanly"
    );
    assert!(
        validate_composer_link_url("https://例子.测试/路径").is_ok(),
        "UTF-8 IDN https:// URL must not panic and should parse host cleanly"
    );
    // Emoji host must never panic regardless of IDNA acceptance
    let _ = validate_composer_link_url("http://🚀.example");
    assert_eq!(
        validate_composer_link_url("https://rust-lang.org/learn").unwrap(),
        "rust-lang.org"
    );
    assert_eq!(
        validate_composer_link_url("http://localhost:8080/api").unwrap(),
        "localhost"
    );
}

#[test]
fn test_m5_bot_studio_validation_grok_clamp_and_snippet() {
    assert_eq!(clamp_grok_avatar_indices(0, 0), (0, 0));
    assert_eq!(clamp_grok_avatar_indices(999, usize::MAX), (7, 10));
    let (sh, col) = resolve_grok_avatar_pair(usize::MAX, 42);
    assert_eq!(sh, "teardrop");
    assert_eq!(col, "gray");

    assert!(validate_bot_studio_registration("", "rust_bot", "30", "12").is_err());
    assert!(validate_bot_studio_registration("Short", "bot", "30", "12").is_err());
    assert!(validate_bot_studio_registration("NoSuffix", "rust_helper", "30", "12").is_err());
    assert!(validate_bot_studio_registration("BadRate", "rust_helper_bot", "30", "0").is_err());
    assert!(validate_bot_studio_registration("BadRate", "rust_helper_bot", "30", "999").is_err());
    assert!(validate_bot_studio_registration("BadCool", "rust_helper_bot", "99999", "10").is_err());
    let ok = validate_bot_studio_registration("Rust Critic", "@rust_critic_bot", "45", "20")
        .expect("valid bot registration");
    assert_eq!(ok, ("rust_critic_bot".to_string(), 45, 20));

    let local_snip = format_bot_api_snippet("http://127.0.0.1:8080/", None, Some("rust"));
    assert!(local_snip.contains("/bot/<LOCAL_BOT_TOKEN>/community.createPost"));
    let live_snip = format_bot_api_snippet("https://api.example.com", Some("tok_123"), Some("ai"));
    assert!(live_snip.contains("https://api.example.com/bot/tok_123/community.createPost"));

    // Token isolation + masked snippet when revealed == false
    let mut tok_map = std::collections::HashMap::new();
    tok_map.insert("b:101".to_string(), "123456:SUPER_SECRET_TOKEN_9876".to_string());
    assert_eq!(
        resolve_bot_studio_token_for_actor(&tok_map, "b:101").as_deref(),
        Some("123456:SUPER_SECRET_TOKEN_9876")
    );
    assert!(
        resolve_bot_studio_token_for_actor(&tok_map, "b:102").is_none(),
        "Bot B must never inherit Bot A's token"
    );
    let masked_snip = format_bot_api_snippet_for_display(
        "https://api.example.com",
        Some("123456:SUPER_SECRET_TOKEN_9876"),
        false,
        Some("rust"),
    );
    assert!(
        !masked_snip.contains("SUPER_SECRET"),
        "Unrevealed Bot Studio snippet must not expose plaintext token"
    );
    assert!(masked_snip.contains("123456••••••••9876"));
    let revealed_snip = format_bot_api_snippet_for_display(
        "https://api.example.com",
        Some("123456:SUPER_SECRET_TOKEN_9876"),
        true,
        Some("rust"),
    );
    assert!(revealed_snip.contains("123456:SUPER_SECRET_TOKEN_9876"));

    // Search operator filtering on Communities & People/Bots
    let req_comm = build_search_request("c/rust", CommunitySearchTab::Communities, CommunitySearchSort::Relevance);
    let sample_res = gpui_telegram_proto::community_contracts::CommunitySearchResultDto {
        query: req_comm.query.clone(),
        tab: CommunitySearchTab::Communities,
        posts: Vec::new(),
        actors: Vec::new(),
        communities: vec![
            gpui_telegram_proto::community_contracts::SubCommunityDto {
                community_id: "c1".into(),
                slug: "rust".into(),
                name: "Rust".into(),
                description: String::new(),
                category: "tech".into(),
                banner_media_id: None,
                icon_media_id: None,
                icon_emoji: String::new(),
                accent_color: "#000000".into(),
                member_count: 1,
                human_member_count: 1,
                bot_member_count: 0,
                posts_count: 0,
                allow_bots: true,
                require_bot_disclosure: true,
                is_nsfw: false,
                rules: Vec::new(),
                post_flairs: Vec::new(),
                user_flairs: Vec::new(),
                wiki_markdown: String::new(),
                wiki_revision: 1,
                pinned_post_ids: Vec::new(),
                automod_rules: Vec::new(),
                viewer_role: None,
                is_joined: false,
                notifications_enabled: false,
                created_by_actor_id: "u:1".into(),
                created_at_ms: 0,
            },
            gpui_telegram_proto::community_contracts::SubCommunityDto {
                community_id: "c2".into(),
                slug: "ai_agents".into(),
                name: "AI Agents".into(),
                description: String::new(),
                category: "ai".into(),
                banner_media_id: None,
                icon_media_id: None,
                icon_emoji: String::new(),
                accent_color: "#000000".into(),
                member_count: 1,
                human_member_count: 1,
                bot_member_count: 0,
                posts_count: 0,
                allow_bots: true,
                require_bot_disclosure: true,
                is_nsfw: false,
                rules: Vec::new(),
                post_flairs: Vec::new(),
                user_flairs: Vec::new(),
                wiki_markdown: String::new(),
                wiki_revision: 1,
                pinned_post_ids: Vec::new(),
                automod_rules: Vec::new(),
                viewer_role: None,
                is_joined: false,
                notifications_enabled: false,
                created_by_actor_id: "u:1".into(),
                created_at_ms: 0,
            },
        ],
        total_hits: 2,
        next_offset: None,
    };
    let filtered = filter_search_result_by_operators(&req_comm, sample_res);
    assert_eq!(filtered.communities.len(), 1);
    assert_eq!(filtered.communities[0].slug, "rust");
    assert_eq!(filtered.total_hits, 1);
}

#[test]
fn test_m5_composer_autocomplete_trigger_english_and_cjk() {
    let m = detect_autocomplete_trigger("你好 @rust_b").expect("mention trigger");
    assert_eq!(m.1, AutocompleteKind::Mention);
    assert_eq!(m.2, "rust_b");
    let h = detect_autocomplete_trigger("讨论 #算法").expect("hashtag trigger");
    assert_eq!(h.1, AutocompleteKind::Hashtag);
    assert_eq!(h.2, "算法");
    let c = detect_autocomplete_trigger("发到 c/ru").expect("community trigger");
    assert_eq!(c.1, AutocompleteKind::Community);
    assert_eq!(c.2, "ru");
}

// ============================================================================
// F17 — PostDetailView: Nested Tree, 6 Sorts, Reply w/ CJK IME & Bot Switch,
// Edit/Delete Tombstone, X Thread & Quote Chain, Summon Bot, Locked Guard
// ============================================================================

#[test]
fn test_m5_f17_post_detail_nested_tree_sorts_reply_edit_delete_thread_summon_and_lock() {
    run_in_headless_subprocess(
        "test_m5_f17_post_detail_nested_tree_sorts_reply_edit_delete_thread_summon_and_lock",
    );
}

async fn body_f17_post_detail(mut app: AsyncApp) {
    let host = open_community_host(&mut app, 1440.0, 900.0, 7101, "m5_f17_human").await;
    let cw = host.cw.clone();

    // Create a fresh owned community so we can also exercise Lock guard as Owner
    let _ = cw.update(&mut app, |w, cx| {
        w.open_create_community_modal(cx);
        w.state.create_community.slug = "f17_lab".to_string();
        w.state.create_community.name = "F17 Detail Lab".to_string();
        w.state.create_community.description = "Nested comment & thread testing".to_string();
        w.submit_create_community(cx);
    });
    assert!(
        pump_until(&mut app, Duration::from_secs(4), |cx| {
            cw.read(cx).state.route == CommunityRoute::CommunityPage("f17_lab".to_string())
        })
        .await,
        "Create Community must navigate to c/f17_lab"
    );

    // Create a multi-part X thread root post directly via facade so thread_chain has >= 2 parts
    let created = host
        .client
        .community()
        .block_on(host.client.community().create_post(&CreatePostRequest {
            community_slug: Some("f17_lab".to_string()),
            format: PostFormat::Micro,
            body: "F17 Root Thread 1/2 — Let's discuss GPUI + Rust!".to_string(),
            thread_parts: vec![gpui_telegram_proto::community_contracts::CreateThreadPartDto {
                body: "F17 Thread Part 2/2 — Follow-up benchmark notes.".to_string(),
                media: Vec::new(),
                link_preview: None,
            }],
            status: PostStatus::Published,
            ..Default::default()
        }))
        .expect("create F17 thread post");
    let pid = created.post.post_id.clone();

    // Navigate to PostDetail & wait for load
    let _ = cw.update(&mut app, |w, cx| w.open_post_detail(pid.clone(), cx));
    assert!(
        pump_until(&mut app, Duration::from_secs(4), |cx| {
            cw.read(cx)
                .state
                .detail_post
                .as_ref()
                .is_some_and(|d| d.post.post_id == pid)
        })
        .await
    );
    host.draw(&mut app);

    // 1. Inline Human reply with CJK IME commit
    let _ = cw.update(&mut app, |w, cx| {
        w.state.focus_input(ActiveCommunityInput::DetailReply);
        w.set_ime_preedit(Some("nihao"), cx);
        w.insert_text_to_active_input("你好世界 🦀 parent comment", cx);
        w.submit_detail_reply(cx);
    });
    assert!(
        pump_until(&mut app, Duration::from_secs(4), |cx| {
            cw.read(cx)
                .state
                .detail_post
                .as_ref()
                .is_some_and(|d| !d.comments.is_empty())
        })
        .await,
        "Human CJK reply must be persisted"
    );
    let parent_cid = cw
        .read_with(&app, |w, _| {
            w.state.detail_post.as_ref().unwrap().comments[0]
                .comment_id
                .clone()
        })
        .unwrap();

    // 2. Register a Bot, switch Act-As Bot, and post a nested child reply under parent_cid
    let _ = cw.update(&mut app, |w, cx| {
        w.state.bot_studio_name = "F17 Helper Bot".to_string();
        w.state.bot_studio_handle = "f17_helper_bot".to_string();
        w.state.bot_studio_autonomy = BotAutonomyMode::Reactive;
        w.submit_register_bot(cx);
    });
    assert!(
        pump_until(&mut app, Duration::from_secs(4), |cx| {
            cw.read(cx)
                .state
                .identities
                .as_ref()
                .is_some_and(|i| !i.bot_actors.is_empty())
        })
        .await
    );
    let bot_id = cw
        .read_with(&app, |w, _| {
            w.state.identities.as_ref().unwrap().bot_actors[0]
                .actor_id
                .clone()
        })
        .unwrap();
    let human_id = cw
        .read_with(&app, |w, _| {
            w.state
                .identities
                .as_ref()
                .unwrap()
                .human_actor
                .as_ref()
                .unwrap()
                .actor_id
                .clone()
        })
        .unwrap();

    let _ = cw.update(&mut app, |w, cx| {
        w.switch_active_actor(bot_id.clone(), cx);
        w.state.detail_reply_parent_id = Some(parent_cid.clone());
        w.state.detail_reply_draft = "Nested child reply authored as Bot citizen!".to_string();
        w.submit_detail_reply(cx);
    });
    assert!(
        pump_until(&mut app, Duration::from_secs(4), |cx| {
            cw.read(cx)
                .state
                .find_comment(&parent_cid)
                .is_some_and(|c| !c.children.is_empty())
        })
        .await,
        "Bot nested child reply must attach under parent_cid"
    );

    // 3. Collapse / Expand branch + Edit child comment + All 6 CommentSorts + XThread mode
    let child_cid = cw
        .read_with(&app, |w, _| {
            w.state.find_comment(&parent_cid).unwrap().children[0]
                .comment_id
                .clone()
        })
        .unwrap();
    let _ = cw.update(&mut app, |w, cx| {
        w.toggle_comment_collapsed(parent_cid.clone(), cx);
        assert!(w.state.detail_collapsed_comments.contains(&parent_cid));
    });
    host.draw(&mut app);
    let _ = cw.update(&mut app, |w, cx| {
        w.toggle_comment_collapsed(parent_cid.clone(), cx);
        assert!(!w.state.detail_collapsed_comments.contains(&parent_cid));
        w.start_edit_comment(child_cid.clone(), "old".to_string(), cx);
        w.state.detail_edit_comment_draft = "Edited Bot nested reply v2".to_string();
        w.submit_edit_comment(cx);
    });
    assert!(
        pump_until(&mut app, Duration::from_secs(4), |cx| {
            cw.read(cx)
                .state
                .find_comment(&child_cid)
                .is_some_and(|c| c.body.contains("Edited Bot nested reply v2"))
        })
        .await
    );

    for sort in [
        CommentSort::Top,
        CommentSort::New,
        CommentSort::Controversial,
        CommentSort::Old,
        CommentSort::Qa,
        CommentSort::Best,
    ] {
        let _ = cw.update(&mut app, |w, cx| w.set_detail_comment_sort(sort, cx));
        assert!(pump_until(&mut app, Duration::from_secs(3), |cx| !cw.read(cx).state.detail_loading).await);
        host.draw(&mut app);
    }

    // Switch back to Human, delete parent comment -> verify `[deleted]` tombstone keeps child subtree!
    let _ = cw.update(&mut app, |w, cx| {
        w.switch_active_actor(human_id.clone(), cx);
        w.delete_comment_action(parent_cid.clone(), cx);
    });
    assert!(
        pump_until(&mut app, Duration::from_secs(4), |cx| {
            cw.read(cx)
                .state
                .find_comment(&parent_cid)
                .is_some_and(|c| c.is_deleted && c.body == "[deleted]" && !c.children.is_empty())
        })
        .await,
        "Deleting a parent comment must tombstone it while preserving child subtree"
    );

    // 4. X Thread & Quote Chain mode + 1-click Summon Bot
    let _ = cw.update(&mut app, |w, cx| {
        w.set_detail_mode(PostDetailMode::XThread, cx);
        w.summon_bot_with_options(
            pid.clone(),
            Some(bot_id.clone()),
            Some("Give a concise Rust take.".to_string()),
            None,
            cx,
        );
    });
    assert!(
        pump_until(&mut app, Duration::from_secs(4), |cx| {
            !cw.read(cx).state.detail_summon_pending
                && cw.read(cx).state.detail_summon_feedback.is_some()
        })
        .await
    );
    host.draw(&mut app);

    // 5. Lock post as Owner and verify reply guard rejects submission with error toast
    let _ = cw.update(&mut app, |w, cx| {
        w.execute_post_mod_action("f17_lab".to_string(), pid.clone(), ModActionKind::Lock, cx);
    });
    assert!(
        pump_until(&mut app, Duration::from_secs(4), |cx| {
            cw.read(cx)
                .state
                .detail_post
                .as_ref()
                .is_some_and(|d| d.post.is_locked)
        })
        .await
    );
    let _ = cw.update(&mut app, |w, cx| {
        let _ = w.take_pending_actions();
        w.state.detail_reply_draft = "Should be blocked on locked post".to_string();
        w.submit_detail_reply(cx);
        let actions = w.take_pending_actions();
        assert!(
            actions
                .iter()
                .any(|a| matches!(a, CommunityWorkspaceAction::ShowToast { is_error: true, .. })),
            "Locked post must reject inline reply with error toast"
        );
    });

    // 6. Pending Summon Bot re-trigger emits informative non-error feedback + toast
    let _ = cw.update(&mut app, |w, cx| {
        let _ = w.take_pending_actions();
        w.state.detail_summon_pending = true;
        w.state.detail_summon_feedback = None;
        w.summon_bot_with_options(pid.clone(), Some(bot_id.clone()), None, None, cx);
        assert!(
            w.state
                .detail_summon_feedback
                .as_ref()
                .is_some_and(|(msg, is_err)| !msg.is_empty() && !is_err),
            "Re-triggering Summon Bot while pending must set informative feedback"
        );
        let acts = w.take_pending_actions();
        assert!(
            acts.iter()
                .any(|a| matches!(a, CommunityWorkspaceAction::ShowToast { is_error: false, .. })),
            "Re-triggering Summon Bot while pending must emit non-error toast"
        );
        w.state.detail_summon_pending = false;
    });

    // 7. Multi-page Comment Load-More Pagination (> 50 direct children) + Cross-Post Async Reload Race
    let post_b = host
        .client
        .community()
        .block_on(host.client.community().create_post(&CreatePostRequest {
            community_slug: Some("f17_lab".to_string()),
            format: PostFormat::Micro,
            body: "F17 Post B for 55-child pagination & cross-post race".to_string(),
            status: PostStatus::Published,
            ..Default::default()
        }))
        .expect("create F17 post_b")
        .post;
    let pid_b = post_b.post_id.clone();
    let p55 = host
        .client
        .community()
        .block_on(host.client.community().create_comment(
            &pid_b,
            &gpui_telegram_proto::community_contracts::CreateCommentRequest {
                parent_comment_id: None,
                body: "Parent with 55 replies".to_string(),
                media: Vec::new(),
                is_mod_distinguished: false,
            },
        ))
        .expect("create parent_55")
        .comment
        .comment_id;
    for idx in 0..55 {
        let _ = host
            .client
            .community()
            .block_on(host.client.community().create_comment(
                &pid_b,
                &gpui_telegram_proto::community_contracts::CreateCommentRequest {
                    parent_comment_id: Some(p55.clone()),
                    body: format!("Child reply #{idx}"),
                    media: Vec::new(),
                    is_mod_distinguished: false,
                },
            ))
            .expect("create child reply");
    }
    let _ = cw.update(&mut app, |w, cx| w.open_post_detail(pid_b.clone(), cx));
    assert!(
        pump_until(&mut app, Duration::from_secs(4), |cx| {
            cw.read(cx)
                .state
                .detail_post
                .as_ref()
                .is_some_and(|d| d.post.post_id == pid_b)
        })
        .await
    );
    // Simulate trimmed parent (0 loaded, 55 hidden) -> page 1 fetches 50 + stores cursor -> page 2 fetches remaining 5
    let _ = cw.update(&mut app, |w, cx| {
        w.state.visit_comment_mut(&p55, |c| {
            c.children.clear();
            c.more_children_count = 55;
        });
        w.load_more_comment_children(pid_b.clone(), p55.clone(), cx);
    });
    assert!(
        pump_until(&mut app, Duration::from_secs(4), |cx| {
            let st = &cw.read(cx).state;
            !st.detail_loading_more_comment_ids.contains(&p55)
                && st
                    .find_comment(&p55)
                    .is_some_and(|c| c.children.len() == 50 && c.more_children_count == 5)
        })
        .await,
        "First load-more page must load 50 children and retain 5 remaining"
    );
    let _ = cw.update(&mut app, |w, cx| {
        assert!(
            w.state.detail_comment_more_cursors.contains_key(&p55),
            "First load-more page must record next_cursor for parent"
        );
        w.load_more_comment_children(pid_b.clone(), p55.clone(), cx);
    });
    assert!(
        pump_until(&mut app, Duration::from_secs(4), |cx| {
            let st = &cw.read(cx).state;
            !st.detail_loading_more_comment_ids.contains(&p55)
                && st
                    .find_comment(&p55)
                    .is_some_and(|c| c.children.len() == 55 && c.more_children_count == 0)
        })
        .await,
        "Second load-more page must paginate via cursor to reach all 55 children"
    );
    let _ = cw.update(&mut app, |w, _| {
        assert!(!w.state.detail_comment_more_cursors.contains_key(&p55));
    });

    // Cross-post async reload race: reply on pid_b then immediately navigate to pid -> pid MUST win!
    let _ = cw.update(&mut app, |w, cx| {
        w.state.detail_reply_draft = "Cross-post race reply on B".to_string();
        w.submit_detail_reply(cx);
        w.open_post_detail(pid.clone(), cx);
    });
    assert!(
        pump_until(&mut app, Duration::from_secs(4), |cx| {
            let st = &cw.read(cx).state;
            !st.detail_loading
                && st.route == CommunityRoute::PostDetail(pid.clone())
                && st.detail_post.as_ref().is_some_and(|d| d.post.post_id == pid)
        })
        .await
    );
    app.background_executor().timer(Duration::from_millis(120)).await;
    let _ = cw.read_with(&app, |w, _| {
        assert_eq!(w.state.route, CommunityRoute::PostDetail(pid.clone()));
        assert_eq!(
            w.state.detail_post.as_ref().unwrap().post.post_id,
            pid,
            "Stale Post B async reply reload must NEVER overwrite active Post A detail_post"
        );
    });

    // Cross-post Summon Bot feedback isolation: summon on Post B then immediately open Post A ->
    // Post B's summon completion MUST NOT leak `detail_summon_feedback` onto Post A!
    let _ = cw.update(&mut app, |w, cx| {
        w.open_post_detail(pid_b.clone(), cx);
    });
    assert!(
        pump_until(&mut app, Duration::from_secs(4), |cx| {
            cw.read(cx)
                .state
                .detail_post
                .as_ref()
                .is_some_and(|d| d.post.post_id == pid_b)
        })
        .await
    );
    let _ = cw.update(&mut app, |w, cx| {
        w.summon_bot_with_options(
            pid_b.clone(),
            Some(bot_id.clone()),
            Some("Cross-post summon isolation test".to_string()),
            None,
            cx,
        );
        w.open_post_detail(pid.clone(), cx);
    });
    assert!(
        pump_until(&mut app, Duration::from_secs(4), |cx| {
            let st = &cw.read(cx).state;
            !st.detail_summon_pending
                && !st.detail_loading
                && st.detail_post.as_ref().is_some_and(|d| d.post.post_id == pid)
        })
        .await
    );
    app.background_executor().timer(Duration::from_millis(80)).await;
    let _ = cw.read_with(&app, |w, _| {
        assert_eq!(w.state.route, CommunityRoute::PostDetail(pid.clone()));
        assert!(
            w.state.detail_summon_feedback.is_none(),
            "Post B summon completion must NEVER leak detail_summon_feedback onto Post A, got: {:?}",
            w.state.detail_summon_feedback
        );
    });
}

// ============================================================================
// F18 — ComposerModal: All 8 Formats, Formatting Toolbar, @/#/c/ Autocomplete,
// Drafts Save/Resume/Publish, Scheduled Post, Ask Bot Co-Author, Escape Cascade
// ============================================================================

#[test]
fn test_m5_f18_composer_modal_all_formats_autocomplete_drafts_schedule_coauthor_and_escape() {
    run_in_headless_subprocess(
        "test_m5_f18_composer_modal_all_formats_autocomplete_drafts_schedule_coauthor_and_escape",
    );
}

async fn body_f18_composer_modal(mut app: AsyncApp) {
    let host = open_community_host(&mut app, 1440.0, 900.0, 7102, "m5_f18_human").await;
    let cw = host.cw.clone();
    let sample_post = cw
        .read_with(&app, |w, _| w.state.feed_items[0].post.clone())
        .unwrap();

    // 1. Open ComposerModal, test Article formatting toolbar + Media/Link/Poll/Thread validation
    let _ = cw.update(&mut app, |w, cx| {
        w.open_composer_modal(Some("rust".to_string()), cx);
        assert_eq!(w.state.active_modal, Some(CommunityModal::Composer));
        w.set_composer_tab(ComposerFormatTab::Article, cx);
        w.state.composer.title = "GPUI Deep Dive".to_string();
        w.state.composer.body = "Intro".to_string();
        w.apply_composer_format(TextStyleKind::Bold, cx);
        w.insert_composer_markdown_block("\n> Quoted design insight", cx);
        assert!(w.state.composer.body.contains("**"));
        assert!(w.state.composer.body.contains("> Quoted design insight"));
    });
    host.draw(&mut app);

    // Media validation (0 items rejected; 1 item valid)
    let now_ms = 1_770_000_000_000_i64;
    let _ = cw.update(&mut app, |w, cx| {
        w.set_composer_tab(ComposerFormatTab::Media, cx);
        w.state.composer.media_items.clear();
        assert!(w
            .build_composer_create_request(PostStatus::Published, now_ms)
            .is_err());
        w.state.composer.media_items.push(ComposerMediaDraft {
            url_or_id: "https://example.com/demo.png".to_string(),
            kind: "image".to_string(),
            alt_text: "Demo".to_string(),
        });
        assert!(w
            .build_composer_create_request(PostStatus::Published, now_ms)
            .is_ok());

        // Link validation (reject hostless & whitespace URLs)
        w.set_composer_tab(ComposerFormatTab::Link, cx);
        for bad_link in [
            "not-a-url",
            "https://",
            "http://   ",
            "https:///no-host",
            "https://bad host.com/x",
        ] {
            w.state.composer.link_url = bad_link.to_string();
            assert!(
                w.build_composer_create_request(PostStatus::Published, now_ms)
                    .is_err(),
                "Composer Link tab must reject `{bad_link}`"
            );
        }
        w.state.composer.link_url = "https://rust-lang.org".to_string();
        assert!(w
            .build_composer_create_request(PostStatus::Published, now_ms)
            .is_ok());

        // Poll validation (2..=6 options, NO silent dropping of blank option slots)
        w.set_composer_tab(ComposerFormatTab::Poll, cx);
        w.state.composer.poll_question = "Best desktop UI stack?".to_string();
        w.state.composer.poll_options = vec!["GPUI".to_string()];
        assert!(w
            .build_composer_create_request(PostStatus::Published, now_ms)
            .is_err());
        w.state.composer.poll_options = vec![
            "GPUI".to_string(),
            "   ".to_string(),
            "Qt".to_string(),
        ];
        assert!(
            w.build_composer_create_request(PostStatus::Published, now_ms)
                .is_err(),
            "Blank poll option slot must be rejected instead of silently dropped"
        );
        w.state.composer.poll_options =
            vec!["GPUI".to_string(), "SwiftUI".to_string(), "Qt".to_string()];
        assert!(w
            .build_composer_create_request(PostStatus::Published, now_ms)
            .is_ok());

        // Thread 1/N validation (0 follow-up parts rejected; 1+ non-empty part accepted)
        w.set_composer_tab(ComposerFormatTab::Thread, cx);
        w.state.composer.body = "Part 1 of thread".to_string();
        w.state.composer.thread_parts.clear();
        assert!(
            w.build_composer_create_request(PostStatus::Published, now_ms)
                .is_err(),
            "Thread with 0 follow-up parts must be rejected"
        );
        w.state.composer.thread_parts = vec!["Part 2 of thread".to_string()];
        let req = w
            .build_composer_create_request(PostStatus::Published, now_ms)
            .expect("valid thread request");
        assert_eq!(req.thread_parts.len(), 1);
    });
    host.draw(&mut app);

    // Quote & Crosspost tabs
    let _ = cw.update(&mut app, |w, cx| {
        w.open_composer_quote(sample_post.clone(), cx);
        assert_eq!(w.state.composer.tab, ComposerFormatTab::Quote);
        w.state.composer.body = "Quoting with insight".to_string();
        assert!(w
            .build_composer_create_request(PostStatus::Published, now_ms)
            .is_ok());

        w.open_composer_crosspost(sample_post.clone(), cx);
        assert_eq!(w.state.composer.tab, ComposerFormatTab::Crosspost);
        w.state.composer.community_slug = Some("ai".to_string());
        assert!(w
            .build_composer_create_request(PostStatus::Published, now_ms)
            .is_ok());
    });
    host.draw(&mut app);

    // 2. Live `@/#`/`c/` autocomplete (English + CJK + localized hashtag subtitle) + Visual Z-Order Escape cascade
    let _ = cw.update(&mut app, |w, cx| {
        w.set_language("zh-CN");
        w.open_composer_modal(Some("rust".to_string()), cx);
        w.state.focus_input(ActiveCommunityInput::ComposerBody);
        w.insert_text_to_active_input("你好 #brandnewtag_zh", cx);
        let ac_tag = compute_composer_autocomplete(w).expect("hashtag autocomplete");
        assert_eq!(ac_tag.1[0].subtitle, "新建话题标签");

        w.state.composer.body.clear();
        w.insert_text_to_active_input("你好 c/r", cx);
        let ac = compute_composer_autocomplete(w).expect("c/ autocomplete must trigger");
        assert!(!ac.1.is_empty());
        let (start, token) = (ac.0, ac.1[0].insert_token.clone());
        // Popover rendered on top of Composer modal MUST be dismissed by Escape BEFORE occluded autocomplete!
        w.toggle_popover(CommunityPopover::IdentitySwitcher, cx);
        assert_eq!(w.state.active_popover, Some(CommunityPopover::IdentitySwitcher));
        assert!(w.handle_escape_key(cx));
        assert_eq!(
            w.state.active_popover, None,
            "Escape #1 must close topmost popover above Composer"
        );
        assert!(
            !w.state.composer.autocomplete_dismissed,
            "Occluded autocomplete must remain untouched until topmost popover is closed"
        );
        // Escape #2 now dismisses autocomplete while keeping modal open
        assert!(w.handle_escape_key(cx));
        assert_eq!(w.state.active_modal, Some(CommunityModal::Composer));
        assert!(w.state.composer.autocomplete_dismissed);
        // Apply autocomplete directly
        w.state.composer.autocomplete_dismissed = false;
        w.apply_composer_autocomplete(start, token, cx);
        assert!(w.state.composer.body.contains("c/"));
        // Escape #3 closes Composer modal while staying inside Community
        assert!(w.handle_escape_key(cx));
        assert_eq!(w.state.active_modal, None);
    });

    // 3. Save Draft -> Load Drafts -> Resume Draft -> Publish Now + Ask Bot Co-Author
    let _ = cw.update(&mut app, |w, cx| {
        w.open_composer_modal(Some("rust".to_string()), cx);
        w.state.composer.body = "Draft body for F18 resume & publish".to_string();
        w.generate_bot_coauthor_suggestion(cx);
        assert!(w.state.composer.bot_coauthor_suggestion.is_some());
        w.submit_composer(PostStatus::Draft, cx);
    });
    assert!(
        pump_until(&mut app, Duration::from_secs(4), |cx| {
            !cw.read(cx).state.composer.submitting && cw.read(cx).state.active_modal.is_none()
        })
        .await
    );
    let _ = cw.update(&mut app, |w, cx| {
        w.open_composer_modal(None, cx);
        w.load_composer_drafts(cx);
    });
    assert!(
        pump_until(&mut app, Duration::from_secs(4), |cx| {
            !cw.read(cx).state.composer.drafts_loading && !cw.read(cx).state.composer.drafts.is_empty()
        })
        .await,
        "Saved draft must appear in Drafts drawer"
    );
    let draft_post = cw
        .read_with(&app, |w, _| w.state.composer.drafts[0].clone())
        .unwrap();
    let _ = cw.update(&mut app, |w, cx| {
        w.resume_draft_into_composer(draft_post.clone(), cx);
        assert_eq!(w.state.composer.editing_post_id.as_deref(), Some(draft_post.post_id.as_str()));
        w.publish_draft_now(draft_post.post_id.clone(), cx);
    });
    assert!(
        pump_until(&mut app, Duration::from_secs(4), |cx| {
            !cw.read(cx)
                .state
                .composer
                .drafts
                .iter()
                .any(|d| d.post_id == draft_post.post_id)
        })
        .await,
        "Published draft must leave drafts list"
    );

    // 4. Scheduled post validation (past/zero rejected; future succeeds)
    let _ = cw.update(&mut app, |w, cx| {
        w.open_composer_modal(Some("rust".to_string()), cx);
        w.state.composer.body = "Scheduled post F18".to_string();
        w.state.composer.schedule_enabled = true;
        w.state.composer.schedule_minutes_input = "0".to_string();
        assert!(w
            .build_composer_create_request(PostStatus::Scheduled, now_ms)
            .is_err());
        w.state.composer.schedule_minutes_input = "30".to_string();
        w.submit_composer(PostStatus::Scheduled, cx);
    });
    assert!(
        pump_until(&mut app, Duration::from_secs(4), |cx| {
            !cw.read(cx).state.composer.submitting && cw.read(cx).state.active_modal.is_none()
        })
        .await
    );

    // 5. Draft Resume Hydration (Poll fields preserved) & Edit Validation (Article empty body rejected)
    let _ = cw.update(&mut app, |w, cx| {
        w.open_composer_modal(Some("rust".to_string()), cx);
        w.set_composer_tab(ComposerFormatTab::Poll, cx);
        w.state.composer.poll_question = "Draft poll question?".to_string();
        w.state.composer.poll_options = vec!["Alpha".to_string(), "Beta".to_string()];
        w.state.composer.poll_multi_select = true;
        w.submit_composer(PostStatus::Draft, cx);
    });
    assert!(
        pump_until(&mut app, Duration::from_secs(4), |cx| {
            !cw.read(cx).state.composer.submitting && cw.read(cx).state.active_modal.is_none()
        })
        .await
    );
    let _ = cw.update(&mut app, |w, cx| {
        w.open_composer_modal(None, cx);
        w.load_composer_drafts(cx);
    });
    assert!(
        pump_until(&mut app, Duration::from_secs(4), |cx| {
            !cw.read(cx).state.composer.drafts_loading
                && cw
                    .read(cx)
                    .state
                    .composer
                    .drafts
                    .iter()
                    .any(|d| d.format == PostFormat::Poll)
        })
        .await
    );
    let poll_draft = cw
        .read_with(&app, |w, _| {
            w.state
                .composer
                .drafts
                .iter()
                .find(|d| d.format == PostFormat::Poll)
                .cloned()
                .unwrap()
        })
        .unwrap();
    let _ = cw.update(&mut app, |w, cx| {
        w.resume_draft_into_composer(poll_draft, cx);
        assert_eq!(w.state.composer.tab, ComposerFormatTab::Poll);
        assert_eq!(w.state.composer.poll_question, "Draft poll question?");
        assert_eq!(
            w.state.composer.poll_options,
            vec!["Alpha".to_string(), "Beta".to_string()]
        );
        assert!(w.state.composer.poll_multi_select);

        // Save Article draft -> resume -> clear body -> Publish MUST be rejected!
        w.open_composer_modal(Some("rust".to_string()), cx);
        w.set_composer_tab(ComposerFormatTab::Article, cx);
        w.state.composer.title = "Article Draft Title".to_string();
        w.state.composer.body = "Original article draft body".to_string();
        w.submit_composer(PostStatus::Draft, cx);
    });
    assert!(
        pump_until(&mut app, Duration::from_secs(4), |cx| {
            !cw.read(cx).state.composer.submitting && cw.read(cx).state.active_modal.is_none()
        })
        .await
    );
    let _ = cw.update(&mut app, |w, cx| {
        w.open_composer_modal(None, cx);
        w.load_composer_drafts(cx);
    });
    assert!(
        pump_until(&mut app, Duration::from_secs(4), |cx| {
            !cw.read(cx).state.composer.drafts_loading
                && cw
                    .read(cx)
                    .state
                    .composer
                    .drafts
                    .iter()
                    .any(|d| d.format == PostFormat::Article)
        })
        .await
    );
    let art_draft = cw
        .read_with(&app, |w, _| {
            w.state
                .composer
                .drafts
                .iter()
                .find(|d| d.format == PostFormat::Article)
                .cloned()
                .unwrap()
        })
        .unwrap();
    let _ = cw.update(&mut app, |w, cx| {
        w.resume_draft_into_composer(art_draft, cx);
        assert_eq!(w.state.composer.tab, ComposerFormatTab::Article);
        // Mid-edit tab switching MUST be locked (immutable PostFormat) so Article validation cannot be bypassed!
        w.set_composer_tab(ComposerFormatTab::Media, cx);
        assert_eq!(
            w.state.composer.tab,
            ComposerFormatTab::Article,
            "set_composer_tab must be a no-op while editing an existing post/draft"
        );
        w.state.composer.body = "   ".to_string();
        w.submit_composer(PostStatus::Published, cx);
        assert!(
            w.state.composer.validation_error.is_some() && !w.state.composer.submitting,
            "Editing/publishing an Article draft with empty body must be rejected even after tab switch attempt"
        );
        assert_eq!(w.state.active_modal, Some(CommunityModal::Composer));

        // Scoped `has_payload` check: editing a Media post with stale link_url but NO media/title/body must reject!
        let mut fake_media_post = sample_post.clone();
        fake_media_post.format = PostFormat::Media;
        fake_media_post.title = None;
        fake_media_post.body = String::new();
        fake_media_post.media.clear();
        w.open_composer_edit(fake_media_post, cx);
        assert_eq!(w.state.composer.tab, ComposerFormatTab::Media);
        w.state.composer.link_url = "https://stale-other-tab.example.com".to_string();
        w.submit_composer(PostStatus::Published, cx);
        assert!(
            w.state.composer.validation_error.is_some() && !w.state.composer.submitting,
            "Media edit must not treat stale link_url from another tab as valid Media payload"
        );
    });
    host.draw(&mut app);
}

// ============================================================================
// F19 — CommunityPageView: Create Community Modal, Banner/Stats/Pinned,
// Join/Bell, Flairs, Rules CRUD, Versioned Wiki & Non-Mod Guard, Mods Roster
// ============================================================================

#[test]
fn test_m5_f19_community_page_create_flairs_rules_wiki_revisions_and_mod_guards() {
    run_in_headless_subprocess(
        "test_m5_f19_community_page_create_flairs_rules_wiki_revisions_and_mod_guards",
    );
}

async fn body_f19_community_page(mut app: AsyncApp) {
    let host = open_community_host(&mut app, 1440.0, 900.0, 7103, "m5_f19_human").await;
    let cw = host.cw.clone();

    // 1. Create Community Modal validation (reject invalid & uppercase slugs preserving entered fields) -> then create c/m5_arena
    let _ = cw.update(&mut app, |w, cx| {
        w.open_create_community_modal(cx);
        assert_eq!(w.state.active_modal, Some(CommunityModal::CreateCommunity));
        w.state.create_community.slug = "bad slug!!".to_string();
        w.state.create_community.name = "Bad".to_string();
        w.submit_create_community(cx);
        assert!(
            w.state.create_community.error.is_some(),
            "Invalid slug must produce inline validation error"
        );
        assert_eq!(w.state.active_modal, Some(CommunityModal::CreateCommunity));

        w.state.create_community.slug = "UPPER_SLUG".to_string();
        w.state.create_community.name = "Preserved Name".to_string();
        w.state.create_community.description = "Preserved Description".to_string();
        w.state.create_community.error = None;
        w.submit_create_community(cx);
        assert!(
            w.state.create_community.error.is_some(),
            "Uppercase slug must be rejected inline"
        );
        assert_eq!(w.state.active_modal, Some(CommunityModal::CreateCommunity));
        assert_eq!(w.state.create_community.slug, "UPPER_SLUG");
        assert_eq!(w.state.create_community.name, "Preserved Name");
        assert_eq!(w.state.create_community.description, "Preserved Description");

        w.state.create_community.slug = "C/mixed_prefix".to_string();
        w.state.create_community.error = None;
        w.submit_create_community(cx);
        assert!(
            w.state.create_community.error.is_some(),
            "Uppercase C/ prefix must be rejected inline"
        );

        w.state.create_community.slug = "m5_arena".to_string();
        w.state.create_community.name = "M5 Community Arena".to_string();
        w.state.create_community.description = "Human + Bot collaborative space".to_string();
        w.submit_create_community(cx);
    });
    assert!(
        pump_until(&mut app, Duration::from_secs(4), |cx| {
            cw.read(cx).state.route == CommunityRoute::CommunityPage("m5_arena".to_string())
                && cw
                    .read(cx)
                    .state
                    .community_detail
                    .as_ref()
                    .is_some_and(|d| d.community.slug == "m5_arena")
        })
        .await,
        "Creating c/m5_arena must navigate directly to its CommunityPage"
    );
    host.draw(&mut app);

    // 2. Bell toggle + Flair filter + Rules CRUD (as Owner)
    let _ = cw.update(&mut app, |w, cx| {
        w.toggle_community_bell("m5_arena".to_string(), cx);
        w.set_community_flair_filter("m5_arena".to_string(), Some("RFC".to_string()), cx);
        w.set_community_tab(CommunityPageTab::Rules, cx);
        w.state.community_new_rule_title = "Respect Human & Bot Citizens".to_string();
        w.state.community_new_rule_desc = "Keep debates constructive and evidence-backed.".to_string();
        w.add_community_rule("m5_arena".to_string(), cx);
    });
    assert!(
        pump_until(&mut app, Duration::from_secs(4), |cx| {
            cw.read(cx)
                .state
                .community_detail
                .as_ref()
                .is_some_and(|d| !d.community.rules.is_empty())
        })
        .await,
        "Owner rule addition must persist into community_detail.community.rules"
    );
    host.draw(&mut app);

    // Remove rule & verify
    let _ = cw.update(&mut app, |w, cx| {
        w.remove_community_rule("m5_arena".to_string(), 0, cx);
    });
    assert!(
        pump_until(&mut app, Duration::from_secs(4), |cx| {
            cw.read(cx)
                .state
                .community_detail
                .as_ref()
                .is_some_and(|d| d.community.rules.is_empty())
        })
        .await
    );

    // 3. Versioned Wiki Edit (Owner creates revision #2) + Mods tab render
    let _ = cw.update(&mut app, |w, cx| {
        w.set_community_tab(CommunityPageTab::Wiki, cx);
        w.start_edit_community_wiki(Some("# M5 Arena Wiki\n\nUpdated by Owner.".to_string()), cx);
        assert!(w.state.community_wiki_editing);
        w.state.community_wiki_summary = "Add M5 guidelines".to_string();
        w.save_community_wiki("m5_arena".to_string(), cx);
    });
    assert!(
        pump_until(&mut app, Duration::from_secs(4), |cx| {
            cw.read(cx)
                .state
                .community_detail
                .as_ref()
                .is_some_and(|d| d.community.wiki_revision >= 2 && !d.wiki_revisions.is_empty())
        })
        .await,
        "Owner wiki edit must increment wiki_revision and append revision history"
    );
    host.draw(&mut app);

    let _ = cw.update(&mut app, |w, cx| {
        w.set_community_tab(CommunityPageTab::Mods, cx);
    });
    host.draw(&mut app);

    // 4. Non-mod Wiki edit guard on a seeded community (e.g. c/rust where fresh user is not mod)
    let seeded_slug = cw
        .read_with(&app, |w, _| {
            w.state
                .communities
                .iter()
                .find(|c| c.slug != "m5_arena" && c.viewer_role.is_none_or(|r| !r.is_moderator()))
                .map(|c| c.slug.clone())
                .unwrap_or_else(|| "rust".to_string())
        })
        .unwrap();
    let _ = cw.update(&mut app, |w, cx| {
        w.navigate_to(CommunityRoute::CommunityPage(seeded_slug.clone()), cx);
    });
    assert!(
        pump_until(&mut app, Duration::from_secs(4), |cx| {
            cw.read(cx)
                .state
                .community_detail
                .as_ref()
                .is_some_and(|d| d.community.slug == seeded_slug)
        })
        .await
    );
    let _ = cw.update(&mut app, |w, cx| {
        w.set_community_tab(CommunityPageTab::Wiki, cx);
        w.state.community_wiki_editing = false;
        w.start_edit_community_wiki(None, cx);
        assert!(
            !w.state.community_wiki_editing,
            "Non-mod MUST NOT enter wiki editing mode"
        );
        // Also verify Join / Leave toggles cleanly
        let joined_before = w
            .state
            .community_detail
            .as_ref()
            .unwrap()
            .community
            .is_joined;
        w.toggle_join_community(seeded_slug.clone(), !joined_before, cx);
    });
    assert!(
        pump_until(&mut app, Duration::from_secs(4), |cx| {
            cw.read(cx).state.community_detail.is_some()
        })
        .await
    );
    host.draw(&mut app);

    // 5. Nonexistent c/does_not_exist renders clean not-found fallback without crashing
    let _ = cw.update(&mut app, |w, cx| {
        w.navigate_to(CommunityRoute::CommunityPage("does_not_exist_m5".to_string()), cx);
    });
    assert!(
        pump_until(&mut app, Duration::from_secs(4), |cx| {
            let st = &cw.read(cx).state;
            !st.community_loading && st.community_detail.is_none() && st.community_error.is_some()
        })
        .await,
        "Nonexistent c/ must resolve cleanly to not-found fallback"
    );
    host.draw(&mut app);
}

// ============================================================================
// F20 — ActorProfileView, NotificationsView & BookmarksListsView
// ============================================================================

#[test]
fn test_m5_f20_profile_dm_bridge_notifications_bookmarks_folders_and_lists_timeline() {
    run_in_headless_subprocess(
        "test_m5_f20_profile_dm_bridge_notifications_bookmarks_folders_and_lists_timeline",
    );
}

async fn body_f20_profile_notifs_bookmarks_lists(mut app: AsyncApp) {
    let host = open_community_host(&mut app, 1440.0, 900.0, 7104, "m5_f20_human").await;
    let cw = host.cw.clone();

    // 1. ActorProfileView: open a Bot profile + test Follow / Mute / Block + Tabs + Own Edit Profile
    let target_bot = cw
        .read_with(&app, |w, _| {
            w.state
                .recommended_actors
                .iter()
                .find(|a| a.actor_kind.is_bot_like())
                .cloned()
                .expect("seeded bot actor exists")
        })
        .unwrap();
    let _ = cw.update(&mut app, |w, cx| {
        w.open_actor_profile(target_bot.handle.clone(), cx);
    });
    assert!(
        pump_until(&mut app, Duration::from_secs(4), |cx| {
            cw.read(cx)
                .state
                .actor_profile
                .as_ref()
                .is_some_and(|p| p.actor.handle == target_bot.handle)
        })
        .await
    );
    let _ = cw.update(&mut app, |w, cx| {
        for tab in [
            ActorProfileTab::Posts,
            ActorProfileTab::Replies,
            ActorProfileTab::Threads,
            ActorProfileTab::Media,
            ActorProfileTab::BotActivity,
        ] {
            w.set_profile_tab(tab, cx);
        }
        w.toggle_mute_actor(target_bot.actor_id.clone(), false, cx);
    });
    assert!(
        pump_until(&mut app, Duration::from_secs(4), |cx| {
            cw.read(cx)
                .state
                .actor_profile
                .as_ref()
                .is_some_and(|p| p.actor.is_muted)
        })
        .await,
        "Mute actor must update actor_profile.actor.is_muted"
    );
    let _ = cw.update(&mut app, |w, cx| {
        w.toggle_mute_actor(target_bot.actor_id.clone(), true, cx);
        w.toggle_block_actor(target_bot.actor_id.clone(), false, cx);
    });
    assert!(
        pump_until(&mut app, Duration::from_secs(4), |cx| {
            cw.read(cx)
                .state
                .actor_profile
                .as_ref()
                .is_some_and(|p| p.actor.is_blocked)
        })
        .await
    );
    let _ = cw.update(&mut app, |w, cx| {
        w.toggle_block_actor(target_bot.actor_id.clone(), true, cx);
        if let Some(ref mut prof) = w.state.actor_profile {
            prof.actor.trust_score = f64::NAN;
        }
        w.set_profile_tab(ActorProfileTab::Likes, cx);
    });
    host.draw(&mut app);
    let _ = cw.update(&mut app, |w, cx| {
        w.set_profile_tab(ActorProfileTab::Upvoted, cx);
    });
    host.draw(&mut app);

    // Edit own human profile + verify OpenDmWithActor bridge action
    let own_human = cw
        .read_with(&app, |w, _| {
            w.state
                .identities
                .as_ref()
                .unwrap()
                .human_actor
                .clone()
                .expect("human actor")
        })
        .unwrap();
    let _ = cw.update(&mut app, |w, cx| {
        w.open_actor_profile(own_human.handle.clone(), cx);
        w.open_edit_profile_modal(own_human.clone(), cx);
        assert_eq!(w.state.active_modal, Some(CommunityModal::EditProfile));
        w.state.edit_profile.display_name = "M5 F20 Updated Human".to_string();
        w.state.edit_profile.bio = "Testing unified Reddit + X profiles".to_string();
        w.submit_edit_profile(cx);
    });
    assert!(
        pump_until(&mut app, Duration::from_secs(4), |cx| {
            !cw.read(cx).state.edit_profile.submitting && cw.read(cx).state.active_modal.is_none()
        })
        .await
    );
    let _ = cw.update(&mut app, |w, cx| {
        let _ = w.take_pending_actions();
        w.emit_action(CommunityWorkspaceAction::OpenDmWithActor { linked_user_id: 42 }, cx);
        let acts = w.take_pending_actions();
        assert!(acts
            .iter()
            .any(|a| matches!(a, CommunityWorkspaceAction::OpenDmWithActor { linked_user_id: 42 })));
    });

    // 2. NotificationsView: Category Filter, Mark Single Read, Mark All Read, Jump to Post+Comment
    let sample_pid = cw
        .read_with(&app, |w, _| w.state.feed_items[0].post.post_id.clone())
        .unwrap();
    let _ = cw.update(&mut app, |w, cx| {
        w.navigate_to(CommunityRoute::Notifications, cx);
        w.state.notifications = vec![
            CommunityNotificationDto {
                notif_id: "n_f20_1".to_string(),
                recipient_actor_id: own_human.actor_id.clone(),
                actor_id: target_bot.actor_id.clone(),
                actor_handle: target_bot.handle.clone(),
                actor_display_name: target_bot.display_name.clone(),
                actor_kind: ActorKind::Bot,
                kind: CommunityNotificationKind::BotReply,
                target_post_id: Some(sample_pid.clone()),
                target_comment_id: Some("c_focus_42".to_string()),
                preview_text: "Bot replied to your benchmark".to_string(),
                is_read: false,
                created_at_ms: 1_700_000_000_000,
            },
            CommunityNotificationDto {
                notif_id: "n_f20_2".to_string(),
                recipient_actor_id: own_human.actor_id.clone(),
                actor_id: target_bot.actor_id.clone(),
                actor_handle: target_bot.handle.clone(),
                actor_display_name: target_bot.display_name.clone(),
                actor_kind: ActorKind::Bot,
                kind: CommunityNotificationKind::Follow,
                target_post_id: None,
                target_comment_id: None,
                preview_text: "started following you".to_string(),
                is_read: false,
                created_at_ms: 1_700_000_001_000,
            },
        ];
        w.state.unread_notifications = 2;
        w.set_notification_category(NotificationCategoryTab::BotActivity, cx);
    });
    host.draw(&mut app);

    let _ = cw.update(&mut app, |w, cx| {
        let notif = w.state.notifications[0].clone();
        let _ = w.take_pending_actions();
        w.open_notification_target(notif, cx);
        assert_eq!(w.state.route, CommunityRoute::PostDetail(sample_pid.clone()));
        assert_eq!(w.state.detail_focused_comment_id.as_deref(), Some("c_focus_42"));
        assert!(w.state.notifications[0].is_read);
        let acts = w.take_pending_actions();
        assert!(acts
            .iter()
            .any(|a| matches!(a, CommunityWorkspaceAction::UnreadCountChanged(1))));

        w.navigate_to(CommunityRoute::Notifications, cx);
        w.mark_all_notifications_read(cx);
        assert!(!w.state.notifications_loading);
    });
    assert!(
        pump_until(&mut app, Duration::from_secs(3), |cx| {
            cw.read(cx).state.unread_notifications == 0
        })
        .await
    );
    // Mark all read with 0 unread stays clean no-op
    let _ = cw.update(&mut app, |w, cx| w.mark_all_notifications_read(cx));
    assert!(
        pump_until(&mut app, Duration::from_secs(2), |cx| {
            !cw.read(cx).state.notifications_loading && cw.read(cx).state.unread_notifications == 0
        })
        .await
    );
    // Jumping from a notification targeting a deleted/missing post displays toast + not-found fallback
    let _ = host.shell.update(&mut app, |s, _| {
        s.window_runtime.controller.current_toast = None;
        s.window_runtime.controller.toast_queue.clear();
    });
    let _ = cw.update(&mut app, |w, cx| {
        let _ = w.take_pending_actions();
        w.open_notification_target(
            CommunityNotificationDto {
                notif_id: "n_deleted_target".to_string(),
                recipient_actor_id: own_human.actor_id.clone(),
                actor_id: target_bot.actor_id.clone(),
                actor_handle: target_bot.handle.clone(),
                actor_display_name: target_bot.display_name.clone(),
                actor_kind: ActorKind::Bot,
                kind: CommunityNotificationKind::Reply,
                target_post_id: Some("deleted_post_does_not_exist".to_string()),
                target_comment_id: Some("c_deleted".to_string()),
                preview_text: "reply on deleted post".to_string(),
                is_read: true,
                created_at_ms: 1_700_000_002_000,
            },
            cx,
        );
    });
    assert!(
        pump_until(&mut app, Duration::from_secs(4), |cx| {
            let st = &cw.read(cx).state;
            !st.detail_loading && st.detail_post.is_none() && st.detail_error.is_some()
        })
        .await,
        "Deleted notification target must resolve to graceful not-found fallback"
    );
    assert!(
        pump_until(&mut app, Duration::from_secs(2), |cx| {
            let ctrl = &host.shell.read(cx).window_runtime.controller;
            ctrl.current_toast.is_some() || !ctrl.toast_queue.is_empty()
        })
        .await,
        "Deleted notification target must route graceful error toast into AppShell"
    );
    // Jumping from a post-level notification (target_comment_id == None) targeting a deleted post ALSO emits error toast!
    let _ = host.shell.update(&mut app, |s, _| {
        s.window_runtime.controller.current_toast = None;
        s.window_runtime.controller.toast_queue.clear();
    });
    let _ = cw.update(&mut app, |w, cx| {
        let _ = w.take_pending_actions();
        w.open_notification_target(
            CommunityNotificationDto {
                notif_id: "n_deleted_post_level".to_string(),
                recipient_actor_id: own_human.actor_id.clone(),
                actor_id: target_bot.actor_id.clone(),
                actor_handle: target_bot.handle.clone(),
                actor_display_name: target_bot.display_name.clone(),
                actor_kind: ActorKind::Bot,
                kind: CommunityNotificationKind::Like,
                target_post_id: Some("deleted_post_level_only_xyz".to_string()),
                target_comment_id: None,
                preview_text: "liked your deleted post".to_string(),
                is_read: true,
                created_at_ms: 1_700_000_003_000,
            },
            cx,
        );
    });
    assert!(
        pump_until(&mut app, Duration::from_secs(4), |cx| {
            let st = &cw.read(cx).state;
            !st.detail_loading && st.detail_post.is_none() && st.detail_error.is_some()
        })
        .await,
        "Deleted post-level notification must resolve to not-found fallback"
    );
    assert!(
        pump_until(&mut app, Duration::from_secs(2), |cx| {
            let ctrl = &host.shell.read(cx).window_runtime.controller;
            ctrl.current_toast.is_some() || !ctrl.toast_queue.is_empty()
        })
        .await,
        "Deleted post-level notification (target_comment_id = None) MUST emit error toast"
    );
    // Direct navigation to another post clears any stale detail_focused_comment_id
    let other_pid = cw
        .read_with(&app, |w, _| {
            w.state
                .feed_items
                .get(1)
                .map(|i| i.post.post_id.clone())
                .unwrap_or_else(|| sample_pid.clone())
        })
        .unwrap();
    let _ = cw.update(&mut app, |w, cx| {
        w.state.detail_focused_comment_id = Some("c_stale_leak".to_string());
        w.open_post_detail(other_pid.clone(), cx);
        assert!(
            w.state.detail_focused_comment_id.is_none(),
            "Direct navigation to another post must clear stale detail_focused_comment_id"
        );

        // In-flight notification jump (Post A) immediately superseded by direct open_post_detail(Post B):
        // `detail_opened_from_notification` must be consumed synchronously so Post B clears Post A's focused comment!
        w.open_notification_target(
            CommunityNotificationDto {
                notif_id: "n_inflight_race".to_string(),
                recipient_actor_id: own_human.actor_id.clone(),
                actor_id: target_bot.actor_id.clone(),
                actor_handle: target_bot.handle.clone(),
                actor_display_name: target_bot.display_name.clone(),
                actor_kind: ActorKind::Bot,
                kind: CommunityNotificationKind::Reply,
                target_post_id: Some(sample_pid.clone()),
                target_comment_id: Some("c_race_stale".to_string()),
                preview_text: "in-flight race notification".to_string(),
                is_read: true,
                created_at_ms: 1_700_000_004_000,
            },
            cx,
        );
        assert!(
            !w.state.detail_opened_from_notification,
            "load_post_detail must consume detail_opened_from_notification synchronously"
        );
        w.open_post_detail(other_pid.clone(), cx);
        assert!(
            w.state.detail_focused_comment_id.is_none(),
            "Immediate direct open_post_detail after in-flight notification jump must clear stale focused comment"
        );
    });
    assert!(
        pump_until(&mut app, Duration::from_secs(4), |cx| {
            let st = &cw.read(cx).state;
            !st.detail_loading && st.detail_post.as_ref().is_some_and(|d| d.post.post_id == other_pid)
        })
        .await
    );
    let _ = cw.read_with(&app, |w, _| {
        assert!(
            w.state.detail_focused_comment_id.is_none(),
            "Post B must settle with no leaked detail_focused_comment_id"
        );
    });
    host.draw(&mut app);

    // Nonexistent @missing_handle profile renders clean not-found fallback
    let _ = cw.update(&mut app, |w, cx| {
        w.open_actor_profile("missing_handle_m5_xyz".to_string(), cx);
    });
    assert!(
        pump_until(&mut app, Duration::from_secs(4), |cx| {
            let st = &cw.read(cx).state;
            !st.profile_loading && st.actor_profile.is_none() && st.profile_error.is_some()
        })
        .await,
        "Nonexistent @handle profile must resolve cleanly to not-found fallback"
    );
    host.draw(&mut app);

    // 3. BookmarksListsView: Custom Folders (Create/Move/Remove) + PostCard toggle_bookmark removal + Lists CRUD + Optimistic Membership + Timeline
    let _ = cw.update(&mut app, |w, cx| {
        w.move_bookmark_to_folder(sample_pid.clone(), "RustDeep".to_string(), cx);
    });
    assert!(
        pump_until(&mut app, Duration::from_secs(4), |cx| {
            cw.read(cx)
                .state
                .find_post(&sample_pid)
                .is_some_and(|p| p.viewer_bookmarked)
        })
        .await
    );
    let _ = cw.update(&mut app, |w, cx| {
        w.navigate_to(CommunityRoute::BookmarksLists, cx);
        w.state.bookmark_new_folder_draft = "Systems".to_string();
        w.create_bookmark_folder(cx);
        assert!(w
            .state
            .bookmark_custom_folders
            .iter()
            .any(|f| f == "Systems"));
        w.select_bookmark_folder("RustDeep".to_string(), cx);
    });
    assert!(
        pump_until(&mut app, Duration::from_secs(4), |cx| {
            !cw.read(cx).state.saved_loading
                && cw
                    .read(cx)
                    .state
                    .saved_items
                    .iter()
                    .any(|it| it.post.post_id == sample_pid)
        })
        .await
    );
    host.draw(&mut app);

    // Unbookmarking via PostCard's toggle_bookmark MUST remove post from saved_items!
    let _ = cw.update(&mut app, |w, cx| {
        w.toggle_bookmark(sample_pid.clone(), cx);
    });
    assert!(
        pump_until(&mut app, Duration::from_secs(4), |cx| {
            !cw.read(cx)
                .state
                .saved_items
                .iter()
                .any(|it| it.post.post_id == sample_pid)
        })
        .await,
        "Unbookmarking via PostCard toggle_bookmark must remove post from saved_items"
    );

    let _ = cw.update(&mut app, |w, cx| {
        // Switch to Lists & Custom Feeds: reject empty name, then create list with community + actor
        w.set_bookmarks_section(BookmarksListsSection::Lists, cx);
        w.open_create_list_form(cx);
        w.state.list_editor.name = "   ".to_string();
        w.submit_list_editor(cx);
        assert!(w.state.list_editor.error.is_some());

        w.state.list_editor.name = "M5 Rust & Bots Feed".to_string();
        w.state.list_editor.description = "Curated Human+Bot engineering stream".to_string();
        w.state.list_editor.is_pinned = true;
        w.state.list_editor.community_slugs = vec!["rust".to_string()];
        w.submit_list_editor(cx);
    });
    assert!(
        pump_until(&mut app, Duration::from_secs(4), |cx| {
            cw.read(cx)
                .state
                .lists
                .iter()
                .any(|l| l.name == "M5 Rust & Bots Feed")
        })
        .await,
        "Created custom list must persist into state.lists"
    );
    let list_id = cw
        .read_with(&app, |w, _| {
            w.state
                .lists
                .iter()
                .find(|l| l.name == "M5 Rust & Bots Feed")
                .unwrap()
                .list_id
                .clone()
        })
        .unwrap();
    // Rapid consecutive chip toggles must optimistically update state.lists without clobbering each other
    let _ = cw.update(&mut app, |w, cx| {
        w.toggle_selected_list_actor(list_id.clone(), target_bot.actor_id.clone(), cx);
        w.toggle_selected_list_community(list_id.clone(), "systems".to_string(), cx);
        let l = w
            .state
            .lists
            .iter()
            .find(|x| x.list_id == list_id)
            .unwrap();
        assert!(l.member_actor_ids.contains(&target_bot.actor_id));
        assert!(l.community_slugs.iter().any(|s| s == "systems"));
        w.toggle_list_pinned(list_id.clone(), false, cx);
    });
    assert!(
        pump_until(&mut app, Duration::from_secs(4), |cx| {
            let st = &cw.read(cx).state;
            !st.list_membership_in_flight.contains_key(&list_id)
                && !st.list_membership_dirty.contains(&list_id)
                && !st.list_feed_loading
        })
        .await
    );
    let facade_for_f20 = cw.read_with(&app, |w, _| w.facade.clone()).unwrap();
    let db_lists_f20 = fetch_db_lists(&facade_for_f20).await;
    let db_list_f20 = db_lists_f20
        .iter()
        .find(|x| x.list_id == list_id)
        .expect("list must persist in SQLite");
    assert!(db_list_f20.member_actor_ids.contains(&target_bot.actor_id));
    assert!(db_list_f20.community_slugs.iter().any(|s| s == "systems"));
    assert!(!db_list_f20.is_pinned);
    host.draw(&mut app);
    let _ = cw.update(&mut app, |w, cx| w.delete_list_action(list_id.clone(), cx));
    assert!(
        pump_until(&mut app, Duration::from_secs(4), |cx| {
            !cw.read(cx).state.lists.iter().any(|l| l.list_id == list_id)
        })
        .await
    );
}

// ============================================================================
// F21 — ExploreSearchView, ModQueueView (Queue + AutoMod + Dry-Run + Log),
// and BotStudioView (Register + 8×11 Grok Clamp + Edit + Snippet + Playground)
// ============================================================================

#[test]
fn test_m5_f21_explore_search_mod_console_automod_dryrun_and_bot_studio() {
    run_in_headless_subprocess(
        "test_m5_f21_explore_search_mod_console_automod_dryrun_and_bot_studio",
    );
}

async fn body_f21_explore_mod_bot_studio(mut app: AsyncApp) {
    let host = open_community_host(&mut app, 1440.0, 900.0, 7105, "m5_f21_human").await;
    let cw = host.cw.clone();

    // 1. ExploreSearchView: all 7 tabs + operator normalization + punctuation-only + Discovery
    let _ = cw.update(&mut app, |w, cx| {
        w.open_explore_with_query("is:bot rust".to_string(), cx);
    });
    assert!(pump_until(&mut app, Duration::from_secs(4), |cx| !cw.read(cx).state.search_loading).await);
    host.draw(&mut app);

    for (query, tab) in [
        ("rust", CommunitySearchTab::Top),
        ("c:rust", CommunitySearchTab::Latest),
        ("", CommunitySearchTab::People),
        ("", CommunitySearchTab::Bots),
        ("", CommunitySearchTab::Communities),
        ("", CommunitySearchTab::Media),
        ("has:poll", CommunitySearchTab::Polls),
        ("(((", CommunitySearchTab::Top),
        ("", CommunitySearchTab::Top),
    ] {
        let _ = cw.update(&mut app, |w, cx| {
            w.state.explore_search_draft = query.to_string();
            w.execute_search(query.to_string(), tab, cx);
        });
        assert!(
            pump_until(&mut app, Duration::from_secs(4), |cx| !cw.read(cx).state.search_loading)
                .await
        );
        host.draw(&mut app);
    }

    // Searching `c/rust` on Communities tab must filter strictly to c/rust!
    let _ = cw.update(&mut app, |w, cx| {
        w.state.explore_search_draft = "c/rust".to_string();
        w.execute_search("c/rust".to_string(), CommunitySearchTab::Communities, cx);
    });
    assert!(
        pump_until(&mut app, Duration::from_secs(4), |cx| {
            !cw.read(cx).state.search_loading
        })
        .await
    );
    let _ = cw.read_with(&app, |w, _| {
        let comms = &w.state.search_result.as_ref().expect("search result").communities;
        assert_eq!(
            comms.len(),
            1,
            "c/rust on Communities tab must return only c/rust, got: {:?}",
            comms.iter().map(|c| &c.slug).collect::<Vec<_>>()
        );
        assert_eq!(comms[0].slug, "rust");
    });
    host.draw(&mut app);

    // 2. ModQueueView: Unauthorized guard on 0-moderated fresh user -> create owned c/mod_lab
    let _ = cw.update(&mut app, |w, cx| {
        w.navigate_to(CommunityRoute::ModQueue(None), cx);
        assert!(
            w.moderated_communities().is_empty(),
            "fresh user moderates 0 communities"
        );
        let _ = w.take_pending_actions();
        w.execute_mod_console_action(
            "rust".to_string(),
            ModActionKind::Remove,
            "post".to_string(),
            "p_unauth".to_string(),
            None,
            cx,
        );
        let acts = w.take_pending_actions();
        assert!(
            acts.iter()
                .any(|a| matches!(a, CommunityWorkspaceAction::ShowToast { is_error: true, .. })),
            "Unauthorized mod action must be blocked with error toast"
        );
    });
    host.draw(&mut app);

    // Create owned community c/mod_lab so user is Owner
    let _ = cw.update(&mut app, |w, cx| {
        w.open_create_community_modal(cx);
        w.state.create_community.slug = "mod_lab".to_string();
        w.state.create_community.name = "Moderation Lab".to_string();
        w.state.create_community.description = "Testing Mod Queue & AutoMod".to_string();
        w.submit_create_community(cx);
    });
    assert!(
        pump_until(&mut app, Duration::from_secs(4), |cx| {
            cw.read(cx).state.route == CommunityRoute::CommunityPage("mod_lab".to_string())
        })
        .await
    );

    // Publish a post in c/mod_lab and report it so it enters the Mod Queue
    let post = host
        .client
        .community()
        .block_on(host.client.community().create_post(&CreatePostRequest {
            community_slug: Some("mod_lab".to_string()),
            format: PostFormat::Micro,
            body: "Check out this crypto_scam test post".to_string(),
            status: PostStatus::Published,
            ..Default::default()
        }))
        .expect("create mod_lab post")
        .post;
    let _ = cw.update(&mut app, |w, cx| {
        w.record_signal(
            post.post_id.clone(),
            EngagementSignalKind::Report,
            Some(post.author_actor_id.clone()),
            Some("mod_lab".to_string()),
            cx,
        );
    });
    app.background_executor().timer(Duration::from_millis(80)).await;

    let _ = cw.update(&mut app, |w, cx| {
        w.navigate_to(CommunityRoute::ModQueue(Some("mod_lab".to_string())), cx);
    });
    assert!(
        pump_until(&mut app, Duration::from_secs(4), |cx| {
            !cw.read(cx).state.mod_loading && cw.read(cx).can_moderate_active_mod_community()
        })
        .await
    );
    // Cross-slug moderation guard: moderating mod_lab MUST NOT authorize actions against c/rust!
    let _ = cw.update(&mut app, |w, cx| {
        assert!(!w.can_moderate_community_slug("rust"));
        let _ = w.take_pending_actions();
        w.execute_mod_console_action(
            "rust".to_string(),
            ModActionKind::Remove,
            "post".to_string(),
            "p_cross_slug".to_string(),
            None,
            cx,
        );
        let acts = w.take_pending_actions();
        assert!(
            acts.iter()
                .any(|a| matches!(a, CommunityWorkspaceAction::ShowToast { is_error: true, .. })),
            "Cross-slug execute_mod_console_action must be rejected synchronously"
        );
        w.execute_mod_decision(
            "rust".to_string(),
            "q_cross_slug".to_string(),
            "post".to_string(),
            "p_cross_slug".to_string(),
            ModActionKind::Remove,
            cx,
        );
        let acts2 = w.take_pending_actions();
        assert!(
            acts2
                .iter()
                .any(|a| matches!(a, CommunityWorkspaceAction::ShowToast { is_error: true, .. })),
            "Cross-slug execute_mod_decision must be rejected synchronously"
        );
    });
    host.draw(&mut app);

    // AutoMod Rules: reject invalid regex -> add valid rule -> run pure-Rust Dry-Run!
    let _ = cw.update(&mut app, |w, cx| {
        w.set_mod_tab(ModConsoleTab::AutoModRules, cx);
        w.state.automod_draft.name = "Bad Regex Rule".to_string();
        w.state.automod_draft.regex_pattern = "(unclosed".to_string();
        w.submit_add_automod_rule(cx);
        assert!(
            w.state.automod_draft.error.is_some(),
            "Invalid regex must be rejected inline"
        );

        w.state.automod_draft.name = "Scam Keyword Guard".to_string();
        w.state.automod_draft.keywords = "crypto_scam, spamlink".to_string();
        w.state.automod_draft.regex_pattern = String::new();
        w.state.automod_draft.action = AutoModActionKind::FlagForReview;
        w.submit_add_automod_rule(cx);
    });
    assert!(
        pump_until(&mut app, Duration::from_secs(4), |cx| {
            !cw.read(cx).state.automod_draft.saving
                && cw
                    .read(cx)
                    .state
                    .mod_community
                    .as_ref()
                    .is_some_and(|c| !c.automod_rules.is_empty())
        })
        .await,
        "AutoMod rule must persist via update_community"
    );
    let _ = cw.update(&mut app, |w, cx| {
        w.state.automod_dry_run_title = "Free Giveaway".to_string();
        w.state.automod_dry_run_body = "This mentions crypto_scam inside body".to_string();
        w.run_automod_dry_run(cx);
        let dec = w
            .state
            .automod_dry_run_result
            .as_ref()
            .expect("dry run result");
        assert_eq!(dec.action, AutoModActionKind::FlagForReview);
        assert!(dec
            .matched_rule_names
            .iter()
            .any(|n| n == "Scam Keyword Guard"));
    });
    host.draw(&mut app);

    // Execute Approve + Lock in Mod Queue and verify transparent Mod Log entry
    let q_item_id = cw
        .read_with(&app, |w, _| w.state.mod_queue.first().map(|i| i.item_id.clone()))
        .unwrap();
    let _ = cw.update(&mut app, |w, cx| {
        w.set_mod_tab(ModConsoleTab::Queue, cx);
        w.state.mod_reason_draft = "Verified safe test post".to_string();
        w.execute_mod_console_action(
            "mod_lab".to_string(),
            ModActionKind::Approve,
            "post".to_string(),
            post.post_id.clone(),
            q_item_id,
            cx,
        );
    });
    assert!(
        pump_until(&mut app, Duration::from_secs(4), |cx| {
            !cw.read(cx).state.mod_logs.is_empty()
        })
        .await,
        "Moderation action must record transparent ModLogEntryDto"
    );
    let _ = cw.update(&mut app, |w, cx| w.set_mod_tab(ModConsoleTab::ModLog, cx));
    host.draw(&mut app);

    // 3. BotStudioView: invalid handle rejected -> register Bot w/ clamped Grok avatar -> Edit -> Copy Snippet -> Live Playground
    let _ = cw.update(&mut app, |w, cx| {
        w.navigate_to(CommunityRoute::BotStudio, cx);
        w.state.bot_studio_name = "Bad Bot".to_string();
        w.state.bot_studio_handle = "bad".to_string();
        w.submit_register_bot(cx);
        assert!(w.state.bot_studio_error.is_some());

        w.set_bot_studio_avatar_indices(99, 99, cx);
        assert_eq!(w.state.bot_studio_shape_idx, 7);
        assert_eq!(w.state.bot_studio_color_idx, 10);

        w.state.bot_studio_name = "M5 Studio Architect".to_string();
        w.state.bot_studio_handle = "m5_studio_bot".to_string();
        w.state.bot_studio_persona = "Analyzes GPUI rendering pipelines.".to_string();
        w.state.bot_studio_autonomy = BotAutonomyMode::Reactive;
        w.submit_register_bot(cx);
    });
    assert!(
        pump_until(&mut app, Duration::from_secs(4), |cx| {
            cw.read(cx)
                .state
                .identities
                .as_ref()
                .is_some_and(|i| i.bot_actors.iter().any(|b| b.handle == "m5_studio_bot"))
        })
        .await,
        "BotStudio must register m5_studio_bot"
    );
    let bot_id = cw
        .read_with(&app, |w, _| {
            w.state
                .identities
                .as_ref()
                .unwrap()
                .bot_actors
                .iter()
                .find(|b| b.handle == "m5_studio_bot")
                .unwrap()
                .actor_id
                .clone()
        })
        .unwrap();

    // Edit Bot + Copy API Snippet + Publish Live Test Post + Trigger Bot Reply
    let _ = cw.update(&mut app, |w, cx| {
        w.state.bot_studio_edit.display_name = "M5 Studio Architect v2".to_string();
        w.set_bot_studio_edit_avatar_indices(2, 5, cx);
        w.save_selected_bot_edits(cx);
        w.copy_bot_api_snippet(
            format_bot_api_snippet(&w.media_base_url, None, Some("mod_lab")),
            cx,
        );
    });
    assert!(
        pump_until(&mut app, Duration::from_secs(4), |cx| {
            !cw.read(cx).state.bot_studio_edit.saving
                && cw
                    .read(cx)
                    .state
                    .identities
                    .as_ref()
                    .is_some_and(|i| {
                        i.bot_actors
                            .iter()
                            .any(|b| b.display_name == "M5 Studio Architect v2")
                    })
        })
        .await
    );

    let _ = cw.update(&mut app, |w, cx| {
        w.state.bot_studio_test_community = Some("mod_lab".to_string());
        w.state.bot_studio_test_content = "Live Bot Studio test post from @m5_studio_bot!".to_string();
        w.run_bot_studio_test_post(bot_id.clone(), cx);
    });
    assert!(
        pump_until(&mut app, Duration::from_secs(4), |cx| {
            !cw.read(cx).state.bot_studio_test_submitting
                && cw.read(cx).state.bot_studio_test_last_post_id.is_some()
        })
        .await,
        "Live Test Post playground must publish a real post as @m5_studio_bot"
    );
    let _ = cw.update(&mut app, |w, cx| {
        w.run_bot_studio_trigger_reply(bot_id.clone(), cx);
    });
    assert!(
        pump_until(&mut app, Duration::from_secs(4), |cx| {
            !cw.read(cx).state.bot_studio_test_submitting
        })
        .await
    );

    // 4. Bot Studio unhydrated edit preservation + auto-hydration on identities arrival
    let owned_bot_dto = cw
        .read_with(&app, |w, _| {
            w.state
                .identities
                .as_ref()
                .unwrap()
                .bot_actors
                .iter()
                .find(|b| b.actor_id == bot_id)
                .cloned()
                .unwrap()
        })
        .unwrap();
    let ids_snap = cw
        .read_with(&app, |w, _| w.state.identities.clone().unwrap())
        .unwrap();
    let _ = cw.update(&mut app, |w, cx| {
        // 4a. Unhydrated direct edits (including Grok avatar shape/color & autonomy) preserved on hydrate
        w.state.bot_studio_selected_bot_id = None;
        w.state.bot_studio_edit = BotStudioEditDraft::default();
        w.state.bot_studio_edit.display_name = "Preserved Unhydrated Draft".to_string();
        w.state.bot_studio_edit.shape_idx = 6;
        w.state.bot_studio_edit.color_idx = 9;
        w.state.bot_studio_edit.bot_autonomy_mode = BotAutonomyMode::Autonomous;
        w.ensure_bot_studio_edit_hydrated(&owned_bot_dto);
        assert_eq!(w.state.bot_studio_edit.actor_id, bot_id);
        assert_eq!(w.state.bot_studio_edit.display_name, "Preserved Unhydrated Draft");
        assert_eq!(w.state.bot_studio_edit.shape_idx, 6);
        assert_eq!(w.state.bot_studio_edit.color_idx, 9);
        assert_eq!(w.state.bot_studio_edit.bot_autonomy_mode, BotAutonomyMode::Autonomous);

        // 4b. Pre-load edits made on BotStudio BEFORE identities arrive (including explicit 0/Manual via setters)
        // MUST be preserved by `apply_identities_response`!
        w.state.identities = None;
        w.state.bot_studio_selected_bot_id = None;
        w.state.bot_studio_edit = BotStudioEditDraft::default();
        w.state.bot_studio_edit.display_name = "User Edited Pre-Load".to_string();
        w.state.bot_studio_edit.bio = "Custom bio before load".to_string();
        w.set_bot_studio_edit_avatar_indices(5, 7, cx);
        w.set_bot_studio_edit_autonomy(BotAutonomyMode::Autonomous, cx);
        w.apply_identities_response(ids_snap.clone(), cx);
        assert_eq!(
            w.state.bot_studio_selected_bot_id.as_deref(),
            Some(bot_id.as_str()),
            "apply_identities_response on BotStudio must auto-hydrate selected bot"
        );
        assert_eq!(w.state.bot_studio_edit.actor_id, bot_id);
        assert_eq!(w.state.bot_studio_edit.display_name, "User Edited Pre-Load");
        assert_eq!(w.state.bot_studio_edit.bio, "Custom bio before load");
        assert_eq!(w.state.bot_studio_edit.shape_idx, 5);
        assert_eq!(w.state.bot_studio_edit.color_idx, 7);
        assert_eq!(w.state.bot_studio_edit.bot_autonomy_mode, BotAutonomyMode::Autonomous);

        // 4c. Explicit pre-load setter edit to (0, 0) + Manual must also survive hydration via dirty flags
        w.state.identities = None;
        w.state.bot_studio_selected_bot_id = None;
        w.state.bot_studio_edit = BotStudioEditDraft::default();
        w.set_bot_studio_edit_avatar_indices(0, 0, cx);
        w.set_bot_studio_edit_autonomy(BotAutonomyMode::Manual, cx);
        w.apply_identities_response(ids_snap, cx);
        assert_eq!(w.state.bot_studio_edit.shape_idx, 0);
        assert_eq!(w.state.bot_studio_edit.color_idx, 0);
        assert_eq!(w.state.bot_studio_edit.bot_autonomy_mode, BotAutonomyMode::Manual);
    });

    // 5. Mod Console actor-switch role revocation (on-route AND off-route!) + mod_gen fence
    let owner_id = cw
        .read_with(&app, |w, _| w.state.active_actor_id.clone())
        .unwrap();
    let _ = cw.update(&mut app, |w, cx| {
        w.navigate_to(CommunityRoute::ModQueue(Some("mod_lab".to_string())), cx);
    });
    assert!(
        pump_until(&mut app, Duration::from_secs(4), |cx| {
            !cw.read(cx).state.mod_loading && cw.read(cx).can_moderate_active_mod_community()
        })
        .await
    );
    // 5a. Off-route switch: Owner leaves ModQueue to Feed, then switches to non-mod Bot ->
    // MUST unconditionally clear mod_community/mod_queue/mod_logs/mod_slug and bump mod_gen!
    let _ = cw.update(&mut app, |w, cx| {
        w.navigate_to(CommunityRoute::Feed, cx);
        assert!(w.state.mod_community.is_some());
        let prev_gen = w.state.mod_gen;
        w.switch_active_actor(bot_id.clone(), cx);
        assert_ne!(w.state.mod_gen, prev_gen, "switch_active_actor must bump mod_gen off-route");
        assert!(w.state.mod_community.is_none());
        assert!(w.state.mod_slug.is_none());
        assert!(!w.can_moderate_community_slug("mod_lab"));
        assert!(w.moderated_communities().is_empty());
    });
    app.background_executor().timer(Duration::from_millis(120)).await;
    let _ = cw.update(&mut app, |w, cx| {
        assert!(
            !w.can_moderate_community_slug("mod_lab"),
            "Non-mod Bot must NOT retain stale mod_community permissions after communities refresh"
        );
        assert!(w.moderated_communities().is_empty());
        w.navigate_to(CommunityRoute::ModQueue(None), cx);
        assert!(
            !w.can_moderate_active_mod_community(),
            "Navigating to ModQueue(None) as non-mod Bot after off-route switch must stay locked"
        );
    });
    host.draw(&mut app);

    // 5b. Switch back to Owner on ModQueue(Some("mod_lab")), then on-route switch to non-mod Bot
    let _ = cw.update(&mut app, |w, cx| {
        w.switch_active_actor(owner_id.clone(), cx);
        w.navigate_to(CommunityRoute::ModQueue(Some("mod_lab".to_string())), cx);
    });
    assert!(
        pump_until(&mut app, Duration::from_secs(4), |cx| {
            !cw.read(cx).state.mod_loading && cw.read(cx).can_moderate_active_mod_community()
        })
        .await
    );
    let _ = cw.update(&mut app, |w, cx| {
        w.switch_active_actor(bot_id.clone(), cx);
        assert!(
            !w.can_moderate_active_mod_community(),
            "Switching from Owner to non-mod Bot on ModQueue must IMMEDIATELY revoke UI mod access"
        );
    });
    assert!(
        pump_until(&mut app, Duration::from_secs(4), |cx| {
            !cw.read(cx).state.mod_loading && !cw.read(cx).can_moderate_active_mod_community()
        })
        .await,
        "Non-mod Bot must remain unauthorized after ModQueue refresh completes"
    );
    host.draw(&mut app);

    // Switching back to Owner must refresh communities & ModQueue cleanly!
    let _ = cw.update(&mut app, |w, cx| {
        w.switch_active_actor(owner_id.clone(), cx);
    });
    assert!(
        pump_until(&mut app, Duration::from_secs(4), |cx| {
            !cw.read(cx).state.mod_loading && cw.read(cx).can_moderate_active_mod_community()
        })
        .await,
        "Switching back to Owner on ModQueue must restore moderator access cleanly"
    );
    host.draw(&mut app);
}

// ============================================================================
// Multi-Theme (Night/Tinted/Day/Classic), Bilingual (zh-CN/en) & 3/2/1-Col
// Responsive Render Smoke Across ALL F17–F21 Routes & Modals
// ============================================================================

#[test]
fn test_m5_multi_theme_bilingual_and_responsive_layout_smoke_all_routes() {
    run_in_headless_subprocess(
        "test_m5_multi_theme_bilingual_and_responsive_layout_smoke_all_routes",
    );
}

async fn body_multi_theme_bilingual_responsive_smoke(mut app: AsyncApp) {
    assert_eq!(IconName::ALL.len(), 64, "IconName::ALL MUST remain 64");
    let host = open_community_host(&mut app, 1440.0, 900.0, 7106, "m5_smoke_human").await;
    let cw = host.cw.clone();

    let sample_post_id = cw
        .read_with(&app, |w, _| w.state.feed_items[0].post.post_id.clone())
        .unwrap();
    let sample_comm = cw
        .read_with(&app, |w, _| w.state.communities[0].slug.clone())
        .unwrap();
    let sample_handle = cw
        .read_with(&app, |w, _| {
            w.state
                .recommended_actors
                .first()
                .map(|a| a.handle.clone())
                .unwrap_or_else(|| "m5_smoke_human".to_string())
        })
        .unwrap();

    // Ensure host.key works cleanly in headless mode too
    host.key(&mut app, "cmd-l");

    let routes = vec![
        CommunityRoute::Feed,
        CommunityRoute::PostDetail(sample_post_id),
        CommunityRoute::CommunityPage(sample_comm.clone()),
        CommunityRoute::ActorProfile(sample_handle),
        CommunityRoute::Explore {
            initial_query: Some("rust".to_string()),
        },
        CommunityRoute::Notifications,
        CommunityRoute::BookmarksLists,
        CommunityRoute::ModQueue(Some(sample_comm)),
        CommunityRoute::BotStudio,
    ];

    let themes = [
        TdesktopThemeMode::Night,
        TdesktopThemeMode::Tinted,
        TdesktopThemeMode::Day,
        TdesktopThemeMode::Classic,
    ];
    let locales = ["zh-CN", "en"];
    let widths = [
        (1440.0_f32, CommunityLayoutMode::ThreeColumn),
        (1040.0_f32, CommunityLayoutMode::TwoColumn),
        (760.0_f32, CommunityLayoutMode::OneColumn),
    ];

    for (idx, route) in routes.into_iter().enumerate() {
        let theme = themes[idx % themes.len()];
        let lang = locales[idx % locales.len()];
        let _ = host.shell.update(&mut app, |s, cx| {
            s.apply_theme_selection(theme, None, None, cx);
        });
        let _ = cw.update(&mut app, |w, cx| {
            w.set_language(lang);
            w.replace_route(route.clone(), cx);
        });
        app.background_executor().timer(Duration::from_millis(30)).await;
        for (w_px, expected_mode) in widths {
            host.draw_at(&mut app, w_px, 800.0);
            let actual_mode = cw.read_with(&app, |w, _| w.last_layout_mode).unwrap();
            assert_eq!(
                actual_mode, expected_mode,
                "route {route:?} at width {w_px}px must resolve to {expected_mode:?}"
            );
        }
    }

    // Also render ComposerModal across all 4 themes & both locales
    for (i, theme) in themes.into_iter().enumerate() {
        let lang = locales[i % locales.len()];
        let _ = host.shell.update(&mut app, |s, cx| {
            s.apply_theme_selection(theme, None, None, cx);
        });
        let _ = cw.update(&mut app, |w, cx| {
            w.set_language(lang);
            w.open_composer_modal(None, cx);
        });
        host.draw_at(&mut app, 1440.0, 900.0);
    }
}

// ============================================================================
// Wave r4 Regression 1: Online Delayed-RTT Actor-Switch Fences
// (`F21-MOD-4` load_community_page + `persist_automod_rules` + `save_community_rules`)
// ============================================================================

async fn fetch_db_lists(
    facade: &gpui_telegram_client::CommunityClientFacade,
) -> Vec<CommunityListDto> {
    let f = facade.clone();
    let handle = f.runtime_handle().clone();
    let (tx, rx) = futures::channel::oneshot::channel();
    handle.spawn(async move {
        let _ = tx.send(f.list_lists().await);
    });
    rx.await
        .expect("tokio oneshot")
        .expect("facade.list_lists()")
        .lists
}

fn mock_actor_dto(actor_id: &str, kind: ActorKind, handle: &str, name: &str) -> CommunityActorDto {
    CommunityActorDto {
        actor_id: actor_id.to_string(),
        linked_user_id: if kind == ActorKind::Human { Some(902301) } else { Some(950000) },
        owner_user_id: Some(902301),
        actor_kind: kind,
        handle: handle.to_string(),
        display_name: name.to_string(),
        bio: String::new(),
        location: None,
        website: None,
        avatar_media_id: None,
        banner_media_id: None,
        avatar_shape: "classic".to_string(),
        avatar_color: "blue".to_string(),
        verified_tier: VerifiedTier::None,
        bot_badge: if kind.is_bot_like() { Some("AI Agent".to_string()) } else { None },
        bot_model: None,
        bot_capabilities: Vec::new(),
        bot_autonomy_mode: BotAutonomyMode::Manual,
        bot_persona: None,
        post_karma: 100,
        comment_karma: 50,
        awardee_karma: 0,
        followers_count: 1,
        following_count: 1,
        posts_count: 0,
        trust_score: 0.9,
        is_following: false,
        is_muted: false,
        is_blocked: false,
        created_at_ms: 1_750_000_000_000,
    }
}

fn mock_community_dto(
    slug: &str,
    owner_id: &str,
    viewer_role: Option<CommunityMemberRole>,
) -> SubCommunityDto {
    SubCommunityDto {
        community_id: format!("comm_{slug}"),
        slug: slug.to_string(),
        name: "Probe Mod Lab".to_string(),
        description: "Delayed-RTT moderation fence test community".to_string(),
        category: "technology".to_string(),
        banner_media_id: None,
        icon_media_id: None,
        icon_emoji: "🛡️".to_string(),
        accent_color: "#3B82F6".to_string(),
        rules: Vec::new(),
        post_flairs: Vec::new(),
        user_flairs: Vec::new(),
        wiki_markdown: String::new(),
        wiki_revision: 1,
        pinned_post_ids: Vec::new(),
        automod_rules: Vec::new(),
        allow_bots: true,
        require_bot_disclosure: false,
        is_nsfw: false,
        member_count: 2,
        human_member_count: 1,
        bot_member_count: 1,
        posts_count: 0,
        viewer_role,
        is_joined: viewer_role.is_some(),
        notifications_enabled: false,
        created_by_actor_id: owner_id.to_string(),
        created_at_ms: 1_750_000_000_000,
    }
}

struct DelayedModRaceServerCounters {
    get_detail_completed: Arc<std::sync::atomic::AtomicUsize>,
    patch_completed: Arc<std::sync::atomic::AtomicUsize>,
}

fn spawn_delayed_mod_race_server(
    rt_handle: &tokio::runtime::Handle,
    owner_id: String,
    non_mod_bot_id: String,
    slug: String,
) -> (std::net::SocketAddr, DelayedModRaceServerCounters) {
    use std::sync::atomic::{AtomicUsize, Ordering};
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    let get_detail_completed = Arc::new(AtomicUsize::new(0));
    let patch_completed = Arc::new(AtomicUsize::new(0));
    let get_cnt = Arc::clone(&get_detail_completed);
    let patch_cnt = Arc::clone(&patch_completed);

    let std_listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind loopback mock");
    std_listener
        .set_nonblocking(true)
        .expect("nonblocking loopback mock");
    let addr = std_listener.local_addr().expect("mock local_addr");

    rt_handle.spawn(async move {
        let listener = tokio::net::TcpListener::from_std(std_listener)
            .expect("tokio TcpListener from_std");
        let owner_actor = mock_actor_dto(&owner_id, ActorKind::Human, "race_owner", "Race Owner");
        let bot_actor = mock_actor_dto(&non_mod_bot_id, ActorKind::Bot, "race_bot", "Race Bot");

        while let Ok((mut sock, _)) = listener.accept().await {
            let owner_id = owner_id.clone();
            let slug = slug.clone();
            let owner_actor = owner_actor.clone();
            let bot_actor = bot_actor.clone();
            let get_cnt = Arc::clone(&get_cnt);
            let patch_cnt = Arc::clone(&patch_cnt);

            tokio::spawn(async move {
                let mut buf = Vec::with_capacity(4096);
                let mut tmp = [0u8; 2048];
                let header_end = loop {
                    match sock.read(&mut tmp).await {
                        Ok(0) => return,
                        Ok(n) => {
                            buf.extend_from_slice(&tmp[..n]);
                            if let Some(pos) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
                                break pos + 4;
                            }
                            if buf.len() > 65536 {
                                return;
                            }
                        }
                        Err(_) => return,
                    }
                };
                let head_str = String::from_utf8_lossy(&buf[..header_end]).to_string();
                let mut lines = head_str.lines();
                let req_line = lines.next().unwrap_or("");
                let mut parts = req_line.split_whitespace();
                let method = parts.next().unwrap_or("GET").to_string();
                let full_target = parts.next().unwrap_or("/");
                let path = full_target.split('?').next().unwrap_or("/").to_string();

                let mut actor_header = String::new();
                let mut content_len: usize = 0;
                for line in lines {
                    if let Some((k, v)) = line.split_once(':') {
                        if k.trim().eq_ignore_ascii_case("x-community-actor-id") {
                            actor_header = v.trim().to_string();
                        } else if k.trim().eq_ignore_ascii_case("content-length") {
                            content_len = v.trim().parse().unwrap_or(0);
                        }
                    }
                }
                while buf.len() < header_end + content_len {
                    match sock.read(&mut tmp).await {
                        Ok(0) => break,
                        Ok(n) => buf.extend_from_slice(&tmp[..n]),
                        Err(_) => break,
                    }
                }

                let is_owner = actor_header.is_empty() || actor_header == owner_id;
                let viewer_role = if is_owner {
                    Some(CommunityMemberRole::Owner)
                } else {
                    None
                };
                let detail_path = format!("/api/v1/community/communities/{slug}");
                let queue_path = format!("/api/v1/community/communities/{slug}/mod-queue");
                let logs_path = format!("/api/v1/community/communities/{slug}/mod-logs");

                let (status, body) = if method == "GET" && path == "/api/v1/community/identities" {
                    let resp = CommunityIdentitiesResponse {
                        active_actor_id: owner_id.clone(),
                        human_actor: Some(owner_actor.clone()),
                        bot_actors: vec![bot_actor.clone()],
                        all_actors: vec![owner_actor.clone(), bot_actor.clone()],
                    };
                    ("200 OK", serde_json::to_string(&resp).unwrap())
                } else if method == "POST" && path == "/api/v1/community/identities/human" {
                    ("200 OK", serde_json::to_string(&owner_actor).unwrap())
                } else if method == "GET" && path == "/api/v1/community/communities" {
                    let resp = ListCommunitiesResponse {
                        communities: vec![mock_community_dto(&slug, &owner_id, viewer_role)],
                    };
                    ("200 OK", serde_json::to_string(&resp).unwrap())
                } else if method == "GET" && path == detail_path {
                    if is_owner {
                        tokio::time::sleep(Duration::from_millis(220)).await;
                        get_cnt.fetch_add(1, Ordering::SeqCst);
                    }
                    let resp = CommunityDetailResponse {
                        community: mock_community_dto(&slug, &owner_id, viewer_role),
                        pinned_posts: Vec::new(),
                        moderators: vec![owner_actor.clone()],
                        wiki_revisions: Vec::new(),
                    };
                    ("200 OK", serde_json::to_string(&resp).unwrap())
                } else if method == "PATCH" && path == detail_path {
                    if is_owner {
                        tokio::time::sleep(Duration::from_millis(220)).await;
                        patch_cnt.fetch_add(1, Ordering::SeqCst);
                    }
                    let resp = mock_community_dto(&slug, &owner_id, viewer_role);
                    ("200 OK", serde_json::to_string(&resp).unwrap())
                } else if method == "GET" && path == queue_path {
                    if is_owner {
                        ("200 OK", serde_json::to_string(&ModQueueResponse::default()).unwrap())
                    } else {
                        (
                            "403 Forbidden",
                            r#"{"error":"permission_denied","message":"actor is not a community moderator"}"#
                                .to_string(),
                        )
                    }
                } else if method == "GET" && path == logs_path {
                    if is_owner {
                        ("200 OK", serde_json::to_string(&ModLogsResponse::default()).unwrap())
                    } else {
                        (
                            "403 Forbidden",
                            r#"{"error":"permission_denied","message":"actor is not a community moderator"}"#
                                .to_string(),
                        )
                    }
                } else if path == "/api/v1/community/feed" {
                    ("200 OK", serde_json::to_string(&FeedResponseDto::default()).unwrap())
                } else if path == "/api/v1/community/notifications" {
                    (
                        "200 OK",
                        serde_json::to_string(&NotificationsListResponse::default()).unwrap(),
                    )
                } else if path == "/api/v1/community/lists" {
                    (
                        "200 OK",
                        serde_json::to_string(&CommunityListsResponse::default()).unwrap(),
                    )
                } else if path == "/api/v1/community/trending" {
                    ("200 OK", serde_json::to_string(&TrendingResponse::default()).unwrap())
                } else if path == "/api/v1/community/algo-prefs" {
                    ("200 OK", serde_json::to_string(&AlgoPreferencesDto::default()).unwrap())
                } else if path == "/api/v1/community/search" {
                    let sr = CommunitySearchResultDto {
                        query: String::new(),
                        tab: CommunitySearchTab::Top,
                        posts: Vec::new(),
                        actors: Vec::new(),
                        communities: Vec::new(),
                        total_hits: 0,
                        next_offset: None,
                    };
                    ("200 OK", serde_json::to_string(&sr).unwrap())
                } else {
                    ("200 OK", "{}".to_string())
                };

                let response = format!(
                    "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                );
                let _ = sock.write_all(response.as_bytes()).await;
                let _ = sock.shutdown().await;
            });
        }
    });

    (
        addr,
        DelayedModRaceServerCounters {
            get_detail_completed,
            patch_completed,
        },
    )
}

#[test]
fn test_m5_f21_mod_race_actor_switch_fences_online_delayed_rtt() {
    run_in_headless_subprocess("test_m5_f21_mod_race_actor_switch_fences_online_delayed_rtt");
}

async fn body_f21_mod_race_actor_switch_fences_online_delayed_rtt(mut app: AsyncApp) {
    use std::sync::atomic::Ordering;

    let dir = TempDir::new().expect("create delayed mod race tempdir");
    let owner_id = "u:902301".to_string();
    let bot_id = "b:950000".to_string();
    let slug = "probe_mod_lab".to_string();

    // Open client on a throwaway runtime first? No — open with placeholder, then bind?
    // Even cleaner: spawn a small temp tokio runtime OR open TelegramClient after binding using
    // a dedicated TcpListener bound synchronously and attached onto client.runtime()!
    let dummy_cfg = ClientConfig {
        server_url: "http://127.0.0.1:9".to_string(),
        max_reconnect_attempts: 0,
        ..Default::default()
    };
    let bootstrap_dir = TempDir::new().expect("bootstrap tempdir");
    let rt_holder =
        TelegramClient::open_with_dir(dummy_cfg, bootstrap_dir.path()).expect("open rt holder");
    let (addr, counters) = spawn_delayed_mod_race_server(
        rt_holder.runtime().handle(),
        owner_id.clone(),
        bot_id.clone(),
        slug.clone(),
    );

    let cfg = ClientConfig {
        server_url: format!("http://{addr}"),
        max_reconnect_attempts: 0,
        ..Default::default()
    };
    let client = Arc::new(
        TelegramClient::open_with_dir(cfg, dir.path()).expect("open online race TelegramClient"),
    );
    // Keep force_local = false so CommunityClientFacade uses our delayed-RTT loopback server!
    let auth = test_auth(902301, "race_owner");
    client.set_auth_session(auth.clone());

    let client_for_win = Arc::clone(&client);
    let auth_for_win = auth.clone();
    let shell = app
        .update(|cx| {
            let _rt_guard = client_for_win.runtime().enter();
            cx.new(|cx| {
                let mut s = AppShell::new_with_client(client_for_win.clone(), cx);
                s.apply_authenticated_session(auth_for_win.clone(), cx);
                s
            })
        })
        .expect("mount online race AppShell");
    let _ = shell.update(&mut app, |s, cx| {
        s.open_community_workspace(cx);
    });
    let cw = shell
        .read_with(&app, |s, _| {
            s.community_workspace
                .clone()
                .expect("community workspace mounted")
        })
        .unwrap();

    assert!(
        pump_until(&mut app, Duration::from_secs(5), |cx| {
            let w = cw.read(cx);
            w.state.communities.len() == 1 && w.can_moderate_community_slug(&slug)
        })
        .await,
        "Online bootstrap must hydrate probe_mod_lab with Owner role"
    );

    // Sub-test A (F21-MOD-4): Off-route load_community_page delayed Owner GET must NEVER leak Owner role!
    let _ = cw.update(&mut app, |w, cx| {
        w.navigate_to(CommunityRoute::CommunityPage(slug.clone()), cx);
    });
    app.background_executor().timer(Duration::from_millis(40)).await;
    let _ = cw.update(&mut app, |w, cx| {
        let prev_comm_gen = w.state.community_gen;
        w.navigate_to(CommunityRoute::Feed, cx);
        w.switch_active_actor(bot_id.clone(), cx);
        assert_ne!(
            w.state.community_gen, prev_comm_gen,
            "switch_active_actor must unconditionally bump community_gen"
        );
        assert!(!w.state.community_loading);
        assert!(!w.can_moderate_community_slug(&slug));
    });
    app.background_executor().timer(Duration::from_millis(340)).await;
    assert!(
        counters.get_detail_completed.load(Ordering::SeqCst) >= 1,
        "delayed Owner GET /communities/probe_mod_lab must have completed on mock server"
    );
    let _ = cw.read_with(&app, |w, _| {
        assert!(!w.can_moderate_community_slug(&slug));
        assert!(w.moderated_communities().is_empty());
        assert!(w
            .state
            .community_detail
            .as_ref()
            .and_then(|d| d.community.viewer_role)
            .is_none());
        assert!(w
            .state
            .communities
            .iter()
            .find(|c| c.slug == slug)
            .and_then(|c| c.viewer_role)
            .is_none());
    });

    // Sub-test B: In-flight persist_automod_rules delayed Owner PATCH must NEVER leak Owner role or mod_community!
    let _ = cw.update(&mut app, |w, cx| {
        w.switch_active_actor(owner_id.clone(), cx);
        w.navigate_to(CommunityRoute::ModQueue(Some(slug.clone())), cx);
    });
    assert!(
        pump_until(&mut app, Duration::from_secs(4), |cx| {
            let w = cw.read(cx);
            !w.state.mod_loading && w.can_moderate_active_mod_community()
        })
        .await
    );
    let _ = cw.update(&mut app, |w, cx| {
        w.state.automod_draft.name = "Delayed Race Rule".to_string();
        w.state.automod_draft.keywords = "spam".to_string();
        w.submit_add_automod_rule(cx);
        assert!(w.state.automod_draft.saving);
    });
    app.background_executor().timer(Duration::from_millis(40)).await;
    let _ = cw.update(&mut app, |w, cx| {
        w.navigate_to(CommunityRoute::Feed, cx);
        w.switch_active_actor(bot_id.clone(), cx);
        assert!(!w.state.automod_draft.saving);
        assert!(w.state.mod_community.is_none());
        assert!(!w.can_moderate_community_slug(&slug));
    });
    app.background_executor().timer(Duration::from_millis(340)).await;
    assert!(
        counters.patch_completed.load(Ordering::SeqCst) >= 1,
        "delayed Owner PATCH (persist_automod_rules) must have completed on mock server"
    );
    let _ = cw.read_with(&app, |w, _| {
        assert!(w.state.mod_community.is_none());
        assert!(!w.can_moderate_community_slug(&slug));
        assert!(w.moderated_communities().is_empty());
        assert!(w
            .state
            .communities
            .iter()
            .find(|c| c.slug == slug)
            .and_then(|c| c.viewer_role)
            .is_none());
        assert!(w
            .state
            .community_detail
            .as_ref()
            .and_then(|d| d.community.viewer_role)
            .is_none());
    });

    // Sub-test C: In-flight save_community_rules delayed Owner PATCH must NEVER leak Owner role!
    let _ = cw.update(&mut app, |w, cx| {
        w.switch_active_actor(owner_id.clone(), cx);
        w.navigate_to(CommunityRoute::CommunityPage(slug.clone()), cx);
    });
    assert!(
        pump_until(&mut app, Duration::from_secs(4), |cx| {
            let w = cw.read(cx);
            !w.state.community_loading
                && w.state
                    .community_detail
                    .as_ref()
                    .is_some_and(|d| d.community.viewer_role == Some(CommunityMemberRole::Owner))
        })
        .await
    );
    let _ = cw.update(&mut app, |w, cx| {
        w.state.community_new_rule_title = "Delayed Rule Race".to_string();
        w.state.community_new_rule_desc = "Must not leak role on switch".to_string();
        w.add_community_rule(slug.clone(), cx);
        assert!(w.state.community_rules_saving);
    });
    app.background_executor().timer(Duration::from_millis(40)).await;
    let _ = cw.update(&mut app, |w, cx| {
        w.navigate_to(CommunityRoute::Feed, cx);
        w.switch_active_actor(bot_id.clone(), cx);
        assert!(!w.state.community_rules_saving);
        assert!(!w.can_moderate_community_slug(&slug));
    });
    app.background_executor().timer(Duration::from_millis(340)).await;
    assert!(
        counters.patch_completed.load(Ordering::SeqCst) >= 2,
        "delayed Owner PATCH (save_community_rules) must have completed on mock server"
    );
    let _ = cw.read_with(&app, |w, _| {
        assert!(!w.can_moderate_community_slug(&slug));
        assert!(w.moderated_communities().is_empty());
        assert!(w
            .state
            .community_detail
            .as_ref()
            .and_then(|d| d.community.viewer_role)
            .is_none());
        assert!(w
            .state
            .communities
            .iter()
            .find(|c| c.slug == slug)
            .and_then(|c| c.viewer_role)
            .is_none());
    });
    drop(rt_holder);
}

// ============================================================================
// Wave r4 Regression 2: F20-LISTS Single-Flight Coalescing SQLite Stress Suite
// ============================================================================

#[test]
fn test_m5_f20_lists_rapid_toggle_single_flight_sqlite_stress() {
    run_in_headless_subprocess("test_m5_f20_lists_rapid_toggle_single_flight_sqlite_stress");
}

async fn body_f20_lists_rapid_toggle_single_flight_sqlite_stress(mut app: AsyncApp) {
    let host = open_community_host(&mut app, 1440.0, 900.0, 7120, "m5_lists_stress").await;
    let cw = host.cw.clone();
    let facade = cw.read_with(&app, |w, _| w.facade.clone()).unwrap();

    // Navigate to BookmarksLists -> Lists and create two real lists L1 and L2
    let _ = cw.update(&mut app, |w, cx| {
        w.navigate_to(CommunityRoute::BookmarksLists, cx);
        w.set_bookmarks_section(BookmarksListsSection::Lists, cx);
        w.open_create_list_form(cx);
        w.state.list_editor.name = "Stress List L1".to_string();
        w.state.list_editor.is_pinned = false;
        w.submit_list_editor(cx);
    });
    assert!(
        pump_until(&mut app, Duration::from_secs(4), |cx| {
            !cw.read(cx).state.list_editor.submitting
                && cw.read(cx).state.lists.iter().any(|l| l.name == "Stress List L1")
        })
        .await
    );
    let _ = cw.update(&mut app, |w, cx| {
        w.open_create_list_form(cx);
        w.state.list_editor.name = "Stress List L2".to_string();
        w.state.list_editor.is_pinned = false;
        w.submit_list_editor(cx);
    });
    assert!(
        pump_until(&mut app, Duration::from_secs(4), |cx| {
            !cw.read(cx).state.list_editor.submitting
                && cw.read(cx).state.lists.iter().any(|l| l.name == "Stress List L2")
        })
        .await
    );
    let (l1_id, l2_id) = cw
        .read_with(&app, |w, _| {
            let l1 = w
                .state
                .lists
                .iter()
                .find(|l| l.name == "Stress List L1")
                .unwrap()
                .list_id
                .clone();
            let l2 = w
                .state
                .lists
                .iter()
                .find(|l| l.name == "Stress List L2")
                .unwrap()
                .list_id
                .clone();
            (l1, l2)
        })
        .unwrap();

    let a1 = "u:100".to_string();
    let a2 = "b:200".to_string();

    // Phase A: 10 rounds x 5 rapid synchronous toggles on L1 (4 membership + pin) — UI == SQLite every round!
    for round in 0..10usize {
        let want_pinned = round % 2 == 0;
        let expect_present = round % 2 == 0;
        let _ = cw.update(&mut app, |w, cx| {
            w.toggle_selected_list_actor(l1_id.clone(), a1.clone(), cx);
            w.toggle_selected_list_actor(l1_id.clone(), a2.clone(), cx);
            w.toggle_selected_list_community(l1_id.clone(), "rust".to_string(), cx);
            w.toggle_selected_list_community(l1_id.clone(), "ai_agents".to_string(), cx);
            w.toggle_list_pinned(l1_id.clone(), want_pinned, cx);
            assert!(
                w.state.list_membership_in_flight.contains_key(&l1_id),
                "round {round}: L1 must be in_flight immediately"
            );
            assert!(
                w.state.list_membership_dirty.contains(&l1_id),
                "round {round}: L1 must be marked dirty by coalesced burst"
            );
        });
        assert!(
            pump_until(&mut app, Duration::from_secs(4), |cx| {
                let st = &cw.read(cx).state;
                !st.list_membership_in_flight.contains_key(&l1_id)
                    && !st.list_membership_dirty.contains(&l1_id)
            })
            .await,
            "round {round}: single-flight flush must settle"
        );
        let ui_list = cw
            .read_with(&app, |w, _| {
                w.state
                    .lists
                    .iter()
                    .find(|x| x.list_id == l1_id)
                    .cloned()
                    .expect("L1 in UI")
            })
            .unwrap();
        let db_lists = fetch_db_lists(&facade).await;
        let db_list = db_lists
            .iter()
            .find(|x| x.list_id == l1_id)
            .expect("L1 in SQLite");
        let expected_actors = if expect_present {
            vec![a1.clone(), a2.clone()]
        } else {
            Vec::new()
        };
        let expected_comms = if expect_present {
            vec!["rust".to_string(), "ai_agents".to_string()]
        } else {
            Vec::new()
        };
        assert_eq!(ui_list.member_actor_ids, expected_actors, "round {round} UI actors");
        assert_eq!(db_list.member_actor_ids, expected_actors, "round {round} DB actors");
        assert_eq!(ui_list.community_slugs, expected_comms, "round {round} UI comms");
        assert_eq!(db_list.community_slugs, expected_comms, "round {round} DB comms");
        assert_eq!(ui_list.is_pinned, want_pinned, "round {round} UI pinned");
        assert_eq!(db_list.is_pinned, want_pinned, "round {round} DB pinned");
    }

    // Phase B: Interleaved L1 + L2 + mid-flight toggle on L1
    let _ = cw.update(&mut app, |w, cx| {
        w.toggle_selected_list_actor(l1_id.clone(), a1.clone(), cx);
        w.toggle_selected_list_actor(l2_id.clone(), a2.clone(), cx);
        w.toggle_selected_list_community(l1_id.clone(), "rust".to_string(), cx);
        w.toggle_selected_list_community(l2_id.clone(), "systems".to_string(), cx);
    });
    app.background_executor().timer(Duration::from_millis(2)).await;
    let _ = cw.update(&mut app, |w, cx| {
        w.toggle_selected_list_community(l1_id.clone(), "ai_agents".to_string(), cx);
    });
    assert!(
        pump_until(&mut app, Duration::from_secs(4), |cx| {
            let st = &cw.read(cx).state;
            st.list_membership_in_flight.is_empty() && st.list_membership_dirty.is_empty()
        })
        .await
    );
    let db_lists_b = fetch_db_lists(&facade).await;
    let db_l1_b = db_lists_b.iter().find(|x| x.list_id == l1_id).unwrap();
    let db_l2_b = db_lists_b.iter().find(|x| x.list_id == l2_id).unwrap();
    assert_eq!(db_l1_b.member_actor_ids, vec![a1.clone()]);
    assert_eq!(db_l1_b.community_slugs, vec!["rust".to_string(), "ai_agents".to_string()]);
    assert_eq!(db_l2_b.member_actor_ids, vec![a2.clone()]);
    assert_eq!(db_l2_b.community_slugs, vec!["systems".to_string()]);

    // Phase C: load_saved_posts racing 4 rapid toggles on L1 (merge_refreshed_lists must preserve optimistic edits)
    let _ = cw.update(&mut app, |w, cx| {
        w.load_saved_posts(cx);
        w.toggle_selected_list_actor(l1_id.clone(), a2.clone(), cx);
        w.toggle_selected_list_community(l1_id.clone(), "systems".to_string(), cx);
        w.toggle_list_pinned(l1_id.clone(), true, cx);
    });
    assert!(
        pump_until(&mut app, Duration::from_secs(4), |cx| {
            let st = &cw.read(cx).state;
            !st.saved_loading
                && st.list_membership_in_flight.is_empty()
                && st.list_membership_dirty.is_empty()
        })
        .await
    );
    let db_lists_c = fetch_db_lists(&facade).await;
    let db_l1_c = db_lists_c.iter().find(|x| x.list_id == l1_id).unwrap();
    let ui_l1_c = cw
        .read_with(&app, |w, _| {
            w.state.lists.iter().find(|x| x.list_id == l1_id).cloned().unwrap()
        })
        .unwrap();
    assert_eq!(ui_l1_c.member_actor_ids, vec![a1.clone(), a2.clone()]);
    assert_eq!(db_l1_c.member_actor_ids, vec![a1.clone(), a2.clone()]);
    assert_eq!(
        ui_l1_c.community_slugs,
        vec!["rust".to_string(), "ai_agents".to_string(), "systems".to_string()]
    );
    assert_eq!(db_l1_c.community_slugs, ui_l1_c.community_slugs);
    assert!(ui_l1_c.is_pinned && db_l1_c.is_pinned);

    // Phase D: delete_list_action while L2 flush is in_flight & dirty -> zero error toasts, deleted cleanly
    let _ = host.shell.update(&mut app, |s, _| {
        s.window_runtime.controller.current_toast = None;
        s.window_runtime.controller.toast_queue.clear();
    });
    let _ = cw.update(&mut app, |w, cx| {
        let _ = w.take_pending_actions();
        w.toggle_selected_list_actor(l2_id.clone(), a1.clone(), cx);
        w.toggle_selected_list_community(l2_id.clone(), "rust".to_string(), cx);
        w.toggle_list_pinned(l2_id.clone(), true, cx);
        w.delete_list_action(l2_id.clone(), cx);
    });
    assert!(
        pump_until(&mut app, Duration::from_secs(4), |cx| {
            let st = &cw.read(cx).state;
            !st.lists.iter().any(|l| l.list_id == l2_id)
                && !st.list_deleting.contains(&l2_id)
                && !st.list_membership_in_flight.contains_key(&l2_id)
                && !st.list_membership_dirty.contains(&l2_id)
        })
        .await
    );
    app.background_executor().timer(Duration::from_millis(80)).await;
    let db_lists_d = fetch_db_lists(&facade).await;
    assert!(!db_lists_d.iter().any(|x| x.list_id == l2_id));
    let shell_err_toasts_d: Vec<String> = host
        .shell
        .read_with(&app, |s, _| {
            let ctrl = &s.window_runtime.controller;
            ctrl.current_toast
                .iter()
                .chain(ctrl.toast_queue.iter())
                .filter(|t| t.duration_ms == 3500)
                .map(|t| t.text.clone())
                .collect()
        })
        .unwrap();
    assert!(
        shell_err_toasts_d.is_empty(),
        "delete_list_action racing in-flight toggles must emit zero AppShell error toasts, got: {shell_err_toasts_d:?}"
    );
    let _ = cw.update(&mut app, |w, _| {
        let errs: Vec<_> = w
            .take_pending_actions()
            .into_iter()
            .filter(|a| matches!(a, CommunityWorkspaceAction::ShowToast { is_error: true, .. }))
            .collect();
        assert!(
            errs.is_empty(),
            "delete_list_action racing in-flight toggles must emit zero error toasts, got: {errs:?}"
        );
    });

    // Phase E: Error recovery (zero deadlock + rollback on nonexistent synthetic list)
    let fake_id = "list_missing_err_probe".to_string();
    let _ = host.shell.update(&mut app, |s, _| {
        s.window_runtime.controller.current_toast = None;
        s.window_runtime.controller.toast_queue.clear();
    });
    let _ = cw.update(&mut app, |w, cx| {
        let _ = w.take_pending_actions();
        w.state.lists.push(CommunityListDto {
            list_id: fake_id.clone(),
            owner_actor_id: w.state.active_actor_id.clone(),
            name: "Missing Probe".to_string(),
            description: String::new(),
            kind: CommunityListKind::Mixed,
            is_private: false,
            is_pinned: false,
            member_actor_ids: Vec::new(),
            community_slugs: Vec::new(),
            created_at_ms: 0,
            updated_at_ms: 0,
        });
        w.toggle_selected_list_actor(fake_id.clone(), a1.clone(), cx);
        w.toggle_selected_list_actor(fake_id.clone(), a2.clone(), cx);
    });
    assert!(
        pump_until(&mut app, Duration::from_secs(4), |cx| {
            !cw.read(cx).state.list_membership_in_flight.contains_key(&fake_id)
        })
        .await
    );
    assert!(
        pump_until(&mut app, Duration::from_secs(2), |cx| {
            let ctrl = &host.shell.read(cx).window_runtime.controller;
            ctrl.current_toast
                .iter()
                .chain(ctrl.toast_queue.iter())
                .any(|t| t.duration_ms == 3500)
        })
        .await,
        "terminal update_list error must surface an error toast into AppShell"
    );
    let _ = cw.update(&mut app, |w, cx| {
        assert!(!w.state.list_membership_dirty.contains(&fake_id));
        let fake_ui = w.state.lists.iter().find(|l| l.list_id == fake_id).unwrap();
        assert!(
            fake_ui.member_actor_ids.is_empty(),
            "terminal error must roll back synthetic list from list_membership_last_synced"
        );
        w.state.lists.retain(|l| l.list_id != fake_id);
        // Verify real L1 still persists cleanly with zero deadlock
        w.toggle_list_pinned(l1_id.clone(), false, cx);
    });
    assert!(
        pump_until(&mut app, Duration::from_secs(4), |cx| {
            let st = &cw.read(cx).state;
            !st.list_membership_in_flight.contains_key(&l1_id)
                && !st.list_membership_dirty.contains(&l1_id)
        })
        .await
    );
    let db_lists_e = fetch_db_lists(&facade).await;
    assert!(!db_lists_e.iter().find(|x| x.list_id == l1_id).unwrap().is_pinned);

    // Phase F: switch_active_actor while L1 flush is in_flight & dirty -> zero cross-actor errors or stuck slots
    let _ = cw.update(&mut app, |w, cx| {
        w.navigate_to(CommunityRoute::BotStudio, cx);
        w.state.bot_studio_name = "Lists Race Bot".to_string();
        w.state.bot_studio_handle = "lists_race_bot".to_string();
        w.state.bot_studio_persona = "Verifies list actor-switch fencing.".to_string();
        w.submit_register_bot(cx);
    });
    assert!(
        pump_until(&mut app, Duration::from_secs(4), |cx| {
            cw.read(cx)
                .state
                .identities
                .as_ref()
                .is_some_and(|i| i.bot_actors.iter().any(|b| b.handle == "lists_race_bot"))
        })
        .await
    );
    let bot_id = cw
        .read_with(&app, |w, _| {
            w.state
                .identities
                .as_ref()
                .unwrap()
                .bot_actors
                .iter()
                .find(|b| b.handle == "lists_race_bot")
                .unwrap()
                .actor_id
                .clone()
        })
        .unwrap();
    let _ = host.shell.update(&mut app, |s, _| {
        s.window_runtime.controller.current_toast = None;
        s.window_runtime.controller.toast_queue.clear();
    });
    let _ = cw.update(&mut app, |w, cx| {
        w.navigate_to(CommunityRoute::BookmarksLists, cx);
        w.set_bookmarks_section(BookmarksListsSection::Lists, cx);
        let _ = w.take_pending_actions();
        w.toggle_selected_list_actor(l1_id.clone(), a1.clone(), cx);
        w.toggle_selected_list_community(l1_id.clone(), "rust".to_string(), cx);
        w.toggle_list_pinned(l1_id.clone(), true, cx);
        w.switch_active_actor(bot_id.clone(), cx);
    });
    app.background_executor().timer(Duration::from_millis(250)).await;
    let shell_err_toasts_f: Vec<String> = host
        .shell
        .read_with(&app, |s, _| {
            let ctrl = &s.window_runtime.controller;
            ctrl.current_toast
                .iter()
                .chain(ctrl.toast_queue.iter())
                .filter(|t| t.duration_ms == 3500)
                .map(|t| t.text.clone())
                .collect()
        })
        .unwrap();
    assert!(
        shell_err_toasts_f.is_empty(),
        "switch_active_actor racing in-flight list toggles must emit zero AppShell error toasts, got: {shell_err_toasts_f:?}"
    );
    let _ = cw.update(&mut app, |w, _| {
        assert_eq!(w.state.active_actor_id, bot_id);
        assert!(w.state.list_membership_in_flight.is_empty());
        assert!(w.state.list_membership_dirty.is_empty());
        let errs: Vec<_> = w
            .take_pending_actions()
            .into_iter()
            .filter(|a| matches!(a, CommunityWorkspaceAction::ShowToast { is_error: true, .. }))
            .collect();
        assert!(
            errs.is_empty(),
            "switch_active_actor racing in-flight list toggles must emit zero error toasts, got: {errs:?}"
        );
    });
}
