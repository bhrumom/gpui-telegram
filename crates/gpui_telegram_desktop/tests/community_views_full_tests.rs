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
use gpui_telegram_desktop::views::community::bookmarks_lists_view::compute_bookmark_folders;
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
    ActorKind, ActorProfileResponse, AlgoPreferencesDto, AutoModActionKind, AutoModRuleDto,
    BookmarkRequest, BookmarkResponse, BotAutonomyMode, CommentSort, CommunityActorDto,
    CommunityCommentDto, CommunityDetailResponse, CommunityIdentitiesResponse, CommunityListDto,
    CommunityListKind, CommunityListsResponse, CommunityMemberRole, CommunityMuteBlockDto,
    CommunityNotificationDto, CommunityNotificationKind, CommunityPostDto,
    CommunitySearchResultDto, CommunitySearchSort, CommunitySearchTab, CreateCommunityListRequest,
    CreatePostRequest, DeleteEntityResponse, EngagementSignalKind, FeedItemDto, FeedRequestDto,
    FeedResponseDto, FeedTab, ListCommunitiesResponse, ModActionKind, ModLogsResponse,
    ModQueueResponse, ModQueueStatus, MuteBlockActionKind, MuteBlockRequest, MuteBlockTargetKind,
    NotificationsListResponse, PostDetailResponse, PostFormat, PostStatus, SubCommunityDto,
    ToggleActionResponse, TrendingResponse, UpdateCommunityListRequest, UpdateCommunityRequest,
    VerifiedTier, VoteRequest, VoteResponse,
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
        "test_m5_f20_bookmarks_single_flight_and_navigate_race_stress" => {
            |app| Box::pin(body_f20_bookmarks_single_flight_and_navigate_race_stress(app))
        }
        "test_m5_r5_online_2f_list_and_2g_automod_races" => {
            |app| Box::pin(body_r5_online_2f_list_and_2g_automod_races(app))
        }
        "test_m5_r6_online_list_editor_create_delete_and_bookmark_rollback_races" => {
            |app| Box::pin(body_r6_online_list_editor_create_delete_and_bookmark_rollback_races(app))
        }
        "test_m5_r6_online_mute_block_like_and_comment_single_flight_races" => {
            |app| Box::pin(body_r6_online_mute_block_like_and_comment_single_flight_races(app))
        }
        "test_m5_r7_online_consistency_races_all_5_fixes" => {
            |app| Box::pin(body_r7_online_consistency_races_all_5_fixes(app))
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
            let st = &cw.read(cx).state;
            !st.detail_summon_pending
                && !st.detail_loading
                && st.detail_summon_feedback.is_some()
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
    assert!(
        pump_until(&mut app, Duration::from_secs(4), |_| {
            host.client
                .community()
                .block_on(host.client.community().get_post(&pid_b, None))
                .is_ok_and(|d| {
                    d.comments
                        .iter()
                        .any(|c| c.body.contains("Cross-post race reply on B"))
                })
        })
        .await,
        "Post B cross-post reply must commit to SQLite before asserting Post A isolation"
    );
    app.background_executor().timer(Duration::from_millis(20)).await;
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
            let st = &cw.read(cx).state;
            !st.community_rules_saving
                && !st.community_loading
                && st
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
            let st = &cw.read(cx).state;
            !st.community_rules_saving
                && !st.community_loading
                && st
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
            let st = &cw.read(cx).state;
            !st.community_wiki_saving
                && !st.community_loading
                && st
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
    let joined_before = cw
        .read_with(&app, |w, _| {
            w.state
                .community_detail
                .as_ref()
                .unwrap()
                .community
                .is_joined
        })
        .unwrap();
    let _ = cw.update(&mut app, |w, cx| {
        w.set_community_tab(CommunityPageTab::Wiki, cx);
        w.state.community_wiki_editing = false;
        w.start_edit_community_wiki(None, cx);
        assert!(
            !w.state.community_wiki_editing,
            "Non-mod MUST NOT enter wiki editing mode"
        );
        // Also verify Join / Leave toggles cleanly
        w.toggle_join_community(seeded_slug.clone(), !joined_before, cx);
    });
    assert!(
        pump_until(&mut app, Duration::from_secs(4), |cx| {
            cw.read(cx)
                .state
                .community_detail
                .as_ref()
                .is_some_and(|d| {
                    d.community.slug == seeded_slug && d.community.is_joined == !joined_before
                })
        })
        .await,
        "toggle_join_community must persist and flip community_detail.community.is_joined"
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
                && host
                    .client
                    .community()
                    .block_on(host.client.community().get_actor_profile(&target_bot.handle))
                    .is_ok_and(|p| p.actor.is_muted)
        })
        .await,
        "Mute actor must persist in SQLite and update actor_profile.actor.is_muted"
    );
    let _ = cw.update(&mut app, |w, cx| {
        w.toggle_mute_actor(target_bot.actor_id.clone(), true, cx);
    });
    assert!(
        pump_until(&mut app, Duration::from_secs(4), |cx| {
            cw.read(cx)
                .state
                .actor_profile
                .as_ref()
                .is_some_and(|p| !p.actor.is_muted)
                && host
                    .client
                    .community()
                    .block_on(host.client.community().get_actor_profile(&target_bot.handle))
                    .is_ok_and(|p| !p.actor.is_muted)
        })
        .await,
        "Unmute actor must persist in SQLite and clear actor_profile.actor.is_muted"
    );
    let _ = cw.update(&mut app, |w, cx| {
        w.toggle_block_actor(target_bot.actor_id.clone(), false, cx);
    });
    assert!(
        pump_until(&mut app, Duration::from_secs(4), |cx| {
            cw.read(cx)
                .state
                .actor_profile
                .as_ref()
                .is_some_and(|p| p.actor.is_blocked)
                && host
                    .client
                    .community()
                    .block_on(host.client.community().get_actor_profile(&target_bot.handle))
                    .is_ok_and(|p| p.actor.is_blocked)
        })
        .await,
        "Block actor must persist in SQLite and update actor_profile.actor.is_blocked"
    );
    let _ = cw.update(&mut app, |w, cx| {
        w.toggle_block_actor(target_bot.actor_id.clone(), true, cx);
    });
    assert!(
        pump_until(&mut app, Duration::from_secs(4), |cx| {
            cw.read(cx)
                .state
                .actor_profile
                .as_ref()
                .is_some_and(|p| !p.actor.is_blocked)
                && host
                    .client
                    .community()
                    .block_on(host.client.community().get_actor_profile(&target_bot.handle))
                    .is_ok_and(|p| !p.actor.is_blocked)
        })
        .await,
        "Unblock actor must persist in SQLite and clear actor_profile.actor.is_blocked"
    );
    app.background_executor().timer(Duration::from_millis(20)).await;
    let _ = cw.update(&mut app, |w, cx| {
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
            let st = &cw.read(cx).state;
            !st.edit_profile.submitting && !st.profile_loading && st.active_modal.is_none()
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
    });
    assert!(
        pump_until(&mut app, Duration::from_secs(4), |cx| {
            !cw.read(cx).state.notifications_loading
        })
        .await,
        "initial load_notifications must settle before injecting synthetic notifications"
    );
    let _ = cw.update(&mut app, |w, cx| {
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
            let st = &cw.read(cx).state;
            !st.notifications_loading
                && st.unread_notifications == 0
                && host
                    .client
                    .community()
                    .block_on(host.client.community().get_notifications(None, Some(10), None))
                    .is_ok_and(|n| n.unread_count == 0)
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
            ctrl.current_toast
                .iter()
                .chain(ctrl.toast_queue.iter())
                .any(|t| t.duration_ms == 3500)
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
            ctrl.current_toast
                .iter()
                .chain(ctrl.toast_queue.iter())
                .any(|t| t.duration_ms == 3500)
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
    let facade_for_f20 = cw.read_with(&app, |w, _| w.facade.clone()).unwrap();
    let _ = cw.update(&mut app, |w, cx| {
        w.move_bookmark_to_folder(sample_pid.clone(), "RustDeep".to_string(), cx);
    });
    assert!(
        pump_until(&mut app, Duration::from_secs(4), |cx| {
            let st = &cw.read(cx).state;
            !st.bookmark_in_flight.contains_key(&sample_pid)
                && !st.bookmark_dirty.contains(&sample_pid)
                && st.find_post(&sample_pid).is_some_and(|p| {
                    p.viewer_bookmarked && p.viewer_bookmark_folder.as_deref() == Some("RustDeep")
                })
        })
        .await,
        "move_bookmark_to_folder must settle single-flight and reconcile folder RustDeep"
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
            let st = &cw.read(cx).state;
            !st.saved_loading
                && st.saved_items.iter().any(|it| {
                    it.post.post_id == sample_pid
                        && it.post.viewer_bookmark_folder.as_deref() == Some("RustDeep")
                })
        })
        .await
    );
    let db_saved_f20 = fetch_db_saved_posts(&facade_for_f20).await;
    assert!(
        db_saved_f20.iter().any(|it| {
            it.post.post_id == sample_pid
                && it.post.viewer_bookmark_folder.as_deref() == Some("RustDeep")
        }),
        "SQLite saved_items must persist sample_pid in RustDeep"
    );
    host.draw(&mut app);

    // Unbookmarking via PostCard's toggle_bookmark MUST remove post from saved_items & SQLite!
    let _ = cw.update(&mut app, |w, cx| {
        w.toggle_bookmark(sample_pid.clone(), cx);
    });
    assert!(
        pump_until(&mut app, Duration::from_secs(4), |cx| {
            let st = &cw.read(cx).state;
            !st.bookmark_in_flight.contains_key(&sample_pid)
                && !st.bookmark_dirty.contains(&sample_pid)
                && !st.saved_items.iter().any(|it| it.post.post_id == sample_pid)
        })
        .await,
        "Unbookmarking via PostCard toggle_bookmark must remove post from saved_items"
    );
    let db_saved_after_remove = fetch_db_saved_posts(&facade_for_f20).await;
    assert!(
        !db_saved_after_remove
            .iter()
            .any(|it| it.post.post_id == sample_pid),
        "Unbookmarking via PostCard toggle_bookmark must delete bookmark in SQLite"
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
    assert!(
        pump_until(&mut app, Duration::from_secs(4), |_| {
            host.client
                .community()
                .block_on(host.client.community().get_mod_queue("mod_lab", None, Some(10)))
                .is_ok_and(|q| q.items.iter().any(|it| it.target_id == post.post_id))
        })
        .await,
        "Report signal must enqueue post into mod_lab mod_queue in SQLite"
    );

    let _ = cw.update(&mut app, |w, cx| {
        w.navigate_to(CommunityRoute::ModQueue(Some("mod_lab".to_string())), cx);
    });
    assert!(
        pump_until(&mut app, Duration::from_secs(4), |cx| {
            let w = cw.read(cx);
            !w.state.mod_loading
                && w.can_moderate_active_mod_community()
                && !w.state.mod_queue.is_empty()
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
            let st = &cw.read(cx).state;
            !st.mod_loading && !st.mod_logs.is_empty()
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
    assert!(
        pump_until(&mut app, Duration::from_secs(4), |cx| {
            let st = &cw.read(cx).state;
            !st.feed_loading && !st.notifications_loading
        })
        .await
    );
    app.background_executor().timer(Duration::from_millis(20)).await;
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
            let w = cw.read(cx);
            !w.state.mod_loading
                && !w.can_moderate_active_mod_community()
                && w.state.mod_error.is_some()
        })
        .await,
        "Non-mod Bot must remain unauthorized with mod_error after ModQueue refresh completes"
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

async fn fetch_db_saved_posts(
    facade: &gpui_telegram_client::CommunityClientFacade,
) -> Vec<FeedItemDto> {
    let f = facade.clone();
    let handle = f.runtime_handle().clone();
    let (tx, rx) = futures::channel::oneshot::channel();
    handle.spawn(async move {
        let _ = tx.send(
            f.get_feed(&FeedRequestDto {
                tab: FeedTab::Saved,
                limit: 50,
                ..Default::default()
            })
            .await,
        );
    });
    rx.await
        .expect("tokio oneshot")
        .expect("facade.get_feed(Saved)")
        .items
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
    get_detail_received: Arc<std::sync::atomic::AtomicUsize>,
    get_detail_completed: Arc<std::sync::atomic::AtomicUsize>,
    patch_received: Arc<std::sync::atomic::AtomicUsize>,
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

    let get_detail_received = Arc::new(AtomicUsize::new(0));
    let get_detail_completed = Arc::new(AtomicUsize::new(0));
    let patch_received = Arc::new(AtomicUsize::new(0));
    let patch_completed = Arc::new(AtomicUsize::new(0));
    let get_rx_cnt = Arc::clone(&get_detail_received);
    let get_cnt = Arc::clone(&get_detail_completed);
    let patch_rx_cnt = Arc::clone(&patch_received);
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
            let get_rx_cnt = Arc::clone(&get_rx_cnt);
            let get_cnt = Arc::clone(&get_cnt);
            let patch_rx_cnt = Arc::clone(&patch_rx_cnt);
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
                        get_rx_cnt.fetch_add(1, Ordering::SeqCst);
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
                        patch_rx_cnt.fetch_add(1, Ordering::SeqCst);
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
            get_detail_received,
            get_detail_completed,
            patch_received,
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
            w.state.communities.len() == 1
                && w.can_moderate_community_slug(&slug)
                && w.state.identities.is_some()
                && !w.state.feed_loading
        })
        .await,
        "Online bootstrap must hydrate probe_mod_lab with Owner role and identities"
    );

    // Sub-test A (F21-MOD-4): Off-route load_community_page delayed Owner GET must NEVER leak Owner role!
    let get_rx_before_a = counters.get_detail_received.load(Ordering::SeqCst);
    let get_done_before_a = counters.get_detail_completed.load(Ordering::SeqCst);
    let _ = cw.update(&mut app, |w, cx| {
        w.navigate_to(CommunityRoute::CommunityPage(slug.clone()), cx);
    });
    assert!(
        pump_until(&mut app, Duration::from_secs(4), |_| {
            counters.get_detail_received.load(Ordering::SeqCst) > get_rx_before_a
        })
        .await,
        "delayed Owner GET /communities/probe_mod_lab must arrive on mock server before actor switch"
    );
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
    assert!(
        pump_until(&mut app, Duration::from_secs(4), |_| {
            counters.get_detail_completed.load(Ordering::SeqCst) > get_done_before_a
        })
        .await,
        "delayed Owner GET /communities/probe_mod_lab must complete on mock server"
    );
    assert!(
        pump_until(&mut app, Duration::from_secs(4), |cx| {
            let st = &cw.read(cx).state;
            !st.feed_loading && !st.notifications_loading
        })
        .await
    );
    app.background_executor().timer(Duration::from_millis(50)).await;
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
    let patch_rx_before_b = counters.patch_received.load(Ordering::SeqCst);
    let patch_done_before_b = counters.patch_completed.load(Ordering::SeqCst);
    let _ = cw.update(&mut app, |w, cx| {
        w.state.automod_draft.name = "Delayed Race Rule".to_string();
        w.state.automod_draft.keywords = "spam".to_string();
        w.submit_add_automod_rule(cx);
        assert!(w.state.automod_draft.saving);
    });
    assert!(
        pump_until(&mut app, Duration::from_secs(4), |_| {
            counters.patch_received.load(Ordering::SeqCst) > patch_rx_before_b
        })
        .await,
        "delayed Owner PATCH (persist_automod_rules) must arrive on mock server before actor switch"
    );
    let _ = cw.update(&mut app, |w, cx| {
        w.navigate_to(CommunityRoute::Feed, cx);
        w.switch_active_actor(bot_id.clone(), cx);
        assert!(!w.state.automod_draft.saving);
        assert!(w.state.mod_community.is_none());
        assert!(!w.can_moderate_community_slug(&slug));
    });
    assert!(
        pump_until(&mut app, Duration::from_secs(4), |_| {
            counters.patch_completed.load(Ordering::SeqCst) > patch_done_before_b
        })
        .await,
        "delayed Owner PATCH (persist_automod_rules) must complete on mock server"
    );
    assert!(
        pump_until(&mut app, Duration::from_secs(4), |cx| {
            let st = &cw.read(cx).state;
            !st.feed_loading && !st.notifications_loading
        })
        .await
    );
    app.background_executor().timer(Duration::from_millis(50)).await;
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
    let patch_rx_before_c = counters.patch_received.load(Ordering::SeqCst);
    let patch_done_before_c = counters.patch_completed.load(Ordering::SeqCst);
    let _ = cw.update(&mut app, |w, cx| {
        w.state.community_new_rule_title = "Delayed Rule Race".to_string();
        w.state.community_new_rule_desc = "Must not leak role on switch".to_string();
        w.add_community_rule(slug.clone(), cx);
        assert!(w.state.community_rules_saving);
    });
    assert!(
        pump_until(&mut app, Duration::from_secs(4), |_| {
            counters.patch_received.load(Ordering::SeqCst) > patch_rx_before_c
        })
        .await,
        "delayed Owner PATCH (save_community_rules) must arrive on mock server before actor switch"
    );
    let _ = cw.update(&mut app, |w, cx| {
        w.navigate_to(CommunityRoute::Feed, cx);
        w.switch_active_actor(bot_id.clone(), cx);
        assert!(!w.state.community_rules_saving);
        assert!(!w.can_moderate_community_slug(&slug));
    });
    assert!(
        pump_until(&mut app, Duration::from_secs(4), |_| {
            counters.patch_completed.load(Ordering::SeqCst) > patch_done_before_c
        })
        .await,
        "delayed Owner PATCH (save_community_rules) must complete on mock server"
    );
    assert!(
        pump_until(&mut app, Duration::from_secs(4), |cx| {
            let st = &cw.read(cx).state;
            !st.feed_loading && !st.notifications_loading
        })
        .await
    );
    app.background_executor().timer(Duration::from_millis(50)).await;
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
    assert!(
        pump_until(&mut app, Duration::from_secs(4), |cx| {
            let st = &cw.read(cx).state;
            !st.saved_loading && !st.feed_loading && !st.notifications_loading
        })
        .await
    );
    app.background_executor().timer(Duration::from_millis(50)).await;
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

// ============================================================================
// Wave r5 Regression 1: Bookmarks Single-Flight Coalescing, Navigate Race
// (Order 1, Order 2, Reverse Order 2), Folder Delete, Rollback & ABA Stress
// ============================================================================

fn mock_post_dto(post_id: &str, author_id: &str, bookmarked: bool, folder: Option<&str>, count: u64) -> CommunityPostDto {
    serde_json::from_value(serde_json::json!({
        "post_id": post_id,
        "community_slug": "rust",
        "author_actor_id": author_id,
        "author_handle": "race_owner",
        "author_display_name": "Race Owner",
        "title": "Online Bookmark Race Post",
        "body": "Deterministic Order 1 & Order 2 bookmark convergence test post",
        "bookmarks_count": count,
        "viewer_bookmarked": bookmarked,
        "viewer_bookmark_folder": folder,
        "created_at_ms": 1_750_000_000_000i64,
        "updated_at_ms": 1_750_000_000_000i64
    }))
    .expect("construct mock CommunityPostDto")
}

struct DelayedBookmarkRaceServerState {
    bookmark_delay_ms: Arc<std::sync::atomic::AtomicU64>,
    lists_delay_ms: Arc<std::sync::atomic::AtomicU64>,
    saved_returns_post: Arc<std::sync::atomic::AtomicBool>,
    lists_received: Arc<std::sync::atomic::AtomicUsize>,
    bookmark_completed: Arc<std::sync::atomic::AtomicUsize>,
    lists_completed: Arc<std::sync::atomic::AtomicUsize>,
}

fn spawn_delayed_bookmark_race_server(
    rt_handle: &tokio::runtime::Handle,
    owner_id: String,
    post_id: String,
) -> (std::net::SocketAddr, DelayedBookmarkRaceServerState) {
    use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    let state = DelayedBookmarkRaceServerState {
        bookmark_delay_ms: Arc::new(AtomicU64::new(0)),
        lists_delay_ms: Arc::new(AtomicU64::new(0)),
        saved_returns_post: Arc::new(AtomicBool::new(false)),
        lists_received: Arc::new(AtomicUsize::new(0)),
        bookmark_completed: Arc::new(AtomicUsize::new(0)),
        lists_completed: Arc::new(AtomicUsize::new(0)),
    };
    let bmark_delay = Arc::clone(&state.bookmark_delay_ms);
    let lists_delay = Arc::clone(&state.lists_delay_ms);
    let saved_ret = Arc::clone(&state.saved_returns_post);
    let lists_rx = Arc::clone(&state.lists_received);
    let bmark_done = Arc::clone(&state.bookmark_completed);
    let lists_done = Arc::clone(&state.lists_completed);

    let std_listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind bookmark mock");
    std_listener.set_nonblocking(true).expect("nonblocking");
    let addr = std_listener.local_addr().expect("local_addr");

    rt_handle.spawn(async move {
        let listener = tokio::net::TcpListener::from_std(std_listener).expect("from_std");
        let owner_actor = mock_actor_dto(&owner_id, ActorKind::Human, "race_owner", "Race Owner");

        while let Ok((mut sock, _)) = listener.accept().await {
            let owner_id = owner_id.clone();
            let post_id = post_id.clone();
            let owner_actor = owner_actor.clone();
            let bmark_delay = Arc::clone(&bmark_delay);
            let lists_delay = Arc::clone(&lists_delay);
            let saved_ret = Arc::clone(&saved_ret);
            let lists_rx = Arc::clone(&lists_rx);
            let bmark_done = Arc::clone(&bmark_done);
            let lists_done = Arc::clone(&lists_done);

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
                let (path, query) = full_target.split_once('?').unwrap_or((full_target, ""));
                let path = path.to_string();
                let query = query.to_string();

                let mut content_len: usize = 0;
                for line in lines {
                    if let Some((k, v)) = line.split_once(':') {
                        if k.trim().eq_ignore_ascii_case("content-length") {
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
                let body_bytes = &buf[header_end..(header_end + content_len).min(buf.len())];
                let bmark_path = format!("/api/v1/community/posts/{post_id}/bookmark");

                let (status, body) = if method == "GET" && path == "/api/v1/community/identities" {
                    let resp = CommunityIdentitiesResponse {
                        active_actor_id: owner_id.clone(),
                        human_actor: Some(owner_actor.clone()),
                        bot_actors: Vec::new(),
                        all_actors: vec![owner_actor.clone()],
                    };
                    ("200 OK", serde_json::to_string(&resp).unwrap())
                } else if method == "POST" && path == "/api/v1/community/identities/human" {
                    ("200 OK", serde_json::to_string(&owner_actor).unwrap())
                } else if method == "GET" && path == "/api/v1/community/communities" {
                    let resp = ListCommunitiesResponse {
                        communities: vec![mock_community_dto("rust", &owner_id, Some(CommunityMemberRole::Owner))],
                    };
                    ("200 OK", serde_json::to_string(&resp).unwrap())
                } else if method == "GET" && path == "/api/v1/community/feed" {
                    if query.contains("tab=saved") {
                        let items = if saved_ret.load(Ordering::SeqCst) {
                            vec![FeedItemDto {
                                post: mock_post_dto(&post_id, &owner_id, true, Some("FastAfterRead"), 1),
                                explanation: None,
                                rank_score: 10.0,
                                is_exploration_slot: false,
                            }]
                        } else {
                            Vec::new()
                        };
                        let resp = FeedResponseDto {
                            items,
                            ..Default::default()
                        };
                        ("200 OK", serde_json::to_string(&resp).unwrap())
                    } else {
                        let resp = FeedResponseDto {
                            items: vec![FeedItemDto {
                                post: mock_post_dto(&post_id, &owner_id, false, None, 0),
                                explanation: None,
                                rank_score: 10.0,
                                is_exploration_slot: false,
                            }],
                            ..Default::default()
                        };
                        ("200 OK", serde_json::to_string(&resp).unwrap())
                    }
                } else if method == "POST" && path == bmark_path {
                    let req: BookmarkRequest = serde_json::from_slice(body_bytes).unwrap_or_default();
                    let delay = bmark_delay.load(Ordering::SeqCst);
                    if delay > 0 {
                        tokio::time::sleep(Duration::from_millis(delay)).await;
                    }
                    let removing = req.remove.unwrap_or(false);
                    let resp = if removing {
                        BookmarkResponse {
                            post_id: post_id.clone(),
                            bookmarked: false,
                            folder: "All".to_string(),
                            bookmarks_count: 0,
                        }
                    } else {
                        BookmarkResponse {
                            post_id: post_id.clone(),
                            bookmarked: true,
                            folder: req.folder.unwrap_or_else(|| "All".to_string()),
                            bookmarks_count: 1,
                        }
                    };
                    bmark_done.fetch_add(1, Ordering::SeqCst);
                    ("200 OK", serde_json::to_string(&resp).unwrap())
                } else if method == "GET" && path == "/api/v1/community/lists" {
                    let delay = lists_delay.load(Ordering::SeqCst);
                    if delay > 0 {
                        lists_rx.fetch_add(1, Ordering::SeqCst);
                        tokio::time::sleep(Duration::from_millis(delay)).await;
                    }
                    lists_done.fetch_add(1, Ordering::SeqCst);
                    ("200 OK", serde_json::to_string(&CommunityListsResponse::default()).unwrap())
                } else if path == "/api/v1/community/notifications" {
                    ("200 OK", serde_json::to_string(&NotificationsListResponse::default()).unwrap())
                } else if path == "/api/v1/community/trending" {
                    ("200 OK", serde_json::to_string(&TrendingResponse::default()).unwrap())
                } else if path == "/api/v1/community/algo-prefs" {
                    ("200 OK", serde_json::to_string(&AlgoPreferencesDto::default()).unwrap())
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

    (addr, state)
}

#[test]
fn test_m5_f20_bookmarks_single_flight_and_navigate_race_stress() {
    run_in_headless_subprocess("test_m5_f20_bookmarks_single_flight_and_navigate_race_stress");
}

async fn body_f20_bookmarks_single_flight_and_navigate_race_stress(mut app: AsyncApp) {
    use std::sync::atomic::Ordering;

    let host = open_community_host(&mut app, 1440.0, 900.0, 7125, "m5_bmark_stress").await;
    let cw = host.cw.clone();
    let facade = cw.read_with(&app, |w, _| w.facade.clone()).unwrap();
    let (pid, pid2, pid2_base_count) = cw
        .read_with(&app, |w, _| {
            let p0 = &w.state.feed_items[0].post;
            let p1 = &w.state.feed_items[1].post;
            (p0.post_id.clone(), p1.post_id.clone(), p1.bookmarks_count)
        })
        .unwrap();

    // Phase A (Auditor Run 1 Direct Race — 15 Repeated Rounds):
    // Same-tick move/toggle + navigate_to(BookmarksLists) / load_saved_posts with ZERO wait!
    for round in 0..15usize {
        if round % 2 == 0 {
            let folder = format!("Folder_{round}");
            let _ = cw.update(&mut app, |w, cx| {
                w.navigate_to(CommunityRoute::Feed, cx);
                w.move_bookmark_to_folder(pid.clone(), folder.clone(), cx);
                w.navigate_to(CommunityRoute::BookmarksLists, cx);
                assert!(
                    w.state.saved_items.iter().any(|it| it.post.post_id == pid),
                    "round {round}: optimistic move must immediately upsert into saved_items"
                );
            });
            assert!(
                pump_until(&mut app, Duration::from_secs(4), |cx| {
                    let st = &cw.read(cx).state;
                    !st.saved_loading
                        && st.bookmark_in_flight.is_empty()
                        && st.bookmark_dirty.is_empty()
                })
                .await,
                "round {round}: bookmark move + navigate must settle cleanly"
            );
            let ui_matches: Vec<Option<String>> = cw
                .read_with(&app, |w, _| {
                    w.state
                        .saved_items
                        .iter()
                        .filter(|it| it.post.post_id == pid)
                        .map(|it| it.post.viewer_bookmark_folder.clone())
                        .collect()
                })
                .unwrap();
            assert_eq!(
                ui_matches,
                vec![Some(folder.clone())],
                "round {round}: UI saved_items must contain pid exactly once in {folder}"
            );
            let db_saved = fetch_db_saved_posts(&facade).await;
            let db_item = db_saved
                .iter()
                .find(|it| it.post.post_id == pid)
                .unwrap_or_else(|| panic!("round {round}: pid missing from SQLite saved_items"));
            assert_eq!(
                db_item.post.viewer_bookmark_folder.as_deref(),
                Some(folder.as_str()),
                "round {round}: SQLite folder mismatch"
            );
        } else {
            let _ = cw.update(&mut app, |w, cx| {
                w.toggle_bookmark(pid.clone(), cx);
                w.load_saved_posts(cx);
                assert!(
                    !w.state.saved_items.iter().any(|it| it.post.post_id == pid),
                    "round {round}: optimistic toggle off must immediately remove from saved_items"
                );
            });
            assert!(
                pump_until(&mut app, Duration::from_secs(4), |cx| {
                    let st = &cw.read(cx).state;
                    !st.saved_loading
                        && st.bookmark_in_flight.is_empty()
                        && st.bookmark_dirty.is_empty()
                })
                .await,
                "round {round}: bookmark toggle off + load_saved_posts must settle cleanly"
            );
            let ui_present = cw
                .read_with(&app, |w, _| {
                    w.state.saved_items.iter().any(|it| it.post.post_id == pid)
                })
                .unwrap();
            assert!(!ui_present, "round {round}: UI must not resurrect removed bookmark");
            let db_saved = fetch_db_saved_posts(&facade).await;
            assert!(
                !db_saved.iter().any(|it| it.post.post_id == pid),
                "round {round}: SQLite must reflect removed bookmark"
            );
        }
    }

    // Phase B: Rapid Burst Single-Flight Coalescing (move->move->remove->move + mid-flight move + load_saved_posts)
    let _ = cw.update(&mut app, |w, cx| {
        w.move_bookmark_to_folder(pid2.clone(), "Alpha".to_string(), cx);
        w.move_bookmark_to_folder(pid2.clone(), "Beta".to_string(), cx);
        w.remove_bookmark_from_saved(pid2.clone(), cx);
        w.move_bookmark_to_folder(pid2.clone(), "Gamma".to_string(), cx);
        assert!(w.state.bookmark_in_flight.contains_key(&pid2));
        assert!(w.state.bookmark_dirty.contains(&pid2));
    });
    app.background_executor().timer(Duration::from_millis(2)).await;
    let _ = cw.update(&mut app, |w, cx| {
        w.move_bookmark_to_folder(pid2.clone(), "Delta".to_string(), cx);
        w.load_saved_posts(cx);
    });
    assert!(
        pump_until(&mut app, Duration::from_secs(4), |cx| {
            let st = &cw.read(cx).state;
            !st.saved_loading && st.bookmark_in_flight.is_empty() && st.bookmark_dirty.is_empty()
        })
        .await
    );
    let (ui_b_folder, ui_b_count) = cw
        .read_with(&app, |w, _| {
            let p = w.state.find_post(&pid2).expect("pid2 in state");
            (p.viewer_bookmark_folder.clone(), p.bookmarks_count)
        })
        .unwrap();
    assert_eq!(ui_b_folder.as_deref(), Some("Delta"));
    assert_eq!(ui_b_count, pid2_base_count + 1);
    let db_b = fetch_db_saved_posts(&facade).await;
    let db_b_item = db_b.iter().find(|it| it.post.post_id == pid2).expect("pid2 in SQLite");
    assert_eq!(db_b_item.post.viewer_bookmark_folder.as_deref(), Some("Delta"));
    assert_eq!(db_b_item.post.bookmarks_count, pid2_base_count + 1);

    // Same-tick move("Epsilon") -> toggle_bookmark(off) + load_saved_posts must converge to removed
    let _ = cw.update(&mut app, |w, cx| {
        w.move_bookmark_to_folder(pid2.clone(), "Epsilon".to_string(), cx);
        w.toggle_bookmark(pid2.clone(), cx);
        w.load_saved_posts(cx);
    });
    assert!(
        pump_until(&mut app, Duration::from_secs(4), |cx| {
            let st = &cw.read(cx).state;
            !st.saved_loading && st.bookmark_in_flight.is_empty() && st.bookmark_dirty.is_empty()
        })
        .await
    );
    let _ = cw.read_with(&app, |w, _| {
        assert!(!w.state.saved_items.iter().any(|it| it.post.post_id == pid2));
        let p = w.state.find_post(&pid2).expect("pid2 in feed");
        assert!(!p.viewer_bookmarked);
        assert_eq!(p.bookmarks_count, pid2_base_count);
    });
    assert!(!fetch_db_saved_posts(&facade).await.iter().any(|it| it.post.post_id == pid2));

    // Phase C: delete_bookmark_folder Race
    let _ = cw.update(&mut app, |w, cx| {
        w.state.bookmark_new_folder_draft = "TempFolder".to_string();
        w.create_bookmark_folder(cx);
        w.move_bookmark_to_folder(pid2.clone(), "TempFolder".to_string(), cx);
        w.delete_bookmark_folder("TempFolder".to_string(), cx);
        w.load_saved_posts(cx);
    });
    assert!(
        pump_until(&mut app, Duration::from_secs(4), |cx| {
            let st = &cw.read(cx).state;
            !st.saved_loading && st.bookmark_in_flight.is_empty() && st.bookmark_dirty.is_empty()
        })
        .await
    );
    let _ = cw.read_with(&app, |w, _| {
        let folders = compute_bookmark_folders(w);
        assert!(
            !folders.iter().any(|f| f.eq_ignore_ascii_case("TempFolder")),
            "Deleted folder must disappear from compute_bookmark_folders: {folders:?}"
        );
        let item = w
            .state
            .saved_items
            .iter()
            .find(|it| it.post.post_id == pid2)
            .expect("pid2 preserved in saved_items after folder delete");
        assert_eq!(item.post.viewer_bookmark_folder.as_deref(), Some("All"));
    });
    let db_c = fetch_db_saved_posts(&facade).await;
    let db_c_item = db_c.iter().find(|it| it.post.post_id == pid2).expect("pid2 in SQLite");
    assert_eq!(db_c_item.post.viewer_bookmark_folder.as_deref(), Some("All"));

    // Phase D: Error Rollback & Zero Deadlock
    let fake_pid = "missing_bmark_post_probe".to_string();
    let _ = host.shell.update(&mut app, |s, _| {
        s.window_runtime.controller.current_toast = None;
        s.window_runtime.controller.toast_queue.clear();
    });
    let _ = cw.update(&mut app, |w, cx| {
        let mut fake_post = w.state.feed_items[0].post.clone();
        fake_post.post_id = fake_pid.clone();
        fake_post.viewer_bookmarked = false;
        fake_post.viewer_bookmark_folder = None;
        fake_post.bookmarks_count = 0;
        w.state.feed_items.push(FeedItemDto {
            post: fake_post,
            explanation: None,
            rank_score: 0.0,
            is_exploration_slot: false,
        });
        w.move_bookmark_to_folder(fake_pid.clone(), "ErrA".to_string(), cx);
        w.move_bookmark_to_folder(fake_pid.clone(), "ErrB".to_string(), cx);
    });
    assert!(
        pump_until(&mut app, Duration::from_secs(4), |cx| {
            !cw.read(cx).state.bookmark_in_flight.contains_key(&fake_pid)
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
        "Terminal bookmark error must emit AppShell error toast"
    );
    let _ = cw.update(&mut app, |w, cx| {
        let st = &w.state;
        assert!(!st.bookmark_dirty.contains(&fake_pid));
        assert!(!st.bookmark_pending_target.contains_key(&fake_pid));
        assert!(!st.bookmark_last_synced.contains_key(&fake_pid));
        assert!(!st.bookmark_cached_items.contains_key(&fake_pid));
        assert!(!st.saved_items.iter().any(|it| it.post.post_id == fake_pid));
        let fp = st.find_post(&fake_pid).expect("synthetic post in feed");
        assert!(!fp.viewer_bookmarked);
        assert!(fp.viewer_bookmark_folder.is_none());
        assert_eq!(fp.bookmarks_count, 0);
        w.state.feed_items.retain(|it| it.post.post_id != fake_pid);
        // Subsequent real post bookmark must still persist cleanly (zero deadlock)
        w.move_bookmark_to_folder(pid2.clone(), "Recovered".to_string(), cx);
    });
    assert!(
        pump_until(&mut app, Duration::from_secs(4), |cx| {
            let st = &cw.read(cx).state;
            st.bookmark_in_flight.is_empty() && st.bookmark_dirty.is_empty()
        })
        .await
    );
    let db_d = fetch_db_saved_posts(&facade).await;
    assert_eq!(
        db_d.iter()
            .find(|it| it.post.post_id == pid2)
            .and_then(|it| it.post.viewer_bookmark_folder.as_deref()),
        Some("Recovered")
    );

    // Phase E: switch_active_actor ABA Race (A -> B -> A)
    let owner_id = cw.read_with(&app, |w, _| w.state.active_actor_id.clone()).unwrap();
    let _ = cw.update(&mut app, |w, cx| {
        w.navigate_to(CommunityRoute::BotStudio, cx);
        w.state.bot_studio_name = "Bookmark Race Bot".to_string();
        w.state.bot_studio_handle = "bmark_race_bot".to_string();
        w.state.bot_studio_persona = "Verifies bookmark ABA actor-switch fencing.".to_string();
        w.submit_register_bot(cx);
    });
    assert!(
        pump_until(&mut app, Duration::from_secs(4), |cx| {
            let st = &cw.read(cx).state;
            !st.bot_studio_submitting
                && st
                    .identities
                    .as_ref()
                    .is_some_and(|i| i.bot_actors.iter().any(|b| b.handle == "bmark_race_bot"))
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
                .find(|b| b.handle == "bmark_race_bot")
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
        w.move_bookmark_to_folder(pid2.clone(), "PreSwitch1".to_string(), cx);
        w.move_bookmark_to_folder(pid2.clone(), "PreSwitch2".to_string(), cx);
        w.switch_active_actor(bot_id.clone(), cx);
        assert!(w.state.saved_items.is_empty());
        assert!(w.state.bookmark_moving_post_id.is_none());
        assert!(w.state.bookmark_in_flight.is_empty());
        assert!(w.state.bookmark_dirty.is_empty());
        assert!(w.state.bookmark_pending_target.is_empty());
        assert!(w.state.bookmark_last_synced.is_empty());
        assert!(w.state.bookmark_cached_items.is_empty());
        assert!(w.state.bookmark_recent_overrides.is_empty());
    });
    assert!(
        pump_until(&mut app, Duration::from_secs(4), |cx| {
            let st = &cw.read(cx).state;
            !st.saved_loading && !st.feed_loading && !st.notifications_loading
        })
        .await
    );
    app.background_executor().timer(Duration::from_millis(40)).await;
    let err_toasts_e: Vec<String> = host
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
    assert!(err_toasts_e.is_empty(), "Unexpected error toasts on actor switch: {err_toasts_e:?}");
    let _ = cw.update(&mut app, |w, cx| {
        w.switch_active_actor(owner_id.clone(), cx);
    });
    assert!(
        pump_until(&mut app, Duration::from_secs(4), |cx| {
            let st = &cw.read(cx).state;
            !st.saved_loading && !st.feed_loading && !st.notifications_loading
        })
        .await
    );
    let _ = cw.update(&mut app, |w, cx| {
        w.move_bookmark_to_folder(pid2.clone(), "PostABA".to_string(), cx);
    });
    assert!(
        pump_until(&mut app, Duration::from_secs(4), |cx| {
            let st = &cw.read(cx).state;
            st.bookmark_in_flight.is_empty() && st.bookmark_dirty.is_empty()
        })
        .await
    );
    assert_eq!(
        fetch_db_saved_posts(&facade)
            .await
            .iter()
            .find(|it| it.post.post_id == pid2)
            .and_then(|it| it.post.viewer_bookmark_folder.as_deref()),
        Some("PostABA")
    );

    // Phase F: Deterministic Online Delayed-RTT Server Proof of Order 1, Order 2, & Reverse Order 2
    let online_dir = TempDir::new().expect("online bookmark tempdir");
    let bootstrap_dir = TempDir::new().expect("online bookmark rt tempdir");
    let rt_holder = TelegramClient::open_with_dir(
        ClientConfig {
            server_url: "http://127.0.0.1:9".to_string(),
            max_reconnect_attempts: 0,
            ..Default::default()
        },
        bootstrap_dir.path(),
    )
    .expect("open rt holder");
    let online_owner = "u:902305".to_string();
    let online_pid = "p_online_bmark_race".to_string();
    let (addr, srv) = spawn_delayed_bookmark_race_server(
        rt_holder.runtime().handle(),
        online_owner.clone(),
        online_pid.clone(),
    );
    let online_client = Arc::new(
        TelegramClient::open_with_dir(
            ClientConfig {
                server_url: format!("http://{addr}"),
                max_reconnect_attempts: 0,
                ..Default::default()
            },
            online_dir.path(),
        )
        .expect("open online bookmark client"),
    );
    let online_auth = test_auth(902305, "race_owner");
    online_client.set_auth_session(online_auth.clone());
    let online_client_win = Arc::clone(&online_client);
    let online_auth_win = online_auth.clone();
    let online_shell = app
        .update(|cx| {
            let _rt_guard = online_client_win.runtime().enter();
            cx.new(|cx| {
                let mut s = AppShell::new_with_client(online_client_win.clone(), cx);
                s.apply_authenticated_session(online_auth_win.clone(), cx);
                s
            })
        })
        .expect("mount online bookmark shell");
    let _ = online_shell.update(&mut app, |s, cx| s.open_community_workspace(cx));
    let ocw = online_shell
        .read_with(&app, |s, _| s.community_workspace.clone().unwrap())
        .unwrap();
    assert!(
        pump_until(&mut app, Duration::from_secs(5), |cx| {
            let st = &ocw.read(cx).state;
            !st.feed_loading
                && st.identities.is_some()
                && st.feed_items.iter().any(|it| it.post.post_id == online_pid)
        })
        .await,
        "Online bookmark race host must bootstrap feed & identities"
    );

    // F1 (Order 1): GET /feed?tab=saved resolves [] immediately while POST .../bookmark sleeps 180ms
    srv.saved_returns_post.store(false, Ordering::SeqCst);
    srv.bookmark_delay_ms.store(180, Ordering::SeqCst);
    srv.lists_delay_ms.store(0, Ordering::SeqCst);
    let bmark_done_f1 = srv.bookmark_completed.load(Ordering::SeqCst);
    let _ = ocw.update(&mut app, |w, cx| {
        w.move_bookmark_to_folder(online_pid.clone(), "SlowAdd".to_string(), cx);
        w.load_saved_posts(cx);
    });
    assert!(
        pump_until(&mut app, Duration::from_secs(4), |cx| !ocw.read(cx).state.saved_loading).await
    );
    let _ = ocw.read_with(&app, |w, _| {
        assert!(
            w.state.saved_items.iter().any(|it| {
                it.post.post_id == online_pid
                    && it.post.viewer_bookmark_folder.as_deref() == Some("SlowAdd")
            }),
            "F1 Order 1: stale [] load_saved_posts must preserve in-flight optimistic bookmark"
        );
    });
    assert!(
        pump_until(&mut app, Duration::from_secs(4), |cx| {
            srv.bookmark_completed.load(Ordering::SeqCst) > bmark_done_f1
                && ocw.read(cx).state.bookmark_in_flight.is_empty()
        })
        .await
    );
    let _ = ocw.read_with(&app, |w, _| {
        let it = w
            .state
            .saved_items
            .iter()
            .find(|it| it.post.post_id == online_pid)
            .expect("F1: pid preserved after settlement");
        assert_eq!(it.post.viewer_bookmark_folder.as_deref(), Some("SlowAdd"));
        assert_eq!(it.post.bookmarks_count, 1);
    });

    // Reset bookmark cleanly before F2
    srv.bookmark_delay_ms.store(0, Ordering::SeqCst);
    let _ = ocw.update(&mut app, |w, cx| w.remove_bookmark_from_saved(online_pid.clone(), cx));
    assert!(
        pump_until(&mut app, Duration::from_secs(4), |cx| {
            ocw.read(cx).state.bookmark_in_flight.is_empty()
        })
        .await
    );

    // F2 (Order 2 — Killer 2-step load_saved_posts race):
    // GET /feed?tab=saved returns [] immediately (0ms), POST .../bookmark commits in 25ms
    // and clears bookmark_in_flight WHILE GET /lists is still sleeping 220ms!
    srv.saved_returns_post.store(false, Ordering::SeqCst);
    srv.bookmark_delay_ms.store(25, Ordering::SeqCst);
    srv.lists_delay_ms.store(220, Ordering::SeqCst);
    let lists_rx_f2 = srv.lists_received.load(Ordering::SeqCst);
    let lists_done_f2 = srv.lists_completed.load(Ordering::SeqCst);
    let _ = ocw.update(&mut app, |w, cx| w.load_saved_posts(cx));
    assert!(
        pump_until(&mut app, Duration::from_secs(4), |_| {
            srv.lists_received.load(Ordering::SeqCst) > lists_rx_f2
        })
        .await,
        "F2: GET /feed?tab=saved must finish with [] and enter GET /lists sleep before bookmark_post"
    );
    let _ = ocw.update(&mut app, |w, cx| {
        w.move_bookmark_to_folder(online_pid.clone(), "FastAfterRead".to_string(), cx);
    });
    assert!(
        pump_until(&mut app, Duration::from_secs(4), |cx| {
            ocw.read(cx).state.bookmark_in_flight.is_empty()
        })
        .await
    );
    let _ = ocw.read_with(&app, |w, _| {
        assert!(
            w.state.saved_loading,
            "F2 Order 2 invariant: bookmark_post must settle while load_saved_posts is still awaiting GET /lists"
        );
        assert!(
            w.state.bookmark_recent_overrides.contains_key(&online_pid),
            "F2 Order 2: bookmark_recent_overrides must record post-settlement override while saved_loading"
        );
    });
    assert!(
        pump_until(&mut app, Duration::from_secs(4), |cx| {
            srv.lists_completed.load(Ordering::SeqCst) > lists_done_f2
                && !ocw.read(cx).state.saved_loading
        })
        .await
    );
    let _ = ocw.read_with(&app, |w, _| {
        let it = w
            .state
            .saved_items
            .iter()
            .find(|it| it.post.post_id == online_pid)
            .expect("F2 Order 2: stale [] load_saved_posts must NOT drop committed bookmark");
        assert_eq!(it.post.viewer_bookmark_folder.as_deref(), Some("FastAfterRead"));
        assert_eq!(it.post.bookmarks_count, 1);
        assert!(
            w.state.bookmark_recent_overrides.is_empty(),
            "F2: bookmark_recent_overrides must clear once saved_loading completes"
        );
    });

    // F3 (Reverse Order 2 — Remove):
    // GET /feed?tab=saved returns [online_pid] immediately (0ms), remove commits in 25ms
    // while GET /lists sleeps 220ms -> online_pid MUST NOT be resurrected!
    srv.saved_returns_post.store(true, Ordering::SeqCst);
    srv.bookmark_delay_ms.store(25, Ordering::SeqCst);
    srv.lists_delay_ms.store(220, Ordering::SeqCst);
    let lists_rx_f3 = srv.lists_received.load(Ordering::SeqCst);
    let lists_done_f3 = srv.lists_completed.load(Ordering::SeqCst);
    let _ = ocw.update(&mut app, |w, cx| w.load_saved_posts(cx));
    assert!(
        pump_until(&mut app, Duration::from_secs(4), |_| {
            srv.lists_received.load(Ordering::SeqCst) > lists_rx_f3
        })
        .await
    );
    let _ = ocw.update(&mut app, |w, cx| w.remove_bookmark_from_saved(online_pid.clone(), cx));
    assert!(
        pump_until(&mut app, Duration::from_secs(4), |cx| {
            ocw.read(cx).state.bookmark_in_flight.is_empty()
        })
        .await
    );
    let _ = ocw.read_with(&app, |w, _| {
        assert!(w.state.saved_loading, "F3 Reverse Order 2: remove must settle while saved_loading");
        assert!(w.state.bookmark_recent_overrides.contains_key(&online_pid));
    });
    assert!(
        pump_until(&mut app, Duration::from_secs(4), |cx| {
            srv.lists_completed.load(Ordering::SeqCst) > lists_done_f3
                && !ocw.read(cx).state.saved_loading
        })
        .await
    );
    let _ = ocw.read_with(&app, |w, _| {
        assert!(
            !w.state.saved_items.iter().any(|it| it.post.post_id == online_pid),
            "F3 Reverse Order 2: stale [pid] load_saved_posts must NOT resurrect removed bookmark"
        );
        assert!(w.state.bookmark_recent_overrides.is_empty());
    });
    drop(rt_holder);
}

// ============================================================================
// Wave r5 Regression 2: Challenger 2 Online Delayed-RTT Races
// (`2F LIST-STALE-LOAD-CLOBBER` & `2G AUTOMOD-FILTER-DROP`)
// ============================================================================

struct Delayed2f2gServerState {
    lists_get_delay_ms: Arc<std::sync::atomic::AtomicU64>,
    lists_get_received: Arc<std::sync::atomic::AtomicUsize>,
    lists_get_completed: Arc<std::sync::atomic::AtomicUsize>,
    automod_patch_delay_ms: Arc<std::sync::atomic::AtomicU64>,
    automod_patch_received: Arc<std::sync::atomic::AtomicUsize>,
    automod_patch_completed: Arc<std::sync::atomic::AtomicUsize>,
    list_truth: Arc<std::sync::Mutex<CommunityListDto>>,
    comm_truth: Arc<std::sync::Mutex<SubCommunityDto>>,
}

fn spawn_delayed_2f_2g_race_server(
    rt_handle: &tokio::runtime::Handle,
    owner_id: String,
    slug: String,
    list_id: String,
) -> (std::net::SocketAddr, Delayed2f2gServerState) {
    use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    let mut initial_comm = mock_community_dto(&slug, &owner_id, Some(CommunityMemberRole::Owner));
    initial_comm.automod_rules = vec![
        AutoModRuleDto {
            rule_id: "rule_1".to_string(),
            community_id: initial_comm.community_id.clone(),
            name: "Rule 1".to_string(),
            enabled: true,
            priority: 1,
            keywords: vec!["spam1".to_string()],
            regex_pattern: None,
            min_author_karma: None,
            min_account_age_secs: None,
            max_links: None,
            block_duplicate_spam: false,
            apply_to_bots: true,
            apply_to_humans: true,
            action: AutoModActionKind::FlagForReview,
            assign_flair: None,
            auto_reply_template: None,
            created_at_ms: 1_750_000_000_000,
        },
        AutoModRuleDto {
            rule_id: "rule_2".to_string(),
            community_id: initial_comm.community_id.clone(),
            name: "Rule 2".to_string(),
            enabled: true,
            priority: 2,
            keywords: vec!["spam2".to_string()],
            regex_pattern: None,
            min_author_karma: None,
            min_account_age_secs: None,
            max_links: None,
            block_duplicate_spam: false,
            apply_to_bots: true,
            apply_to_humans: true,
            action: AutoModActionKind::Remove,
            assign_flair: None,
            auto_reply_template: None,
            created_at_ms: 1_750_000_000_001,
        },
    ];
    let initial_list = CommunityListDto {
        list_id: list_id.clone(),
        owner_actor_id: owner_id.clone(),
        name: "2F Race List".to_string(),
        description: "Stale load_saved_posts clobber test".to_string(),
        kind: CommunityListKind::Mixed,
        is_private: false,
        is_pinned: false,
        member_actor_ids: Vec::new(),
        community_slugs: Vec::new(),
        created_at_ms: 1_750_000_000_000,
        updated_at_ms: 1_750_000_000_000,
    };

    let state = Delayed2f2gServerState {
        lists_get_delay_ms: Arc::new(AtomicU64::new(0)),
        lists_get_received: Arc::new(AtomicUsize::new(0)),
        lists_get_completed: Arc::new(AtomicUsize::new(0)),
        automod_patch_delay_ms: Arc::new(AtomicU64::new(0)),
        automod_patch_received: Arc::new(AtomicUsize::new(0)),
        automod_patch_completed: Arc::new(AtomicUsize::new(0)),
        list_truth: Arc::new(std::sync::Mutex::new(initial_list)),
        comm_truth: Arc::new(std::sync::Mutex::new(initial_comm)),
    };
    let lg_delay = Arc::clone(&state.lists_get_delay_ms);
    let lg_rx = Arc::clone(&state.lists_get_received);
    let lg_done = Arc::clone(&state.lists_get_completed);
    let am_delay = Arc::clone(&state.automod_patch_delay_ms);
    let am_rx = Arc::clone(&state.automod_patch_received);
    let am_done = Arc::clone(&state.automod_patch_completed);
    let list_truth = Arc::clone(&state.list_truth);
    let comm_truth = Arc::clone(&state.comm_truth);

    let std_listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind 2f/2g mock");
    std_listener.set_nonblocking(true).expect("nonblocking");
    let addr = std_listener.local_addr().expect("local_addr");

    rt_handle.spawn(async move {
        let listener = tokio::net::TcpListener::from_std(std_listener).expect("from_std");
        let owner_actor = mock_actor_dto(&owner_id, ActorKind::Human, "race_owner", "Race Owner");

        while let Ok((mut sock, _)) = listener.accept().await {
            let owner_id = owner_id.clone();
            let slug = slug.clone();
            let list_id = list_id.clone();
            let owner_actor = owner_actor.clone();
            let lg_delay = Arc::clone(&lg_delay);
            let lg_rx = Arc::clone(&lg_rx);
            let lg_done = Arc::clone(&lg_done);
            let am_delay = Arc::clone(&am_delay);
            let am_rx = Arc::clone(&am_rx);
            let am_done = Arc::clone(&am_done);
            let list_truth = Arc::clone(&list_truth);
            let comm_truth = Arc::clone(&comm_truth);

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

                let mut content_len: usize = 0;
                for line in lines {
                    if let Some((k, v)) = line.split_once(':') {
                        if k.trim().eq_ignore_ascii_case("content-length") {
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
                let body_bytes = &buf[header_end..(header_end + content_len).min(buf.len())];
                let detail_path = format!("/api/v1/community/communities/{slug}");
                let queue_path = format!("/api/v1/community/communities/{slug}/mod-queue");
                let logs_path = format!("/api/v1/community/communities/{slug}/mod-logs");
                let list_patch_path = format!("/api/v1/community/lists/{list_id}");

                let (status, body) = if method == "GET" && path == "/api/v1/community/identities" {
                    let resp = CommunityIdentitiesResponse {
                        active_actor_id: owner_id.clone(),
                        human_actor: Some(owner_actor.clone()),
                        bot_actors: Vec::new(),
                        all_actors: vec![owner_actor.clone()],
                    };
                    ("200 OK", serde_json::to_string(&resp).unwrap())
                } else if method == "POST" && path == "/api/v1/community/identities/human" {
                    ("200 OK", serde_json::to_string(&owner_actor).unwrap())
                } else if method == "GET" && path == "/api/v1/community/communities" {
                    let c = comm_truth.lock().unwrap().clone();
                    ("200 OK", serde_json::to_string(&ListCommunitiesResponse { communities: vec![c] }).unwrap())
                } else if method == "GET" && path == detail_path {
                    let c = comm_truth.lock().unwrap().clone();
                    let resp = CommunityDetailResponse {
                        community: c,
                        pinned_posts: Vec::new(),
                        moderators: vec![owner_actor.clone()],
                        wiki_revisions: Vec::new(),
                    };
                    ("200 OK", serde_json::to_string(&resp).unwrap())
                } else if method == "PATCH" && path == detail_path {
                    let req: UpdateCommunityRequest =
                        serde_json::from_slice(body_bytes).unwrap_or_default();
                    let seq = am_rx.fetch_add(1, Ordering::SeqCst);
                    let delay = am_delay.load(Ordering::SeqCst);
                    if delay > 0 && seq == 0 {
                        tokio::time::sleep(Duration::from_millis(delay)).await;
                    }
                    let updated = {
                        let mut c = comm_truth.lock().unwrap();
                        if let Some(rules) = req.automod_rules {
                            c.automod_rules = rules;
                        }
                        c.clone()
                    };
                    am_done.fetch_add(1, Ordering::SeqCst);
                    ("200 OK", serde_json::to_string(&updated).unwrap())
                } else if method == "GET" && path == "/api/v1/community/lists" {
                    // Snapshot BEFORE sleeping to deterministically reproduce 2F stale read!
                    let snap = list_truth.lock().unwrap().clone();
                    let delay = lg_delay.load(Ordering::SeqCst);
                    if delay > 0 {
                        lg_rx.fetch_add(1, Ordering::SeqCst);
                        tokio::time::sleep(Duration::from_millis(delay)).await;
                        lg_done.fetch_add(1, Ordering::SeqCst);
                    }
                    let resp = CommunityListsResponse { lists: vec![snap] };
                    ("200 OK", serde_json::to_string(&resp).unwrap())
                } else if method == "PATCH" && path == list_patch_path {
                    let req: UpdateCommunityListRequest =
                        serde_json::from_slice(body_bytes).unwrap_or_default();
                    let updated = {
                        let mut l = list_truth.lock().unwrap();
                        if let Some(p) = req.is_pinned {
                            l.is_pinned = p;
                        }
                        if let Some(m) = req.member_actor_ids {
                            l.member_actor_ids = m;
                        }
                        if let Some(s) = req.community_slugs {
                            l.community_slugs = s;
                        }
                        l.clone()
                    };
                    ("200 OK", serde_json::to_string(&updated).unwrap())
                } else if method == "GET" && path == queue_path {
                    ("200 OK", serde_json::to_string(&ModQueueResponse::default()).unwrap())
                } else if method == "GET" && path == logs_path {
                    ("200 OK", serde_json::to_string(&ModLogsResponse::default()).unwrap())
                } else if path == "/api/v1/community/feed" {
                    ("200 OK", serde_json::to_string(&FeedResponseDto::default()).unwrap())
                } else if path == "/api/v1/community/notifications" {
                    ("200 OK", serde_json::to_string(&NotificationsListResponse::default()).unwrap())
                } else if path == "/api/v1/community/trending" {
                    ("200 OK", serde_json::to_string(&TrendingResponse::default()).unwrap())
                } else if path == "/api/v1/community/algo-prefs" {
                    ("200 OK", serde_json::to_string(&AlgoPreferencesDto::default()).unwrap())
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

    (addr, state)
}

#[test]
fn test_m5_r5_online_2f_list_and_2g_automod_races() {
    run_in_headless_subprocess("test_m5_r5_online_2f_list_and_2g_automod_races");
}

async fn body_r5_online_2f_list_and_2g_automod_races(mut app: AsyncApp) {
    use std::sync::atomic::Ordering;

    let dir = TempDir::new().expect("2f/2g tempdir");
    let rt_dir = TempDir::new().expect("2f/2g rt tempdir");
    let rt_holder = TelegramClient::open_with_dir(
        ClientConfig {
            server_url: "http://127.0.0.1:9".to_string(),
            max_reconnect_attempts: 0,
            ..Default::default()
        },
        rt_dir.path(),
    )
    .expect("open rt holder");
    let owner_id = "u:902309".to_string();
    let slug = "probe_mod_lab".to_string();
    let list_id = "list_race_1".to_string();
    let (addr, srv) = spawn_delayed_2f_2g_race_server(
        rt_holder.runtime().handle(),
        owner_id.clone(),
        slug.clone(),
        list_id.clone(),
    );

    let client = Arc::new(
        TelegramClient::open_with_dir(
            ClientConfig {
                server_url: format!("http://{addr}"),
                max_reconnect_attempts: 0,
                ..Default::default()
            },
            dir.path(),
        )
        .expect("open 2f/2g client"),
    );
    let auth = test_auth(902309, "race_owner");
    client.set_auth_session(auth.clone());
    let client_win = Arc::clone(&client);
    let auth_win = auth.clone();
    let shell = app
        .update(|cx| {
            let _rt_guard = client_win.runtime().enter();
            cx.new(|cx| {
                let mut s = AppShell::new_with_client(client_win.clone(), cx);
                s.apply_authenticated_session(auth_win.clone(), cx);
                s
            })
        })
        .expect("mount 2f/2g AppShell");
    let _ = shell.update(&mut app, |s, cx| s.open_community_workspace(cx));
    let cw = shell
        .read_with(&app, |s, _| s.community_workspace.clone().unwrap())
        .unwrap();

    assert!(
        pump_until(&mut app, Duration::from_secs(5), |cx| {
            let w = cw.read(cx);
            !w.state.feed_loading
                && w.state.identities.is_some()
                && w.state.lists.iter().any(|l| l.list_id == list_id)
                && w.can_moderate_community_slug(&slug)
        })
        .await,
        "2F/2G online host must bootstrap lists & probe_mod_lab"
    );

    // Sub-test 2F (LIST-STALE-LOAD-CLOBBER):
    // Delayed GET /lists snapshots [] -> fast PATCH commits ["u:100"] & clears in_flight ->
    // stale GET /lists finishes -> must NOT clobber UI or wipe ["u:100"] on next community toggle!
    let _ = cw.update(&mut app, |w, cx| {
        w.navigate_to(CommunityRoute::BookmarksLists, cx);
        w.set_bookmarks_section(BookmarksListsSection::Lists, cx);
    });
    assert!(
        pump_until(&mut app, Duration::from_secs(4), |cx| !cw.read(cx).state.saved_loading).await
    );
    srv.lists_get_delay_ms.store(240, Ordering::SeqCst);
    let lg_rx_before = srv.lists_get_received.load(Ordering::SeqCst);
    let lg_done_before = srv.lists_get_completed.load(Ordering::SeqCst);
    let _ = cw.update(&mut app, |w, cx| w.load_saved_posts(cx));
    assert!(
        pump_until(&mut app, Duration::from_secs(4), |_| {
            srv.lists_get_received.load(Ordering::SeqCst) > lg_rx_before
        })
        .await,
        "2F: stale GET /lists must snapshot [] and enter 240ms sleep before list toggle"
    );
    let _ = cw.update(&mut app, |w, cx| {
        w.toggle_selected_list_actor(list_id.clone(), "u:100".to_string(), cx);
    });
    assert!(
        pump_until(&mut app, Duration::from_secs(4), |cx| {
            let st = &cw.read(cx).state;
            !st.list_membership_in_flight.contains_key(&list_id)
                && !st.list_membership_dirty.contains(&list_id)
        })
        .await,
        "2F: fast PATCH must commit and clear in_flight while delayed GET /lists is still sleeping"
    );
    assert!(
        pump_until(&mut app, Duration::from_secs(4), |cx| {
            srv.lists_get_completed.load(Ordering::SeqCst) > lg_done_before
                && !cw.read(cx).state.saved_loading
        })
        .await
    );
    app.background_executor().timer(Duration::from_millis(30)).await;
    let ui_after_stale_load = cw
        .read_with(&app, |w, _| {
            w.state
                .lists
                .iter()
                .find(|l| l.list_id == list_id)
                .unwrap()
                .member_actor_ids
                .clone()
        })
        .unwrap();
    assert_eq!(
        ui_after_stale_load,
        vec!["u:100".to_string()],
        "2F LIST-STALE-LOAD-CLOBBER: delayed GET /lists must NOT clobber committed actor toggle"
    );
    srv.lists_get_delay_ms.store(0, Ordering::SeqCst);
    let _ = cw.update(&mut app, |w, cx| {
        w.toggle_selected_list_community(list_id.clone(), "rust".to_string(), cx);
    });
    assert!(
        pump_until(&mut app, Duration::from_secs(4), |cx| {
            let st = &cw.read(cx).state;
            !st.list_membership_in_flight.contains_key(&list_id)
                && !st.list_membership_dirty.contains(&list_id)
        })
        .await
    );
    let srv_list = srv.list_truth.lock().unwrap().clone();
    assert_eq!(srv_list.member_actor_ids, vec!["u:100".to_string()]);
    assert_eq!(srv_list.community_slugs, vec!["rust".to_string()]);

    // Sub-test 2G (AUTOMOD-FILTER-DROP):
    // Rapidly toggle rule_1 + rule_2 (Flight 1 PATCH sleeps 180ms; rule_2 queued in automod_dirty) ->
    // mid-flight set_mod_status_filter triggers same-slug load_mod_queue ->
    // must NOT clear automod_in_flight/dirty or drop rule_2 toggle!
    let _ = cw.update(&mut app, |w, cx| {
        w.navigate_to(CommunityRoute::ModQueue(Some(slug.clone())), cx);
    });
    assert!(
        pump_until(&mut app, Duration::from_secs(4), |cx| {
            let w = cw.read(cx);
            !w.state.mod_loading
                && w.state
                    .mod_community
                    .as_ref()
                    .is_some_and(|c| c.automod_rules.len() == 2)
        })
        .await
    );
    srv.automod_patch_delay_ms.store(180, Ordering::SeqCst);
    let _ = cw.update(&mut app, |w, cx| {
        w.toggle_automod_rule_enabled("rule_1".to_string(), cx);
        w.toggle_automod_rule_enabled("rule_2".to_string(), cx);
        assert!(w.state.automod_in_flight.contains_key(&slug));
        assert!(w.state.automod_dirty.contains(&slug));
    });
    assert!(
        pump_until(&mut app, Duration::from_secs(4), |_| {
            srv.automod_patch_received.load(Ordering::SeqCst) >= 1
        })
        .await,
        "2G: Flight 1 AutoMod PATCH must arrive on mock server before mid-flight filter reload"
    );
    let _ = cw.update(&mut app, |w, cx| {
        w.set_mod_status_filter(Some(ModQueueStatus::Removed), cx);
        assert!(
            w.state.automod_in_flight.contains_key(&slug),
            "2G: same-slug load_mod_queue must preserve automod_in_flight"
        );
        assert!(
            w.state.automod_dirty.contains(&slug),
            "2G: same-slug load_mod_queue must preserve automod_dirty"
        );
    });
    assert!(
        pump_until(&mut app, Duration::from_secs(5), |cx| {
            let st = &cw.read(cx).state;
            !st.mod_loading
                && !st.automod_draft.saving
                && st.automod_in_flight.is_empty()
                && st.automod_dirty.is_empty()
                && srv.automod_patch_completed.load(Ordering::SeqCst) >= 2
        })
        .await,
        "2G: Flight 1 + dirty Flight 2 AutoMod PATCHes and same-slug load_mod_queue must all settle"
    );
    let srv_rules = srv.comm_truth.lock().unwrap().automod_rules.clone();
    let r1_srv = srv_rules.iter().find(|r| r.rule_id == "rule_1").unwrap();
    let r2_srv = srv_rules.iter().find(|r| r.rule_id == "rule_2").unwrap();
    assert!(
        !r1_srv.enabled && !r2_srv.enabled,
        "2G AUTOMOD-FILTER-DROP: both rule_1 and rule_2 must persist disabled on server (r1={}, r2={})",
        r1_srv.enabled,
        r2_srv.enabled
    );
    let _ = cw.read_with(&app, |w, _| {
        let ui_rules = &w.state.mod_community.as_ref().unwrap().automod_rules;
        let r1_ui = ui_rules.iter().find(|r| r.rule_id == "rule_1").unwrap();
        let r2_ui = ui_rules.iter().find(|r| r.rule_id == "rule_2").unwrap();
        assert!(
            !r1_ui.enabled && !r2_ui.enabled,
            "2G AUTOMOD-FILTER-DROP: UI mod_community.automod_rules must converge to both disabled"
        );
    });
    drop(rt_holder);
}

// ============================================================================
// Wave r6 Regressions: 2F_3C, 2F_3B, rollback_bookmark_flight, S2A-5a/5b, S2C-3b
// ============================================================================

struct AbortOnDrop(tokio::task::JoinHandle<()>);

impl Drop for AbortOnDrop {
    fn drop(&mut self) {
        self.0.abort();
    }
}

struct R6ListBookmarkServerState {
    lists: Arc<std::sync::Mutex<Vec<CommunityListDto>>>,
    post_list_delay_ms: Arc<std::sync::atomic::AtomicU64>,
    post_list_received: Arc<std::sync::atomic::AtomicUsize>,
    post_list_completed: Arc<std::sync::atomic::AtomicUsize>,
    lists_get_delay_ms: Arc<std::sync::atomic::AtomicU64>,
    lists_get_received: Arc<std::sync::atomic::AtomicUsize>,
    lists_get_completed: Arc<std::sync::atomic::AtomicUsize>,
    delete_list_completed: Arc<std::sync::atomic::AtomicUsize>,
    bookmark_fail_500: Arc<std::sync::atomic::AtomicBool>,
    bookmark_completed: Arc<std::sync::atomic::AtomicUsize>,
    bookmarked_folder: Arc<std::sync::Mutex<Option<String>>>,
}

fn mock_list_dto(list_id: &str, owner_id: &str, name: &str) -> CommunityListDto {
    CommunityListDto {
        list_id: list_id.to_string(),
        owner_actor_id: owner_id.to_string(),
        name: name.to_string(),
        description: format!("Desc for {name}"),
        kind: CommunityListKind::Mixed,
        is_private: false,
        is_pinned: false,
        member_actor_ids: Vec::new(),
        community_slugs: Vec::new(),
        created_at_ms: 1_750_000_000_000,
        updated_at_ms: 1_750_000_000_000,
    }
}

fn spawn_r6_list_bookmark_race_server(
    rt_handle: &tokio::runtime::Handle,
    owner_id: String,
    post_id: String,
) -> (std::net::SocketAddr, R6ListBookmarkServerState, AbortOnDrop) {
    use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    let initial_lists = vec![
        mock_list_dto("list_race_1", &owner_id, "Race List 1"),
        mock_list_dto("list_race_2", &owner_id, "Race List 2"),
        mock_list_dto("list_ed_2", &owner_id, "Editable Ghost List"),
    ];
    let state = R6ListBookmarkServerState {
        lists: Arc::new(std::sync::Mutex::new(initial_lists)),
        post_list_delay_ms: Arc::new(AtomicU64::new(0)),
        post_list_received: Arc::new(AtomicUsize::new(0)),
        post_list_completed: Arc::new(AtomicUsize::new(0)),
        lists_get_delay_ms: Arc::new(AtomicU64::new(0)),
        lists_get_received: Arc::new(AtomicUsize::new(0)),
        lists_get_completed: Arc::new(AtomicUsize::new(0)),
        delete_list_completed: Arc::new(AtomicUsize::new(0)),
        bookmark_fail_500: Arc::new(AtomicBool::new(false)),
        bookmark_completed: Arc::new(AtomicUsize::new(0)),
        bookmarked_folder: Arc::new(std::sync::Mutex::new(None)),
    };

    let lists = Arc::clone(&state.lists);
    let created_seq = Arc::new(AtomicUsize::new(0));
    let pl_delay = Arc::clone(&state.post_list_delay_ms);
    let pl_rx = Arc::clone(&state.post_list_received);
    let pl_done = Arc::clone(&state.post_list_completed);
    let lg_delay = Arc::clone(&state.lists_get_delay_ms);
    let lg_rx = Arc::clone(&state.lists_get_received);
    let lg_done = Arc::clone(&state.lists_get_completed);
    let del_done = Arc::clone(&state.delete_list_completed);
    let bm_fail = Arc::clone(&state.bookmark_fail_500);
    let bm_done = Arc::clone(&state.bookmark_completed);
    let bm_folder = Arc::clone(&state.bookmarked_folder);

    let std_listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind r6 list mock");
    std_listener.set_nonblocking(true).expect("nonblocking");
    let addr = std_listener.local_addr().expect("local_addr");

    let join = rt_handle.spawn(async move {
        let listener = tokio::net::TcpListener::from_std(std_listener).expect("from_std");
        let owner_actor = mock_actor_dto(&owner_id, ActorKind::Human, "r6_list_owner", "R6 Owner");

        while let Ok((mut sock, _)) = listener.accept().await {
            let owner_id = owner_id.clone();
            let post_id = post_id.clone();
            let owner_actor = owner_actor.clone();
            let lists = Arc::clone(&lists);
            let created_seq = Arc::clone(&created_seq);
            let pl_delay = Arc::clone(&pl_delay);
            let pl_rx = Arc::clone(&pl_rx);
            let pl_done = Arc::clone(&pl_done);
            let lg_delay = Arc::clone(&lg_delay);
            let lg_rx = Arc::clone(&lg_rx);
            let lg_done = Arc::clone(&lg_done);
            let del_done = Arc::clone(&del_done);
            let bm_fail = Arc::clone(&bm_fail);
            let bm_done = Arc::clone(&bm_done);
            let bm_folder = Arc::clone(&bm_folder);

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
                let (path, query) = full_target
                    .split_once('?')
                    .map(|(p, q)| (p.to_string(), q.to_string()))
                    .unwrap_or_else(|| (full_target.to_string(), String::new()));

                let mut content_len: usize = 0;
                for line in lines {
                    if let Some((k, v)) = line.split_once(':') {
                        if k.trim().eq_ignore_ascii_case("content-length") {
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
                let body_bytes = &buf[header_end..(header_end + content_len).min(buf.len())];
                let bookmark_path = format!("/api/v1/community/posts/{post_id}/bookmark");

                let (status, body) = if method == "GET" && path == "/api/v1/community/identities" {
                    let resp = CommunityIdentitiesResponse {
                        active_actor_id: owner_id.clone(),
                        human_actor: Some(owner_actor.clone()),
                        bot_actors: Vec::new(),
                        all_actors: vec![owner_actor.clone()],
                    };
                    ("200 OK", serde_json::to_string(&resp).unwrap())
                } else if method == "POST" && path == "/api/v1/community/identities/human" {
                    ("200 OK", serde_json::to_string(&owner_actor).unwrap())
                } else if method == "GET" && path == "/api/v1/community/lists" {
                    let snap = lists.lock().unwrap().clone();
                    let delay = lg_delay.load(Ordering::SeqCst);
                    if delay > 0 {
                        lg_rx.fetch_add(1, Ordering::SeqCst);
                        tokio::time::sleep(Duration::from_millis(delay)).await;
                        lg_done.fetch_add(1, Ordering::SeqCst);
                    }
                    ("200 OK", serde_json::to_string(&CommunityListsResponse { lists: snap }).unwrap())
                } else if method == "POST" && path == "/api/v1/community/lists" {
                    let req: CreateCommunityListRequest =
                        serde_json::from_slice(body_bytes).expect("parse create list req");
                    pl_rx.fetch_add(1, Ordering::SeqCst);
                    let delay = pl_delay.load(Ordering::SeqCst);
                    if delay > 0 {
                        tokio::time::sleep(Duration::from_millis(delay)).await;
                    }
                    let seq = created_seq.fetch_add(1, Ordering::SeqCst) + 1;
                    let created = CommunityListDto {
                        list_id: format!("list_created_{seq}"),
                        owner_actor_id: owner_id.clone(),
                        name: req.name,
                        description: req.description,
                        kind: req.kind,
                        is_private: req.is_private,
                        is_pinned: req.is_pinned,
                        member_actor_ids: req.member_actor_ids,
                        community_slugs: req.community_slugs,
                        created_at_ms: 1_750_000_000_100 + seq as i64,
                        updated_at_ms: 1_750_000_000_100 + seq as i64,
                    };
                    lists.lock().unwrap().insert(0, created.clone());
                    pl_done.fetch_add(1, Ordering::SeqCst);
                    ("200 OK", serde_json::to_string(&created).unwrap())
                } else if method == "PATCH" && path.starts_with("/api/v1/community/lists/") {
                    let lid = path.trim_start_matches("/api/v1/community/lists/");
                    let req: UpdateCommunityListRequest =
                        serde_json::from_slice(body_bytes).unwrap_or_default();
                    let mut guard = lists.lock().unwrap();
                    if let Some(l) = guard.iter_mut().find(|x| x.list_id == lid) {
                        if let Some(n) = req.name {
                            l.name = n;
                        }
                        if let Some(d) = req.description {
                            l.description = d;
                        }
                        if let Some(k) = req.kind {
                            l.kind = k;
                        }
                        if let Some(priv_) = req.is_private {
                            l.is_private = priv_;
                        }
                        if let Some(p) = req.is_pinned {
                            l.is_pinned = p;
                        }
                        if let Some(m) = req.member_actor_ids {
                            l.member_actor_ids = m;
                        }
                        if let Some(s) = req.community_slugs {
                            l.community_slugs = s;
                        }
                        ("200 OK", serde_json::to_string(l).unwrap())
                    } else {
                        ("404 Not Found", r#"{"error":"not_found"}"#.to_string())
                    }
                } else if method == "DELETE" && path.starts_with("/api/v1/community/lists/") {
                    let lid = path.trim_start_matches("/api/v1/community/lists/").to_string();
                    lists.lock().unwrap().retain(|x| x.list_id != lid);
                    del_done.fetch_add(1, Ordering::SeqCst);
                    (
                        "200 OK",
                        serde_json::to_string(&DeleteEntityResponse { id: lid, deleted: true })
                            .unwrap(),
                    )
                } else if (method == "POST" || method == "DELETE") && path == bookmark_path {
                    if bm_fail.load(Ordering::SeqCst) {
                        bm_done.fetch_add(1, Ordering::SeqCst);
                        ("500 Internal Server Error", r#"{"error":"injected_bookmark_500"}"#.to_string())
                    } else if method == "POST" {
                        let req: BookmarkRequest =
                            serde_json::from_slice(body_bytes).unwrap_or_default();
                        let folder = req
                            .folder
                            .as_deref()
                            .map(str::trim)
                            .filter(|s| !s.is_empty())
                            .unwrap_or("All")
                            .to_string();
                        *bm_folder.lock().unwrap() = Some(folder.clone());
                        bm_done.fetch_add(1, Ordering::SeqCst);
                        let resp = BookmarkResponse {
                            post_id: post_id.clone(),
                            bookmarked: true,
                            folder,
                            bookmarks_count: 1,
                        };
                        ("200 OK", serde_json::to_string(&resp).unwrap())
                    } else {
                        *bm_folder.lock().unwrap() = None;
                        bm_done.fetch_add(1, Ordering::SeqCst);
                        let resp = BookmarkResponse {
                            post_id: post_id.clone(),
                            bookmarked: false,
                            folder: String::new(),
                            bookmarks_count: 0,
                        };
                        ("200 OK", serde_json::to_string(&resp).unwrap())
                    }
                } else if method == "GET" && path == "/api/v1/community/feed" {
                    let cur_folder = bm_folder.lock().unwrap().clone();
                    let items = if query.contains("tab=saved") {
                        cur_folder
                            .as_deref()
                            .map(|f| {
                                vec![FeedItemDto {
                                    post: mock_post_dto(&post_id, &owner_id, true, Some(f), 1),
                                    explanation: None,
                                    rank_score: 1.0,
                                    is_exploration_slot: false,
                                }]
                            })
                            .unwrap_or_default()
                    } else if query.contains("tab=list") {
                        Vec::new()
                    } else {
                        vec![FeedItemDto {
                            post: mock_post_dto(
                                &post_id,
                                &owner_id,
                                cur_folder.is_some(),
                                cur_folder.as_deref(),
                                u64::from(cur_folder.is_some()),
                            ),
                            explanation: None,
                            rank_score: 1.0,
                            is_exploration_slot: false,
                        }]
                    };
                    (
                        "200 OK",
                        serde_json::to_string(&FeedResponseDto {
                            items,
                            next_cursor: None,
                            ..Default::default()
                        })
                        .unwrap(),
                    )
                } else if path == "/api/v1/community/communities" {
                    ("200 OK", serde_json::to_string(&ListCommunitiesResponse::default()).unwrap())
                } else if path == "/api/v1/community/notifications" {
                    ("200 OK", serde_json::to_string(&NotificationsListResponse::default()).unwrap())
                } else if path == "/api/v1/community/trending" {
                    ("200 OK", serde_json::to_string(&TrendingResponse::default()).unwrap())
                } else if path == "/api/v1/community/algo-prefs" {
                    ("200 OK", serde_json::to_string(&AlgoPreferencesDto::default()).unwrap())
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

    (addr, state, AbortOnDrop(join))
}

#[test]
fn test_m5_r6_online_list_editor_create_delete_and_bookmark_rollback_races() {
    run_in_headless_subprocess(
        "test_m5_r6_online_list_editor_create_delete_and_bookmark_rollback_races",
    );
}

async fn body_r6_online_list_editor_create_delete_and_bookmark_rollback_races(mut app: AsyncApp) {
    use std::sync::atomic::Ordering;

    let dir = TempDir::new().expect("r6 list tempdir");
    let rt_dir = TempDir::new().expect("r6 list rt tempdir");
    let rt_holder = TelegramClient::open_with_dir(
        ClientConfig {
            server_url: "http://127.0.0.1:9".to_string(),
            max_reconnect_attempts: 0,
            ..Default::default()
        },
        rt_dir.path(),
    )
    .expect("open rt holder");
    let owner_id = "u:906101".to_string();
    let post_id = "p_r6_bm_rollback".to_string();
    let (addr, srv, _server_guard) = spawn_r6_list_bookmark_race_server(
        rt_holder.runtime().handle(),
        owner_id.clone(),
        post_id.clone(),
    );

    let client = Arc::new(
        TelegramClient::open_with_dir(
            ClientConfig {
                server_url: format!("http://{addr}"),
                max_reconnect_attempts: 0,
                ..Default::default()
            },
            dir.path(),
        )
        .expect("open r6 list client"),
    );
    let auth = test_auth(906101, "r6_list_owner");
    client.set_auth_session(auth.clone());
    let client_win = Arc::clone(&client);
    let auth_win = auth.clone();
    let shell = app
        .update(|cx| {
            let _rt_guard = client_win.runtime().enter();
            cx.new(|cx| {
                let mut s = AppShell::new_with_client(client_win.clone(), cx);
                s.apply_authenticated_session(auth_win.clone(), cx);
                s
            })
        })
        .expect("mount r6 list AppShell");
    let _ = shell.update(&mut app, |s, cx| s.open_community_workspace(cx));
    let cw = shell
        .read_with(&app, |s, _| s.community_workspace.clone().unwrap())
        .unwrap();

    assert!(
        pump_until(&mut app, Duration::from_secs(5), |cx| {
            let st = &cw.read(cx).state;
            !st.feed_loading
                && st.identities.is_some()
                && st.lists.iter().any(|l| l.list_id == "list_ed_2")
                && st.feed_items.iter().any(|it| it.post.post_id == post_id)
        })
        .await,
        "R6 list host must bootstrap lists and feed post"
    );

    // ------------------------------------------------------------------
    // 1. 2F_3C: Create-list racing concurrent toggle_list_pinned AND older GET /lists
    // ------------------------------------------------------------------
    let _ = cw.update(&mut app, |w, cx| {
        w.navigate_to(CommunityRoute::BookmarksLists, cx);
        w.set_bookmarks_section(BookmarksListsSection::Lists, cx);
    });
    assert!(
        pump_until(&mut app, Duration::from_secs(4), |cx| !cw.read(cx).state.saved_loading).await
    );

    srv.post_list_delay_ms.store(180, Ordering::SeqCst);
    srv.lists_get_delay_ms.store(0, Ordering::SeqCst);
    let _ = cw.update(&mut app, |w, cx| {
        w.open_create_list_form(cx);
        w.state.list_editor.name = "R6 Created Racing Toggle".to_string();
        w.submit_list_editor(cx);
    });
    assert!(
        pump_until(&mut app, Duration::from_secs(4), |_| {
            srv.post_list_received.load(Ordering::SeqCst) >= 1
        })
        .await,
        "2F_3C: POST /lists must arrive and enter 180ms sleep before concurrent pin toggle"
    );
    let _ = cw.update(&mut app, |w, cx| {
        w.toggle_list_pinned("list_race_2".to_string(), true, cx);
    });
    assert!(
        pump_until(&mut app, Duration::from_secs(5), |cx| {
            let st = &cw.read(cx).state;
            !st.list_editor.submitting
                && srv.post_list_completed.load(Ordering::SeqCst) >= 1
                && st.list_membership_in_flight.is_empty()
                && st.list_membership_dirty.is_empty()
                && !st.list_feed_loading
        })
        .await,
        "2F_3C: create_list and concurrent pin toggle must both settle"
    );
    let _ = cw.read_with(&app, |w, _| {
        assert!(
            w.state.lists.iter().any(|l| l.list_id == "list_created_1"),
            "2F_3C: newly created list must remain in state.lists despite concurrent lists_gen bump"
        );
        assert_eq!(
            w.state.selected_list_id.as_deref(),
            Some("list_created_1"),
            "2F_3C: newly created list must be selected"
        );
        assert!(
            w.state
                .lists
                .iter()
                .find(|l| l.list_id == "list_race_2")
                .unwrap()
                .is_pinned,
            "2F_3C: concurrent pin toggle on list_race_2 must also persist"
        );
    });

    // Verify commit lists_gen bump also protects newly created list from an older delayed GET /lists!
    srv.lists_get_delay_ms.store(240, Ordering::SeqCst);
    srv.post_list_delay_ms.store(20, Ordering::SeqCst);
    let lg_rx_older = srv.lists_get_received.load(Ordering::SeqCst);
    let lg_done_older = srv.lists_get_completed.load(Ordering::SeqCst);
    let _ = cw.update(&mut app, |w, cx| w.load_saved_posts(cx));
    assert!(
        pump_until(&mut app, Duration::from_secs(4), |_| {
            srv.lists_get_received.load(Ordering::SeqCst) > lg_rx_older
        })
        .await,
        "2F_3C: older GET /lists must snapshot BEFORE list_created_2 exists"
    );
    srv.lists_get_delay_ms.store(0, Ordering::SeqCst);
    let _ = cw.update(&mut app, |w, cx| {
        w.open_create_list_form(cx);
        w.state.list_editor.name = "R6 Protected From Older Get".to_string();
        w.submit_list_editor(cx);
    });
    assert!(
        pump_until(&mut app, Duration::from_secs(4), |cx| {
            !cw.read(cx).state.list_editor.submitting
                && srv.post_list_completed.load(Ordering::SeqCst) >= 2
        })
        .await
    );
    assert!(
        pump_until(&mut app, Duration::from_secs(4), |cx| {
            srv.lists_get_completed.load(Ordering::SeqCst) > lg_done_older
                && !cw.read(cx).state.saved_loading
        })
        .await
    );
    let _ = cw.read_with(&app, |w, _| {
        assert!(
            w.state.lists.iter().any(|l| l.list_id == "list_created_2"),
            "2F_3C: older in-flight GET /lists must NOT drop list_created_2"
        );
    });

    // ------------------------------------------------------------------
    // 2. 2F_3B: Edit-list racing faster delete_list_action -> no ghost hijack or "List saved" toast
    // ------------------------------------------------------------------
    srv.post_list_delay_ms.store(0, Ordering::SeqCst);
    srv.lists_get_delay_ms.store(220, Ordering::SeqCst);
    let ed_list = cw
        .read_with(&app, |w, _| {
            w.state
                .lists
                .iter()
                .find(|l| l.list_id == "list_ed_2")
                .cloned()
                .expect("list_ed_2 present")
        })
        .unwrap();
    let lg_rx_2f3b = srv.lists_get_received.load(Ordering::SeqCst);
    let lg_done_2f3b = srv.lists_get_completed.load(Ordering::SeqCst);
    let del_before_2f3b = srv.delete_list_completed.load(Ordering::SeqCst);
    let _ = cw.update(&mut app, |w, cx| {
        w.select_list_for_preview("list_ed_2".to_string(), cx);
        w.open_edit_list_form(ed_list, cx);
        w.state.list_editor.name = "Ghost Renamed".to_string();
        w.submit_list_editor(cx);
    });
    assert!(
        pump_until(&mut app, Duration::from_secs(4), |_| {
            srv.lists_get_received.load(Ordering::SeqCst) > lg_rx_2f3b
        })
        .await,
        "2F_3B: submit_list_editor PATCH must finish and enter delayed GET /lists before delete"
    );
    srv.lists_get_delay_ms.store(0, Ordering::SeqCst);
    let _ = cw.update(&mut app, |w, cx| {
        w.delete_list_action("list_ed_2".to_string(), cx);
    });
    assert!(
        pump_until(&mut app, Duration::from_secs(4), |cx| {
            let st = &cw.read(cx).state;
            srv.delete_list_completed.load(Ordering::SeqCst) > del_before_2f3b
                && !st.list_deleting.contains("list_ed_2")
                && !st.lists.iter().any(|l| l.list_id == "list_ed_2")
        })
        .await,
        "2F_3B: fast delete_list_action must finish while submit_list_editor is still awaiting GET /lists"
    );
    let sel_after_delete = cw
        .read_with(&app, |w, _| w.state.selected_list_id.clone())
        .unwrap();
    assert!(
        sel_after_delete.is_some() && sel_after_delete.as_deref() != Some("list_ed_2"),
        "2F_3B: delete must select a live fallback list, got {sel_after_delete:?}"
    );
    let _ = shell.update(&mut app, |s, _| {
        s.window_runtime.controller.current_toast = None;
        s.window_runtime.controller.toast_queue.clear();
    });
    let _ = cw.update(&mut app, |w, _| {
        let _ = w.take_pending_actions();
    });

    assert!(
        pump_until(&mut app, Duration::from_secs(4), |cx| {
            srv.lists_get_completed.load(Ordering::SeqCst) > lg_done_2f3b
                && !cw.read(cx).state.list_editor.submitting
        })
        .await,
        "2F_3B: delayed submit_list_editor must settle"
    );
    app.background_executor().timer(Duration::from_millis(35)).await;
    let _ = cw.read_with(&app, |w, _| {
        assert!(
            !w.state.lists.iter().any(|l| l.list_id == "list_ed_2"),
            "2F_3B: deleted list must NEVER be resurrected into state.lists"
        );
        assert_eq!(
            w.state.selected_list_id, sel_after_delete,
            "2F_3B: selected_list_id must stay on live fallback list and NOT hijack to deleted list_ed_2"
        );
        assert!(!w.state.list_editor_open, "2F_3B: list editor must close cleanly");
    });
    let _ = shell.read_with(&app, |s, _| {
        let ctrl = &s.window_runtime.controller;
        let has_saved_toast = ctrl
            .current_toast
            .iter()
            .chain(ctrl.toast_queue.iter())
            .any(|t| t.text.contains("List saved") || t.text.contains("列表已保存"));
        assert!(
            !has_saved_toast,
            "2F_3B: delayed edit of already-deleted list must emit ZERO 'List saved' toast"
        );
    });

    // ------------------------------------------------------------------
    // 3. Fix 3: rollback_bookmark_flight keeps bookmark_cached_items until after snapshot_feed_item_for_post
    // ------------------------------------------------------------------
    srv.bookmark_fail_500.store(false, Ordering::SeqCst);
    let _ = cw.update(&mut app, |w, cx| {
        w.move_bookmark_to_folder(post_id.clone(), "CachedFolder".to_string(), cx);
    });
    assert!(
        pump_until(&mut app, Duration::from_secs(4), |cx| {
            let st = &cw.read(cx).state;
            st.bookmark_in_flight.is_empty()
                && st.saved_items.iter().any(|it| it.post.post_id == post_id)
        })
        .await
    );
    let bm_done_before = srv.bookmark_completed.load(Ordering::SeqCst);
    srv.bookmark_fail_500.store(true, Ordering::SeqCst);
    let _ = cw.update(&mut app, |w, cx| {
        // Simulate post rotating out of feed_items while bookmark_cached_items holds the cached snapshot.
        let cached = w
            .state
            .saved_items
            .iter()
            .find(|it| it.post.post_id == post_id)
            .cloned()
            .expect("saved item exists");
        w.state.feed_items.retain(|it| it.post.post_id != post_id);
        w.state
            .bookmark_cached_items
            .insert(post_id.clone(), cached);
        w.remove_bookmark_from_saved(post_id.clone(), cx);
        if let Some(snap) = w.state.bookmark_last_synced.get_mut(&post_id) {
            snap.saved_item = None;
        }
    });
    assert!(
        pump_until(&mut app, Duration::from_secs(4), |cx| {
            srv.bookmark_completed.load(Ordering::SeqCst) > bm_done_before
                && cw.read(cx).state.bookmark_in_flight.is_empty()
        })
        .await
    );
    let _ = cw.read_with(&app, |w, _| {
        assert!(
            w.state.saved_items.iter().any(|it| {
                it.post.post_id == post_id
                    && it.post.viewer_bookmarked
                    && it.post.viewer_bookmark_folder.as_deref() == Some("CachedFolder")
            }),
            "Fix 3: rollback_bookmark_flight must restore saved_items via bookmark_cached_items fallback"
        );
        assert!(
            !w.state.bookmark_cached_items.contains_key(&post_id),
            "Fix 3: bookmark_cached_items must be cleaned up after rollback"
        );
    });
    drop(rt_holder);
}

struct R6SingleFlightServerState {
    muted: Arc<std::sync::atomic::AtomicBool>,
    mute_set_delay_ms: Arc<std::sync::atomic::AtomicU64>,
    mute_fail_500: Arc<std::sync::atomic::AtomicBool>,
    mute_in_flight: Arc<std::sync::atomic::AtomicUsize>,
    mute_in_flight_max: Arc<std::sync::atomic::AtomicUsize>,
    mute_set_completed: Arc<std::sync::atomic::AtomicUsize>,
    mute_remove_completed: Arc<std::sync::atomic::AtomicUsize>,

    blocked: Arc<std::sync::atomic::AtomicBool>,
    following: Arc<std::sync::atomic::AtomicBool>,
    block_set_delay_ms: Arc<std::sync::atomic::AtomicU64>,
    block_fail_500: Arc<std::sync::atomic::AtomicBool>,
    block_in_flight: Arc<std::sync::atomic::AtomicUsize>,
    block_in_flight_max: Arc<std::sync::atomic::AtomicUsize>,
    block_set_completed: Arc<std::sync::atomic::AtomicUsize>,
    block_remove_completed: Arc<std::sync::atomic::AtomicUsize>,

    post_liked: Arc<std::sync::atomic::AtomicBool>,
    post_likes_count: Arc<std::sync::atomic::AtomicU64>,
    post_like_delay_ms: Arc<std::sync::atomic::AtomicU64>,
    post_like_fail_500: Arc<std::sync::atomic::AtomicBool>,
    post_like_in_flight: Arc<std::sync::atomic::AtomicUsize>,
    post_like_in_flight_max: Arc<std::sync::atomic::AtomicUsize>,
    post_like_completed: Arc<std::sync::atomic::AtomicUsize>,

    comment_vote_dir: Arc<std::sync::atomic::AtomicI8>,
    comment_vote_first_delay_ms: Arc<std::sync::atomic::AtomicU64>,
    comment_vote_received: Arc<std::sync::atomic::AtomicUsize>,
    comment_vote_completed: Arc<std::sync::atomic::AtomicUsize>,

    comment_liked: Arc<std::sync::atomic::AtomicBool>,
    comment_likes_count: Arc<std::sync::atomic::AtomicU64>,
    comment_like_delay_ms: Arc<std::sync::atomic::AtomicU64>,
    comment_like_in_flight: Arc<std::sync::atomic::AtomicUsize>,
    comment_like_in_flight_max: Arc<std::sync::atomic::AtomicUsize>,
    comment_like_completed: Arc<std::sync::atomic::AtomicUsize>,
}

fn mock_comment_dto(comment_id: &str, post_id: &str, author_id: &str) -> CommunityCommentDto {
    CommunityCommentDto {
        comment_id: comment_id.to_string(),
        post_id: post_id.to_string(),
        parent_comment_id: None,
        depth: 0,
        path: format!("0001_{comment_id}"),
        author_actor_id: author_id.to_string(),
        author_handle: "race_bot".to_string(),
        author_display_name: "Race Bot".to_string(),
        author_kind: ActorKind::Bot,
        author_verified_tier: VerifiedTier::None,
        author_bot_badge: Some("AI".to_string()),
        author_avatar_shape: "Hex".to_string(),
        author_avatar_color: "#4A9EFF".to_string(),
        is_op: false,
        body: "Comment body for Wave r6 single-flight race test".to_string(),
        media: Vec::new(),
        upvotes: 0,
        downvotes: 0,
        score: 0,
        likes_count: 0,
        awards: Vec::new(),
        viewer_vote: 0,
        viewer_liked: false,
        is_stickied: false,
        is_mod_distinguished: false,
        is_removed: false,
        is_deleted: false,
        children: Vec::new(),
        more_children_count: 0,
        created_at_ms: 1_750_000_000_000,
        updated_at_ms: 1_750_000_000_000,
    }
}

fn spawn_r6_single_flight_server(
    rt_handle: &tokio::runtime::Handle,
    owner_id: String,
    bot_id: String,
    post_id: String,
    comment_id: String,
) -> (std::net::SocketAddr, R6SingleFlightServerState, AbortOnDrop) {
    use std::sync::atomic::{AtomicBool, AtomicI8, AtomicU64, AtomicUsize, Ordering};
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    let state = R6SingleFlightServerState {
        muted: Arc::new(AtomicBool::new(false)),
        mute_set_delay_ms: Arc::new(AtomicU64::new(0)),
        mute_fail_500: Arc::new(AtomicBool::new(false)),
        mute_in_flight: Arc::new(AtomicUsize::new(0)),
        mute_in_flight_max: Arc::new(AtomicUsize::new(0)),
        mute_set_completed: Arc::new(AtomicUsize::new(0)),
        mute_remove_completed: Arc::new(AtomicUsize::new(0)),

        blocked: Arc::new(AtomicBool::new(false)),
        following: Arc::new(AtomicBool::new(true)),
        block_set_delay_ms: Arc::new(AtomicU64::new(0)),
        block_fail_500: Arc::new(AtomicBool::new(false)),
        block_in_flight: Arc::new(AtomicUsize::new(0)),
        block_in_flight_max: Arc::new(AtomicUsize::new(0)),
        block_set_completed: Arc::new(AtomicUsize::new(0)),
        block_remove_completed: Arc::new(AtomicUsize::new(0)),

        post_liked: Arc::new(AtomicBool::new(false)),
        post_likes_count: Arc::new(AtomicU64::new(0)),
        post_like_delay_ms: Arc::new(AtomicU64::new(0)),
        post_like_fail_500: Arc::new(AtomicBool::new(false)),
        post_like_in_flight: Arc::new(AtomicUsize::new(0)),
        post_like_in_flight_max: Arc::new(AtomicUsize::new(0)),
        post_like_completed: Arc::new(AtomicUsize::new(0)),

        comment_vote_dir: Arc::new(AtomicI8::new(0)),
        comment_vote_first_delay_ms: Arc::new(AtomicU64::new(0)),
        comment_vote_received: Arc::new(AtomicUsize::new(0)),
        comment_vote_completed: Arc::new(AtomicUsize::new(0)),

        comment_liked: Arc::new(AtomicBool::new(false)),
        comment_likes_count: Arc::new(AtomicU64::new(0)),
        comment_like_delay_ms: Arc::new(AtomicU64::new(0)),
        comment_like_in_flight: Arc::new(AtomicUsize::new(0)),
        comment_like_in_flight_max: Arc::new(AtomicUsize::new(0)),
        comment_like_completed: Arc::new(AtomicUsize::new(0)),
    };

    let muted = Arc::clone(&state.muted);
    let mute_delay = Arc::clone(&state.mute_set_delay_ms);
    let mute_fail = Arc::clone(&state.mute_fail_500);
    let mute_if = Arc::clone(&state.mute_in_flight);
    let mute_if_max = Arc::clone(&state.mute_in_flight_max);
    let mute_set_done = Arc::clone(&state.mute_set_completed);
    let mute_rem_done = Arc::clone(&state.mute_remove_completed);

    let blocked = Arc::clone(&state.blocked);
    let following = Arc::clone(&state.following);
    let block_delay = Arc::clone(&state.block_set_delay_ms);
    let block_fail = Arc::clone(&state.block_fail_500);
    let block_if = Arc::clone(&state.block_in_flight);
    let block_if_max = Arc::clone(&state.block_in_flight_max);
    let block_set_done = Arc::clone(&state.block_set_completed);
    let block_rem_done = Arc::clone(&state.block_remove_completed);

    let pliked = Arc::clone(&state.post_liked);
    let plikes_cnt = Arc::clone(&state.post_likes_count);
    let plike_delay = Arc::clone(&state.post_like_delay_ms);
    let plike_fail = Arc::clone(&state.post_like_fail_500);
    let plike_if = Arc::clone(&state.post_like_in_flight);
    let plike_if_max = Arc::clone(&state.post_like_in_flight_max);
    let plike_done = Arc::clone(&state.post_like_completed);

    let cvote_dir = Arc::clone(&state.comment_vote_dir);
    let cvote_delay = Arc::clone(&state.comment_vote_first_delay_ms);
    let cvote_rx = Arc::clone(&state.comment_vote_received);
    let cvote_done = Arc::clone(&state.comment_vote_completed);

    let cliked = Arc::clone(&state.comment_liked);
    let clikes_cnt = Arc::clone(&state.comment_likes_count);
    let clike_delay = Arc::clone(&state.comment_like_delay_ms);
    let clike_if = Arc::clone(&state.comment_like_in_flight);
    let clike_if_max = Arc::clone(&state.comment_like_in_flight_max);
    let clike_done = Arc::clone(&state.comment_like_completed);

    let std_listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind r6 sf mock");
    std_listener.set_nonblocking(true).expect("nonblocking");
    let addr = std_listener.local_addr().expect("local_addr");

    let join = rt_handle.spawn(async move {
        let listener = tokio::net::TcpListener::from_std(std_listener).expect("from_std");
        let owner_actor = mock_actor_dto(&owner_id, ActorKind::Human, "r6_sf_owner", "R6 SF Owner");

        while let Ok((mut sock, _)) = listener.accept().await {
            let owner_id = owner_id.clone();
            let bot_id = bot_id.clone();
            let post_id = post_id.clone();
            let comment_id = comment_id.clone();
            let owner_actor = owner_actor.clone();

            let muted = Arc::clone(&muted);
            let mute_delay = Arc::clone(&mute_delay);
            let mute_fail = Arc::clone(&mute_fail);
            let mute_if = Arc::clone(&mute_if);
            let mute_if_max = Arc::clone(&mute_if_max);
            let mute_set_done = Arc::clone(&mute_set_done);
            let mute_rem_done = Arc::clone(&mute_rem_done);

            let blocked = Arc::clone(&blocked);
            let following = Arc::clone(&following);
            let block_delay = Arc::clone(&block_delay);
            let block_fail = Arc::clone(&block_fail);
            let block_if = Arc::clone(&block_if);
            let block_if_max = Arc::clone(&block_if_max);
            let block_set_done = Arc::clone(&block_set_done);
            let block_rem_done = Arc::clone(&block_rem_done);

            let pliked = Arc::clone(&pliked);
            let plikes_cnt = Arc::clone(&plikes_cnt);
            let plike_delay = Arc::clone(&plike_delay);
            let plike_fail = Arc::clone(&plike_fail);
            let plike_if = Arc::clone(&plike_if);
            let plike_if_max = Arc::clone(&plike_if_max);
            let plike_done = Arc::clone(&plike_done);

            let cvote_dir = Arc::clone(&cvote_dir);
            let cvote_delay = Arc::clone(&cvote_delay);
            let cvote_rx = Arc::clone(&cvote_rx);
            let cvote_done = Arc::clone(&cvote_done);

            let cliked = Arc::clone(&cliked);
            let clikes_cnt = Arc::clone(&clikes_cnt);
            let clike_delay = Arc::clone(&clike_delay);
            let clike_if = Arc::clone(&clike_if);
            let clike_if_max = Arc::clone(&clike_if_max);
            let clike_done = Arc::clone(&clike_done);

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
                let (path, query) = full_target
                    .split_once('?')
                    .map(|(p, q)| (p.to_string(), q.to_string()))
                    .unwrap_or_else(|| (full_target.to_string(), String::new()));

                let mut content_len: usize = 0;
                for line in lines {
                    if let Some((k, v)) = line.split_once(':') {
                        if k.trim().eq_ignore_ascii_case("content-length") {
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
                let body_bytes = &buf[header_end..(header_end + content_len).min(buf.len())];

                let post_detail_path = format!("/api/v1/community/posts/{post_id}");
                let post_like_path = format!("/api/v1/community/posts/{post_id}/like");
                let comment_vote_path = format!("/api/v1/community/comments/{comment_id}/vote");
                let comment_like_path = format!("/api/v1/community/comments/{comment_id}/like");

                let build_bot = || {
                    let mut b = mock_actor_dto(&bot_id, ActorKind::Bot, "race_bot", "Race Bot");
                    b.is_muted = muted.load(Ordering::SeqCst);
                    b.is_blocked = blocked.load(Ordering::SeqCst);
                    b.is_following = following.load(Ordering::SeqCst);
                    b
                };

                let (status, body) = if method == "GET" && path == "/api/v1/community/identities" {
                    let resp = CommunityIdentitiesResponse {
                        active_actor_id: owner_id.clone(),
                        human_actor: Some(owner_actor.clone()),
                        bot_actors: Vec::new(),
                        all_actors: vec![owner_actor.clone()],
                    };
                    ("200 OK", serde_json::to_string(&resp).unwrap())
                } else if method == "POST" && path == "/api/v1/community/identities/human" {
                    ("200 OK", serde_json::to_string(&owner_actor).unwrap())
                } else if method == "GET" && path == "/api/v1/community/actors/race_bot" {
                    let resp = ActorProfileResponse {
                        actor: build_bot(),
                        recent_posts: Vec::new(),
                        recent_comments: Vec::new(),
                        moderated_communities: Vec::new(),
                    };
                    ("200 OK", serde_json::to_string(&resp).unwrap())
                } else if (method == "POST" || method == "DELETE")
                    && path == "/api/v1/community/mutes-blocks"
                {
                    let req_opt: Option<MuteBlockRequest> = serde_json::from_slice(body_bytes).ok();
                    let is_block = req_opt
                        .as_ref()
                        .map(|r| r.action == MuteBlockActionKind::Block)
                        .unwrap_or_else(|| query.contains("action=block"));
                    if is_block {
                        let cur = block_if.fetch_add(1, Ordering::SeqCst) + 1;
                        block_if_max.fetch_max(cur, Ordering::SeqCst);
                        let res = if method == "POST" {
                            let delay = block_delay.swap(0, Ordering::SeqCst);
                            if delay > 0 {
                                tokio::time::sleep(Duration::from_millis(delay)).await;
                            }
                            if block_fail.load(Ordering::SeqCst) {
                                block_set_done.fetch_add(1, Ordering::SeqCst);
                                ("500 Internal Server Error", r#"{"error":"block_500"}"#.to_string())
                            } else {
                                blocked.store(true, Ordering::SeqCst);
                                following.store(false, Ordering::SeqCst);
                                block_set_done.fetch_add(1, Ordering::SeqCst);
                                let dto = CommunityMuteBlockDto {
                                    id: "mb_blk_1".to_string(),
                                    owner_actor_id: owner_id.clone(),
                                    target_kind: MuteBlockTargetKind::Actor,
                                    target_value: bot_id.clone(),
                                    action: MuteBlockActionKind::Block,
                                    reason: None,
                                    created_at_ms: 1_750_000_000_000,
                                };
                                ("200 OK", serde_json::to_string(&dto).unwrap())
                            }
                        } else {
                            blocked.store(false, Ordering::SeqCst);
                            block_rem_done.fetch_add(1, Ordering::SeqCst);
                            (
                                "200 OK",
                                serde_json::to_string(&DeleteEntityResponse {
                                    id: bot_id.clone(),
                                    deleted: true,
                                })
                                .unwrap(),
                            )
                        };
                        block_if.fetch_sub(1, Ordering::SeqCst);
                        res
                    } else {
                        let cur = mute_if.fetch_add(1, Ordering::SeqCst) + 1;
                        mute_if_max.fetch_max(cur, Ordering::SeqCst);
                        let res = if method == "POST" {
                            let delay = mute_delay.swap(0, Ordering::SeqCst);
                            if delay > 0 {
                                tokio::time::sleep(Duration::from_millis(delay)).await;
                            }
                            if mute_fail.load(Ordering::SeqCst) {
                                mute_set_done.fetch_add(1, Ordering::SeqCst);
                                ("500 Internal Server Error", r#"{"error":"mute_500"}"#.to_string())
                            } else {
                                muted.store(true, Ordering::SeqCst);
                                mute_set_done.fetch_add(1, Ordering::SeqCst);
                                let dto = CommunityMuteBlockDto {
                                    id: "mb_mute_1".to_string(),
                                    owner_actor_id: owner_id.clone(),
                                    target_kind: MuteBlockTargetKind::Actor,
                                    target_value: bot_id.clone(),
                                    action: MuteBlockActionKind::Mute,
                                    reason: None,
                                    created_at_ms: 1_750_000_000_000,
                                };
                                ("200 OK", serde_json::to_string(&dto).unwrap())
                            }
                        } else {
                            muted.store(false, Ordering::SeqCst);
                            mute_rem_done.fetch_add(1, Ordering::SeqCst);
                            (
                                "200 OK",
                                serde_json::to_string(&DeleteEntityResponse {
                                    id: bot_id.clone(),
                                    deleted: true,
                                })
                                .unwrap(),
                            )
                        };
                        mute_if.fetch_sub(1, Ordering::SeqCst);
                        res
                    }
                } else if method == "GET" && path == post_detail_path {
                    let mut post = mock_post_dto(&post_id, &owner_id, false, None, 0);
                    post.viewer_liked = pliked.load(Ordering::SeqCst);
                    post.likes_count = plikes_cnt.load(Ordering::SeqCst);
                    let mut c = mock_comment_dto(&comment_id, &post_id, &bot_id);
                    c.viewer_vote = cvote_dir.load(Ordering::SeqCst);
                    c.score = i64::from(c.viewer_vote);
                    c.upvotes = i64::from(c.viewer_vote == 1);
                    c.downvotes = i64::from(c.viewer_vote == -1);
                    c.viewer_liked = cliked.load(Ordering::SeqCst);
                    c.likes_count = clikes_cnt.load(Ordering::SeqCst);
                    let resp = PostDetailResponse {
                        post,
                        thread_chain: Vec::new(),
                        quote_chain: Vec::new(),
                        comments: vec![c],
                        total_comments: 1,
                    };
                    ("200 OK", serde_json::to_string(&resp).unwrap())
                } else if method == "POST" && path == post_like_path {
                    let cur = plike_if.fetch_add(1, Ordering::SeqCst) + 1;
                    plike_if_max.fetch_max(cur, Ordering::SeqCst);
                    let delay = plike_delay.swap(0, Ordering::SeqCst);
                    if delay > 0 {
                        tokio::time::sleep(Duration::from_millis(delay)).await;
                    }
                    let res = if plike_fail.load(Ordering::SeqCst) {
                        plike_done.fetch_add(1, Ordering::SeqCst);
                        ("500 Internal Server Error", r#"{"error":"like_500"}"#.to_string())
                    } else {
                        let was = pliked.load(Ordering::SeqCst);
                        let now = !was;
                        pliked.store(now, Ordering::SeqCst);
                        let cnt = if now { 1 } else { 0 };
                        plikes_cnt.store(cnt, Ordering::SeqCst);
                        plike_done.fetch_add(1, Ordering::SeqCst);
                        let resp = ToggleActionResponse {
                            target_id: post_id.clone(),
                            active: now,
                            count: cnt,
                        };
                        ("200 OK", serde_json::to_string(&resp).unwrap())
                    };
                    plike_if.fetch_sub(1, Ordering::SeqCst);
                    res
                } else if method == "POST" && path == comment_vote_path {
                    let req: VoteRequest = serde_json::from_slice(body_bytes).expect("parse vote");
                    cvote_dir.store(req.direction, Ordering::SeqCst);
                    let seq = cvote_rx.fetch_add(1, Ordering::SeqCst);
                    let resp = VoteResponse {
                        target_id: comment_id.clone(),
                        direction: req.direction,
                        upvotes: i64::from(req.direction == 1),
                        downvotes: i64::from(req.direction == -1),
                        score: i64::from(req.direction),
                    };
                    if seq == 0 {
                        let delay = cvote_delay.load(Ordering::SeqCst);
                        if delay > 0 {
                            tokio::time::sleep(Duration::from_millis(delay)).await;
                        }
                    }
                    cvote_done.fetch_add(1, Ordering::SeqCst);
                    ("200 OK", serde_json::to_string(&resp).unwrap())
                } else if method == "POST" && path == comment_like_path {
                    let cur = clike_if.fetch_add(1, Ordering::SeqCst) + 1;
                    clike_if_max.fetch_max(cur, Ordering::SeqCst);
                    let delay = clike_delay.swap(0, Ordering::SeqCst);
                    if delay > 0 {
                        tokio::time::sleep(Duration::from_millis(delay)).await;
                    }
                    let was = cliked.load(Ordering::SeqCst);
                    let now = !was;
                    cliked.store(now, Ordering::SeqCst);
                    let cnt = if now { 1 } else { 0 };
                    clikes_cnt.store(cnt, Ordering::SeqCst);
                    clike_done.fetch_add(1, Ordering::SeqCst);
                    clike_if.fetch_sub(1, Ordering::SeqCst);
                    let resp = ToggleActionResponse {
                        target_id: comment_id.clone(),
                        active: now,
                        count: cnt,
                    };
                    ("200 OK", serde_json::to_string(&resp).unwrap())
                } else if method == "GET" && path == "/api/v1/community/feed" {
                    let mut post = mock_post_dto(&post_id, &owner_id, false, None, 0);
                    post.viewer_liked = pliked.load(Ordering::SeqCst);
                    post.likes_count = plikes_cnt.load(Ordering::SeqCst);
                    let resp = FeedResponseDto {
                        items: vec![FeedItemDto {
                            post,
                            explanation: None,
                            rank_score: 1.0,
                            is_exploration_slot: false,
                        }],
                        next_cursor: None,
                        ..Default::default()
                    };
                    ("200 OK", serde_json::to_string(&resp).unwrap())
                } else if path == "/api/v1/community/communities" {
                    ("200 OK", serde_json::to_string(&ListCommunitiesResponse::default()).unwrap())
                } else if path == "/api/v1/community/lists" {
                    ("200 OK", serde_json::to_string(&CommunityListsResponse::default()).unwrap())
                } else if path == "/api/v1/community/notifications" {
                    ("200 OK", serde_json::to_string(&NotificationsListResponse::default()).unwrap())
                } else if path == "/api/v1/community/trending" {
                    ("200 OK", serde_json::to_string(&TrendingResponse::default()).unwrap())
                } else if path == "/api/v1/community/algo-prefs" {
                    ("200 OK", serde_json::to_string(&AlgoPreferencesDto::default()).unwrap())
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

    (addr, state, AbortOnDrop(join))
}

#[test]
fn test_m5_r6_online_mute_block_like_and_comment_single_flight_races() {
    run_in_headless_subprocess(
        "test_m5_r6_online_mute_block_like_and_comment_single_flight_races",
    );
}

async fn body_r6_online_mute_block_like_and_comment_single_flight_races(mut app: AsyncApp) {
    use std::sync::atomic::Ordering;

    let dir = TempDir::new().expect("r6 sf tempdir");
    let rt_dir = TempDir::new().expect("r6 sf rt tempdir");
    let rt_holder = TelegramClient::open_with_dir(
        ClientConfig {
            server_url: "http://127.0.0.1:9".to_string(),
            max_reconnect_attempts: 0,
            ..Default::default()
        },
        rt_dir.path(),
    )
    .expect("open rt holder");
    let owner_id = "u:906202".to_string();
    let bot_id = "b:906202".to_string();
    let post_id = "p_r6_sf_like".to_string();
    let comment_id = "c_r6_sf_1".to_string();
    let (addr, srv, _server_guard) = spawn_r6_single_flight_server(
        rt_holder.runtime().handle(),
        owner_id.clone(),
        bot_id.clone(),
        post_id.clone(),
        comment_id.clone(),
    );

    let client = Arc::new(
        TelegramClient::open_with_dir(
            ClientConfig {
                server_url: format!("http://{addr}"),
                max_reconnect_attempts: 0,
                ..Default::default()
            },
            dir.path(),
        )
        .expect("open r6 sf client"),
    );
    let auth = test_auth(906202, "r6_sf_owner");
    client.set_auth_session(auth.clone());
    let client_win = Arc::clone(&client);
    let auth_win = auth.clone();
    let shell = app
        .update(|cx| {
            let _rt_guard = client_win.runtime().enter();
            cx.new(|cx| {
                let mut s = AppShell::new_with_client(client_win.clone(), cx);
                s.apply_authenticated_session(auth_win.clone(), cx);
                s
            })
        })
        .expect("mount r6 sf AppShell");
    let _ = shell.update(&mut app, |s, cx| s.open_community_workspace(cx));
    let cw = shell
        .read_with(&app, |s, _| s.community_workspace.clone().unwrap())
        .unwrap();

    assert!(
        pump_until(&mut app, Duration::from_secs(5), |cx| {
            let st = &cw.read(cx).state;
            !st.feed_loading
                && st.identities.is_some()
                && st.feed_items.iter().any(|it| it.post.post_id == post_id)
        })
        .await,
        "R6 single-flight host must bootstrap feed"
    );

    // ------------------------------------------------------------------
    // 1. S2A-5a: toggle_mute_actor rapid Mute(delayed 180ms) -> Unmute(fast) + error rollback recovery
    // ------------------------------------------------------------------
    let _ = cw.update(&mut app, |w, cx| {
        w.open_actor_profile("race_bot".to_string(), cx);
    });
    assert!(
        pump_until(&mut app, Duration::from_secs(4), |cx| {
            let st = &cw.read(cx).state;
            !st.profile_loading
                && st
                    .actor_profile
                    .as_ref()
                    .is_some_and(|p| p.actor.actor_id == bot_id && p.actor.is_following)
        })
        .await
    );

    srv.mute_set_delay_ms.store(180, Ordering::SeqCst);
    let _ = cw.update(&mut app, |w, cx| {
        w.toggle_mute_actor(bot_id.clone(), false, cx);
        w.toggle_mute_actor(bot_id.clone(), true, cx);
        assert!(w.state.actor_mute_in_flight.contains_key(&bot_id));
        assert!(w.state.actor_mute_dirty.contains(&bot_id));
    });
    assert!(
        pump_until(&mut app, Duration::from_secs(5), |cx| {
            let st = &cw.read(cx).state;
            st.actor_mute_in_flight.is_empty()
                && st.actor_mute_dirty.is_empty()
                && srv.mute_set_completed.load(Ordering::SeqCst) == 1
                && srv.mute_remove_completed.load(Ordering::SeqCst) == 1
        })
        .await,
        "S2A-5a: Mute(delayed) + Unmute(dirty) must settle sequentially"
    );
    assert_eq!(
        srv.mute_in_flight_max.load(Ordering::SeqCst),
        1,
        "S2A-5a: single-flight must NEVER send concurrent mute/unmute RPCs"
    );
    let _ = cw.read_with(&app, |w, _| {
        let ui_muted = w.state.actor_profile.as_ref().unwrap().actor.is_muted;
        assert!(
            !ui_muted && !srv.muted.load(Ordering::SeqCst),
            "S2A-5a: UI and server must both converge to is_muted=false"
        );
    });

    // Mute HTTP 500 error rollback + clean subsequent recovery
    srv.mute_fail_500.store(true, Ordering::SeqCst);
    let mute_set_before = srv.mute_set_completed.load(Ordering::SeqCst);
    let _ = cw.update(&mut app, |w, cx| {
        w.toggle_mute_actor(bot_id.clone(), false, cx);
    });
    assert!(
        pump_until(&mut app, Duration::from_secs(4), |cx| {
            srv.mute_set_completed.load(Ordering::SeqCst) > mute_set_before
                && cw.read(cx).state.actor_mute_in_flight.is_empty()
        })
        .await
    );
    let _ = cw.read_with(&app, |w, _| {
        assert!(
            !w.state.actor_profile.as_ref().unwrap().actor.is_muted,
            "S2A-5a: HTTP 500 on mute must roll back UI to is_muted=false"
        );
    });
    srv.mute_fail_500.store(false, Ordering::SeqCst);
    let mute_rec_before = srv.mute_set_completed.load(Ordering::SeqCst);
    let _ = cw.update(&mut app, |w, cx| {
        w.toggle_mute_actor(bot_id.clone(), false, cx);
    });
    assert!(
        pump_until(&mut app, Duration::from_secs(4), |cx| {
            srv.mute_set_completed.load(Ordering::SeqCst) > mute_rec_before
                && cw.read(cx).state.actor_mute_in_flight.is_empty()
        })
        .await
    );
    let _ = cw.read_with(&app, |w, _| {
        assert!(
            w.state.actor_profile.as_ref().unwrap().actor.is_muted
                && srv.muted.load(Ordering::SeqCst),
            "S2A-5a: recovery after HTTP 500 must cleanly mute actor"
        );
    });

    // ------------------------------------------------------------------
    // 2. S2A-5b: toggle_block_actor HTTP 500 rollback (restoring is_following=true!)
    //    + rapid Block(delayed 180ms) -> Unblock(fast)
    // ------------------------------------------------------------------
    srv.block_fail_500.store(true, Ordering::SeqCst);
    let blk_err_before = srv.block_set_completed.load(Ordering::SeqCst);
    let _ = cw.update(&mut app, |w, cx| {
        assert!(w.state.actor_profile.as_ref().unwrap().actor.is_following);
        w.toggle_block_actor(bot_id.clone(), false, cx);
        // Optimistic block clears is_following immediately
        let a = &w.state.actor_profile.as_ref().unwrap().actor;
        assert!(a.is_blocked && !a.is_following);
    });
    assert!(
        pump_until(&mut app, Duration::from_secs(4), |cx| {
            srv.block_set_completed.load(Ordering::SeqCst) > blk_err_before
                && cw.read(cx).state.actor_block_in_flight.is_empty()
        })
        .await
    );
    let _ = cw.read_with(&app, |w, _| {
        let a = &w.state.actor_profile.as_ref().unwrap().actor;
        assert!(
            !a.is_blocked && a.is_following,
            "S2A-5b: HTTP 500 rollback on block must restore BOTH is_blocked=false AND is_following=true"
        );
    });

    srv.block_fail_500.store(false, Ordering::SeqCst);
    srv.block_set_delay_ms.store(180, Ordering::SeqCst);
    let blk_set_before = srv.block_set_completed.load(Ordering::SeqCst);
    let blk_rem_before = srv.block_remove_completed.load(Ordering::SeqCst);
    let _ = cw.update(&mut app, |w, cx| {
        w.toggle_block_actor(bot_id.clone(), false, cx);
        w.toggle_block_actor(bot_id.clone(), true, cx);
        assert!(w.state.actor_block_in_flight.contains_key(&bot_id));
        assert!(w.state.actor_block_dirty.contains(&bot_id));
    });
    assert!(
        pump_until(&mut app, Duration::from_secs(5), |cx| {
            let st = &cw.read(cx).state;
            st.actor_block_in_flight.is_empty()
                && st.actor_block_dirty.is_empty()
                && srv.block_set_completed.load(Ordering::SeqCst) > blk_set_before
                && srv.block_remove_completed.load(Ordering::SeqCst) > blk_rem_before
        })
        .await,
        "S2A-5b: Block(delayed) + Unblock(dirty) must settle sequentially"
    );
    assert_eq!(
        srv.block_in_flight_max.load(Ordering::SeqCst),
        1,
        "S2A-5b: single-flight must NEVER send concurrent block/unblock RPCs"
    );
    let _ = cw.read_with(&app, |w, _| {
        let ui_blocked = w.state.actor_profile.as_ref().unwrap().actor.is_blocked;
        assert!(
            !ui_blocked && !srv.blocked.load(Ordering::SeqCst),
            "S2A-5b: UI and server must both converge to is_blocked=false"
        );
    });

    // ------------------------------------------------------------------
    // 3. S2C-3b: toggle_like rapid Like(delayed 180ms)->Unlike(fast), 3-click parity,
    //    error rollback, plus vote_comment_action & toggle_comment_like_action races
    // ------------------------------------------------------------------
    let _ = cw.update(&mut app, |w, cx| {
        w.open_post_detail(post_id.clone(), cx);
    });
    assert!(
        pump_until(&mut app, Duration::from_secs(4), |cx| {
            let st = &cw.read(cx).state;
            !st.detail_loading
                && st
                    .detail_post
                    .as_ref()
                    .is_some_and(|d| d.post.post_id == post_id && d.comments.len() == 1)
        })
        .await
    );

    // 3a. Rapid 2-click Like(delayed 180ms) -> Unlike(fast)
    srv.post_like_delay_ms.store(180, Ordering::SeqCst);
    let _ = cw.update(&mut app, |w, cx| {
        w.toggle_like(post_id.clone(), cx);
        w.toggle_like(post_id.clone(), cx);
        assert!(w.state.post_like_in_flight.contains_key(&post_id));
        assert!(w.state.post_like_pending_toggle.contains(&post_id));
    });
    assert!(
        pump_until(&mut app, Duration::from_secs(5), |cx| {
            let st = &cw.read(cx).state;
            st.post_like_in_flight.is_empty()
                && st.post_like_pending_toggle.is_empty()
                && srv.post_like_completed.load(Ordering::SeqCst) == 2
        })
        .await,
        "S2C-3b: rapid 2-click toggle_like must serialize into 2 sequential RPCs"
    );
    assert_eq!(
        srv.post_like_in_flight_max.load(Ordering::SeqCst),
        1,
        "S2C-3b: single-flight toggle_like must NEVER send concurrent like RPCs"
    );
    let _ = cw.read_with(&app, |w, _| {
        let p = &w.state.detail_post.as_ref().unwrap().post;
        assert!(
            !p.viewer_liked && p.likes_count == 0 && !srv.post_liked.load(Ordering::SeqCst),
            "S2C-3b: rapid Like->Unlike must converge UI == server == (false, 0)"
        );
    });

    // 3b. Rapid 3-click parity (Like -> Unlike -> Like): even pending toggles cancel out (ONLY 1 RPC!)
    srv.post_like_delay_ms.store(140, Ordering::SeqCst);
    let likes_before_3 = srv.post_like_completed.load(Ordering::SeqCst);
    let _ = cw.update(&mut app, |w, cx| {
        w.toggle_like(post_id.clone(), cx);
        w.toggle_like(post_id.clone(), cx);
        w.toggle_like(post_id.clone(), cx);
        assert!(!w.state.post_like_pending_toggle.contains(&post_id));
    });
    assert!(
        pump_until(&mut app, Duration::from_secs(4), |cx| {
            cw.read(cx).state.post_like_in_flight.is_empty()
                && srv.post_like_completed.load(Ordering::SeqCst) == likes_before_3 + 1
        })
        .await
    );
    let _ = cw.read_with(&app, |w, _| {
        let p = &w.state.detail_post.as_ref().unwrap().post;
        assert!(
            p.viewer_liked && p.likes_count == 1 && srv.post_liked.load(Ordering::SeqCst),
            "S2C-3b: 3-click parity must converge to (true, 1) with 1 RPC"
        );
    });

    // 3c. Post like HTTP 500 error rollback -> restores (true, 1)
    srv.post_like_fail_500.store(true, Ordering::SeqCst);
    let likes_before_err = srv.post_like_completed.load(Ordering::SeqCst);
    let _ = cw.update(&mut app, |w, cx| w.toggle_like(post_id.clone(), cx));
    assert!(
        pump_until(&mut app, Duration::from_secs(4), |cx| {
            cw.read(cx).state.post_like_in_flight.is_empty()
                && srv.post_like_completed.load(Ordering::SeqCst) > likes_before_err
        })
        .await
    );
    let _ = cw.read_with(&app, |w, _| {
        let p = &w.state.detail_post.as_ref().unwrap().post;
        assert!(
            p.viewer_liked && p.likes_count == 1,
            "S2C-3b: HTTP 500 on toggle_like must roll back to last_synced (true, 1)"
        );
    });

    // 3d. Comment vote rapid +1(delayed 180ms) -> -1(fast): single-flight serializes +1 then -1!
    srv.comment_vote_first_delay_ms.store(180, Ordering::SeqCst);
    let _ = cw.update(&mut app, |w, cx| {
        w.vote_comment_action(comment_id.clone(), 1, cx);
    });
    assert!(
        pump_until(&mut app, Duration::from_secs(4), |_| {
            srv.comment_vote_received.load(Ordering::SeqCst) >= 1
        })
        .await,
        "S2C-3b: first +1 comment vote must hit server before rapid -1 flip"
    );
    let _ = cw.update(&mut app, |w, cx| {
        w.vote_comment_action(comment_id.clone(), -1, cx);
    });
    assert!(
        pump_until(&mut app, Duration::from_secs(5), |cx| {
            let st = &cw.read(cx).state;
            st.comment_vote_in_flight.is_empty()
                && st.comment_vote_dirty.is_empty()
                && srv.comment_vote_completed.load(Ordering::SeqCst) >= 2
        })
        .await,
        "S2C-3b: both comment votes (including delayed +1) must complete"
    );
    app.background_executor().timer(Duration::from_millis(35)).await;
    let _ = cw.read_with(&app, |w, _| {
        let c = w.state.find_comment(&comment_id).expect("comment present");
        assert_eq!(
            c.viewer_vote, -1,
            "S2C-3b: single-flight comment vote must converge UI to -1"
        );
        assert_eq!(c.score, -1);
        assert_eq!(srv.comment_vote_dir.load(Ordering::SeqCst), -1);
    });

    // 3e. Comment like rapid Like(delayed 180ms) -> Unlike(fast) single-flight parity
    srv.comment_like_delay_ms.store(180, Ordering::SeqCst);
    let _ = cw.update(&mut app, |w, cx| {
        w.toggle_comment_like_action(comment_id.clone(), cx);
        w.toggle_comment_like_action(comment_id.clone(), cx);
        assert!(w.state.comment_like_in_flight.contains_key(&comment_id));
        assert!(w.state.comment_like_pending_toggle.contains(&comment_id));
    });
    assert!(
        pump_until(&mut app, Duration::from_secs(5), |cx| {
            let st = &cw.read(cx).state;
            st.comment_like_in_flight.is_empty()
                && st.comment_like_pending_toggle.is_empty()
                && srv.comment_like_completed.load(Ordering::SeqCst) == 2
        })
        .await,
        "S2C-3b: rapid comment like->unlike must serialize into 2 sequential RPCs"
    );
    assert_eq!(
        srv.comment_like_in_flight_max.load(Ordering::SeqCst),
        1,
        "S2C-3b: single-flight toggle_comment_like_action must NEVER send concurrent RPCs"
    );
    let _ = cw.read_with(&app, |w, _| {
        let c = w.state.find_comment(&comment_id).expect("comment present");
        assert!(
            !c.viewer_liked && c.likes_count == 0 && !srv.comment_liked.load(Ordering::SeqCst),
            "S2C-3b: rapid comment Like->Unlike must converge UI == server == (false, 0)"
        );
    });
    drop(rt_holder);
}

struct R7ConsistencyServerState {
    // Fix 1: Block->Unblock vs load_actor_profile
    blocked: Arc<std::sync::atomic::AtomicBool>,
    following: Arc<std::sync::atomic::AtomicBool>,
    profile_get_delay_ms: Arc<std::sync::atomic::AtomicU64>,
    profile_get_received: Arc<std::sync::atomic::AtomicUsize>,
    profile_get_completed: Arc<std::sync::atomic::AtomicUsize>,
    block_set_delay_ms: Arc<std::sync::atomic::AtomicU64>,
    block_in_flight: Arc<std::sync::atomic::AtomicUsize>,
    block_in_flight_max: Arc<std::sync::atomic::AtomicUsize>,
    block_set_completed: Arc<std::sync::atomic::AtomicUsize>,
    block_remove_completed: Arc<std::sync::atomic::AtomicUsize>,

    // Fix 2 & Fix 3: PostDetail Case B + Post/Comment single-flight votes + Comment like/edit/award
    detail_get_next_delay_ms: Arc<std::sync::atomic::AtomicU64>,
    detail_get_received: Arc<std::sync::atomic::AtomicUsize>,
    detail_get_completed: Arc<std::sync::atomic::AtomicUsize>,

    post_vote_dir: Arc<std::sync::atomic::AtomicI8>,
    post_vote_delay_ms: Arc<std::sync::atomic::AtomicU64>,
    post_vote_fail_500: Arc<std::sync::atomic::AtomicBool>,
    post_vote_in_flight: Arc<std::sync::atomic::AtomicUsize>,
    post_vote_in_flight_max: Arc<std::sync::atomic::AtomicUsize>,
    post_vote_received: Arc<std::sync::atomic::AtomicUsize>,
    post_vote_completed: Arc<std::sync::atomic::AtomicUsize>,

    comment_vote_dir: Arc<std::sync::atomic::AtomicI8>,
    comment_vote_delay_ms: Arc<std::sync::atomic::AtomicU64>,
    comment_vote_fail_500: Arc<std::sync::atomic::AtomicBool>,
    comment_vote_in_flight: Arc<std::sync::atomic::AtomicUsize>,
    comment_vote_in_flight_max: Arc<std::sync::atomic::AtomicUsize>,
    comment_vote_received: Arc<std::sync::atomic::AtomicUsize>,
    comment_vote_completed: Arc<std::sync::atomic::AtomicUsize>,

    comment_liked: Arc<std::sync::atomic::AtomicBool>,
    comment_likes_count: Arc<std::sync::atomic::AtomicU64>,
    comment_like_completed: Arc<std::sync::atomic::AtomicUsize>,
    comment_body: Arc<std::sync::Mutex<String>>,
    comment_awards_count: Arc<std::sync::atomic::AtomicU32>,

    // Fix 4 & Fix 5: 2F_3D & 2F_3F list editor races
    lists: Arc<std::sync::Mutex<Vec<CommunityListDto>>>,
    lists_get_delay_ms: Arc<std::sync::atomic::AtomicU64>,
    lists_get_received: Arc<std::sync::atomic::AtomicUsize>,
    lists_get_completed: Arc<std::sync::atomic::AtomicUsize>,
    list_toggle_patch_delay_ms: Arc<std::sync::atomic::AtomicU64>,
    list_editor_patch_fail_500: Arc<std::sync::atomic::AtomicBool>,
    list_editor_patch_completed: Arc<std::sync::atomic::AtomicUsize>,
    list_toggle_patch_completed: Arc<std::sync::atomic::AtomicUsize>,
}

fn spawn_r7_consistency_server(
    rt_handle: &tokio::runtime::Handle,
    owner_id: String,
    bot_id: String,
    post_id: String,
    comment_id: String,
) -> (std::net::SocketAddr, R7ConsistencyServerState, AbortOnDrop) {
    use gpui_telegram_proto::community_contracts::{
        AwardResponse, GiveAwardRequest, PostAwardDto, UpdateCommentRequest,
    };
    use std::sync::atomic::{AtomicBool, AtomicI8, AtomicU32, AtomicU64, AtomicUsize, Ordering};
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    let seed_lists = vec![
        CommunityListDto {
            list_id: "list_ed_toggle".to_string(),
            owner_actor_id: owner_id.clone(),
            name: "Original Name".to_string(),
            description: "2F_3D target".to_string(),
            kind: CommunityListKind::Mixed,
            is_private: false,
            is_pinned: false,
            member_actor_ids: Vec::new(),
            community_slugs: Vec::new(),
            created_at_ms: 1_750_000_000_001,
            updated_at_ms: 1_750_000_000_001,
        },
        CommunityListDto {
            list_id: "list_ed_500".to_string(),
            owner_actor_id: owner_id.clone(),
            name: "Editor 500 List".to_string(),
            description: "2F_3F target".to_string(),
            kind: CommunityListKind::Mixed,
            is_private: false,
            is_pinned: false,
            member_actor_ids: Vec::new(),
            community_slugs: Vec::new(),
            created_at_ms: 1_750_000_000_002,
            updated_at_ms: 1_750_000_000_002,
        },
    ];

    let state = R7ConsistencyServerState {
        blocked: Arc::new(AtomicBool::new(false)),
        following: Arc::new(AtomicBool::new(true)),
        profile_get_delay_ms: Arc::new(AtomicU64::new(0)),
        profile_get_received: Arc::new(AtomicUsize::new(0)),
        profile_get_completed: Arc::new(AtomicUsize::new(0)),
        block_set_delay_ms: Arc::new(AtomicU64::new(0)),
        block_in_flight: Arc::new(AtomicUsize::new(0)),
        block_in_flight_max: Arc::new(AtomicUsize::new(0)),
        block_set_completed: Arc::new(AtomicUsize::new(0)),
        block_remove_completed: Arc::new(AtomicUsize::new(0)),

        detail_get_next_delay_ms: Arc::new(AtomicU64::new(0)),
        detail_get_received: Arc::new(AtomicUsize::new(0)),
        detail_get_completed: Arc::new(AtomicUsize::new(0)),

        post_vote_dir: Arc::new(AtomicI8::new(0)),
        post_vote_delay_ms: Arc::new(AtomicU64::new(0)),
        post_vote_fail_500: Arc::new(AtomicBool::new(false)),
        post_vote_in_flight: Arc::new(AtomicUsize::new(0)),
        post_vote_in_flight_max: Arc::new(AtomicUsize::new(0)),
        post_vote_received: Arc::new(AtomicUsize::new(0)),
        post_vote_completed: Arc::new(AtomicUsize::new(0)),

        comment_vote_dir: Arc::new(AtomicI8::new(0)),
        comment_vote_delay_ms: Arc::new(AtomicU64::new(0)),
        comment_vote_fail_500: Arc::new(AtomicBool::new(false)),
        comment_vote_in_flight: Arc::new(AtomicUsize::new(0)),
        comment_vote_in_flight_max: Arc::new(AtomicUsize::new(0)),
        comment_vote_received: Arc::new(AtomicUsize::new(0)),
        comment_vote_completed: Arc::new(AtomicUsize::new(0)),

        comment_liked: Arc::new(AtomicBool::new(false)),
        comment_likes_count: Arc::new(AtomicU64::new(0)),
        comment_like_completed: Arc::new(AtomicUsize::new(0)),
        comment_body: Arc::new(std::sync::Mutex::new(
            "Initial r7 comment body".to_string(),
        )),
        comment_awards_count: Arc::new(AtomicU32::new(0)),

        lists: Arc::new(std::sync::Mutex::new(seed_lists)),
        lists_get_delay_ms: Arc::new(AtomicU64::new(0)),
        lists_get_received: Arc::new(AtomicUsize::new(0)),
        lists_get_completed: Arc::new(AtomicUsize::new(0)),
        list_toggle_patch_delay_ms: Arc::new(AtomicU64::new(0)),
        list_editor_patch_fail_500: Arc::new(AtomicBool::new(false)),
        list_editor_patch_completed: Arc::new(AtomicUsize::new(0)),
        list_toggle_patch_completed: Arc::new(AtomicUsize::new(0)),
    };

    let blocked = Arc::clone(&state.blocked);
    let following = Arc::clone(&state.following);
    let prof_delay = Arc::clone(&state.profile_get_delay_ms);
    let prof_rx = Arc::clone(&state.profile_get_received);
    let prof_done = Arc::clone(&state.profile_get_completed);
    let blk_delay = Arc::clone(&state.block_set_delay_ms);
    let blk_if = Arc::clone(&state.block_in_flight);
    let blk_if_max = Arc::clone(&state.block_in_flight_max);
    let blk_set_done = Arc::clone(&state.block_set_completed);
    let blk_rem_done = Arc::clone(&state.block_remove_completed);

    let det_delay = Arc::clone(&state.detail_get_next_delay_ms);
    let det_rx = Arc::clone(&state.detail_get_received);
    let det_done = Arc::clone(&state.detail_get_completed);

    let pvote_dir = Arc::clone(&state.post_vote_dir);
    let pvote_delay = Arc::clone(&state.post_vote_delay_ms);
    let pvote_fail = Arc::clone(&state.post_vote_fail_500);
    let pvote_if = Arc::clone(&state.post_vote_in_flight);
    let pvote_if_max = Arc::clone(&state.post_vote_in_flight_max);
    let pvote_rx = Arc::clone(&state.post_vote_received);
    let pvote_done = Arc::clone(&state.post_vote_completed);

    let cvote_dir = Arc::clone(&state.comment_vote_dir);
    let cvote_delay = Arc::clone(&state.comment_vote_delay_ms);
    let cvote_fail = Arc::clone(&state.comment_vote_fail_500);
    let cvote_if = Arc::clone(&state.comment_vote_in_flight);
    let cvote_if_max = Arc::clone(&state.comment_vote_in_flight_max);
    let cvote_rx = Arc::clone(&state.comment_vote_received);
    let cvote_done = Arc::clone(&state.comment_vote_completed);

    let cliked = Arc::clone(&state.comment_liked);
    let clikes_cnt = Arc::clone(&state.comment_likes_count);
    let clike_done = Arc::clone(&state.comment_like_completed);
    let cbody = Arc::clone(&state.comment_body);
    let cawards = Arc::clone(&state.comment_awards_count);

    let lists = Arc::clone(&state.lists);
    let lg_delay = Arc::clone(&state.lists_get_delay_ms);
    let lg_rx = Arc::clone(&state.lists_get_received);
    let lg_done = Arc::clone(&state.lists_get_completed);
    let lt_delay = Arc::clone(&state.list_toggle_patch_delay_ms);
    let le_fail = Arc::clone(&state.list_editor_patch_fail_500);
    let le_done = Arc::clone(&state.list_editor_patch_completed);
    let lt_done = Arc::clone(&state.list_toggle_patch_completed);

    let std_listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind r7 mock");
    std_listener.set_nonblocking(true).expect("nonblocking");
    let addr = std_listener.local_addr().expect("local_addr");

    let join = rt_handle.spawn(async move {
        let listener = tokio::net::TcpListener::from_std(std_listener).expect("from_std");
        let owner_actor = mock_actor_dto(&owner_id, ActorKind::Human, "r7_owner", "R7 Owner");

        while let Ok((mut sock, _)) = listener.accept().await {
            let owner_id = owner_id.clone();
            let bot_id = bot_id.clone();
            let post_id = post_id.clone();
            let comment_id = comment_id.clone();
            let owner_actor = owner_actor.clone();

            let blocked = Arc::clone(&blocked);
            let following = Arc::clone(&following);
            let prof_delay = Arc::clone(&prof_delay);
            let prof_rx = Arc::clone(&prof_rx);
            let prof_done = Arc::clone(&prof_done);
            let blk_delay = Arc::clone(&blk_delay);
            let blk_if = Arc::clone(&blk_if);
            let blk_if_max = Arc::clone(&blk_if_max);
            let blk_set_done = Arc::clone(&blk_set_done);
            let blk_rem_done = Arc::clone(&blk_rem_done);

            let det_delay = Arc::clone(&det_delay);
            let det_rx = Arc::clone(&det_rx);
            let det_done = Arc::clone(&det_done);

            let pvote_dir = Arc::clone(&pvote_dir);
            let pvote_delay = Arc::clone(&pvote_delay);
            let pvote_fail = Arc::clone(&pvote_fail);
            let pvote_if = Arc::clone(&pvote_if);
            let pvote_if_max = Arc::clone(&pvote_if_max);
            let pvote_rx = Arc::clone(&pvote_rx);
            let pvote_done = Arc::clone(&pvote_done);

            let cvote_dir = Arc::clone(&cvote_dir);
            let cvote_delay = Arc::clone(&cvote_delay);
            let cvote_fail = Arc::clone(&cvote_fail);
            let cvote_if = Arc::clone(&cvote_if);
            let cvote_if_max = Arc::clone(&cvote_if_max);
            let cvote_rx = Arc::clone(&cvote_rx);
            let cvote_done = Arc::clone(&cvote_done);

            let cliked = Arc::clone(&cliked);
            let clikes_cnt = Arc::clone(&clikes_cnt);
            let clike_done = Arc::clone(&clike_done);
            let cbody = Arc::clone(&cbody);
            let cawards = Arc::clone(&cawards);

            let lists = Arc::clone(&lists);
            let lg_delay = Arc::clone(&lg_delay);
            let lg_rx = Arc::clone(&lg_rx);
            let lg_done = Arc::clone(&lg_done);
            let lt_delay = Arc::clone(&lt_delay);
            let le_fail = Arc::clone(&le_fail);
            let le_done = Arc::clone(&le_done);
            let lt_done = Arc::clone(&lt_done);

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
                let (path, _query) = full_target
                    .split_once('?')
                    .map(|(p, q)| (p.to_string(), q.to_string()))
                    .unwrap_or_else(|| (full_target.to_string(), String::new()));

                let mut content_len: usize = 0;
                for line in lines {
                    if let Some((k, v)) = line.split_once(':') {
                        if k.trim().eq_ignore_ascii_case("content-length") {
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
                let body_bytes = &buf[header_end..(header_end + content_len).min(buf.len())];

                let post_detail_path = format!("/api/v1/community/posts/{post_id}");
                let post_vote_path = format!("/api/v1/community/posts/{post_id}/vote");
                let comment_path = format!("/api/v1/community/comments/{comment_id}");
                let comment_vote_path = format!("/api/v1/community/comments/{comment_id}/vote");
                let comment_like_path = format!("/api/v1/community/comments/{comment_id}/like");
                let comment_award_path = format!("/api/v1/community/comments/{comment_id}/award");

                let build_post = || {
                    let mut p = mock_post_dto(&post_id, &owner_id, false, None, 0);
                    let pv = pvote_dir.load(Ordering::SeqCst);
                    p.viewer_vote = pv;
                    p.upvotes = i64::from(pv == 1);
                    p.downvotes = i64::from(pv == -1);
                    p.score = i64::from(pv);
                    p
                };
                let build_comment = || {
                    let mut c = mock_comment_dto(&comment_id, &post_id, &owner_id);
                    let cv = cvote_dir.load(Ordering::SeqCst);
                    c.viewer_vote = cv;
                    c.upvotes = i64::from(cv == 1);
                    c.downvotes = i64::from(cv == -1);
                    c.score = i64::from(cv);
                    c.viewer_liked = cliked.load(Ordering::SeqCst);
                    c.likes_count = clikes_cnt.load(Ordering::SeqCst);
                    c.body = cbody.lock().unwrap().clone();
                    let aw_n = cawards.load(Ordering::SeqCst);
                    if aw_n > 0 {
                        c.awards = vec![PostAwardDto {
                            award_id: format!("aw_r7_{aw_n}"),
                            award_kind: "gold".to_string(),
                            icon_emoji: "🏆".to_string(),
                            giver_actor_id: owner_id.clone(),
                            giver_handle: "r7_owner".to_string(),
                            message: None,
                            created_at_ms: 1_750_000_001_000,
                        }];
                    }
                    c
                };

                let (status, body) = if method == "GET" && path == "/api/v1/community/identities" {
                    let resp = CommunityIdentitiesResponse {
                        active_actor_id: owner_id.clone(),
                        human_actor: Some(owner_actor.clone()),
                        bot_actors: Vec::new(),
                        all_actors: vec![owner_actor.clone()],
                    };
                    ("200 OK", serde_json::to_string(&resp).unwrap())
                } else if method == "POST" && path == "/api/v1/community/identities/human" {
                    ("200 OK", serde_json::to_string(&owner_actor).unwrap())
                } else if method == "GET" && path == "/api/v1/community/actors/r7_bot" {
                    let mut actor = mock_actor_dto(&bot_id, ActorKind::Bot, "r7_bot", "R7 Bot");
                    actor.is_blocked = blocked.load(Ordering::SeqCst);
                    actor.is_following = following.load(Ordering::SeqCst);
                    prof_rx.fetch_add(1, Ordering::SeqCst);
                    let delay = prof_delay.swap(0, Ordering::SeqCst);
                    if delay > 0 {
                        tokio::time::sleep(Duration::from_millis(delay)).await;
                    }
                    prof_done.fetch_add(1, Ordering::SeqCst);
                    let resp = ActorProfileResponse {
                        actor,
                        recent_posts: Vec::new(),
                        recent_comments: Vec::new(),
                        moderated_communities: Vec::new(),
                    };
                    ("200 OK", serde_json::to_string(&resp).unwrap())
                } else if (method == "POST" || method == "DELETE")
                    && path == "/api/v1/community/mutes-blocks"
                {
                    let cur = blk_if.fetch_add(1, Ordering::SeqCst) + 1;
                    blk_if_max.fetch_max(cur, Ordering::SeqCst);
                    let res = if method == "POST" {
                        let delay = blk_delay.swap(0, Ordering::SeqCst);
                        if delay > 0 {
                            tokio::time::sleep(Duration::from_millis(delay)).await;
                        }
                        blocked.store(true, Ordering::SeqCst);
                        following.store(false, Ordering::SeqCst);
                        blk_set_done.fetch_add(1, Ordering::SeqCst);
                        let dto = CommunityMuteBlockDto {
                            id: "mb_r7_blk".to_string(),
                            owner_actor_id: owner_id.clone(),
                            target_kind: MuteBlockTargetKind::Actor,
                            target_value: bot_id.clone(),
                            action: MuteBlockActionKind::Block,
                            reason: None,
                            created_at_ms: 1_750_000_000_000,
                        };
                        ("200 OK", serde_json::to_string(&dto).unwrap())
                    } else {
                        blocked.store(false, Ordering::SeqCst);
                        blk_rem_done.fetch_add(1, Ordering::SeqCst);
                        (
                            "200 OK",
                            serde_json::to_string(&DeleteEntityResponse {
                                id: bot_id.clone(),
                                deleted: true,
                            })
                            .unwrap(),
                        )
                    };
                    blk_if.fetch_sub(1, Ordering::SeqCst);
                    res
                } else if method == "GET" && path == post_detail_path {
                    // Snapshot BEFORE optional delay so Case B slow GET returns stale pre-mutation state!
                    let resp = PostDetailResponse {
                        post: build_post(),
                        thread_chain: Vec::new(),
                        quote_chain: Vec::new(),
                        comments: vec![build_comment()],
                        total_comments: 1,
                    };
                    let delay = det_delay.swap(0, Ordering::SeqCst);
                    det_rx.fetch_add(1, Ordering::SeqCst);
                    if delay > 0 {
                        tokio::time::sleep(Duration::from_millis(delay)).await;
                    }
                    det_done.fetch_add(1, Ordering::SeqCst);
                    ("200 OK", serde_json::to_string(&resp).unwrap())
                } else if method == "POST" && path == post_vote_path {
                    let req: VoteRequest =
                        serde_json::from_slice(body_bytes).expect("parse post vote");
                    let cur = pvote_if.fetch_add(1, Ordering::SeqCst) + 1;
                    pvote_if_max.fetch_max(cur, Ordering::SeqCst);
                    let delay = pvote_delay.swap(0, Ordering::SeqCst);
                    pvote_rx.fetch_add(1, Ordering::SeqCst);
                    // Pre-commit delay simulates network/handler arrival reordering!
                    if delay > 0 {
                        tokio::time::sleep(Duration::from_millis(delay)).await;
                    }
                    let res = if pvote_fail.load(Ordering::SeqCst) {
                        pvote_done.fetch_add(1, Ordering::SeqCst);
                        (
                            "500 Internal Server Error",
                            r#"{"error":"post_vote_500"}"#.to_string(),
                        )
                    } else {
                        pvote_dir.store(req.direction, Ordering::SeqCst);
                        pvote_done.fetch_add(1, Ordering::SeqCst);
                        let resp = VoteResponse {
                            target_id: post_id.clone(),
                            direction: req.direction,
                            upvotes: i64::from(req.direction == 1),
                            downvotes: i64::from(req.direction == -1),
                            score: i64::from(req.direction),
                        };
                        ("200 OK", serde_json::to_string(&resp).unwrap())
                    };
                    pvote_if.fetch_sub(1, Ordering::SeqCst);
                    res
                } else if method == "POST" && path == comment_vote_path {
                    let req: VoteRequest =
                        serde_json::from_slice(body_bytes).expect("parse comment vote");
                    let cur = cvote_if.fetch_add(1, Ordering::SeqCst) + 1;
                    cvote_if_max.fetch_max(cur, Ordering::SeqCst);
                    let delay = cvote_delay.swap(0, Ordering::SeqCst);
                    cvote_rx.fetch_add(1, Ordering::SeqCst);
                    // Pre-commit delay simulates network/handler arrival reordering!
                    if delay > 0 {
                        tokio::time::sleep(Duration::from_millis(delay)).await;
                    }
                    let res = if cvote_fail.load(Ordering::SeqCst) {
                        cvote_done.fetch_add(1, Ordering::SeqCst);
                        (
                            "500 Internal Server Error",
                            r#"{"error":"comment_vote_500"}"#.to_string(),
                        )
                    } else {
                        cvote_dir.store(req.direction, Ordering::SeqCst);
                        cvote_done.fetch_add(1, Ordering::SeqCst);
                        let resp = VoteResponse {
                            target_id: comment_id.clone(),
                            direction: req.direction,
                            upvotes: i64::from(req.direction == 1),
                            downvotes: i64::from(req.direction == -1),
                            score: i64::from(req.direction),
                        };
                        ("200 OK", serde_json::to_string(&resp).unwrap())
                    };
                    cvote_if.fetch_sub(1, Ordering::SeqCst);
                    res
                } else if method == "POST" && path == comment_like_path {
                    let now = !cliked.load(Ordering::SeqCst);
                    cliked.store(now, Ordering::SeqCst);
                    let cnt = u64::from(now);
                    clikes_cnt.store(cnt, Ordering::SeqCst);
                    clike_done.fetch_add(1, Ordering::SeqCst);
                    let resp = ToggleActionResponse {
                        target_id: comment_id.clone(),
                        active: now,
                        count: cnt,
                    };
                    ("200 OK", serde_json::to_string(&resp).unwrap())
                } else if method == "PATCH" && path == comment_path {
                    let req: UpdateCommentRequest =
                        serde_json::from_slice(body_bytes).expect("parse comment edit");
                    *cbody.lock().unwrap() = req.body;
                    let mut c = build_comment();
                    c.updated_at_ms = 1_750_000_001_000;
                    ("200 OK", serde_json::to_string(&c).unwrap())
                } else if method == "POST" && path == comment_award_path {
                    let req: GiveAwardRequest =
                        serde_json::from_slice(body_bytes).expect("parse comment award");
                    let n = cawards.fetch_add(1, Ordering::SeqCst) + 1;
                    let resp = AwardResponse {
                        target_id: comment_id.clone(),
                        awards: vec![PostAwardDto {
                            award_id: format!("aw_r7_{n}"),
                            award_kind: req.award_kind,
                            icon_emoji: "🏆".to_string(),
                            giver_actor_id: owner_id.clone(),
                            giver_handle: "r7_owner".to_string(),
                            message: None,
                            created_at_ms: 1_750_000_001_000,
                        }],
                    };
                    ("200 OK", serde_json::to_string(&resp).unwrap())
                } else if method == "GET" && path == "/api/v1/community/lists" {
                    let snap = lists.lock().unwrap().clone();
                    let delay = lg_delay.swap(0, Ordering::SeqCst);
                    lg_rx.fetch_add(1, Ordering::SeqCst);
                    if delay > 0 {
                        tokio::time::sleep(Duration::from_millis(delay)).await;
                    }
                    lg_done.fetch_add(1, Ordering::SeqCst);
                    (
                        "200 OK",
                        serde_json::to_string(&CommunityListsResponse { lists: snap }).unwrap(),
                    )
                } else if method == "PATCH" && path.starts_with("/api/v1/community/lists/") {
                    let lid = path.trim_start_matches("/api/v1/community/lists/").to_string();
                    let req: UpdateCommunityListRequest =
                        serde_json::from_slice(body_bytes).unwrap_or_default();
                    let is_editor_patch = req.name.is_some();
                    if is_editor_patch && le_fail.load(Ordering::SeqCst) {
                        le_done.fetch_add(1, Ordering::SeqCst);
                        (
                            "500 Internal Server Error",
                            r#"{"error":"list_editor_500"}"#.to_string(),
                        )
                    } else {
                        if !is_editor_patch {
                            let delay = lt_delay.swap(0, Ordering::SeqCst);
                            if delay > 0 {
                                tokio::time::sleep(Duration::from_millis(delay)).await;
                            }
                        }
                        let mut guard = lists.lock().unwrap();
                        if let Some(l) = guard.iter_mut().find(|x| x.list_id == lid) {
                            if let Some(n) = req.name {
                                l.name = n;
                            }
                            if let Some(d) = req.description {
                                l.description = d;
                            }
                            if let Some(k) = req.kind {
                                l.kind = k;
                            }
                            if let Some(priv_) = req.is_private {
                                l.is_private = priv_;
                            }
                            if let Some(p) = req.is_pinned {
                                l.is_pinned = p;
                            }
                            if let Some(m) = req.member_actor_ids {
                                l.member_actor_ids = m;
                            }
                            if let Some(s) = req.community_slugs {
                                l.community_slugs = s;
                            }
                            let out = serde_json::to_string(l).unwrap();
                            drop(guard);
                            if is_editor_patch {
                                le_done.fetch_add(1, Ordering::SeqCst);
                            } else {
                                lt_done.fetch_add(1, Ordering::SeqCst);
                            }
                            ("200 OK", out)
                        } else {
                            ("404 Not Found", r#"{"error":"not_found"}"#.to_string())
                        }
                    }
                } else if method == "GET" && path == "/api/v1/community/feed" {
                    let resp = FeedResponseDto {
                        items: vec![FeedItemDto {
                            post: build_post(),
                            explanation: None,
                            rank_score: 1.0,
                            is_exploration_slot: false,
                        }],
                        next_cursor: None,
                        ..Default::default()
                    };
                    ("200 OK", serde_json::to_string(&resp).unwrap())
                } else if path == "/api/v1/community/communities" {
                    ("200 OK", serde_json::to_string(&ListCommunitiesResponse::default()).unwrap())
                } else if path == "/api/v1/community/notifications" {
                    ("200 OK", serde_json::to_string(&NotificationsListResponse::default()).unwrap())
                } else if path == "/api/v1/community/trending" {
                    ("200 OK", serde_json::to_string(&TrendingResponse::default()).unwrap())
                } else if path == "/api/v1/community/algo-prefs" {
                    ("200 OK", serde_json::to_string(&AlgoPreferencesDto::default()).unwrap())
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

    (addr, state, AbortOnDrop(join))
}

#[test]
fn test_m5_r7_online_consistency_races_all_5_fixes() {
    run_in_headless_subprocess("test_m5_r7_online_consistency_races_all_5_fixes");
}

async fn body_r7_online_consistency_races_all_5_fixes(mut app: AsyncApp) {
    use std::sync::atomic::Ordering;

    let dir = TempDir::new().expect("r7 consistency tempdir");
    let rt_dir = TempDir::new().expect("r7 consistency rt tempdir");
    let rt_holder = TelegramClient::open_with_dir(
        ClientConfig {
            server_url: "http://127.0.0.1:9".to_string(),
            max_reconnect_attempts: 0,
            ..Default::default()
        },
        rt_dir.path(),
    )
    .expect("open rt holder");
    let owner_id = "u:907001".to_string();
    let bot_id = "b:907001".to_string();
    let post_id = "p_r7_cons_1".to_string();
    let comment_id = "c_r7_cons_1".to_string();
    let (addr, srv, _server_guard) = spawn_r7_consistency_server(
        rt_holder.runtime().handle(),
        owner_id.clone(),
        bot_id.clone(),
        post_id.clone(),
        comment_id.clone(),
    );

    let client = Arc::new(
        TelegramClient::open_with_dir(
            ClientConfig {
                server_url: format!("http://{addr}"),
                max_reconnect_attempts: 0,
                ..Default::default()
            },
            dir.path(),
        )
        .expect("open r7 client"),
    );
    let auth = test_auth(907001, "r7_owner");
    client.set_auth_session(auth.clone());
    let client_win = Arc::clone(&client);
    let auth_win = auth.clone();
    let shell = app
        .update(|cx| {
            let _rt_guard = client_win.runtime().enter();
            cx.new(|cx| {
                let mut s = AppShell::new_with_client(client_win.clone(), cx);
                s.apply_authenticated_session(auth_win.clone(), cx);
                s
            })
        })
        .expect("mount r7 AppShell");
    let _ = shell.update(&mut app, |s, cx| s.open_community_workspace(cx));
    let cw = shell
        .read_with(&app, |s, _| s.community_workspace.clone().unwrap())
        .unwrap();

    assert!(
        pump_until(&mut app, Duration::from_secs(5), |cx| {
            let st = &cw.read(cx).state;
            !st.feed_loading
                && st.identities.is_some()
                && st.feed_items.iter().any(|it| it.post.post_id == post_id)
        })
        .await,
        "R7 host must bootstrap feed"
    );

    // ------------------------------------------------------------------
    // Fix 1 (S2A-5b-6): Block -> Unblock while is_following==true racing load_actor_profile
    // ------------------------------------------------------------------
    let _ = cw.update(&mut app, |w, cx| {
        w.open_actor_profile("r7_bot".to_string(), cx);
    });
    assert!(
        pump_until(&mut app, Duration::from_secs(4), |cx| {
            let st = &cw.read(cx).state;
            !st.profile_loading
                && st
                    .actor_profile
                    .as_ref()
                    .is_some_and(|p| p.actor.actor_id == bot_id && p.actor.is_following)
        })
        .await
    );
    let prof_rx_before = srv.profile_get_received.load(Ordering::SeqCst);
    let prof_done_before = srv.profile_get_completed.load(Ordering::SeqCst);
    srv.profile_get_delay_ms.store(90, Ordering::SeqCst);
    srv.block_set_delay_ms.store(220, Ordering::SeqCst);
    let _ = cw.update(&mut app, |w, cx| {
        w.load_actor_profile("r7_bot".to_string(), cx);
    });
    assert!(
        pump_until(&mut app, Duration::from_secs(4), |_| {
            srv.profile_get_received.load(Ordering::SeqCst) > prof_rx_before
        })
        .await,
        "S2A-5b-6: load_actor_profile must capture pre-Block snapshot"
    );
    let _ = cw.update(&mut app, |w, cx| {
        w.toggle_block_actor(bot_id.clone(), false, cx);
        w.toggle_block_actor(bot_id.clone(), true, cx);
        assert!(w.state.actor_block_in_flight.contains_key(&bot_id));
        assert!(w.state.actor_block_dirty.contains(&bot_id));
    });
    assert!(
        pump_until(&mut app, Duration::from_secs(4), |cx| {
            srv.profile_get_completed.load(Ordering::SeqCst) > prof_done_before
                && !cw.read(cx).state.profile_loading
        })
        .await,
        "S2A-5b-6: stale load_actor_profile must settle mid-flight"
    );
    let _ = cw.read_with(&app, |w, _| {
        let a = &w.state.actor_profile.as_ref().unwrap().actor;
        assert!(
            !a.is_blocked && !a.is_following,
            "S2A-5b-6: mid-flight load_actor_profile must NEVER flash is_following=true"
        );
    });
    assert!(
        pump_until(&mut app, Duration::from_secs(5), |cx| {
            let st = &cw.read(cx).state;
            st.actor_block_in_flight.is_empty()
                && st.actor_block_dirty.is_empty()
                && srv.block_set_completed.load(Ordering::SeqCst) == 1
                && srv.block_remove_completed.load(Ordering::SeqCst) == 1
        })
        .await,
        "S2A-5b-6: Block -> Unblock flights must settle sequentially"
    );
    assert_eq!(srv.block_in_flight_max.load(Ordering::SeqCst), 1);
    let _ = cw.read_with(&app, |w, _| {
        let a = &w.state.actor_profile.as_ref().unwrap().actor;
        assert!(
            !a.is_blocked
                && !a.is_following
                && !srv.blocked.load(Ordering::SeqCst)
                && !srv.following.load(Ordering::SeqCst),
            "S2A-5b-6: UI and server must converge to (blk=false, fol=false)"
        );
    });

    // ------------------------------------------------------------------
    // Fix 2 (S2C-4f): Comment vote, like, edit & award Case B vs slow load_post_detail
    // ------------------------------------------------------------------
    let _ = cw.update(&mut app, |w, cx| {
        w.open_post_detail(post_id.clone(), cx);
    });
    assert!(
        pump_until(&mut app, Duration::from_secs(4), |cx| {
            let st = &cw.read(cx).state;
            !st.detail_loading
                && st
                    .detail_post
                    .as_ref()
                    .is_some_and(|d| d.post.post_id == post_id && d.comments.len() == 1)
        })
        .await
    );
    let det_rx_before = srv.detail_get_received.load(Ordering::SeqCst);
    let det_done_before = srv.detail_get_completed.load(Ordering::SeqCst);
    srv.detail_get_next_delay_ms.store(260, Ordering::SeqCst);
    let _ = cw.update(&mut app, |w, cx| {
        w.load_post_detail(post_id.clone(), cx);
    });
    assert!(
        pump_until(&mut app, Duration::from_secs(4), |_| {
            srv.detail_get_received.load(Ordering::SeqCst) > det_rx_before
        })
        .await,
        "S2C-4f: slow load_post_detail must capture stale comment snapshot"
    );
    let _ = cw.update(&mut app, |w, cx| {
        w.vote_comment_action(comment_id.clone(), 1, cx);
        w.toggle_comment_like_action(comment_id.clone(), cx);
    });
    assert!(
        pump_until(&mut app, Duration::from_secs(5), |cx| {
            let st = &cw.read(cx).state;
            !st.detail_loading
                && st.comment_vote_in_flight.is_empty()
                && st.comment_like_in_flight.is_empty()
                && srv.comment_vote_completed.load(Ordering::SeqCst) >= 1
                && srv.comment_like_completed.load(Ordering::SeqCst) >= 1
                && srv.detail_get_completed.load(Ordering::SeqCst) >= det_done_before + 2
        })
        .await,
        "S2C-4f: comment vote/like Case B must trigger reload_detail_if_loading_post"
    );
    assert!(
        srv.detail_get_received.load(Ordering::SeqCst) >= det_rx_before + 2,
        "S2C-4f: detail_calls must be >= 2 after Case B reload"
    );
    let _ = cw.read_with(&app, |w, _| {
        let c = w.state.find_comment(&comment_id).expect("comment present");
        assert_eq!(c.viewer_vote, 1);
        assert_eq!(c.score, 1);
        assert!(c.viewer_liked);
        assert_eq!(c.likes_count, 1);
        assert_eq!(srv.comment_vote_dir.load(Ordering::SeqCst), 1);
        assert!(srv.comment_liked.load(Ordering::SeqCst));
    });

    // Also verify edit & award Case B reload_detail_if_loading_post
    let det_rx_edit = srv.detail_get_received.load(Ordering::SeqCst);
    let det_done_edit = srv.detail_get_completed.load(Ordering::SeqCst);
    srv.detail_get_next_delay_ms.store(240, Ordering::SeqCst);
    let _ = cw.update(&mut app, |w, cx| {
        w.load_post_detail(post_id.clone(), cx);
    });
    assert!(
        pump_until(&mut app, Duration::from_secs(4), |_| {
            srv.detail_get_received.load(Ordering::SeqCst) > det_rx_edit
        })
        .await
    );
    let _ = cw.update(&mut app, |w, cx| {
        w.start_edit_comment(comment_id.clone(), "Edited Case B body".to_string(), cx);
        w.submit_edit_comment(cx);
        w.give_comment_award_action(comment_id.clone(), "gold".to_string(), cx);
    });
    assert!(
        pump_until(&mut app, Duration::from_secs(5), |cx| {
            let st = &cw.read(cx).state;
            !st.detail_loading
                && srv.comment_awards_count.load(Ordering::SeqCst) >= 1
                && srv.detail_get_completed.load(Ordering::SeqCst) >= det_done_edit + 2
        })
        .await
    );
    let _ = cw.read_with(&app, |w, _| {
        let c = w.state.find_comment(&comment_id).expect("comment present");
        assert_eq!(c.body, "Edited Case B body");
        assert_eq!(c.awards.len(), 1);
        assert_eq!(c.awards[0].award_kind, "gold");
    });

    // ------------------------------------------------------------------
    // Fix 3 (S2C-3c-1, S2C-3c-2, S2C-4d, S2C-4e): Single-flight + last_synced for vote_post & vote_comment_action
    // ------------------------------------------------------------------
    // 3A. Establish post vote (1, 1), then rapid +1 -> -1 under HTTP 500 -> rolls back to (1, 1)
    let _ = cw.update(&mut app, |w, cx| {
        w.vote_post(post_id.clone(), 1, cx);
    });
    assert!(
        pump_until(&mut app, Duration::from_secs(4), |cx| {
            cw.read(cx).state.post_vote_in_flight.is_empty()
                && srv.post_vote_completed.load(Ordering::SeqCst) == 1
        })
        .await
    );
    srv.post_vote_in_flight_max.store(0, Ordering::SeqCst);
    srv.post_vote_fail_500.store(true, Ordering::SeqCst);
    srv.post_vote_delay_ms.store(120, Ordering::SeqCst);
    let pv_done_500 = srv.post_vote_completed.load(Ordering::SeqCst);
    let _ = cw.update(&mut app, |w, cx| {
        w.vote_post(post_id.clone(), 1, cx);
        w.vote_post(post_id.clone(), -1, cx);
        assert!(w.state.post_vote_in_flight.contains_key(&post_id));
        assert!(w.state.post_vote_dirty.contains(&post_id));
    });
    assert!(
        pump_until(&mut app, Duration::from_secs(4), |cx| {
            let st = &cw.read(cx).state;
            st.post_vote_in_flight.is_empty()
                && st.post_vote_dirty.is_empty()
                && srv.post_vote_completed.load(Ordering::SeqCst) > pv_done_500
        })
        .await
    );
    assert_eq!(srv.post_vote_in_flight_max.load(Ordering::SeqCst), 1);
    let _ = cw.read_with(&app, |w, _| {
        let p = w.state.find_post(&post_id).expect("post present");
        assert_eq!(
            (p.viewer_vote, p.score),
            (1, 1),
            "S2C-3c-1: rapid +1->-1 HTTP 500 must roll back to server truth (1, 1)"
        );
        assert_eq!(srv.post_vote_dir.load(Ordering::SeqCst), 1);
    });

    // 3B. vote_post pre-commit delayed reorder +1(180ms) -> -1(fast)
    srv.post_vote_in_flight_max.store(0, Ordering::SeqCst);
    srv.post_vote_fail_500.store(false, Ordering::SeqCst);
    srv.post_vote_delay_ms.store(180, Ordering::SeqCst);
    let pv_rx_reord = srv.post_vote_received.load(Ordering::SeqCst);
    let pv_done_reord = srv.post_vote_completed.load(Ordering::SeqCst);
    let _ = cw.update(&mut app, |w, cx| {
        w.vote_post(post_id.clone(), 1, cx);
    });
    assert!(
        pump_until(&mut app, Duration::from_secs(4), |_| {
            srv.post_vote_received.load(Ordering::SeqCst) > pv_rx_reord
        })
        .await
    );
    let _ = cw.update(&mut app, |w, cx| {
        w.vote_post(post_id.clone(), -1, cx);
    });
    assert!(
        pump_until(&mut app, Duration::from_secs(5), |cx| {
            let st = &cw.read(cx).state;
            st.post_vote_in_flight.is_empty()
                && st.post_vote_dirty.is_empty()
                && srv.post_vote_completed.load(Ordering::SeqCst) >= pv_done_reord + 2
        })
        .await
    );
    assert_eq!(
        srv.post_vote_in_flight_max.load(Ordering::SeqCst),
        1,
        "S2C-3c-2: single-flight vote_post must have in_flight_max == 1"
    );
    let _ = cw.read_with(&app, |w, _| {
        let p = w.state.find_post(&post_id).expect("post present");
        assert_eq!((p.viewer_vote, p.score), (-1, -1));
        assert_eq!(srv.post_vote_dir.load(Ordering::SeqCst), -1);
    });

    // 3C. vote_comment_action rapid +1 -> -1 under HTTP 500 -> rolls back to server truth (1, 1)
    srv.comment_vote_in_flight_max.store(0, Ordering::SeqCst);
    srv.comment_vote_fail_500.store(true, Ordering::SeqCst);
    srv.comment_vote_delay_ms.store(120, Ordering::SeqCst);
    let cv_done_500 = srv.comment_vote_completed.load(Ordering::SeqCst);
    let _ = cw.update(&mut app, |w, cx| {
        w.vote_comment_action(comment_id.clone(), 1, cx);
        w.vote_comment_action(comment_id.clone(), -1, cx);
        assert!(w.state.comment_vote_in_flight.contains_key(&comment_id));
        assert!(w.state.comment_vote_dirty.contains(&comment_id));
    });
    assert!(
        pump_until(&mut app, Duration::from_secs(4), |cx| {
            let st = &cw.read(cx).state;
            st.comment_vote_in_flight.is_empty()
                && st.comment_vote_dirty.is_empty()
                && srv.comment_vote_completed.load(Ordering::SeqCst) > cv_done_500
        })
        .await
    );
    assert_eq!(srv.comment_vote_in_flight_max.load(Ordering::SeqCst), 1);
    let _ = cw.read_with(&app, |w, _| {
        let c = w.state.find_comment(&comment_id).expect("comment present");
        assert_eq!(
            (c.viewer_vote, c.score),
            (1, 1),
            "S2C-4d: comment vote rapid +1->-1 HTTP 500 must roll back to server truth (1, 1)"
        );
        assert_eq!(srv.comment_vote_dir.load(Ordering::SeqCst), 1);
    });

    // 3D. vote_comment_action pre-commit delayed reorder +1(180ms) -> -1(fast)
    srv.comment_vote_in_flight_max.store(0, Ordering::SeqCst);
    srv.comment_vote_fail_500.store(false, Ordering::SeqCst);
    srv.comment_vote_delay_ms.store(180, Ordering::SeqCst);
    let cv_rx_reord = srv.comment_vote_received.load(Ordering::SeqCst);
    let cv_done_reord = srv.comment_vote_completed.load(Ordering::SeqCst);
    let _ = cw.update(&mut app, |w, cx| {
        w.vote_comment_action(comment_id.clone(), 1, cx);
    });
    assert!(
        pump_until(&mut app, Duration::from_secs(4), |_| {
            srv.comment_vote_received.load(Ordering::SeqCst) > cv_rx_reord
        })
        .await
    );
    let _ = cw.update(&mut app, |w, cx| {
        w.vote_comment_action(comment_id.clone(), -1, cx);
    });
    assert!(
        pump_until(&mut app, Duration::from_secs(5), |cx| {
            let st = &cw.read(cx).state;
            st.comment_vote_in_flight.is_empty()
                && st.comment_vote_dirty.is_empty()
                && srv.comment_vote_completed.load(Ordering::SeqCst) >= cv_done_reord + 2
        })
        .await
    );
    assert_eq!(
        srv.comment_vote_in_flight_max.load(Ordering::SeqCst),
        1,
        "S2C-4e: single-flight vote_comment_action must have in_flight_max == 1"
    );
    let _ = cw.read_with(&app, |w, _| {
        let c = w.state.find_comment(&comment_id).expect("comment present");
        assert_eq!((c.viewer_vote, c.score), (-1, -1));
        assert_eq!(srv.comment_vote_dir.load(Ordering::SeqCst), -1);
    });

    // ------------------------------------------------------------------
    // Fix 4 (2F_3D): submit_list_editor racing fast-settled pin + actor toggle + subsequent community toggle
    // ------------------------------------------------------------------
    let _ = cw.update(&mut app, |w, cx| {
        w.navigate_to(CommunityRoute::BookmarksLists, cx);
        w.set_bookmarks_section(BookmarksListsSection::Lists, cx);
    });
    assert!(
        pump_until(&mut app, Duration::from_secs(4), |cx| {
            let st = &cw.read(cx).state;
            !st.saved_loading && st.lists.iter().any(|l| l.list_id == "list_ed_toggle")
        })
        .await
    );
    let lg_rx_2f3d = srv.lists_get_received.load(Ordering::SeqCst);
    let lg_done_2f3d = srv.lists_get_completed.load(Ordering::SeqCst);
    srv.lists_get_delay_ms.store(240, Ordering::SeqCst);
    let _ = cw.update(&mut app, |w, cx| {
        let target = w
            .state
            .lists
            .iter()
            .find(|l| l.list_id == "list_ed_toggle")
            .cloned()
            .expect("list_ed_toggle present");
        w.open_edit_list_form(target, cx);
        w.state.list_editor.name = "Renamed List".to_string();
        w.submit_list_editor(cx);
    });
    assert!(
        pump_until(&mut app, Duration::from_secs(4), |_| {
            srv.list_editor_patch_completed.load(Ordering::SeqCst) >= 1
                && srv.lists_get_received.load(Ordering::SeqCst) > lg_rx_2f3d
        })
        .await,
        "2F_3D: editor PATCH must commit and enter delayed GET /lists"
    );
    let lt_before_2f3d = srv.list_toggle_patch_completed.load(Ordering::SeqCst);
    let _ = cw.update(&mut app, |w, cx| {
        w.toggle_list_pinned("list_ed_toggle".to_string(), true, cx);
        w.toggle_selected_list_actor("list_ed_toggle".to_string(), "u:300".to_string(), cx);
    });
    assert!(
        pump_until(&mut app, Duration::from_secs(4), |cx| {
            let st = &cw.read(cx).state;
            st.list_membership_in_flight.is_empty()
                && st.list_membership_dirty.is_empty()
                && srv.list_toggle_patch_completed.load(Ordering::SeqCst) >= lt_before_2f3d + 1
        })
        .await,
        "2F_3D: fast quick-toggles must settle before delayed GET /lists returns"
    );
    assert!(
        pump_until(&mut app, Duration::from_secs(5), |cx| {
            let st = &cw.read(cx).state;
            !st.list_editor.submitting
                && srv.lists_get_completed.load(Ordering::SeqCst) > lg_done_2f3d
                && st.list_membership_in_flight.is_empty()
                && st.list_membership_dirty.is_empty()
        })
        .await,
        "2F_3D: delayed submit_list_editor and any re-flush must settle"
    );
    let _ = cw.update(&mut app, |w, cx| {
        w.toggle_selected_list_community("list_ed_toggle".to_string(), "rust".to_string(), cx);
    });
    assert!(
        pump_until(&mut app, Duration::from_secs(4), |cx| {
            let st = &cw.read(cx).state;
            st.list_membership_in_flight.is_empty() && st.list_membership_dirty.is_empty()
        })
        .await
    );
    let _ = cw.read_with(&app, |w, _| {
        let ui = w
            .state
            .lists
            .iter()
            .find(|l| l.list_id == "list_ed_toggle")
            .expect("list_ed_toggle in UI");
        let guard = srv.lists.lock().unwrap();
        let sv = guard
            .iter()
            .find(|l| l.list_id == "list_ed_toggle")
            .expect("list_ed_toggle on server");
        assert_eq!(ui.name, "Renamed List");
        assert!(ui.is_pinned);
        assert_eq!(ui.member_actor_ids, vec!["u:300".to_string()]);
        assert_eq!(ui.community_slugs, vec!["rust".to_string()]);
        assert_eq!(sv.name, "Renamed List");
        assert!(sv.is_pinned);
        assert_eq!(sv.member_actor_ids, vec!["u:300".to_string()]);
        assert_eq!(sv.community_slugs, vec!["rust".to_string()]);
    });

    // ------------------------------------------------------------------
    // Fix 5 (2F_3F): submit_list_editor HTTP 500 while Flight 1 in-flight + u:300 dirty
    // ------------------------------------------------------------------
    let lt_before_2f3f = srv.list_toggle_patch_completed.load(Ordering::SeqCst);
    let le_before_2f3f = srv.list_editor_patch_completed.load(Ordering::SeqCst);
    srv.list_toggle_patch_delay_ms.store(180, Ordering::SeqCst);
    srv.list_editor_patch_fail_500.store(true, Ordering::SeqCst);
    let _ = cw.update(&mut app, |w, cx| {
        let target = w
            .state
            .lists
            .iter()
            .find(|l| l.list_id == "list_ed_500")
            .cloned()
            .expect("list_ed_500 present");
        w.open_edit_list_form(target, cx);
        w.state.list_editor.name = "Should Fail 500".to_string();
        w.toggle_list_pinned("list_ed_500".to_string(), true, cx);
        w.toggle_selected_list_actor("list_ed_500".to_string(), "u:300".to_string(), cx);
        assert!(w.state.list_membership_in_flight.contains_key("list_ed_500"));
        assert!(w.state.list_membership_dirty.contains("list_ed_500"));
        w.submit_list_editor(cx);
        assert!(
            w.state.list_membership_dirty.contains("list_ed_500"),
            "2F_3F: submit_list_editor must NEVER eagerly remove list_membership_dirty pre-RPC"
        );
    });
    assert!(
        pump_until(&mut app, Duration::from_secs(5), |cx| {
            let st = &cw.read(cx).state;
            !st.list_editor.submitting
                && st.list_membership_in_flight.is_empty()
                && st.list_membership_dirty.is_empty()
                && srv.list_editor_patch_completed.load(Ordering::SeqCst) > le_before_2f3f
                && srv.list_toggle_patch_completed.load(Ordering::SeqCst) >= lt_before_2f3f + 2
        })
        .await,
        "2F_3F: Flight 1 and dirty Flight 2 must both flush despite editor HTTP 500"
    );
    let _ = cw.read_with(&app, |w, _| {
        assert!(
            w.state.list_editor.error.is_some(),
            "2F_3F: failed editor PATCH must report error"
        );
        let ui = w
            .state
            .lists
            .iter()
            .find(|l| l.list_id == "list_ed_500")
            .expect("list_ed_500 in UI");
        let guard = srv.lists.lock().unwrap();
        let sv = guard
            .iter()
            .find(|l| l.list_id == "list_ed_500")
            .expect("list_ed_500 on server");
        assert!(ui.is_pinned && ui.member_actor_ids == vec!["u:300".to_string()]);
        assert!(sv.is_pinned && sv.member_actor_ids == vec!["u:300".to_string()]);
    });

    drop(rt_holder);
}




