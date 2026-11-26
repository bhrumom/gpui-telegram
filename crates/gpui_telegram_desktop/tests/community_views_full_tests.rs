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
        "test_m5_r8_universal_consistency_and_secondary_mutations" => {
            |app| Box::pin(body_r8_universal_consistency_and_secondary_mutations(app))
        }
        "test_m5_r9_cross_lane_and_facade_isolation" => {
            |app| Box::pin(body_r9_cross_lane_and_facade_isolation(app))
        }
        "test_m5_r10_field_scoped_patches_and_symmetric_serialization" => {
            |app| Box::pin(body_r10_field_scoped_patches_and_symmetric_serialization(app))
        }
        "test_m5_r11_identity_switch_seq_decoupling_and_cross_route_deletes" => {
            |app| Box::pin(body_r11_identity_switch_seq_decoupling_and_cross_route_deletes(app))
        }
        "test_m5_r12_cross_actor_automod_tombstone_and_settled_routes" => {
            |app| Box::pin(body_r12_cross_actor_automod_tombstone_and_settled_routes(app))
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
            let sb_w = s.apple_sidebar.read(cx).effective_width();
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
    // Production local DBs start empty (zero fake data); explicitly seed UI test fixture here.
    let now_ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(1_700_000_000_000);
    let _ = client
        .community()
        .local_runtime()
        .store()
        .seed_if_empty(now_ms);

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
    let seeded_slug_key = seeded_slug.to_ascii_lowercase();
    assert!(
        pump_until(&mut app, Duration::from_secs(4), |cx| {
            let st = &cw.read(cx).state;
            !st.community_join_in_flight.contains_key(&seeded_slug_key)
                && !st.community_join_dirty.contains(&seeded_slug_key)
                && st.community_detail.as_ref().is_some_and(|d| {
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
        w.state
            .actor_profile
            .as_mut()
            .expect("F20: actor_profile must be loaded before NaN trust_score render")
            .actor
            .trust_score = f64::NAN;
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
        w.state
            .bookmark_last_synced
            .get_mut(&post_id)
            .expect("R6 Fix 3: bookmark_last_synced must exist while remove_bookmark_from_saved is in flight")
            .saved_item = None;
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

// ============================================================================
// Wave r8 — Universal 4-Invariant Consistency & Secondary Mutations Regression Suite
// ============================================================================

struct R8MockState {
    owner_id: String,
    alt_id: String,
    bot_b_id: String,
    bob_id: String,
    charlie_id: String,
    post_id: String,
    mpoll_id: String,
    draft_id: String,
    comment_id: String,
    slug: String,

    // Actors (bob mute/block/follow + bot edits)
    bob_muted: std::sync::atomic::AtomicBool,
    bob_blocked: std::sync::atomic::AtomicBool,
    bob_following: std::sync::atomic::AtomicBool,
    bob_followers_count: std::sync::atomic::AtomicU64,
    bot_a_display_name: std::sync::Mutex<String>,
    profile_get_delay_ms: std::sync::atomic::AtomicU64,
    profile_get_received: std::sync::atomic::AtomicUsize,
    profile_get_completed: std::sync::atomic::AtomicUsize,
    mute_delay_ms: std::sync::atomic::AtomicU64,
    mute_fail_500: std::sync::atomic::AtomicBool,
    mute_completed: std::sync::atomic::AtomicUsize,
    block_delay_ms: std::sync::atomic::AtomicU64,
    block_fail_500: std::sync::atomic::AtomicBool,
    block_completed: std::sync::atomic::AtomicUsize,
    follow_delay_ms: std::sync::atomic::AtomicU64,
    follow_fail_500: std::sync::atomic::AtomicBool,
    follow_in_flight: std::sync::atomic::AtomicUsize,
    follow_in_flight_max: std::sync::atomic::AtomicUsize,
    follow_received: std::sync::atomic::AtomicUsize,
    follow_completed: std::sync::atomic::AtomicUsize,
    actor_patch_delay_ms: std::sync::atomic::AtomicU64,
    actor_patch_completed: std::sync::atomic::AtomicUsize,

    // Posts, Polls, Comments, Awards, Bookmarks, Feed, Drafts
    post_body: std::sync::Mutex<String>,
    post_deleted: std::sync::atomic::AtomicBool,
    post_vote_dir: std::sync::atomic::AtomicI8,
    post_vote_delay_ms: std::sync::atomic::AtomicU64,
    post_vote_fail_500: std::sync::atomic::AtomicBool,
    post_vote_completed: std::sync::atomic::AtomicUsize,
    post_liked: std::sync::atomic::AtomicBool,
    post_like_delay_ms: std::sync::atomic::AtomicU64,
    post_like_fail_500: std::sync::atomic::AtomicBool,
    post_like_completed: std::sync::atomic::AtomicUsize,
    post_reposted: std::sync::atomic::AtomicBool,
    post_repost_delay_ms: std::sync::atomic::AtomicU64,
    post_repost_fail_500: std::sync::atomic::AtomicBool,
    post_repost_in_flight: std::sync::atomic::AtomicUsize,
    post_repost_in_flight_max: std::sync::atomic::AtomicUsize,
    post_repost_received: std::sync::atomic::AtomicUsize,
    post_repost_completed: std::sync::atomic::AtomicUsize,
    post_bookmarked: std::sync::atomic::AtomicBool,
    post_bookmark_delay_ms: std::sync::atomic::AtomicU64,
    post_bookmark_fail_500: std::sync::atomic::AtomicBool,
    post_bookmark_completed: std::sync::atomic::AtomicUsize,
    post_awards: std::sync::Mutex<Vec<gpui_telegram_proto::community_contracts::PostAwardDto>>,
    post_award_delay_ms: std::sync::atomic::AtomicU64,
    post_award_received: std::sync::atomic::AtomicUsize,
    post_award_completed: std::sync::atomic::AtomicUsize,
    single_poll_voted: std::sync::Mutex<Vec<String>>,
    multi_poll_voted: std::sync::Mutex<Vec<String>>,
    poll_vote_delay_ms: std::sync::atomic::AtomicU64,
    poll_vote_fail_500: std::sync::atomic::AtomicBool,
    poll_vote_in_flight: std::sync::atomic::AtomicUsize,
    poll_vote_in_flight_max: std::sync::atomic::AtomicUsize,
    poll_vote_received: std::sync::atomic::AtomicUsize,
    poll_vote_completed: std::sync::atomic::AtomicUsize,
    comment_vote_dir: std::sync::atomic::AtomicI8,
    comment_vote_delay_ms: std::sync::atomic::AtomicU64,
    comment_vote_fail_500: std::sync::atomic::AtomicBool,
    comment_vote_completed: std::sync::atomic::AtomicUsize,
    comment_liked: std::sync::atomic::AtomicBool,
    comment_like_delay_ms: std::sync::atomic::AtomicU64,
    comment_like_fail_500: std::sync::atomic::AtomicBool,
    comment_like_completed: std::sync::atomic::AtomicUsize,
    detail_get_delay_ms: std::sync::atomic::AtomicU64,
    detail_get_received: std::sync::atomic::AtomicUsize,
    detail_get_completed: std::sync::atomic::AtomicUsize,
    feed_get_delay_ms: std::sync::atomic::AtomicU64,
    feed_get_received: std::sync::atomic::AtomicUsize,
    feed_get_completed: std::sync::atomic::AtomicUsize,
    drafts_get_delay_ms: std::sync::atomic::AtomicU64,
    drafts_get_received: std::sync::atomic::AtomicUsize,
    drafts_get_completed: std::sync::atomic::AtomicUsize,
    draft_published: std::sync::atomic::AtomicBool,
    extra_posts: std::sync::Mutex<Vec<CommunityPostDto>>,
    create_post_delay_ms: std::sync::atomic::AtomicU64,
    create_post_completed: std::sync::atomic::AtomicUsize,
    update_post_delay_ms: std::sync::atomic::AtomicU64,
    update_post_completed: std::sync::atomic::AtomicUsize,
    delete_post_delay_ms: std::sync::atomic::AtomicU64,
    delete_post_completed: std::sync::atomic::AtomicUsize,
    create_comment_delay_ms: std::sync::atomic::AtomicU64,
    create_comment_completed: std::sync::atomic::AtomicUsize,
    signal_delay_ms: std::sync::atomic::AtomicU64,
    signal_completed: std::sync::atomic::AtomicUsize,
    summon_delay_ms: std::sync::atomic::AtomicU64,
    summon_completed: std::sync::atomic::AtomicUsize,

    // Community, Wiki, Rules, Join, Mod Queue, AutoMod
    comm_joined: std::sync::atomic::AtomicBool,
    comm_members: std::sync::atomic::AtomicU64,
    comm_wiki: std::sync::Mutex<String>,
    comm_wiki_rev: std::sync::atomic::AtomicU32,
    comm_rules: std::sync::Mutex<Vec<gpui_telegram_proto::community_contracts::CommunityRuleDto>>,
    comm_automod_rules: std::sync::Mutex<Vec<AutoModRuleDto>>,
    comm_get_delay_ms: std::sync::atomic::AtomicU64,
    comm_get_received: std::sync::atomic::AtomicUsize,
    comm_get_completed: std::sync::atomic::AtomicUsize,
    join_delay_ms: std::sync::atomic::AtomicU64,
    join_fail_500: std::sync::atomic::AtomicBool,
    join_in_flight: std::sync::atomic::AtomicUsize,
    join_in_flight_max: std::sync::atomic::AtomicUsize,
    join_received: std::sync::atomic::AtomicUsize,
    join_completed: std::sync::atomic::AtomicUsize,
    comm_patch_delay_ms: std::sync::atomic::AtomicU64,
    comm_patch_fail_500: std::sync::atomic::AtomicBool,
    comm_patch_completed: std::sync::atomic::AtomicUsize,
    wiki_put_delay_ms: std::sync::atomic::AtomicU64,
    wiki_put_completed: std::sync::atomic::AtomicUsize,
    mod_queue_items: std::sync::Mutex<Vec<gpui_telegram_proto::community_contracts::ModQueueItemDto>>,
    mod_queue_get_delay_ms: std::sync::atomic::AtomicU64,
    mod_queue_get_received: std::sync::atomic::AtomicUsize,
    mod_queue_get_completed: std::sync::atomic::AtomicUsize,
    mod_action_delay_ms: std::sync::atomic::AtomicU64,
    mod_action_completed: std::sync::atomic::AtomicUsize,

    // Lists
    lists: std::sync::Mutex<Vec<CommunityListDto>>,
    lists_get_delay_ms: std::sync::atomic::AtomicU64,
    lists_get_received: std::sync::atomic::AtomicUsize,
    lists_get_completed: std::sync::atomic::AtomicUsize,
    list_editor_patch_delay_ms: std::sync::atomic::AtomicU64,
    list_editor_patch_completed: std::sync::atomic::AtomicUsize,
    list_toggle_patch_delay_ms: std::sync::atomic::AtomicU64,
    list_toggle_patch_fail_500: std::sync::atomic::AtomicBool,
    list_toggle_patch_received: std::sync::atomic::AtomicUsize,
    list_toggle_patch_completed: std::sync::atomic::AtomicUsize,
    list_delete_delay_ms: std::sync::atomic::AtomicU64,
    list_delete_fail_500: std::sync::atomic::AtomicBool,
    list_delete_received: std::sync::atomic::AtomicUsize,
    list_delete_completed: std::sync::atomic::AtomicUsize,

    // Notifications
    notifs: std::sync::Mutex<Vec<CommunityNotificationDto>>,
    notifs_get_completed: std::sync::atomic::AtomicUsize,
    notif_read_fail_500: std::sync::atomic::AtomicBool,
    notif_read_override_unread: std::sync::atomic::AtomicI64,
    notif_read_completed: std::sync::atomic::AtomicUsize,
    fail_second_flight: std::sync::atomic::AtomicBool,
}

fn should_fail_r8(
    fail_500: &std::sync::atomic::AtomicBool,
    fail_second: &std::sync::atomic::AtomicBool,
) -> bool {
    use std::sync::atomic::Ordering;
    if fail_500.load(Ordering::SeqCst) {
        return true;
    }
    if fail_second.swap(false, Ordering::SeqCst) {
        fail_500.store(true, Ordering::SeqCst);
        return false;
    }
    false
}

impl R8MockState {
    fn build_post(&self) -> CommunityPostDto {
        use gpui_telegram_proto::community_contracts::{CommunityPollOptionDto, PollDto};
        use std::sync::atomic::Ordering;
        let mut p = mock_post_dto(
            &self.post_id,
            &self.owner_id,
            self.post_bookmarked.load(Ordering::SeqCst),
            if self.post_bookmarked.load(Ordering::SeqCst) {
                Some("All")
            } else {
                None
            },
            u64::from(self.post_bookmarked.load(Ordering::SeqCst)),
        );
        p.body = self.post_body.lock().unwrap().clone();
        p.community_slug = Some(self.slug.clone());
        let pv = self.post_vote_dir.load(Ordering::SeqCst);
        p.viewer_vote = pv;
        p.upvotes = i64::from(pv == 1);
        p.downvotes = i64::from(pv == -1);
        p.score = i64::from(pv);
        let pl = self.post_liked.load(Ordering::SeqCst);
        p.viewer_liked = pl;
        p.likes_count = u64::from(pl);
        let pr = self.post_reposted.load(Ordering::SeqCst);
        p.viewer_reposted = pr;
        p.reposts_count = u64::from(pr);
        p.awards = self.post_awards.lock().unwrap().clone();
        let voted = self.single_poll_voted.lock().unwrap().clone();
        p.poll = Some(PollDto {
            poll_id: "poll_r8_single".to_string(),
            question: "Single choice?".to_string(),
            options: vec![
                CommunityPollOptionDto {
                    option_id: "opt_a".to_string(),
                    text: "Option A".to_string(),
                    all_votes: u64::from(voted.iter().any(|x| x == "opt_a")),
                    human_votes: u64::from(voted.iter().any(|x| x == "opt_a")),
                    bot_votes: 0,
                },
                CommunityPollOptionDto {
                    option_id: "opt_b".to_string(),
                    text: "Option B".to_string(),
                    all_votes: u64::from(voted.iter().any(|x| x == "opt_b")),
                    human_votes: u64::from(voted.iter().any(|x| x == "opt_b")),
                    bot_votes: 0,
                },
            ],
            multi_select: false,
            expires_at_ms: 1_999_999_999_000,
            is_closed: false,
            total_votes: voted.len() as u64,
            total_human_votes: voted.len() as u64,
            total_bot_votes: 0,
            viewer_voted_option_ids: voted,
        });
        p
    }

    fn build_mpoll_post(&self) -> CommunityPostDto {
        use gpui_telegram_proto::community_contracts::{CommunityPollOptionDto, PollDto};
        let mut p = mock_post_dto(&self.mpoll_id, &self.owner_id, false, None, 0);
        p.community_slug = Some(self.slug.clone());
        let voted = self.multi_poll_voted.lock().unwrap().clone();
        p.poll = Some(PollDto {
            poll_id: "poll_r8_multi".to_string(),
            question: "Multi choice?".to_string(),
            options: vec![
                CommunityPollOptionDto {
                    option_id: "opt_a".to_string(),
                    text: "Multi A".to_string(),
                    all_votes: u64::from(voted.iter().any(|x| x == "opt_a")),
                    human_votes: u64::from(voted.iter().any(|x| x == "opt_a")),
                    bot_votes: 0,
                },
                CommunityPollOptionDto {
                    option_id: "opt_b".to_string(),
                    text: "Multi B".to_string(),
                    all_votes: u64::from(voted.iter().any(|x| x == "opt_b")),
                    human_votes: u64::from(voted.iter().any(|x| x == "opt_b")),
                    bot_votes: 0,
                },
            ],
            multi_select: true,
            expires_at_ms: 1_999_999_999_000,
            is_closed: false,
            total_votes: voted.len() as u64,
            total_human_votes: voted.len() as u64,
            total_bot_votes: 0,
            viewer_voted_option_ids: voted,
        });
        p
    }

    fn build_draft_post(&self) -> CommunityPostDto {
        use std::sync::atomic::Ordering;
        let mut p = mock_post_dto(&self.draft_id, &self.owner_id, false, None, 0);
        p.title = Some("Draft Title R8".to_string());
        p.body = "Draft Body R8".to_string();
        p.status = if self.draft_published.load(Ordering::SeqCst) {
            PostStatus::Published
        } else {
            PostStatus::Draft
        };
        p
    }

    fn build_comment(&self) -> CommunityCommentDto {
        use std::sync::atomic::Ordering;
        let mut c = mock_comment_dto(&self.comment_id, &self.post_id, &self.owner_id);
        let cv = self.comment_vote_dir.load(Ordering::SeqCst);
        c.viewer_vote = cv;
        c.upvotes = i64::from(cv == 1);
        c.downvotes = i64::from(cv == -1);
        c.score = i64::from(cv);
        let cl = self.comment_liked.load(Ordering::SeqCst);
        c.viewer_liked = cl;
        c.likes_count = u64::from(cl);
        c
    }

    fn build_community(&self, acting_actor: Option<&str>) -> SubCommunityDto {
        use std::sync::atomic::Ordering;
        let is_owner = acting_actor.is_none_or(|a| a == self.owner_id);
        let role = if is_owner {
            Some(CommunityMemberRole::Mod)
        } else {
            None
        };
        let mut c = mock_community_dto(&self.slug, &self.owner_id, role);
        c.is_joined = self.comm_joined.load(Ordering::SeqCst);
        c.member_count = self.comm_members.load(Ordering::SeqCst);
        c.wiki_markdown = self.comm_wiki.lock().unwrap().clone();
        c.wiki_revision = self.comm_wiki_rev.load(Ordering::SeqCst);
        c.rules = self.comm_rules.lock().unwrap().clone();
        c.automod_rules = self.comm_automod_rules.lock().unwrap().clone();
        c
    }
}

fn spawn_r8_universal_server(
    rt_handle: &tokio::runtime::Handle,
) -> (std::net::SocketAddr, Arc<R8MockState>, AbortOnDrop) {
    use gpui_telegram_proto::community_contracts::{
        AwardResponse, CommunityPollVoteRequest, CommunityWikiRevisionDto, CreatePostResponse,
        FollowActionResponse, GiveAwardRequest, JoinCommunityResponse,
        MarkNotificationsReadRequest, MarkNotificationsReadResponse, ModActionRequest,
        ModLogEntryDto, ModQueueItemDto, PostAwardDto, RepostResponse, SummonBotResponse,
        UpdateCommunityActorRequest, UpdateCommunityWikiRequest, UpdatePostRequest,
    };
    use std::sync::atomic::{AtomicBool, AtomicI64, AtomicI8, AtomicU32, AtomicU64, AtomicUsize, Ordering};
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    let owner_id = "u:908001".to_string();
    let alt_id = "b:908002".to_string();
    let bot_b_id = "b:908003".to_string();
    let bob_id = "u:908010".to_string();
    let charlie_id = "u:908011".to_string();
    let post_id = "p_r8_main".to_string();
    let mpoll_id = "p_r8_mpoll".to_string();
    let draft_id = "p_r8_draft".to_string();
    let comment_id = "c_r8_main".to_string();
    let slug = "r8_lab".to_string();

    let seed_lists = vec![
        mock_list_dto("l_3e", &owner_id, "Orig 3E"),
        mock_list_dto("l_3g_b", &owner_id, "Orig B"),
        mock_list_dto("l_3g", &owner_id, "Orig 3G"),
        mock_list_dto("l_3g_err", &owner_id, "Orig 3G Err"),
        mock_list_dto("l_3d_500", &owner_id, "Orig 3D 500"),
    ];
    let seed_queue = vec![
        ModQueueItemDto {
            item_id: "mq_r8_1".to_string(),
            community_id: format!("comm_{slug}"),
            target_kind: "post".to_string(),
            target_id: post_id.clone(),
            reporter_actor_id: Some(bob_id.clone()),
            source: "report".to_string(),
            reason: "Spam check 1".to_string(),
            rule_triggered: None,
            status: ModQueueStatus::Pending,
            resolved_by_actor_id: None,
            resolved_at_ms: None,
            created_at_ms: 1_750_000_000_000,
        },
        ModQueueItemDto {
            item_id: "mq_r8_2".to_string(),
            community_id: format!("comm_{slug}"),
            target_kind: "post".to_string(),
            target_id: post_id.clone(),
            reporter_actor_id: Some(bob_id.clone()),
            source: "report".to_string(),
            reason: "Spam check 2".to_string(),
            rule_triggered: None,
            status: ModQueueStatus::Pending,
            resolved_by_actor_id: None,
            resolved_at_ms: None,
            created_at_ms: 1_750_000_001_000,
        },
    ];
    let seed_notifs = vec![
        CommunityNotificationDto {
            notif_id: "n_r8_1".to_string(),
            recipient_actor_id: owner_id.clone(),
            actor_id: bob_id.clone(),
            actor_handle: "r8_bob".to_string(),
            actor_display_name: "R8 Bob".to_string(),
            actor_kind: ActorKind::Human,
            kind: CommunityNotificationKind::Reply,
            target_post_id: Some(post_id.clone()),
            target_comment_id: None,
            preview_text: "Unread notif 1".to_string(),
            is_read: false,
            created_at_ms: 1_750_000_000_000,
        },
        CommunityNotificationDto {
            notif_id: "n_r8_2".to_string(),
            recipient_actor_id: owner_id.clone(),
            actor_id: bob_id.clone(),
            actor_handle: "r8_bob".to_string(),
            actor_display_name: "R8 Bob".to_string(),
            actor_kind: ActorKind::Human,
            kind: CommunityNotificationKind::Mention,
            target_post_id: Some(post_id.clone()),
            target_comment_id: None,
            preview_text: "Unread notif 2".to_string(),
            is_read: false,
            created_at_ms: 1_750_000_001_000,
        },
    ];

    let state = Arc::new(R8MockState {
        owner_id,
        alt_id,
        bot_b_id,
        bob_id,
        charlie_id,
        post_id,
        mpoll_id,
        draft_id,
        comment_id,
        slug,
        bob_muted: AtomicBool::new(false),
        bob_blocked: AtomicBool::new(false),
        bob_following: AtomicBool::new(true),
        bob_followers_count: AtomicU64::new(5),
        bot_a_display_name: std::sync::Mutex::new("R8 Bot A".to_string()),
        profile_get_delay_ms: AtomicU64::new(0),
        profile_get_received: AtomicUsize::new(0),
        profile_get_completed: AtomicUsize::new(0),
        mute_delay_ms: AtomicU64::new(0),
        mute_fail_500: AtomicBool::new(false),
        mute_completed: AtomicUsize::new(0),
        block_delay_ms: AtomicU64::new(0),
        block_fail_500: AtomicBool::new(false),
        block_completed: AtomicUsize::new(0),
        follow_delay_ms: AtomicU64::new(0),
        follow_fail_500: AtomicBool::new(false),
        follow_in_flight: AtomicUsize::new(0),
        follow_in_flight_max: AtomicUsize::new(0),
        follow_received: AtomicUsize::new(0),
        follow_completed: AtomicUsize::new(0),
        actor_patch_delay_ms: AtomicU64::new(0),
        actor_patch_completed: AtomicUsize::new(0),
        post_body: std::sync::Mutex::new("Initial R8 post body".to_string()),
        post_deleted: AtomicBool::new(false),
        post_vote_dir: AtomicI8::new(0),
        post_vote_delay_ms: AtomicU64::new(0),
        post_vote_fail_500: AtomicBool::new(false),
        post_vote_completed: AtomicUsize::new(0),
        post_liked: AtomicBool::new(false),
        post_like_delay_ms: AtomicU64::new(0),
        post_like_fail_500: AtomicBool::new(false),
        post_like_completed: AtomicUsize::new(0),
        post_reposted: AtomicBool::new(false),
        post_repost_delay_ms: AtomicU64::new(0),
        post_repost_fail_500: AtomicBool::new(false),
        post_repost_in_flight: AtomicUsize::new(0),
        post_repost_in_flight_max: AtomicUsize::new(0),
        post_repost_received: AtomicUsize::new(0),
        post_repost_completed: AtomicUsize::new(0),
        post_bookmarked: AtomicBool::new(false),
        post_bookmark_delay_ms: AtomicU64::new(0),
        post_bookmark_fail_500: AtomicBool::new(false),
        post_bookmark_completed: AtomicUsize::new(0),
        post_awards: std::sync::Mutex::new(Vec::new()),
        post_award_delay_ms: AtomicU64::new(0),
        post_award_received: AtomicUsize::new(0),
        post_award_completed: AtomicUsize::new(0),
        single_poll_voted: std::sync::Mutex::new(Vec::new()),
        multi_poll_voted: std::sync::Mutex::new(Vec::new()),
        poll_vote_delay_ms: AtomicU64::new(0),
        poll_vote_fail_500: AtomicBool::new(false),
        poll_vote_in_flight: AtomicUsize::new(0),
        poll_vote_in_flight_max: AtomicUsize::new(0),
        poll_vote_received: AtomicUsize::new(0),
        poll_vote_completed: AtomicUsize::new(0),
        comment_vote_dir: AtomicI8::new(0),
        comment_vote_delay_ms: AtomicU64::new(0),
        comment_vote_fail_500: AtomicBool::new(false),
        comment_vote_completed: AtomicUsize::new(0),
        comment_liked: AtomicBool::new(false),
        comment_like_delay_ms: AtomicU64::new(0),
        comment_like_fail_500: AtomicBool::new(false),
        comment_like_completed: AtomicUsize::new(0),
        detail_get_delay_ms: AtomicU64::new(0),
        detail_get_received: AtomicUsize::new(0),
        detail_get_completed: AtomicUsize::new(0),
        feed_get_delay_ms: AtomicU64::new(0),
        feed_get_received: AtomicUsize::new(0),
        feed_get_completed: AtomicUsize::new(0),
        drafts_get_delay_ms: AtomicU64::new(0),
        drafts_get_received: AtomicUsize::new(0),
        drafts_get_completed: AtomicUsize::new(0),
        draft_published: AtomicBool::new(false),
        extra_posts: std::sync::Mutex::new(Vec::new()),
        create_post_delay_ms: AtomicU64::new(0),
        create_post_completed: AtomicUsize::new(0),
        update_post_delay_ms: AtomicU64::new(0),
        update_post_completed: AtomicUsize::new(0),
        delete_post_delay_ms: AtomicU64::new(0),
        delete_post_completed: AtomicUsize::new(0),
        create_comment_delay_ms: AtomicU64::new(0),
        create_comment_completed: AtomicUsize::new(0),
        signal_delay_ms: AtomicU64::new(0),
        signal_completed: AtomicUsize::new(0),
        summon_delay_ms: AtomicU64::new(0),
        summon_completed: AtomicUsize::new(0),
        comm_joined: AtomicBool::new(false),
        comm_members: AtomicU64::new(10),
        comm_wiki: std::sync::Mutex::new("Initial Wiki".to_string()),
        comm_wiki_rev: AtomicU32::new(1),
        comm_rules: std::sync::Mutex::new(Vec::new()),
        comm_automod_rules: std::sync::Mutex::new(Vec::new()),
        comm_get_delay_ms: AtomicU64::new(0),
        comm_get_received: AtomicUsize::new(0),
        comm_get_completed: AtomicUsize::new(0),
        join_delay_ms: AtomicU64::new(0),
        join_fail_500: AtomicBool::new(false),
        join_in_flight: AtomicUsize::new(0),
        join_in_flight_max: AtomicUsize::new(0),
        join_received: AtomicUsize::new(0),
        join_completed: AtomicUsize::new(0),
        comm_patch_delay_ms: AtomicU64::new(0),
        comm_patch_fail_500: AtomicBool::new(false),
        comm_patch_completed: AtomicUsize::new(0),
        wiki_put_delay_ms: AtomicU64::new(0),
        wiki_put_completed: AtomicUsize::new(0),
        mod_queue_items: std::sync::Mutex::new(seed_queue),
        mod_queue_get_delay_ms: AtomicU64::new(0),
        mod_queue_get_received: AtomicUsize::new(0),
        mod_queue_get_completed: AtomicUsize::new(0),
        mod_action_delay_ms: AtomicU64::new(0),
        mod_action_completed: AtomicUsize::new(0),
        lists: std::sync::Mutex::new(seed_lists),
        lists_get_delay_ms: AtomicU64::new(0),
        lists_get_received: AtomicUsize::new(0),
        lists_get_completed: AtomicUsize::new(0),
        list_editor_patch_delay_ms: AtomicU64::new(0),
        list_editor_patch_completed: AtomicUsize::new(0),
        list_toggle_patch_delay_ms: AtomicU64::new(0),
        list_toggle_patch_fail_500: AtomicBool::new(false),
        list_toggle_patch_received: AtomicUsize::new(0),
        list_toggle_patch_completed: AtomicUsize::new(0),
        list_delete_delay_ms: AtomicU64::new(0),
        list_delete_fail_500: AtomicBool::new(false),
        list_delete_received: AtomicUsize::new(0),
        list_delete_completed: AtomicUsize::new(0),
        notifs: std::sync::Mutex::new(seed_notifs),
        notifs_get_completed: AtomicUsize::new(0),
        notif_read_fail_500: AtomicBool::new(false),
        notif_read_override_unread: AtomicI64::new(-1),
        notif_read_completed: AtomicUsize::new(0),
        fail_second_flight: AtomicBool::new(false),
    });

    let std_listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind r8 mock");
    std_listener.set_nonblocking(true).expect("nonblocking");
    let addr = std_listener.local_addr().expect("local_addr");
    let state_loop = Arc::clone(&state);

    let join = rt_handle.spawn(async move {
        let listener = tokio::net::TcpListener::from_std(std_listener).expect("from_std");
        while let Ok((mut sock, _)) = listener.accept().await {
            let st = Arc::clone(&state_loop);
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
                let mut acting_actor: Option<String> = None;
                for line in lines {
                    if let Some((k, v)) = line.split_once(':') {
                        if k.trim().eq_ignore_ascii_case("content-length") {
                            content_len = v.trim().parse().unwrap_or(0);
                        } else if k.trim().eq_ignore_ascii_case("x-community-actor-id") {
                            acting_actor = Some(v.trim().to_string());
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

                let owner_actor = mock_actor_dto(&st.owner_id, ActorKind::Human, "r8_owner", "R8 Owner");
                let bot_a_name = st.bot_a_display_name.lock().unwrap().clone();
                let bot_a_actor = mock_actor_dto(&st.alt_id, ActorKind::Bot, "r8_bot_a", &bot_a_name);
                let bot_b_actor = mock_actor_dto(&st.bot_b_id, ActorKind::Bot, "r8_bot_b", "R8 Bot B");

                let (status, body) = if method == "GET" && path == "/api/v1/community/identities" {
                    let resp = CommunityIdentitiesResponse {
                        active_actor_id: st.owner_id.clone(),
                        human_actor: Some(owner_actor.clone()),
                        bot_actors: vec![bot_a_actor.clone(), bot_b_actor.clone()],
                        all_actors: vec![owner_actor, bot_a_actor, bot_b_actor],
                    };
                    ("200 OK", serde_json::to_string(&resp).unwrap())
                } else if method == "POST" && path == "/api/v1/community/identities/human" {
                    ("200 OK", serde_json::to_string(&owner_actor).unwrap())
                } else if method == "GET" && path == "/api/v1/community/actors/r8_bob" {
                    // Snapshot BEFORE optional delay so Case B slow GET returns stale pre-mutation state!
                    let mut actor = mock_actor_dto(&st.bob_id, ActorKind::Human, "r8_bob", "R8 Bob");
                    actor.is_muted = st.bob_muted.load(Ordering::SeqCst);
                    actor.is_blocked = st.bob_blocked.load(Ordering::SeqCst);
                    actor.is_following = st.bob_following.load(Ordering::SeqCst);
                    actor.followers_count = st.bob_followers_count.load(Ordering::SeqCst);
                    st.profile_get_received.fetch_add(1, Ordering::SeqCst);
                    let delay = st.profile_get_delay_ms.swap(0, Ordering::SeqCst);
                    if delay > 0 {
                        tokio::time::sleep(Duration::from_millis(delay)).await;
                    }
                    st.profile_get_completed.fetch_add(1, Ordering::SeqCst);
                    let resp = ActorProfileResponse {
                        actor,
                        recent_posts: Vec::new(),
                        recent_comments: Vec::new(),
                        moderated_communities: Vec::new(),
                    };
                    ("200 OK", serde_json::to_string(&resp).unwrap())
                } else if method == "GET" && path == "/api/v1/community/actors/r8_charlie" {
                    let actor = mock_actor_dto(&st.charlie_id, ActorKind::Human, "r8_charlie", "R8 Charlie");
                    let resp = ActorProfileResponse {
                        actor,
                        recent_posts: Vec::new(),
                        recent_comments: Vec::new(),
                        moderated_communities: Vec::new(),
                    };
                    ("200 OK", serde_json::to_string(&resp).unwrap())
                } else if method == "PATCH" && path.starts_with("/api/v1/community/actors/") {
                    let aid = path.trim_start_matches("/api/v1/community/actors/").to_string();
                    let req: UpdateCommunityActorRequest =
                        serde_json::from_slice(body_bytes).unwrap_or_default();
                    let delay = st.actor_patch_delay_ms.swap(0, Ordering::SeqCst);
                    if delay > 0 {
                        tokio::time::sleep(Duration::from_millis(delay)).await;
                    }
                    if aid == st.alt_id {
                        if let Some(dn) = req.display_name {
                            *st.bot_a_display_name.lock().unwrap() = dn;
                        }
                    }
                    st.actor_patch_completed.fetch_add(1, Ordering::SeqCst);
                    let updated = if aid == st.alt_id {
                        let dn = st.bot_a_display_name.lock().unwrap().clone();
                        mock_actor_dto(&st.alt_id, ActorKind::Bot, "r8_bot_a", &dn)
                    } else {
                        bot_b_actor
                    };
                    ("200 OK", serde_json::to_string(&updated).unwrap())
                } else if (method == "POST" || method == "DELETE")
                    && path == "/api/v1/community/mutes-blocks"
                {
                    let req: MuteBlockRequest = serde_json::from_slice(body_bytes).expect("mb req");
                    let is_mute = req.action == MuteBlockActionKind::Mute;
                    let delay = if is_mute {
                        st.mute_delay_ms.swap(0, Ordering::SeqCst)
                    } else {
                        st.block_delay_ms.swap(0, Ordering::SeqCst)
                    };
                    if delay > 0 {
                        tokio::time::sleep(Duration::from_millis(delay)).await;
                    }
                    let fail = if is_mute {
                        should_fail_r8(&st.mute_fail_500, &st.fail_second_flight)
                    } else {
                        should_fail_r8(&st.block_fail_500, &st.fail_second_flight)
                    };
                    if is_mute {
                        st.mute_completed.fetch_add(1, Ordering::SeqCst);
                    } else {
                        st.block_completed.fetch_add(1, Ordering::SeqCst);
                    }
                    if fail {
                        ("500 Internal Server Error", r#"{"error":"mb_500"}"#.to_string())
                    } else if method == "POST" {
                        if is_mute {
                            st.bob_muted.store(true, Ordering::SeqCst);
                        } else {
                            st.bob_blocked.store(true, Ordering::SeqCst);
                            st.bob_following.store(false, Ordering::SeqCst);
                        }
                        let dto = CommunityMuteBlockDto {
                            id: "mb_r8".to_string(),
                            owner_actor_id: st.owner_id.clone(),
                            target_kind: MuteBlockTargetKind::Actor,
                            target_value: req.target_value,
                            action: req.action,
                            reason: None,
                            created_at_ms: 1_750_000_000_000,
                        };
                        ("200 OK", serde_json::to_string(&dto).unwrap())
                    } else {
                        if is_mute {
                            st.bob_muted.store(false, Ordering::SeqCst);
                        } else {
                            st.bob_blocked.store(false, Ordering::SeqCst);
                        }
                        (
                            "200 OK",
                            serde_json::to_string(&DeleteEntityResponse {
                                id: req.target_value,
                                deleted: true,
                            })
                            .unwrap(),
                        )
                    }
                } else if (method == "POST" || method == "DELETE")
                    && path.starts_with("/api/v1/community/actors/")
                    && path.ends_with("/follow")
                {
                    let cur = st.follow_in_flight.fetch_add(1, Ordering::SeqCst) + 1;
                    st.follow_in_flight_max.fetch_max(cur, Ordering::SeqCst);
                    st.follow_received.fetch_add(1, Ordering::SeqCst);
                    let delay = st.follow_delay_ms.swap(0, Ordering::SeqCst);
                    if delay > 0 {
                        tokio::time::sleep(Duration::from_millis(delay)).await;
                    }
                    let res = if should_fail_r8(&st.follow_fail_500, &st.fail_second_flight) {
                        st.follow_completed.fetch_add(1, Ordering::SeqCst);
                        ("500 Internal Server Error", r#"{"error":"follow_500"}"#.to_string())
                    } else {
                        let want = method == "POST";
                        // Server rejects/ignores follow if blocked (just like SQLite store)
                        let effective = if st.bob_blocked.load(Ordering::SeqCst) {
                            false
                        } else {
                            let prev = st.bob_following.swap(want, Ordering::SeqCst);
                            if !prev && want {
                                st.bob_followers_count.fetch_add(1, Ordering::SeqCst);
                            } else if prev && !want {
                                st.bob_followers_count.fetch_sub(1, Ordering::SeqCst);
                            }
                            want
                        };
                        st.follow_completed.fetch_add(1, Ordering::SeqCst);
                        let resp = FollowActionResponse {
                            actor_id: st.bob_id.clone(),
                            is_following: effective,
                            followers_count: st.bob_followers_count.load(Ordering::SeqCst),
                        };
                        ("200 OK", serde_json::to_string(&resp).unwrap())
                    };
                    st.follow_in_flight.fetch_sub(1, Ordering::SeqCst);
                    res
                } else if method == "GET" && path == format!("/api/v1/community/posts/{}", st.post_id) {
                    if st.post_deleted.load(Ordering::SeqCst) {
                        st.detail_get_received.fetch_add(1, Ordering::SeqCst);
                        st.detail_get_completed.fetch_add(1, Ordering::SeqCst);
                        ("404 Not Found", r#"{"error":"deleted"}"#.to_string())
                    } else {
                        let resp = PostDetailResponse {
                            post: st.build_post(),
                            thread_chain: Vec::new(),
                            quote_chain: Vec::new(),
                            comments: vec![st.build_comment()],
                            total_comments: 1,
                        };
                        st.detail_get_received.fetch_add(1, Ordering::SeqCst);
                        let delay = st.detail_get_delay_ms.swap(0, Ordering::SeqCst);
                        if delay > 0 {
                            tokio::time::sleep(Duration::from_millis(delay)).await;
                        }
                        st.detail_get_completed.fetch_add(1, Ordering::SeqCst);
                        ("200 OK", serde_json::to_string(&resp).unwrap())
                    }
                } else if method == "GET" && path == format!("/api/v1/community/posts/{}", st.mpoll_id) {
                    let resp = PostDetailResponse {
                        post: st.build_mpoll_post(),
                        thread_chain: Vec::new(),
                        quote_chain: Vec::new(),
                        comments: Vec::new(),
                        total_comments: 0,
                    };
                    st.detail_get_received.fetch_add(1, Ordering::SeqCst);
                    let delay = st.detail_get_delay_ms.swap(0, Ordering::SeqCst);
                    if delay > 0 {
                        tokio::time::sleep(Duration::from_millis(delay)).await;
                    }
                    st.detail_get_completed.fetch_add(1, Ordering::SeqCst);
                    ("200 OK", serde_json::to_string(&resp).unwrap())
                } else if method == "POST" && path == format!("/api/v1/community/posts/{}/vote", st.post_id) {
                    let req: VoteRequest = serde_json::from_slice(body_bytes).expect("pv req");
                    let delay = st.post_vote_delay_ms.swap(0, Ordering::SeqCst);
                    if delay > 0 {
                        tokio::time::sleep(Duration::from_millis(delay)).await;
                    }
                    st.post_vote_completed.fetch_add(1, Ordering::SeqCst);
                    if should_fail_r8(&st.post_vote_fail_500, &st.fail_second_flight) {
                        ("500 Internal Server Error", r#"{"error":"pv_500"}"#.to_string())
                    } else {
                        st.post_vote_dir.store(req.direction, Ordering::SeqCst);
                        let resp = VoteResponse {
                            target_id: st.post_id.clone(),
                            direction: req.direction,
                            upvotes: i64::from(req.direction == 1),
                            downvotes: i64::from(req.direction == -1),
                            score: i64::from(req.direction),
                        };
                        ("200 OK", serde_json::to_string(&resp).unwrap())
                    }
                } else if method == "POST" && path == format!("/api/v1/community/posts/{}/like", st.post_id) {
                    let delay = st.post_like_delay_ms.swap(0, Ordering::SeqCst);
                    if delay > 0 {
                        tokio::time::sleep(Duration::from_millis(delay)).await;
                    }
                    st.post_like_completed.fetch_add(1, Ordering::SeqCst);
                    if should_fail_r8(&st.post_like_fail_500, &st.fail_second_flight) {
                        ("500 Internal Server Error", r#"{"error":"pl_500"}"#.to_string())
                    } else {
                        let next = !st.post_liked.load(Ordering::SeqCst);
                        st.post_liked.store(next, Ordering::SeqCst);
                        let resp = ToggleActionResponse {
                            target_id: st.post_id.clone(),
                            active: next,
                            count: u64::from(next),
                        };
                        ("200 OK", serde_json::to_string(&resp).unwrap())
                    }
                } else if method == "POST" && path == format!("/api/v1/community/posts/{}/repost", st.post_id) {
                    let cur = st.post_repost_in_flight.fetch_add(1, Ordering::SeqCst) + 1;
                    st.post_repost_in_flight_max.fetch_max(cur, Ordering::SeqCst);
                    st.post_repost_received.fetch_add(1, Ordering::SeqCst);
                    let delay = st.post_repost_delay_ms.swap(0, Ordering::SeqCst);
                    if delay > 0 {
                        tokio::time::sleep(Duration::from_millis(delay)).await;
                    }
                    let res = if should_fail_r8(&st.post_repost_fail_500, &st.fail_second_flight) {
                        st.post_repost_completed.fetch_add(1, Ordering::SeqCst);
                        ("500 Internal Server Error", r#"{"error":"repost_500"}"#.to_string())
                    } else {
                        let next = !st.post_reposted.load(Ordering::SeqCst);
                        st.post_reposted.store(next, Ordering::SeqCst);
                        st.post_repost_completed.fetch_add(1, Ordering::SeqCst);
                        let resp = RepostResponse {
                            post_id: st.post_id.clone(),
                            reposted: next,
                            reposts_count: u64::from(next),
                            repost_post_id: None,
                        };
                        ("200 OK", serde_json::to_string(&resp).unwrap())
                    };
                    st.post_repost_in_flight.fetch_sub(1, Ordering::SeqCst);
                    res
                } else if method == "POST" && path == format!("/api/v1/community/posts/{}/bookmark", st.post_id) {
                    let req: BookmarkRequest = serde_json::from_slice(body_bytes).expect("bm req");
                    let delay = st.post_bookmark_delay_ms.swap(0, Ordering::SeqCst);
                    if delay > 0 {
                        tokio::time::sleep(Duration::from_millis(delay)).await;
                    }
                    st.post_bookmark_completed.fetch_add(1, Ordering::SeqCst);
                    if should_fail_r8(&st.post_bookmark_fail_500, &st.fail_second_flight) {
                        ("500 Internal Server Error", r#"{"error":"bm_500"}"#.to_string())
                    } else {
                        let bookmarked = !req.remove.unwrap_or(false);
                        st.post_bookmarked.store(bookmarked, Ordering::SeqCst);
                        let resp = BookmarkResponse {
                            post_id: st.post_id.clone(),
                            bookmarked,
                            folder: req.folder.unwrap_or_else(|| "All".to_string()),
                            bookmarks_count: u64::from(bookmarked),
                        };
                        ("200 OK", serde_json::to_string(&resp).unwrap())
                    }
                } else if method == "POST" && path == format!("/api/v1/community/posts/{}/award", st.post_id) {
                    let req: GiveAwardRequest = serde_json::from_slice(body_bytes).expect("aw req");
                    st.post_award_received.fetch_add(1, Ordering::SeqCst);
                    let delay = st.post_award_delay_ms.swap(0, Ordering::SeqCst);
                    if delay > 0 {
                        tokio::time::sleep(Duration::from_millis(delay)).await;
                    }
                    let mut guard = st.post_awards.lock().unwrap();
                    let idx = guard.len() + 1;
                    guard.push(PostAwardDto {
                        award_id: format!("aw_r8_{idx}"),
                        award_kind: req.award_kind,
                        icon_emoji: "🚀".to_string(),
                        giver_actor_id: acting_actor.clone().unwrap_or_else(|| st.owner_id.clone()),
                        giver_handle: "r8_owner".to_string(),
                        message: None,
                        created_at_ms: 1_750_000_002_000,
                    });
                    let awards = guard.clone();
                    drop(guard);
                    st.post_award_completed.fetch_add(1, Ordering::SeqCst);
                    let resp = AwardResponse {
                        target_id: st.post_id.clone(),
                        awards,
                    };
                    ("200 OK", serde_json::to_string(&resp).unwrap())
                } else if method == "POST"
                    && (path == format!("/api/v1/community/posts/{}/poll-vote", st.post_id)
                        || path == format!("/api/v1/community/posts/{}/poll-vote", st.mpoll_id))
                {
                    let is_multi = path.contains(&st.mpoll_id);
                    let req: CommunityPollVoteRequest =
                        serde_json::from_slice(body_bytes).expect("poll req");
                    let cur = st.poll_vote_in_flight.fetch_add(1, Ordering::SeqCst) + 1;
                    st.poll_vote_in_flight_max.fetch_max(cur, Ordering::SeqCst);
                    st.poll_vote_received.fetch_add(1, Ordering::SeqCst);
                    let delay = st.poll_vote_delay_ms.swap(0, Ordering::SeqCst);
                    if delay > 0 {
                        tokio::time::sleep(Duration::from_millis(delay)).await;
                    }
                    let res = if should_fail_r8(&st.poll_vote_fail_500, &st.fail_second_flight) {
                        st.poll_vote_completed.fetch_add(1, Ordering::SeqCst);
                        ("500 Internal Server Error", r#"{"error":"poll_500"}"#.to_string())
                    } else {
                        if is_multi {
                            *st.multi_poll_voted.lock().unwrap() = req.option_ids;
                            st.poll_vote_completed.fetch_add(1, Ordering::SeqCst);
                            let poll = st.build_mpoll_post().poll.unwrap();
                            ("200 OK", serde_json::to_string(&poll).unwrap())
                        } else {
                            *st.single_poll_voted.lock().unwrap() = req.option_ids;
                            st.poll_vote_completed.fetch_add(1, Ordering::SeqCst);
                            let poll = st.build_post().poll.unwrap();
                            ("200 OK", serde_json::to_string(&poll).unwrap())
                        }
                    };
                    st.poll_vote_in_flight.fetch_sub(1, Ordering::SeqCst);
                    res
                } else if method == "POST"
                    && path == format!("/api/v1/community/comments/{}/vote", st.comment_id)
                {
                    let req: VoteRequest = serde_json::from_slice(body_bytes).expect("cv req");
                    let delay = st.comment_vote_delay_ms.swap(0, Ordering::SeqCst);
                    if delay > 0 {
                        tokio::time::sleep(Duration::from_millis(delay)).await;
                    }
                    st.comment_vote_completed.fetch_add(1, Ordering::SeqCst);
                    if should_fail_r8(&st.comment_vote_fail_500, &st.fail_second_flight) {
                        ("500 Internal Server Error", r#"{"error":"cv_500"}"#.to_string())
                    } else {
                        st.comment_vote_dir.store(req.direction, Ordering::SeqCst);
                        let resp = VoteResponse {
                            target_id: st.comment_id.clone(),
                            direction: req.direction,
                            upvotes: i64::from(req.direction == 1),
                            downvotes: i64::from(req.direction == -1),
                            score: i64::from(req.direction),
                        };
                        ("200 OK", serde_json::to_string(&resp).unwrap())
                    }
                } else if method == "POST"
                    && path == format!("/api/v1/community/comments/{}/like", st.comment_id)
                {
                    let delay = st.comment_like_delay_ms.swap(0, Ordering::SeqCst);
                    if delay > 0 {
                        tokio::time::sleep(Duration::from_millis(delay)).await;
                    }
                    st.comment_like_completed.fetch_add(1, Ordering::SeqCst);
                    if should_fail_r8(&st.comment_like_fail_500, &st.fail_second_flight) {
                        ("500 Internal Server Error", r#"{"error":"cl_500"}"#.to_string())
                    } else {
                        let next = !st.comment_liked.load(Ordering::SeqCst);
                        st.comment_liked.store(next, Ordering::SeqCst);
                        let resp = ToggleActionResponse {
                            target_id: st.comment_id.clone(),
                            active: next,
                            count: u64::from(next),
                        };
                        ("200 OK", serde_json::to_string(&resp).unwrap())
                    }
                } else if method == "POST" && path == "/api/v1/community/posts" {
                    let req: CreatePostRequest = serde_json::from_slice(body_bytes).expect("cp req");
                    let delay = st.create_post_delay_ms.swap(0, Ordering::SeqCst);
                    if delay > 0 {
                        tokio::time::sleep(Duration::from_millis(delay)).await;
                    }
                    let mut guard = st.extra_posts.lock().unwrap();
                    let idx = guard.len() + 1;
                    let mut created = mock_post_dto(
                        &format!("p_r8_new_{idx}"),
                        acting_actor.as_deref().unwrap_or(&st.owner_id),
                        false,
                        None,
                        0,
                    );
                    created.title = req.title;
                    created.body = req.body;
                    created.status = req.status;
                    created.community_slug = req.community_slug;
                    guard.insert(0, created.clone());
                    drop(guard);
                    st.create_post_completed.fetch_add(1, Ordering::SeqCst);
                    let resp = CreatePostResponse {
                        post: created,
                        thread_posts: Vec::new(),
                        automod_action: AutoModActionKind::Approve,
                        automod_note: None,
                    };
                    ("200 OK", serde_json::to_string(&resp).unwrap())
                } else if method == "PATCH" && path.starts_with("/api/v1/community/posts/") {
                    let pid = path.trim_start_matches("/api/v1/community/posts/").to_string();
                    let req: UpdatePostRequest = serde_json::from_slice(body_bytes).expect("up req");
                    let delay = st.update_post_delay_ms.swap(0, Ordering::SeqCst);
                    if delay > 0 {
                        tokio::time::sleep(Duration::from_millis(delay)).await;
                    }
                    let updated = if pid == st.draft_id {
                        if req.status == Some(PostStatus::Published) {
                            st.draft_published.store(true, Ordering::SeqCst);
                        }
                        st.build_draft_post()
                    } else {
                        if let Some(b) = req.body {
                            *st.post_body.lock().unwrap() = b;
                        }
                        st.build_post()
                    };
                    st.update_post_completed.fetch_add(1, Ordering::SeqCst);
                    ("200 OK", serde_json::to_string(&updated).unwrap())
                } else if method == "DELETE" && path == format!("/api/v1/community/posts/{}", st.post_id) {
                    let delay = st.delete_post_delay_ms.swap(0, Ordering::SeqCst);
                    if delay > 0 {
                        tokio::time::sleep(Duration::from_millis(delay)).await;
                    }
                    st.post_deleted.store(true, Ordering::SeqCst);
                    st.delete_post_completed.fetch_add(1, Ordering::SeqCst);
                    (
                        "200 OK",
                        serde_json::to_string(&DeleteEntityResponse {
                            id: st.post_id.clone(),
                            deleted: true,
                        })
                        .unwrap(),
                    )
                } else if method == "POST"
                    && path == format!("/api/v1/community/posts/{}/comments", st.post_id)
                {
                    let delay = st.create_comment_delay_ms.swap(0, Ordering::SeqCst);
                    if delay > 0 {
                        tokio::time::sleep(Duration::from_millis(delay)).await;
                    }
                    st.create_comment_completed.fetch_add(1, Ordering::SeqCst);
                    let c = st.build_comment();
                    ("200 OK", serde_json::to_string(&c).unwrap())
                } else if method == "POST" && path == "/api/v1/community/signals" {
                    let delay = st.signal_delay_ms.swap(0, Ordering::SeqCst);
                    if delay > 0 {
                        tokio::time::sleep(Duration::from_millis(delay)).await;
                    }
                    st.signal_completed.fetch_add(1, Ordering::SeqCst);
                    ("200 OK", r#"{"ok":true}"#.to_string())
                } else if method == "POST"
                    && path == format!("/api/v1/community/posts/{}/summon-bot", st.post_id)
                {
                    let delay = st.summon_delay_ms.swap(0, Ordering::SeqCst);
                    if delay > 0 {
                        tokio::time::sleep(Duration::from_millis(delay)).await;
                    }
                    st.summon_completed.fetch_add(1, Ordering::SeqCst);
                    let resp = SummonBotResponse {
                        summoned_bot_actor_id: st.alt_id.clone(),
                        triggered: true,
                        reason: "ok".to_string(),
                        created_comment: None,
                    };
                    ("200 OK", serde_json::to_string(&resp).unwrap())
                } else if method == "GET" && path == "/api/v1/community/communities" {
                    let resp = ListCommunitiesResponse {
                        communities: vec![st.build_community(acting_actor.as_deref())],
                    };
                    ("200 OK", serde_json::to_string(&resp).unwrap())
                } else if method == "GET" && path == format!("/api/v1/community/communities/{}", st.slug) {
                    let comm = st.build_community(acting_actor.as_deref());
                    let resp = CommunityDetailResponse {
                        community: comm,
                        pinned_posts: Vec::new(),
                        wiki_revisions: Vec::new(),
                        moderators: Vec::new(),
                    };
                    st.comm_get_received.fetch_add(1, Ordering::SeqCst);
                    let delay = st.comm_get_delay_ms.swap(0, Ordering::SeqCst);
                    if delay > 0 {
                        tokio::time::sleep(Duration::from_millis(delay)).await;
                    }
                    st.comm_get_completed.fetch_add(1, Ordering::SeqCst);
                    ("200 OK", serde_json::to_string(&resp).unwrap())
                } else if (method == "POST" || method == "DELETE")
                    && path == format!("/api/v1/community/communities/{}/join", st.slug)
                {
                    let cur = st.join_in_flight.fetch_add(1, Ordering::SeqCst) + 1;
                    st.join_in_flight_max.fetch_max(cur, Ordering::SeqCst);
                    st.join_received.fetch_add(1, Ordering::SeqCst);
                    let delay = st.join_delay_ms.swap(0, Ordering::SeqCst);
                    if delay > 0 {
                        tokio::time::sleep(Duration::from_millis(delay)).await;
                    }
                    let res = if should_fail_r8(&st.join_fail_500, &st.fail_second_flight) {
                        st.join_completed.fetch_add(1, Ordering::SeqCst);
                        ("500 Internal Server Error", r#"{"error":"join_500"}"#.to_string())
                    } else {
                        let want = method == "POST";
                        let prev = st.comm_joined.swap(want, Ordering::SeqCst);
                        if !prev && want {
                            st.comm_members.fetch_add(1, Ordering::SeqCst);
                        } else if prev && !want {
                            st.comm_members.fetch_sub(1, Ordering::SeqCst);
                        }
                        st.join_completed.fetch_add(1, Ordering::SeqCst);
                        let is_owner = acting_actor.as_deref().is_none_or(|a| a == st.owner_id);
                        let resp = JoinCommunityResponse {
                            community_slug: st.slug.clone(),
                            is_joined: want,
                            member_count: st.comm_members.load(Ordering::SeqCst),
                            viewer_role: if want && is_owner {
                                Some(CommunityMemberRole::Mod)
                            } else if want {
                                Some(CommunityMemberRole::Member)
                            } else {
                                None
                            },
                        };
                        ("200 OK", serde_json::to_string(&resp).unwrap())
                    };
                    st.join_in_flight.fetch_sub(1, Ordering::SeqCst);
                    res
                } else if method == "PATCH" && path == format!("/api/v1/community/communities/{}", st.slug) {
                    let req: UpdateCommunityRequest = serde_json::from_slice(body_bytes).expect("uc req");
                    let delay = st.comm_patch_delay_ms.swap(0, Ordering::SeqCst);
                    if delay > 0 {
                        tokio::time::sleep(Duration::from_millis(delay)).await;
                    }
                    st.comm_patch_completed.fetch_add(1, Ordering::SeqCst);
                    if should_fail_r8(&st.comm_patch_fail_500, &st.fail_second_flight) {
                        ("500 Internal Server Error", r#"{"error":"comm_patch_500"}"#.to_string())
                    } else {
                        if let Some(rules) = req.rules {
                            *st.comm_rules.lock().unwrap() = rules;
                        }
                        if let Some(am) = req.automod_rules {
                            *st.comm_automod_rules.lock().unwrap() = am;
                        }
                        let comm = st.build_community(acting_actor.as_deref());
                        ("200 OK", serde_json::to_string(&comm).unwrap())
                    }
                } else if method == "PUT" && path == format!("/api/v1/community/communities/{}/wiki", st.slug) {
                    let req: UpdateCommunityWikiRequest =
                        serde_json::from_slice(body_bytes).expect("wiki req");
                    let delay = st.wiki_put_delay_ms.swap(0, Ordering::SeqCst);
                    if delay > 0 {
                        tokio::time::sleep(Duration::from_millis(delay)).await;
                    }
                    *st.comm_wiki.lock().unwrap() = req.markdown.clone();
                    let rev_num = st.comm_wiki_rev.fetch_add(1, Ordering::SeqCst) + 1;
                    st.wiki_put_completed.fetch_add(1, Ordering::SeqCst);
                    let rev = CommunityWikiRevisionDto {
                        revision: rev_num,
                        markdown: req.markdown,
                        editor_actor_id: st.owner_id.clone(),
                        editor_handle: "r8_owner".to_string(),
                        summary: req.summary,
                        created_at_ms: 1_750_000_002_000,
                    };
                    ("200 OK", serde_json::to_string(&rev).unwrap())
                } else if method == "GET" && path == format!("/api/v1/community/communities/{}/mod-queue", st.slug) {
                    let items = st.mod_queue_items.lock().unwrap().clone();
                    st.mod_queue_get_received.fetch_add(1, Ordering::SeqCst);
                    let delay = st.mod_queue_get_delay_ms.swap(0, Ordering::SeqCst);
                    if delay > 0 {
                        tokio::time::sleep(Duration::from_millis(delay)).await;
                    }
                    st.mod_queue_get_completed.fetch_add(1, Ordering::SeqCst);
                    ("200 OK", serde_json::to_string(&ModQueueResponse { items }).unwrap())
                } else if method == "GET" && path == format!("/api/v1/community/communities/{}/mod-logs", st.slug) {
                    ("200 OK", serde_json::to_string(&ModLogsResponse { logs: Vec::new() }).unwrap())
                } else if method == "POST" && path == format!("/api/v1/community/communities/{}/mod-actions", st.slug) {
                    let req: ModActionRequest = serde_json::from_slice(body_bytes).expect("ma req");
                    let delay = st.mod_action_delay_ms.swap(0, Ordering::SeqCst);
                    if delay > 0 {
                        tokio::time::sleep(Duration::from_millis(delay)).await;
                    }
                    if let Some(qid) = req.queue_item_id {
                        st.mod_queue_items.lock().unwrap().retain(|it| it.item_id != qid);
                    }
                    st.mod_action_completed.fetch_add(1, Ordering::SeqCst);
                    let entry = ModLogEntryDto {
                        log_id: "ml_r8".to_string(),
                        community_id: format!("comm_{}", st.slug),
                        moderator_actor_id: st.owner_id.clone(),
                        moderator_handle: "r8_owner".to_string(),
                        moderator_kind: ActorKind::Human,
                        action: req.action,
                        target_kind: req.target_kind,
                        target_id: req.target_id,
                        reason: req.reason,
                        rule_triggered: None,
                        created_at_ms: 1_750_000_002_000,
                    };
                    ("200 OK", serde_json::to_string(&entry).unwrap())
                } else if method == "GET" && path == "/api/v1/community/lists" {
                    st.lists_get_received.fetch_add(1, Ordering::SeqCst);
                    let delay = st.lists_get_delay_ms.swap(0, Ordering::SeqCst);
                    if delay > 0 {
                        tokio::time::sleep(Duration::from_millis(delay)).await;
                    }
                    let snapshot = st.lists.lock().unwrap().clone();
                    st.lists_get_completed.fetch_add(1, Ordering::SeqCst);
                    ("200 OK", serde_json::to_string(&CommunityListsResponse { lists: snapshot }).unwrap())
                } else if method == "PATCH" && path.starts_with("/api/v1/community/lists/") {
                    let lid = path.trim_start_matches("/api/v1/community/lists/").to_string();
                    let req: UpdateCommunityListRequest =
                        serde_json::from_slice(body_bytes).expect("list patch");
                    let is_editor = req.name.is_some();
                    if is_editor {
                        let delay = st.list_editor_patch_delay_ms.swap(0, Ordering::SeqCst);
                        if delay > 0 {
                            tokio::time::sleep(Duration::from_millis(delay)).await;
                        }
                    } else {
                        st.list_toggle_patch_received.fetch_add(1, Ordering::SeqCst);
                        let delay = st.list_toggle_patch_delay_ms.swap(0, Ordering::SeqCst);
                        if delay > 0 {
                            tokio::time::sleep(Duration::from_millis(delay)).await;
                        }
                        if st.list_toggle_patch_fail_500.load(Ordering::SeqCst) {
                            st.list_toggle_patch_completed.fetch_add(1, Ordering::SeqCst);
                            let resp_bytes = "HTTP/1.1 500 Internal Server Error\r\nContent-Type: application/json\r\nContent-Length: 21\r\nConnection: close\r\n\r\n{\"error\":\"lt_500_r8\"}";
                            let _ = sock.write_all(resp_bytes.as_bytes()).await;
                            let _ = sock.shutdown().await;
                            return;
                        }
                    }
                    let updated = {
                        let mut guard = st.lists.lock().unwrap();
                        let entry = guard
                            .iter_mut()
                            .find(|l| l.list_id == lid)
                            .expect("list exists");
                        if let Some(n) = req.name {
                            entry.name = n;
                        }
                        if let Some(d) = req.description {
                            entry.description = d;
                        }
                        if let Some(p) = req.is_pinned {
                            entry.is_pinned = p;
                        }
                        if let Some(m) = req.member_actor_ids {
                            entry.member_actor_ids = m;
                        }
                        if let Some(c) = req.community_slugs {
                            entry.community_slugs = c;
                        }
                        entry.clone()
                    };
                    if is_editor {
                        st.list_editor_patch_completed.fetch_add(1, Ordering::SeqCst);
                    } else {
                        st.list_toggle_patch_completed.fetch_add(1, Ordering::SeqCst);
                    }
                    ("200 OK", serde_json::to_string(&updated).unwrap())
                } else if method == "DELETE" && path.starts_with("/api/v1/community/lists/") {
                    let lid = path.trim_start_matches("/api/v1/community/lists/").to_string();
                    st.list_delete_received.fetch_add(1, Ordering::SeqCst);
                    let delay = st.list_delete_delay_ms.swap(0, Ordering::SeqCst);
                    if delay > 0 {
                        tokio::time::sleep(Duration::from_millis(delay)).await;
                    }
                    st.list_delete_completed.fetch_add(1, Ordering::SeqCst);
                    if st.list_delete_fail_500.load(Ordering::SeqCst) {
                        ("500 Internal Server Error", r#"{"error":"del_500"}"#.to_string())
                    } else {
                        st.lists.lock().unwrap().retain(|l| l.list_id != lid);
                        (
                            "200 OK",
                            serde_json::to_string(&DeleteEntityResponse {
                                id: lid,
                                deleted: true,
                            })
                            .unwrap(),
                        )
                    }
                } else if method == "GET" && path == "/api/v1/community/notifications" {
                    let items = st.notifs.lock().unwrap().clone();
                    let unread = items.iter().filter(|n| !n.is_read).count() as u64;
                    st.notifs_get_completed.fetch_add(1, Ordering::SeqCst);
                    let resp = NotificationsListResponse {
                        notifications: items,
                        unread_count: unread,
                    };
                    ("200 OK", serde_json::to_string(&resp).unwrap())
                } else if method == "POST" && path == "/api/v1/community/notifications/read" {
                    let req: MarkNotificationsReadRequest =
                        serde_json::from_slice(body_bytes).unwrap_or_default();
                    st.notif_read_completed.fetch_add(1, Ordering::SeqCst);
                    if st.notif_read_fail_500.load(Ordering::SeqCst) {
                        ("500 Internal Server Error", r#"{"error":"notif_500"}"#.to_string())
                    } else {
                        let mut guard = st.notifs.lock().unwrap();
                        let mut marked = 0u64;
                        match req.notif_ids {
                            Some(ids) => {
                                for n in guard.iter_mut() {
                                    if ids.iter().any(|id| id == &n.notif_id) && !n.is_read {
                                        n.is_read = true;
                                        marked += 1;
                                    }
                                }
                            }
                            None => {
                                for n in guard.iter_mut() {
                                    if !n.is_read {
                                        n.is_read = true;
                                        marked += 1;
                                    }
                                }
                            }
                        }
                        let actual_unread = guard.iter().filter(|n| !n.is_read).count() as u64;
                        drop(guard);
                        let ov = st.notif_read_override_unread.swap(-1, Ordering::SeqCst);
                        let unread_count = if ov >= 0 { ov as u64 } else { actual_unread };
                        let resp = MarkNotificationsReadResponse {
                            marked_count: marked,
                            unread_count,
                        };
                        ("200 OK", serde_json::to_string(&resp).unwrap())
                    }
                } else if method == "GET" && path == "/api/v1/community/feed" {
                    let is_actor_posts = query.contains("actor=");
                    let is_saved = query.contains("tab=saved");
                    let mut items = Vec::new();
                    if is_actor_posts {
                        items.push(FeedItemDto {
                            post: st.build_draft_post(),
                            explanation: None,
                            rank_score: 10.0,
                            is_exploration_slot: false,
                        });
                        st.drafts_get_received.fetch_add(1, Ordering::SeqCst);
                        let delay = st.drafts_get_delay_ms.swap(0, Ordering::SeqCst);
                        if delay > 0 {
                            tokio::time::sleep(Duration::from_millis(delay)).await;
                        }
                        st.drafts_get_completed.fetch_add(1, Ordering::SeqCst);
                    } else if is_saved {
                        if !st.post_deleted.load(Ordering::SeqCst)
                            && st.post_bookmarked.load(Ordering::SeqCst)
                        {
                            items.push(FeedItemDto {
                                post: st.build_post(),
                                explanation: None,
                                rank_score: 100.0,
                                is_exploration_slot: false,
                            });
                        }
                    } else {
                        for ep in st.extra_posts.lock().unwrap().iter() {
                            items.push(FeedItemDto {
                                post: ep.clone(),
                                explanation: None,
                                rank_score: 110.0,
                                is_exploration_slot: false,
                            });
                        }
                        if !st.post_deleted.load(Ordering::SeqCst) {
                            items.push(FeedItemDto {
                                post: st.build_post(),
                                explanation: None,
                                rank_score: 100.0,
                                is_exploration_slot: false,
                            });
                        }
                        items.push(FeedItemDto {
                            post: st.build_mpoll_post(),
                            explanation: None,
                            rank_score: 90.0,
                            is_exploration_slot: false,
                        });
                        st.feed_get_received.fetch_add(1, Ordering::SeqCst);
                        let delay = st.feed_get_delay_ms.swap(0, Ordering::SeqCst);
                        if delay > 0 {
                            tokio::time::sleep(Duration::from_millis(delay)).await;
                        }
                        st.feed_get_completed.fetch_add(1, Ordering::SeqCst);
                    }
                    let resp = FeedResponseDto {
                        items,
                        next_cursor: None,
                        total_candidates: 0,
                        applied_preset: Default::default(),
                        generated_at_ms: 1_750_000_000_000,
                    };
                    ("200 OK", serde_json::to_string(&resp).unwrap())
                } else if method == "GET" && path == "/api/v1/community/algo-prefs" {
                    ("200 OK", serde_json::to_string(&AlgoPreferencesDto::default()).unwrap())
                } else if method == "GET" && path == "/api/v1/community/trending" {
                    (
                        "200 OK",
                        serde_json::to_string(&TrendingResponse {
                            topics: Vec::new(),
                            generated_at_ms: 1_750_000_000_000,
                        })
                        .unwrap(),
                    )
                } else if method == "GET" && path == "/api/v1/community/search" {
                    ("200 OK", serde_json::to_string(&CommunitySearchResultDto::default()).unwrap())
                } else {
                    ("404 Not Found", r#"{"error":"not_found"}"#.to_string())
                };

                let http = format!(
                    "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                );
                let _ = sock.write_all(http.as_bytes()).await;
                let _ = sock.shutdown().await;
            });
        }
    });

    (addr, state, AbortOnDrop(join))
}

#[test]
fn test_m5_r8_universal_consistency_and_secondary_mutations() {
    run_in_headless_subprocess("test_m5_r8_universal_consistency_and_secondary_mutations");
}

async fn body_r8_universal_consistency_and_secondary_mutations(mut app: AsyncApp) {
    let dir = TempDir::new().expect("r8 consistency tempdir");
    let rt_dir = TempDir::new().expect("r8 consistency rt tempdir");
    let rt_holder = TelegramClient::open_with_dir(
        ClientConfig {
            server_url: "http://127.0.0.1:9".to_string(),
            max_reconnect_attempts: 0,
            ..Default::default()
        },
        rt_dir.path(),
    )
    .expect("open r8 rt holder");
    let (addr, srv, _server_guard) = spawn_r8_universal_server(rt_holder.runtime().handle());

    let client = Arc::new(
        TelegramClient::open_with_dir(
            ClientConfig {
                server_url: format!("http://{addr}"),
                max_reconnect_attempts: 0,
                ..Default::default()
            },
            dir.path(),
        )
        .expect("open r8 client"),
    );
    let auth = test_auth(908001, "r8_owner");
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
        .expect("mount r8 AppShell");
    let _ = shell.update(&mut app, |s, cx| s.open_community_workspace(cx));
    let cw = shell
        .read_with(&app, |s, _| s.community_workspace.clone().unwrap())
        .unwrap();

    assert!(
        pump_until(&mut app, Duration::from_secs(5), |cx| {
            let st = &cw.read(cx).state;
            !st.feed_loading
                && st.identities.is_some()
                && !st.communities.is_empty()
                && st.feed_items.iter().any(|it| it.post.post_id == srv.post_id)
        })
        .await,
        "R8 host must bootstrap feed, identities, and communities"
    );

    run_r8_group_a_list_and_automod(&mut app, &cw, &srv).await;
    run_r8_group_b_case_a_and_error_case_b(&mut app, &cw, &srv).await;
    run_r8_group_c_four_new_single_flight(&mut app, &cw, &srv).await;
    run_r8_group_d_award_fences_and_secondary(&mut app, &shell, &cw, &srv).await;

    drop(rt_holder);
}

async fn run_r8_group_a_list_and_automod(
    app: &mut AsyncApp,
    cw: &Entity<CommunityWorkspace>,
    srv: &Arc<R8MockState>,
) {
    use std::sync::atomic::Ordering;

    let _ = cw.update(app, |w, cx| {
        w.navigate_to(CommunityRoute::BookmarksLists, cx);
        w.set_bookmarks_section(BookmarksListsSection::Lists, cx);
    });
    assert!(
        pump_until(app, Duration::from_secs(4), |cx| {
            let st = &cw.read(cx).state;
            !st.saved_loading && st.lists.iter().any(|l| l.list_id == "l_3e")
        })
        .await,
        "Group A: initial lists must load"
    );

    // T8-A1 (2F_3E / R1 2F_3G Variant A): pre-submit in-flight + dirty quick-toggles settle
    // during submit_list_editor's delayed GET /lists, followed by community toggle -> 0 data loss.
    let lt_done_a1 = srv.list_toggle_patch_completed.load(Ordering::SeqCst);
    let le_done_a1 = srv.list_editor_patch_completed.load(Ordering::SeqCst);
    let lg_done_a1 = srv.lists_get_completed.load(Ordering::SeqCst);
    srv.list_toggle_patch_delay_ms.store(90, Ordering::SeqCst);
    srv.list_editor_patch_delay_ms.store(30, Ordering::SeqCst);
    srv.lists_get_delay_ms.store(260, Ordering::SeqCst);
    let _ = cw.update(app, |w, cx| {
        let target = w.state.lists.iter().find(|l| l.list_id == "l_3e").cloned().unwrap();
        w.open_edit_list_form(target, cx);
        w.state.list_editor.name = "Renamed 3E".to_string();
        w.toggle_list_pinned("l_3e".to_string(), true, cx);
        w.toggle_selected_list_actor("l_3e".to_string(), "u:300".to_string(), cx);
        w.submit_list_editor(cx);
        // R8-5 mutex check: attempting to open create/edit form while submitting must be ignored!
        w.open_create_list_form(cx);
        assert!(
            w.state.list_editor.submitting && w.state.list_editor.name == "Renamed 3E",
            "R8-5: open_create_list_form must be blocked while submitting"
        );
        let dummy = w.state.lists.iter().find(|l| l.list_id == "l_3g").cloned().unwrap();
        w.open_edit_list_form(dummy, cx);
        assert!(
            w.state.list_editor.submitting && w.state.list_editor.name == "Renamed 3E",
            "R8-5: open_edit_list_form must be blocked while submitting"
        );
    });
    assert!(
        pump_until(app, Duration::from_secs(5), |cx| {
            let st = &cw.read(cx).state;
            !st.list_editor.submitting
                && st.list_membership_in_flight.is_empty()
                && st.list_membership_dirty.is_empty()
                && srv.list_editor_patch_completed.load(Ordering::SeqCst) > le_done_a1
                && srv.lists_get_completed.load(Ordering::SeqCst) > lg_done_a1
                && srv.list_toggle_patch_completed.load(Ordering::SeqCst) >= lt_done_a1 + 2
        })
        .await,
        "T8-A1: editor + pre-submit quick-toggles + any re-flush must settle"
    );
    let _ = cw.update(app, |w, cx| {
        w.toggle_selected_list_community("l_3e".to_string(), "rust".to_string(), cx);
    });
    assert!(
        pump_until(app, Duration::from_secs(4), |cx| {
            let st = &cw.read(cx).state;
            st.list_membership_in_flight.is_empty() && st.list_membership_dirty.is_empty()
        })
        .await
    );
    let _ = cw.read_with(app, |w, _| {
        let ui = w.state.lists.iter().find(|l| l.list_id == "l_3e").unwrap();
        let sv = srv.lists.lock().unwrap().iter().find(|l| l.list_id == "l_3e").cloned().unwrap();
        assert_eq!(ui.name, "Renamed 3E");
        assert!(ui.is_pinned && ui.member_actor_ids == vec!["u:300".to_string()] && ui.community_slugs == vec!["rust".to_string()]);
        assert_eq!(sv.name, "Renamed 3E");
        assert!(sv.is_pinned && sv.member_actor_ids == vec!["u:300".to_string()] && sv.community_slugs == vec!["rust".to_string()]);
    });

    // T8-A2 (R1 2F_3G Variant B): editor commits ("Renamed B", is_pinned=true) -> mid-GET /lists,
    // quick-toggle fails HTTP 500 & rolls back -> toggle_seq NOT bumped -> editor save NOT reverted!
    let lg_rx_a2 = srv.lists_get_received.load(Ordering::SeqCst);
    let lg_done_a2 = srv.lists_get_completed.load(Ordering::SeqCst);
    let lt_done_a2 = srv.list_toggle_patch_completed.load(Ordering::SeqCst);
    srv.lists_get_delay_ms.store(240, Ordering::SeqCst);
    srv.list_toggle_patch_fail_500.store(true, Ordering::SeqCst);
    let _ = cw.update(app, |w, cx| {
        let target = w.state.lists.iter().find(|l| l.list_id == "l_3g_b").cloned().unwrap();
        w.open_edit_list_form(target, cx);
        w.state.list_editor.name = "Renamed B".to_string();
        w.state.list_editor.is_pinned = true;
        w.submit_list_editor(cx);
    });
    assert!(
        pump_until(app, Duration::from_secs(4), |_| {
            srv.lists_get_received.load(Ordering::SeqCst) > lg_rx_a2
        })
        .await
    );
    let _ = cw.update(app, |w, cx| {
        w.toggle_selected_list_actor("l_3g_b".to_string(), "u:999".to_string(), cx);
    });
    assert!(
        pump_until(app, Duration::from_secs(5), |cx| {
            let st = &cw.read(cx).state;
            !st.list_editor.submitting
                && st.list_membership_in_flight.is_empty()
                && st.list_membership_dirty.is_empty()
                && srv.list_toggle_patch_completed.load(Ordering::SeqCst) > lt_done_a2
                && srv.lists_get_completed.load(Ordering::SeqCst) > lg_done_a2
        })
        .await,
        "T8-A2: failed quick-toggle + editor GET /lists must settle"
    );
    srv.list_toggle_patch_fail_500.store(false, Ordering::SeqCst);
    let _ = cw.read_with(app, |w, _| {
        let ui = w.state.lists.iter().find(|l| l.list_id == "l_3g_b").unwrap();
        let sv = srv.lists.lock().unwrap().iter().find(|l| l.list_id == "l_3g_b").cloned().unwrap();
        assert_eq!(ui.name, "Renamed B");
        assert!(ui.is_pinned && ui.member_actor_ids.is_empty(), "T8-A2: failed toggle must NOT revert editor is_pinned=true");
        assert_eq!(sv.name, "Renamed B");
        assert!(sv.is_pinned && sv.member_actor_ids.is_empty());
    });

    // T8-A3 (2F_3G delete-500 dirty re-flush + R8-3 merge_refreshed_lists + R8-4 edit during delete-500)
    let del_done_a3 = srv.list_delete_completed.load(Ordering::SeqCst);
    let lt_done_a3 = srv.list_toggle_patch_completed.load(Ordering::SeqCst);
    srv.list_toggle_patch_delay_ms.store(90, Ordering::SeqCst);
    srv.list_delete_delay_ms.store(240, Ordering::SeqCst);
    srv.list_delete_fail_500.store(true, Ordering::SeqCst);
    let _ = cw.update(app, |w, cx| {
        w.toggle_list_pinned("l_3g".to_string(), true, cx);
        w.toggle_selected_list_actor("l_3g".to_string(), "u:300".to_string(), cx);
        w.delete_list_action("l_3g".to_string(), cx);
        // R8-3: simulate GET /lists finishing while list_deleting contains l_3g
        let snap = srv.lists.lock().unwrap().clone();
        w.state.merge_refreshed_lists(snap);
        assert!(
            w.state.lists.iter().any(|l| l.list_id == "l_3g"),
            "R8-3: merge_refreshed_lists must NOT prematurely drop a deleting list before DELETE settles"
        );
    });
    assert!(
        pump_until(app, Duration::from_secs(5), |cx| {
            let st = &cw.read(cx).state;
            !st.list_deleting.contains("l_3g")
                && st.list_membership_in_flight.is_empty()
                && st.list_membership_dirty.is_empty()
                && srv.list_delete_completed.load(Ordering::SeqCst) > del_done_a3
                && srv.list_toggle_patch_completed.load(Ordering::SeqCst) >= lt_done_a3 + 2
        })
        .await,
        "T8-A3: DELETE 500 must flush deferred dirty toggle Flight 2"
    );
    let _ = cw.read_with(app, |w, _| {
        let ui = w.state.lists.iter().find(|l| l.list_id == "l_3g").unwrap();
        let sv = srv.lists.lock().unwrap().iter().find(|l| l.list_id == "l_3g").cloned().unwrap();
        assert_eq!(ui.name, "Orig 3G");
        assert!(ui.is_pinned && ui.member_actor_ids == vec!["u:300".to_string()]);
        assert!(sv.is_pinned && sv.member_actor_ids == vec!["u:300".to_string()]);
    });

    // R8-4: submit_list_editor Edit commits while delete_list_action is in-flight, then DELETE returns HTTP 500
    let del_done_r84 = srv.list_delete_completed.load(Ordering::SeqCst);
    let le_done_r84 = srv.list_editor_patch_completed.load(Ordering::SeqCst);
    srv.list_editor_patch_delay_ms.store(60, Ordering::SeqCst);
    srv.list_delete_delay_ms.store(220, Ordering::SeqCst);
    let _ = cw.update(app, |w, cx| {
        let target = w.state.lists.iter().find(|l| l.list_id == "l_3g").cloned().unwrap();
        w.open_edit_list_form(target, cx);
        w.state.list_editor.name = "Renamed 3G".to_string();
        w.submit_list_editor(cx);
        w.delete_list_action("l_3g".to_string(), cx);
    });
    assert!(
        pump_until(app, Duration::from_secs(5), |cx| {
            let st = &cw.read(cx).state;
            !st.list_editor.submitting
                && !st.list_deleting.contains("l_3g")
                && srv.list_editor_patch_completed.load(Ordering::SeqCst) > le_done_r84
                && srv.list_delete_completed.load(Ordering::SeqCst) > del_done_r84
        })
        .await,
        "R8-4: Edit + DELETE 500 race must settle"
    );
    let _ = cw.read_with(app, |w, _| {
        let ui = w.state.lists.iter().find(|l| l.list_id == "l_3g").unwrap();
        let sv = srv.lists.lock().unwrap().iter().find(|l| l.list_id == "l_3g").cloned().unwrap();
        assert_eq!(ui.name, "Renamed 3G", "R8-4: UI must preserve committed editor rename after DELETE 500");
        assert_eq!(sv.name, "Renamed 3G");
    });

    // T8-A4 (2F_3G delete-500 + Flight 1 HTTP 500): rolls back cleanly to pre-Flight-1 last_synced
    let del_done_a4 = srv.list_delete_completed.load(Ordering::SeqCst);
    let lt_done_a4 = srv.list_toggle_patch_completed.load(Ordering::SeqCst);
    srv.list_toggle_patch_delay_ms.store(60, Ordering::SeqCst);
    srv.list_toggle_patch_fail_500.store(true, Ordering::SeqCst);
    srv.list_delete_delay_ms.store(200, Ordering::SeqCst);
    let _ = cw.update(app, |w, cx| {
        w.toggle_list_pinned("l_3g_err".to_string(), true, cx);
        w.delete_list_action("l_3g_err".to_string(), cx);
    });
    assert!(
        pump_until(app, Duration::from_secs(5), |cx| {
            let st = &cw.read(cx).state;
            !st.list_deleting.contains("l_3g_err")
                && st.list_membership_in_flight.is_empty()
                && srv.list_toggle_patch_completed.load(Ordering::SeqCst) > lt_done_a4
                && srv.list_delete_completed.load(Ordering::SeqCst) > del_done_a4
        })
        .await
    );
    srv.list_toggle_patch_fail_500.store(false, Ordering::SeqCst);
    srv.list_delete_fail_500.store(false, Ordering::SeqCst);
    let _ = cw.read_with(app, |w, _| {
        let ui = w.state.lists.iter().find(|l| l.list_id == "l_3g_err").unwrap();
        assert!(!ui.is_pinned, "T8-A4: failed Flight 1 during DELETE 500 must roll back to is_pinned=false");
    });

    // T8-A5 (2F_3D re-flush HTTP 500 rolls back to saved via seeded list_membership_last_synced)
    let lg_rx_a5 = srv.lists_get_received.load(Ordering::SeqCst);
    let lg_done_a5 = srv.lists_get_completed.load(Ordering::SeqCst);
    let lt_done_a5 = srv.list_toggle_patch_completed.load(Ordering::SeqCst);
    srv.lists_get_delay_ms.store(260, Ordering::SeqCst);
    let _ = cw.update(app, |w, cx| {
        let target = w.state.lists.iter().find(|l| l.list_id == "l_3d_500").cloned().unwrap();
        w.open_edit_list_form(target, cx);
        w.state.list_editor.name = "Saved 3D".to_string();
        w.submit_list_editor(cx);
    });
    assert!(
        pump_until(app, Duration::from_secs(4), |_| {
            srv.lists_get_received.load(Ordering::SeqCst) > lg_rx_a5
        })
        .await
    );
    // Quick toggle commits pinned=true while editor GET /lists is delayed, AND server resets back
    // to editor's saved state? No: trigger re-flush by having toggle commit BEFORE editor PATCH
    // or seed via fast toggle then failing re-flush:
    // Actually: if fast toggle commits pinned=true, then we mutate server back to pinned=false before GET /lists
    // so server_confirmed_toggle has pinned=false while UI has pinned=true -> triggers re-flush which fails 500!
    let _ = cw.update(app, |w, cx| {
        w.toggle_list_pinned("l_3d_500".to_string(), true, cx);
    });
    assert!(
        pump_until(app, Duration::from_secs(4), |cx| {
            let st = &cw.read(cx).state;
            st.list_membership_in_flight.is_empty()
                && srv.list_toggle_patch_completed.load(Ordering::SeqCst) > lt_done_a5
        })
        .await
    );
    // Simulate Order 1 (T_P < E_P < E_G): server list snapshot has saved is_pinned=false, and re-flush fails 500.
    {
        let mut guard = srv.lists.lock().unwrap();
        guard
            .iter_mut()
            .find(|x| x.list_id == "l_3d_500")
            .expect("T8-A5: l_3d_500 must exist in mock server lists")
            .is_pinned = false;
    }
    let lt_before_reflush = srv.list_toggle_patch_completed.load(Ordering::SeqCst);
    srv.list_toggle_patch_fail_500.store(true, Ordering::SeqCst);
    assert!(
        pump_until(app, Duration::from_secs(5), |cx| {
            let st = &cw.read(cx).state;
            !st.list_editor.submitting
                && st.list_membership_in_flight.is_empty()
                && st.list_membership_dirty.is_empty()
                && srv.lists_get_completed.load(Ordering::SeqCst) > lg_done_a5
                && srv.list_toggle_patch_completed.load(Ordering::SeqCst) > lt_before_reflush
        })
        .await,
        "T8-A5: editor re-flush HTTP 500 must settle"
    );
    srv.list_toggle_patch_fail_500.store(false, Ordering::SeqCst);
    let _ = cw.read_with(app, |w, _| {
        let ui = w.state.lists.iter().find(|l| l.list_id == "l_3d_500").unwrap();
        assert_eq!(ui.name, "Saved 3D");
        assert!(
            !ui.is_pinned,
            "T8-A5: failed re-flush must roll back to server_confirmed_toggle (is_pinned=false)"
        );
    });

    // R8-9 (AutoMod F1 OK + F2 500 vs slow load_mod_queue): cross-cache sync + draft reset + Case B reload
    let _ = cw.update(app, |w, cx| {
        w.navigate_to(CommunityRoute::ModQueue(Some(srv.slug.clone())), cx);
    });
    assert!(
        pump_until(app, Duration::from_secs(4), |cx| {
            let st = &cw.read(cx).state;
            !st.mod_loading && st.mod_community.is_some()
        })
        .await
    );
    let mq_rx_r89 = srv.mod_queue_get_received.load(Ordering::SeqCst);
    let cp_done_r89 = srv.comm_patch_completed.load(Ordering::SeqCst);
    srv.mod_queue_get_delay_ms.store(300, Ordering::SeqCst);
    srv.comm_patch_delay_ms.store(60, Ordering::SeqCst);
    srv.fail_second_flight.store(true, Ordering::SeqCst);
    let _ = cw.update(app, |w, cx| {
        w.load_mod_queue(srv.slug.clone(), cx);
        w.state.automod_draft.name = "Rule R8-9".to_string();
        w.state.automod_draft.keywords = "scam".to_string();
        w.submit_add_automod_rule(cx);
        let rid = w
            .state
            .mod_community
            .as_ref()
            .unwrap()
            .automod_rules
            .first()
            .unwrap()
            .rule_id
            .clone();
        // Queue Flight 2 (toggle enabled) while Flight 1 is in-flight
        w.toggle_automod_rule_enabled(rid, cx);
    });
    assert!(
        pump_until(app, Duration::from_secs(5), |cx| {
            let st = &cw.read(cx).state;
            !st.automod_draft.saving
                && st.automod_in_flight.is_empty()
                && !st.mod_loading
                && srv.comm_patch_completed.load(Ordering::SeqCst) >= cp_done_r89 + 2
                && srv.mod_queue_get_received.load(Ordering::SeqCst) >= mq_rx_r89 + 2
        })
        .await,
        "R8-9: AutoMod F1 OK + F2 500 must trigger Case B load_mod_queue and settle"
    );
    srv.comm_patch_fail_500.store(false, Ordering::SeqCst);
    let _ = cw.read_with(app, |w, _| {
        assert!(
            w.state.automod_draft.name.is_empty(),
            "R8-9: automod_draft must reset immediately when F1 (Add Rule) commits"
        );
        let mc = w.state.mod_community.as_ref().unwrap();
        assert_eq!(mc.automod_rules.len(), 1);
        assert!(mc.automod_rules[0].enabled, "R8-9: F2 500 rolls back enabled=true from F1");
        let c_list = w.state.communities.iter().find(|c| c.slug == srv.slug).unwrap();
        assert_eq!(c_list.automod_rules.len(), 1);
        assert!(c_list.automod_rules[0].enabled, "R8-9: communities[pos] stays synced with F1");
    });
}

async fn run_r8_group_b_case_a_and_error_case_b(
    app: &mut AsyncApp,
    cw: &Entity<CommunityWorkspace>,
    srv: &Arc<R8MockState>,
) {
    use std::sync::atomic::Ordering;

    // Open PostDetail for p_r8_main
    let _ = cw.update(app, |w, cx| {
        w.navigate_to(CommunityRoute::PostDetail(srv.post_id.clone()), cx);
    });
    assert!(
        pump_until(app, Duration::from_secs(4), |cx| {
            let st = &cw.read(cx).state;
            !st.detail_loading
                && st
                    .detail_post
                    .as_ref()
                    .is_some_and(|d| d.post.post_id == srv.post_id)
        })
        .await
    );

    // T8-B1 (ADV-2a & ADV-2b): vote_post(+1) & vote_comment_action(+1) -> stale load_post_detail
    // settles mid-Flight-1 -> assert mid_ui == (+1, 1) -> Click 2 (+1 toggle-off) -> final (0, 0), calls==2
    let pv_before = srv.post_vote_completed.load(Ordering::SeqCst);
    let cv_before = srv.comment_vote_completed.load(Ordering::SeqCst);
    let det_done_b1 = srv.detail_get_completed.load(Ordering::SeqCst);
    srv.detail_get_delay_ms.store(70, Ordering::SeqCst);
    srv.post_vote_delay_ms.store(220, Ordering::SeqCst);
    srv.comment_vote_delay_ms.store(220, Ordering::SeqCst);
    let _ = cw.update(app, |w, cx| {
        w.load_post_detail(srv.post_id.clone(), cx);
        w.vote_post(srv.post_id.clone(), 1, cx);
        w.vote_comment_action(srv.comment_id.clone(), 1, cx);
    });
    assert!(
        pump_until(app, Duration::from_secs(4), |cx| {
            srv.detail_get_completed.load(Ordering::SeqCst) > det_done_b1
                && !cw.read(cx).state.detail_loading
        })
        .await,
        "T8-B1: pre-vote load_post_detail must settle mid-Flight-1"
    );
    let _ = cw.update(app, |w, cx| {
        let p = w.state.find_post(&srv.post_id).unwrap();
        let c = w.state.find_comment(&srv.comment_id).unwrap();
        assert_eq!((p.viewer_vote, p.score), (1, 1), "ADV-2b: stale load_post_detail must NOT flicker post vote to (0,0)");
        assert_eq!((c.viewer_vote, c.score), (1, 1), "ADV-2a: stale load_post_detail must NOT flicker comment vote to (0,0)");
        // Click 2: toggle off back to 0
        w.vote_post(srv.post_id.clone(), 1, cx);
        w.vote_comment_action(srv.comment_id.clone(), 1, cx);
    });
    assert!(
        pump_until(app, Duration::from_secs(5), |cx| {
            let st = &cw.read(cx).state;
            st.post_vote_in_flight.is_empty()
                && st.post_vote_dirty.is_empty()
                && st.comment_vote_in_flight.is_empty()
                && st.comment_vote_dirty.is_empty()
                && srv.post_vote_completed.load(Ordering::SeqCst) >= pv_before + 2
                && srv.comment_vote_completed.load(Ordering::SeqCst) >= cv_before + 2
        })
        .await
    );
    let _ = cw.read_with(app, |w, _| {
        let p = w.state.find_post(&srv.post_id).unwrap();
        let c = w.state.find_comment(&srv.comment_id).unwrap();
        assert_eq!((p.viewer_vote, p.score), (0, 0));
        assert_eq!(srv.post_vote_dir.load(Ordering::SeqCst), 0);
        assert_eq!((c.viewer_vote, c.score), (0, 0));
        assert_eq!(srv.comment_vote_dir.load(Ordering::SeqCst), 0);
    });

    // T8-B2 (ADV-2c Like Case A 2-click): toggle_like & toggle_comment_like_action vs mid-flight load_post_detail
    let pl_before = srv.post_like_completed.load(Ordering::SeqCst);
    let cl_before = srv.comment_like_completed.load(Ordering::SeqCst);
    let det_done_b2 = srv.detail_get_completed.load(Ordering::SeqCst);
    srv.detail_get_delay_ms.store(70, Ordering::SeqCst);
    srv.post_like_delay_ms.store(220, Ordering::SeqCst);
    srv.comment_like_delay_ms.store(220, Ordering::SeqCst);
    let _ = cw.update(app, |w, cx| {
        w.load_post_detail(srv.post_id.clone(), cx);
        w.toggle_like(srv.post_id.clone(), cx);
        w.toggle_comment_like_action(srv.comment_id.clone(), cx);
    });
    assert!(
        pump_until(app, Duration::from_secs(4), |cx| {
            srv.detail_get_completed.load(Ordering::SeqCst) > det_done_b2
                && !cw.read(cx).state.detail_loading
        })
        .await
    );
    let _ = cw.update(app, |w, cx| {
        let p = w.state.find_post(&srv.post_id).unwrap();
        let c = w.state.find_comment(&srv.comment_id).unwrap();
        assert_eq!((p.viewer_liked, p.likes_count), (true, 1), "ADV-2c: stale load_post_detail must NOT flicker post like");
        assert_eq!((c.viewer_liked, c.likes_count), (true, 1), "ADV-2c: stale load_post_detail must NOT flicker comment like");
        w.toggle_like(srv.post_id.clone(), cx);
        w.toggle_comment_like_action(srv.comment_id.clone(), cx);
    });
    assert!(
        pump_until(app, Duration::from_secs(5), |cx| {
            let st = &cw.read(cx).state;
            st.post_like_in_flight.is_empty()
                && st.post_like_pending_toggle.is_empty()
                && st.comment_like_in_flight.is_empty()
                && st.comment_like_pending_toggle.is_empty()
                && srv.post_like_completed.load(Ordering::SeqCst) >= pl_before + 2
                && srv.comment_like_completed.load(Ordering::SeqCst) >= cl_before + 2
        })
        .await
    );
    let _ = cw.read_with(app, |w, _| {
        let p = w.state.find_post(&srv.post_id).unwrap();
        let c = w.state.find_comment(&srv.comment_id).unwrap();
        assert_eq!((p.viewer_liked, p.likes_count), (false, 0));
        assert!(!srv.post_liked.load(Ordering::SeqCst));
        assert_eq!((c.viewer_liked, c.likes_count), (false, 0));
        assert!(!srv.comment_liked.load(Ordering::SeqCst));
    });

    // T8-B3 (R7-*-F2-500-VS-GET across ALL 7 single-flight error-rollback arms including R8-7 bookmark!)
    // 1) post_vote F1 OK (+1) + F2 500 (-1) vs slow load_post_detail
    let det_rx_pv = srv.detail_get_received.load(Ordering::SeqCst);
    let pv_b3 = srv.post_vote_completed.load(Ordering::SeqCst);
    srv.detail_get_delay_ms.store(320, Ordering::SeqCst);
    srv.post_vote_delay_ms.store(60, Ordering::SeqCst);
    srv.fail_second_flight.store(true, Ordering::SeqCst);
    let _ = cw.update(app, |w, cx| {
        w.load_post_detail(srv.post_id.clone(), cx);
        w.vote_post(srv.post_id.clone(), 1, cx);
        w.vote_post(srv.post_id.clone(), -1, cx);
    });
    assert!(
        pump_until(app, Duration::from_secs(5), |cx| {
            let st = &cw.read(cx).state;
            st.post_vote_in_flight.is_empty()
                && !st.detail_loading
                && srv.post_vote_completed.load(Ordering::SeqCst) >= pv_b3 + 2
                && srv.detail_get_received.load(Ordering::SeqCst) >= det_rx_pv + 2
        })
        .await,
        "T8-B3 post_vote: F2 500 must trigger Case B detail reload"
    );
    srv.post_vote_fail_500.store(false, Ordering::SeqCst);
    let _ = cw.read_with(app, |w, _| {
        assert_eq!(w.state.detail_post.as_ref().unwrap().post.viewer_vote, 1);
    });

    // 2) post_like F1 OK (true) + F2 500 (false) vs slow load_post_detail
    let det_rx_pl = srv.detail_get_received.load(Ordering::SeqCst);
    let pl_b3 = srv.post_like_completed.load(Ordering::SeqCst);
    srv.detail_get_delay_ms.store(320, Ordering::SeqCst);
    srv.post_like_delay_ms.store(60, Ordering::SeqCst);
    srv.fail_second_flight.store(true, Ordering::SeqCst);
    let _ = cw.update(app, |w, cx| {
        w.load_post_detail(srv.post_id.clone(), cx);
        w.toggle_like(srv.post_id.clone(), cx);
        w.toggle_like(srv.post_id.clone(), cx);
    });
    assert!(
        pump_until(app, Duration::from_secs(5), |cx| {
            let st = &cw.read(cx).state;
            st.post_like_in_flight.is_empty()
                && !st.detail_loading
                && srv.post_like_completed.load(Ordering::SeqCst) >= pl_b3 + 2
                && srv.detail_get_received.load(Ordering::SeqCst) >= det_rx_pl + 2
        })
        .await,
        "T8-B3 post_like: F2 500 must trigger Case B detail reload"
    );
    srv.post_like_fail_500.store(false, Ordering::SeqCst);
    let _ = cw.read_with(app, |w, _| {
        assert!(w.state.detail_post.as_ref().unwrap().post.viewer_liked);
    });

    // 3) comment_vote F1 OK (+1) + F2 500 (-1) vs slow load_post_detail
    let det_rx_cv = srv.detail_get_received.load(Ordering::SeqCst);
    let cv_b3 = srv.comment_vote_completed.load(Ordering::SeqCst);
    srv.detail_get_delay_ms.store(320, Ordering::SeqCst);
    srv.comment_vote_delay_ms.store(60, Ordering::SeqCst);
    srv.fail_second_flight.store(true, Ordering::SeqCst);
    let _ = cw.update(app, |w, cx| {
        w.load_post_detail(srv.post_id.clone(), cx);
        w.vote_comment_action(srv.comment_id.clone(), 1, cx);
        w.vote_comment_action(srv.comment_id.clone(), -1, cx);
    });
    assert!(
        pump_until(app, Duration::from_secs(5), |cx| {
            let st = &cw.read(cx).state;
            st.comment_vote_in_flight.is_empty()
                && !st.detail_loading
                && srv.comment_vote_completed.load(Ordering::SeqCst) >= cv_b3 + 2
                && srv.detail_get_received.load(Ordering::SeqCst) >= det_rx_cv + 2
        })
        .await,
        "T8-B3 comment_vote: F2 500 must trigger Case B detail reload"
    );
    srv.comment_vote_fail_500.store(false, Ordering::SeqCst);
    let _ = cw.read_with(app, |w, _| {
        assert_eq!(w.state.find_comment(&srv.comment_id).unwrap().viewer_vote, 1);
    });

    // 4) comment_like F1 OK (true) + F2 500 (false) vs slow load_post_detail
    let det_rx_cl = srv.detail_get_received.load(Ordering::SeqCst);
    let cl_b3 = srv.comment_like_completed.load(Ordering::SeqCst);
    srv.detail_get_delay_ms.store(320, Ordering::SeqCst);
    srv.comment_like_delay_ms.store(60, Ordering::SeqCst);
    srv.fail_second_flight.store(true, Ordering::SeqCst);
    let _ = cw.update(app, |w, cx| {
        w.load_post_detail(srv.post_id.clone(), cx);
        w.toggle_comment_like_action(srv.comment_id.clone(), cx);
        w.toggle_comment_like_action(srv.comment_id.clone(), cx);
    });
    assert!(
        pump_until(app, Duration::from_secs(5), |cx| {
            let st = &cw.read(cx).state;
            st.comment_like_in_flight.is_empty()
                && !st.detail_loading
                && srv.comment_like_completed.load(Ordering::SeqCst) >= cl_b3 + 2
                && srv.detail_get_received.load(Ordering::SeqCst) >= det_rx_cl + 2
        })
        .await,
        "T8-B3 comment_like: F2 500 must trigger Case B detail reload"
    );
    srv.comment_like_fail_500.store(false, Ordering::SeqCst);
    let _ = cw.read_with(app, |w, _| {
        assert!(w.state.find_comment(&srv.comment_id).unwrap().viewer_liked);
    });

    // 5) bookmark (R8-7!) F1 OK (true) + F2 500 (false) vs slow load_post_detail
    let det_rx_bm = srv.detail_get_received.load(Ordering::SeqCst);
    let bm_b3 = srv.post_bookmark_completed.load(Ordering::SeqCst);
    srv.detail_get_delay_ms.store(320, Ordering::SeqCst);
    srv.post_bookmark_delay_ms.store(60, Ordering::SeqCst);
    srv.fail_second_flight.store(true, Ordering::SeqCst);
    let _ = cw.update(app, |w, cx| {
        w.load_post_detail(srv.post_id.clone(), cx);
        w.toggle_bookmark(srv.post_id.clone(), cx);
        w.toggle_bookmark(srv.post_id.clone(), cx);
    });
    assert!(
        pump_until(app, Duration::from_secs(5), |cx| {
            let st = &cw.read(cx).state;
            st.bookmark_in_flight.is_empty()
                && !st.detail_loading
                && srv.post_bookmark_completed.load(Ordering::SeqCst) >= bm_b3 + 2
                && srv.detail_get_received.load(Ordering::SeqCst) >= det_rx_bm + 2
        })
        .await,
        "R8-7 bookmark: F2 500 must trigger Case B detail reload"
    );
    srv.post_bookmark_fail_500.store(false, Ordering::SeqCst);
    let _ = cw.read_with(app, |w, _| {
        assert!(w.state.detail_post.as_ref().unwrap().post.viewer_bookmarked);
    });

    // 6) mute & 7) block F1 OK + F2 500 vs slow load_actor_profile, plus T8-B4 (S2A-5b-6f)
    let _ = cw.update(app, |w, cx| {
        w.open_actor_profile("r8_bob".to_string(), cx);
    });
    assert!(
        pump_until(app, Duration::from_secs(4), |cx| {
            let st = &cw.read(cx).state;
            !st.profile_loading
                && st
                    .actor_profile
                    .as_ref()
                    .is_some_and(|p| p.actor.actor_id == srv.bob_id && p.actor.is_following)
        })
        .await
    );

    // 6) mute F1 OK (true) + F2 500 (false) vs slow load_actor_profile
    let pr_rx_m = srv.profile_get_received.load(Ordering::SeqCst);
    let m_b3 = srv.mute_completed.load(Ordering::SeqCst);
    srv.profile_get_delay_ms.store(320, Ordering::SeqCst);
    srv.mute_delay_ms.store(60, Ordering::SeqCst);
    srv.fail_second_flight.store(true, Ordering::SeqCst);
    let _ = cw.update(app, |w, cx| {
        w.load_actor_profile("r8_bob".to_string(), cx);
        w.toggle_mute_actor(srv.bob_id.clone(), false, cx);
        w.toggle_mute_actor(srv.bob_id.clone(), true, cx);
    });
    assert!(
        pump_until(app, Duration::from_secs(5), |cx| {
            let st = &cw.read(cx).state;
            st.actor_mute_in_flight.is_empty()
                && !st.profile_loading
                && srv.mute_completed.load(Ordering::SeqCst) >= m_b3 + 2
                && srv.profile_get_received.load(Ordering::SeqCst) >= pr_rx_m + 2
        })
        .await,
        "T8-B3 mute: F2 500 must trigger Case B profile reload"
    );
    srv.mute_fail_500.store(false, Ordering::SeqCst);
    let _ = cw.read_with(app, |w, _| {
        assert!(w.state.actor_profile.as_ref().unwrap().actor.is_muted);
    });

    // 7) block F1 OK (true) + F2 500 (false) vs slow load_actor_profile
    let pr_rx_b = srv.profile_get_received.load(Ordering::SeqCst);
    let b_b3 = srv.block_completed.load(Ordering::SeqCst);
    srv.profile_get_delay_ms.store(320, Ordering::SeqCst);
    srv.block_delay_ms.store(60, Ordering::SeqCst);
    srv.fail_second_flight.store(true, Ordering::SeqCst);
    let _ = cw.update(app, |w, cx| {
        w.load_actor_profile("r8_bob".to_string(), cx);
        w.toggle_block_actor(srv.bob_id.clone(), false, cx);
        w.toggle_block_actor(srv.bob_id.clone(), true, cx);
    });
    assert!(
        pump_until(app, Duration::from_secs(5), |cx| {
            let st = &cw.read(cx).state;
            st.actor_block_in_flight.is_empty()
                && !st.profile_loading
                && srv.block_completed.load(Ordering::SeqCst) >= b_b3 + 2
                && srv.profile_get_received.load(Ordering::SeqCst) >= pr_rx_b + 2
        })
        .await,
        "T8-B3 block: F2 500 must trigger Case B profile reload"
    );
    srv.block_fail_500.store(false, Ordering::SeqCst);
    let _ = cw.read_with(app, |w, _| {
        let a = &w.state.actor_profile.as_ref().unwrap().actor;
        assert!(a.is_blocked && !a.is_following);
    });

    // T8-B4 (S2A-5b-6f nav-away-and-back): reset bob to (blocked=false, following=true), then
    // Block -> Unblock + navigate bob -> charlie -> bob BEFORE Flight 1 settles -> transient_fol == false!
    srv.bob_blocked.store(false, Ordering::SeqCst);
    srv.bob_following.store(true, Ordering::SeqCst);
    let _ = cw.update(app, |w, cx| {
        w.load_actor_profile("r8_bob".to_string(), cx);
    });
    assert!(
        pump_until(app, Duration::from_secs(4), |cx| {
            let st = &cw.read(cx).state;
            !st.profile_loading
                && st
                    .actor_profile
                    .as_ref()
                    .is_some_and(|p| p.actor.actor_id == srv.bob_id && !p.actor.is_blocked && p.actor.is_following)
        })
        .await
    );
    let b_b4 = srv.block_completed.load(Ordering::SeqCst);
    srv.block_delay_ms.store(240, Ordering::SeqCst);
    let _ = cw.update(app, |w, cx| {
        w.toggle_block_actor(srv.bob_id.clone(), false, cx);
        w.toggle_block_actor(srv.bob_id.clone(), true, cx);
        // Navigate away to charlie (clears actor_profile for bob!)
        w.open_actor_profile("r8_charlie".to_string(), cx);
    });
    assert!(
        pump_until(app, Duration::from_secs(4), |cx| {
            let st = &cw.read(cx).state;
            !st.profile_loading
                && st
                    .actor_profile
                    .as_ref()
                    .is_some_and(|p| p.actor.actor_id == srv.charlie_id)
        })
        .await
    );
    // Navigate BACK to bob BEFORE Flight 1 settles! Server still returns is_following=true!
    let _ = cw.update(app, |w, cx| {
        w.open_actor_profile("r8_bob".to_string(), cx);
    });
    assert!(
        pump_until(app, Duration::from_secs(4), |cx| {
            let st = &cw.read(cx).state;
            !st.profile_loading
                && st
                    .actor_profile
                    .as_ref()
                    .is_some_and(|p| p.actor.actor_id == srv.bob_id)
        })
        .await
    );
    let _ = cw.read_with(app, |w, _| {
        let a = &w.state.actor_profile.as_ref().unwrap().actor;
        assert!(
            !a.is_following,
            "T8-B4 (S2A-5b-6f): nav-away-and-back during Block->Unblock Flight 1 must NEVER flash is_following=true"
        );
    });
    assert!(
        pump_until(app, Duration::from_secs(5), |cx| {
            let st = &cw.read(cx).state;
            st.actor_block_in_flight.is_empty()
                && st.actor_block_dirty.is_empty()
                && st.actor_block_chain_had_block.is_empty()
                && srv.block_completed.load(Ordering::SeqCst) >= b_b4 + 2
        })
        .await
    );
    let _ = cw.read_with(app, |w, _| {
        let a = &w.state.actor_profile.as_ref().unwrap().actor;
        assert!(!a.is_blocked && !a.is_following);
    });
}

async fn run_r8_group_c_four_new_single_flight(
    app: &mut AsyncApp,
    cw: &Entity<CommunityWorkspace>,
    srv: &Arc<R8MockState>,
) {
    use std::sync::atomic::Ordering;

    // ------------------------------------------------------------------
    // T8-C1: repost_post single-flight (2-click, 3-click elision, Case A, F1 OK+F2 500 Case B, ABA)
    // ------------------------------------------------------------------
    let _ = cw.update(app, |w, cx| {
        w.navigate_to(CommunityRoute::PostDetail(srv.post_id.clone()), cx);
    });
    assert!(
        pump_until(app, Duration::from_secs(4), |cx| {
            !cw.read(cx).state.detail_loading && cw.read(cx).state.detail_post.is_some()
        })
        .await
    );
    srv.post_repost_in_flight_max.store(0, Ordering::SeqCst);
    let rp_done_1 = srv.post_repost_completed.load(Ordering::SeqCst);
    srv.post_repost_delay_ms.store(120, Ordering::SeqCst);
    let _ = cw.update(app, |w, cx| {
        w.repost_post(srv.post_id.clone(), cx);
        w.repost_post(srv.post_id.clone(), cx);
        assert!(w.state.post_repost_in_flight.contains_key(&srv.post_id));
        assert!(w.state.post_repost_pending_toggle.contains(&srv.post_id));
    });
    assert!(
        pump_until(app, Duration::from_secs(5), |cx| {
            let st = &cw.read(cx).state;
            st.post_repost_in_flight.is_empty()
                && st.post_repost_pending_toggle.is_empty()
                && srv.post_repost_completed.load(Ordering::SeqCst) >= rp_done_1 + 2
        })
        .await,
        "T8-C1: rapid 2-click repost->unrepost must flush sequentially"
    );
    assert_eq!(srv.post_repost_in_flight_max.load(Ordering::SeqCst), 1);
    let _ = cw.read_with(app, |w, _| {
        let p = w.state.find_post(&srv.post_id).unwrap();
        assert!(!p.viewer_reposted && p.reposts_count == 0);
        assert!(!srv.post_reposted.load(Ordering::SeqCst));
    });

    // 3-click elision (only 1 HTTP call!) + Case A load_post_detail overlay mid-Flight-1
    let rp_done_3c = srv.post_repost_completed.load(Ordering::SeqCst);
    let det_done_c1 = srv.detail_get_completed.load(Ordering::SeqCst);
    srv.post_repost_delay_ms.store(220, Ordering::SeqCst);
    srv.detail_get_delay_ms.store(60, Ordering::SeqCst);
    let _ = cw.update(app, |w, cx| {
        w.load_post_detail(srv.post_id.clone(), cx);
        w.repost_post(srv.post_id.clone(), cx);
        w.repost_post(srv.post_id.clone(), cx);
        w.repost_post(srv.post_id.clone(), cx);
        assert!(!w.state.post_repost_pending_toggle.contains(&srv.post_id), "T8-C1: 3-click parity elides Flight 2");
    });
    assert!(
        pump_until(app, Duration::from_secs(4), |cx| {
            srv.detail_get_completed.load(Ordering::SeqCst) > det_done_c1
                && !cw.read(cx).state.detail_loading
        })
        .await
    );
    let _ = cw.read_with(app, |w, _| {
        assert!(
            w.state.detail_post.as_ref().unwrap().post.viewer_reposted,
            "T8-C1 Case A: stale load_post_detail must overlay viewer_reposted=true"
        );
    });
    assert!(
        pump_until(app, Duration::from_secs(4), |cx| {
            cw.read(cx).state.post_repost_in_flight.is_empty()
                && srv.post_repost_completed.load(Ordering::SeqCst) == rp_done_3c + 1
        })
        .await
    );
    assert_eq!(srv.post_repost_completed.load(Ordering::SeqCst), rp_done_3c + 1);

    // Repost F1 OK (false) + F2 500 (true) vs slow load_post_detail -> Case B reload
    let det_rx_rp = srv.detail_get_received.load(Ordering::SeqCst);
    let rp_b = srv.post_repost_completed.load(Ordering::SeqCst);
    srv.detail_get_delay_ms.store(320, Ordering::SeqCst);
    srv.post_repost_delay_ms.store(60, Ordering::SeqCst);
    srv.fail_second_flight.store(true, Ordering::SeqCst);
    let _ = cw.update(app, |w, cx| {
        w.load_post_detail(srv.post_id.clone(), cx);
        w.repost_post(srv.post_id.clone(), cx);
        w.repost_post(srv.post_id.clone(), cx);
    });
    assert!(
        pump_until(app, Duration::from_secs(5), |cx| {
            let st = &cw.read(cx).state;
            st.post_repost_in_flight.is_empty()
                && !st.detail_loading
                && srv.post_repost_completed.load(Ordering::SeqCst) >= rp_b + 2
                && srv.detail_get_received.load(Ordering::SeqCst) >= det_rx_rp + 2
        })
        .await,
        "T8-C1 Case B: repost F2 500 must trigger Case B detail reload"
    );
    srv.post_repost_fail_500.store(false, Ordering::SeqCst);
    let _ = cw.read_with(app, |w, _| {
        assert!(!w.state.detail_post.as_ref().unwrap().post.viewer_reposted);
    });

    // ------------------------------------------------------------------
    // T8-C2: toggle_join_community single-flight (Join->Leave, Case A, F1 OK+F2 500 Case B, 2D JOIN-RACE!)
    // ------------------------------------------------------------------
    let _ = cw.update(app, |w, cx| {
        w.navigate_to(CommunityRoute::CommunityPage(srv.slug.clone()), cx);
    });
    assert!(
        pump_until(app, Duration::from_secs(4), |cx| {
            !cw.read(cx).state.community_loading && cw.read(cx).state.community_detail.is_some()
        })
        .await
    );
    srv.join_in_flight_max.store(0, Ordering::SeqCst);
    let j_done_1 = srv.join_completed.load(Ordering::SeqCst);
    let comm_done_c2 = srv.comm_get_completed.load(Ordering::SeqCst);
    srv.join_delay_ms.store(180, Ordering::SeqCst);
    srv.comm_get_delay_ms.store(60, Ordering::SeqCst);
    let _ = cw.update(app, |w, cx| {
        w.load_community_page(srv.slug.clone(), cx);
        w.toggle_join_community(srv.slug.clone(), true, cx);
        w.toggle_join_community(srv.slug.clone(), false, cx);
        assert!(w.state.community_join_in_flight.contains_key(&srv.slug));
        assert!(w.state.community_join_dirty.contains(&srv.slug));
    });
    assert!(
        pump_until(app, Duration::from_secs(4), |cx| {
            srv.comm_get_completed.load(Ordering::SeqCst) > comm_done_c2
                && !cw.read(cx).state.community_loading
        })
        .await
    );
    let _ = cw.read_with(app, |w, _| {
        // Pending target is false -> Case A overlay keeps is_joined=false
        assert!(!w.state.community_detail.as_ref().unwrap().community.is_joined);
    });
    assert!(
        pump_until(app, Duration::from_secs(5), |cx| {
            let st = &cw.read(cx).state;
            st.community_join_in_flight.is_empty()
                && st.community_join_dirty.is_empty()
                && srv.join_completed.load(Ordering::SeqCst) >= j_done_1 + 2
        })
        .await
    );
    assert_eq!(srv.join_in_flight_max.load(Ordering::SeqCst), 1);
    let _ = cw.read_with(app, |w, _| {
        let c = &w.state.community_detail.as_ref().unwrap().community;
        assert!(!c.is_joined && c.member_count == 10);
        assert!(!srv.comm_joined.load(Ordering::SeqCst));
    });

    // Join F1 OK (true) + F2 500 (false) vs slow load_community_page -> Case B reload
    let comm_rx_j = srv.comm_get_received.load(Ordering::SeqCst);
    let j_b = srv.join_completed.load(Ordering::SeqCst);
    srv.comm_get_delay_ms.store(320, Ordering::SeqCst);
    srv.join_delay_ms.store(60, Ordering::SeqCst);
    srv.fail_second_flight.store(true, Ordering::SeqCst);
    let _ = cw.update(app, |w, cx| {
        w.load_community_page(srv.slug.clone(), cx);
        w.toggle_join_community(srv.slug.clone(), true, cx);
        w.toggle_join_community(srv.slug.clone(), false, cx);
    });
    assert!(
        pump_until(app, Duration::from_secs(5), |cx| {
            let st = &cw.read(cx).state;
            st.community_join_in_flight.is_empty()
                && !st.community_loading
                && srv.join_completed.load(Ordering::SeqCst) >= j_b + 2
                && srv.comm_get_received.load(Ordering::SeqCst) >= comm_rx_j + 2
        })
        .await,
        "T8-C2 Case B: join F2 500 must trigger Case B community reload"
    );
    srv.join_fail_500.store(false, Ordering::SeqCst);
    let _ = cw.read_with(app, |w, _| {
        assert!(w.state.community_detail.as_ref().unwrap().community.is_joined);
    });

    // 2D JOIN-RACE security check: switch to non-mod alt_id, call toggle_join_community(true)
    // and assert viewer_role is NEVER optimistically granted!
    srv.comm_joined.store(false, Ordering::SeqCst);
    srv.join_delay_ms.store(180, Ordering::SeqCst);
    let _ = cw.update(app, |w, cx| {
        w.switch_active_actor(srv.alt_id.clone(), cx);
    });
    assert!(
        pump_until(app, Duration::from_secs(4), |cx| {
            let st = &cw.read(cx).state;
            !st.community_loading && st.active_actor_id == srv.alt_id
        })
        .await
    );
    let _ = cw.update(app, |w, cx| {
        w.toggle_join_community(srv.slug.clone(), true, cx);
        assert!(
            w.state.community_detail.as_ref().unwrap().community.viewer_role.is_none(),
            "2D JOIN-RACE: toggle_join_community must NEVER optimistically grant viewer_role"
        );
        // Switch back to owner mid-flight -> stale alt_id join callback must be fenced!
        w.switch_active_actor(srv.owner_id.clone(), cx);
    });
    assert!(
        pump_until(app, Duration::from_secs(4), |cx| {
            let st = &cw.read(cx).state;
            !st.community_loading && st.active_actor_id == srv.owner_id
        })
        .await
    );

    // ------------------------------------------------------------------
    // T8-C3: toggle_follow_actor single-flight + Cross-Lane Follow -> Block fence!
    // ------------------------------------------------------------------
    srv.bob_blocked.store(false, Ordering::SeqCst);
    srv.bob_following.store(false, Ordering::SeqCst);
    srv.bob_followers_count.store(5, Ordering::SeqCst);
    let _ = cw.update(app, |w, cx| {
        w.open_actor_profile("r8_bob".to_string(), cx);
    });
    assert!(
        pump_until(app, Duration::from_secs(4), |cx| {
            let st = &cw.read(cx).state;
            !st.profile_loading
                && st
                    .actor_profile
                    .as_ref()
                    .is_some_and(|p| p.actor.actor_id == srv.bob_id && !p.actor.is_following)
        })
        .await
    );
    // Rapid Follow -> Unfollow single-flight + Case A overlay
    srv.follow_in_flight_max.store(0, Ordering::SeqCst);
    let fl_done_1 = srv.follow_completed.load(Ordering::SeqCst);
    let pr_done_c3 = srv.profile_get_completed.load(Ordering::SeqCst);
    srv.follow_delay_ms.store(180, Ordering::SeqCst);
    srv.profile_get_delay_ms.store(60, Ordering::SeqCst);
    let _ = cw.update(app, |w, cx| {
        w.load_actor_profile("r8_bob".to_string(), cx);
        w.toggle_follow_actor(srv.bob_id.clone(), true, cx);
        w.toggle_follow_actor(srv.bob_id.clone(), false, cx);
        assert!(w.state.actor_follow_in_flight.contains_key(&srv.bob_id));
        assert!(w.state.actor_follow_dirty.contains(&srv.bob_id));
    });
    assert!(
        pump_until(app, Duration::from_secs(4), |cx| {
            srv.profile_get_completed.load(Ordering::SeqCst) > pr_done_c3
                && !cw.read(cx).state.profile_loading
        })
        .await
    );
    assert!(
        pump_until(app, Duration::from_secs(5), |cx| {
            let st = &cw.read(cx).state;
            st.actor_follow_in_flight.is_empty()
                && st.actor_follow_dirty.is_empty()
                && srv.follow_completed.load(Ordering::SeqCst) >= fl_done_1 + 2
        })
        .await
    );
    assert_eq!(srv.follow_in_flight_max.load(Ordering::SeqCst), 1);
    let _ = cw.read_with(app, |w, _| {
        let a = &w.state.actor_profile.as_ref().unwrap().actor;
        assert!(!a.is_following && a.followers_count == 5);
    });

    // Follow F1 OK (true) + F2 500 (false) vs slow load_actor_profile -> Case B reload
    let pr_rx_f = srv.profile_get_received.load(Ordering::SeqCst);
    let fl_b = srv.follow_completed.load(Ordering::SeqCst);
    srv.profile_get_delay_ms.store(320, Ordering::SeqCst);
    srv.follow_delay_ms.store(60, Ordering::SeqCst);
    srv.fail_second_flight.store(true, Ordering::SeqCst);
    let _ = cw.update(app, |w, cx| {
        w.load_actor_profile("r8_bob".to_string(), cx);
        w.toggle_follow_actor(srv.bob_id.clone(), true, cx);
        w.toggle_follow_actor(srv.bob_id.clone(), false, cx);
    });
    assert!(
        pump_until(app, Duration::from_secs(5), |cx| {
            let st = &cw.read(cx).state;
            st.actor_follow_in_flight.is_empty()
                && !st.profile_loading
                && srv.follow_completed.load(Ordering::SeqCst) >= fl_b + 2
                && srv.profile_get_received.load(Ordering::SeqCst) >= pr_rx_f + 2
        })
        .await,
        "T8-C3 Case B: follow F2 500 must trigger Case B profile reload"
    );
    srv.follow_fail_500.store(false, Ordering::SeqCst);
    let _ = cw.read_with(app, |w, _| {
        assert!(w.state.actor_profile.as_ref().unwrap().actor.is_following);
    });

    // Cross-Lane Follow -> Block race: slow Follow (220ms) -> fast Block (10ms)
    srv.bob_following.store(false, Ordering::SeqCst);
    srv.bob_blocked.store(false, Ordering::SeqCst);
    let _ = cw.update(app, |w, cx| {
        w.load_actor_profile("r8_bob".to_string(), cx);
    });
    assert!(
        pump_until(app, Duration::from_secs(4), |cx| {
            !cw.read(cx).state.profile_loading
                && cw
                    .read(cx)
                    .state
                    .actor_profile
                    .as_ref()
                    .is_some_and(|p| !p.actor.is_following && !p.actor.is_blocked)
        })
        .await
    );
    let fl_done_xl = srv.follow_completed.load(Ordering::SeqCst);
    let blk_done_xl = srv.block_completed.load(Ordering::SeqCst);
    srv.follow_delay_ms.store(220, Ordering::SeqCst);
    srv.block_delay_ms.store(10, Ordering::SeqCst);
    let _ = cw.update(app, |w, cx| {
        w.toggle_follow_actor(srv.bob_id.clone(), true, cx);
        w.toggle_block_actor(srv.bob_id.clone(), false, cx);
        let a = &w.state.actor_profile.as_ref().unwrap().actor;
        assert!(
            w.state.actor_block_dirty.contains(&srv.bob_id)
                && !w.state.actor_block_in_flight.contains_key(&srv.bob_id)
                && w.state.actor_block_chain_had_block.contains(&srv.bob_id)
                && a.is_blocked
                && !a.is_following,
            "T8-C3 Cross-Lane: Block must defer behind active Follow flight while optimistically showing blocked & unfollowed"
        );
    });
    assert!(
        pump_until(app, Duration::from_secs(5), |cx| {
            let st = &cw.read(cx).state;
            st.actor_block_in_flight.is_empty()
                && st.actor_block_dirty.is_empty()
                && st.actor_follow_in_flight.is_empty()
                && srv.block_completed.load(Ordering::SeqCst) > blk_done_xl
                && srv.follow_completed.load(Ordering::SeqCst) > fl_done_xl
        })
        .await
    );
    let _ = cw.read_with(app, |w, _| {
        let a = &w.state.actor_profile.as_ref().unwrap().actor;
        assert!(
            a.is_blocked && !a.is_following,
            "T8-C3 Cross-Lane: delayed Follow response must NEVER set is_following=true onto blocked actor"
        );
        assert!(srv.bob_blocked.load(Ordering::SeqCst) && !srv.bob_following.load(Ordering::SeqCst));
    });

    // ------------------------------------------------------------------
    // T8-C4: vote_poll single-flight (single-select opt_a->opt_b, multi-select opt_a->opt_b 0 lost update, Case A, F1 OK+F2 500 Case B)
    // ------------------------------------------------------------------
    let _ = cw.update(app, |w, cx| {
        w.navigate_to(CommunityRoute::PostDetail(srv.post_id.clone()), cx);
    });
    assert!(
        pump_until(app, Duration::from_secs(4), |cx| {
            !cw.read(cx).state.detail_loading
                && cw
                    .read(cx)
                    .state
                    .detail_post
                    .as_ref()
                    .is_some_and(|d| d.post.post_id == srv.post_id)
        })
        .await
    );
    // Single-select rapid opt_a -> opt_b + Case A overlay
    srv.poll_vote_in_flight_max.store(0, Ordering::SeqCst);
    let pv_done_s = srv.poll_vote_completed.load(Ordering::SeqCst);
    let det_done_p = srv.detail_get_completed.load(Ordering::SeqCst);
    srv.poll_vote_delay_ms.store(180, Ordering::SeqCst);
    srv.detail_get_delay_ms.store(60, Ordering::SeqCst);
    let _ = cw.update(app, |w, cx| {
        w.load_post_detail(srv.post_id.clone(), cx);
        w.vote_poll(srv.post_id.clone(), "opt_a".to_string(), cx);
        w.vote_poll(srv.post_id.clone(), "opt_b".to_string(), cx);
    });
    assert!(
        pump_until(app, Duration::from_secs(4), |cx| {
            srv.detail_get_completed.load(Ordering::SeqCst) > det_done_p
                && !cw.read(cx).state.detail_loading
        })
        .await
    );
    let _ = cw.read_with(app, |w, _| {
        let voted = &w.state.detail_post.as_ref().unwrap().post.poll.as_ref().unwrap().viewer_voted_option_ids;
        assert_eq!(voted, &vec!["opt_b".to_string()], "T8-C4 Case A: stale detail must preserve pending opt_b");
    });
    assert!(
        pump_until(app, Duration::from_secs(5), |cx| {
            let st = &cw.read(cx).state;
            st.poll_vote_in_flight.is_empty()
                && st.poll_vote_dirty.is_empty()
                && srv.poll_vote_completed.load(Ordering::SeqCst) >= pv_done_s + 2
        })
        .await
    );
    assert_eq!(srv.poll_vote_in_flight_max.load(Ordering::SeqCst), 1);
    assert_eq!(*srv.single_poll_voted.lock().unwrap(), vec!["opt_b".to_string()]);

    // Single-select F1 OK (opt_a) + F2 500 (opt_b) vs slow load_post_detail -> Case B reload
    let det_rx_poll = srv.detail_get_received.load(Ordering::SeqCst);
    let pv_b = srv.poll_vote_completed.load(Ordering::SeqCst);
    srv.detail_get_delay_ms.store(320, Ordering::SeqCst);
    srv.poll_vote_delay_ms.store(60, Ordering::SeqCst);
    srv.fail_second_flight.store(true, Ordering::SeqCst);
    let _ = cw.update(app, |w, cx| {
        w.load_post_detail(srv.post_id.clone(), cx);
        w.vote_poll(srv.post_id.clone(), "opt_a".to_string(), cx);
        w.vote_poll(srv.post_id.clone(), "opt_b".to_string(), cx);
    });
    assert!(
        pump_until(app, Duration::from_secs(5), |cx| {
            let st = &cw.read(cx).state;
            st.poll_vote_in_flight.is_empty()
                && !st.detail_loading
                && srv.poll_vote_completed.load(Ordering::SeqCst) >= pv_b + 2
                && srv.detail_get_received.load(Ordering::SeqCst) >= det_rx_poll + 2
        })
        .await,
        "T8-C4 Case B: poll F2 500 must trigger Case B detail reload"
    );
    srv.poll_vote_fail_500.store(false, Ordering::SeqCst);
    let _ = cw.read_with(app, |w, _| {
        let voted = &w.state.detail_post.as_ref().unwrap().post.poll.as_ref().unwrap().viewer_voted_option_ids;
        assert_eq!(voted, &vec!["opt_a".to_string()]);
    });

    // Multi-select rapid opt_a -> opt_b (0 lost update!)
    let _ = cw.update(app, |w, cx| {
        w.navigate_to(CommunityRoute::PostDetail(srv.mpoll_id.clone()), cx);
    });
    assert!(
        pump_until(app, Duration::from_secs(4), |cx| {
            !cw.read(cx).state.detail_loading
                && cw
                    .read(cx)
                    .state
                    .detail_post
                    .as_ref()
                    .is_some_and(|d| d.post.post_id == srv.mpoll_id)
        })
        .await
    );
    srv.poll_vote_in_flight_max.store(0, Ordering::SeqCst);
    let pv_done_m = srv.poll_vote_completed.load(Ordering::SeqCst);
    srv.poll_vote_delay_ms.store(120, Ordering::SeqCst);
    let _ = cw.update(app, |w, cx| {
        w.vote_poll(srv.mpoll_id.clone(), "opt_a".to_string(), cx);
        w.vote_poll(srv.mpoll_id.clone(), "opt_b".to_string(), cx);
    });
    assert!(
        pump_until(app, Duration::from_secs(5), |cx| {
            let st = &cw.read(cx).state;
            st.poll_vote_in_flight.is_empty()
                && st.poll_vote_dirty.is_empty()
                && srv.poll_vote_completed.load(Ordering::SeqCst) >= pv_done_m + 2
        })
        .await,
        "T8-C4 multi-select rapid opt_a->opt_b must flush sequentially without lost update"
    );
    assert_eq!(srv.poll_vote_in_flight_max.load(Ordering::SeqCst), 1);
    let _ = cw.read_with(app, |w, _| {
        let voted = &w.state.detail_post.as_ref().unwrap().post.poll.as_ref().unwrap().viewer_voted_option_ids;
        assert!(
            voted.contains(&"opt_a".to_string()) && voted.contains(&"opt_b".to_string()),
            "T8-C4 multi-select UI must contain BOTH opt_a and opt_b, got {voted:?}"
        );
        let srv_v = srv.multi_poll_voted.lock().unwrap().clone();
        assert!(
            srv_v.contains(&"opt_a".to_string()) && srv_v.contains(&"opt_b".to_string()),
            "T8-C4 multi-select Server must contain BOTH opt_a and opt_b, got {srv_v:?}"
        );
    });
}

async fn run_r8_group_d_award_fences_and_secondary(
    app: &mut AsyncApp,
    shell: &Entity<AppShell>,
    cw: &Entity<CommunityWorkspace>,
    srv: &Arc<R8MockState>,
) {
    use std::sync::atomic::Ordering;

    // ------------------------------------------------------------------
    // T8-D1 (ADV-1 give_award): Case B vs slow load_post_detail + actor-switch fence
    // ------------------------------------------------------------------
    let _ = cw.update(app, |w, cx| {
        w.navigate_to(CommunityRoute::PostDetail(srv.post_id.clone()), cx);
    });
    assert!(
        pump_until(app, Duration::from_secs(4), |cx| {
            !cw.read(cx).state.detail_loading
                && cw
                    .read(cx)
                    .state
                    .detail_post
                    .as_ref()
                    .is_some_and(|d| d.post.post_id == srv.post_id)
        })
        .await
    );
    let det_rx_aw = srv.detail_get_received.load(Ordering::SeqCst);
    let aw_done = srv.post_award_completed.load(Ordering::SeqCst);
    srv.detail_get_delay_ms.store(260, Ordering::SeqCst);
    srv.post_award_delay_ms.store(40, Ordering::SeqCst);
    let _ = cw.update(app, |w, cx| {
        w.load_post_detail(srv.post_id.clone(), cx);
        w.give_award(srv.post_id.clone(), "rocket".to_string(), cx);
    });
    assert!(
        pump_until(app, Duration::from_secs(5), |cx| {
            !cw.read(cx).state.detail_loading
                && srv.post_award_completed.load(Ordering::SeqCst) > aw_done
                && srv.detail_get_received.load(Ordering::SeqCst) >= det_rx_aw + 2
        })
        .await,
        "T8-D1 (ADV-1): give_award must trigger Case B detail reload"
    );
    let _ = cw.read_with(app, |w, _| {
        let ui_rockets = w
            .state
            .detail_post
            .as_ref()
            .unwrap()
            .post
            .awards
            .iter()
            .filter(|a| a.award_kind == "rocket")
            .count();
        let srv_rockets = srv
            .post_awards
            .lock()
            .unwrap()
            .iter()
            .filter(|a| a.award_kind == "rocket")
            .count();
        assert_eq!(ui_rockets, 1);
        assert_eq!(srv_rockets, 1);
    });

    // ------------------------------------------------------------------
    // T8-D2: Actor-Switch Fence + All 4 Busy-Flag Resets + Bot Studio Facade Restore-Before-Fence
    // ------------------------------------------------------------------
    let cp_before = srv.create_post_completed.load(Ordering::SeqCst);
    let up_before = srv.update_post_completed.load(Ordering::SeqCst);
    let cc_before = srv.create_comment_completed.load(Ordering::SeqCst);
    let sig_before = srv.signal_completed.load(Ordering::SeqCst);
    let sum_before = srv.summon_completed.load(Ordering::SeqCst);
    let aw_rx_before = srv.post_award_received.load(Ordering::SeqCst);
    let aw_before = srv.post_award_completed.load(Ordering::SeqCst);
    srv.create_post_delay_ms.store(140, Ordering::SeqCst);
    srv.update_post_delay_ms.store(140, Ordering::SeqCst);
    srv.create_comment_delay_ms.store(140, Ordering::SeqCst);
    srv.signal_delay_ms.store(140, Ordering::SeqCst);
    srv.summon_delay_ms.store(140, Ordering::SeqCst);
    // Wave r12 (Fix 6): give_award has a pre-dispatch actor-switch abort guard to prevent
    // same-tick header theft; let owner_id's give_award genuinely enter the server's in-flight
    // delay BEFORE switch_active_actor(alt_id) so we test the real post-await actor-switch fence.
    srv.post_award_delay_ms.store(220, Ordering::SeqCst);
    let _ = cw.update(app, |w, cx| {
        w.give_award(srv.post_id.clone(), "gold".to_string(), cx);
    });
    assert!(
        pump_until(app, Duration::from_secs(4), |_| {
            srv.post_award_received.load(Ordering::SeqCst) > aw_rx_before
        })
        .await,
        "T8-D2: owner_id give_award must enter server in-flight delay before actor switch"
    );
    let _ = cw.update(app, |w, cx| {
        // Trigger busy flags & delayed RPCs under owner_id
        w.state.quick_post_body = "Delayed quick post".to_string();
        w.submit_quick_post(cx);
        assert!(w.state.quick_post_submitting);

        w.state.quote_post_draft = "Delayed quote".to_string();
        w.submit_quote_post(srv.post_id.clone(), cx);

        w.state.detail_reply_draft = "Delayed reply".to_string();
        w.submit_detail_reply(cx);

        w.report_post_or_comment(srv.post_id.clone(), None, Some(srv.slug.clone()), "spam".to_string(), cx);

        w.summon_bot_with_options(srv.post_id.clone(), Some(srv.alt_id.clone()), Some("hi".to_string()), None, cx);
        assert!(w.state.detail_summon_pending);

        // Composer Edit busy flag
        let p = srv.build_post();
        w.open_composer_edit(p, cx);
        w.state.composer.body = "Delayed edit".to_string();
        w.submit_composer(PostStatus::Published, cx);
        assert!(w.state.composer.submitting);

        // Bot Studio test post busy flag (must restore facade.active_actor_id BEFORE early return!)
        w.state.bot_studio_test_content = "Delayed bot test".to_string();
        w.run_bot_studio_test_post(srv.bot_b_id.clone(), cx);
        assert!(w.state.bot_studio_test_submitting);

        // Switch immediately to alt_id! ALL 4 busy flags must reset synchronously!
        w.switch_active_actor(srv.alt_id.clone(), cx);
        assert!(!w.state.quick_post_submitting, "T8-D2: quick_post_submitting must reset on actor switch");
        assert!(!w.state.detail_summon_pending, "T8-D2: detail_summon_pending must reset on actor switch");
        assert!(!w.state.composer.submitting, "T8-D2: composer.submitting must reset on actor switch");
        assert!(!w.state.bot_studio_test_submitting, "T8-D2: bot_studio_test_submitting must reset on actor switch");
    });
    // Clear toasts right after switch so we can assert ZERO stale toasts leak into alt_id!
    let _ = shell.update(app, |s, _| {
        s.window_runtime.controller.current_toast = None;
        s.window_runtime.controller.toast_queue.clear();
    });
    assert!(
        pump_until(app, Duration::from_secs(5), |_| {
            srv.create_post_completed.load(Ordering::SeqCst) >= cp_before + 3
                && srv.update_post_completed.load(Ordering::SeqCst) > up_before
                && srv.create_comment_completed.load(Ordering::SeqCst) > cc_before
                && srv.signal_completed.load(Ordering::SeqCst) > sig_before
                && srv.summon_completed.load(Ordering::SeqCst) > sum_before
                && srv.post_award_completed.load(Ordering::SeqCst) > aw_before
        })
        .await,
        "T8-D2: all delayed owner RPCs must finish server-side"
    );
    app.background_executor().timer(Duration::from_millis(40)).await;
    let _ = cw.read_with(app, |w, _| {
        assert_eq!(w.state.active_actor_id, srv.alt_id);
        assert_eq!(
            w.facade.active_actor_id().as_deref(),
            Some(srv.alt_id.as_str()),
            "T8-D2 CRITICAL: run_bot_studio_test_post must restore facade active_actor_id to alt_id BEFORE actor_switch_gen early return!"
        );
        assert!(
            w.state.detail_summon_feedback.is_none(),
            "T8-D2: stale summon feedback must NOT leak into alt_id"
        );
    });
    let _ = shell.read_with(app, |s, _| {
        let ctrl = &s.window_runtime.controller;
        let leaked = ctrl
            .current_toast
            .iter()
            .chain(ctrl.toast_queue.iter())
            .any(|t| {
                t.text.contains("Post published")
                    || t.text.contains("Report submitted")
                    || t.text.contains("Bot summoned")
                    || t.text.contains("Bot test post")
                    || t.text.contains("Post saved")
            });
        assert!(!leaked, "T8-D2: stale Actor A toasts must NOT leak into Actor B");
    });

    // Also test publish_draft_now, submit_composer Create, and run_bot_studio_trigger_reply actor-switch fences
    let cp_before_2 = srv.create_post_completed.load(Ordering::SeqCst);
    let up_before_2 = srv.update_post_completed.load(Ordering::SeqCst);
    let sum_before_2 = srv.summon_completed.load(Ordering::SeqCst);
    srv.create_post_delay_ms.store(120, Ordering::SeqCst);
    srv.update_post_delay_ms.store(120, Ordering::SeqCst);
    srv.summon_delay_ms.store(120, Ordering::SeqCst);
    let _ = cw.update(app, |w, cx| {
        w.open_composer_modal(None, cx);
        w.state.composer.body = "Delayed composer create".to_string();
        w.submit_composer(PostStatus::Published, cx);
        w.publish_draft_now(srv.draft_id.clone(), cx);
        w.state.bot_studio_test_last_post_id = Some(srv.post_id.clone());
        w.run_bot_studio_trigger_reply(srv.bot_b_id.clone(), cx);
        w.switch_active_actor(srv.owner_id.clone(), cx);
        assert!(!w.state.composer.submitting && !w.state.bot_studio_test_submitting);
    });
    assert!(
        pump_until(app, Duration::from_secs(5), |_| {
            srv.create_post_completed.load(Ordering::SeqCst) > cp_before_2
                && srv.update_post_completed.load(Ordering::SeqCst) > up_before_2
                && srv.summon_completed.load(Ordering::SeqCst) > sum_before_2
        })
        .await
    );
    srv.draft_published.store(false, Ordering::SeqCst);

    // ------------------------------------------------------------------
    // T8-D3: Non-Single-Flight Case B & Secondary-Action Matrix
    // ------------------------------------------------------------------
    // 1) save_community_wiki & save_community_rules vs slow load_community_page
    let _ = cw.update(app, |w, cx| {
        w.navigate_to(CommunityRoute::CommunityPage(srv.slug.clone()), cx);
    });
    assert!(
        pump_until(app, Duration::from_secs(4), |cx| {
            !cw.read(cx).state.community_loading && cw.read(cx).state.community_detail.is_some()
        })
        .await
    );
    let comm_rx_w = srv.comm_get_received.load(Ordering::SeqCst);
    let wiki_done = srv.wiki_put_completed.load(Ordering::SeqCst);
    srv.comm_get_delay_ms.store(260, Ordering::SeqCst);
    srv.wiki_put_delay_ms.store(40, Ordering::SeqCst);
    let _ = cw.update(app, |w, cx| {
        w.load_community_page(srv.slug.clone(), cx);
        w.state.community_wiki_draft = "Updated Wiki R8".to_string();
        w.save_community_wiki(srv.slug.clone(), cx);
    });
    assert!(
        pump_until(app, Duration::from_secs(5), |cx| {
            !cw.read(cx).state.community_wiki_saving
                && !cw.read(cx).state.community_loading
                && srv.wiki_put_completed.load(Ordering::SeqCst) > wiki_done
                && srv.comm_get_received.load(Ordering::SeqCst) >= comm_rx_w + 2
        })
        .await,
        "T8-D3: save_community_wiki must trigger Case B community reload"
    );
    let _ = cw.read_with(app, |w, _| {
        assert_eq!(w.state.community_detail.as_ref().unwrap().community.wiki_markdown, "Updated Wiki R8");
    });

    let comm_rx_r = srv.comm_get_received.load(Ordering::SeqCst);
    let cp_done_r = srv.comm_patch_completed.load(Ordering::SeqCst);
    srv.comm_get_delay_ms.store(260, Ordering::SeqCst);
    srv.comm_patch_delay_ms.store(40, Ordering::SeqCst);
    let _ = cw.update(app, |w, cx| {
        w.load_community_page(srv.slug.clone(), cx);
        w.state.community_new_rule_title = "Rule One R8".to_string();
        w.add_community_rule(srv.slug.clone(), cx);
    });
    assert!(
        pump_until(app, Duration::from_secs(5), |cx| {
            !cw.read(cx).state.community_rules_saving
                && !cw.read(cx).state.community_loading
                && srv.comm_patch_completed.load(Ordering::SeqCst) > cp_done_r
                && srv.comm_get_received.load(Ordering::SeqCst) >= comm_rx_r + 2
        })
        .await,
        "T8-D3: save_community_rules must trigger Case B community reload"
    );
    let _ = cw.read_with(app, |w, _| {
        assert!(w
            .state
            .community_detail
            .as_ref()
            .unwrap()
            .community
            .rules
            .iter()
            .any(|r| r.title == "Rule One R8"));
    });

    // 2) execute_mod_decision & execute_mod_console_action vs mid-flight load_mod_queue (mod_gen bumped!)
    let _ = cw.update(app, |w, cx| {
        w.navigate_to(CommunityRoute::ModQueue(Some(srv.slug.clone())), cx);
    });
    assert!(
        pump_until(app, Duration::from_secs(4), |cx| {
            !cw.read(cx).state.mod_loading && cw.read(cx).state.mod_queue.len() == 2
        })
        .await
    );
    let ma_done_1 = srv.mod_action_completed.load(Ordering::SeqCst);
    srv.mod_action_delay_ms.store(120, Ordering::SeqCst);
    let _ = cw.update(app, |w, cx| {
        w.execute_mod_decision(
            srv.slug.clone(),
            "mq_r8_1".to_string(),
            "post".to_string(),
            srv.post_id.clone(),
            ModActionKind::Approve,
            cx,
        );
        // Mid-flight load_mod_queue bumps mod_gen! Previously this aborted post-commit reload!
        w.load_mod_queue(srv.slug.clone(), cx);
    });
    assert!(
        pump_until(app, Duration::from_secs(5), |cx| {
            let st = &cw.read(cx).state;
            !st.mod_loading
                && srv.mod_action_completed.load(Ordering::SeqCst) > ma_done_1
                && !st.mod_queue.iter().any(|it| it.item_id == "mq_r8_1")
        })
        .await,
        "T8-D3: execute_mod_decision must reload mod_queue even when mid-flight load_mod_queue bumped mod_gen"
    );
    let ma_done_2 = srv.mod_action_completed.load(Ordering::SeqCst);
    srv.mod_action_delay_ms.store(120, Ordering::SeqCst);
    let _ = cw.update(app, |w, cx| {
        w.execute_mod_console_action(
            srv.slug.clone(),
            ModActionKind::Approve,
            "post".to_string(),
            srv.post_id.clone(),
            Some("mq_r8_2".to_string()),
            cx,
        );
        w.load_mod_queue(srv.slug.clone(), cx);
    });
    assert!(
        pump_until(app, Duration::from_secs(5), |cx| {
            let st = &cw.read(cx).state;
            !st.mod_loading
                && srv.mod_action_completed.load(Ordering::SeqCst) > ma_done_2
                && st.mod_queue.is_empty()
        })
        .await,
        "T8-D3: execute_mod_console_action must reload mod_queue even when mod_gen bumped"
    );

    // 3) publish_draft_now vs slow load_composer_drafts (0 draft resurrection)
    let dr_rx = srv.drafts_get_received.load(Ordering::SeqCst);
    let up_dr = srv.update_post_completed.load(Ordering::SeqCst);
    srv.drafts_get_delay_ms.store(260, Ordering::SeqCst);
    srv.update_post_delay_ms.store(40, Ordering::SeqCst);
    let _ = cw.update(app, |w, cx| {
        w.load_composer_drafts(cx);
        w.publish_draft_now(srv.draft_id.clone(), cx);
    });
    assert!(
        pump_until(app, Duration::from_secs(5), |cx| {
            !cw.read(cx).state.composer.drafts_loading
                && srv.update_post_completed.load(Ordering::SeqCst) > up_dr
                && srv.drafts_get_received.load(Ordering::SeqCst) >= dr_rx + 2
        })
        .await,
        "T8-D3: publish_draft_now must reload drafts when drafts_loading is in-flight"
    );
    let _ = cw.read_with(app, |w, _| {
        assert!(
            !w.state.composer.drafts.iter().any(|d| d.post_id == srv.draft_id),
            "T8-D3: published draft must NOT resurrect"
        );
    });

    // 4) submit_quick_post & submit_composer Edit vs slow load_feed / load_post_detail
    let _ = cw.update(app, |w, cx| {
        w.navigate_to(CommunityRoute::Feed, cx);
    });
    assert!(pump_until(app, Duration::from_secs(4), |cx| !cw.read(cx).state.feed_loading).await);
    let feed_rx_qp = srv.feed_get_received.load(Ordering::SeqCst);
    let cp_qp = srv.create_post_completed.load(Ordering::SeqCst);
    srv.feed_get_delay_ms.store(260, Ordering::SeqCst);
    srv.create_post_delay_ms.store(40, Ordering::SeqCst);
    let _ = cw.update(app, |w, cx| {
        w.load_feed(cx);
        w.state.quick_post_body = "Fresh Quick R8".to_string();
        w.submit_quick_post(cx);
    });
    assert!(
        pump_until(app, Duration::from_secs(5), |cx| {
            !cw.read(cx).state.feed_loading
                && !cw.read(cx).state.quick_post_submitting
                && srv.create_post_completed.load(Ordering::SeqCst) > cp_qp
                && srv.feed_get_received.load(Ordering::SeqCst) >= feed_rx_qp + 2
        })
        .await,
        "T8-D3: submit_quick_post must reload feed when feed_loading is in-flight"
    );
    let _ = cw.read_with(app, |w, _| {
        assert!(w.state.feed_items.iter().any(|it| it.post.body == "Fresh Quick R8"));
    });

    // submit_composer Edit vs slow load_post_detail
    let _ = cw.update(app, |w, cx| {
        w.navigate_to(CommunityRoute::PostDetail(srv.post_id.clone()), cx);
    });
    assert!(pump_until(app, Duration::from_secs(4), |cx| !cw.read(cx).state.detail_loading).await);
    let det_rx_ed = srv.detail_get_received.load(Ordering::SeqCst);
    let up_ed = srv.update_post_completed.load(Ordering::SeqCst);
    srv.detail_get_delay_ms.store(260, Ordering::SeqCst);
    srv.update_post_delay_ms.store(40, Ordering::SeqCst);
    let _ = cw.update(app, |w, cx| {
        w.load_post_detail(srv.post_id.clone(), cx);
        let p = srv.build_post();
        w.open_composer_edit(p, cx);
        w.state.composer.body = "Edited Body R8".to_string();
        w.submit_composer(PostStatus::Published, cx);
    });
    assert!(
        pump_until(app, Duration::from_secs(5), |cx| {
            !cw.read(cx).state.detail_loading
                && !cw.read(cx).state.composer.submitting
                && srv.update_post_completed.load(Ordering::SeqCst) > up_ed
                && srv.detail_get_received.load(Ordering::SeqCst) >= det_rx_ed + 2
        })
        .await,
        "T8-D3: submit_composer Edit must reload PostDetail when detail_loading is in-flight"
    );
    let _ = cw.read_with(app, |w, _| {
        assert_eq!(w.state.detail_post.as_ref().unwrap().post.body, "Edited Body R8");
    });

    // 5) mark_single_notification_read HTTP 500 resync & mark_all_notifications_read unread > 0 resync
    let _ = cw.update(app, |w, cx| {
        w.navigate_to(CommunityRoute::Notifications, cx);
    });
    assert!(
        pump_until(app, Duration::from_secs(4), |cx| {
            !cw.read(cx).state.notifications_loading && cw.read(cx).state.unread_notifications == 2
        })
        .await
    );
    let ng_before_500 = srv.notifs_get_completed.load(Ordering::SeqCst);
    let nr_before_500 = srv.notif_read_completed.load(Ordering::SeqCst);
    srv.notif_read_fail_500.store(true, Ordering::SeqCst);
    let _ = cw.update(app, |w, cx| {
        w.mark_single_notification_read("n_r8_1".to_string(), cx);
    });
    assert!(
        pump_until(app, Duration::from_secs(4), |cx| {
            !cw.read(cx).state.notifications_loading
                && srv.notif_read_completed.load(Ordering::SeqCst) > nr_before_500
                && srv.notifs_get_completed.load(Ordering::SeqCst) > ng_before_500
        })
        .await,
        "T8-D3: mark_single_notification_read HTTP 500 must resync via load_notifications"
    );
    srv.notif_read_fail_500.store(false, Ordering::SeqCst);
    let _ = cw.read_with(app, |w, _| {
        assert_eq!(w.state.unread_notifications, 2, "T8-D3: HTTP 500 resync restores unread=2");
        assert!(w.state.notifications.iter().any(|n| n.notif_id == "n_r8_1" && !n.is_read));
    });

    // mark_all_notifications_read with unread > 0 mismatch when notifications_gen bumped
    let ng_before_all = srv.notifs_get_completed.load(Ordering::SeqCst);
    let nr_before_all = srv.notif_read_completed.load(Ordering::SeqCst);
    srv.notif_read_override_unread.store(1, Ordering::SeqCst);
    let _ = cw.update(app, |w, cx| {
        w.mark_all_notifications_read(cx);
        w.state.notifications_gen = w.state.notifications_gen.wrapping_add(1);
    });
    assert!(
        pump_until(app, Duration::from_secs(4), |cx| {
            !cw.read(cx).state.notifications_loading
                && srv.notif_read_completed.load(Ordering::SeqCst) > nr_before_all
                && srv.notifs_get_completed.load(Ordering::SeqCst) > ng_before_all
        })
        .await,
        "T8-D3: mark_all_notifications_read must resync when unread > 0 mismatches stale UI"
    );

    // 6) save_selected_bot_edits switching Bot A -> Bot B mid-save -> 0 selection hijack!
    let _ = cw.update(app, |w, cx| {
        w.navigate_to(CommunityRoute::BotStudio, cx);
        let bot_a = w
            .state
            .identities
            .as_ref()
            .unwrap()
            .bot_actors
            .iter()
            .find(|b| b.actor_id == srv.alt_id)
            .cloned()
            .unwrap();
        let bot_b = w
            .state
            .identities
            .as_ref()
            .unwrap()
            .bot_actors
            .iter()
            .find(|b| b.actor_id == srv.bot_b_id)
            .cloned()
            .unwrap();
        w.select_bot_for_studio(bot_a, cx);
        w.state.bot_studio_edit.display_name = "Bot A Renamed".to_string();
        srv.actor_patch_delay_ms.store(140, Ordering::SeqCst);
        w.save_selected_bot_edits(cx);
        // Immediately switch selection to Bot B while Bot A save is in-flight!
        w.select_bot_for_studio(bot_b, cx);
    });
    assert!(
        pump_until(app, Duration::from_secs(4), |cx| {
            !cw.read(cx).state.bot_studio_edit.saving
                && srv.actor_patch_completed.load(Ordering::SeqCst) >= 1
        })
        .await
    );
    let _ = cw.read_with(app, |w, _| {
        assert_eq!(
            w.state.bot_studio_selected_bot_id.as_deref(),
            Some(srv.bot_b_id.as_str()),
            "T8-D3: save_selected_bot_edits must NOT hijack selection back from Bot B to Bot A"
        );
    });

    // 7) delete_post_action vs slow load_feed & slow load_post_detail (!navigate_back) -> 0 ghost resurrection!
    let _ = cw.update(app, |w, cx| {
        w.navigate_to(CommunityRoute::PostDetail(srv.post_id.clone()), cx);
    });
    assert!(pump_until(app, Duration::from_secs(4), |cx| !cw.read(cx).state.detail_loading).await);
    let del_done = srv.delete_post_completed.load(Ordering::SeqCst);
    let feed_rx_del = srv.feed_get_received.load(Ordering::SeqCst);
    srv.detail_get_delay_ms.store(260, Ordering::SeqCst);
    srv.feed_get_delay_ms.store(260, Ordering::SeqCst);
    srv.delete_post_delay_ms.store(40, Ordering::SeqCst);
    let _ = cw.update(app, |w, cx| {
        w.load_post_detail(srv.post_id.clone(), cx);
        w.load_feed(cx);
        w.delete_post_action(srv.post_id.clone(), false, cx);
    });
    assert!(
        pump_until(app, Duration::from_secs(5), |cx| {
            let st = &cw.read(cx).state;
            !st.detail_loading
                && !st.feed_loading
                && srv.delete_post_completed.load(Ordering::SeqCst) > del_done
                && srv.feed_get_received.load(Ordering::SeqCst) >= feed_rx_del + 2
        })
        .await,
        "T8-D3: delete_post_action must fence detail_gen & reload feed_loading without resurrecting deleted post"
    );
    app.background_executor().timer(Duration::from_millis(60)).await;
    let _ = cw.read_with(app, |w, _| {
        assert!(w.state.detail_post.is_none(), "T8-D3: deleted post must NOT resurrect into detail_post");
        assert!(
            !w.state.feed_items.iter().any(|it| it.post.post_id == srv.post_id),
            "T8-D3: deleted post must NOT resurrect into feed_items"
        );
    });
}

// =============================================================================
// Wave r9 Regression Suite: Cross-Lane, Facade Isolation, Active-Route Precedence,
// ADV-1..ADV-7B, BUG-HUNT-1..6, and All 8 Sibling Modal Mutexes (T9-01..T9-09)
// =============================================================================

struct R9MockState {
    alice_id: String,
    bot_a_id: String,
    bot_b_id: String,
    bob_id: String,
    slug: String,
    p_main: String,
    p_saved: String,
    p_page2: String,
    p_cpage2: String,

    // T9-01 header capture
    last_create_post_actor: std::sync::Mutex<Option<String>>,
    last_vote_post_actor: std::sync::Mutex<Option<String>>,
    last_vote_dir: std::sync::atomic::AtomicI8,

    // Per-(actor_id, post_id) vote/like/repost/bookmark truth
    votes: std::sync::Mutex<std::collections::HashMap<(String, String), i8>>,
    likes: std::sync::Mutex<std::collections::HashSet<(String, String)>>,
    reposts: std::sync::Mutex<std::collections::HashSet<(String, String)>>,
    saved_bookmarked: std::sync::atomic::AtomicBool,
    deleted_posts: std::sync::Mutex<std::collections::HashSet<String>>,

    // T9-02 Bob follow/block state + cross-lane concurrency meter
    bob_blocked: std::sync::atomic::AtomicBool,
    bob_following: std::sync::atomic::AtomicBool,
    bob_followers: std::sync::atomic::AtomicU64,
    bot_a_following: std::sync::atomic::AtomicBool,
    bot_a_followers: std::sync::atomic::AtomicU64,
    bot_a_display_name: std::sync::Mutex<String>,
    follow_block_in_flight: std::sync::atomic::AtomicUsize,
    follow_block_max_in_flight: std::sync::atomic::AtomicUsize,
    follow_delay_ms: std::sync::atomic::AtomicU64,
    follow_fail_500: std::sync::atomic::AtomicBool,
    follow_completed: std::sync::atomic::AtomicUsize,
    block_delay_ms: std::sync::atomic::AtomicU64,
    block_fail_500: std::sync::atomic::AtomicBool,
    block_completed: std::sync::atomic::AtomicUsize,

    // T9-03 Lists (with distinct pre/post-commit delays & timestamps)
    lists: std::sync::Mutex<Vec<CommunityListDto>>,
    list_ts_clock: std::sync::atomic::AtomicI64,
    list_toggle_pre_received: std::sync::atomic::AtomicUsize,
    list_toggle_pre_delay_ms: std::sync::atomic::AtomicU64,
    list_toggle_post_delay_ms: std::sync::atomic::AtomicU64,
    list_toggle_fail_500: std::sync::atomic::AtomicBool,
    list_toggle_completed: std::sync::atomic::AtomicUsize,
    list_editor_delay_ms: std::sync::atomic::AtomicU64,
    list_editor_completed: std::sync::atomic::AtomicUsize,
    lists_get_delay_ms: std::sync::atomic::AtomicU64,
    lists_get_received: std::sync::atomic::AtomicUsize,
    lists_get_completed: std::sync::atomic::AtomicUsize,

    // T9-04 / T9-05 / T9-07 feed & detail delays + counters
    feed_get_delay_ms: std::sync::atomic::AtomicU64,
    feed_more_delay_ms: std::sync::atomic::AtomicU64,
    feed_get_received: std::sync::atomic::AtomicUsize,
    feed_get_completed: std::sync::atomic::AtomicUsize,
    saved_get_received: std::sync::atomic::AtomicUsize,
    detail_get_delay_ms: std::sync::atomic::AtomicU64,
    detail_get_completed: std::sync::atomic::AtomicUsize,
    post_vote_delay_ms: std::sync::atomic::AtomicU64,
    post_vote_completed: std::sync::atomic::AtomicUsize,
    post_like_completed: std::sync::atomic::AtomicUsize,
    post_repost_completed: std::sync::atomic::AtomicUsize,
    post_bookmark_completed: std::sync::atomic::AtomicUsize,
    create_post_received: std::sync::atomic::AtomicUsize,
    create_post_delay_ms: std::sync::atomic::AtomicU64,
    create_post_completed: std::sync::atomic::AtomicUsize,
    delete_post_delay_ms: std::sync::atomic::AtomicU64,
    delete_post_completed: std::sync::atomic::AtomicUsize,

    // T9-06 / T9-08 / T9-09 community, wiki, rules, automod, actor patch
    comm_joined: std::sync::atomic::AtomicBool,
    comm_members: std::sync::atomic::AtomicU64,
    comm_wiki: std::sync::Mutex<String>,
    comm_wiki_rev: std::sync::atomic::AtomicU32,
    comm_rules: std::sync::Mutex<Vec<gpui_telegram_proto::community_contracts::CommunityRuleDto>>,
    comm_automod_rules: std::sync::Mutex<Vec<AutoModRuleDto>>,
    communities_list_delay_ms: std::sync::atomic::AtomicU64,
    communities_list_received: std::sync::atomic::AtomicUsize,
    communities_list_completed: std::sync::atomic::AtomicUsize,
    comm_get_delay_ms: std::sync::atomic::AtomicU64,
    comm_feed_delay_ms: std::sync::atomic::AtomicU64,
    comm_more_delay_ms: std::sync::atomic::AtomicU64,
    comm_get_received: std::sync::atomic::AtomicUsize,
    comm_get_completed: std::sync::atomic::AtomicUsize,
    comm_more_completed: std::sync::atomic::AtomicUsize,
    join_delay_ms: std::sync::atomic::AtomicU64,
    join_completed: std::sync::atomic::AtomicUsize,
    wiki_put_delay_ms: std::sync::atomic::AtomicU64,
    wiki_put_completed: std::sync::atomic::AtomicUsize,
    rules_patch_delay_ms: std::sync::atomic::AtomicU64,
    rules_patch_completed: std::sync::atomic::AtomicUsize,
    automod_patch_delay_ms: std::sync::atomic::AtomicU64,
    automod_patch_completed: std::sync::atomic::AtomicUsize,
    create_comm_delay_ms: std::sync::atomic::AtomicU64,
    create_comm_completed: std::sync::atomic::AtomicUsize,
    actor_patch_delays_by_name: std::sync::Mutex<std::collections::HashMap<String, u64>>,
    actor_patch_completed: std::sync::atomic::AtomicUsize,
    profile_get_completed: std::sync::atomic::AtomicUsize,
}

impl R9MockState {
    fn build_actor(&self, actor_id: &str) -> CommunityActorDto {
        use std::sync::atomic::Ordering;
        if actor_id == self.alice_id {
            let mut a = mock_actor_dto(&self.alice_id, ActorKind::Human, "r9_alice", "R9 Alice");
            a.linked_user_id = Some(909001);
            a
        } else if actor_id == self.bot_a_id {
            let mut a = mock_actor_dto(
                &self.bot_a_id,
                ActorKind::Bot,
                "r9_bot_a",
                &self.bot_a_display_name.lock().unwrap(),
            );
            a.owner_user_id = Some(909001);
            a.is_following = self.bot_a_following.load(Ordering::SeqCst);
            a.followers_count = self.bot_a_followers.load(Ordering::SeqCst);
            a
        } else if actor_id == self.bot_b_id {
            let mut a = mock_actor_dto(&self.bot_b_id, ActorKind::Bot, "r9_bot_b", "R9 Bot B");
            a.owner_user_id = Some(909001);
            a
        } else {
            let mut a = mock_actor_dto(&self.bob_id, ActorKind::Human, "r9_bob", "R9 Bob");
            a.is_blocked = self.bob_blocked.load(Ordering::SeqCst);
            a.is_following = self.bob_following.load(Ordering::SeqCst);
            a.followers_count = self.bob_followers.load(Ordering::SeqCst);
            a
        }
    }

    fn build_post(&self, post_id: &str, viewer: &str) -> CommunityPostDto {
        use std::sync::atomic::Ordering;
        let votes = self.votes.lock().unwrap();
        let viewer_vote = votes
            .get(&(viewer.to_string(), post_id.to_string()))
            .copied()
            .unwrap_or(0);
        let upvotes: i64 = votes
            .iter()
            .filter(|((_, pid), d)| pid == post_id && **d == 1)
            .count() as i64;
        let downvotes: i64 = votes
            .iter()
            .filter(|((_, pid), d)| pid == post_id && **d == -1)
            .count() as i64;
        drop(votes);
        let likes = self.likes.lock().unwrap();
        let viewer_liked = likes.contains(&(viewer.to_string(), post_id.to_string()));
        let likes_count = likes.iter().filter(|(_, pid)| pid == post_id).count() as u64;
        drop(likes);
        let reposts = self.reposts.lock().unwrap();
        let viewer_reposted = reposts.contains(&(viewer.to_string(), post_id.to_string()));
        let reposts_count = reposts.iter().filter(|(_, pid)| pid == post_id).count() as u64;
        drop(reposts);
        let bk = post_id == self.p_saved && self.saved_bookmarked.load(Ordering::SeqCst);
        let mut p = mock_post_dto(
            post_id,
            &self.alice_id,
            bk,
            bk.then_some("All"),
            u64::from(bk),
        );
        p.community_slug = Some(self.slug.clone());
        p.viewer_vote = viewer_vote;
        p.upvotes = upvotes;
        p.downvotes = downvotes;
        p.score = upvotes - downvotes;
        p.viewer_liked = viewer_liked;
        p.likes_count = likes_count;
        p.viewer_reposted = viewer_reposted;
        p.reposts_count = reposts_count;
        p
    }

    fn build_community(&self) -> SubCommunityDto {
        use std::sync::atomic::Ordering;
        let mut c = mock_community_dto(&self.slug, &self.alice_id, Some(CommunityMemberRole::Owner));
        c.is_joined = self.comm_joined.load(Ordering::SeqCst);
        c.member_count = self.comm_members.load(Ordering::SeqCst);
        c.wiki_markdown = self.comm_wiki.lock().unwrap().clone();
        c.wiki_revision = self.comm_wiki_rev.load(Ordering::SeqCst);
        c.rules = self.comm_rules.lock().unwrap().clone();
        c.automod_rules = self.comm_automod_rules.lock().unwrap().clone();
        c
    }
}

fn spawn_r9_server(
    rt: &tokio::runtime::Handle,
) -> (std::net::SocketAddr, Arc<R9MockState>, AbortOnDrop) {
    use gpui_telegram_proto::community_contracts::{
        CommunityWikiRevisionDto, CreateCommunityRequest, CreatePostResponse, FollowActionResponse,
        JoinCommunityResponse, RepostResponse, UpdateCommunityActorRequest,
        UpdateCommunityWikiRequest,
    };
    use std::sync::atomic::{AtomicBool, AtomicI64, AtomicI8, AtomicU32, AtomicU64, AtomicUsize, Ordering};
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    let alice_id = "u:909001".to_string();
    let bot_a_id = "b:909002".to_string();
    let bot_b_id = "b:909003".to_string();
    let bob_id = "u:909300".to_string();
    let slug = "rust".to_string();
    let p_main = "p_r9_main".to_string();
    let p_saved = "p_r9_saved".to_string();
    let p_page2 = "p_r9_page2".to_string();
    let p_cpage2 = "p_r9_cpage2".to_string();

    let mut initial_votes = std::collections::HashMap::new();
    initial_votes.insert((alice_id.clone(), p_main.clone()), 1i8);

    let mut l_post = mock_list_dto("l_adv1_post", &alice_id, "Old ADV1");
    l_post.updated_at_ms = 1_000;
    let mut l_pre = mock_list_dto("l_adv1_pre", &alice_id, "Old Pre");
    l_pre.updated_at_ms = 2_000;
    let seed_lists = vec![l_post, l_pre];
    let seed_automod = vec![AutoModRuleDto {
        rule_id: "am_r9_1".to_string(),
        community_id: format!("comm_{slug}"),
        name: "Block Spam".to_string(),
        enabled: true,
        priority: 1,
        keywords: vec!["spam".to_string()],
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
    }];

    let state = Arc::new(R9MockState {
        alice_id,
        bot_a_id,
        bot_b_id,
        bob_id,
        slug,
        p_main,
        p_saved,
        p_page2,
        p_cpage2,
        last_create_post_actor: std::sync::Mutex::new(None),
        last_vote_post_actor: std::sync::Mutex::new(None),
        last_vote_dir: AtomicI8::new(0),
        votes: std::sync::Mutex::new(initial_votes),
        likes: std::sync::Mutex::new(std::collections::HashSet::new()),
        reposts: std::sync::Mutex::new(std::collections::HashSet::new()),
        saved_bookmarked: AtomicBool::new(true),
        deleted_posts: std::sync::Mutex::new(std::collections::HashSet::new()),
        bob_blocked: AtomicBool::new(false),
        bob_following: AtomicBool::new(false),
        bob_followers: AtomicU64::new(0),
        bot_a_following: AtomicBool::new(false),
        bot_a_followers: AtomicU64::new(2),
        bot_a_display_name: std::sync::Mutex::new("R9 Bot A".to_string()),
        follow_block_in_flight: AtomicUsize::new(0),
        follow_block_max_in_flight: AtomicUsize::new(0),
        follow_delay_ms: AtomicU64::new(0),
        follow_fail_500: AtomicBool::new(false),
        follow_completed: AtomicUsize::new(0),
        block_delay_ms: AtomicU64::new(0),
        block_fail_500: AtomicBool::new(false),
        block_completed: AtomicUsize::new(0),
        lists: std::sync::Mutex::new(seed_lists),
        list_ts_clock: AtomicI64::new(3_000),
        list_toggle_pre_received: AtomicUsize::new(0),
        list_toggle_pre_delay_ms: AtomicU64::new(0),
        list_toggle_post_delay_ms: AtomicU64::new(0),
        list_toggle_fail_500: AtomicBool::new(false),
        list_toggle_completed: AtomicUsize::new(0),
        list_editor_delay_ms: AtomicU64::new(0),
        list_editor_completed: AtomicUsize::new(0),
        lists_get_delay_ms: AtomicU64::new(0),
        lists_get_received: AtomicUsize::new(0),
        lists_get_completed: AtomicUsize::new(0),
        feed_get_delay_ms: AtomicU64::new(0),
        feed_more_delay_ms: AtomicU64::new(0),
        feed_get_received: AtomicUsize::new(0),
        feed_get_completed: AtomicUsize::new(0),
        saved_get_received: AtomicUsize::new(0),
        detail_get_delay_ms: AtomicU64::new(0),
        detail_get_completed: AtomicUsize::new(0),
        post_vote_delay_ms: AtomicU64::new(0),
        post_vote_completed: AtomicUsize::new(0),
        post_like_completed: AtomicUsize::new(0),
        post_repost_completed: AtomicUsize::new(0),
        post_bookmark_completed: AtomicUsize::new(0),
        create_post_received: AtomicUsize::new(0),
        create_post_delay_ms: AtomicU64::new(0),
        create_post_completed: AtomicUsize::new(0),
        delete_post_delay_ms: AtomicU64::new(0),
        delete_post_completed: AtomicUsize::new(0),
        comm_joined: AtomicBool::new(false),
        comm_members: AtomicU64::new(10),
        comm_wiki: std::sync::Mutex::new("Initial Wiki".to_string()),
        comm_wiki_rev: AtomicU32::new(1),
        comm_rules: std::sync::Mutex::new(Vec::new()),
        comm_automod_rules: std::sync::Mutex::new(seed_automod),
        communities_list_delay_ms: AtomicU64::new(0),
        communities_list_received: AtomicUsize::new(0),
        communities_list_completed: AtomicUsize::new(0),
        comm_get_delay_ms: AtomicU64::new(0),
        comm_feed_delay_ms: AtomicU64::new(0),
        comm_more_delay_ms: AtomicU64::new(0),
        comm_get_received: AtomicUsize::new(0),
        comm_get_completed: AtomicUsize::new(0),
        comm_more_completed: AtomicUsize::new(0),
        join_delay_ms: AtomicU64::new(0),
        join_completed: AtomicUsize::new(0),
        wiki_put_delay_ms: AtomicU64::new(0),
        wiki_put_completed: AtomicUsize::new(0),
        rules_patch_delay_ms: AtomicU64::new(0),
        rules_patch_completed: AtomicUsize::new(0),
        automod_patch_delay_ms: AtomicU64::new(0),
        automod_patch_completed: AtomicUsize::new(0),
        create_comm_delay_ms: AtomicU64::new(0),
        create_comm_completed: AtomicUsize::new(0),
        actor_patch_delays_by_name: std::sync::Mutex::new(std::collections::HashMap::new()),
        actor_patch_completed: AtomicUsize::new(0),
        profile_get_completed: AtomicUsize::new(0),
    });

    let std_listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind r9 server");
    std_listener.set_nonblocking(true).expect("nonblocking");
    let addr = std_listener.local_addr().expect("r9 local_addr");
    let st_accept = Arc::clone(&state);
    let join = rt.spawn(async move {
        let listener = tokio::net::TcpListener::from_std(std_listener).expect("tokio listener");
        loop {
            let Ok((mut sock, _)) = listener.accept().await else {
                break;
            };
            let st = Arc::clone(&st_accept);
            tokio::spawn(async move {
                let mut buf = Vec::with_capacity(8192);
                let mut tmp = [0u8; 4096];
                let mut header_end = None;
                let mut content_len = 0usize;
                loop {
                    let Ok(n) = sock.read(&mut tmp).await else {
                        return;
                    };
                    if n == 0 {
                        return;
                    }
                    buf.extend_from_slice(&tmp[..n]);
                    if header_end.is_none() {
                        if let Some(pos) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
                            header_end = Some(pos + 4);
                            let hdr = String::from_utf8_lossy(&buf[..pos]);
                            for line in hdr.lines() {
                                if let Some((k, v)) = line.split_once(':') {
                                    if k.eq_ignore_ascii_case("content-length") {
                                        content_len = v.trim().parse().unwrap_or(0);
                                    }
                                }
                            }
                        }
                    }
                    if let Some(he) = header_end {
                        if buf.len() >= he + content_len {
                            break;
                        }
                    }
                }
                let he = header_end.unwrap();
                let hdr_str = String::from_utf8_lossy(&buf[..he]).to_string();
                let mut lines = hdr_str.lines();
                let req_line = lines.next().unwrap_or_default();
                let mut parts = req_line.split_whitespace();
                let method = parts.next().unwrap_or("GET").to_string();
                let raw_target = parts.next().unwrap_or("/").to_string();
                let (raw_path, query) = raw_target
                    .split_once('?')
                    .map(|(p, q)| (p.to_string(), q.to_string()))
                    .unwrap_or((raw_target, String::new()));
                let path = raw_path.replace("%3A", ":").replace("%3a", ":");
                let mut acting_actor: Option<String> = None;
                for line in lines {
                    if let Some((k, v)) = line.split_once(':') {
                        if k.eq_ignore_ascii_case("x-community-actor-id") {
                            acting_actor = Some(v.trim().to_string());
                        }
                    }
                }
                let viewer = acting_actor.clone().unwrap_or_else(|| st.alice_id.clone());
                let body_bytes = &buf[he..he + content_len];

                let (status, resp_body): (&str, String) = if method == "GET"
                    && path == "/api/v1/community/identities"
                {
                    let human = st.build_actor(&st.alice_id);
                    let bot_a = st.build_actor(&st.bot_a_id);
                    let bot_b = st.build_actor(&st.bot_b_id);
                    let resp = CommunityIdentitiesResponse {
                        active_actor_id: viewer.clone(),
                        human_actor: Some(human.clone()),
                        bot_actors: vec![bot_a.clone(), bot_b.clone()],
                        all_actors: vec![human, bot_a, bot_b],
                    };
                    ("200 OK", serde_json::to_string(&resp).unwrap())
                } else if method == "GET" && path == "/api/v1/community/algo-prefs" {
                    ("200 OK", serde_json::to_string(&AlgoPreferencesDto::default()).unwrap())
                } else if method == "GET" && path == "/api/v1/community/trending" {
                    ("200 OK", serde_json::to_string(&TrendingResponse::default()).unwrap())
                } else if method == "GET" && path == "/api/v1/community/notifications" {
                    ("200 OK", serde_json::to_string(&NotificationsListResponse::default()).unwrap())
                } else if method == "GET" && path == "/api/v1/community/search" {
                    let resp = CommunitySearchResultDto {
                        query: String::new(),
                        tab: CommunitySearchTab::Top,
                        posts: Vec::new(),
                        actors: vec![st.build_actor(&st.bob_id)],
                        communities: vec![st.build_community()],
                        total_hits: 1,
                        next_offset: None,
                    };
                    ("200 OK", serde_json::to_string(&resp).unwrap())
                } else if method == "GET" && path == "/api/v1/community/communities" {
                    st.communities_list_received.fetch_add(1, Ordering::SeqCst);
                    let snap = ListCommunitiesResponse {
                        communities: vec![st.build_community()],
                    };
                    let delay = st.communities_list_delay_ms.swap(0, Ordering::SeqCst);
                    if delay > 0 {
                        tokio::time::sleep(Duration::from_millis(delay)).await;
                    }
                    st.communities_list_completed.fetch_add(1, Ordering::SeqCst);
                    ("200 OK", serde_json::to_string(&snap).unwrap())
                } else if method == "POST" && path == "/api/v1/community/communities" {
                    let req: CreateCommunityRequest = serde_json::from_slice(body_bytes).expect("cc");
                    let delay = st.create_comm_delay_ms.swap(0, Ordering::SeqCst);
                    if delay > 0 {
                        tokio::time::sleep(Duration::from_millis(delay)).await;
                    }
                    st.create_comm_completed.fetch_add(1, Ordering::SeqCst);
                    let mut c = st.build_community();
                    c.community_id = format!("c_{}", req.slug);
                    c.slug = req.slug;
                    c.name = req.name;
                    ("200 OK", serde_json::to_string(&c).unwrap())
                } else if method == "GET" && path == format!("/api/v1/community/communities/{}", st.slug) {
                    st.comm_get_received.fetch_add(1, Ordering::SeqCst);
                    let snap = CommunityDetailResponse {
                        community: st.build_community(),
                        pinned_posts: Vec::new(),
                        moderators: vec![st.build_actor(&st.alice_id), st.build_actor(&st.bob_id)],
                        wiki_revisions: Vec::new(),
                    };
                    let delay = st.comm_get_delay_ms.swap(0, Ordering::SeqCst);
                    if delay > 0 {
                        tokio::time::sleep(Duration::from_millis(delay)).await;
                    }
                    st.comm_get_completed.fetch_add(1, Ordering::SeqCst);
                    ("200 OK", serde_json::to_string(&snap).unwrap())
                } else if method == "PATCH" && path == format!("/api/v1/community/communities/{}", st.slug) {
                    let req: UpdateCommunityRequest = serde_json::from_slice(body_bytes).expect("uc");
                    let is_automod = req.automod_rules.is_some();
                    let delay = if is_automod {
                        st.automod_patch_delay_ms.swap(0, Ordering::SeqCst)
                    } else {
                        st.rules_patch_delay_ms.swap(0, Ordering::SeqCst)
                    };
                    if delay > 0 {
                        tokio::time::sleep(Duration::from_millis(delay)).await;
                    }
                    if let Some(rules) = req.rules {
                        *st.comm_rules.lock().unwrap() = rules;
                    }
                    if let Some(am) = req.automod_rules {
                        *st.comm_automod_rules.lock().unwrap() = am;
                    }
                    if is_automod {
                        st.automod_patch_completed.fetch_add(1, Ordering::SeqCst);
                    } else {
                        st.rules_patch_completed.fetch_add(1, Ordering::SeqCst);
                    }
                    ("200 OK", serde_json::to_string(&st.build_community()).unwrap())
                } else if method == "PUT" && path == format!("/api/v1/community/communities/{}/wiki", st.slug) {
                    let req: UpdateCommunityWikiRequest = serde_json::from_slice(body_bytes).expect("wiki");
                    let delay = st.wiki_put_delay_ms.swap(0, Ordering::SeqCst);
                    if delay > 0 {
                        tokio::time::sleep(Duration::from_millis(delay)).await;
                    }
                    *st.comm_wiki.lock().unwrap() = req.markdown.clone();
                    let rev_num = st.comm_wiki_rev.fetch_add(1, Ordering::SeqCst) + 1;
                    st.wiki_put_completed.fetch_add(1, Ordering::SeqCst);
                    let rev = CommunityWikiRevisionDto {
                        revision: rev_num,
                        editor_actor_id: viewer,
                        editor_handle: "r9_alice".to_string(),
                        markdown: req.markdown,
                        summary: req.summary,
                        created_at_ms: 1_750_000_005_000,
                    };
                    ("200 OK", serde_json::to_string(&rev).unwrap())
                } else if (method == "POST" || method == "DELETE")
                    && path == format!("/api/v1/community/communities/{}/join", st.slug)
                {
                    let delay = st.join_delay_ms.swap(0, Ordering::SeqCst);
                    if delay > 0 {
                        tokio::time::sleep(Duration::from_millis(delay)).await;
                    }
                    let want = method == "POST";
                    let prev = st.comm_joined.swap(want, Ordering::SeqCst);
                    if !prev && want {
                        st.comm_members.fetch_add(1, Ordering::SeqCst);
                    } else if prev && !want {
                        st.comm_members.fetch_sub(1, Ordering::SeqCst);
                    }
                    st.join_completed.fetch_add(1, Ordering::SeqCst);
                    let is_alice = viewer == st.alice_id;
                    let resp = JoinCommunityResponse {
                        community_slug: st.slug.clone(),
                        is_joined: want,
                        member_count: st.comm_members.load(Ordering::SeqCst),
                        viewer_role: if want && is_alice {
                            Some(CommunityMemberRole::Owner)
                        } else if want {
                            Some(CommunityMemberRole::Member)
                        } else {
                            None
                        },
                    };
                    ("200 OK", serde_json::to_string(&resp).unwrap())
                } else if method == "GET" && path == format!("/api/v1/community/communities/{}/mod-queue", st.slug) {
                    ("200 OK", serde_json::to_string(&ModQueueResponse::default()).unwrap())
                } else if method == "GET" && path == format!("/api/v1/community/communities/{}/mod-logs", st.slug) {
                    ("200 OK", serde_json::to_string(&ModLogsResponse::default()).unwrap())
                } else if method == "GET" && path.starts_with("/api/v1/community/actors/") {
                    let key = path.trim_start_matches("/api/v1/community/actors/");
                    let aid = if key == "r9_bob" || key == st.bob_id {
                        st.bob_id.clone()
                    } else if key == "r9_bot_a" || key == st.bot_a_id {
                        st.bot_a_id.clone()
                    } else {
                        st.alice_id.clone()
                    };
                    st.profile_get_completed.fetch_add(1, Ordering::SeqCst);
                    let resp = ActorProfileResponse {
                        actor: st.build_actor(&aid),
                        recent_posts: Vec::new(),
                        recent_comments: Vec::new(),
                        moderated_communities: Vec::new(),
                    };
                    ("200 OK", serde_json::to_string(&resp).unwrap())
                } else if method == "PATCH" && path.starts_with("/api/v1/community/actors/") {
                    let aid = path.trim_start_matches("/api/v1/community/actors/").to_string();
                    let req: UpdateCommunityActorRequest = serde_json::from_slice(body_bytes).expect("ua");
                    let delay = req
                        .display_name
                        .as_deref()
                        .and_then(|n| st.actor_patch_delays_by_name.lock().unwrap().remove(n))
                        .unwrap_or(0);
                    if delay > 0 {
                        tokio::time::sleep(Duration::from_millis(delay)).await;
                    }
                    if aid == st.bot_a_id {
                        if let Some(name) = req.display_name {
                            *st.bot_a_display_name.lock().unwrap() = name;
                        }
                    }
                    st.actor_patch_completed.fetch_add(1, Ordering::SeqCst);
                    ("200 OK", serde_json::to_string(&st.build_actor(&aid)).unwrap())
                } else if (method == "POST" || method == "DELETE")
                    && path.starts_with("/api/v1/community/actors/")
                    && path.ends_with("/follow")
                {
                    let target_id = path
                        .trim_start_matches("/api/v1/community/actors/")
                        .trim_end_matches("/follow")
                        .to_string();
                    let is_bob = target_id == st.bob_id;
                    if is_bob {
                        let cur = st.follow_block_in_flight.fetch_add(1, Ordering::SeqCst) + 1;
                        st.follow_block_max_in_flight.fetch_max(cur, Ordering::SeqCst);
                    }
                    let delay = st.follow_delay_ms.swap(0, Ordering::SeqCst);
                    if delay > 0 {
                        tokio::time::sleep(Duration::from_millis(delay)).await;
                    }
                    let res = if st.follow_fail_500.load(Ordering::SeqCst) {
                        st.follow_completed.fetch_add(1, Ordering::SeqCst);
                        ("500 Internal Server Error", r#"{"error":"follow_500"}"#.to_string())
                    } else if is_bob && method == "POST" && st.bob_blocked.load(Ordering::SeqCst) {
                        st.follow_completed.fetch_add(1, Ordering::SeqCst);
                        ("403 Forbidden", r#"{"error":"blocked"}"#.to_string())
                    } else {
                        let want = method == "POST";
                        let (fl, cnt) = if is_bob {
                            let prev = st.bob_following.swap(want, Ordering::SeqCst);
                            if !prev && want {
                                st.bob_followers.fetch_add(1, Ordering::SeqCst);
                            } else if prev && !want {
                                st.bob_followers.fetch_sub(1, Ordering::SeqCst);
                            }
                            (want, st.bob_followers.load(Ordering::SeqCst))
                        } else {
                            let prev = st.bot_a_following.swap(want, Ordering::SeqCst);
                            if !prev && want {
                                st.bot_a_followers.fetch_add(1, Ordering::SeqCst);
                            } else if prev && !want {
                                st.bot_a_followers.fetch_sub(1, Ordering::SeqCst);
                            }
                            (want, st.bot_a_followers.load(Ordering::SeqCst))
                        };
                        st.follow_completed.fetch_add(1, Ordering::SeqCst);
                        let resp = FollowActionResponse {
                            actor_id: target_id,
                            is_following: fl,
                            followers_count: cnt,
                        };
                        ("200 OK", serde_json::to_string(&resp).unwrap())
                    };
                    if is_bob {
                        st.follow_block_in_flight.fetch_sub(1, Ordering::SeqCst);
                    }
                    res
                } else if (method == "POST" || method == "DELETE")
                    && path == "/api/v1/community/mutes-blocks"
                {
                    let req: MuteBlockRequest = serde_json::from_slice(body_bytes).expect("mb");
                    let cur = st.follow_block_in_flight.fetch_add(1, Ordering::SeqCst) + 1;
                    st.follow_block_max_in_flight.fetch_max(cur, Ordering::SeqCst);
                    let delay = st.block_delay_ms.swap(0, Ordering::SeqCst);
                    if delay > 0 {
                        tokio::time::sleep(Duration::from_millis(delay)).await;
                    }
                    let res = if st.block_fail_500.load(Ordering::SeqCst) {
                        st.block_completed.fetch_add(1, Ordering::SeqCst);
                        ("500 Internal Server Error", r#"{"error":"block_500"}"#.to_string())
                    } else if method == "POST" {
                        st.bob_blocked.store(true, Ordering::SeqCst);
                        if st.bob_following.swap(false, Ordering::SeqCst) {
                            st.bob_followers.fetch_sub(1, Ordering::SeqCst);
                        }
                        st.block_completed.fetch_add(1, Ordering::SeqCst);
                        let dto = CommunityMuteBlockDto {
                            id: "mb_r9".to_string(),
                            owner_actor_id: viewer,
                            target_kind: MuteBlockTargetKind::Actor,
                            target_value: req.target_value,
                            action: req.action,
                            reason: None,
                            created_at_ms: 1_750_000_000_000,
                        };
                        ("200 OK", serde_json::to_string(&dto).unwrap())
                    } else {
                        st.bob_blocked.store(false, Ordering::SeqCst);
                        st.block_completed.fetch_add(1, Ordering::SeqCst);
                        (
                            "200 OK",
                            serde_json::to_string(&DeleteEntityResponse {
                                id: req.target_value,
                                deleted: true,
                            })
                            .unwrap(),
                        )
                    };
                    st.follow_block_in_flight.fetch_sub(1, Ordering::SeqCst);
                    res
                } else if method == "GET" && path == "/api/v1/community/lists" {
                    st.lists_get_received.fetch_add(1, Ordering::SeqCst);
                    let delay = st.lists_get_delay_ms.swap(0, Ordering::SeqCst);
                    if delay > 0 {
                        tokio::time::sleep(Duration::from_millis(delay)).await;
                    }
                    let snap = st.lists.lock().unwrap().clone();
                    st.lists_get_completed.fetch_add(1, Ordering::SeqCst);
                    ("200 OK", serde_json::to_string(&CommunityListsResponse { lists: snap }).unwrap())
                } else if method == "PATCH" && path.starts_with("/api/v1/community/lists/") {
                    let lid = path.trim_start_matches("/api/v1/community/lists/").to_string();
                    let req: UpdateCommunityListRequest = serde_json::from_slice(body_bytes).expect("ul");
                    let is_editor = req.name.is_some();
                    if is_editor {
                        let delay = st.list_editor_delay_ms.swap(0, Ordering::SeqCst);
                        if delay > 0 {
                            tokio::time::sleep(Duration::from_millis(delay)).await;
                        }
                        let ts = st.list_ts_clock.fetch_add(10, Ordering::SeqCst);
                        let mut guard = st.lists.lock().unwrap();
                        let updated = if let Some(item) = guard.iter_mut().find(|l| l.list_id == lid) {
                            if let Some(n) = req.name {
                                item.name = n;
                            }
                            if let Some(p) = req.is_pinned {
                                item.is_pinned = p;
                            }
                            item.updated_at_ms = ts;
                            Some(item.clone())
                        } else {
                            None
                        };
                        drop(guard);
                        st.list_editor_completed.fetch_add(1, Ordering::SeqCst);
                        if let Some(u) = updated {
                            ("200 OK", serde_json::to_string(&u).unwrap())
                        } else {
                            ("404 Not Found", r#"{"error":"not_found"}"#.to_string())
                        }
                    } else {
                        st.list_toggle_pre_received.fetch_add(1, Ordering::SeqCst);
                        let pre = st.list_toggle_pre_delay_ms.swap(0, Ordering::SeqCst);
                        if pre > 0 {
                            tokio::time::sleep(Duration::from_millis(pre)).await;
                        }
                        if st.list_toggle_fail_500.load(Ordering::SeqCst) {
                            st.list_toggle_completed.fetch_add(1, Ordering::SeqCst);
                            ("500 Internal Server Error", r#"{"error":"list_500"}"#.to_string())
                        } else {
                            let ts = st.list_ts_clock.fetch_add(10, Ordering::SeqCst);
                            let updated = {
                                let mut guard = st.lists.lock().unwrap();
                                if let Some(item) = guard.iter_mut().find(|l| l.list_id == lid) {
                                    if let Some(p) = req.is_pinned {
                                        item.is_pinned = p;
                                    }
                                    if let Some(m) = req.member_actor_ids {
                                        item.member_actor_ids = m;
                                    }
                                    if let Some(s) = req.community_slugs {
                                        item.community_slugs = s;
                                    }
                                    item.updated_at_ms = ts;
                                    Some(item.clone())
                                } else {
                                    None
                                }
                            };
                            let post = st.list_toggle_post_delay_ms.swap(0, Ordering::SeqCst);
                            if post > 0 {
                                tokio::time::sleep(Duration::from_millis(post)).await;
                            }
                            st.list_toggle_completed.fetch_add(1, Ordering::SeqCst);
                            if let Some(u) = updated {
                                ("200 OK", serde_json::to_string(&u).unwrap())
                            } else {
                                ("404 Not Found", r#"{"error":"not_found"}"#.to_string())
                            }
                        }
                    }
                } else if method == "GET" && path == "/api/v1/community/feed" {
                    let is_saved = query.contains("tab=saved");
                    let is_comm = query.contains("community=");
                    let is_cursor = query.contains("cursor=");
                    if is_saved {
                        st.saved_get_received.fetch_add(1, Ordering::SeqCst);
                        let mut items = Vec::new();
                        if st.saved_bookmarked.load(Ordering::SeqCst)
                            && !st.deleted_posts.lock().unwrap().contains(&st.p_saved)
                        {
                            items.push(FeedItemDto {
                                post: st.build_post(&st.p_saved, &viewer),
                                explanation: None,
                                rank_score: 1.0,
                                is_exploration_slot: false,
                            });
                        }
                        let resp = FeedResponseDto {
                            items,
                            next_cursor: None,
                            total_candidates: 0,
                            applied_preset: Default::default(),
                            generated_at_ms: 1_750_000_000_000,
                        };
                        ("200 OK", serde_json::to_string(&resp).unwrap())
                    } else if is_comm {
                        if is_cursor {
                            let snap_post = st.build_post(&st.p_cpage2, &viewer);
                            let already_del = st.deleted_posts.lock().unwrap().contains(&st.p_cpage2);
                            let delay = st.comm_more_delay_ms.swap(0, Ordering::SeqCst);
                            if delay > 0 {
                                tokio::time::sleep(Duration::from_millis(delay)).await;
                            }
                            st.comm_more_completed.fetch_add(1, Ordering::SeqCst);
                            let items = if already_del {
                                Vec::new()
                            } else {
                                vec![FeedItemDto {
                                    post: snap_post,
                                    explanation: None,
                                    rank_score: 0.5,
                                    is_exploration_slot: false,
                                }]
                            };
                            let resp = FeedResponseDto {
                                items,
                                next_cursor: None,
                                total_candidates: 0,
                                applied_preset: Default::default(),
                                generated_at_ms: 1_750_000_000_000,
                            };
                            ("200 OK", serde_json::to_string(&resp).unwrap())
                        } else {
                            let delay = st.comm_feed_delay_ms.swap(0, Ordering::SeqCst);
                            if delay > 0 {
                                tokio::time::sleep(Duration::from_millis(delay)).await;
                            }
                            let deleted = st.deleted_posts.lock().unwrap().clone();
                            let mut items = Vec::new();
                            if !deleted.contains(&st.p_main) {
                                items.push(FeedItemDto {
                                    post: st.build_post(&st.p_main, &viewer),
                                    explanation: None,
                                    rank_score: 1.0,
                                    is_exploration_slot: false,
                                });
                            }
                            let resp = FeedResponseDto {
                                items,
                                next_cursor: Some("cursor_cp2".to_string()),
                                total_candidates: 0,
                                applied_preset: Default::default(),
                                generated_at_ms: 1_750_000_000_000,
                            };
                            ("200 OK", serde_json::to_string(&resp).unwrap())
                        }
                    } else if is_cursor {
                        let snap_post = st.build_post(&st.p_page2, &viewer);
                        let already_del = st.deleted_posts.lock().unwrap().contains(&st.p_page2);
                        let delay = st.feed_more_delay_ms.swap(0, Ordering::SeqCst);
                        if delay > 0 {
                            tokio::time::sleep(Duration::from_millis(delay)).await;
                        }
                        st.feed_get_completed.fetch_add(1, Ordering::SeqCst);
                        let items = if already_del {
                            Vec::new()
                        } else {
                            vec![FeedItemDto {
                                post: snap_post,
                                explanation: None,
                                rank_score: 0.5,
                                is_exploration_slot: false,
                            }]
                        };
                        let resp = FeedResponseDto {
                            items,
                            next_cursor: None,
                            total_candidates: 0,
                            applied_preset: Default::default(),
                            generated_at_ms: 1_750_000_000_000,
                        };
                        ("200 OK", serde_json::to_string(&resp).unwrap())
                    } else {
                        st.feed_get_received.fetch_add(1, Ordering::SeqCst);
                        let deleted = st.deleted_posts.lock().unwrap().clone();
                        let mut items = Vec::new();
                        for pid in [&st.p_main, &st.p_saved] {
                            if !deleted.contains(pid) {
                                items.push(FeedItemDto {
                                    post: st.build_post(pid, &viewer),
                                    explanation: None,
                                    rank_score: 1.0,
                                    is_exploration_slot: false,
                                });
                            }
                        }
                        let resp = FeedResponseDto {
                            items,
                            next_cursor: Some("cursor_p2".to_string()),
                            total_candidates: 0,
                            applied_preset: Default::default(),
                            generated_at_ms: 1_750_000_000_000,
                        };
                        let delay = st.feed_get_delay_ms.swap(0, Ordering::SeqCst);
                        if delay > 0 {
                            tokio::time::sleep(Duration::from_millis(delay)).await;
                        }
                        st.feed_get_completed.fetch_add(1, Ordering::SeqCst);
                        ("200 OK", serde_json::to_string(&resp).unwrap())
                    }
                } else if method == "GET" && path.starts_with("/api/v1/community/posts/") {
                    let pid = path.trim_start_matches("/api/v1/community/posts/").to_string();
                    let resp = PostDetailResponse {
                        post: st.build_post(&pid, &viewer),
                        thread_chain: Vec::new(),
                        quote_chain: Vec::new(),
                        comments: Vec::new(),
                        total_comments: 0,
                    };
                    let delay = st.detail_get_delay_ms.swap(0, Ordering::SeqCst);
                    if delay > 0 {
                        tokio::time::sleep(Duration::from_millis(delay)).await;
                    }
                    st.detail_get_completed.fetch_add(1, Ordering::SeqCst);
                    ("200 OK", serde_json::to_string(&resp).unwrap())
                } else if method == "POST" && path == "/api/v1/community/posts" {
                    let req: CreatePostRequest = serde_json::from_slice(body_bytes).expect("cp");
                    *st.last_create_post_actor.lock().unwrap() = acting_actor.clone();
                    st.create_post_received.fetch_add(1, Ordering::SeqCst);
                    let delay = st.create_post_delay_ms.swap(0, Ordering::SeqCst);
                    if delay > 0 {
                        tokio::time::sleep(Duration::from_millis(delay)).await;
                    }
                    let idx = st.create_post_completed.fetch_add(1, Ordering::SeqCst) + 1;
                    let mut created = mock_post_dto(
                        &format!("p_r9_created_{idx}"),
                        &viewer,
                        false,
                        None,
                        0,
                    );
                    created.body = req.body;
                    created.community_slug = req.community_slug;
                    let resp = CreatePostResponse {
                        post: created,
                        thread_posts: Vec::new(),
                        automod_action: AutoModActionKind::Approve,
                        automod_note: None,
                    };
                    ("200 OK", serde_json::to_string(&resp).unwrap())
                } else if method == "POST" && path.ends_with("/vote") && path.contains("/posts/") {
                    let pid = path
                        .trim_start_matches("/api/v1/community/posts/")
                        .trim_end_matches("/vote")
                        .to_string();
                    let req: VoteRequest = serde_json::from_slice(body_bytes).expect("vr");
                    *st.last_vote_post_actor.lock().unwrap() = acting_actor.clone();
                    st.last_vote_dir.store(req.direction, Ordering::SeqCst);
                    let delay = st.post_vote_delay_ms.swap(0, Ordering::SeqCst);
                    if delay > 0 {
                        tokio::time::sleep(Duration::from_millis(delay)).await;
                    }
                    {
                        let mut v = st.votes.lock().unwrap();
                        if req.direction == 0 {
                            v.remove(&(viewer.clone(), pid.clone()));
                        } else {
                            v.insert((viewer.clone(), pid.clone()), req.direction);
                        }
                    }
                    let p = st.build_post(&pid, &viewer);
                    st.post_vote_completed.fetch_add(1, Ordering::SeqCst);
                    let resp = VoteResponse {
                        target_id: pid,
                        direction: p.viewer_vote,
                        upvotes: p.upvotes,
                        downvotes: p.downvotes,
                        score: p.score,
                    };
                    ("200 OK", serde_json::to_string(&resp).unwrap())
                } else if method == "POST" && path.ends_with("/like") && path.contains("/posts/") {
                    let pid = path
                        .trim_start_matches("/api/v1/community/posts/")
                        .trim_end_matches("/like")
                        .to_string();
                    let active = {
                        let mut l = st.likes.lock().unwrap();
                        let k = (viewer.clone(), pid.clone());
                        if l.contains(&k) {
                            l.remove(&k);
                            false
                        } else {
                            l.insert(k);
                            true
                        }
                    };
                    let p = st.build_post(&pid, &viewer);
                    st.post_like_completed.fetch_add(1, Ordering::SeqCst);
                    let resp = ToggleActionResponse {
                        target_id: pid,
                        active,
                        count: p.likes_count,
                    };
                    ("200 OK", serde_json::to_string(&resp).unwrap())
                } else if method == "POST" && path.ends_with("/repost") && path.contains("/posts/") {
                    let pid = path
                        .trim_start_matches("/api/v1/community/posts/")
                        .trim_end_matches("/repost")
                        .to_string();
                    let reposted = {
                        let mut r = st.reposts.lock().unwrap();
                        let k = (viewer.clone(), pid.clone());
                        if r.contains(&k) {
                            r.remove(&k);
                            false
                        } else {
                            r.insert(k);
                            true
                        }
                    };
                    let p = st.build_post(&pid, &viewer);
                    st.post_repost_completed.fetch_add(1, Ordering::SeqCst);
                    let resp = RepostResponse {
                        post_id: pid,
                        reposted,
                        reposts_count: p.reposts_count,
                        repost_post_id: None,
                    };
                    ("200 OK", serde_json::to_string(&resp).unwrap())
                } else if method == "POST" && path.ends_with("/bookmark") && path.contains("/posts/") {
                    let pid = path
                        .trim_start_matches("/api/v1/community/posts/")
                        .trim_end_matches("/bookmark")
                        .to_string();
                    let req: BookmarkRequest = serde_json::from_slice(body_bytes).expect("bk");
                    let bookmarked = !req.remove.unwrap_or(false);
                    if pid == st.p_saved {
                        st.saved_bookmarked.store(bookmarked, Ordering::SeqCst);
                    }
                    st.post_bookmark_completed.fetch_add(1, Ordering::SeqCst);
                    let resp = BookmarkResponse {
                        post_id: pid,
                        bookmarked,
                        folder: req.folder.unwrap_or_else(|| "All".to_string()),
                        bookmarks_count: u64::from(bookmarked),
                    };
                    ("200 OK", serde_json::to_string(&resp).unwrap())
                } else if method == "DELETE" && path.starts_with("/api/v1/community/posts/") {
                    let pid = path.trim_start_matches("/api/v1/community/posts/").to_string();
                    let delay = st.delete_post_delay_ms.swap(0, Ordering::SeqCst);
                    if delay > 0 {
                        tokio::time::sleep(Duration::from_millis(delay)).await;
                    }
                    st.deleted_posts.lock().unwrap().insert(pid.clone());
                    st.delete_post_completed.fetch_add(1, Ordering::SeqCst);
                    (
                        "200 OK",
                        serde_json::to_string(&DeleteEntityResponse {
                            id: pid,
                            deleted: true,
                        })
                        .unwrap(),
                    )
                } else {
                    ("200 OK", "{}".to_string())
                };

                let http = format!(
                    "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{resp_body}",
                    resp_body.len()
                );
                let _ = sock.write_all(http.as_bytes()).await;
            });
        }
    });

    (addr, state, AbortOnDrop(join))
}

#[test]
fn test_m5_r9_cross_lane_and_facade_isolation() {
    run_in_headless_subprocess("test_m5_r9_cross_lane_and_facade_isolation");
}

async fn body_r9_cross_lane_and_facade_isolation(mut app: AsyncApp) {
    let dir = TempDir::new().expect("r9 tempdir");
    let rt_dir = TempDir::new().expect("r9 rt tempdir");
    let rt_holder = TelegramClient::open_with_dir(
        ClientConfig {
            server_url: "http://127.0.0.1:9".to_string(),
            max_reconnect_attempts: 0,
            ..Default::default()
        },
        rt_dir.path(),
    )
    .expect("open r9 rt holder");
    let (addr, srv, _server_guard) = spawn_r9_server(rt_holder.runtime().handle());

    let client = Arc::new(
        TelegramClient::open_with_dir(
            ClientConfig {
                server_url: format!("http://{addr}"),
                max_reconnect_attempts: 0,
                ..Default::default()
            },
            dir.path(),
        )
        .expect("open r9 client"),
    );
    let auth = test_auth(909001, "r9_alice");
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
        .expect("mount r9 AppShell");
    let _ = shell.update(&mut app, |s, cx| s.open_community_workspace(cx));
    let cw = shell
        .read_with(&app, |s, _| s.community_workspace.clone().unwrap())
        .unwrap();

    assert!(
        pump_until(&mut app, Duration::from_secs(5), |cx| {
            let st = &cw.read(cx).state;
            !st.feed_loading
                && !st.communities_loading
                && st.identities.is_some()
                && !st.communities.is_empty()
                && st.feed_items.iter().any(|it| it.post.post_id == srv.p_main)
        })
        .await,
        "R9 workspace must bootstrap cleanly"
    );

    run_r9_t01_to_t03(&mut app, &cw, &srv).await;
    run_r9_t04_to_t06(&mut app, &cw, &srv).await;
    run_r9_t07_to_t09(&mut app, &cw, &srv).await;

    drop(rt_holder);
}

async fn run_r9_t01_to_t03(
    app: &mut AsyncApp,
    cw: &Entity<CommunityWorkspace>,
    srv: &Arc<R9MockState>,
) {
    use std::sync::atomic::Ordering;

    // ------------------------------------------------------------------
    // T9-01 (BUG-HUNT-5): Bot Studio test post uses create_post_as without mutating
    // shared facade.active_actor_id; concurrent vote_post sends Human u:909001 header!
    // ------------------------------------------------------------------
    let cp_rx = srv.create_post_received.load(Ordering::SeqCst);
    let cp_before = srv.create_post_completed.load(Ordering::SeqCst);
    let pv_before = srv.post_vote_completed.load(Ordering::SeqCst);
    srv.create_post_delay_ms.store(280, Ordering::SeqCst);
    let _ = cw.update(app, |w, cx| {
        assert_eq!(w.facade.active_actor_id().as_deref(), Some(srv.alice_id.as_str()));
        w.state.bot_studio_test_content = "Bot Studio isolated post".to_string();
        w.run_bot_studio_test_post(srv.bot_a_id.clone(), cx);
    });
    assert!(
        pump_until(app, Duration::from_secs(4), |_| {
            srv.create_post_received.load(Ordering::SeqCst) > cp_rx
        })
        .await,
        "T9-01: create_post_as request must reach mock server before concurrent vote_post"
    );
    let _ = cw.update(app, |w, cx| {
        assert!(w.state.bot_studio_test_submitting, "T9-01: create_post must still be in flight");
        assert_eq!(
            w.facade.active_actor_id().as_deref(),
            Some(srv.alice_id.as_str()),
            "T9-01: facade.active_actor_id must NEVER mutate during run_bot_studio_test_post"
        );
        w.vote_post(srv.p_saved.clone(), 1, cx);
    });
    assert!(
        pump_until(app, Duration::from_secs(5), |cx| {
            let st = &cw.read(cx).state;
            !st.bot_studio_test_submitting
                && st.post_vote_in_flight.is_empty()
                && srv.create_post_completed.load(Ordering::SeqCst) > cp_before
                && srv.post_vote_completed.load(Ordering::SeqCst) > pv_before
        })
        .await,
        "T9-01: both create_post_as and concurrent vote_post must settle"
    );
    assert_eq!(
        srv.last_create_post_actor.lock().unwrap().as_deref(),
        Some(srv.bot_a_id.as_str()),
        "T9-01: create_post must carry bot actor header"
    );
    assert_eq!(
        srv.last_vote_post_actor.lock().unwrap().as_deref(),
        Some(srv.alice_id.as_str()),
        "T9-01: concurrent vote_post must carry Human actor header, NOT bot header!"
    );
    // Validate empty / space-only / CRLF / Unicode whitespace / obs-text rejection on create_post_as
    let facade_clone = cw.read_with(app, |w, _| w.facade.clone()).unwrap();
    let bad_req = CreatePostRequest {
        body: "validation check".to_string(),
        ..Default::default()
    };
    let cp_rx_before_bad = srv.create_post_received.load(Ordering::SeqCst);
    for bad in [
        "",
        "   ",
        "\r\n",
        "b:909002\r\nX-Injected: 1",
        "\u{00A0}",
        "\u{3000}",
        "\u{200B}",
        "\u{FEFF}",
        "b:909002🦀",
        "b:909002 evil",
    ] {
        let res = facade_clone.block_on(facade_clone.create_post_as(bad, &bad_req));
        assert!(
            matches!(res, Err(gpui_telegram_core::error::TelegramError::Validation(_))),
            "T9-01: create_post_as({bad:?}) must return Validation error, got {res:?}"
        );
    }
    assert_eq!(
        srv.create_post_received.load(Ordering::SeqCst),
        cp_rx_before_bad,
        "T9-01: invalid create_post_as payloads must send 0 HTTP requests"
    );
    assert_eq!(
        facade_clone.active_actor_id().as_deref(),
        Some(srv.alice_id.as_str())
    );
    // Reset p_saved vote cleanly for T9-04
    srv.votes.lock().unwrap().remove(&(srv.alice_id.clone(), srv.p_saved.clone()));

    // ------------------------------------------------------------------
    // T9-02 (BUG-HUNT-1a / ADV-5 + BUG-HUNT-1b): All 5 Follow -> Block interleavings
    // serialized (max_concurrent_follow_block == 1) + exact 3-tuple convergence + local SQLite guard
    // ------------------------------------------------------------------
    let _ = cw.update(app, |w, cx| {
        w.navigate_to(CommunityRoute::ActorProfile("r9_bob".to_string()), cx);
    });
    assert!(
        pump_until(app, Duration::from_secs(4), |cx| {
            !cw.read(cx).state.profile_loading && cw.read(cx).state.actor_profile.is_some()
        })
        .await
    );

    let cases: [(&str, bool, bool, bool, (bool, bool, u64)); 5] = [
        ("Follow200->Block200", false, false, false, (true, false, 0)),
        ("Follow500->Block500", true, true, false, (false, false, 0)),
        ("Follow200->Block500", false, true, false, (false, true, 1)),
        ("Follow500->Block200", true, false, false, (true, false, 0)),
        ("Follow200->Block->Unblock", false, false, true, (false, true, 1)),
    ];
    for (label, fl_fail, blk_fail, do_unblock, expected) in cases {
        srv.bob_blocked.store(false, Ordering::SeqCst);
        srv.bob_following.store(false, Ordering::SeqCst);
        srv.bob_followers.store(0, Ordering::SeqCst);
        srv.follow_fail_500.store(false, Ordering::SeqCst);
        srv.block_fail_500.store(false, Ordering::SeqCst);
        srv.follow_block_max_in_flight.store(0, Ordering::SeqCst);
        let pg_b = srv.profile_get_completed.load(Ordering::SeqCst);
        let _ = cw.update(app, |w, cx| {
            w.load_actor_profile("r9_bob".to_string(), cx);
        });
        assert!(
            pump_until(app, Duration::from_secs(4), |cx| {
                !cw.read(cx).state.profile_loading
                    && srv.profile_get_completed.load(Ordering::SeqCst) > pg_b
            })
            .await
        );
        let fl_b = srv.follow_completed.load(Ordering::SeqCst);
        let blk_b = srv.block_completed.load(Ordering::SeqCst);
        srv.follow_delay_ms.store(150, Ordering::SeqCst);
        srv.block_delay_ms.store(15, Ordering::SeqCst);
        srv.follow_fail_500.store(fl_fail, Ordering::SeqCst);
        srv.block_fail_500.store(blk_fail, Ordering::SeqCst);
        let _ = cw.update(app, |w, cx| {
            w.toggle_follow_actor(srv.bob_id.clone(), true, cx);
            w.toggle_block_actor(srv.bob_id.clone(), false, cx);
            if do_unblock {
                w.toggle_block_actor(srv.bob_id.clone(), true, cx);
            }
        });
        assert!(
            pump_until(app, Duration::from_secs(5), |cx| {
                let st = &cw.read(cx).state;
                st.actor_follow_in_flight.is_empty()
                    && st.actor_block_in_flight.is_empty()
                    && st.actor_block_dirty.is_empty()
                    && srv.follow_completed.load(Ordering::SeqCst) > fl_b
                    && srv.block_completed.load(Ordering::SeqCst) > blk_b
            })
            .await,
            "T9-02 ({label}): Follow->Block chain must settle"
        );
        assert_eq!(
            srv.follow_block_max_in_flight.load(Ordering::SeqCst),
            1,
            "T9-02 ({label}): Follow and Block MUST be serialized (max_in_flight == 1)"
        );
        let srv_tuple = (
            srv.bob_blocked.load(Ordering::SeqCst),
            srv.bob_following.load(Ordering::SeqCst),
            srv.bob_followers.load(Ordering::SeqCst),
        );
        let ui_tuple = cw
            .read_with(app, |w, _| {
                let a = &w.state.actor_profile.as_ref().unwrap().actor;
                (a.is_blocked, a.is_following, a.followers_count)
            })
            .unwrap();
        assert_eq!(srv_tuple, expected, "T9-02 ({label}): server tuple mismatch");
        assert_eq!(ui_tuple, expected, "T9-02 ({label}): UI != Server split-brain!");
    }
    srv.follow_fail_500.store(false, Ordering::SeqCst);
    srv.block_fail_500.store(false, Ordering::SeqCst);
    srv.bob_blocked.store(false, Ordering::SeqCst);
    srv.bob_following.store(false, Ordering::SeqCst);
    srv.bob_followers.store(0, Ordering::SeqCst);

    // Direct local SQLite store verification for BUG-HUNT-1b bidirectional Block guard
    let local_rt = facade_clone.local_runtime().clone();
    let local_actors = local_rt
        .search(
            None,
            &gpui_telegram_proto::community_contracts::CommunitySearchRequestDto {
                tab: CommunitySearchTab::People,
                limit: 10,
                ..Default::default()
            },
        )
        .expect("local actors");
    let other = local_actors
        .actors
        .iter()
        .find(|a| a.actor_id != srv.alice_id)
        .expect("T9-02: local runtime must contain a non-alice actor");
    let _ = local_rt
        .set_mute_block(
            Some(&srv.alice_id),
            &MuteBlockRequest {
                target_kind: MuteBlockTargetKind::Actor,
                target_value: other.actor_id.clone(),
                action: MuteBlockActionKind::Block,
                reason: None,
            },
        )
        .expect("local block");
    let err = local_rt
        .follow_actor(Some(&srv.alice_id), &other.actor_id)
        .expect_err("BUG-HUNT-1b: local SQLite follow_actor must reject blocked target");
    assert!(matches!(
        err,
        gpui_telegram_core::error::TelegramError::PermissionDenied(_)
    ));

    // ------------------------------------------------------------------
    // T9-03 (ADV-1): Post-commit delayed Flight 1 (ts_f1 <= ts_editor) AND
    // Pre-commit delayed Flight 1 (ts_f1 > ts_editor) -> Flight 2 HTTP 500
    // ------------------------------------------------------------------
    let _ = cw.update(app, |w, cx| {
        w.navigate_to(CommunityRoute::BookmarksLists, cx);
        w.set_bookmarks_section(BookmarksListsSection::Lists, cx);
    });
    assert!(
        pump_until(app, Duration::from_secs(4), |cx| {
            let st = &cw.read(cx).state;
            !st.saved_loading && st.lists.iter().any(|l| l.list_id == "l_adv1_post")
        })
        .await
    );

    // Subcase A: Post-commit delayed Flight 1 (commits at ts=3000, response delayed 360ms),
    // Editor commits ("Renamed ADV1", false) at ts=3010 + GET /lists confirms false -> Flight 2 fails 500!
    let lt_b = srv.list_toggle_completed.load(Ordering::SeqCst);
    let le_b = srv.list_editor_completed.load(Ordering::SeqCst);
    srv.list_toggle_post_delay_ms.store(360, Ordering::SeqCst);
    srv.list_editor_delay_ms.store(15, Ordering::SeqCst);
    srv.lists_get_delay_ms.store(15, Ordering::SeqCst);
    let _ = cw.update(app, |w, cx| {
        let target = w
            .state
            .lists
            .iter()
            .find(|l| l.list_id == "l_adv1_post")
            .cloned()
            .unwrap();
        w.open_edit_list_form(target, cx);
        w.state.list_editor.name = "Renamed ADV1".to_string();
        w.state.list_editor.is_pinned = false;
        w.toggle_list_pinned("l_adv1_post".to_string(), true, cx);
    });
    assert!(
        pump_until(app, Duration::from_secs(4), |_| {
            srv.lists
                .lock()
                .unwrap()
                .iter()
                .find(|l| l.list_id == "l_adv1_post")
                .is_some_and(|l| l.is_pinned)
        })
        .await,
        "T9-03A: Flight 1 must commit is_pinned=true and enter post-commit delay before Editor submits"
    );
    // Arm Flight 2 to fail HTTP 500 IMMEDIATELY (Flight 1 already passed the 500 check and is in post-sleep)!
    srv.list_toggle_fail_500.store(true, Ordering::SeqCst);
    let _ = cw.update(app, |w, cx| {
        w.submit_list_editor(cx);
    });
    assert!(
        pump_until(app, Duration::from_secs(4), |cx| {
            !cw.read(cx).state.list_editor.submitting
                && srv.list_editor_completed.load(Ordering::SeqCst) > le_b
        })
        .await,
        "T9-03A: Editor must finish BEFORE post-commit delayed Flight 1 returns"
    );
    assert!(
        pump_until(app, Duration::from_secs(5), |cx| {
            let st = &cw.read(cx).state;
            st.list_membership_in_flight.is_empty()
                && st.list_membership_dirty.is_empty()
                && srv.list_toggle_completed.load(Ordering::SeqCst) >= lt_b + 2
        })
        .await,
        "T9-03A: Flight 1 + Flight 2 (500) must settle"
    );
    srv.list_toggle_fail_500.store(false, Ordering::SeqCst);
    let _ = cw.read_with(app, |w, _| {
        let ui = w.state.lists.iter().find(|l| l.list_id == "l_adv1_post").unwrap();
        let sv = srv
            .lists
            .lock()
            .unwrap()
            .iter()
            .find(|l| l.list_id == "l_adv1_post")
            .cloned()
            .unwrap();
        assert_eq!((ui.name.as_str(), ui.is_pinned), ("Renamed ADV1", false));
        assert_eq!((sv.name.as_str(), sv.is_pinned), ("Renamed ADV1", false));
    });

    // Subcase B: Pre-commit delayed Flight 1 (sleeps 260ms BEFORE hitting server, so ts_f1 > ts_editor!).
    // Editor commits ("Renamed Pre", false) at ts=3020; Flight 1 commits (true) at ts=3030 > 3020;
    // Flight 2 fails 500 -> MUST roll back to Flight 1's newer server truth ("Renamed Pre", true)!
    let lt_pre_b2 = srv.list_toggle_pre_received.load(Ordering::SeqCst);
    let lt_b2 = srv.list_toggle_completed.load(Ordering::SeqCst);
    let le_b2 = srv.list_editor_completed.load(Ordering::SeqCst);
    srv.list_toggle_pre_delay_ms.store(260, Ordering::SeqCst);
    srv.list_toggle_post_delay_ms.store(260, Ordering::SeqCst);
    srv.list_editor_delay_ms.store(15, Ordering::SeqCst);
    srv.lists_get_delay_ms.store(15, Ordering::SeqCst);
    let _ = cw.update(app, |w, cx| {
        let target = w
            .state
            .lists
            .iter()
            .find(|l| l.list_id == "l_adv1_pre")
            .cloned()
            .unwrap();
        w.open_edit_list_form(target, cx);
        w.state.list_editor.name = "Renamed Pre".to_string();
        w.state.list_editor.is_pinned = false;
        w.toggle_list_pinned("l_adv1_pre".to_string(), true, cx);
    });
    assert!(
        pump_until(app, Duration::from_secs(4), |_| {
            srv.list_toggle_pre_received.load(Ordering::SeqCst) > lt_pre_b2
        })
        .await,
        "T9-03B: Flight 1 must enter pre-commit sleep before Editor submits"
    );
    let _ = cw.update(app, |w, cx| {
        w.submit_list_editor(cx);
    });
    assert!(
        pump_until(app, Duration::from_secs(4), |cx| {
            !cw.read(cx).state.list_editor.submitting
                && srv.list_editor_completed.load(Ordering::SeqCst) > le_b2
        })
        .await,
        "T9-03B: Editor must finish while Flight 1 is still in pre-commit delay"
    );
    assert_eq!(
        srv.list_toggle_completed.load(Ordering::SeqCst),
        lt_b2,
        "T9-03B: Flight 1 must NOT have completed before Editor settled"
    );
    // Wait until Flight 1 has committed is_pinned=true on server strictly AFTER Editor
    // (ts_f1 > ts_editor) while Flight 1 is still in its post-commit wire delay.
    assert!(
        pump_until(app, Duration::from_secs(4), |_| {
            srv.lists
                .lock()
                .unwrap()
                .iter()
                .find(|l| l.list_id == "l_adv1_pre")
                .is_some_and(|l| l.is_pinned)
        })
        .await,
        "T9-03B: Flight 1 must commit is_pinned=true strictly AFTER Editor"
    );
    // Arm HTTP 500 for Flight 2 before Flight 1's response arrives.
    srv.list_toggle_fail_500.store(true, Ordering::SeqCst);
    assert!(
        pump_until(app, Duration::from_secs(5), |cx| {
            let st = &cw.read(cx).state;
            st.list_membership_in_flight.is_empty()
                && st.list_membership_dirty.is_empty()
                && srv.list_toggle_completed.load(Ordering::SeqCst) >= lt_b2 + 2
        })
        .await,
        "T9-03B: pre-commit Flight 1 + Flight 2 (500) must settle"
    );
    srv.list_toggle_fail_500.store(false, Ordering::SeqCst);
    let _ = cw.read_with(app, |w, _| {
        let ui = w.state.lists.iter().find(|l| l.list_id == "l_adv1_pre").unwrap();
        let sv = srv
            .lists
            .lock()
            .unwrap()
            .iter()
            .find(|l| l.list_id == "l_adv1_pre")
            .cloned()
            .unwrap();
        assert_eq!((ui.name.as_str(), ui.is_pinned), ("Renamed Pre", true));
        assert_eq!((sv.name.as_str(), sv.is_pinned), ("Renamed Pre", true));
    });
}

async fn run_r9_t04_to_t06(
    app: &mut AsyncApp,
    cw: &Entity<CommunityWorkspace>,
    srv: &Arc<R9MockState>,
) {
    use std::sync::atomic::Ordering;

    // ------------------------------------------------------------------
    // T9-04 (ADV-2 / BUG-HUNT-2): Saved tab slow load_saved_posts vs fast
    // vote_post + toggle_like + repost_post + verify bookmark Order 2 (saved_get_calls == 1)
    // ------------------------------------------------------------------
    let _ = cw.update(app, |w, cx| {
        w.set_bookmarks_section(BookmarksListsSection::Bookmarks, cx);
    });
    let sav_rx = srv.saved_get_received.load(Ordering::SeqCst);
    let lg_rx = srv.lists_get_received.load(Ordering::SeqCst);
    let pv_b = srv.post_vote_completed.load(Ordering::SeqCst);
    let pl_b = srv.post_like_completed.load(Ordering::SeqCst);
    let pr_b = srv.post_repost_completed.load(Ordering::SeqCst);
    srv.lists_get_delay_ms.store(240, Ordering::SeqCst);
    let _ = cw.update(app, |w, cx| {
        w.load_saved_posts(cx);
    });
    assert!(
        pump_until(app, Duration::from_secs(4), |_| {
            srv.lists_get_received.load(Ordering::SeqCst) > lg_rx
        })
        .await,
        "T9-04: load_saved_posts must have fetched saved feed and be awaiting slow GET /lists"
    );
    let _ = cw.update(app, |w, cx| {
        assert!(w.state.saved_loading);
        w.vote_post(srv.p_saved.clone(), 1, cx);
        w.toggle_like(srv.p_saved.clone(), cx);
        w.repost_post(srv.p_saved.clone(), cx);
    });
    assert!(
        pump_until(app, Duration::from_secs(5), |cx| {
            let st = &cw.read(cx).state;
            !st.saved_loading
                && st.post_vote_in_flight.is_empty()
                && st.post_like_in_flight.is_empty()
                && st.post_repost_in_flight.is_empty()
                && srv.post_vote_completed.load(Ordering::SeqCst) > pv_b
                && srv.post_like_completed.load(Ordering::SeqCst) > pl_b
                && srv.post_repost_completed.load(Ordering::SeqCst) > pr_b
        })
        .await,
        "T9-04: vote + like + repost + Case B saved reload must settle"
    );
    assert!(
        srv.saved_get_received.load(Ordering::SeqCst) >= sav_rx + 2,
        "T9-04: non-bookmark mutations must trigger saved reload when saved_loading == true"
    );
    let _ = cw.read_with(app, |w, _| {
        let p = &w
            .state
            .saved_items
            .iter()
            .find(|it| it.post.post_id == srv.p_saved)
            .expect("p_saved present")
            .post;
        assert_eq!(
            (p.viewer_vote, p.viewer_liked, p.viewer_reposted),
            (1, true, true),
            "T9-04: saved_items must converge to (1, true, true)"
        );
    });

    // Verify F2/F3 Order 2 invariant (allow_saved_reload = false in flush_bookmark_sync -> saved_get_calls == +1)
    let sav_rx_bk = srv.saved_get_received.load(Ordering::SeqCst);
    let lg_rx_bk = srv.lists_get_received.load(Ordering::SeqCst);
    let bk_done = srv.post_bookmark_completed.load(Ordering::SeqCst);
    srv.lists_get_delay_ms.store(220, Ordering::SeqCst);
    let _ = cw.update(app, |w, cx| {
        w.load_saved_posts(cx);
    });
    assert!(
        pump_until(app, Duration::from_secs(4), |_| {
            srv.lists_get_received.load(Ordering::SeqCst) > lg_rx_bk
        })
        .await
    );
    let _ = cw.update(app, |w, cx| {
        w.toggle_bookmark(srv.p_saved.clone(), cx);
    });
    assert!(
        pump_until(app, Duration::from_secs(5), |cx| {
            let st = &cw.read(cx).state;
            !st.saved_loading
                && st.bookmark_in_flight.is_empty()
                && srv.post_bookmark_completed.load(Ordering::SeqCst) > bk_done
        })
        .await
    );
    assert_eq!(
        srv.saved_get_received.load(Ordering::SeqCst),
        sav_rx_bk + 1,
        "T9-04 F2/F3: flush_bookmark_sync must NOT restart load_saved_posts (saved_get_calls == 1)"
    );
    let _ = cw.read_with(app, |w, _| {
        assert!(
            !w.state.saved_items.iter().any(|it| it.post.post_id == srv.p_saved),
            "T9-04 F2/F3: unbookmarked post must remain removed via bookmark_recent_overrides"
        );
    });

    // ------------------------------------------------------------------
    // T9-05 (BUG-HUNT-4a & BUG-HUNT-4b): Active-route find_post precedence
    // + unconditional cross-cache feed reload on actor switch from PostDetail
    // ------------------------------------------------------------------
    let _ = cw.update(app, |w, cx| {
        w.navigate_to(CommunityRoute::Feed, cx);
        w.load_feed(cx);
    });
    assert!(
        pump_until(app, Duration::from_secs(4), |cx| {
            let st = &cw.read(cx).state;
            !st.feed_loading
                && st
                    .feed_items
                    .iter()
                    .find(|it| it.post.post_id == srv.p_main)
                    .is_some_and(|it| it.post.viewer_vote == 1)
        })
        .await,
        "T9-05: Alice's feed must have viewer_vote=+1 on p_main"
    );
    let _ = cw.update(app, |w, cx| {
        w.navigate_to(CommunityRoute::PostDetail(srv.p_main.clone()), cx);
    });
    assert!(
        pump_until(app, Duration::from_secs(4), |cx| {
            !cw.read(cx).state.detail_loading && cw.read(cx).state.detail_post.is_some()
        })
        .await
    );
    let feed_rx_sw = srv.feed_get_received.load(Ordering::SeqCst);
    let det_done_sw = srv.detail_get_completed.load(Ordering::SeqCst);
    srv.feed_get_delay_ms.store(280, Ordering::SeqCst);
    srv.detail_get_delay_ms.store(10, Ordering::SeqCst);
    let _ = cw.update(app, |w, cx| {
        w.switch_active_actor(srv.bot_a_id.clone(), cx);
    });
    // Wait until PostDetail settled for bot_a (viewer_vote=0) WHILE slow load_feed is STILL in flight!
    assert!(
        pump_until(app, Duration::from_secs(4), |cx| {
            let st = &cw.read(cx).state;
            !st.detail_loading
                && srv.detail_get_completed.load(Ordering::SeqCst) > det_done_sw
                && st.detail_post.as_ref().is_some_and(|d| d.post.viewer_vote == 0)
        })
        .await,
        "T9-05: bot_a PostDetail must settle before slow background load_feed"
    );
    let pv_b5 = srv.post_vote_completed.load(Ordering::SeqCst);
    let _ = cw.update(app, |w, cx| {
        assert!(
            w.state.feed_loading,
            "T9-05 setup: background load_feed must still be in flight"
        );
        // Wave r11: switch_active_actor synchronously scrubs feed_items viewer_vote to 0.
        let feed_post = &mut w
            .state
            .feed_items
            .iter_mut()
            .find(|it| it.post.post_id == srv.p_main)
            .unwrap()
            .post;
        assert_eq!(
            feed_post.viewer_vote, 0,
            "T9-05 (R11): switch_active_actor must synchronously scrub stale feed_items viewer_vote"
        );
        // Re-inject stale viewer_vote=+1 on off-route feed_items to still prove find_post
        // prioritizes active PostDetail over off-route feed_items.
        feed_post.viewer_vote = 1;
        w.vote_post(srv.p_main.clone(), 1, cx);
    });
    assert!(
        pump_until(app, Duration::from_secs(5), |cx| {
            let st = &cw.read(cx).state;
            st.post_vote_in_flight.is_empty()
                && !st.feed_loading
                && srv.post_vote_completed.load(Ordering::SeqCst) > pv_b5
                && srv.feed_get_received.load(Ordering::SeqCst) >= feed_rx_sw + 2
        })
        .await,
        "T9-05: bot_a upvote must commit AND reload background feed_loading"
    );
    assert_eq!(
        srv.last_vote_dir.load(Ordering::SeqCst),
        1,
        "T9-05 (BUG-HUNT-4a): find_post MUST prioritize active PostDetail over stale feed_items so direction=+1 is sent"
    );
    let _ = cw.update(app, |w, cx| {
        w.go_back(cx);
    });
    let _ = cw.read_with(app, |w, _| {
        let p = &w
            .state
            .feed_items
            .iter()
            .find(|it| it.post.post_id == srv.p_main)
            .unwrap()
            .post;
        assert_eq!(
            (p.viewer_vote, p.score),
            (1, 2),
            "T9-05 (BUG-HUNT-4b): go_back to Feed must show (1, 2), NOT stale (0, 1)!"
        );
    });
    // Switch back to Alice for T9-06..T9-09
    let _ = cw.update(app, |w, cx| {
        w.switch_active_actor(srv.alice_id.clone(), cx);
    });
    assert!(
        pump_until(app, Duration::from_secs(4), |cx| {
            let st = &cw.read(cx).state;
            !st.feed_loading && !st.communities_loading
        })
        .await
    );

    // ------------------------------------------------------------------
    // T9-06 (ADV-3 & ADV-4):
    // - ADV-3: slow refresh_trending_discovery GET /communities vs fast toggle_join_community
    // - ADV-4: slow load_community_page vs fast toggle_follow_actor on moderator
    // ------------------------------------------------------------------
    srv.comm_joined.store(false, Ordering::SeqCst);
    srv.comm_members.store(10, Ordering::SeqCst);
    let cl_rx = srv.communities_list_received.load(Ordering::SeqCst);
    let jn_b = srv.join_completed.load(Ordering::SeqCst);
    srv.communities_list_delay_ms.store(240, Ordering::SeqCst);
    let _ = cw.update(app, |w, cx| {
        w.refresh_trending_discovery(cx);
    });
    assert!(
        pump_until(app, Duration::from_secs(4), |_| {
            srv.communities_list_received.load(Ordering::SeqCst) > cl_rx
        })
        .await
    );
    let _ = cw.update(app, |w, cx| {
        assert!(w.state.communities_loading);
        w.toggle_join_community(srv.slug.clone(), true, cx);
    });
    assert!(
        pump_until(app, Duration::from_secs(5), |cx| {
            let st = &cw.read(cx).state;
            !st.communities_loading
                && st.community_join_in_flight.is_empty()
                && srv.join_completed.load(Ordering::SeqCst) > jn_b
                && srv.communities_list_received.load(Ordering::SeqCst) >= cl_rx + 2
        })
        .await,
        "T9-06 (ADV-3): settled join must reload in-flight communities list"
    );
    let _ = cw.read_with(app, |w, _| {
        let c = w
            .state
            .communities
            .iter()
            .find(|c| c.slug == srv.slug)
            .unwrap();
        assert_eq!(
            (c.is_joined, c.member_count),
            (true, 11),
            "T9-06 (ADV-3): communities['rust'] must converge to (true, 11)"
        );
    });

    // ADV-4: slow load_community_page (fast GET /communities/rust + slow GET /feed?tab=community)
    // vs fast toggle_follow_actor on moderator bob!
    srv.bob_blocked.store(false, Ordering::SeqCst);
    srv.bob_following.store(false, Ordering::SeqCst);
    srv.bob_followers.store(0, Ordering::SeqCst);
    let cg_rx = srv.comm_get_received.load(Ordering::SeqCst);
    let fl_b6 = srv.follow_completed.load(Ordering::SeqCst);
    srv.comm_feed_delay_ms.store(240, Ordering::SeqCst);
    let _ = cw.update(app, |w, cx| {
        w.navigate_to(CommunityRoute::CommunityPage(srv.slug.clone()), cx);
    });
    assert!(
        pump_until(app, Duration::from_secs(4), |_| {
            srv.comm_get_completed.load(Ordering::SeqCst) > cg_rx
        })
        .await
    );
    let _ = cw.update(app, |w, cx| {
        assert!(w.state.community_loading);
        w.toggle_follow_actor(srv.bob_id.clone(), true, cx);
    });
    assert!(
        pump_until(app, Duration::from_secs(5), |cx| {
            let st = &cw.read(cx).state;
            !st.community_loading
                && st.actor_follow_in_flight.is_empty()
                && srv.follow_completed.load(Ordering::SeqCst) > fl_b6
                && srv.comm_get_received.load(Ordering::SeqCst) >= cg_rx + 2
        })
        .await,
        "T9-06 (ADV-4): settled follow on CommunityPage must reload community_loading"
    );
    let _ = cw.read_with(app, |w, _| {
        let m = w
            .state
            .community_detail
            .as_ref()
            .unwrap()
            .moderators
            .iter()
            .find(|m| m.actor_id == srv.bob_id)
            .unwrap();
        assert!(
            m.is_following,
            "T9-06 (ADV-4): moderator bob must converge to is_following == true"
        );
    });
}

async fn run_r9_t07_to_t09(
    app: &mut AsyncApp,
    cw: &Entity<CommunityWorkspace>,
    srv: &Arc<R9MockState>,
) {
    use std::sync::atomic::Ordering;

    // ------------------------------------------------------------------
    // T9-07 (ADV-6 + load_community_page community_loading_more reset):
    // load_more_feed & load_more_community_posts racing delete_post_action -> 0 resurrection
    // AND community_loading_more == false so subsequent pagination still works!
    // ------------------------------------------------------------------
    let _ = cw.update(app, |w, cx| {
        w.navigate_to(CommunityRoute::Feed, cx);
        w.load_feed(cx);
    });
    assert!(
        pump_until(app, Duration::from_secs(4), |cx| {
            !cw.read(cx).state.feed_loading && cw.read(cx).state.next_cursor.is_some()
        })
        .await
    );
    let fg_b = srv.feed_get_received.load(Ordering::SeqCst);
    let del_b = srv.delete_post_completed.load(Ordering::SeqCst);
    srv.feed_more_delay_ms.store(240, Ordering::SeqCst);
    srv.delete_post_delay_ms.store(15, Ordering::SeqCst);
    let _ = cw.update(app, |w, cx| {
        w.load_more_feed(cx);
        assert!(w.state.feed_loading_more);
        w.delete_post_action(srv.p_page2.clone(), false, cx);
    });
    assert!(
        pump_until(app, Duration::from_secs(5), |cx| {
            let st = &cw.read(cx).state;
            !st.feed_loading
                && !st.feed_loading_more
                && srv.delete_post_completed.load(Ordering::SeqCst) > del_b
                && srv.feed_get_received.load(Ordering::SeqCst) > fg_b
        })
        .await,
        "T9-07 (ADV-6 feed): delete_post_action must fence load_more_feed"
    );
    app.background_executor().timer(Duration::from_millis(60)).await;
    let _ = cw.read_with(app, |w, _| {
        assert!(
            !w.state.feed_items.iter().any(|it| it.post.post_id == srv.p_page2),
            "T9-07 (ADV-6 feed): deleted p_page2 must NOT resurrect"
        );
    });

    // CommunityPage pagination race + community_loading_more reset check
    let _ = cw.update(app, |w, cx| {
        w.navigate_to(CommunityRoute::CommunityPage(srv.slug.clone()), cx);
    });
    assert!(
        pump_until(app, Duration::from_secs(4), |cx| {
            !cw.read(cx).state.community_loading && cw.read(cx).state.community_next_cursor.is_some()
        })
        .await
    );
    let cg_b7 = srv.comm_get_received.load(Ordering::SeqCst);
    let del_b7 = srv.delete_post_completed.load(Ordering::SeqCst);
    srv.comm_more_delay_ms.store(240, Ordering::SeqCst);
    srv.delete_post_delay_ms.store(15, Ordering::SeqCst);
    let _ = cw.update(app, |w, cx| {
        w.load_more_community_posts(srv.slug.clone(), cx);
        assert!(w.state.community_loading_more);
        w.delete_post_action(srv.p_cpage2.clone(), false, cx);
    });
    assert!(
        pump_until(app, Duration::from_secs(5), |cx| {
            let st = &cw.read(cx).state;
            !st.community_loading
                && !st.community_loading_more
                && srv.delete_post_completed.load(Ordering::SeqCst) > del_b7
                && srv.comm_get_received.load(Ordering::SeqCst) > cg_b7
        })
        .await,
        "T9-07 (ADV-6 comm): delete_post_action must fence load_more_community_posts and reset community_loading_more"
    );
    app.background_executor().timer(Duration::from_millis(60)).await;
    let cm_b = srv.comm_more_completed.load(Ordering::SeqCst);
    let _ = cw.update(app, |w, cx| {
        assert!(
            !w.state.community_posts.iter().any(|it| it.post.post_id == srv.p_cpage2),
            "T9-07 (ADV-6 comm): deleted p_cpage2 must NOT resurrect"
        );
        assert!(
            !w.state.community_loading_more,
            "T9-07: community_loading_more MUST be false after load_community_page"
        );
        // Verify subsequent load_more_community_posts still works (not permanently wedged!)
        w.load_more_community_posts(srv.slug.clone(), cx);
    });
    assert!(
        pump_until(app, Duration::from_secs(4), |cx| {
            !cw.read(cx).state.community_loading_more
                && srv.comm_more_completed.load(Ordering::SeqCst) > cm_b
        })
        .await,
        "T9-07: subsequent load_more_community_posts must execute cleanly"
    );

    // ------------------------------------------------------------------
    // T9-08 (ADV-7A/B + Siblings): save_community_rules & flush_automod_rules_sync
    // preserve in-flight Join/Leave + AutoMod; submit_edit_profile & save_selected_bot_edits
    // preserve in-flight Follow!
    // ------------------------------------------------------------------
    srv.comm_joined.store(false, Ordering::SeqCst);
    srv.comm_members.store(10, Ordering::SeqCst);
    let _ = cw.update(app, |w, cx| {
        w.load_community_page(srv.slug.clone(), cx);
        w.load_mod_queue(srv.slug.clone(), cx);
    });
    assert!(
        pump_until(app, Duration::from_secs(4), |cx| {
            let st = &cw.read(cx).state;
            !st.community_loading && !st.mod_loading && st.mod_community.is_some()
        })
        .await
    );
    let rp_b = srv.rules_patch_completed.load(Ordering::SeqCst);
    let jn_b8 = srv.join_completed.load(Ordering::SeqCst);
    let am_b8 = srv.automod_patch_completed.load(Ordering::SeqCst);
    srv.join_delay_ms.store(200, Ordering::SeqCst);
    srv.automod_patch_delay_ms.store(280, Ordering::SeqCst);
    srv.rules_patch_delay_ms.store(15, Ordering::SeqCst);
    let _ = cw.update(app, |w, cx| {
        w.toggle_join_community(srv.slug.clone(), true, cx);
        w.toggle_automod_rule_enabled("am_r9_1".to_string(), cx);
        w.state.community_new_rule_title = "Rule R9".to_string();
        w.add_community_rule(srv.slug.clone(), cx);
    });
    // Wait for fast save_community_rules to land WHILE slow Join & slow AutoMod are STILL in flight!
    assert!(
        pump_until(app, Duration::from_secs(4), |cx| {
            !cw.read(cx).state.community_rules_saving
                && srv.rules_patch_completed.load(Ordering::SeqCst) > rp_b
        })
        .await
    );
    let _ = cw.read_with(app, |w, _| {
        assert!(
            w.state.community_detail.as_ref().unwrap().community.is_joined,
            "T9-08 (ADV-7A): save_community_rules must preserve in-flight optimistic Join"
        );
        assert!(
            !w.state.mod_community.as_ref().unwrap().automod_rules[0].enabled,
            "T9-08 (ADV-7A): save_community_rules must preserve in-flight optimistic AutoMod toggle"
        );
    });
    assert!(
        pump_until(app, Duration::from_secs(5), |cx| {
            let st = &cw.read(cx).state;
            st.community_join_in_flight.is_empty()
                && st.automod_in_flight.is_empty()
                && srv.join_completed.load(Ordering::SeqCst) > jn_b8
                && srv.automod_patch_completed.load(Ordering::SeqCst) > am_b8
        })
        .await
    );

    // ADV-7B: flush_automod_rules_sync mid-flight Leave must preserve optimistic is_joined=false
    let am_b8b = srv.automod_patch_completed.load(Ordering::SeqCst);
    let jn_b8b = srv.join_completed.load(Ordering::SeqCst);
    srv.join_delay_ms.store(220, Ordering::SeqCst);
    srv.automod_patch_delay_ms.store(15, Ordering::SeqCst);
    let _ = cw.update(app, |w, cx| {
        w.toggle_join_community(srv.slug.clone(), false, cx);
        w.toggle_automod_rule_enabled("am_r9_1".to_string(), cx);
    });
    assert!(
        pump_until(app, Duration::from_secs(4), |cx| {
            cw.read(cx).state.automod_in_flight.is_empty()
                && srv.automod_patch_completed.load(Ordering::SeqCst) > am_b8b
        })
        .await
    );
    let _ = cw.read_with(app, |w, _| {
        assert!(
            !w.state.community_detail.as_ref().unwrap().community.is_joined,
            "T9-08 (ADV-7B): flush_automod_rules_sync must preserve in-flight Leave"
        );
    });
    assert!(
        pump_until(app, Duration::from_secs(5), |cx| {
            cw.read(cx).state.community_join_in_flight.is_empty()
                && srv.join_completed.load(Ordering::SeqCst) > jn_b8b
        })
        .await
    );

    // Siblings: submit_edit_profile & save_selected_bot_edits preserve in-flight Follow!
    let _ = cw.update(app, |w, cx| {
        w.navigate_to(CommunityRoute::ActorProfile(srv.bot_a_id.clone()), cx);
    });
    assert!(
        pump_until(app, Duration::from_secs(4), |cx| {
            !cw.read(cx).state.profile_loading
                && cw
                    .read(cx)
                    .state
                    .actor_profile
                    .as_ref()
                    .is_some_and(|p| p.actor.actor_id == srv.bot_a_id)
        })
        .await
    );
    let ap_b = srv.actor_patch_completed.load(Ordering::SeqCst);
    let fl_b8 = srv.follow_completed.load(Ordering::SeqCst);
    srv.follow_delay_ms.store(220, Ordering::SeqCst);
    srv.actor_patch_delays_by_name
        .lock()
        .unwrap()
        .insert("R9 Bot A Profile".to_string(), 15);
    let _ = cw.update(app, |w, cx| {
        let a = w.state.actor_profile.as_ref().unwrap().actor.clone();
        w.open_edit_profile_modal(a, cx);
        w.state.edit_profile.display_name = "R9 Bot A Profile".to_string();
        w.toggle_follow_actor(srv.bot_a_id.clone(), true, cx);
        w.submit_edit_profile(cx);
    });
    assert!(
        pump_until(app, Duration::from_secs(4), |cx| {
            !cw.read(cx).state.edit_profile.submitting
                && srv.actor_patch_completed.load(Ordering::SeqCst) > ap_b
        })
        .await
    );
    let _ = cw.read_with(app, |w, _| {
        assert!(
            w.state.actor_profile.as_ref().unwrap().actor.is_following,
            "T9-08 sibling: submit_edit_profile must preserve in-flight Follow"
        );
    });
    assert!(
        pump_until(app, Duration::from_secs(5), |cx| {
            cw.read(cx).state.actor_follow_in_flight.is_empty()
                && srv.follow_completed.load(Ordering::SeqCst) > fl_b8
        })
        .await
    );

    let ap_b2 = srv.actor_patch_completed.load(Ordering::SeqCst);
    let fl_b8b = srv.follow_completed.load(Ordering::SeqCst);
    srv.follow_delay_ms.store(220, Ordering::SeqCst);
    srv.actor_patch_delays_by_name
        .lock()
        .unwrap()
        .insert("R9 Bot A Studio".to_string(), 15);
    let _ = cw.update(app, |w, cx| {
        let bot_a = w.state.actor_profile.as_ref().unwrap().actor.clone();
        w.select_bot_for_studio(bot_a, cx);
        w.state.bot_studio_edit.display_name = "R9 Bot A Studio".to_string();
        w.toggle_follow_actor(srv.bot_a_id.clone(), false, cx);
        w.save_selected_bot_edits(cx);
    });
    assert!(
        pump_until(app, Duration::from_secs(4), |cx| {
            !cw.read(cx).state.bot_studio_edit.saving
                && srv.actor_patch_completed.load(Ordering::SeqCst) > ap_b2
        })
        .await
    );
    let _ = cw.read_with(app, |w, _| {
        assert!(
            !w.state.actor_profile.as_ref().unwrap().actor.is_following,
            "T9-08 sibling: save_selected_bot_edits must preserve in-flight Unfollow"
        );
    });
    assert!(
        pump_until(app, Duration::from_secs(5), |cx| {
            cw.read(cx).state.actor_follow_in_flight.is_empty()
                && srv.follow_completed.load(Ordering::SeqCst) > fl_b8b
        })
        .await
    );

    // ------------------------------------------------------------------
    // T9-09 (BUG-HUNT-3, BUG-HUNT-6, & All 8 Sibling Modal Mutexes)
    // ------------------------------------------------------------------
    // 1) BUG-HUNT-3 Fast-First ABA: Bot A Save#1 (80ms) -> B -> A Save#2 (260ms)
    let ap_aba1 = srv.actor_patch_completed.load(Ordering::SeqCst);
    {
        let mut m = srv.actor_patch_delays_by_name.lock().unwrap();
        m.clear();
        m.insert("Bot A v1".to_string(), 80);
        m.insert("Bot A v2".to_string(), 340);
    }
    let _ = cw.update(app, |w, cx| {
        let bots = w.state.identities.as_ref().unwrap().bot_actors.clone();
        let bot_a = bots.iter().find(|b| b.actor_id == srv.bot_a_id).cloned().unwrap();
        let bot_b = bots.iter().find(|b| b.actor_id == srv.bot_b_id).cloned().unwrap();
        w.select_bot_for_studio(bot_a.clone(), cx);
        w.state.bot_studio_edit.display_name = "Bot A v1".to_string();
        w.save_selected_bot_edits(cx);
        w.select_bot_for_studio(bot_b, cx);
        w.select_bot_for_studio(bot_a, cx);
        w.state.bot_studio_edit.display_name = "Bot A v2".to_string();
        w.save_selected_bot_edits(cx);
    });
    // Wait for Save#1 to land while Save#2 is still in flight!
    assert!(
        pump_until(app, Duration::from_secs(4), |_| {
            srv.actor_patch_completed.load(Ordering::SeqCst) >= ap_aba1 + 1
        })
        .await
    );
    app.background_executor().timer(Duration::from_millis(25)).await;
    let _ = cw.read_with(app, |w, _| {
        assert!(
            w.state.bot_studio_edit.saving,
            "T9-09 (BUG-HUNT-3): stale ABA Save#1 must NOT clear Save#2's saving flag"
        );
        assert_eq!(
            w.state.bot_studio_edit.display_name, "Bot A v2",
            "T9-09 (BUG-HUNT-3): stale ABA Save#1 must NOT clobber 'Bot A v2'"
        );
    });
    assert!(
        pump_until(app, Duration::from_secs(5), |cx| {
            !cw.read(cx).state.bot_studio_edit.saving
                && srv.actor_patch_completed.load(Ordering::SeqCst) >= ap_aba1 + 2
        })
        .await
    );
    let _ = cw.read_with(app, |w, _| {
        assert_eq!(w.state.bot_studio_edit.display_name, "Bot A v2");
    });

    // 2) BUG-HUNT-3 Slow-First (reverse-order) ABA: Save#1 (260ms) -> B -> A Save#2 (20ms)
    let ap_aba2 = srv.actor_patch_completed.load(Ordering::SeqCst);
    {
        let mut m = srv.actor_patch_delays_by_name.lock().unwrap();
        m.clear();
        m.insert("Bot A Stale".to_string(), 260);
        m.insert("Bot A Final".to_string(), 20);
    }
    let _ = cw.update(app, |w, cx| {
        let bots = w.state.identities.as_ref().unwrap().bot_actors.clone();
        let bot_a = bots.iter().find(|b| b.actor_id == srv.bot_a_id).cloned().unwrap();
        let bot_b = bots.iter().find(|b| b.actor_id == srv.bot_b_id).cloned().unwrap();
        w.select_bot_for_studio(bot_a.clone(), cx);
        w.state.bot_studio_edit.display_name = "Bot A Stale".to_string();
        w.save_selected_bot_edits(cx);
        w.select_bot_for_studio(bot_b, cx);
        w.select_bot_for_studio(bot_a, cx);
        w.state.bot_studio_edit.display_name = "Bot A Final".to_string();
        w.save_selected_bot_edits(cx);
    });
    assert!(
        pump_until(app, Duration::from_secs(5), |cx| {
            !cw.read(cx).state.bot_studio_edit.saving
                && srv.actor_patch_completed.load(Ordering::SeqCst) >= ap_aba2 + 2
        })
        .await
    );
    app.background_executor().timer(Duration::from_millis(80)).await;
    let _ = cw.read_with(app, |w, _| {
        assert_eq!(w.state.bot_studio_edit.display_name, "Bot A Final");
        assert_eq!(
            w.state.actor_profile.as_ref().unwrap().actor.display_name,
            "Bot A Final",
            "T9-09 (BUG-HUNT-3 sibling): reverse-order Save#1 must NOT clobber prof.actor"
        );
    });

    // 3) BUG-HUNT-6: save_community_wiki mid-flight same-slug load_community_page
    let _ = cw.update(app, |w, cx| {
        w.navigate_to(CommunityRoute::CommunityPage(srv.slug.clone()), cx);
    });
    assert!(
        pump_until(app, Duration::from_secs(4), |cx| {
            !cw.read(cx).state.community_loading && cw.read(cx).state.community_detail.is_some()
        })
        .await
    );
    let wk_b = srv.wiki_put_completed.load(Ordering::SeqCst);
    srv.wiki_put_delay_ms.store(140, Ordering::SeqCst);
    srv.comm_get_delay_ms.store(15, Ordering::SeqCst);
    let _ = cw.update(app, |w, cx| {
        w.state.community_wiki_editing = true;
        w.state.community_wiki_draft = "Wiki R9 Closed".to_string();
        w.save_community_wiki(srv.slug.clone(), cx);
        // Same-slug reload mid-save must NOT clear community_wiki_saving!
        w.load_community_page(srv.slug.clone(), cx);
        assert!(
            w.state.community_wiki_saving,
            "T9-09 (BUG-HUNT-6 sibling): same-slug load_community_page must NOT unlock community_wiki_saving"
        );
        // Second save attempt while in flight must be ignored!
        w.save_community_wiki(srv.slug.clone(), cx);
    });
    assert!(
        pump_until(app, Duration::from_secs(5), |cx| {
            let st = &cw.read(cx).state;
            !st.community_wiki_saving
                && !st.community_loading
                && srv.wiki_put_completed.load(Ordering::SeqCst) > wk_b
        })
        .await
    );
    let _ = cw.read_with(app, |w, _| {
        assert!(
            !w.state.community_wiki_editing,
            "T9-09 (BUG-HUNT-6): save_community_wiki must exit editing mode after same-slug reload"
        );
        assert_eq!(
            w.state.community_detail.as_ref().unwrap().community.wiki_markdown,
            "Wiki R9 Closed"
        );
        assert_eq!(
            srv.wiki_put_completed.load(Ordering::SeqCst),
            wk_b + 1,
            "T9-09: duplicate wiki submit must be blocked"
        );
    });

    // 4) All 8 Sibling Modal Mutexes (EditProfile, CreateCommunity, and 6 Composer entrypoints)
    let ap_m = srv.actor_patch_completed.load(Ordering::SeqCst);
    srv.actor_patch_delays_by_name
        .lock()
        .unwrap()
        .insert("Alice Submitting".to_string(), 120);
    let _ = cw.update(app, |w, cx| {
        let a = srv.build_actor(&srv.alice_id);
        w.open_edit_profile_modal(a.clone(), cx);
        w.state.edit_profile.display_name = "Alice Submitting".to_string();
        w.submit_edit_profile(cx);
        assert!(w.state.edit_profile.submitting);
        w.open_edit_profile_modal(a, cx);
        assert!(
            w.state.edit_profile.submitting && w.state.edit_profile.display_name == "Alice Submitting",
            "T9-09 mutex: open_edit_profile_modal must be blocked while submitting"
        );
    });
    assert!(
        pump_until(app, Duration::from_secs(4), |cx| {
            !cw.read(cx).state.edit_profile.submitting
                && srv.actor_patch_completed.load(Ordering::SeqCst) > ap_m
        })
        .await
    );

    let cc_m = srv.create_comm_completed.load(Ordering::SeqCst);
    srv.create_comm_delay_ms.store(120, Ordering::SeqCst);
    let _ = cw.update(app, |w, cx| {
        w.open_create_community_modal(cx);
        w.state.create_community.slug = "r9_new_comm".to_string();
        w.state.create_community.name = "R9 New Comm".to_string();
        w.submit_create_community(cx);
        assert!(w.state.create_community.submitting);
        w.open_create_community_modal(cx);
        assert!(
            w.state.create_community.submitting && w.state.create_community.slug == "r9_new_comm",
            "T9-09 mutex: open_create_community_modal must be blocked while submitting"
        );
    });
    assert!(
        pump_until(app, Duration::from_secs(4), |cx| {
            !cw.read(cx).state.create_community.submitting
                && srv.create_comm_completed.load(Ordering::SeqCst) > cc_m
        })
        .await
    );

    let cp_m = srv.create_post_completed.load(Ordering::SeqCst);
    srv.create_post_delay_ms.store(140, Ordering::SeqCst);
    let _ = cw.update(app, |w, cx| {
        let sample = srv.build_post(&srv.p_main, &srv.alice_id);
        w.open_composer_modal(None, cx);
        w.state.composer.body = "Original Submitting Body".to_string();
        w.submit_composer(PostStatus::Published, cx);
        assert!(w.state.composer.submitting);

        // All 6 Composer open/resume entrypoints MUST be blocked while submitting!
        w.open_composer_modal(Some("rust".to_string()), cx);
        w.open_composer_from_quick_post(cx);
        w.open_composer_quote(sample.clone(), cx);
        w.open_composer_crosspost(sample.clone(), cx);
        w.open_composer_edit(sample.clone(), cx);
        w.resume_draft_into_composer(sample, cx);
        assert!(
            w.state.composer.submitting && w.state.composer.body == "Original Submitting Body",
            "T9-09 mutex: all 6 Composer entrypoints must be blocked while submitting"
        );
        // Second submit attempt while submitting must also be a no-op!
        w.submit_composer(PostStatus::Published, cx);
    });
    assert!(
        pump_until(app, Duration::from_secs(4), |cx| {
            !cw.read(cx).state.composer.submitting
                && srv.create_post_completed.load(Ordering::SeqCst) > cp_m
        })
        .await
    );
    assert_eq!(
        srv.create_post_completed.load(Ordering::SeqCst),
        cp_m + 1,
        "T9-09 mutex: 0 duplicate Composer submits permitted"
    );
}

// =============================================================================
// Wave r10 Regression Suite: Dead End #4 Field-Scoped Patching, Symmetric
// Unblock <-> Follow Serialization, Monotonic save_seq ABA, R9-ADV-NEW-6,
// and recommended_actors Case B Convergence (`T10-01`..`T10-08` + `T10-DE4-5`)
// =============================================================================

struct R10MockState {
    alice_id: String,
    bot_a_id: String,
    bob_id: String,
    slug: String,
    alt_slug: String,
    p_edit: String,
    p_part1: String,
    p_part2: String,
    c1_id: String,
    c2_id: String,
    list_id: String,

    // Community state (for rust; alt_slug is static "ai")
    comm_joined: std::sync::atomic::AtomicBool,
    comm_members: std::sync::atomic::AtomicU64,
    comm_wiki: std::sync::Mutex<String>,
    comm_wiki_rev: std::sync::atomic::AtomicU32,
    comm_rules: std::sync::Mutex<Vec<gpui_telegram_proto::community_contracts::CommunityRuleDto>>,
    comm_automod_rules: std::sync::Mutex<Vec<AutoModRuleDto>>,

    // Post engagement & edit state
    post_titles: std::sync::Mutex<std::collections::HashMap<String, Option<String>>>,
    post_bodies: std::sync::Mutex<std::collections::HashMap<String, String>>,
    post_flairs: std::sync::Mutex<std::collections::HashMap<String, Option<String>>>,
    post_nsfw: std::sync::Mutex<std::collections::HashMap<String, bool>>,
    post_spoiler: std::sync::Mutex<std::collections::HashMap<String, bool>>,
    post_updated_ts: std::sync::atomic::AtomicI64,
    post_votes: std::sync::Mutex<std::collections::HashMap<String, i8>>,
    post_likes: std::sync::Mutex<std::collections::HashSet<String>>,
    post_reposts: std::sync::Mutex<std::collections::HashSet<String>>,
    post_bookmarked: std::sync::Mutex<std::collections::HashSet<String>>,
    poll_voted_opt: std::sync::Mutex<Vec<String>>,
    post_awards: std::sync::Mutex<Vec<gpui_telegram_proto::community_contracts::PostAwardDto>>,

    // Comment state
    comment_bodies: std::sync::Mutex<std::collections::HashMap<String, String>>,
    comment_votes: std::sync::Mutex<std::collections::HashMap<String, i8>>,

    // Actor state (alice, bot_a, bob)
    alice_display_name: std::sync::Mutex<String>,
    bot_a_display_name: std::sync::Mutex<String>,
    bot_a_following: std::sync::atomic::AtomicBool,
    bot_a_followers: std::sync::atomic::AtomicU64,
    bot_a_muted: std::sync::atomic::AtomicBool,
    bob_blocked: std::sync::atomic::AtomicBool,
    bob_following: std::sync::atomic::AtomicBool,
    bob_followers: std::sync::atomic::AtomicU64,
    bob_muted: std::sync::atomic::AtomicBool,

    // List state
    list_name: std::sync::Mutex<String>,
    list_pinned: std::sync::atomic::AtomicBool,
    list_updated_ts: std::sync::atomic::AtomicI64,

    // Explicit pre-commit / post-commit delay knobs & atomic meters
    rules_post_delay_ms: std::sync::atomic::AtomicU64,
    rules_patch_received: std::sync::atomic::AtomicUsize,
    rules_patch_completed: std::sync::atomic::AtomicUsize,
    automod_post_delay_ms: std::sync::atomic::AtomicU64,
    automod_fail_500: std::sync::atomic::AtomicBool,
    automod_patch_received: std::sync::atomic::AtomicUsize,
    automod_patch_completed: std::sync::atomic::AtomicUsize,
    join_delay_ms: std::sync::atomic::AtomicU64,
    join_completed: std::sync::atomic::AtomicUsize,
    wiki_post_delay_ms: std::sync::atomic::AtomicU64,
    wiki_put_received: std::sync::atomic::AtomicUsize,
    wiki_put_completed: std::sync::atomic::AtomicUsize,
    post_patch_post_delay_ms: std::sync::atomic::AtomicU64,
    post_patch_received: std::sync::atomic::AtomicUsize,
    post_patch_completed: std::sync::atomic::AtomicUsize,
    post_vote_completed: std::sync::atomic::AtomicUsize,
    post_like_completed: std::sync::atomic::AtomicUsize,
    post_repost_completed: std::sync::atomic::AtomicUsize,
    post_bookmark_completed: std::sync::atomic::AtomicUsize,
    poll_vote_completed: std::sync::atomic::AtomicUsize,
    award_completed: std::sync::atomic::AtomicUsize,
    actor_patch_post_delays_by_name: std::sync::Mutex<std::collections::HashMap<String, u64>>,
    actor_patch_received: std::sync::atomic::AtomicUsize,
    actor_patch_completed: std::sync::atomic::AtomicUsize,
    list_toggle_post_delay_ms: std::sync::atomic::AtomicU64,
    list_toggle_received: std::sync::atomic::AtomicUsize,
    list_toggle_completed: std::sync::atomic::AtomicUsize,
    lists_get_completed: std::sync::atomic::AtomicUsize,
    follow_block_in_flight: std::sync::atomic::AtomicUsize,
    follow_block_max_in_flight: std::sync::atomic::AtomicUsize,
    follow_403_count: std::sync::atomic::AtomicUsize,
    follow_calls: std::sync::atomic::AtomicUsize,
    block_calls: std::sync::atomic::AtomicUsize,
    follow_pre_delay_ms: std::sync::atomic::AtomicU64,
    follow_fail_500: std::sync::atomic::AtomicBool,
    follow_completed: std::sync::atomic::AtomicUsize,
    block_pre_delay_ms: std::sync::atomic::AtomicU64,
    block_post_delay_ms: std::sync::atomic::AtomicU64,
    block_fail_500: std::sync::atomic::AtomicBool,
    block_received: std::sync::atomic::AtomicUsize,
    block_completed: std::sync::atomic::AtomicUsize,
    mute_completed: std::sync::atomic::AtomicUsize,
    comment_patch_delays_by_body: std::sync::Mutex<std::collections::HashMap<String, u64>>,
    comment_patch_received: std::sync::atomic::AtomicUsize,
    comment_patch_completed: std::sync::atomic::AtomicUsize,
    comment_vote_completed: std::sync::atomic::AtomicUsize,
    detail_get_delay_ms: std::sync::atomic::AtomicU64,
    detail_get_received: std::sync::atomic::AtomicUsize,
    detail_get_completed: std::sync::atomic::AtomicUsize,
    search_delay_ms: std::sync::atomic::AtomicU64,
    search_received: std::sync::atomic::AtomicUsize,
    search_completed: std::sync::atomic::AtomicUsize,
    profile_get_completed: std::sync::atomic::AtomicUsize,
}

impl R10MockState {
    fn build_actor(&self, actor_id: &str) -> CommunityActorDto {
        use std::sync::atomic::Ordering;
        if actor_id == self.alice_id {
            let mut a = mock_actor_dto(
                &self.alice_id,
                ActorKind::Human,
                "r10_alice",
                &self.alice_display_name.lock().unwrap(),
            );
            a.linked_user_id = Some(910001);
            a
        } else if actor_id == self.bot_a_id {
            let mut a = mock_actor_dto(
                &self.bot_a_id,
                ActorKind::Bot,
                "r10_bot_a",
                &self.bot_a_display_name.lock().unwrap(),
            );
            a.owner_user_id = Some(910001);
            a.is_following = self.bot_a_following.load(Ordering::SeqCst);
            a.followers_count = self.bot_a_followers.load(Ordering::SeqCst);
            a.is_muted = self.bot_a_muted.load(Ordering::SeqCst);
            a
        } else {
            let mut a = mock_actor_dto(&self.bob_id, ActorKind::Human, "r10_bob", "R10 Bob");
            a.is_blocked = self.bob_blocked.load(Ordering::SeqCst);
            a.is_following = self.bob_following.load(Ordering::SeqCst);
            a.followers_count = self.bob_followers.load(Ordering::SeqCst);
            a.is_muted = self.bob_muted.load(Ordering::SeqCst);
            a
        }
    }

    fn build_community(&self, slug: &str) -> SubCommunityDto {
        use std::sync::atomic::Ordering;
        if slug.eq_ignore_ascii_case(&self.alt_slug) {
            let mut c = mock_community_dto(&self.alt_slug, &self.alice_id, Some(CommunityMemberRole::Owner));
            c.is_joined = true;
            c.member_count = 4;
            return c;
        }
        let mut c = mock_community_dto(&self.slug, &self.alice_id, Some(CommunityMemberRole::Owner));
        c.is_joined = self.comm_joined.load(Ordering::SeqCst);
        c.member_count = self.comm_members.load(Ordering::SeqCst);
        c.wiki_markdown = self.comm_wiki.lock().unwrap().clone();
        c.wiki_revision = self.comm_wiki_rev.load(Ordering::SeqCst);
        c.rules = self.comm_rules.lock().unwrap().clone();
        c.automod_rules = self.comm_automod_rules.lock().unwrap().clone();
        c
    }

    fn build_post(&self, post_id: &str) -> CommunityPostDto {
        use gpui_telegram_proto::community_contracts::{CommunityPollOptionDto, PollDto};
        use std::sync::atomic::Ordering;
        let bk = self.post_bookmarked.lock().unwrap().contains(post_id);
        let mut p = mock_post_dto(
            post_id,
            &self.alice_id,
            bk,
            bk.then_some("All"),
            u64::from(bk),
        );
        p.community_slug = Some(self.slug.clone());
        p.title = self
            .post_titles
            .lock()
            .unwrap()
            .get(post_id)
            .cloned()
            .unwrap_or_else(|| Some(format!("Title {post_id}")));
        p.body = self
            .post_bodies
            .lock()
            .unwrap()
            .get(post_id)
            .cloned()
            .unwrap_or_else(|| format!("Body {post_id}"));
        p.flair = self
            .post_flairs
            .lock()
            .unwrap()
            .get(post_id)
            .cloned()
            .unwrap_or(None);
        p.is_nsfw = *self.post_nsfw.lock().unwrap().get(post_id).unwrap_or(&false);
        p.is_spoiler = *self.post_spoiler.lock().unwrap().get(post_id).unwrap_or(&false);
        p.updated_at_ms = self.post_updated_ts.load(Ordering::SeqCst);
        let vote = *self.post_votes.lock().unwrap().get(post_id).unwrap_or(&0);
        p.viewer_vote = vote;
        p.upvotes = i64::from(vote == 1);
        p.downvotes = i64::from(vote == -1);
        p.score = p.upvotes - p.downvotes;
        let liked = self.post_likes.lock().unwrap().contains(post_id);
        p.viewer_liked = liked;
        p.likes_count = u64::from(liked);
        let reposted = self.post_reposts.lock().unwrap().contains(post_id);
        p.viewer_reposted = reposted;
        p.reposts_count = u64::from(reposted);
        if post_id == self.p_edit {
            let voted_opts = self.poll_voted_opt.lock().unwrap().clone();
            let opt1_votes = u64::from(voted_opts.iter().any(|o| o == "opt_1"));
            p.poll = Some(PollDto {
                poll_id: "poll_r10".to_string(),
                question: "Best patch?".to_string(),
                options: vec![
                    CommunityPollOptionDto {
                        option_id: "opt_1".to_string(),
                        text: "Field-scoped".to_string(),
                        all_votes: opt1_votes,
                        human_votes: opt1_votes,
                        bot_votes: 0,
                    },
                    CommunityPollOptionDto {
                        option_id: "opt_2".to_string(),
                        text: "Full DTO".to_string(),
                        all_votes: 0,
                        human_votes: 0,
                        bot_votes: 0,
                    },
                ],
                multi_select: false,
                expires_at_ms: 1_900_000_000_000,
                is_closed: false,
                total_votes: opt1_votes,
                total_human_votes: opt1_votes,
                total_bot_votes: 0,
                viewer_voted_option_ids: voted_opts,
            });
            p.awards = self.post_awards.lock().unwrap().clone();
        }
        p
    }

    fn build_comment(&self, cid: &str, post_id: &str) -> CommunityCommentDto {
        let body = self
            .comment_bodies
            .lock()
            .unwrap()
            .get(cid)
            .cloned()
            .unwrap_or_else(|| format!("Comment {cid}"));
        let vote = *self.comment_votes.lock().unwrap().get(cid).unwrap_or(&0);
        CommunityCommentDto {
            comment_id: cid.to_string(),
            post_id: post_id.to_string(),
            parent_comment_id: None,
            depth: 0,
            path: cid.to_string(),
            author_actor_id: self.alice_id.clone(),
            author_handle: "r10_alice".to_string(),
            author_display_name: "R10 Alice".to_string(),
            author_kind: ActorKind::Human,
            author_verified_tier: VerifiedTier::None,
            author_bot_badge: None,
            author_avatar_shape: "circle".to_string(),
            author_avatar_color: "blue".to_string(),
            is_op: true,
            body,
            media: Vec::new(),
            upvotes: i64::from(vote == 1),
            downvotes: i64::from(vote == -1),
            score: i64::from(vote),
            likes_count: 0,
            awards: Vec::new(),
            viewer_vote: vote,
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

    fn build_list(&self) -> CommunityListDto {
        use std::sync::atomic::Ordering;
        let mut l = mock_list_dto(
            &self.list_id,
            &self.alice_id,
            &self.list_name.lock().unwrap(),
        );
        l.is_pinned = self.list_pinned.load(Ordering::SeqCst);
        l.updated_at_ms = self.list_updated_ts.load(Ordering::SeqCst);
        l
    }
}

fn spawn_r10_server(
    rt: &tokio::runtime::Handle,
) -> (std::net::SocketAddr, Arc<R10MockState>, AbortOnDrop) {
    use gpui_telegram_proto::community_contracts::{
        AwardResponse, CommunityPollVoteRequest, CommunityRuleDto, CommunityWikiRevisionDto,
        FollowActionResponse, GiveAwardRequest, JoinCommunityResponse, PostAwardDto,
        RepostResponse, UpdateCommentRequest, UpdateCommunityActorRequest,
        UpdateCommunityWikiRequest, UpdatePostRequest,
    };
    use std::sync::atomic::{AtomicBool, AtomicI64, AtomicU32, AtomicU64, AtomicUsize, Ordering};
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    let alice_id = "u:910001".to_string();
    let bot_a_id = "b:910002".to_string();
    let bob_id = "u:910300".to_string();
    let slug = "rust".to_string();
    let alt_slug = "ai".to_string();
    let p_edit = "p_r10_edit".to_string();
    let p_part1 = "p_r10_part1".to_string();
    let p_part2 = "p_r10_part2".to_string();
    let c1_id = "c_r10_1".to_string();
    let c2_id = "c_r10_2".to_string();
    let list_id = "l_r10_1".to_string();

    let seed_rules = vec![CommunityRuleDto {
        id: "rule_1".to_string(),
        title: "Initial Rule".to_string(),
        description: "Be kind".to_string(),
        order: 1,
    }];
    let seed_automod = vec![AutoModRuleDto {
        rule_id: "am_r10_1".to_string(),
        community_id: format!("comm_{slug}"),
        name: "Block Spam".to_string(),
        enabled: true,
        priority: 1,
        keywords: vec!["spam".to_string()],
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
    }];
    let mut seed_bodies = std::collections::HashMap::new();
    seed_bodies.insert(p_edit.clone(), "Initial Edit Body".to_string());
    seed_bodies.insert(p_part1.clone(), "Thread Part 1".to_string());
    seed_bodies.insert(p_part2.clone(), "Thread Part 2".to_string());
    let mut seed_comments = std::collections::HashMap::new();
    seed_comments.insert(c1_id.clone(), "Initial C1".to_string());
    seed_comments.insert(c2_id.clone(), "Initial C2".to_string());

    let state = Arc::new(R10MockState {
        alice_id,
        bot_a_id,
        bob_id,
        slug,
        alt_slug,
        p_edit,
        p_part1,
        p_part2,
        c1_id,
        c2_id,
        list_id,
        comm_joined: AtomicBool::new(true),
        comm_members: AtomicU64::new(10),
        comm_wiki: std::sync::Mutex::new("Initial Wiki".to_string()),
        comm_wiki_rev: AtomicU32::new(1),
        comm_rules: std::sync::Mutex::new(seed_rules),
        comm_automod_rules: std::sync::Mutex::new(seed_automod),
        post_titles: std::sync::Mutex::new(std::collections::HashMap::new()),
        post_bodies: std::sync::Mutex::new(seed_bodies),
        post_flairs: std::sync::Mutex::new(std::collections::HashMap::new()),
        post_nsfw: std::sync::Mutex::new(std::collections::HashMap::new()),
        post_spoiler: std::sync::Mutex::new(std::collections::HashMap::new()),
        post_updated_ts: AtomicI64::new(1_750_000_001_000),
        post_votes: std::sync::Mutex::new(std::collections::HashMap::new()),
        post_likes: std::sync::Mutex::new(std::collections::HashSet::new()),
        post_reposts: std::sync::Mutex::new(std::collections::HashSet::new()),
        post_bookmarked: std::sync::Mutex::new(std::collections::HashSet::new()),
        poll_voted_opt: std::sync::Mutex::new(Vec::new()),
        post_awards: std::sync::Mutex::new(Vec::new()),
        comment_bodies: std::sync::Mutex::new(seed_comments),
        comment_votes: std::sync::Mutex::new(std::collections::HashMap::new()),
        alice_display_name: std::sync::Mutex::new("R10 Alice".to_string()),
        bot_a_display_name: std::sync::Mutex::new("R10 Bot A".to_string()),
        bot_a_following: AtomicBool::new(false),
        bot_a_followers: AtomicU64::new(2),
        bot_a_muted: AtomicBool::new(false),
        bob_blocked: AtomicBool::new(false),
        bob_following: AtomicBool::new(false),
        bob_followers: AtomicU64::new(5),
        bob_muted: AtomicBool::new(false),
        list_name: std::sync::Mutex::new("Old List".to_string()),
        list_pinned: AtomicBool::new(false),
        list_updated_ts: AtomicI64::new(1_000),
        rules_post_delay_ms: AtomicU64::new(0),
        rules_patch_received: AtomicUsize::new(0),
        rules_patch_completed: AtomicUsize::new(0),
        automod_post_delay_ms: AtomicU64::new(0),
        automod_fail_500: AtomicBool::new(false),
        automod_patch_received: AtomicUsize::new(0),
        automod_patch_completed: AtomicUsize::new(0),
        join_delay_ms: AtomicU64::new(0),
        join_completed: AtomicUsize::new(0),
        wiki_post_delay_ms: AtomicU64::new(0),
        wiki_put_received: AtomicUsize::new(0),
        wiki_put_completed: AtomicUsize::new(0),
        post_patch_post_delay_ms: AtomicU64::new(0),
        post_patch_received: AtomicUsize::new(0),
        post_patch_completed: AtomicUsize::new(0),
        post_vote_completed: AtomicUsize::new(0),
        post_like_completed: AtomicUsize::new(0),
        post_repost_completed: AtomicUsize::new(0),
        post_bookmark_completed: AtomicUsize::new(0),
        poll_vote_completed: AtomicUsize::new(0),
        award_completed: AtomicUsize::new(0),
        actor_patch_post_delays_by_name: std::sync::Mutex::new(std::collections::HashMap::new()),
        actor_patch_received: AtomicUsize::new(0),
        actor_patch_completed: AtomicUsize::new(0),
        list_toggle_post_delay_ms: AtomicU64::new(0),
        list_toggle_received: AtomicUsize::new(0),
        list_toggle_completed: AtomicUsize::new(0),
        lists_get_completed: AtomicUsize::new(0),
        follow_block_in_flight: AtomicUsize::new(0),
        follow_block_max_in_flight: AtomicUsize::new(0),
        follow_403_count: AtomicUsize::new(0),
        follow_calls: AtomicUsize::new(0),
        block_calls: AtomicUsize::new(0),
        follow_pre_delay_ms: AtomicU64::new(0),
        follow_fail_500: AtomicBool::new(false),
        follow_completed: AtomicUsize::new(0),
        block_pre_delay_ms: AtomicU64::new(0),
        block_post_delay_ms: AtomicU64::new(0),
        block_fail_500: AtomicBool::new(false),
        block_received: AtomicUsize::new(0),
        block_completed: AtomicUsize::new(0),
        mute_completed: AtomicUsize::new(0),
        comment_patch_delays_by_body: std::sync::Mutex::new(std::collections::HashMap::new()),
        comment_patch_received: AtomicUsize::new(0),
        comment_patch_completed: AtomicUsize::new(0),
        comment_vote_completed: AtomicUsize::new(0),
        detail_get_delay_ms: AtomicU64::new(0),
        detail_get_received: AtomicUsize::new(0),
        detail_get_completed: AtomicUsize::new(0),
        search_delay_ms: AtomicU64::new(0),
        search_received: AtomicUsize::new(0),
        search_completed: AtomicUsize::new(0),
        profile_get_completed: AtomicUsize::new(0),
    });

    let std_listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind r10 server");
    std_listener.set_nonblocking(true).expect("nonblocking");
    let addr = std_listener.local_addr().expect("r10 local_addr");
    let st_accept = Arc::clone(&state);
    let join = rt.spawn(async move {
        let listener = tokio::net::TcpListener::from_std(std_listener).expect("tokio listener");
        loop {
            let Ok((mut sock, _)) = listener.accept().await else {
                break;
            };
            let st = Arc::clone(&st_accept);
            tokio::spawn(async move {
                let mut buf = Vec::with_capacity(8192);
                let mut tmp = [0u8; 4096];
                let mut header_end = None;
                let mut content_len = 0usize;
                loop {
                    let Ok(n) = sock.read(&mut tmp).await else {
                        return;
                    };
                    if n == 0 {
                        return;
                    }
                    buf.extend_from_slice(&tmp[..n]);
                    if header_end.is_none() {
                        if let Some(pos) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
                            header_end = Some(pos + 4);
                            let hdr = String::from_utf8_lossy(&buf[..pos]);
                            for line in hdr.lines() {
                                if let Some((k, v)) = line.split_once(':') {
                                    if k.eq_ignore_ascii_case("content-length") {
                                        content_len = v.trim().parse().unwrap_or(0);
                                    }
                                }
                            }
                        }
                    }
                    if let Some(he) = header_end {
                        if buf.len() >= he + content_len {
                            break;
                        }
                    }
                }
                let he = header_end.unwrap();
                let hdr_str = String::from_utf8_lossy(&buf[..he]).to_string();
                let mut lines = hdr_str.lines();
                let req_line = lines.next().unwrap_or_default();
                let mut parts = req_line.split_whitespace();
                let method = parts.next().unwrap_or("GET").to_string();
                let raw_target = parts.next().unwrap_or("/").to_string();
                let (raw_path, _query) = raw_target
                    .split_once('?')
                    .map(|(p, q)| (p.to_string(), q.to_string()))
                    .unwrap_or((raw_target, String::new()));
                let path = raw_path.replace("%3A", ":").replace("%3a", ":");
                let mut acting_actor: Option<String> = None;
                for line in lines {
                    if let Some((k, v)) = line.split_once(':') {
                        if k.eq_ignore_ascii_case("x-community-actor-id") {
                            acting_actor = Some(v.trim().to_string());
                        }
                    }
                }
                let viewer = acting_actor.unwrap_or_else(|| st.alice_id.clone());
                let body_bytes = &buf[he..he + content_len];

                let (status, resp_body): (&str, String) = if method == "GET"
                    && path == "/api/v1/community/identities"
                {
                    let human = st.build_actor(&st.alice_id);
                    let bot_a = st.build_actor(&st.bot_a_id);
                    let resp = CommunityIdentitiesResponse {
                        active_actor_id: viewer.clone(),
                        human_actor: Some(human.clone()),
                        bot_actors: vec![bot_a.clone()],
                        all_actors: vec![human, bot_a],
                    };
                    ("200 OK", serde_json::to_string(&resp).unwrap())
                } else if method == "GET" && path == "/api/v1/community/algo-prefs" {
                    ("200 OK", serde_json::to_string(&AlgoPreferencesDto::default()).unwrap())
                } else if method == "GET" && path == "/api/v1/community/trending" {
                    ("200 OK", serde_json::to_string(&TrendingResponse::default()).unwrap())
                } else if method == "GET" && path == "/api/v1/community/notifications" {
                    ("200 OK", serde_json::to_string(&NotificationsListResponse::default()).unwrap())
                } else if method == "GET" && path == "/api/v1/community/search" {
                    st.search_received.fetch_add(1, Ordering::SeqCst);
                    let snap = CommunitySearchResultDto {
                        query: String::new(),
                        tab: CommunitySearchTab::Top,
                        posts: vec![st.build_post(&st.p_edit)],
                        actors: vec![st.build_actor(&st.bot_a_id), st.build_actor(&st.bob_id)],
                        communities: vec![st.build_community(&st.slug)],
                        total_hits: 2,
                        next_offset: None,
                    };
                    let delay = st.search_delay_ms.swap(0, Ordering::SeqCst);
                    if delay > 0 {
                        tokio::time::sleep(Duration::from_millis(delay)).await;
                    }
                    st.search_completed.fetch_add(1, Ordering::SeqCst);
                    ("200 OK", serde_json::to_string(&snap).unwrap())
                } else if method == "GET" && path == "/api/v1/community/communities" {
                    let resp = ListCommunitiesResponse {
                        communities: vec![
                            st.build_community(&st.slug),
                            st.build_community(&st.alt_slug),
                        ],
                    };
                    ("200 OK", serde_json::to_string(&resp).unwrap())
                } else if method == "GET" && path == format!("/api/v1/community/communities/{}", st.slug) {
                    let resp = CommunityDetailResponse {
                        community: st.build_community(&st.slug),
                        pinned_posts: Vec::new(),
                        moderators: vec![
                            st.build_actor(&st.alice_id),
                            st.build_actor(&st.bot_a_id),
                            st.build_actor(&st.bob_id),
                        ],
                        wiki_revisions: Vec::new(),
                    };
                    ("200 OK", serde_json::to_string(&resp).unwrap())
                } else if method == "GET" && path == format!("/api/v1/community/communities/{}", st.alt_slug) {
                    let resp = CommunityDetailResponse {
                        community: st.build_community(&st.alt_slug),
                        pinned_posts: Vec::new(),
                        moderators: vec![st.build_actor(&st.alice_id)],
                        wiki_revisions: Vec::new(),
                    };
                    ("200 OK", serde_json::to_string(&resp).unwrap())
                } else if method == "PATCH" && path == format!("/api/v1/community/communities/{}", st.slug) {
                    let req: UpdateCommunityRequest = serde_json::from_slice(body_bytes).expect("uc");
                    let is_automod = req.automod_rules.is_some();
                    if is_automod {
                        st.automod_patch_received.fetch_add(1, Ordering::SeqCst);
                        if st.automod_fail_500.load(Ordering::SeqCst) {
                            let post_delay = st.automod_post_delay_ms.swap(0, Ordering::SeqCst);
                            if post_delay > 0 {
                                tokio::time::sleep(Duration::from_millis(post_delay)).await;
                            }
                            st.automod_patch_completed.fetch_add(1, Ordering::SeqCst);
                            ("500 Internal Server Error", r#"{"error":"automod_500"}"#.to_string())
                        } else {
                            if let Some(am) = req.automod_rules {
                                *st.comm_automod_rules.lock().unwrap() = am;
                            }
                            // Capture pre-orthogonal DTO snapshot immediately after commit, THEN sleep!
                            let snap = st.build_community(&st.slug);
                            let post_delay = st.automod_post_delay_ms.swap(0, Ordering::SeqCst);
                            if post_delay > 0 {
                                tokio::time::sleep(Duration::from_millis(post_delay)).await;
                            }
                            st.automod_patch_completed.fetch_add(1, Ordering::SeqCst);
                            ("200 OK", serde_json::to_string(&snap).unwrap())
                        }
                    } else {
                        st.rules_patch_received.fetch_add(1, Ordering::SeqCst);
                        if let Some(rules) = req.rules {
                            *st.comm_rules.lock().unwrap() = rules;
                        }
                        // Capture pre-orthogonal DTO snapshot immediately after commit, THEN sleep!
                        let snap = st.build_community(&st.slug);
                        let post_delay = st.rules_post_delay_ms.swap(0, Ordering::SeqCst);
                        if post_delay > 0 {
                            tokio::time::sleep(Duration::from_millis(post_delay)).await;
                        }
                        st.rules_patch_completed.fetch_add(1, Ordering::SeqCst);
                        ("200 OK", serde_json::to_string(&snap).unwrap())
                    }
                } else if method == "PUT" && path == format!("/api/v1/community/communities/{}/wiki", st.slug) {
                    let req: UpdateCommunityWikiRequest = serde_json::from_slice(body_bytes).expect("wiki");
                    st.wiki_put_received.fetch_add(1, Ordering::SeqCst);
                    *st.comm_wiki.lock().unwrap() = req.markdown.clone();
                    let rev_num = st.comm_wiki_rev.fetch_add(1, Ordering::SeqCst) + 1;
                    let rev = CommunityWikiRevisionDto {
                        revision: rev_num,
                        editor_actor_id: viewer.clone(),
                        editor_handle: "r10_alice".to_string(),
                        markdown: req.markdown,
                        summary: req.summary,
                        created_at_ms: 1_750_000_005_000,
                    };
                    let post_delay = st.wiki_post_delay_ms.swap(0, Ordering::SeqCst);
                    if post_delay > 0 {
                        tokio::time::sleep(Duration::from_millis(post_delay)).await;
                    }
                    st.wiki_put_completed.fetch_add(1, Ordering::SeqCst);
                    ("200 OK", serde_json::to_string(&rev).unwrap())
                } else if (method == "POST" || method == "DELETE")
                    && path == format!("/api/v1/community/communities/{}/join", st.slug)
                {
                    let delay = st.join_delay_ms.swap(0, Ordering::SeqCst);
                    if delay > 0 {
                        tokio::time::sleep(Duration::from_millis(delay)).await;
                    }
                    let want = method == "POST";
                    let prev = st.comm_joined.swap(want, Ordering::SeqCst);
                    if !prev && want {
                        st.comm_members.fetch_add(1, Ordering::SeqCst);
                    } else if prev && !want {
                        st.comm_members.fetch_sub(1, Ordering::SeqCst);
                    }
                    st.join_completed.fetch_add(1, Ordering::SeqCst);
                    let resp = JoinCommunityResponse {
                        community_slug: st.slug.clone(),
                        is_joined: want,
                        member_count: st.comm_members.load(Ordering::SeqCst),
                        viewer_role: if want {
                            Some(CommunityMemberRole::Owner)
                        } else {
                            None
                        },
                    };
                    ("200 OK", serde_json::to_string(&resp).unwrap())
                } else if method == "GET" && path == format!("/api/v1/community/communities/{}/mod-queue", st.slug) {
                    ("200 OK", serde_json::to_string(&ModQueueResponse::default()).unwrap())
                } else if method == "GET" && path == format!("/api/v1/community/communities/{}/mod-logs", st.slug) {
                    ("200 OK", serde_json::to_string(&ModLogsResponse::default()).unwrap())
                } else if method == "GET" && path == "/api/v1/community/feed" {
                    let resp = FeedResponseDto {
                        items: vec![
                            FeedItemDto {
                                post: st.build_post(&st.p_edit),
                                rank_score: 100.0,
                                explanation: None,
                                is_exploration_slot: false,
                            },
                            FeedItemDto {
                                post: st.build_post(&st.p_part2),
                                rank_score: 90.0,
                                explanation: None,
                                is_exploration_slot: false,
                            },
                        ],
                        next_cursor: None,
                        ..Default::default()
                    };
                    ("200 OK", serde_json::to_string(&resp).unwrap())
                } else if method == "GET" && path.starts_with("/api/v1/community/posts/") {
                    let pid = path.trim_start_matches("/api/v1/community/posts/").to_string();
                    st.detail_get_received.fetch_add(1, Ordering::SeqCst);
                    let resp = if pid == st.p_part2 {
                        PostDetailResponse {
                            post: st.build_post(&st.p_part2),
                            thread_chain: vec![st.build_post(&st.p_part1), st.build_post(&st.p_part2)],
                            quote_chain: Vec::new(),
                            comments: vec![st.build_comment(&st.c1_id, &st.p_part2)],
                            total_comments: 1,
                        }
                    } else {
                        PostDetailResponse {
                            post: st.build_post(&pid),
                            thread_chain: Vec::new(),
                            quote_chain: Vec::new(),
                            comments: vec![
                                st.build_comment(&st.c1_id, &pid),
                                st.build_comment(&st.c2_id, &pid),
                            ],
                            total_comments: 2,
                        }
                    };
                    let delay = st.detail_get_delay_ms.swap(0, Ordering::SeqCst);
                    if delay > 0 {
                        tokio::time::sleep(Duration::from_millis(delay)).await;
                    }
                    st.detail_get_completed.fetch_add(1, Ordering::SeqCst);
                    ("200 OK", serde_json::to_string(&resp).unwrap())
                } else if method == "PATCH" && path.starts_with("/api/v1/community/posts/") {
                    let pid = path.trim_start_matches("/api/v1/community/posts/").to_string();
                    let req: UpdatePostRequest = serde_json::from_slice(body_bytes).expect("up");
                    st.post_patch_received.fetch_add(1, Ordering::SeqCst);
                    if let Some(t) = req.title {
                        st.post_titles.lock().unwrap().insert(pid.clone(), Some(t));
                    }
                    if let Some(b) = req.body {
                        st.post_bodies.lock().unwrap().insert(pid.clone(), b);
                    }
                    if let Some(f) = req.flair {
                        st.post_flairs.lock().unwrap().insert(pid.clone(), Some(f));
                    }
                    if let Some(n) = req.is_nsfw {
                        st.post_nsfw.lock().unwrap().insert(pid.clone(), n);
                    }
                    if let Some(sp) = req.is_spoiler {
                        st.post_spoiler.lock().unwrap().insert(pid.clone(), sp);
                    }
                    st.post_updated_ts.fetch_add(500, Ordering::SeqCst);
                    // Capture pre-orthogonal snapshot immediately, THEN sleep!
                    let snap = st.build_post(&pid);
                    let post_delay = st.post_patch_post_delay_ms.swap(0, Ordering::SeqCst);
                    if post_delay > 0 {
                        tokio::time::sleep(Duration::from_millis(post_delay)).await;
                    }
                    st.post_patch_completed.fetch_add(1, Ordering::SeqCst);
                    ("200 OK", serde_json::to_string(&snap).unwrap())
                } else if method == "POST" && path.ends_with("/vote") && path.contains("/posts/") {
                    let pid = path
                        .trim_start_matches("/api/v1/community/posts/")
                        .trim_end_matches("/vote")
                        .to_string();
                    let req: VoteRequest = serde_json::from_slice(body_bytes).expect("vr");
                    if req.direction == 0 {
                        st.post_votes.lock().unwrap().remove(&pid);
                    } else {
                        st.post_votes.lock().unwrap().insert(pid.clone(), req.direction);
                    }
                    let p = st.build_post(&pid);
                    st.post_vote_completed.fetch_add(1, Ordering::SeqCst);
                    let resp = VoteResponse {
                        target_id: pid,
                        direction: p.viewer_vote,
                        upvotes: p.upvotes,
                        downvotes: p.downvotes,
                        score: p.score,
                    };
                    ("200 OK", serde_json::to_string(&resp).unwrap())
                } else if method == "POST" && path.ends_with("/like") && path.contains("/posts/") {
                    let pid = path
                        .trim_start_matches("/api/v1/community/posts/")
                        .trim_end_matches("/like")
                        .to_string();
                    let active = {
                        let mut g = st.post_likes.lock().unwrap();
                        if !g.remove(&pid) {
                            g.insert(pid.clone());
                            true
                        } else {
                            false
                        }
                    };
                    st.post_like_completed.fetch_add(1, Ordering::SeqCst);
                    let resp = ToggleActionResponse {
                        target_id: pid,
                        active,
                        count: u64::from(active),
                    };
                    ("200 OK", serde_json::to_string(&resp).unwrap())
                } else if method == "POST" && path.ends_with("/repost") && path.contains("/posts/") {
                    let pid = path
                        .trim_start_matches("/api/v1/community/posts/")
                        .trim_end_matches("/repost")
                        .to_string();
                    let active = {
                        let mut g = st.post_reposts.lock().unwrap();
                        if !g.remove(&pid) {
                            g.insert(pid.clone());
                            true
                        } else {
                            false
                        }
                    };
                    st.post_repost_completed.fetch_add(1, Ordering::SeqCst);
                    let resp = RepostResponse {
                        post_id: pid,
                        reposted: active,
                        reposts_count: u64::from(active),
                        repost_post_id: None,
                    };
                    ("200 OK", serde_json::to_string(&resp).unwrap())
                } else if method == "POST" && path.ends_with("/bookmark") && path.contains("/posts/") {
                    let pid = path
                        .trim_start_matches("/api/v1/community/posts/")
                        .trim_end_matches("/bookmark")
                        .to_string();
                    let req: BookmarkRequest = serde_json::from_slice(body_bytes).expect("bm");
                    let active = {
                        let mut g = st.post_bookmarked.lock().unwrap();
                        if req.remove == Some(true) {
                            g.remove(&pid);
                            false
                        } else if req.remove == Some(false) {
                            g.insert(pid.clone());
                            true
                        } else if !g.remove(&pid) {
                            g.insert(pid.clone());
                            true
                        } else {
                            false
                        }
                    };
                    st.post_bookmark_completed.fetch_add(1, Ordering::SeqCst);
                    let resp = BookmarkResponse {
                        post_id: pid,
                        bookmarked: active,
                        folder: if active {
                            req.folder.unwrap_or_else(|| "All".to_string())
                        } else {
                            String::new()
                        },
                        bookmarks_count: u64::from(active),
                    };
                    ("200 OK", serde_json::to_string(&resp).unwrap())
                } else if method == "POST" && path.ends_with("/poll-vote") && path.contains("/posts/") {
                    let pid = path
                        .trim_start_matches("/api/v1/community/posts/")
                        .trim_end_matches("/poll-vote")
                        .to_string();
                    let req: CommunityPollVoteRequest = serde_json::from_slice(body_bytes).expect("pv");
                    *st.poll_voted_opt.lock().unwrap() = req.option_ids;
                    let poll = st.build_post(&pid).poll.expect("poll");
                    st.poll_vote_completed.fetch_add(1, Ordering::SeqCst);
                    ("200 OK", serde_json::to_string(&poll).unwrap())
                } else if method == "POST" && path.ends_with("/award") && path.contains("/posts/") {
                    let pid = path
                        .trim_start_matches("/api/v1/community/posts/")
                        .trim_end_matches("/award")
                        .to_string();
                    let req: GiveAwardRequest = serde_json::from_slice(body_bytes).expect("aw");
                    let award = PostAwardDto {
                        award_id: "aw_r10_1".to_string(),
                        award_kind: req.award_kind,
                        icon_emoji: "🏆".to_string(),
                        giver_actor_id: viewer.clone(),
                        giver_handle: "r10_alice".to_string(),
                        message: req.message,
                        created_at_ms: 1_750_000_010_000,
                    };
                    let awards = {
                        let mut g = st.post_awards.lock().unwrap();
                        g.push(award);
                        g.clone()
                    };
                    st.award_completed.fetch_add(1, Ordering::SeqCst);
                    let resp = AwardResponse {
                        target_id: pid,
                        awards,
                    };
                    ("200 OK", serde_json::to_string(&resp).unwrap())
                } else if method == "PATCH" && path.starts_with("/api/v1/community/comments/") {
                    let cid = path.trim_start_matches("/api/v1/community/comments/").to_string();
                    let req: UpdateCommentRequest = serde_json::from_slice(body_bytes).expect("ucm");
                    st.comment_patch_received.fetch_add(1, Ordering::SeqCst);
                    let delay = st
                        .comment_patch_delays_by_body
                        .lock()
                        .unwrap()
                        .remove(&req.body)
                        .unwrap_or(0);
                    st.comment_bodies.lock().unwrap().insert(cid.clone(), req.body);
                    let snap = st.build_comment(&cid, &st.p_edit);
                    if delay > 0 {
                        tokio::time::sleep(Duration::from_millis(delay)).await;
                    }
                    st.comment_patch_completed.fetch_add(1, Ordering::SeqCst);
                    ("200 OK", serde_json::to_string(&snap).unwrap())
                } else if method == "POST" && path.ends_with("/vote") && path.contains("/comments/") {
                    let cid = path
                        .trim_start_matches("/api/v1/community/comments/")
                        .trim_end_matches("/vote")
                        .to_string();
                    let req: VoteRequest = serde_json::from_slice(body_bytes).expect("cvr");
                    if req.direction == 0 {
                        st.comment_votes.lock().unwrap().remove(&cid);
                    } else {
                        st.comment_votes.lock().unwrap().insert(cid.clone(), req.direction);
                    }
                    let c = st.build_comment(&cid, &st.p_part2);
                    st.comment_vote_completed.fetch_add(1, Ordering::SeqCst);
                    let resp = VoteResponse {
                        target_id: cid,
                        direction: c.viewer_vote,
                        upvotes: c.upvotes,
                        downvotes: c.downvotes,
                        score: c.score,
                    };
                    ("200 OK", serde_json::to_string(&resp).unwrap())
                } else if method == "GET" && path.starts_with("/api/v1/community/actors/") {
                    let key = path.trim_start_matches("/api/v1/community/actors/");
                    let aid = if key == "r10_bob" || key == st.bob_id {
                        st.bob_id.clone()
                    } else if key == "r10_bot_a" || key == st.bot_a_id {
                        st.bot_a_id.clone()
                    } else {
                        st.alice_id.clone()
                    };
                    st.profile_get_completed.fetch_add(1, Ordering::SeqCst);
                    let resp = ActorProfileResponse {
                        actor: st.build_actor(&aid),
                        recent_posts: vec![st.build_post(&st.p_edit)],
                        recent_comments: vec![st.build_comment(&st.c1_id, &st.p_part2)],
                        moderated_communities: Vec::new(),
                    };
                    ("200 OK", serde_json::to_string(&resp).unwrap())
                } else if method == "PATCH" && path.starts_with("/api/v1/community/actors/") {
                    let aid = path.trim_start_matches("/api/v1/community/actors/").to_string();
                    let req: UpdateCommunityActorRequest = serde_json::from_slice(body_bytes).expect("ua");
                    st.actor_patch_received.fetch_add(1, Ordering::SeqCst);
                    let post_delay = req
                        .display_name
                        .as_deref()
                        .and_then(|n| st.actor_patch_post_delays_by_name.lock().unwrap().remove(n))
                        .unwrap_or(0);
                    if aid == st.bot_a_id {
                        if let Some(name) = req.display_name {
                            *st.bot_a_display_name.lock().unwrap() = name;
                        }
                    } else if aid == st.alice_id {
                        if let Some(name) = req.display_name {
                            *st.alice_display_name.lock().unwrap() = name;
                        }
                    }
                    // Mirror real server `update_actor_tx` (`store/actors.rs:444`): returns
                    // viewer-unhydrated `is_following=false, is_muted=false, is_blocked=false`!
                    let mut snap = st.build_actor(&aid);
                    snap.is_following = false;
                    snap.is_muted = false;
                    snap.is_blocked = false;
                    if post_delay > 0 {
                        tokio::time::sleep(Duration::from_millis(post_delay)).await;
                    }
                    st.actor_patch_completed.fetch_add(1, Ordering::SeqCst);
                    ("200 OK", serde_json::to_string(&snap).unwrap())
                } else if (method == "POST" || method == "DELETE")
                    && path.starts_with("/api/v1/community/actors/")
                    && path.ends_with("/follow")
                {
                    let target_id = path
                        .trim_start_matches("/api/v1/community/actors/")
                        .trim_end_matches("/follow")
                        .to_string();
                    let is_bob = target_id == st.bob_id;
                    if is_bob {
                        st.follow_calls.fetch_add(1, Ordering::SeqCst);
                        let cur = st.follow_block_in_flight.fetch_add(1, Ordering::SeqCst) + 1;
                        st.follow_block_max_in_flight.fetch_max(cur, Ordering::SeqCst);
                    }
                    let delay = st.follow_pre_delay_ms.swap(0, Ordering::SeqCst);
                    if delay > 0 {
                        tokio::time::sleep(Duration::from_millis(delay)).await;
                    }
                    let res = if st.follow_fail_500.load(Ordering::SeqCst) {
                        st.follow_completed.fetch_add(1, Ordering::SeqCst);
                        ("500 Internal Server Error", r#"{"error":"follow_500"}"#.to_string())
                    } else if is_bob && method == "POST" && st.bob_blocked.load(Ordering::SeqCst) {
                        st.follow_403_count.fetch_add(1, Ordering::SeqCst);
                        st.follow_completed.fetch_add(1, Ordering::SeqCst);
                        ("403 Forbidden", r#"{"error":"blocked"}"#.to_string())
                    } else {
                        let want = method == "POST";
                        let (fl, cnt) = if is_bob {
                            let prev = st.bob_following.swap(want, Ordering::SeqCst);
                            if !prev && want {
                                st.bob_followers.fetch_add(1, Ordering::SeqCst);
                            } else if prev && !want {
                                st.bob_followers.fetch_sub(1, Ordering::SeqCst);
                            }
                            (want, st.bob_followers.load(Ordering::SeqCst))
                        } else {
                            let prev = st.bot_a_following.swap(want, Ordering::SeqCst);
                            if !prev && want {
                                st.bot_a_followers.fetch_add(1, Ordering::SeqCst);
                            } else if prev && !want {
                                st.bot_a_followers.fetch_sub(1, Ordering::SeqCst);
                            }
                            (want, st.bot_a_followers.load(Ordering::SeqCst))
                        };
                        st.follow_completed.fetch_add(1, Ordering::SeqCst);
                        let resp = FollowActionResponse {
                            actor_id: target_id,
                            is_following: fl,
                            followers_count: cnt,
                        };
                        ("200 OK", serde_json::to_string(&resp).unwrap())
                    };
                    if is_bob {
                        st.follow_block_in_flight.fetch_sub(1, Ordering::SeqCst);
                    }
                    res
                } else if (method == "POST" || method == "DELETE")
                    && path == "/api/v1/community/mutes-blocks"
                {
                    let req: MuteBlockRequest = serde_json::from_slice(body_bytes).expect("mb");
                    let want = method == "POST";
                    if req.action == MuteBlockActionKind::Mute {
                        if req.target_value == st.bob_id {
                            st.bob_muted.store(want, Ordering::SeqCst);
                        } else if req.target_value == st.bot_a_id {
                            st.bot_a_muted.store(want, Ordering::SeqCst);
                        }
                        st.mute_completed.fetch_add(1, Ordering::SeqCst);
                        if want {
                            let dto = CommunityMuteBlockDto {
                                id: "mb_mute_r10".to_string(),
                                owner_actor_id: viewer,
                                target_kind: MuteBlockTargetKind::Actor,
                                target_value: req.target_value,
                                action: req.action,
                                reason: None,
                                created_at_ms: 1_750_000_000_000,
                            };
                            ("200 OK", serde_json::to_string(&dto).unwrap())
                        } else {
                            (
                                "200 OK",
                                serde_json::to_string(&DeleteEntityResponse {
                                    id: req.target_value,
                                    deleted: true,
                                })
                                .unwrap(),
                            )
                        }
                    } else {
                        st.block_calls.fetch_add(1, Ordering::SeqCst);
                        st.block_received.fetch_add(1, Ordering::SeqCst);
                        let cur = st.follow_block_in_flight.fetch_add(1, Ordering::SeqCst) + 1;
                        st.follow_block_max_in_flight.fetch_max(cur, Ordering::SeqCst);
                        let pre = st.block_pre_delay_ms.swap(0, Ordering::SeqCst);
                        if pre > 0 {
                            tokio::time::sleep(Duration::from_millis(pre)).await;
                        }
                        let res = if st.block_fail_500.load(Ordering::SeqCst) {
                            st.block_completed.fetch_add(1, Ordering::SeqCst);
                            ("500 Internal Server Error", r#"{"error":"block_500"}"#.to_string())
                        } else {
                            if want {
                                st.bob_blocked.store(true, Ordering::SeqCst);
                                if st.bob_following.swap(false, Ordering::SeqCst) {
                                    st.bob_followers.fetch_sub(1, Ordering::SeqCst);
                                }
                            } else {
                                st.bob_blocked.store(false, Ordering::SeqCst);
                            }
                            let post = st.block_post_delay_ms.swap(0, Ordering::SeqCst);
                            if post > 0 {
                                tokio::time::sleep(Duration::from_millis(post)).await;
                            }
                            st.block_completed.fetch_add(1, Ordering::SeqCst);
                            if want {
                                let dto = CommunityMuteBlockDto {
                                    id: "mb_blk_r10".to_string(),
                                    owner_actor_id: viewer,
                                    target_kind: MuteBlockTargetKind::Actor,
                                    target_value: req.target_value,
                                    action: req.action,
                                    reason: None,
                                    created_at_ms: 1_750_000_000_000,
                                };
                                ("200 OK", serde_json::to_string(&dto).unwrap())
                            } else {
                                (
                                    "200 OK",
                                    serde_json::to_string(&DeleteEntityResponse {
                                        id: req.target_value,
                                        deleted: true,
                                    })
                                    .unwrap(),
                                )
                            }
                        };
                        st.follow_block_in_flight.fetch_sub(1, Ordering::SeqCst);
                        res
                    }
                } else if method == "GET" && path == "/api/v1/community/lists" {
                    let resp = CommunityListsResponse {
                        lists: vec![st.build_list()],
                    };
                    st.lists_get_completed.fetch_add(1, Ordering::SeqCst);
                    ("200 OK", serde_json::to_string(&resp).unwrap())
                } else if method == "PATCH" && path.starts_with("/api/v1/community/lists/") {
                    let req: UpdateCommunityListRequest = serde_json::from_slice(body_bytes).expect("ul");
                    st.list_toggle_received.fetch_add(1, Ordering::SeqCst);
                    if let Some(p) = req.is_pinned {
                        st.list_pinned.store(p, Ordering::SeqCst);
                    }
                    st.list_updated_ts.fetch_add(10, Ordering::SeqCst);
                    // Capture stale-name snapshot BEFORE post-commit delay!
                    let snap = st.build_list();
                    let post = st.list_toggle_post_delay_ms.swap(0, Ordering::SeqCst);
                    if post > 0 {
                        tokio::time::sleep(Duration::from_millis(post)).await;
                    }
                    st.list_toggle_completed.fetch_add(1, Ordering::SeqCst);
                    ("200 OK", serde_json::to_string(&snap).unwrap())
                } else {
                    ("200 OK", "{}".to_string())
                };

                let http_resp = format!(
                    "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{resp_body}",
                    resp_body.len()
                );
                let _ = sock.write_all(http_resp.as_bytes()).await;
                let _ = sock.shutdown().await;
            });
        }
    });

    (addr, state, AbortOnDrop(join))
}

#[test]
fn test_m5_r10_field_scoped_patches_and_symmetric_serialization() {
    run_in_headless_subprocess("test_m5_r10_field_scoped_patches_and_symmetric_serialization");
}

async fn body_r10_field_scoped_patches_and_symmetric_serialization(mut app: AsyncApp) {
    let dir = TempDir::new().expect("r10 tempdir");
    let rt_dir = TempDir::new().expect("r10 rt tempdir");
    let rt_holder = TelegramClient::open_with_dir(
        ClientConfig {
            server_url: "http://127.0.0.1:9".to_string(),
            max_reconnect_attempts: 0,
            ..Default::default()
        },
        rt_dir.path(),
    )
    .expect("open r10 rt holder");
    let (addr, srv, _server_guard) = spawn_r10_server(rt_holder.runtime().handle());

    let client = Arc::new(
        TelegramClient::open_with_dir(
            ClientConfig {
                server_url: format!("http://{addr}"),
                max_reconnect_attempts: 0,
                ..Default::default()
            },
            dir.path(),
        )
        .expect("open r10 client"),
    );
    let auth = test_auth(910001, "r10_alice");
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
        .expect("mount r10 AppShell");
    let _ = shell.update(&mut app, |s, cx| s.open_community_workspace(cx));
    let cw = shell
        .read_with(&app, |s, _| s.community_workspace.clone().unwrap())
        .unwrap();

    assert!(
        pump_until(&mut app, Duration::from_secs(5), |cx| {
            let st = &cw.read(cx).state;
            !st.feed_loading
                && !st.communities_loading
                && !st.recommended_actors_loading
                && st.identities.is_some()
                && !st.communities.is_empty()
                && !st.recommended_actors.is_empty()
                && st.feed_items.iter().any(|it| it.post.post_id == srv.p_edit)
        })
        .await,
        "R10 workspace must bootstrap cleanly"
    );

    run_r10_t01_to_t04(&mut app, &cw, &srv).await;
    run_r10_t05_to_t08(&mut app, &cw, &srv).await;

    drop(rt_holder);
}

async fn run_r10_t01_to_t04(
    app: &mut AsyncApp,
    cw: &Entity<CommunityWorkspace>,
    srv: &Arc<R10MockState>,
) {
    use std::sync::atomic::Ordering;

    // Seed all 4 community caches + all 5 actor caches + post caches via real routes
    let _ = cw.update(app, |w, cx| {
        w.execute_search(String::new(), CommunitySearchTab::Top, cx);
        w.load_community_page(srv.slug.clone(), cx);
        w.load_mod_queue(srv.slug.clone(), cx);
    });
    assert!(
        pump_until(app, Duration::from_secs(4), |cx| {
            let st = &cw.read(cx).state;
            !st.search_loading
                && !st.community_loading
                && !st.mod_loading
                && st.search_result.is_some()
                && st.community_detail.is_some()
                && st.mod_community.is_some()
        })
        .await,
        "R10 setup: search, community_detail, and mod_community must hydrate"
    );

    // ------------------------------------------------------------------
    // T10-01 (R9-ADV-NEW-1 / T10-DE4-1): Slow post-commit save_community_rules (320ms)
    // vs fast SETTLED toggle_automod_rule_enabled + save_community_wiki + toggle_join(false)
    // ------------------------------------------------------------------
    let rp_rx = srv.rules_patch_received.load(Ordering::SeqCst);
    let rp_done = srv.rules_patch_completed.load(Ordering::SeqCst);
    let am_done = srv.automod_patch_completed.load(Ordering::SeqCst);
    let wk_done = srv.wiki_put_completed.load(Ordering::SeqCst);
    let jn_done = srv.join_completed.load(Ordering::SeqCst);
    srv.rules_post_delay_ms.store(320, Ordering::SeqCst);
    let _ = cw.update(app, |w, cx| {
        w.state.community_new_rule_title = "Rule Two".to_string();
        w.add_community_rule(srv.slug.clone(), cx);
    });
    assert!(
        pump_until(app, Duration::from_secs(4), |_| {
            srv.rules_patch_received.load(Ordering::SeqCst) > rp_rx
        })
        .await,
        "T10-01: rules PATCH must commit and enter post-commit delay"
    );
    let _ = cw.update(app, |w, cx| {
        assert!(w.state.community_rules_saving);
        w.toggle_automod_rule_enabled("am_r10_1".to_string(), cx);
        w.state.community_wiki_editing = true;
        w.state.community_wiki_draft = "Wiki Settled T10-01".to_string();
        w.save_community_wiki(srv.slug.clone(), cx);
    });
    assert!(
        pump_until(app, Duration::from_secs(4), |cx| {
            let st = &cw.read(cx).state;
            st.automod_in_flight.is_empty()
                && !st.community_wiki_saving
                && srv.automod_patch_completed.load(Ordering::SeqCst) > am_done
                && srv.wiki_put_completed.load(Ordering::SeqCst) > wk_done
        })
        .await,
        "T10-01: orthogonal AutoMod + Wiki must settle while rules PATCH is still in flight"
    );
    let _ = cw.update(app, |w, cx| {
        w.toggle_join_community(srv.slug.clone(), false, cx);
    });
    assert!(
        pump_until(app, Duration::from_secs(4), |cx| {
            cw.read(cx).state.community_join_in_flight.is_empty()
                && srv.join_completed.load(Ordering::SeqCst) > jn_done
        })
        .await,
        "T10-01: orthogonal Leave must settle while rules PATCH is still in flight"
    );
    assert!(
        pump_until(app, Duration::from_secs(5), |cx| {
            !cw.read(cx).state.community_rules_saving
                && srv.rules_patch_completed.load(Ordering::SeqCst) > rp_done
        })
        .await,
        "T10-01: slow rules PATCH must settle"
    );
    let _ = cw.read_with(app, |w, _| {
        let caches = [
            ("communities", w.state.communities.iter().find(|c| c.slug == srv.slug).unwrap()),
            ("community_detail", &w.state.community_detail.as_ref().unwrap().community),
            ("mod_community", w.state.mod_community.as_ref().unwrap()),
            (
                "search_result",
                w.state
                    .search_result
                    .as_ref()
                    .unwrap()
                    .communities
                    .iter()
                    .find(|c| c.slug == srv.slug)
                    .unwrap(),
            ),
        ];
        for (name, c) in caches {
            assert_eq!(c.rules.len(), 2, "T10-01 ({name}): new rule missing");
            assert!(
                !c.automod_rules[0].enabled,
                "T10-01 ({name}): settled AutoMod toggle clobbered by save_community_rules!"
            );
            assert!(
                !c.is_joined && c.member_count == 9 && c.viewer_role.is_none(),
                "T10-01 ({name}): settled Leave clobbered by save_community_rules!"
            );
            assert_eq!(
                c.wiki_markdown, "Wiki Settled T10-01",
                "T10-01 ({name}): settled Wiki clobbered by save_community_rules!"
            );
        }
    });

    // Re-join as Owner before T10-02
    let jn_rejoin = srv.join_completed.load(Ordering::SeqCst);
    let _ = cw.update(app, |w, cx| {
        w.toggle_join_community(srv.slug.clone(), true, cx);
    });
    assert!(
        pump_until(app, Duration::from_secs(4), |cx| {
            cw.read(cx).state.community_join_in_flight.is_empty()
                && srv.join_completed.load(Ordering::SeqCst) > jn_rejoin
        })
        .await
    );

    // ------------------------------------------------------------------
    // T10-02 (R9-ADV-NEW-2 / T10-DE4-2): Slow post-commit flush_automod_rules_sync (320ms)
    // vs fast SETTLED add_community_rule + save_community_wiki + toggle_join(false),
    // PLUS HTTP 500 rollback preserving settled orthogonal fields across all 4 caches!
    // ------------------------------------------------------------------
    let am_rx2 = srv.automod_patch_received.load(Ordering::SeqCst);
    let am_done2 = srv.automod_patch_completed.load(Ordering::SeqCst);
    let rp_done2 = srv.rules_patch_completed.load(Ordering::SeqCst);
    let wk_done2 = srv.wiki_put_completed.load(Ordering::SeqCst);
    let jn_done2 = srv.join_completed.load(Ordering::SeqCst);
    srv.automod_post_delay_ms.store(320, Ordering::SeqCst);
    let _ = cw.update(app, |w, cx| {
        w.toggle_automod_rule_enabled("am_r10_1".to_string(), cx);
    });
    assert!(
        pump_until(app, Duration::from_secs(4), |_| {
            srv.automod_patch_received.load(Ordering::SeqCst) > am_rx2
        })
        .await
    );
    let _ = cw.update(app, |w, cx| {
        assert!(!w.state.automod_in_flight.is_empty());
        w.state.community_new_rule_title = "Rule Three".to_string();
        w.add_community_rule(srv.slug.clone(), cx);
        w.state.community_wiki_editing = true;
        w.state.community_wiki_draft = "Wiki Settled T10-02".to_string();
        w.save_community_wiki(srv.slug.clone(), cx);
    });
    assert!(
        pump_until(app, Duration::from_secs(4), |cx| {
            let st = &cw.read(cx).state;
            !st.community_rules_saving
                && !st.community_wiki_saving
                && srv.rules_patch_completed.load(Ordering::SeqCst) > rp_done2
                && srv.wiki_put_completed.load(Ordering::SeqCst) > wk_done2
        })
        .await
    );
    let _ = cw.update(app, |w, cx| {
        w.toggle_join_community(srv.slug.clone(), false, cx);
    });
    assert!(
        pump_until(app, Duration::from_secs(4), |cx| {
            cw.read(cx).state.community_join_in_flight.is_empty()
                && srv.join_completed.load(Ordering::SeqCst) > jn_done2
        })
        .await
    );
    assert!(
        pump_until(app, Duration::from_secs(5), |cx| {
            cw.read(cx).state.automod_in_flight.is_empty()
                && srv.automod_patch_completed.load(Ordering::SeqCst) > am_done2
        })
        .await
    );
    let _ = cw.read_with(app, |w, _| {
        let caches = [
            ("communities", w.state.communities.iter().find(|c| c.slug == srv.slug).unwrap()),
            ("community_detail", &w.state.community_detail.as_ref().unwrap().community),
            ("mod_community", w.state.mod_community.as_ref().unwrap()),
            (
                "search_result",
                w.state
                    .search_result
                    .as_ref()
                    .unwrap()
                    .communities
                    .iter()
                    .find(|c| c.slug == srv.slug)
                    .unwrap(),
            ),
        ];
        for (name, c) in caches {
            assert!(c.automod_rules[0].enabled, "T10-02 ({name}): AutoMod toggle must apply");
            assert_eq!(
                c.rules.len(),
                3,
                "T10-02 ({name}): settled Rule Three clobbered by flush_automod_rules_sync!"
            );
            assert!(
                !c.is_joined && c.member_count == 9 && c.viewer_role.is_none(),
                "T10-02 ({name}): settled Leave clobbered by flush_automod_rules_sync!"
            );
            assert_eq!(
                c.wiki_markdown, "Wiki Settled T10-02",
                "T10-02 ({name}): settled Wiki clobbered by flush_automod_rules_sync!"
            );
        }
    });

    // T10-02 Rollback (HTTP 500): rolls back automod_rules across all 4 caches (incl. search_result)
    // while preserving settled rules & is_joined!
    let jn_rejoin2 = srv.join_completed.load(Ordering::SeqCst);
    let _ = cw.update(app, |w, cx| {
        w.toggle_join_community(srv.slug.clone(), true, cx);
    });
    assert!(
        pump_until(app, Duration::from_secs(4), |cx| {
            cw.read(cx).state.community_join_in_flight.is_empty()
                && srv.join_completed.load(Ordering::SeqCst) > jn_rejoin2
        })
        .await
    );
    let am_rx_500 = srv.automod_patch_received.load(Ordering::SeqCst);
    let am_done_500 = srv.automod_patch_completed.load(Ordering::SeqCst);
    let rp_done_500 = srv.rules_patch_completed.load(Ordering::SeqCst);
    srv.automod_fail_500.store(true, Ordering::SeqCst);
    srv.automod_post_delay_ms.store(260, Ordering::SeqCst);
    let _ = cw.update(app, |w, cx| {
        w.toggle_automod_rule_enabled("am_r10_1".to_string(), cx);
    });
    assert!(
        pump_until(app, Duration::from_secs(4), |_| {
            srv.automod_patch_received.load(Ordering::SeqCst) > am_rx_500
        })
        .await
    );
    let _ = cw.update(app, |w, cx| {
        w.state.community_new_rule_title = "Rule Four".to_string();
        w.add_community_rule(srv.slug.clone(), cx);
    });
    assert!(
        pump_until(app, Duration::from_secs(4), |cx| {
            !cw.read(cx).state.community_rules_saving
                && srv.rules_patch_completed.load(Ordering::SeqCst) > rp_done_500
        })
        .await
    );
    assert!(
        pump_until(app, Duration::from_secs(5), |cx| {
            cw.read(cx).state.automod_in_flight.is_empty()
                && srv.automod_patch_completed.load(Ordering::SeqCst) > am_done_500
        })
        .await
    );
    srv.automod_fail_500.store(false, Ordering::SeqCst);
    let _ = cw.read_with(app, |w, _| {
        let sr_c = w
            .state
            .search_result
            .as_ref()
            .unwrap()
            .communities
            .iter()
            .find(|c| c.slug == srv.slug)
            .unwrap();
        assert!(
            sr_c.automod_rules[0].enabled,
            "T10-02 rollback: search_result.communities automod_rules must roll back on 500"
        );
        assert_eq!(sr_c.rules.len(), 4, "T10-02 rollback: settled Rule Four must survive 500 rollback");
        assert!(sr_c.is_joined, "T10-02 rollback: settled is_joined must survive 500 rollback");
    });

    // ------------------------------------------------------------------
    // T10-03 (R9-ADV-NEW-3 / T10-DE4-3): Slow post-commit submit_composer Edit (320ms)
    // vs fast SETTLED vote + like + repost + bookmark + poll_vote + award
    // ------------------------------------------------------------------
    let _ = cw.update(app, |w, cx| {
        w.load_actor_profile("r10_alice".to_string(), cx);
        w.navigate_to(CommunityRoute::PostDetail(srv.p_edit.clone()), cx);
    });
    assert!(
        pump_until(app, Duration::from_secs(4), |cx| {
            let st = &cw.read(cx).state;
            !st.detail_loading
                && !st.profile_loading
                && st.detail_post.as_ref().is_some_and(|d| d.post.post_id == srv.p_edit)
                && st.actor_profile.is_some()
        })
        .await
    );
    let pp_rx = srv.post_patch_received.load(Ordering::SeqCst);
    let pp_done = srv.post_patch_completed.load(Ordering::SeqCst);
    let pv_b = srv.post_vote_completed.load(Ordering::SeqCst);
    let pl_b = srv.post_like_completed.load(Ordering::SeqCst);
    let pr_b = srv.post_repost_completed.load(Ordering::SeqCst);
    let pb_b = srv.post_bookmark_completed.load(Ordering::SeqCst);
    let po_b = srv.poll_vote_completed.load(Ordering::SeqCst);
    let aw_b = srv.award_completed.load(Ordering::SeqCst);
    srv.post_patch_post_delay_ms.store(320, Ordering::SeqCst);
    let _ = cw.update(app, |w, cx| {
        let post = w.state.detail_post.as_ref().unwrap().post.clone();
        w.open_composer_edit(post, cx);
        w.state.composer.title = "Edited Title R10".to_string();
        w.state.composer.body = "Edited Body R10".to_string();
        w.state.composer.flair = Some("Meta".to_string());
        w.state.composer.is_nsfw = true;
        w.state.composer.is_spoiler = true;
        w.submit_composer(PostStatus::Published, cx);
    });
    assert!(
        pump_until(app, Duration::from_secs(4), |_| {
            srv.post_patch_received.load(Ordering::SeqCst) > pp_rx
        })
        .await
    );
    let _ = cw.update(app, |w, cx| {
        assert!(w.state.composer.submitting);
        w.close_composer_modal(cx);
        w.vote_post(srv.p_edit.clone(), 1, cx);
        w.toggle_like(srv.p_edit.clone(), cx);
        w.repost_post(srv.p_edit.clone(), cx);
        w.toggle_bookmark(srv.p_edit.clone(), cx);
        w.vote_poll(srv.p_edit.clone(), "opt_1".to_string(), cx);
        w.give_award(srv.p_edit.clone(), "gold".to_string(), cx);
    });
    assert!(
        pump_until(app, Duration::from_secs(4), |cx| {
            let st = &cw.read(cx).state;
            st.post_vote_in_flight.is_empty()
                && st.post_like_in_flight.is_empty()
                && st.post_repost_in_flight.is_empty()
                && st.bookmark_in_flight.is_empty()
                && st.poll_vote_in_flight.is_empty()
                && srv.post_vote_completed.load(Ordering::SeqCst) > pv_b
                && srv.post_like_completed.load(Ordering::SeqCst) > pl_b
                && srv.post_repost_completed.load(Ordering::SeqCst) > pr_b
                && srv.post_bookmark_completed.load(Ordering::SeqCst) > pb_b
                && srv.poll_vote_completed.load(Ordering::SeqCst) > po_b
                && srv.award_completed.load(Ordering::SeqCst) > aw_b
        })
        .await,
        "T10-03: all 6 orthogonal engagement mutations must settle while Edit is in flight"
    );
    assert!(
        pump_until(app, Duration::from_secs(5), |cx| {
            !cw.read(cx).state.composer.submitting
                && !cw.read(cx).state.feed_loading
                && srv.post_patch_completed.load(Ordering::SeqCst) > pp_done
        })
        .await
    );
    let _ = cw.read_with(app, |w, _| {
        let posts = [
            ("detail_post", &w.state.detail_post.as_ref().unwrap().post),
            (
                "feed_items",
                &w.state
                    .feed_items
                    .iter()
                    .find(|i| i.post.post_id == srv.p_edit)
                    .unwrap()
                    .post,
            ),
            (
                "actor_profile.recent_posts",
                w.state
                    .actor_profile
                    .as_ref()
                    .unwrap()
                    .recent_posts
                    .iter()
                    .find(|p| p.post_id == srv.p_edit)
                    .unwrap(),
            ),
            (
                "search_result.posts",
                w.state
                    .search_result
                    .as_ref()
                    .unwrap()
                    .posts
                    .iter()
                    .find(|p| p.post_id == srv.p_edit)
                    .unwrap(),
            ),
        ];
        for (name, p) in posts {
            assert_eq!(p.title.as_deref(), Some("Edited Title R10"), "T10-03 ({name}) title");
            assert_eq!(p.body, "Edited Body R10", "T10-03 ({name}) body");
            assert_eq!(p.flair.as_deref(), Some("Meta"), "T10-03 ({name}) flair");
            assert!(p.is_nsfw && p.is_spoiler, "T10-03 ({name}) nsfw/spoiler");
            assert_eq!(p.viewer_vote, 1, "T10-03 ({name}): settled vote clobbered!");
            assert!(p.viewer_liked, "T10-03 ({name}): settled like clobbered!");
            assert!(p.viewer_reposted, "T10-03 ({name}): settled repost clobbered!");
            assert!(p.viewer_bookmarked, "T10-03 ({name}): settled bookmark clobbered!");
            assert_eq!(
                p.poll.as_ref().unwrap().viewer_voted_option_ids,
                vec!["opt_1".to_string()],
                "T10-03 ({name}): settled poll vote clobbered!"
            );
            assert_eq!(p.awards.len(), 1, "T10-03 ({name}): settled award clobbered!");
        }
    });

    // ------------------------------------------------------------------
    // T10-04 (R9-ADV-NEW-4 + R9-HUNT-4 + T10-DE4-4):
    // (4A/4B) Slow save_selected_bot_edits & submit_edit_profile vs settled Follow+Mute
    // (4C) Bidirectional cross-surface EditProfile <-> BotStudio race
    // (4D) Cross-actor concurrent edit (bot_a + human_alice BOTH survive)
    // ------------------------------------------------------------------
    let _ = cw.update(app, |w, cx| {
        w.navigate_to(CommunityRoute::ActorProfile(srv.bot_a_id.clone()), cx);
    });
    assert!(
        pump_until(app, Duration::from_secs(4), |cx| {
            !cw.read(cx).state.profile_loading
                && cw
                    .read(cx)
                    .state
                    .actor_profile
                    .as_ref()
                    .is_some_and(|p| p.actor.actor_id == srv.bot_a_id)
        })
        .await
    );

    // 4A: Slow save_selected_bot_edits (300ms post-commit returning unhydrated flags) vs settled Follow+Mute
    let ap_rx = srv.actor_patch_received.load(Ordering::SeqCst);
    let ap_done = srv.actor_patch_completed.load(Ordering::SeqCst);
    let fl_b = srv.follow_completed.load(Ordering::SeqCst);
    let mt_b = srv.mute_completed.load(Ordering::SeqCst);
    srv.actor_patch_post_delays_by_name
        .lock()
        .unwrap()
        .insert("Bot A Edited 4A".to_string(), 300);
    let _ = cw.update(app, |w, cx| {
        let bot_a = w.state.actor_profile.as_ref().unwrap().actor.clone();
        w.select_bot_for_studio(bot_a, cx);
        w.state.bot_studio_edit.display_name = "Bot A Edited 4A".to_string();
        w.save_selected_bot_edits(cx);
    });
    assert!(
        pump_until(app, Duration::from_secs(4), |_| {
            srv.actor_patch_received.load(Ordering::SeqCst) > ap_rx
        })
        .await
    );
    let _ = cw.update(app, |w, cx| {
        w.toggle_follow_actor(srv.bot_a_id.clone(), true, cx);
        w.toggle_mute_actor(srv.bot_a_id.clone(), false, cx);
    });
    assert!(
        pump_until(app, Duration::from_secs(4), |cx| {
            let st = &cw.read(cx).state;
            st.actor_follow_in_flight.is_empty()
                && st.actor_mute_in_flight.is_empty()
                && srv.follow_completed.load(Ordering::SeqCst) > fl_b
                && srv.mute_completed.load(Ordering::SeqCst) > mt_b
        })
        .await
    );
    assert!(
        pump_until(app, Duration::from_secs(5), |cx| {
            !cw.read(cx).state.bot_studio_edit.saving
                && srv.actor_patch_completed.load(Ordering::SeqCst) > ap_done
        })
        .await
    );
    let _ = cw.read_with(app, |w, _| {
        let caches = [
            ("actor_profile", &w.state.actor_profile.as_ref().unwrap().actor),
            (
                "recommended_actors",
                w.state
                    .recommended_actors
                    .iter()
                    .find(|a| a.actor_id == srv.bot_a_id)
                    .unwrap(),
            ),
            (
                "moderators",
                w.state
                    .community_detail
                    .as_ref()
                    .unwrap()
                    .moderators
                    .iter()
                    .find(|a| a.actor_id == srv.bot_a_id)
                    .unwrap(),
            ),
            (
                "search_result",
                w.state
                    .search_result
                    .as_ref()
                    .unwrap()
                    .actors
                    .iter()
                    .find(|a| a.actor_id == srv.bot_a_id)
                    .unwrap(),
            ),
        ];
        for (name, a) in caches {
            assert_eq!(a.display_name, "Bot A Edited 4A", "T10-04A ({name}) display_name");
            assert!(
                a.is_following && a.followers_count == 3 && a.is_muted,
                "T10-04A ({name}): settled Follow+Mute clobbered by save_selected_bot_edits! got {a:?}"
            );
        }
    });

    // 4B: Slow submit_edit_profile (300ms post-commit) preserves pre-existing Follow+Mute AND
    // direct local SQLite update_actor_tx verification
    let ap_rx_4b = srv.actor_patch_received.load(Ordering::SeqCst);
    let ap_done_4b = srv.actor_patch_completed.load(Ordering::SeqCst);
    srv.actor_patch_post_delays_by_name
        .lock()
        .unwrap()
        .insert("Bot A Edited 4B".to_string(), 240);
    let _ = cw.update(app, |w, cx| {
        let a = w.state.actor_profile.as_ref().unwrap().actor.clone();
        w.open_edit_profile_modal(a, cx);
        w.state.edit_profile.display_name = "Bot A Edited 4B".to_string();
        w.submit_edit_profile(cx);
    });
    assert!(
        pump_until(app, Duration::from_secs(4), |_| {
            srv.actor_patch_received.load(Ordering::SeqCst) > ap_rx_4b
        })
        .await
    );
    assert!(
        pump_until(app, Duration::from_secs(5), |cx| {
            !cw.read(cx).state.edit_profile.submitting
                && srv.actor_patch_completed.load(Ordering::SeqCst) > ap_done_4b
        })
        .await
    );
    let _ = cw.read_with(app, |w, _| {
        let a = &w.state.actor_profile.as_ref().unwrap().actor;
        assert_eq!(a.display_name, "Bot A Edited 4B");
        assert!(
            a.is_following && a.followers_count == 3 && a.is_muted,
            "T10-04B: submit_edit_profile must preserve Follow+Mute even when server update_actor returns unhydrated flags"
        );
    });
    // Real SQLite LocalCommunityRuntime proof: update_actor returns unhydrated is_following=false,
    // and patch_edited_actor_metadata preserves hydrated viewer flags!
    {
        let local_rt = cw.read_with(app, |w, _| w.facade.local_runtime().clone()).unwrap();
        let store = local_rt.store();
        let now = 1_750_000_100_000_i64;
        let viewer = store
            .create_human_actor(
                910881,
                &gpui_telegram_proto::community_contracts::RegisterHumanActorRequest {
                    handle: "r10_sql_viewer".into(),
                    display_name: "SQL Viewer".into(),
                    bio: None,
                    location: None,
                    website: None,
                    avatar_media_id: None,
                    banner_media_id: None,
                    avatar_shape: None,
                    avatar_color: None,
                },
                now,
            )
            .expect("sql viewer");
        let target = store
            .create_human_actor(
                910882,
                &gpui_telegram_proto::community_contracts::RegisterHumanActorRequest {
                    handle: "r10_sql_target".into(),
                    display_name: "SQL Target".into(),
                    bio: None,
                    location: None,
                    website: None,
                    avatar_media_id: None,
                    banner_media_id: None,
                    avatar_shape: None,
                    avatar_color: None,
                },
                now,
            )
            .expect("sql target");
        store
            .follow_actor(&viewer.actor_id, &target.actor_id, now + 1)
            .expect("sql follow");
        let hydrated = store
            .get_actor(&target.actor_id, Some(&viewer.actor_id))
            .expect("get")
            .expect("exists");
        assert!(hydrated.is_following && hydrated.followers_count == 1);
        let updated = store
            .update_actor(
                &target.actor_id,
                &gpui_telegram_proto::community_contracts::UpdateCommunityActorRequest {
                    display_name: Some("SQL Target Renamed".into()),
                    ..Default::default()
                },
                now + 2,
            )
            .expect("sql update_actor");
        assert!(
            !updated.is_following,
            "Real store::update_actor_tx returns viewer-unhydrated is_following=false"
        );
        let mut st = gpui_telegram_desktop::views::community::CommunityWorkspaceState::default();
        st.recommended_actors.push(hydrated);
        st.patch_edited_actor_metadata(&updated);
        assert_eq!(st.recommended_actors[0].display_name, "SQL Target Renamed");
        assert!(
            st.recommended_actors[0].is_following && st.recommended_actors[0].followers_count == 1,
            "T10-04B SQLite: patch_edited_actor_metadata must preserve hydrated follow flags"
        );
    }

    // 4C: R9-HUNT-4 (EditProf V1 slow -> BotStudio V2 fast) AND reverse (BotStudio V3 slow -> EditProf V4 fast)
    let ap_rx_c1 = srv.actor_patch_received.load(Ordering::SeqCst);
    let ap_done_c1 = srv.actor_patch_completed.load(Ordering::SeqCst);
    {
        let mut d = srv.actor_patch_post_delays_by_name.lock().unwrap();
        d.insert("EditProf V1".to_string(), 280);
        d.insert("BotStudio V2".to_string(), 10);
    }
    let _ = cw.update(app, |w, cx| {
        let a = w.state.actor_profile.as_ref().unwrap().actor.clone();
        w.open_edit_profile_modal(a, cx);
        w.state.edit_profile.display_name = "EditProf V1".to_string();
        w.submit_edit_profile(cx);
    });
    assert!(
        pump_until(app, Duration::from_secs(4), |_| {
            srv.actor_patch_received.load(Ordering::SeqCst) > ap_rx_c1
        })
        .await
    );
    let _ = cw.update(app, |w, cx| {
        w.close_edit_profile_modal(cx);
        let bot_a = w.state.actor_profile.as_ref().unwrap().actor.clone();
        w.select_bot_for_studio(bot_a, cx);
        w.state.bot_studio_edit.display_name = "BotStudio V2".to_string();
        w.save_selected_bot_edits(cx);
    });
    assert!(
        pump_until(app, Duration::from_secs(5), |cx| {
            let st = &cw.read(cx).state;
            !st.edit_profile.submitting
                && !st.bot_studio_edit.saving
                && srv.actor_patch_completed.load(Ordering::SeqCst) >= ap_done_c1 + 2
        })
        .await
    );
    let _ = cw.read_with(app, |w, _| {
        assert_eq!(
            w.state.actor_profile.as_ref().unwrap().actor.display_name,
            "BotStudio V2",
            "T10-04C (R9-HUNT-4): slow EditProf V1 must NOT clobber newer BotStudio V2!"
        );
    });

    // Reverse: BotStudio V3 slow (280ms) -> EditProf V4 fast (10ms)
    let ap_rx_c2 = srv.actor_patch_received.load(Ordering::SeqCst);
    let ap_done_c2 = srv.actor_patch_completed.load(Ordering::SeqCst);
    {
        let mut d = srv.actor_patch_post_delays_by_name.lock().unwrap();
        d.insert("BotStudio V3".to_string(), 280);
        d.insert("EditProf V4".to_string(), 10);
    }
    let _ = cw.update(app, |w, cx| {
        let bot_a = w.state.actor_profile.as_ref().unwrap().actor.clone();
        w.select_bot_for_studio(bot_a, cx);
        w.state.bot_studio_edit.display_name = "BotStudio V3".to_string();
        w.save_selected_bot_edits(cx);
    });
    assert!(
        pump_until(app, Duration::from_secs(4), |_| {
            srv.actor_patch_received.load(Ordering::SeqCst) > ap_rx_c2
        })
        .await
    );
    let _ = cw.update(app, |w, cx| {
        let a = w.state.actor_profile.as_ref().unwrap().actor.clone();
        w.open_edit_profile_modal(a, cx);
        w.state.edit_profile.display_name = "EditProf V4".to_string();
        w.submit_edit_profile(cx);
    });
    assert!(
        pump_until(app, Duration::from_secs(5), |cx| {
            let st = &cw.read(cx).state;
            !st.edit_profile.submitting
                && !st.bot_studio_edit.saving
                && srv.actor_patch_completed.load(Ordering::SeqCst) >= ap_done_c2 + 2
        })
        .await
    );
    let _ = cw.read_with(app, |w, _| {
        assert_eq!(
            w.state.actor_profile.as_ref().unwrap().actor.display_name,
            "EditProf V4",
            "T10-04C reverse: slow BotStudio V3 must NOT clobber newer EditProf V4!"
        );
        assert!(!w.state.bot_studio_edit.saving);
        assert_ne!(
            w.state.bot_studio_edit.display_name, "BotStudio V3",
            "T10-04C reverse: stale BotStudio V3 must NOT call select_bot_for_studio over EditProf V4!"
        );
        assert_eq!(w.state.bot_studio_edit.display_name, "EditProf V4");
    });

    // 4D: Cross-actor concurrent edits: slow bot_a ("Bot Cross A2", 260ms) + fast human_alice ("Alice Cross 2", 10ms)
    let ap_rx_4d = srv.actor_patch_received.load(Ordering::SeqCst);
    let ap_done_4d = srv.actor_patch_completed.load(Ordering::SeqCst);
    {
        let mut d = srv.actor_patch_post_delays_by_name.lock().unwrap();
        d.insert("Bot Cross A2".to_string(), 260);
        d.insert("Alice Cross 2".to_string(), 10);
    }
    let _ = cw.update(app, |w, cx| {
        let bot_a = w.state.actor_profile.as_ref().unwrap().actor.clone();
        w.select_bot_for_studio(bot_a, cx);
        w.state.bot_studio_edit.display_name = "Bot Cross A2".to_string();
        w.save_selected_bot_edits(cx);
    });
    assert!(
        pump_until(app, Duration::from_secs(4), |_| {
            srv.actor_patch_received.load(Ordering::SeqCst) > ap_rx_4d
        })
        .await
    );
    let _ = cw.update(app, |w, cx| {
        let alice = w
            .state
            .identities
            .as_ref()
            .unwrap()
            .human_actor
            .clone()
            .unwrap();
        w.open_edit_profile_modal(alice, cx);
        w.state.edit_profile.display_name = "Alice Cross 2".to_string();
        w.submit_edit_profile(cx);
    });
    assert!(
        pump_until(app, Duration::from_secs(5), |cx| {
            let st = &cw.read(cx).state;
            !st.edit_profile.submitting
                && !st.bot_studio_edit.saving
                && srv.actor_patch_completed.load(Ordering::SeqCst) >= ap_done_4d + 2
        })
        .await
    );
    let _ = cw.read_with(app, |w, _| {
        let ids = w.state.identities.as_ref().unwrap();
        assert_eq!(
            ids.human_actor.as_ref().unwrap().display_name,
            "Alice Cross 2",
            "T10-04D: cross-actor fast human_alice edit must survive"
        );
        assert_eq!(
            ids.bot_actors[0].display_name,
            "Bot Cross A2",
            "T10-04D: cross-actor slow bot_a edit must survive in identities"
        );
        assert_eq!(
            w.state.actor_profile.as_ref().unwrap().actor.display_name,
            "Bot Cross A2",
            "T10-04D: cross-actor slow bot_a edit must survive in actor_profile"
        );
    });

    // ------------------------------------------------------------------
    // T10-DE4-5 (Bonus flush_list_sync Sibling): Slow post-commit toggle_list_pinned
    // returning stale name="Old List" while load_saved_posts refreshes name="Renamed Elsewhere"
    // ------------------------------------------------------------------
    let _ = cw.update(app, |w, cx| {
        w.navigate_to(CommunityRoute::BookmarksLists, cx);
        w.set_bookmarks_section(BookmarksListsSection::Lists, cx);
    });
    assert!(
        pump_until(app, Duration::from_secs(4), |cx| {
            let st = &cw.read(cx).state;
            !st.saved_loading && st.lists.iter().any(|l| l.list_id == srv.list_id)
        })
        .await
    );
    let lt_rx = srv.list_toggle_received.load(Ordering::SeqCst);
    let lt_done = srv.list_toggle_completed.load(Ordering::SeqCst);
    let lg_done = srv.lists_get_completed.load(Ordering::SeqCst);
    srv.list_toggle_post_delay_ms.store(300, Ordering::SeqCst);
    let _ = cw.update(app, |w, cx| {
        w.toggle_list_pinned(srv.list_id.clone(), true, cx);
    });
    assert!(
        pump_until(app, Duration::from_secs(4), |_| {
            srv.list_toggle_received.load(Ordering::SeqCst) > lt_rx
        })
        .await
    );
    // Remote rename happens on server AFTER flush_list_sync captured stale "Old List" snapshot,
    // and load_saved_posts refreshes the newer name while flush_list_sync is still in post-sleep!
    *srv.list_name.lock().unwrap() = "Renamed Elsewhere".to_string();
    let _ = cw.update(app, |w, cx| {
        w.load_saved_posts(cx);
    });
    assert!(
        pump_until(app, Duration::from_secs(4), |cx| {
            !cw.read(cx).state.saved_loading
                && srv.lists_get_completed.load(Ordering::SeqCst) > lg_done
        })
        .await
    );
    assert!(
        pump_until(app, Duration::from_secs(5), |cx| {
            cw.read(cx).state.list_membership_in_flight.is_empty()
                && srv.list_toggle_completed.load(Ordering::SeqCst) > lt_done
        })
        .await
    );
    let _ = cw.read_with(app, |w, _| {
        let l = w
            .state
            .lists
            .iter()
            .find(|x| x.list_id == srv.list_id)
            .unwrap();
        assert_eq!(
            (l.name.as_str(), l.is_pinned),
            ("Renamed Elsewhere", true),
            "T10-DE4-5: flush_list_sync must NOT clobber list name with stale DTO snapshot!"
        );
    });
}

async fn run_r10_t05_to_t08(
    app: &mut AsyncApp,
    cw: &Entity<CommunityWorkspace>,
    srv: &Arc<R10MockState>,
) {
    use std::sync::atomic::Ordering;

    // ------------------------------------------------------------------
    // T10-05 (R9-HUNT-2a/2b == R9-ADV-NEW-5): Symmetric Unblock <-> Follow Serialization
    // All 6 cases (5A..5F) with bob starting (blocked=true, following=false, followers=5)
    // ------------------------------------------------------------------
    let _ = cw.update(app, |w, cx| {
        w.navigate_to(CommunityRoute::ActorProfile("r10_bob".to_string()), cx);
    });
    assert!(
        pump_until(app, Duration::from_secs(4), |cx| {
            !cw.read(cx).state.profile_loading
                && cw
                    .read(cx)
                    .state
                    .actor_profile
                    .as_ref()
                    .is_some_and(|p| p.actor.actor_id == srv.bob_id)
        })
        .await
    );

    // Helper closure to reset bob to (true, false, 5) and reload ActorProfile
    async fn reset_bob_blocked(
        app: &mut AsyncApp,
        cw: &Entity<CommunityWorkspace>,
        srv: &Arc<R10MockState>,
    ) {
        use std::sync::atomic::Ordering;
        srv.bob_blocked.store(true, Ordering::SeqCst);
        srv.bob_following.store(false, Ordering::SeqCst);
        srv.bob_followers.store(5, Ordering::SeqCst);
        srv.follow_fail_500.store(false, Ordering::SeqCst);
        srv.block_fail_500.store(false, Ordering::SeqCst);
        srv.follow_block_max_in_flight.store(0, Ordering::SeqCst);
        srv.follow_403_count.store(0, Ordering::SeqCst);
        let pg = srv.profile_get_completed.load(Ordering::SeqCst);
        let _ = cw.update(app, |w, cx| {
            w.load_actor_profile("r10_bob".to_string(), cx);
        });
        assert!(
            pump_until(app, Duration::from_secs(4), |cx| {
                !cw.read(cx).state.profile_loading
                    && srv.profile_get_completed.load(Ordering::SeqCst) > pg
            })
            .await
        );
    }

    // 5A: Unblock(pre 180ms) -> Follow(20ms) -> serialized (max_fb == 1, 403 == 0), (false, true, 6)
    reset_bob_blocked(app, cw, srv).await;
    let blk_b = srv.block_completed.load(Ordering::SeqCst);
    let fl_b = srv.follow_completed.load(Ordering::SeqCst);
    srv.block_pre_delay_ms.store(180, Ordering::SeqCst);
    srv.follow_pre_delay_ms.store(20, Ordering::SeqCst);
    let _ = cw.update(app, |w, cx| {
        w.toggle_block_actor(srv.bob_id.clone(), true, cx);
        w.toggle_follow_actor(srv.bob_id.clone(), true, cx);
    });
    assert!(
        pump_until(app, Duration::from_secs(5), |cx| {
            let st = &cw.read(cx).state;
            st.actor_block_in_flight.is_empty()
                && st.actor_follow_in_flight.is_empty()
                && st.actor_follow_dirty.is_empty()
                && srv.block_completed.load(Ordering::SeqCst) > blk_b
                && srv.follow_completed.load(Ordering::SeqCst) > fl_b
        })
        .await,
        "T10-05A: Unblock(pre)->Follow must settle"
    );
    assert_eq!(srv.follow_block_max_in_flight.load(Ordering::SeqCst), 1, "T10-05A max_fb");
    assert_eq!(srv.follow_403_count.load(Ordering::SeqCst), 0, "T10-05A follow_403");
    let _ = cw.read_with(app, |w, _| {
        let a = &w.state.actor_profile.as_ref().unwrap().actor;
        assert_eq!((a.is_blocked, a.is_following, a.followers_count), (false, true, 6));
    });
    assert_eq!(
        (
            srv.bob_blocked.load(Ordering::SeqCst),
            srv.bob_following.load(Ordering::SeqCst),
            srv.bob_followers.load(Ordering::SeqCst),
        ),
        (false, true, 6)
    );

    // 5B: Unblock(post 140ms) -> Follow(240ms): assert ZERO mid-flight UI wipe when Unblock completes!
    reset_bob_blocked(app, cw, srv).await;
    let blk_rx_5b = srv.block_received.load(Ordering::SeqCst);
    let blk_b_5b = srv.block_completed.load(Ordering::SeqCst);
    let fl_b_5b = srv.follow_completed.load(Ordering::SeqCst);
    srv.block_post_delay_ms.store(140, Ordering::SeqCst);
    srv.follow_pre_delay_ms.store(240, Ordering::SeqCst);
    let _ = cw.update(app, |w, cx| {
        w.toggle_block_actor(srv.bob_id.clone(), true, cx);
    });
    assert!(
        pump_until(app, Duration::from_secs(4), |_| {
            srv.block_received.load(Ordering::SeqCst) > blk_rx_5b
        })
        .await
    );
    let _ = cw.update(app, |w, cx| {
        w.toggle_follow_actor(srv.bob_id.clone(), true, cx);
    });
    // Wait for Unblock to complete and launch deferred Follow while Follow is still in its 240ms sleep
    assert!(
        pump_until(app, Duration::from_secs(4), |cx| {
            let st = &cw.read(cx).state;
            srv.block_completed.load(Ordering::SeqCst) > blk_b_5b
                && st.actor_block_in_flight.is_empty()
                && st.actor_follow_in_flight.contains_key(&srv.bob_id)
        })
        .await,
        "T10-05B: Unblock settlement must immediately launch deferred Follow"
    );
    let _ = cw.read_with(app, |w, _| {
        let a = &w.state.actor_profile.as_ref().unwrap().actor;
        assert_eq!(
            (a.is_blocked, a.is_following, a.followers_count),
            (false, true, 6),
            "T10-05B (R9-HUNT-2b): Unblock settlement must NEVER wipe optimistic Follow UI mid-flight!"
        );
    });
    assert!(
        pump_until(app, Duration::from_secs(5), |cx| {
            cw.read(cx).state.actor_follow_in_flight.is_empty()
                && srv.follow_completed.load(Ordering::SeqCst) > fl_b_5b
        })
        .await
    );
    assert_eq!(srv.follow_block_max_in_flight.load(Ordering::SeqCst), 1, "T10-05B max_fb");
    assert_eq!(srv.follow_403_count.load(Ordering::SeqCst), 0, "T10-05B follow_403");
    let _ = cw.read_with(app, |w, _| {
        let a = &w.state.actor_profile.as_ref().unwrap().actor;
        assert_eq!((a.is_blocked, a.is_following, a.followers_count), (false, true, 6));
    });

    // 5C: Unblock(500) -> Follow: follow_calls == 0, rolls back to (true, false, 5), all follow maps empty
    reset_bob_blocked(app, cw, srv).await;
    let fl_calls_5c = srv.follow_calls.load(Ordering::SeqCst);
    let blk_b_5c = srv.block_completed.load(Ordering::SeqCst);
    srv.block_pre_delay_ms.store(120, Ordering::SeqCst);
    srv.block_fail_500.store(true, Ordering::SeqCst);
    let _ = cw.update(app, |w, cx| {
        w.toggle_block_actor(srv.bob_id.clone(), true, cx);
        w.toggle_follow_actor(srv.bob_id.clone(), true, cx);
    });
    assert!(
        pump_until(app, Duration::from_secs(4), |cx| {
            cw.read(cx).state.actor_block_in_flight.is_empty()
                && srv.block_completed.load(Ordering::SeqCst) > blk_b_5c
        })
        .await
    );
    srv.block_fail_500.store(false, Ordering::SeqCst);
    assert_eq!(srv.follow_calls.load(Ordering::SeqCst), fl_calls_5c, "T10-05C: 0 follow calls");
    let _ = cw.read_with(app, |w, _| {
        let a = &w.state.actor_profile.as_ref().unwrap().actor;
        assert_eq!((a.is_blocked, a.is_following, a.followers_count), (true, false, 5));
        assert!(
            w.state.actor_follow_dirty.is_empty()
                && w.state.actor_follow_pending_target.is_empty()
                && w.state.actor_follow_last_synced.is_empty()
                && w.state.actor_follow_in_flight.is_empty(),
            "T10-05C: deferred Follow maps must be cleared on Unblock 500"
        );
    });

    // 5D: Unblock(200) -> Follow -> Unfollow: follow_calls == 0, (false, false, 5), maps empty
    reset_bob_blocked(app, cw, srv).await;
    let fl_calls_5d = srv.follow_calls.load(Ordering::SeqCst);
    let blk_b_5d = srv.block_completed.load(Ordering::SeqCst);
    srv.block_pre_delay_ms.store(140, Ordering::SeqCst);
    let _ = cw.update(app, |w, cx| {
        w.toggle_block_actor(srv.bob_id.clone(), true, cx);
        w.toggle_follow_actor(srv.bob_id.clone(), true, cx);
        w.toggle_follow_actor(srv.bob_id.clone(), false, cx);
    });
    assert!(
        pump_until(app, Duration::from_secs(4), |cx| {
            cw.read(cx).state.actor_block_in_flight.is_empty()
                && srv.block_completed.load(Ordering::SeqCst) > blk_b_5d
        })
        .await
    );
    assert_eq!(srv.follow_calls.load(Ordering::SeqCst), fl_calls_5d, "T10-05D: 0 follow calls");
    let _ = cw.read_with(app, |w, _| {
        let a = &w.state.actor_profile.as_ref().unwrap().actor;
        assert_eq!((a.is_blocked, a.is_following, a.followers_count), (false, false, 5));
        assert!(
            w.state.actor_follow_dirty.is_empty()
                && w.state.actor_follow_pending_target.is_empty()
                && w.state.actor_follow_last_synced.is_empty()
                && w.state.actor_follow_in_flight.is_empty(),
            "T10-05D: Follow+Unfollow cancellation must leave all follow maps empty"
        );
    });

    // 5E: Unblock(200) -> Follow -> Re-Block(200): follow_calls == 0, (true, false, 5), no last_synced leak
    reset_bob_blocked(app, cw, srv).await;
    let fl_calls_5e = srv.follow_calls.load(Ordering::SeqCst);
    let blk_calls_5e = srv.block_calls.load(Ordering::SeqCst);
    let blk_b_5e = srv.block_completed.load(Ordering::SeqCst);
    srv.block_pre_delay_ms.store(120, Ordering::SeqCst);
    let _ = cw.update(app, |w, cx| {
        w.toggle_block_actor(srv.bob_id.clone(), true, cx);
        w.toggle_follow_actor(srv.bob_id.clone(), true, cx);
        w.toggle_block_actor(srv.bob_id.clone(), false, cx);
    });
    assert!(
        pump_until(app, Duration::from_secs(5), |cx| {
            let st = &cw.read(cx).state;
            st.actor_block_in_flight.is_empty()
                && st.actor_block_dirty.is_empty()
                && srv.block_completed.load(Ordering::SeqCst) >= blk_b_5e + 2
        })
        .await
    );
    assert_eq!(srv.follow_calls.load(Ordering::SeqCst), fl_calls_5e, "T10-05E: 0 follow calls");
    assert!(srv.block_calls.load(Ordering::SeqCst) >= blk_calls_5e + 2);
    let _ = cw.read_with(app, |w, _| {
        let a = &w.state.actor_profile.as_ref().unwrap().actor;
        assert_eq!((a.is_blocked, a.is_following, a.followers_count), (true, false, 5));
        assert!(
            w.state.actor_follow_dirty.is_empty()
                && w.state.actor_follow_pending_target.is_empty()
                && w.state.actor_follow_last_synced.is_empty()
                && w.state.actor_follow_in_flight.is_empty(),
            "T10-05E: Re-Block must not leak actor_follow_last_synced"
        );
    });

    // 5F: Unblock(200) -> Follow(500) + fast mid-flight GET /actors/bob overlay check -> final (false, false, 5)
    reset_bob_blocked(app, cw, srv).await;
    let blk_b_5f = srv.block_completed.load(Ordering::SeqCst);
    let fl_b_5f = srv.follow_completed.load(Ordering::SeqCst);
    let pg_5f = srv.profile_get_completed.load(Ordering::SeqCst);
    srv.block_pre_delay_ms.store(160, Ordering::SeqCst);
    srv.follow_pre_delay_ms.store(140, Ordering::SeqCst);
    srv.follow_fail_500.store(true, Ordering::SeqCst);
    let _ = cw.update(app, |w, cx| {
        w.toggle_block_actor(srv.bob_id.clone(), true, cx);
        w.toggle_follow_actor(srv.bob_id.clone(), true, cx);
        w.load_actor_profile("r10_bob".to_string(), cx);
    });
    // Mid-flight GET completes while Unblock is still in pre-sleep -> overlay must preserve (false, true, 6)
    assert!(
        pump_until(app, Duration::from_secs(4), |cx| {
            !cw.read(cx).state.profile_loading
                && srv.profile_get_completed.load(Ordering::SeqCst) > pg_5f
        })
        .await
    );
    let _ = cw.read_with(app, |w, _| {
        if !w.state.actor_block_in_flight.is_empty() || !w.state.actor_follow_in_flight.is_empty() {
            let a = &w.state.actor_profile.as_ref().unwrap().actor;
            assert_eq!(
                (a.is_blocked, a.is_following, a.followers_count),
                (false, true, 6),
                "T10-05F: mid-flight GET overlay must preserve Unblock+deferred Follow"
            );
        }
    });
    assert!(
        pump_until(app, Duration::from_secs(5), |cx| {
            let st = &cw.read(cx).state;
            st.actor_block_in_flight.is_empty()
                && st.actor_follow_in_flight.is_empty()
                && srv.block_completed.load(Ordering::SeqCst) > blk_b_5f
                && srv.follow_completed.load(Ordering::SeqCst) > fl_b_5f
        })
        .await
    );
    srv.follow_fail_500.store(false, Ordering::SeqCst);
    let _ = cw.read_with(app, |w, _| {
        let a = &w.state.actor_profile.as_ref().unwrap().actor;
        assert_eq!((a.is_blocked, a.is_following, a.followers_count), (false, false, 5));
    });

    // ------------------------------------------------------------------
    // T10-06 (R9-HUNT-3a/3b + submit_edit_comment ABA)
    // ------------------------------------------------------------------
    // 6A: Wiki Save #1 (320ms) -> ai -> rust -> Save #2 (550ms)
    let _ = cw.update(app, |w, cx| {
        w.navigate_to(CommunityRoute::CommunityPage(srv.slug.clone()), cx);
    });
    assert!(
        pump_until(app, Duration::from_secs(4), |cx| {
            !cw.read(cx).state.community_loading
                && cw
                    .read(cx)
                    .state
                    .community_detail
                    .as_ref()
                    .is_some_and(|d| d.community.slug == srv.slug)
        })
        .await
    );
    let wk_rx_a = srv.wiki_put_received.load(Ordering::SeqCst);
    let wk_done_a = srv.wiki_put_completed.load(Ordering::SeqCst);
    srv.wiki_post_delay_ms.store(320, Ordering::SeqCst);
    let _ = cw.update(app, |w, cx| {
        w.state.community_wiki_editing = true;
        w.state.community_wiki_draft = "Wiki V1".to_string();
        w.save_community_wiki(srv.slug.clone(), cx);
    });
    assert!(
        pump_until(app, Duration::from_secs(4), |_| {
            srv.wiki_put_received.load(Ordering::SeqCst) > wk_rx_a
        })
        .await
    );
    // Nav rust -> ai -> rust
    let _ = cw.update(app, |w, cx| {
        w.navigate_to(CommunityRoute::CommunityPage(srv.alt_slug.clone()), cx);
    });
    assert!(
        pump_until(app, Duration::from_secs(4), |cx| {
            !cw.read(cx).state.community_loading
                && cw
                    .read(cx)
                    .state
                    .community_detail
                    .as_ref()
                    .is_some_and(|d| d.community.slug == srv.alt_slug)
        })
        .await
    );
    let _ = cw.update(app, |w, cx| {
        w.navigate_to(CommunityRoute::CommunityPage(srv.slug.clone()), cx);
    });
    assert!(
        pump_until(app, Duration::from_secs(4), |cx| {
            !cw.read(cx).state.community_loading
                && cw
                    .read(cx)
                    .state
                    .community_detail
                    .as_ref()
                    .is_some_and(|d| d.community.slug == srv.slug)
        })
        .await
    );
    srv.wiki_post_delay_ms.store(550, Ordering::SeqCst);
    let _ = cw.update(app, |w, cx| {
        w.state.community_wiki_editing = true;
        w.state.community_wiki_draft = "Wiki V2".to_string();
        w.save_community_wiki(srv.slug.clone(), cx);
    });
    // Wait for Save #1 to complete while Save #2 is still in flight
    assert!(
        pump_until(app, Duration::from_secs(4), |_| {
            srv.wiki_put_completed.load(Ordering::SeqCst) >= wk_done_a + 1
        })
        .await
    );
    app.background_executor().timer(Duration::from_millis(30)).await;
    let _ = cw.read_with(app, |w, _| {
        assert!(
            w.state.community_wiki_saving && w.state.community_wiki_editing,
            "T10-06A (R9-HUNT-3a): stale Wiki Save #1 must NOT clear Save #2 mutex or close editor!"
        );
    });
    assert!(
        pump_until(app, Duration::from_secs(5), |cx| {
            !cw.read(cx).state.community_wiki_saving
                && srv.wiki_put_completed.load(Ordering::SeqCst) >= wk_done_a + 2
        })
        .await
    );
    let _ = cw.read_with(app, |w, _| {
        assert_eq!(
            w.state.community_detail.as_ref().unwrap().community.wiki_markdown,
            "Wiki V2"
        );
    });

    // 6B: Rules Save #1 (360ms post-commit) -> ai -> rust -> Rules Save #2 (10ms)
    let rp_rx_b = srv.rules_patch_received.load(Ordering::SeqCst);
    let rp_done_b = srv.rules_patch_completed.load(Ordering::SeqCst);
    let rules_before_b = srv.comm_rules.lock().unwrap().len();
    srv.rules_post_delay_ms.store(360, Ordering::SeqCst);
    let _ = cw.update(app, |w, cx| {
        w.state.community_new_rule_title = "Rule ABA1".to_string();
        w.add_community_rule(srv.slug.clone(), cx);
    });
    assert!(
        pump_until(app, Duration::from_secs(4), |_| {
            srv.rules_patch_received.load(Ordering::SeqCst) > rp_rx_b
        })
        .await
    );
    let _ = cw.update(app, |w, cx| {
        w.navigate_to(CommunityRoute::CommunityPage(srv.alt_slug.clone()), cx);
    });
    assert!(
        pump_until(app, Duration::from_secs(4), |cx| {
            !cw.read(cx).state.community_loading
                && cw
                    .read(cx)
                    .state
                    .community_detail
                    .as_ref()
                    .is_some_and(|d| d.community.slug == srv.alt_slug)
        })
        .await
    );
    let _ = cw.update(app, |w, cx| {
        w.navigate_to(CommunityRoute::CommunityPage(srv.slug.clone()), cx);
    });
    assert!(
        pump_until(app, Duration::from_secs(4), |cx| {
            !cw.read(cx).state.community_loading
                && cw
                    .read(cx)
                    .state
                    .community_detail
                    .as_ref()
                    .is_some_and(|d| d.community.slug == srv.slug)
        })
        .await
    );
    srv.rules_post_delay_ms.store(10, Ordering::SeqCst);
    let _ = cw.update(app, |w, cx| {
        w.state.community_new_rule_title = "Rule ABA2".to_string();
        w.add_community_rule(srv.slug.clone(), cx);
    });
    assert!(
        pump_until(app, Duration::from_secs(5), |cx| {
            !cw.read(cx).state.community_rules_saving
                && srv.rules_patch_completed.load(Ordering::SeqCst) >= rp_done_b + 2
        })
        .await
    );
    app.background_executor().timer(Duration::from_millis(50)).await;
    let _ = cw.read_with(app, |w, _| {
        let want = rules_before_b + 2;
        assert_eq!(
            w.state.communities.iter().find(|c| c.slug == srv.slug).unwrap().rules.len(),
            want,
            "T10-06B (R9-HUNT-3b): stale Rules Save #1 must NOT clobber communities!"
        );
        assert_eq!(
            w.state.community_detail.as_ref().unwrap().community.rules.len(),
            want,
            "T10-06B: stale Rules Save #1 must NOT clobber community_detail!"
        );
        assert_eq!(
            w.state.mod_community.as_ref().unwrap().rules.len(),
            want,
            "T10-06B: stale Rules Save #1 must NOT clobber mod_community!"
        );
    });

    // 6C: submit_edit_comment double-submit mutex + nav-away/back c1->c2 + reverse-order V1 vs V2
    let _ = cw.update(app, |w, cx| {
        w.navigate_to(CommunityRoute::PostDetail(srv.p_edit.clone()), cx);
    });
    assert!(
        pump_until(app, Duration::from_secs(4), |cx| {
            !cw.read(cx).state.detail_loading
                && cw
                    .read(cx)
                    .state
                    .detail_post
                    .as_ref()
                    .is_some_and(|d| d.post.post_id == srv.p_edit)
        })
        .await
    );
    let cm_rx = srv.comment_patch_received.load(Ordering::SeqCst);
    let cm_done = srv.comment_patch_completed.load(Ordering::SeqCst);
    srv.comment_patch_delays_by_body
        .lock()
        .unwrap()
        .insert("C1 Mutex".to_string(), 140);
    let _ = cw.update(app, |w, cx| {
        w.start_edit_comment(srv.c1_id.clone(), "Initial C1".to_string(), cx);
        w.state.detail_edit_comment_draft = "C1 Mutex".to_string();
        w.submit_edit_comment(cx);
        assert!(w.state.detail_edit_comment_saving);
        // Duplicate submit AND start_edit_comment(c2) must be blocked while saving!
        w.submit_edit_comment(cx);
        w.start_edit_comment(srv.c2_id.clone(), "Initial C2".to_string(), cx);
        assert_eq!(
            w.state.detail_editing_comment_id.as_deref(),
            Some(srv.c1_id.as_str()),
            "T10-06C: start_edit_comment must be blocked while detail_edit_comment_saving"
        );
    });
    assert!(
        pump_until(app, Duration::from_secs(4), |cx| {
            !cw.read(cx).state.detail_edit_comment_saving
                && srv.comment_patch_completed.load(Ordering::SeqCst) > cm_done
        })
        .await
    );
    assert_eq!(
        srv.comment_patch_received.load(Ordering::SeqCst),
        cm_rx + 1,
        "T10-06C: double-submit mutex must send exactly 1 PATCH"
    );

    // Nav away/back ABA: c1 Save #1 (280ms) -> p_part2 -> p_edit -> start_edit_comment(c2)
    let cm_rx2 = srv.comment_patch_received.load(Ordering::SeqCst);
    let cm_done2 = srv.comment_patch_completed.load(Ordering::SeqCst);
    srv.comment_patch_delays_by_body
        .lock()
        .unwrap()
        .insert("C1 ABA".to_string(), 280);
    let _ = cw.update(app, |w, cx| {
        w.start_edit_comment(srv.c1_id.clone(), "C1 Mutex".to_string(), cx);
        w.state.detail_edit_comment_draft = "C1 ABA".to_string();
        w.submit_edit_comment(cx);
    });
    assert!(
        pump_until(app, Duration::from_secs(4), |_| {
            srv.comment_patch_received.load(Ordering::SeqCst) > cm_rx2
        })
        .await
    );
    let _ = cw.update(app, |w, cx| {
        w.navigate_to(CommunityRoute::PostDetail(srv.p_part2.clone()), cx);
    });
    assert!(
        pump_until(app, Duration::from_secs(4), |cx| {
            !cw.read(cx).state.detail_loading
                && cw
                    .read(cx)
                    .state
                    .detail_post
                    .as_ref()
                    .is_some_and(|d| d.post.post_id == srv.p_part2)
        })
        .await
    );
    let _ = cw.update(app, |w, cx| {
        w.navigate_to(CommunityRoute::PostDetail(srv.p_edit.clone()), cx);
    });
    assert!(
        pump_until(app, Duration::from_secs(4), |cx| {
            !cw.read(cx).state.detail_loading
                && cw
                    .read(cx)
                    .state
                    .detail_post
                    .as_ref()
                    .is_some_and(|d| d.post.post_id == srv.p_edit)
        })
        .await
    );
    let _ = cw.update(app, |w, cx| {
        w.start_edit_comment(srv.c2_id.clone(), "Initial C2".to_string(), cx);
    });
    assert!(
        pump_until(app, Duration::from_secs(4), |_| {
            srv.comment_patch_completed.load(Ordering::SeqCst) > cm_done2
        })
        .await
    );
    app.background_executor().timer(Duration::from_millis(40)).await;
    let _ = cw.read_with(app, |w, _| {
        assert_eq!(
            w.state.detail_editing_comment_id.as_deref(),
            Some(srv.c2_id.as_str()),
            "T10-06C: stale c1 Save #1 must NOT cancel active c2 editor!"
        );
    });

    // Reverse-order c1: "C1 V1" (280ms) -> nav away/back -> "C1 V2" (10ms)
    let cm_rx3 = srv.comment_patch_received.load(Ordering::SeqCst);
    let cm_done3 = srv.comment_patch_completed.load(Ordering::SeqCst);
    {
        let mut d = srv.comment_patch_delays_by_body.lock().unwrap();
        d.insert("C1 V1".to_string(), 280);
        d.insert("C1 V2".to_string(), 10);
    }
    let _ = cw.update(app, |w, cx| {
        w.cancel_edit_comment(cx);
        w.start_edit_comment(srv.c1_id.clone(), "C1 ABA".to_string(), cx);
        w.state.detail_edit_comment_draft = "C1 V1".to_string();
        w.submit_edit_comment(cx);
    });
    assert!(
        pump_until(app, Duration::from_secs(4), |_| {
            srv.comment_patch_received.load(Ordering::SeqCst) > cm_rx3
        })
        .await
    );
    let _ = cw.update(app, |w, cx| {
        w.navigate_to(CommunityRoute::PostDetail(srv.p_part2.clone()), cx);
    });
    assert!(
        pump_until(app, Duration::from_secs(4), |cx| {
            !cw.read(cx).state.detail_loading
                && cw
                    .read(cx)
                    .state
                    .detail_post
                    .as_ref()
                    .is_some_and(|d| d.post.post_id == srv.p_part2)
        })
        .await
    );
    let _ = cw.update(app, |w, cx| {
        w.navigate_to(CommunityRoute::PostDetail(srv.p_edit.clone()), cx);
    });
    assert!(
        pump_until(app, Duration::from_secs(4), |cx| {
            !cw.read(cx).state.detail_loading
                && cw
                    .read(cx)
                    .state
                    .detail_post
                    .as_ref()
                    .is_some_and(|d| d.post.post_id == srv.p_edit)
        })
        .await
    );
    let _ = cw.update(app, |w, cx| {
        w.start_edit_comment(srv.c1_id.clone(), "C1 V1".to_string(), cx);
        w.state.detail_edit_comment_draft = "C1 V2".to_string();
        w.submit_edit_comment(cx);
    });
    assert!(
        pump_until(app, Duration::from_secs(5), |cx| {
            !cw.read(cx).state.detail_edit_comment_saving
                && srv.comment_patch_completed.load(Ordering::SeqCst) >= cm_done3 + 2
        })
        .await
    );
    app.background_executor().timer(Duration::from_millis(40)).await;
    let _ = cw.read_with(app, |w, _| {
        let c1 = w
            .state
            .detail_post
            .as_ref()
            .unwrap()
            .comments
            .iter()
            .find(|c| c.comment_id == srv.c1_id)
            .unwrap();
        assert_eq!(c1.body, "C1 V2", "T10-06C: stale 'C1 V1' must NOT clobber 'C1 V2'!");
    });

    // ------------------------------------------------------------------
    // T10-07 (R9-ADV-NEW-6 + cross-route comment Case B):
    // Non-primary thread_chain post vote/like settling during slow load_post_detail(p_part2),
    // PLUS ActorProfile comment vote settling while PostDetail(p_part2) is loading!
    // ------------------------------------------------------------------
    let _ = cw.update(app, |w, cx| {
        w.navigate_to(CommunityRoute::PostDetail(srv.p_part2.clone()), cx);
    });
    assert!(
        pump_until(app, Duration::from_secs(4), |cx| {
            !cw.read(cx).state.detail_loading
                && cw
                    .read(cx)
                    .state
                    .detail_post
                    .as_ref()
                    .is_some_and(|d| d.post.post_id == srv.p_part2)
        })
        .await
    );
    let dg_rx = srv.detail_get_received.load(Ordering::SeqCst);
    let dg_done = srv.detail_get_completed.load(Ordering::SeqCst);
    let pv_7 = srv.post_vote_completed.load(Ordering::SeqCst);
    let pl_7 = srv.post_like_completed.load(Ordering::SeqCst);
    srv.detail_get_delay_ms.store(260, Ordering::SeqCst);
    let _ = cw.update(app, |w, cx| {
        w.load_post_detail(srv.p_part2.clone(), cx);
    });
    assert!(
        pump_until(app, Duration::from_secs(4), |_| {
            srv.detail_get_received.load(Ordering::SeqCst) > dg_rx
        })
        .await
    );
    let _ = cw.update(app, |w, cx| {
        w.vote_post(srv.p_part1.clone(), 1, cx);
        w.toggle_like(srv.p_part1.clone(), cx);
    });
    assert!(
        pump_until(app, Duration::from_secs(4), |cx| {
            let st = &cw.read(cx).state;
            st.post_vote_in_flight.is_empty()
                && st.post_like_in_flight.is_empty()
                && srv.post_vote_completed.load(Ordering::SeqCst) > pv_7
                && srv.post_like_completed.load(Ordering::SeqCst) > pl_7
        })
        .await
    );
    assert!(
        pump_until(app, Duration::from_secs(5), |cx| {
            !cw.read(cx).state.detail_loading
                && srv.detail_get_completed.load(Ordering::SeqCst) >= dg_done + 2
        })
        .await,
        "T10-07 (R9-ADV-NEW-6): non-primary thread_chain vote/like must trigger PostDetail Case B reload"
    );
    let _ = cw.read_with(app, |w, _| {
        let p1 = &w.state.detail_post.as_ref().unwrap().thread_chain[0];
        assert_eq!(p1.post_id, srv.p_part1);
        assert_eq!(p1.viewer_vote, 1, "T10-07: thread_chain[0] vote must survive!");
        assert!(p1.viewer_liked, "T10-07: thread_chain[0] like must survive!");
    });

    // Cross-route comment vote: start on ActorProfile (detail_pid == None), navigate to PostDetail
    // with slow GET, comment vote settles while PostDetail is loading -> triggers Case B reload!
    let _ = cw.update(app, |w, cx| {
        w.navigate_to(CommunityRoute::ActorProfile("r10_alice".to_string()), cx);
    });
    assert!(
        pump_until(app, Duration::from_secs(4), |cx| {
            !cw.read(cx).state.profile_loading && cw.read(cx).state.actor_profile.is_some()
        })
        .await
    );
    let dg_done_c = srv.detail_get_completed.load(Ordering::SeqCst);
    let cv_7 = srv.comment_vote_completed.load(Ordering::SeqCst);
    srv.detail_get_delay_ms.store(260, Ordering::SeqCst);
    let _ = cw.update(app, |w, cx| {
        // Clear detail_post so detail_pid is None on ActorProfile!
        w.state.detail_post = None;
        w.vote_comment_action(srv.c1_id.clone(), 1, cx);
        w.navigate_to(CommunityRoute::PostDetail(srv.p_part2.clone()), cx);
    });
    assert!(
        pump_until(app, Duration::from_secs(5), |cx| {
            let st = &cw.read(cx).state;
            !st.detail_loading
                && st.comment_vote_in_flight.is_empty()
                && srv.comment_vote_completed.load(Ordering::SeqCst) > cv_7
                && srv.detail_get_completed.load(Ordering::SeqCst) >= dg_done_c + 2
        })
        .await,
        "T10-07 cross-route comment Case B: ActorProfile comment vote settling during PostDetail load must reload"
    );
    let _ = cw.read_with(app, |w, _| {
        let c1 = w
            .state
            .detail_post
            .as_ref()
            .unwrap()
            .comments
            .iter()
            .find(|c| c.comment_id == srv.c1_id)
            .unwrap();
        assert_eq!(c1.viewer_vote, 1, "T10-07: cross-route comment vote must survive!");
    });

    // ------------------------------------------------------------------
    // T10-08 (C1 Unchallenged #2 — recommended_actors Case B):
    // Slow bootstrap_initial_load (search_delay=260ms) returning bob(F,F,5)
    // vs fast settled toggle_follow_actor(bob, true) + toggle_mute_actor(bob, false)
    // ------------------------------------------------------------------
    srv.bob_blocked.store(false, Ordering::SeqCst);
    srv.bob_following.store(false, Ordering::SeqCst);
    srv.bob_muted.store(false, Ordering::SeqCst);
    srv.bob_followers.store(5, Ordering::SeqCst);
    let sr_rx = srv.search_received.load(Ordering::SeqCst);
    let sr_done = srv.search_completed.load(Ordering::SeqCst);
    let fl_8 = srv.follow_completed.load(Ordering::SeqCst);
    let mt_8 = srv.mute_completed.load(Ordering::SeqCst);
    srv.search_delay_ms.store(260, Ordering::SeqCst);
    let _ = cw.update(app, |w, cx| {
        w.bootstrap_initial_load(cx);
    });
    assert!(
        pump_until(app, Duration::from_secs(4), |cx| {
            cw.read(cx).state.recommended_actors_loading
                && srv.search_received.load(Ordering::SeqCst) > sr_rx
        })
        .await,
        "T10-08: bootstrap_initial_load search must enter delay while recommended_actors_loading"
    );
    let _ = cw.update(app, |w, cx| {
        w.toggle_follow_actor(srv.bob_id.clone(), true, cx);
        w.toggle_mute_actor(srv.bob_id.clone(), false, cx);
    });
    assert!(
        pump_until(app, Duration::from_secs(4), |cx| {
            let st = &cw.read(cx).state;
            st.actor_follow_in_flight.is_empty()
                && st.actor_mute_in_flight.is_empty()
                && srv.follow_completed.load(Ordering::SeqCst) > fl_8
                && srv.mute_completed.load(Ordering::SeqCst) > mt_8
        })
        .await,
        "T10-08: fast Follow+Mute must settle while bootstrap recommended_actors is loading"
    );
    assert!(
        pump_until(app, Duration::from_secs(5), |cx| {
            !cw.read(cx).state.recommended_actors_loading
                && srv.search_completed.load(Ordering::SeqCst) >= sr_done + 3
        })
        .await,
        "T10-08: Case B reload_recommended_actors must settle"
    );
    let _ = cw.read_with(app, |w, _| {
        let bob = w
            .state
            .recommended_actors
            .iter()
            .find(|a| a.actor_id == srv.bob_id)
            .expect("bob in recommended_actors");
        assert!(
            bob.is_following && bob.is_muted && bob.followers_count == 6,
            "T10-08: recommended_actors must converge to settled Follow+Mute (true, true, 6), got {bob:?}"
        );
    });
}

#[test]
fn test_m5_r11_identity_switch_seq_decoupling_and_cross_route_deletes() {
    run_in_headless_subprocess("test_m5_r11_identity_switch_seq_decoupling_and_cross_route_deletes");
}

struct R11MockState {
    alice_id: String,
    bot_a_id: String,
    bob_id: String,
    slug_rust: String,
    slug_python: String,
    p_part1: String,
    p_part2: String,
    p_quoted: String,
    p_mod_rem: String,
    p_mod_lock: String,
    c_leaf: String,
    c_leaf2: String,
    c_parent: String,
    c_child: String,
    c_edit: String,

    display_names: std::sync::Mutex<std::collections::HashMap<String, String>>,
    registered_bots: std::sync::Mutex<Vec<CommunityActorDto>>,
    follows: std::sync::Mutex<std::collections::HashSet<(String, String)>>,
    blocks: std::sync::Mutex<std::collections::HashSet<(String, String)>>,
    mutes: std::sync::Mutex<std::collections::HashSet<(String, String)>>,

    comm_wiki: std::sync::Mutex<std::collections::HashMap<String, (String, u32)>>,
    comm_wiki_revs: std::sync::Mutex<
        std::collections::HashMap<
            String,
            Vec<gpui_telegram_proto::community_contracts::CommunityWikiRevisionDto>,
        >,
    >,
    comm_rules: std::sync::Mutex<
        std::collections::HashMap<
            String,
            Vec<gpui_telegram_proto::community_contracts::CommunityRuleDto>,
        >,
    >,
    comm_automod: std::sync::Mutex<std::collections::HashMap<String, Vec<AutoModRuleDto>>>,

    deleted_posts: std::sync::Mutex<std::collections::HashSet<String>>,
    locked_posts: std::sync::Mutex<std::collections::HashSet<String>>,
    removed_posts: std::sync::Mutex<std::collections::HashSet<String>>,
    comment_bodies: std::sync::Mutex<std::collections::HashMap<String, String>>,
    deleted_comments: std::sync::Mutex<std::collections::HashSet<String>>,
    comment_updated_ts: std::sync::atomic::AtomicI64,

    // Deterministic delay knobs & atomic counters
    identities_get_received: std::sync::atomic::AtomicUsize,
    actor_patch_pre_delays_by_name: std::sync::Mutex<std::collections::HashMap<String, u64>>,
    actor_patch_received: std::sync::atomic::AtomicUsize,
    actor_patch_completed: std::sync::atomic::AtomicUsize,
    register_bot_pre_delay_ms: std::sync::atomic::AtomicU64,
    register_bot_received: std::sync::atomic::AtomicUsize,
    register_bot_completed: std::sync::atomic::AtomicUsize,
    profile_get_post_delay_ms: std::sync::atomic::AtomicU64,
    profile_get_received: std::sync::atomic::AtomicUsize,
    profile_get_completed: std::sync::atomic::AtomicUsize,
    search_pre_delay_ms: std::sync::atomic::AtomicU64,
    search_received: std::sync::atomic::AtomicUsize,
    search_completed: std::sync::atomic::AtomicUsize,
    follow_calls: std::sync::atomic::AtomicUsize,
    follow_completed: std::sync::atomic::AtomicUsize,
    block_completed: std::sync::atomic::AtomicUsize,

    wiki_pre_delays_by_md: std::sync::Mutex<std::collections::HashMap<String, u64>>,
    wiki_post_delays_by_md: std::sync::Mutex<std::collections::HashMap<String, u64>>,
    wiki_put_received: std::sync::atomic::AtomicUsize,
    wiki_put_completed: std::sync::atomic::AtomicUsize,
    rules_pre_delays_by_last_title: std::sync::Mutex<std::collections::HashMap<String, u64>>,
    rules_post_delays_by_last_title: std::sync::Mutex<std::collections::HashMap<String, u64>>,
    rules_patch_received: std::sync::atomic::AtomicUsize,
    rules_patch_completed: std::sync::atomic::AtomicUsize,
    automod_pre_delay_ms: std::sync::atomic::AtomicU64,
    automod_patch_received: std::sync::atomic::AtomicUsize,
    automod_patch_completed: std::sync::atomic::AtomicUsize,

    comment_patch_pre_delays_by_body: std::sync::Mutex<std::collections::HashMap<String, u64>>,
    comment_patch_post_delays_by_body: std::sync::Mutex<std::collections::HashMap<String, u64>>,
    comment_patch_received: std::sync::atomic::AtomicUsize,
    comment_patch_completed: std::sync::atomic::AtomicUsize,

    delete_post_pre_delay_ms: std::sync::atomic::AtomicU64,
    delete_post_received: std::sync::atomic::AtomicUsize,
    delete_post_completed: std::sync::atomic::AtomicUsize,
    delete_comment_pre_delay_ms: std::sync::atomic::AtomicU64,
    delete_comment_received: std::sync::atomic::AtomicUsize,
    delete_comment_completed: std::sync::atomic::AtomicUsize,
    detail_get_post_delay_ms: std::sync::atomic::AtomicU64,
    detail_get_received: std::sync::atomic::AtomicUsize,
    detail_get_completed: std::sync::atomic::AtomicUsize,
    mod_action_completed: std::sync::atomic::AtomicUsize,
}

impl R11MockState {
    fn build_actor(&self, actor_id: &str, viewer: &str) -> CommunityActorDto {
        if let Some(reg) = self
            .registered_bots
            .lock()
            .unwrap()
            .iter()
            .find(|b| b.actor_id == actor_id)
            .cloned()
        {
            return reg;
        }
        let is_bot = actor_id.starts_with("b:");
        let handle = if actor_id == self.alice_id {
            "r11_alice"
        } else if actor_id == self.bot_a_id {
            "r11_bot_a"
        } else {
            "r11_bob"
        };
        let display_name = self
            .display_names
            .lock()
            .unwrap()
            .get(actor_id)
            .cloned()
            .unwrap_or_else(|| handle.to_string());
        let follows = self.follows.lock().unwrap();
        let blocks = self.blocks.lock().unwrap();
        let mutes = self.mutes.lock().unwrap();
        let is_following = follows.contains(&(viewer.to_string(), actor_id.to_string()));
        let is_blocked = blocks.contains(&(viewer.to_string(), actor_id.to_string()));
        let is_muted = mutes.contains(&(viewer.to_string(), actor_id.to_string()));
        let followers_count = follows.iter().filter(|(_, tgt)| tgt == actor_id).count() as u64;
        let mut a = mock_actor_dto(
            actor_id,
            if is_bot { ActorKind::Bot } else { ActorKind::Human },
            handle,
            &display_name,
        );
        a.bio = "Wave r11 citizen".to_string();
        a.linked_user_id = Some(911001);
        if is_bot {
            a.owner_user_id = Some(911001);
        }
        a.is_following = is_following;
        a.is_blocked = is_blocked;
        a.is_muted = is_muted;
        a.followers_count = followers_count;
        a
    }

    fn build_community(&self, slug: &str, viewer: &str) -> SubCommunityDto {
        let (wiki_markdown, wiki_revision) = self
            .comm_wiki
            .lock()
            .unwrap()
            .get(slug)
            .cloned()
            .unwrap_or_else(|| ("# Initial Wiki".to_string(), 1));
        let rules = self
            .comm_rules
            .lock()
            .unwrap()
            .get(slug)
            .cloned()
            .unwrap_or_default();
        let automod_rules = self
            .comm_automod
            .lock()
            .unwrap()
            .get(slug)
            .cloned()
            .unwrap_or_default();
        let is_owner = viewer == self.alice_id;
        let role = if is_owner {
            Some(CommunityMemberRole::Owner)
        } else {
            None
        };
        let mut c = mock_community_dto(slug, &self.alice_id, role);
        c.rules = rules;
        c.automod_rules = automod_rules;
        c.wiki_markdown = wiki_markdown;
        c.wiki_revision = wiki_revision;
        c.member_count = 12;
        c.is_joined = is_owner;
        c.allow_bots = true;
        c
    }

    fn build_post(&self, post_id: &str, viewer: &str) -> CommunityPostDto {
        let is_locked = self.locked_posts.lock().unwrap().contains(post_id);
        let is_removed = self.removed_posts.lock().unwrap().contains(post_id);
        let mut p = mock_post_dto(post_id, &self.alice_id, false, None, 0);
        p.community_slug = Some(self.slug_rust.clone());
        p.author_handle = "r11_alice".to_string();
        p.author_display_name = "Alice R11".to_string();
        p.format = PostFormat::Article;
        p.title = Some(format!("Title {post_id}"));
        p.body = format!("Body {post_id}");
        p.is_locked = is_locked;
        p.is_removed = is_removed;
        p.viewer_vote = if viewer == self.alice_id { 1 } else { 0 };
        p.upvotes = 1;
        p.score = 1;
        p
    }

    fn build_comment(
        &self,
        cid: &str,
        post_id: &str,
        children: Vec<CommunityCommentDto>,
    ) -> CommunityCommentDto {
        let is_del = self.deleted_comments.lock().unwrap().contains(cid);
        let body = if is_del {
            "[deleted]".to_string()
        } else {
            self.comment_bodies
                .lock()
                .unwrap()
                .get(cid)
                .cloned()
                .unwrap_or_else(|| format!("Body {cid}"))
        };
        CommunityCommentDto {
            comment_id: cid.to_string(),
            post_id: post_id.to_string(),
            parent_comment_id: None,
            depth: 0,
            path: cid.to_string(),
            author_actor_id: self.alice_id.clone(),
            author_handle: "r11_alice".to_string(),
            author_display_name: "Alice R11".to_string(),
            author_kind: ActorKind::Human,
            author_verified_tier: VerifiedTier::None,
            author_bot_badge: None,
            author_avatar_shape: "circle".to_string(),
            author_avatar_color: "blue".to_string(),
            is_op: true,
            body,
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
            is_removed: is_del,
            is_deleted: is_del,
            children,
            more_children_count: 0,
            created_at_ms: 1_750_000_000_000,
            updated_at_ms: self.comment_updated_ts.load(std::sync::atomic::Ordering::SeqCst),
        }
    }

    fn build_post_detail_comments(&self, post_id: &str) -> Vec<CommunityCommentDto> {
        if post_id != self.p_part2 {
            return Vec::new();
        }
        let del = self.deleted_comments.lock().unwrap().clone();
        let mut out = Vec::new();
        if !del.contains(&self.c_edit) {
            out.push(self.build_comment(&self.c_edit, post_id, Vec::new()));
        }
        if !del.contains(&self.c_leaf) {
            out.push(self.build_comment(&self.c_leaf, post_id, Vec::new()));
        }
        if !del.contains(&self.c_leaf2) {
            out.push(self.build_comment(&self.c_leaf2, post_id, Vec::new()));
        }
        let child = if !del.contains(&self.c_child) {
            vec![self.build_comment(&self.c_child, post_id, Vec::new())]
        } else {
            Vec::new()
        };
        // Parent stays tombstoned when deleted if child exists!
        if !del.contains(&self.c_parent) || !child.is_empty() {
            out.push(self.build_comment(&self.c_parent, post_id, child));
        }
        out
    }
}

fn spawn_r11_server(
    rt: &tokio::runtime::Handle,
) -> (std::net::SocketAddr, Arc<R11MockState>, AbortOnDrop) {
    use gpui_telegram_proto::community_contracts::{
        CommunityRuleDto, CommunityWikiRevisionDto, FollowActionResponse, ModActionRequest,
        ModLogEntryDto, RegisterBotActorRequest, UpdateCommentRequest, UpdateCommunityActorRequest,
        UpdateCommunityRequest, UpdateCommunityWikiRequest,
    };
    use std::sync::atomic::{AtomicI64, AtomicU64, AtomicUsize, Ordering};
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    let mut display_names = std::collections::HashMap::new();
    display_names.insert("u:911001".to_string(), "Alice V1".to_string());
    display_names.insert("b:911002".to_string(), "Bot V1".to_string());
    display_names.insert("u:911300".to_string(), "Bob R11".to_string());

    let mut comm_wiki = std::collections::HashMap::new();
    comm_wiki.insert("r11_rust".to_string(), ("# Rust Wiki V1".to_string(), 1));
    comm_wiki.insert("r11_python".to_string(), ("# Python Wiki V1".to_string(), 1));

    let mut comm_rules = std::collections::HashMap::new();
    comm_rules.insert(
        "r11_rust".to_string(),
        vec![CommunityRuleDto {
            id: "r11_rule_1".to_string(),
            title: "Rule One".to_string(),
            description: "Be kind".to_string(),
            order: 1,
        }],
    );
    comm_rules.insert("r11_python".to_string(), Vec::new());

    let mut comm_automod = std::collections::HashMap::new();
    comm_automod.insert(
        "r11_rust".to_string(),
        vec![AutoModRuleDto {
            rule_id: "am_r11_1".to_string(),
            community_id: "comm_r11_rust".to_string(),
            name: "Spam Guard R11".to_string(),
            enabled: false,
            priority: 10,
            keywords: vec!["spam".to_string()],
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
        }],
    );
    comm_automod.insert("r11_python".to_string(), Vec::new());

    let mut comment_bodies = std::collections::HashMap::new();
    comment_bodies.insert("cm_r11_edit".to_string(), "Edit Target Initial".to_string());
    comment_bodies.insert("cm_r11_leaf".to_string(), "Leaf Comment 1".to_string());
    comment_bodies.insert("cm_r11_leaf2".to_string(), "Leaf Comment 2".to_string());
    comment_bodies.insert("cm_r11_parent".to_string(), "Parent Comment".to_string());
    comment_bodies.insert("cm_r11_child".to_string(), "Child Comment".to_string());

    let state = Arc::new(R11MockState {
        alice_id: "u:911001".to_string(),
        bot_a_id: "b:911002".to_string(),
        bob_id: "u:911300".to_string(),
        slug_rust: "r11_rust".to_string(),
        slug_python: "r11_python".to_string(),
        p_part1: "post_r11_part1".to_string(),
        p_part2: "post_r11_part2".to_string(),
        p_quoted: "post_r11_quoted".to_string(),
        p_mod_rem: "post_r11_mod_rem".to_string(),
        p_mod_lock: "post_r11_mod_lock".to_string(),
        c_leaf: "cm_r11_leaf".to_string(),
        c_leaf2: "cm_r11_leaf2".to_string(),
        c_parent: "cm_r11_parent".to_string(),
        c_child: "cm_r11_child".to_string(),
        c_edit: "cm_r11_edit".to_string(),
        display_names: std::sync::Mutex::new(display_names),
        registered_bots: std::sync::Mutex::new(Vec::new()),
        follows: std::sync::Mutex::new(std::collections::HashSet::new()),
        blocks: std::sync::Mutex::new(std::collections::HashSet::new()),
        mutes: std::sync::Mutex::new(std::collections::HashSet::new()),
        comm_wiki: std::sync::Mutex::new(comm_wiki),
        comm_wiki_revs: std::sync::Mutex::new(std::collections::HashMap::new()),
        comm_rules: std::sync::Mutex::new(comm_rules),
        comm_automod: std::sync::Mutex::new(comm_automod),
        deleted_posts: std::sync::Mutex::new(std::collections::HashSet::new()),
        locked_posts: std::sync::Mutex::new(std::collections::HashSet::new()),
        removed_posts: std::sync::Mutex::new(std::collections::HashSet::new()),
        comment_bodies: std::sync::Mutex::new(comment_bodies),
        deleted_comments: std::sync::Mutex::new(std::collections::HashSet::new()),
        comment_updated_ts: AtomicI64::new(1_750_000_000_000),
        identities_get_received: AtomicUsize::new(0),
        actor_patch_pre_delays_by_name: std::sync::Mutex::new(std::collections::HashMap::new()),
        actor_patch_received: AtomicUsize::new(0),
        actor_patch_completed: AtomicUsize::new(0),
        register_bot_pre_delay_ms: AtomicU64::new(0),
        register_bot_received: AtomicUsize::new(0),
        register_bot_completed: AtomicUsize::new(0),
        profile_get_post_delay_ms: AtomicU64::new(0),
        profile_get_received: AtomicUsize::new(0),
        profile_get_completed: AtomicUsize::new(0),
        search_pre_delay_ms: AtomicU64::new(0),
        search_received: AtomicUsize::new(0),
        search_completed: AtomicUsize::new(0),
        follow_calls: AtomicUsize::new(0),
        follow_completed: AtomicUsize::new(0),
        block_completed: AtomicUsize::new(0),
        wiki_pre_delays_by_md: std::sync::Mutex::new(std::collections::HashMap::new()),
        wiki_post_delays_by_md: std::sync::Mutex::new(std::collections::HashMap::new()),
        wiki_put_received: AtomicUsize::new(0),
        wiki_put_completed: AtomicUsize::new(0),
        rules_pre_delays_by_last_title: std::sync::Mutex::new(std::collections::HashMap::new()),
        rules_post_delays_by_last_title: std::sync::Mutex::new(std::collections::HashMap::new()),
        rules_patch_received: AtomicUsize::new(0),
        rules_patch_completed: AtomicUsize::new(0),
        automod_pre_delay_ms: AtomicU64::new(0),
        automod_patch_received: AtomicUsize::new(0),
        automod_patch_completed: AtomicUsize::new(0),
        comment_patch_pre_delays_by_body: std::sync::Mutex::new(std::collections::HashMap::new()),
        comment_patch_post_delays_by_body: std::sync::Mutex::new(std::collections::HashMap::new()),
        comment_patch_received: AtomicUsize::new(0),
        comment_patch_completed: AtomicUsize::new(0),
        delete_post_pre_delay_ms: AtomicU64::new(0),
        delete_post_received: AtomicUsize::new(0),
        delete_post_completed: AtomicUsize::new(0),
        delete_comment_pre_delay_ms: AtomicU64::new(0),
        delete_comment_received: AtomicUsize::new(0),
        delete_comment_completed: AtomicUsize::new(0),
        detail_get_post_delay_ms: AtomicU64::new(0),
        detail_get_received: AtomicUsize::new(0),
        detail_get_completed: AtomicUsize::new(0),
        mod_action_completed: AtomicUsize::new(0),
    });

    let std_listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind r11 server");
    std_listener.set_nonblocking(true).expect("nonblocking");
    let addr = std_listener.local_addr().expect("r11 local_addr");
    let st_accept = Arc::clone(&state);
    let join = rt.spawn(async move {
        let listener = tokio::net::TcpListener::from_std(std_listener).expect("tokio listener");
        loop {
            let Ok((mut sock, _)) = listener.accept().await else {
                break;
            };
            let st = Arc::clone(&st_accept);
            tokio::spawn(async move {
                let mut buf = Vec::with_capacity(8192);
                let mut tmp = [0u8; 4096];
                let mut header_end = None;
                let mut content_len = 0usize;
                loop {
                    let Ok(n) = sock.read(&mut tmp).await else {
                        return;
                    };
                    if n == 0 {
                        return;
                    }
                    buf.extend_from_slice(&tmp[..n]);
                    if header_end.is_none() {
                        if let Some(pos) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
                            header_end = Some(pos + 4);
                            let hdr = String::from_utf8_lossy(&buf[..pos]);
                            for line in hdr.lines() {
                                if let Some((k, v)) = line.split_once(':') {
                                    if k.eq_ignore_ascii_case("content-length") {
                                        content_len = v.trim().parse().unwrap_or(0);
                                    }
                                }
                            }
                        }
                    }
                    if let Some(he) = header_end {
                        if buf.len() >= he + content_len {
                            break;
                        }
                    }
                }
                let he = header_end.unwrap();
                let hdr_str = String::from_utf8_lossy(&buf[..he]).to_string();
                let mut lines = hdr_str.lines();
                let req_line = lines.next().unwrap_or_default();
                let mut parts = req_line.split_whitespace();
                let method = parts.next().unwrap_or("GET").to_string();
                let raw_target = parts.next().unwrap_or("/").to_string();
                let (raw_path, _query) = raw_target
                    .split_once('?')
                    .map(|(p, q)| (p.to_string(), q.to_string()))
                    .unwrap_or((raw_target, String::new()));
                let path = raw_path.replace("%3A", ":").replace("%3a", ":");
                let mut acting_actor: Option<String> = None;
                for line in lines {
                    if let Some((k, v)) = line.split_once(':') {
                        if k.eq_ignore_ascii_case("x-community-actor-id") {
                            acting_actor = Some(v.trim().to_string());
                        }
                    }
                }
                let viewer = acting_actor.unwrap_or_else(|| st.alice_id.clone());
                let body_bytes = &buf[he..he + content_len];

                let (status, resp_body): (&str, String) = if method == "GET"
                    && path == "/api/v1/community/identities"
                {
                    st.identities_get_received.fetch_add(1, Ordering::SeqCst);
                    let human = st.build_actor(&st.alice_id, &viewer);
                    let mut bot_actors = vec![st.build_actor(&st.bot_a_id, &viewer)];
                    bot_actors.extend(st.registered_bots.lock().unwrap().iter().cloned());
                    let mut all_actors = vec![human.clone()];
                    all_actors.extend(bot_actors.iter().cloned());
                    let resp = CommunityIdentitiesResponse {
                        active_actor_id: viewer.clone(),
                        human_actor: Some(human),
                        bot_actors,
                        all_actors,
                    };
                    ("200 OK", serde_json::to_string(&resp).unwrap())
                } else if method == "POST" && path == "/api/v1/community/identities/bot" {
                    let req: RegisterBotActorRequest =
                        serde_json::from_slice(body_bytes).expect("r11 reg bot");
                    st.register_bot_received.fetch_add(1, Ordering::SeqCst);
                    let pre = st.register_bot_pre_delay_ms.swap(0, Ordering::SeqCst);
                    if pre > 0 {
                        tokio::time::sleep(Duration::from_millis(pre)).await;
                    }
                    let mut actor = mock_actor_dto(
                        &format!("b:r11_{}", req.handle),
                        ActorKind::Bot,
                        &req.handle,
                        &req.display_name,
                    );
                    actor.bio = req.bio.unwrap_or_default();
                    actor.avatar_shape = req.avatar_shape.unwrap_or_else(|| "orb".to_string());
                    actor.avatar_color = req.avatar_color.unwrap_or_else(|| "indigo".to_string());
                    actor.owner_user_id = Some(911001);
                    actor.linked_user_id = Some(911999);
                    actor.bot_badge = req.bot_badge;
                    actor.bot_model = req.bot_model;
                    actor.bot_capabilities = req.bot_capabilities;
                    actor.bot_autonomy_mode = req.bot_autonomy_mode;
                    actor.bot_persona = req.bot_persona;
                    st.registered_bots.lock().unwrap().push(actor.clone());
                    st.register_bot_completed.fetch_add(1, Ordering::SeqCst);
                    let payload = serde_json::json!({
                        "actor": actor,
                        "bot_token": "tok_r11_new",
                        "bot_user_id": 911999,
                        "username": req.handle,
                    });
                    ("200 OK", payload.to_string())
                } else if method == "GET" && path == "/api/v1/community/algo-prefs" {
                    ("200 OK", serde_json::to_string(&AlgoPreferencesDto::default()).unwrap())
                } else if method == "GET" && path == "/api/v1/community/trending" {
                    ("200 OK", serde_json::to_string(&TrendingResponse::default()).unwrap())
                } else if method == "GET" && path == "/api/v1/community/notifications" {
                    ("200 OK", serde_json::to_string(&NotificationsListResponse::default()).unwrap())
                } else if method == "GET" && path == "/api/v1/community/lists" {
                    ("200 OK", serde_json::to_string(&CommunityListsResponse::default()).unwrap())
                } else if method == "GET" && path == "/api/v1/community/search" {
                    st.search_received.fetch_add(1, Ordering::SeqCst);
                    let pre = st.search_pre_delay_ms.swap(0, Ordering::SeqCst);
                    if pre > 0 {
                        tokio::time::sleep(Duration::from_millis(pre)).await;
                    }
                    let snap = CommunitySearchResultDto {
                        query: String::new(),
                        tab: CommunitySearchTab::Top,
                        posts: vec![st.build_post(&st.p_part2, &viewer)],
                        actors: vec![
                            st.build_actor(&st.bot_a_id, &viewer),
                            st.build_actor(&st.bob_id, &viewer),
                        ],
                        communities: vec![
                            st.build_community(&st.slug_rust, &viewer),
                            st.build_community(&st.slug_python, &viewer),
                        ],
                        total_hits: 2,
                        next_offset: None,
                    };
                    st.search_completed.fetch_add(1, Ordering::SeqCst);
                    ("200 OK", serde_json::to_string(&snap).unwrap())
                } else if method == "GET" && path == "/api/v1/community/communities" {
                    let resp = ListCommunitiesResponse {
                        communities: vec![
                            st.build_community(&st.slug_rust, &viewer),
                            st.build_community(&st.slug_python, &viewer),
                        ],
                    };
                    ("200 OK", serde_json::to_string(&resp).unwrap())
                } else if method == "GET"
                    && path.starts_with("/api/v1/community/communities/")
                    && !path.ends_with("/mod-queue")
                    && !path.ends_with("/mod-logs")
                {
                    let slug = path
                        .trim_start_matches("/api/v1/community/communities/")
                        .to_string();
                    let wiki_revisions = st
                        .comm_wiki_revs
                        .lock()
                        .unwrap()
                        .get(&slug)
                        .cloned()
                        .unwrap_or_default();
                    let resp = CommunityDetailResponse {
                        community: st.build_community(&slug, &viewer),
                        pinned_posts: Vec::new(),
                        moderators: vec![
                            st.build_actor(&st.alice_id, &viewer),
                            st.build_actor(&st.bob_id, &viewer),
                        ],
                        wiki_revisions,
                    };
                    ("200 OK", serde_json::to_string(&resp).unwrap())
                } else if method == "PUT"
                    && path.starts_with("/api/v1/community/communities/")
                    && path.ends_with("/wiki")
                {
                    let slug = path
                        .trim_start_matches("/api/v1/community/communities/")
                        .trim_end_matches("/wiki")
                        .to_string();
                    let req: UpdateCommunityWikiRequest =
                        serde_json::from_slice(body_bytes).expect("r11 wiki");
                    st.wiki_put_received.fetch_add(1, Ordering::SeqCst);
                    let pre = st
                        .wiki_pre_delays_by_md
                        .lock()
                        .unwrap()
                        .remove(&req.markdown)
                        .unwrap_or(0);
                    if pre > 0 {
                        tokio::time::sleep(Duration::from_millis(pre)).await;
                    }
                    let rev_num = {
                        let mut wk = st.comm_wiki.lock().unwrap();
                        let next_rev = wk.get(&slug).map(|x| x.1 + 1).unwrap_or(2);
                        wk.insert(slug.clone(), (req.markdown.clone(), next_rev));
                        next_rev
                    };
                    let rev = CommunityWikiRevisionDto {
                        revision: rev_num,
                        editor_actor_id: viewer.clone(),
                        editor_handle: "r11_alice".to_string(),
                        markdown: req.markdown.clone(),
                        summary: req.summary,
                        created_at_ms: 1_750_000_005_000 + i64::from(rev_num),
                    };
                    st.comm_wiki_revs
                        .lock()
                        .unwrap()
                        .entry(slug)
                        .or_default()
                        .insert(0, rev.clone());
                    let post = st
                        .wiki_post_delays_by_md
                        .lock()
                        .unwrap()
                        .remove(&req.markdown)
                        .unwrap_or(0);
                    if post > 0 {
                        tokio::time::sleep(Duration::from_millis(post)).await;
                    }
                    st.wiki_put_completed.fetch_add(1, Ordering::SeqCst);
                    ("200 OK", serde_json::to_string(&rev).unwrap())
                } else if method == "PATCH" && path.starts_with("/api/v1/community/communities/") {
                    let slug = path
                        .trim_start_matches("/api/v1/community/communities/")
                        .to_string();
                    let req: UpdateCommunityRequest =
                        serde_json::from_slice(body_bytes).expect("r11 patch comm");
                    if let Some(am) = req.automod_rules {
                        st.automod_patch_received.fetch_add(1, Ordering::SeqCst);
                        let pre = st.automod_pre_delay_ms.swap(0, Ordering::SeqCst);
                        if pre > 0 {
                            tokio::time::sleep(Duration::from_millis(pre)).await;
                        }
                        st.comm_automod.lock().unwrap().insert(slug.clone(), am);
                        let snap = st.build_community(&slug, &viewer);
                        st.automod_patch_completed.fetch_add(1, Ordering::SeqCst);
                        ("200 OK", serde_json::to_string(&snap).unwrap())
                    } else {
                        let rules = req.rules.unwrap_or_default();
                        let last_title =
                            rules.last().map(|r| r.title.clone()).unwrap_or_default();
                        st.rules_patch_received.fetch_add(1, Ordering::SeqCst);
                        let pre = st
                            .rules_pre_delays_by_last_title
                            .lock()
                            .unwrap()
                            .remove(&last_title)
                            .unwrap_or(0);
                        if pre > 0 {
                            tokio::time::sleep(Duration::from_millis(pre)).await;
                        }
                        st.comm_rules.lock().unwrap().insert(slug.clone(), rules);
                        let snap = st.build_community(&slug, &viewer);
                        let post = st
                            .rules_post_delays_by_last_title
                            .lock()
                            .unwrap()
                            .remove(&last_title)
                            .unwrap_or(0);
                        if post > 0 {
                            tokio::time::sleep(Duration::from_millis(post)).await;
                        }
                        st.rules_patch_completed.fetch_add(1, Ordering::SeqCst);
                        ("200 OK", serde_json::to_string(&snap).unwrap())
                    }
                } else if method == "GET" && path.ends_with("/mod-queue") {
                    ("200 OK", serde_json::to_string(&ModQueueResponse::default()).unwrap())
                } else if method == "GET" && path.ends_with("/mod-logs") {
                    ("200 OK", serde_json::to_string(&ModLogsResponse::default()).unwrap())
                } else if method == "POST" && path.ends_with("/mod-actions") {
                    let req: ModActionRequest =
                        serde_json::from_slice(body_bytes).expect("r11 mod action");
                    if req.target_kind.eq_ignore_ascii_case("post") {
                        match req.action {
                            ModActionKind::Remove => {
                                st.removed_posts.lock().unwrap().insert(req.target_id.clone());
                            }
                            ModActionKind::Lock => {
                                st.locked_posts.lock().unwrap().insert(req.target_id.clone());
                            }
                            _ => {}
                        }
                    }
                    st.mod_action_completed.fetch_add(1, Ordering::SeqCst);
                    let entry = ModLogEntryDto {
                        log_id: "ml_r11_1".to_string(),
                        community_id: "comm_r11_rust".to_string(),
                        moderator_actor_id: viewer,
                        moderator_handle: "r11_alice".to_string(),
                        moderator_kind: ActorKind::Human,
                        action: req.action,
                        target_kind: req.target_kind,
                        target_id: req.target_id,
                        reason: req.reason,
                        rule_triggered: None,
                        created_at_ms: 1_750_000_020_000,
                    };
                    ("200 OK", serde_json::to_string(&entry).unwrap())
                } else if method == "GET" && path == "/api/v1/community/feed" {
                    let del = st.deleted_posts.lock().unwrap().clone();
                    let rem = st.removed_posts.lock().unwrap().clone();
                    let all_pids = [
                        st.p_part1.as_str(),
                        st.p_part2.as_str(),
                        st.p_quoted.as_str(),
                        st.p_mod_rem.as_str(),
                        st.p_mod_lock.as_str(),
                    ];
                    let items: Vec<FeedItemDto> = all_pids
                        .into_iter()
                        .filter(|pid| !del.contains(*pid) && !rem.contains(*pid))
                        .map(|pid| FeedItemDto {
                            post: st.build_post(pid, &viewer),
                            rank_score: 100.0,
                            explanation: None,
                            is_exploration_slot: false,
                        })
                        .collect();
                    let resp = FeedResponseDto {
                        items,
                        next_cursor: None,
                        ..Default::default()
                    };
                    ("200 OK", serde_json::to_string(&resp).unwrap())
                } else if method == "GET" && path.starts_with("/api/v1/community/posts/") {
                    let pid = path.trim_start_matches("/api/v1/community/posts/").to_string();
                    st.detail_get_received.fetch_add(1, Ordering::SeqCst);
                    let del = st.deleted_posts.lock().unwrap().clone();
                    let resp = if pid == st.p_part2 {
                        let mut thread_chain = Vec::new();
                        if !del.contains(&st.p_part1) {
                            thread_chain.push(st.build_post(&st.p_part1, &viewer));
                        }
                        thread_chain.push(st.build_post(&st.p_part2, &viewer));
                        let mut quote_chain = Vec::new();
                        if !del.contains(&st.p_quoted) {
                            quote_chain.push(st.build_post(&st.p_quoted, &viewer));
                        }
                        let comments = st.build_post_detail_comments(&pid);
                        let total_comments = comments.len() as u64;
                        PostDetailResponse {
                            post: st.build_post(&pid, &viewer),
                            thread_chain,
                            quote_chain,
                            comments,
                            total_comments,
                        }
                    } else {
                        PostDetailResponse {
                            post: st.build_post(&pid, &viewer),
                            thread_chain: Vec::new(),
                            quote_chain: Vec::new(),
                            comments: Vec::new(),
                            total_comments: 0,
                        }
                    };
                    let post = st.detail_get_post_delay_ms.swap(0, Ordering::SeqCst);
                    if post > 0 {
                        tokio::time::sleep(Duration::from_millis(post)).await;
                    }
                    st.detail_get_completed.fetch_add(1, Ordering::SeqCst);
                    ("200 OK", serde_json::to_string(&resp).unwrap())
                } else if method == "DELETE" && path.starts_with("/api/v1/community/posts/") {
                    let pid = path.trim_start_matches("/api/v1/community/posts/").to_string();
                    st.delete_post_received.fetch_add(1, Ordering::SeqCst);
                    let pre = st.delete_post_pre_delay_ms.swap(0, Ordering::SeqCst);
                    if pre > 0 {
                        tokio::time::sleep(Duration::from_millis(pre)).await;
                    }
                    st.deleted_posts.lock().unwrap().insert(pid.clone());
                    st.delete_post_completed.fetch_add(1, Ordering::SeqCst);
                    (
                        "200 OK",
                        serde_json::to_string(&DeleteEntityResponse {
                            id: pid,
                            deleted: true,
                        })
                        .unwrap(),
                    )
                } else if method == "PATCH" && path.starts_with("/api/v1/community/comments/") {
                    let cid = path
                        .trim_start_matches("/api/v1/community/comments/")
                        .to_string();
                    let req: UpdateCommentRequest =
                        serde_json::from_slice(body_bytes).expect("r11 patch cm");
                    st.comment_patch_received.fetch_add(1, Ordering::SeqCst);
                    let pre = st
                        .comment_patch_pre_delays_by_body
                        .lock()
                        .unwrap()
                        .remove(&req.body)
                        .unwrap_or(0);
                    if pre > 0 {
                        tokio::time::sleep(Duration::from_millis(pre)).await;
                    }
                    st.comment_bodies
                        .lock()
                        .unwrap()
                        .insert(cid.clone(), req.body.clone());
                    st.comment_updated_ts.fetch_add(500, Ordering::SeqCst);
                    let snap = st.build_comment(&cid, &st.p_part2, Vec::new());
                    let post = st
                        .comment_patch_post_delays_by_body
                        .lock()
                        .unwrap()
                        .remove(&req.body)
                        .unwrap_or(0);
                    if post > 0 {
                        tokio::time::sleep(Duration::from_millis(post)).await;
                    }
                    st.comment_patch_completed.fetch_add(1, Ordering::SeqCst);
                    ("200 OK", serde_json::to_string(&snap).unwrap())
                } else if method == "DELETE" && path.starts_with("/api/v1/community/comments/") {
                    let cid = path
                        .trim_start_matches("/api/v1/community/comments/")
                        .to_string();
                    st.delete_comment_received.fetch_add(1, Ordering::SeqCst);
                    let pre = st.delete_comment_pre_delay_ms.swap(0, Ordering::SeqCst);
                    if pre > 0 {
                        tokio::time::sleep(Duration::from_millis(pre)).await;
                    }
                    st.deleted_comments.lock().unwrap().insert(cid.clone());
                    st.delete_comment_completed.fetch_add(1, Ordering::SeqCst);
                    (
                        "200 OK",
                        serde_json::to_string(&DeleteEntityResponse {
                            id: cid,
                            deleted: true,
                        })
                        .unwrap(),
                    )
                } else if method == "GET" && path.starts_with("/api/v1/community/actors/") {
                    let key = path.trim_start_matches("/api/v1/community/actors/");
                    let aid = if key == "r11_bob" || key == st.bob_id {
                        st.bob_id.clone()
                    } else if key == "r11_bot_a" || key == st.bot_a_id {
                        st.bot_a_id.clone()
                    } else {
                        st.alice_id.clone()
                    };
                    st.profile_get_received.fetch_add(1, Ordering::SeqCst);
                    let del_c = st.deleted_comments.lock().unwrap().clone();
                    let recent_comments: Vec<CommunityCommentDto> =
                        [st.c_edit.as_str(), st.c_leaf.as_str(), st.c_leaf2.as_str()]
                            .into_iter()
                            .filter(|cid| !del_c.contains(*cid))
                            .map(|cid| st.build_comment(cid, &st.p_part2, Vec::new()))
                            .collect();
                    let resp = ActorProfileResponse {
                        actor: st.build_actor(&aid, &viewer),
                        recent_posts: vec![st.build_post(&st.p_part2, &viewer)],
                        recent_comments,
                        moderated_communities: Vec::new(),
                    };
                    let post = st.profile_get_post_delay_ms.swap(0, Ordering::SeqCst);
                    if post > 0 {
                        tokio::time::sleep(Duration::from_millis(post)).await;
                    }
                    st.profile_get_completed.fetch_add(1, Ordering::SeqCst);
                    ("200 OK", serde_json::to_string(&resp).unwrap())
                } else if method == "PATCH" && path.starts_with("/api/v1/community/actors/") {
                    let aid = path
                        .trim_start_matches("/api/v1/community/actors/")
                        .to_string();
                    let req: UpdateCommunityActorRequest =
                        serde_json::from_slice(body_bytes).expect("r11 patch actor");
                    st.actor_patch_received.fetch_add(1, Ordering::SeqCst);
                    let pre = req
                        .display_name
                        .as_deref()
                        .and_then(|n| {
                            st.actor_patch_pre_delays_by_name
                                .lock()
                                .unwrap()
                                .remove(n)
                        })
                        .unwrap_or(0);
                    if pre > 0 {
                        tokio::time::sleep(Duration::from_millis(pre)).await;
                    }
                    if let Some(name) = req.display_name {
                        st.display_names.lock().unwrap().insert(aid.clone(), name);
                    }
                    let mut snap = st.build_actor(&aid, &viewer);
                    snap.is_following = false;
                    snap.is_muted = false;
                    snap.is_blocked = false;
                    st.actor_patch_completed.fetch_add(1, Ordering::SeqCst);
                    ("200 OK", serde_json::to_string(&snap).unwrap())
                } else if (method == "POST" || method == "DELETE")
                    && path.starts_with("/api/v1/community/actors/")
                    && path.ends_with("/follow")
                {
                    let target_id = path
                        .trim_start_matches("/api/v1/community/actors/")
                        .trim_end_matches("/follow")
                        .to_string();
                    st.follow_calls.fetch_add(1, Ordering::SeqCst);
                    let want = method == "POST";
                    let is_blocked = st
                        .blocks
                        .lock()
                        .unwrap()
                        .contains(&(viewer.clone(), target_id.clone()));
                    if want && is_blocked {
                        st.follow_completed.fetch_add(1, Ordering::SeqCst);
                        ("403 Forbidden", r#"{"error":"blocked"}"#.to_string())
                    } else {
                        let cnt = {
                            let mut f = st.follows.lock().unwrap();
                            if want {
                                f.insert((viewer.clone(), target_id.clone()));
                            } else {
                                f.remove(&(viewer.clone(), target_id.clone()));
                            }
                            f.iter().filter(|(_, t)| t == &target_id).count() as u64
                        };
                        st.follow_completed.fetch_add(1, Ordering::SeqCst);
                        let resp = FollowActionResponse {
                            actor_id: target_id,
                            is_following: want,
                            followers_count: cnt,
                        };
                        ("200 OK", serde_json::to_string(&resp).unwrap())
                    }
                } else if (method == "POST" || method == "DELETE")
                    && path == "/api/v1/community/mutes-blocks"
                {
                    let req: MuteBlockRequest =
                        serde_json::from_slice(body_bytes).expect("r11 mb");
                    let want = method == "POST";
                    if req.action == MuteBlockActionKind::Block {
                        let mut b = st.blocks.lock().unwrap();
                        if want {
                            b.insert((viewer.clone(), req.target_value.clone()));
                            st.follows
                                .lock()
                                .unwrap()
                                .remove(&(viewer.clone(), req.target_value.clone()));
                        } else {
                            b.remove(&(viewer.clone(), req.target_value.clone()));
                        }
                        st.block_completed.fetch_add(1, Ordering::SeqCst);
                    } else {
                        let mut m = st.mutes.lock().unwrap();
                        if want {
                            m.insert((viewer.clone(), req.target_value.clone()));
                        } else {
                            m.remove(&(viewer.clone(), req.target_value.clone()));
                        }
                    }
                    if want {
                        let dto = CommunityMuteBlockDto {
                            id: "mb_r11_1".to_string(),
                            owner_actor_id: viewer,
                            target_kind: MuteBlockTargetKind::Actor,
                            target_value: req.target_value,
                            action: req.action,
                            reason: None,
                            created_at_ms: 1_750_000_000_000,
                        };
                        ("200 OK", serde_json::to_string(&dto).unwrap())
                    } else {
                        (
                            "200 OK",
                            serde_json::to_string(&DeleteEntityResponse {
                                id: req.target_value,
                                deleted: true,
                            })
                            .unwrap(),
                        )
                    }
                } else {
                    ("404 Not Found", r#"{"error":"r11_not_found"}"#.to_string())
                };

                let http = format!(
                    "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                    resp_body.len(),
                    resp_body
                );
                let _ = sock.write_all(http.as_bytes()).await;
            });
        }
    });

    (addr, state, AbortOnDrop(join))
}

async fn body_r11_identity_switch_seq_decoupling_and_cross_route_deletes(mut app: AsyncApp) {
    let dir = tempfile::TempDir::new().expect("r11 tempdir");
    let rt_dir = tempfile::TempDir::new().expect("r11 rt tempdir");
    let rt_holder = TelegramClient::open_with_dir(
        ClientConfig {
            server_url: "http://127.0.0.1:9".to_string(),
            max_reconnect_attempts: 0,
            ..Default::default()
        },
        rt_dir.path(),
    )
    .expect("open r11 rt holder");
    let (addr, srv, _server_guard) = spawn_r11_server(rt_holder.runtime().handle());

    let client = Arc::new(
        TelegramClient::open_with_dir(
            ClientConfig {
                server_url: format!("http://{addr}"),
                max_reconnect_attempts: 0,
                ..Default::default()
            },
            dir.path(),
        )
        .expect("open r11 client"),
    );
    let auth = test_auth(911001, "r11_alice");
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
        .expect("mount r11 AppShell");
    let _ = shell.update(&mut app, |s, cx| s.open_community_workspace(cx));
    let cw = shell
        .read_with(&app, |s, _| s.community_workspace.clone().unwrap())
        .unwrap();

    assert!(
        pump_until(&mut app, Duration::from_secs(5), |cx| {
            let st = &cw.read(cx).state;
            !st.feed_loading
                && !st.identities_loading
                && !st.communities_loading
                && !st.recommended_actors_loading
                && st.identities.is_some()
                && st.communities.len() == 2
                && !st.recommended_actors.is_empty()
                && st.feed_items.iter().any(|it| it.post.post_id == srv.p_part2)
        })
        .await,
        "R11 workspace must bootstrap cleanly"
    );

    run_r11_t01_to_t03(&mut app, &cw, &srv).await;
    run_r11_t04(&mut app, &cw, &srv).await;
    run_r11_t05_to_t06(&mut app, &cw, &srv).await;

    drop(rt_holder);
}

async fn run_r11_t01_to_t03(
    app: &mut AsyncApp,
    cw: &Entity<CommunityWorkspace>,
    srv: &Arc<R11MockState>,
) {
    use std::sync::atomic::Ordering;

    // ------------------------------------------------------------------
    // T11-01 (R10-R2-1): Cross-actor-switch Case B reload in submit_edit_profile
    // ------------------------------------------------------------------
    let _ = cw.update(app, |w, cx| {
        w.navigate_to(CommunityRoute::ActorProfile("r11_bot_a".to_string()), cx);
    });
    assert!(
        pump_until(app, Duration::from_secs(4), |cx| {
            let st = &cw.read(cx).state;
            !st.profile_loading
                && st
                    .actor_profile
                    .as_ref()
                    .is_some_and(|p| p.actor.actor_id == srv.bot_a_id)
        })
        .await,
        "T11-01 setup: ActorProfile(r11_bot_a) must load"
    );
    let ap_rx1 = srv.actor_patch_received.load(Ordering::SeqCst);
    let ap_done1 = srv.actor_patch_completed.load(Ordering::SeqCst);
    let pg_rx1 = srv.profile_get_received.load(Ordering::SeqCst);
    let pg_done1 = srv.profile_get_completed.load(Ordering::SeqCst);
    srv.actor_patch_pre_delays_by_name
        .lock()
        .unwrap()
        .insert("Bot V2".to_string(), 180);
    srv.profile_get_post_delay_ms.store(380, Ordering::SeqCst);
    let _ = cw.update(app, |w, cx| {
        let a = w.state.actor_profile.as_ref().unwrap().actor.clone();
        w.open_edit_profile_modal(a, cx);
        w.state.edit_profile.display_name = "Bot V2".to_string();
        w.submit_edit_profile(cx);
    });
    assert!(
        pump_until(app, Duration::from_secs(4), |_| {
            srv.actor_patch_received.load(Ordering::SeqCst) > ap_rx1
        })
        .await,
        "T11-01: PATCH /actors/b:911002 must arrive in pre-commit delay"
    );
    let _ = cw.update(app, |w, cx| {
        w.switch_active_actor(srv.bot_a_id.clone(), cx);
    });
    assert!(
        pump_until(app, Duration::from_secs(4), |cx| {
            cw.read(cx).state.profile_loading
                && srv.profile_get_received.load(Ordering::SeqCst) > pg_rx1
        })
        .await,
        "T11-01: switch_active_actor GET #1 must read stale Bot V1 while profile_loading"
    );
    assert!(
        pump_until(app, Duration::from_secs(5), |cx| {
            let st = &cw.read(cx).state;
            !st.profile_loading
                && !st.feed_loading
                && !st.communities_loading
                && !st.recommended_actors_loading
                && srv.actor_patch_completed.load(Ordering::SeqCst) > ap_done1
                && srv.profile_get_completed.load(Ordering::SeqCst) >= pg_done1 + 2
        })
        .await,
        "T11-01: cross-actor-switch submit_edit_profile must fire reload_actor_route_if_loading"
    );
    assert!(
        srv.profile_get_received.load(Ordering::SeqCst) >= pg_rx1 + 2,
        "T11-01 (R10-R2-1): reload_actor_route_if_loading must fire GET #2 after PATCH commits"
    );
    let _ = cw.read_with(app, |w, _| {
        assert_eq!(
            w.state.actor_profile.as_ref().unwrap().actor.display_name,
            "Bot V2",
            "T11-01 (R10-R2-1): actor_profile must converge to Bot V2 (no Bot V1 reversion!)"
        );
        let ids = w.state.identities.as_ref().unwrap();
        assert_eq!(
            ids.bot_actors
                .iter()
                .find(|b| b.actor_id == srv.bot_a_id)
                .unwrap()
                .display_name,
            "Bot V2",
            "T11-01: identities.bot_actors must hold Bot V2"
        );
    });

    // Switch back to Alice and wait settle
    let _ = cw.update(app, |w, cx| {
        w.switch_active_actor(srv.alice_id.clone(), cx);
    });
    assert!(
        pump_until(app, Duration::from_secs(5), |cx| {
            let st = &cw.read(cx).state;
            !st.profile_loading
                && !st.feed_loading
                && !st.communities_loading
                && !st.recommended_actors_loading
        })
        .await
    );

    // ------------------------------------------------------------------
    // T11-02 (R10-HUNT-1a / 1b / 1c): Zero full-snapshot state.identities overwrites
    // ------------------------------------------------------------------
    let id_rx_before = srv.identities_get_received.load(Ordering::SeqCst);

    // (2a) Slow pre-commit save_selected_bot_edits("CrossBot V2") vs fast submit_edit_profile("CrossAlice V2")
    let ap_rx2a = srv.actor_patch_received.load(Ordering::SeqCst);
    let ap_done2a = srv.actor_patch_completed.load(Ordering::SeqCst);
    srv.actor_patch_pre_delays_by_name
        .lock()
        .unwrap()
        .insert("CrossBot V2".to_string(), 240);
    let _ = cw.update(app, |w, cx| {
        let bot_a = w
            .state
            .identities
            .as_ref()
            .unwrap()
            .bot_actors
            .iter()
            .find(|b| b.actor_id == srv.bot_a_id)
            .unwrap()
            .clone();
        w.select_bot_for_studio(bot_a, cx);
        w.state.bot_studio_edit.display_name = "CrossBot V2".to_string();
        w.save_selected_bot_edits(cx);
    });
    assert!(
        pump_until(app, Duration::from_secs(4), |_| {
            srv.actor_patch_received.load(Ordering::SeqCst) > ap_rx2a
        })
        .await
    );
    let _ = cw.update(app, |w, cx| {
        let alice = w
            .state
            .identities
            .as_ref()
            .unwrap()
            .human_actor
            .clone()
            .unwrap();
        w.open_edit_profile_modal(alice, cx);
        w.state.edit_profile.display_name = "CrossAlice V2".to_string();
        w.submit_edit_profile(cx);
    });
    assert!(
        pump_until(app, Duration::from_secs(5), |cx| {
            let st = &cw.read(cx).state;
            !st.bot_studio_edit.saving
                && !st.edit_profile.submitting
                && srv.actor_patch_completed.load(Ordering::SeqCst) >= ap_done2a + 2
        })
        .await,
        "T11-02a: both cross-actor edits must settle"
    );
    let _ = cw.read_with(app, |w, _| {
        let ids = w.state.identities.as_ref().unwrap();
        assert_eq!(ids.human_actor.as_ref().unwrap().display_name, "CrossAlice V2");
        assert_eq!(
            ids.bot_actors
                .iter()
                .find(|b| b.actor_id == srv.bot_a_id)
                .unwrap()
                .display_name,
            "CrossBot V2"
        );
        assert_eq!(
            ids.all_actors
                .iter()
                .find(|a| a.actor_id == srv.alice_id)
                .unwrap()
                .display_name,
            "CrossAlice V2"
        );
        assert_eq!(
            ids.all_actors
                .iter()
                .find(|a| a.actor_id == srv.bot_a_id)
                .unwrap()
                .display_name,
            "CrossBot V2"
        );
    });

    // (2b) Symmetric: slow submit_edit_profile("CrossAlice V2B") vs fast save_selected_bot_edits("CrossBot V2B")
    let ap_rx2b = srv.actor_patch_received.load(Ordering::SeqCst);
    let ap_done2b = srv.actor_patch_completed.load(Ordering::SeqCst);
    srv.actor_patch_pre_delays_by_name
        .lock()
        .unwrap()
        .insert("CrossAlice V2B".to_string(), 240);
    let _ = cw.update(app, |w, cx| {
        let alice = w
            .state
            .identities
            .as_ref()
            .unwrap()
            .human_actor
            .clone()
            .unwrap();
        w.open_edit_profile_modal(alice, cx);
        w.state.edit_profile.display_name = "CrossAlice V2B".to_string();
        w.submit_edit_profile(cx);
    });
    assert!(
        pump_until(app, Duration::from_secs(4), |_| {
            srv.actor_patch_received.load(Ordering::SeqCst) > ap_rx2b
        })
        .await
    );
    let _ = cw.update(app, |w, cx| {
        let bot_a = w
            .state
            .identities
            .as_ref()
            .unwrap()
            .bot_actors
            .iter()
            .find(|b| b.actor_id == srv.bot_a_id)
            .unwrap()
            .clone();
        w.select_bot_for_studio(bot_a, cx);
        w.state.bot_studio_edit.display_name = "CrossBot V2B".to_string();
        w.save_selected_bot_edits(cx);
    });
    assert!(
        pump_until(app, Duration::from_secs(5), |cx| {
            let st = &cw.read(cx).state;
            !st.bot_studio_edit.saving
                && !st.edit_profile.submitting
                && srv.actor_patch_completed.load(Ordering::SeqCst) >= ap_done2b + 2
        })
        .await,
        "T11-02b: symmetric cross-actor edits must settle"
    );
    let _ = cw.read_with(app, |w, _| {
        let ids = w.state.identities.as_ref().unwrap();
        assert_eq!(ids.human_actor.as_ref().unwrap().display_name, "CrossAlice V2B");
        assert_eq!(
            ids.bot_actors
                .iter()
                .find(|b| b.actor_id == srv.bot_a_id)
                .unwrap()
                .display_name,
            "CrossBot V2B"
        );
    });

    // (2c) Slow pre-commit submit_register_bot("r11_new_bot") vs fast submit_edit_profile("CrossAlice V3")
    let rb_rx = srv.register_bot_received.load(Ordering::SeqCst);
    let rb_done = srv.register_bot_completed.load(Ordering::SeqCst);
    let ap_done2c = srv.actor_patch_completed.load(Ordering::SeqCst);
    srv.register_bot_pre_delay_ms.store(240, Ordering::SeqCst);
    let _ = cw.update(app, |w, cx| {
        w.state.bot_studio_name = "New Bot R11".to_string();
        w.state.bot_studio_handle = "r11_new_bot".to_string();
        w.submit_register_bot(cx);
    });
    assert!(
        pump_until(app, Duration::from_secs(4), |_| {
            srv.register_bot_received.load(Ordering::SeqCst) > rb_rx
        })
        .await
    );
    let _ = cw.update(app, |w, cx| {
        let alice = w
            .state
            .identities
            .as_ref()
            .unwrap()
            .human_actor
            .clone()
            .unwrap();
        w.open_edit_profile_modal(alice, cx);
        w.state.edit_profile.display_name = "CrossAlice V3".to_string();
        w.submit_edit_profile(cx);
    });
    assert!(
        pump_until(app, Duration::from_secs(5), |cx| {
            let st = &cw.read(cx).state;
            !st.bot_studio_submitting
                && !st.edit_profile.submitting
                && srv.register_bot_completed.load(Ordering::SeqCst) > rb_done
                && srv.actor_patch_completed.load(Ordering::SeqCst) > ap_done2c
        })
        .await,
        "T11-02c: register_bot + concurrent human edit must both settle"
    );
    assert_eq!(
        srv.identities_get_received.load(Ordering::SeqCst),
        id_rx_before,
        "T11-02 (Dead End #4): ZERO redundant GET /identities calls during edits or registration!"
    );
    let _ = cw.read_with(app, |w, _| {
        let ids = w.state.identities.as_ref().unwrap();
        assert_eq!(ids.human_actor.as_ref().unwrap().display_name, "CrossAlice V3");
        assert!(
            ids.bot_actors.iter().any(|b| b.handle == "r11_new_bot"),
            "T11-02c: newly registered r11_new_bot must stay in identities.bot_actors"
        );
        assert!(
            ids.all_actors.iter().any(|a| a.handle == "r11_new_bot"),
            "T11-02c: newly registered r11_new_bot must stay in identities.all_actors"
        );
    });

    // ------------------------------------------------------------------
    // T11-03 (R10-HUNT-2a/2b + R10-ADV-NEW-3): switch_active_actor recommended_actors reload + scrubbing
    // ------------------------------------------------------------------
    // (3a) Mid-bootstrap actor switch while bootstrap search is stalled
    let sr_rx3a = srv.search_received.load(Ordering::SeqCst);
    srv.search_pre_delay_ms.store(260, Ordering::SeqCst);
    let _ = cw.update(app, |w, cx| {
        w.state.recommended_actors.clear();
        w.bootstrap_initial_load(cx);
    });
    assert!(
        pump_until(app, Duration::from_secs(4), |cx| {
            cw.read(cx).state.recommended_actors_loading
                && srv.search_received.load(Ordering::SeqCst) > sr_rx3a
        })
        .await
    );
    let _ = cw.update(app, |w, cx| {
        w.switch_active_actor(srv.bot_a_id.clone(), cx);
    });
    assert!(
        pump_until(app, Duration::from_secs(5), |cx| {
            let st = &cw.read(cx).state;
            !st.recommended_actors_loading
                && !st.feed_loading
                && !st.communities_loading
                && !st.profile_loading
                && !st.recommended_actors.is_empty()
        })
        .await,
        "T11-03a: switch_active_actor mid-bootstrap must reload non-empty recommended_actors"
    );

    // (3b) Human blocks Bob, navigates to Feed, switches to Bot -> synchronous scrub + Bot follows Bob!
    let _ = cw.update(app, |w, cx| {
        w.switch_active_actor(srv.alice_id.clone(), cx);
    });
    assert!(
        pump_until(app, Duration::from_secs(5), |cx| {
            let st = &cw.read(cx).state;
            !st.recommended_actors_loading
                && !st.feed_loading
                && !st.communities_loading
                && !st.profile_loading
        })
        .await
    );
    let _ = cw.update(app, |w, cx| {
        w.execute_search(String::new(), CommunitySearchTab::Top, cx);
        w.load_community_page(srv.slug_rust.clone(), cx);
        w.navigate_to(CommunityRoute::ActorProfile("r11_bob".to_string()), cx);
    });
    assert!(
        pump_until(app, Duration::from_secs(4), |cx| {
            let st = &cw.read(cx).state;
            !st.search_loading
                && !st.community_loading
                && !st.profile_loading
                && st.search_result.is_some()
                && st.community_detail.is_some()
                && st
                    .actor_profile
                    .as_ref()
                    .is_some_and(|p| p.actor.actor_id == srv.bob_id)
        })
        .await
    );
    let blk_done3b = srv.block_completed.load(Ordering::SeqCst);
    let _ = cw.update(app, |w, cx| {
        w.toggle_block_actor(srv.bob_id.clone(), false, cx);
    });
    assert!(
        pump_until(app, Duration::from_secs(4), |cx| {
            cw.read(cx).state.actor_block_in_flight.is_empty()
                && srv.block_completed.load(Ordering::SeqCst) > blk_done3b
        })
        .await
    );
    let _ = cw.update(app, |w, cx| {
        assert!(w.state.actor_profile.as_ref().unwrap().actor.is_blocked);
        w.navigate_to(CommunityRoute::Feed, cx);
        w.switch_active_actor(srv.bot_a_id.clone(), cx);
        // Synchronous scrubbing + off-route drop assertions:
        assert!(
            w.state.actor_profile.is_none(),
            "T11-03b: off-route actor_profile must be dropped on switch_active_actor"
        );
        assert!(
            w.state.search_result.is_none(),
            "T11-03b: off-route search_result must be dropped on switch_active_actor"
        );
        assert!(
            w.state.community_detail.is_none(),
            "T11-03b: off-route community_detail must be dropped on switch_active_actor"
        );
        assert!(
            w.state
                .recommended_actors
                .iter()
                .all(|a| !a.is_blocked && !a.is_following && !a.is_muted),
            "T11-03b: recommended_actors must be synchronously scrubbed on switch_active_actor"
        );
        assert!(
            w.state
                .feed_items
                .iter()
                .all(|i| i.post.viewer_vote == 0 && !i.post.viewer_liked && !i.post.viewer_bookmarked),
            "T11-03b: feed_items viewer flags must be synchronously scrubbed"
        );
    });
    assert!(
        pump_until(app, Duration::from_secs(5), |cx| {
            let st = &cw.read(cx).state;
            !st.recommended_actors_loading && !st.feed_loading && !st.communities_loading
        })
        .await
    );
    let fl_calls_before = srv.follow_calls.load(Ordering::SeqCst);
    let fl_done_before = srv.follow_completed.load(Ordering::SeqCst);
    let _ = cw.update(app, |w, cx| {
        w.toggle_follow_actor(srv.bob_id.clone(), true, cx);
    });
    assert!(
        pump_until(app, Duration::from_secs(4), |cx| {
            cw.read(cx).state.actor_follow_in_flight.is_empty()
                && srv.follow_completed.load(Ordering::SeqCst) > fl_done_before
        })
        .await,
        "T11-03b: Bot must be able to follow Bob after Human blocked Bob"
    );
    assert_eq!(
        srv.follow_calls.load(Ordering::SeqCst),
        fl_calls_before + 1,
        "T11-03b: follow_calls must increment (not silently suppressed by stale Human block!)"
    );
    assert!(
        srv.follows
            .lock()
            .unwrap()
            .contains(&(srv.bot_a_id.clone(), srv.bob_id.clone())),
        "T11-03b: server must record (bot_a, bob) follow"
    );
    let _ = cw.read_with(app, |w, _| {
        let bob = w
            .state
            .recommended_actors
            .iter()
            .find(|a| a.actor_id == srv.bob_id)
            .unwrap();
        assert!(
            bob.is_following && !bob.is_blocked,
            "T11-03b: Bot UI must show Bob is_following=true, is_blocked=false"
        );
    });

    // Restore Alice for T11-04..06
    let _ = cw.update(app, |w, cx| {
        w.switch_active_actor(srv.alice_id.clone(), cx);
    });
    assert!(
        pump_until(app, Duration::from_secs(5), |cx| {
            let st = &cw.read(cx).state;
            !st.recommended_actors_loading && !st.feed_loading && !st.communities_loading
        })
        .await
    );
}

async fn run_r11_t04(
    app: &mut AsyncApp,
    cw: &Entity<CommunityWorkspace>,
    srv: &Arc<R11MockState>,
) {
    use std::sync::atomic::Ordering;

    // ------------------------------------------------------------------
    // T11-04a (R10-ADV-NEW-4): Save #1 on r11_rust -> nav to r11_python and STAY on r11_python
    // ------------------------------------------------------------------
    let _ = cw.update(app, |w, cx| {
        w.execute_search(String::new(), CommunitySearchTab::Top, cx);
        w.load_mod_queue(srv.slug_rust.clone(), cx);
        w.navigate_to(CommunityRoute::CommunityPage(srv.slug_rust.clone()), cx);
    });
    assert!(
        pump_until(app, Duration::from_secs(4), |cx| {
            let st = &cw.read(cx).state;
            !st.search_loading
                && !st.mod_loading
                && !st.community_loading
                && st.search_result.is_some()
                && st
                    .mod_community
                    .as_ref()
                    .is_some_and(|c| c.slug == srv.slug_rust)
                && st
                    .community_detail
                    .as_ref()
                    .is_some_and(|d| d.community.slug == srv.slug_rust)
        })
        .await
    );
    let wk_rx4a = srv.wiki_put_received.load(Ordering::SeqCst);
    let wk_done4a = srv.wiki_put_completed.load(Ordering::SeqCst);
    let rp_rx4a = srv.rules_patch_received.load(Ordering::SeqCst);
    let rp_done4a = srv.rules_patch_completed.load(Ordering::SeqCst);
    srv.wiki_pre_delays_by_md
        .lock()
        .unwrap()
        .insert("# Wiki NavAway".to_string(), 240);
    srv.rules_pre_delays_by_last_title
        .lock()
        .unwrap()
        .insert("Rule NavAway".to_string(), 240);
    let _ = cw.update(app, |w, cx| {
        w.state.community_wiki_editing = true;
        w.state.community_wiki_draft = "# Wiki NavAway".to_string();
        w.save_community_wiki(srv.slug_rust.clone(), cx);
        w.state.community_new_rule_title = "Rule NavAway".to_string();
        w.add_community_rule(srv.slug_rust.clone(), cx);
    });
    assert!(
        pump_until(app, Duration::from_secs(4), |_| {
            srv.wiki_put_received.load(Ordering::SeqCst) > wk_rx4a
                && srv.rules_patch_received.load(Ordering::SeqCst) > rp_rx4a
        })
        .await
    );
    let _ = cw.update(app, |w, cx| {
        w.navigate_to(CommunityRoute::CommunityPage(srv.slug_python.clone()), cx);
    });
    assert!(
        pump_until(app, Duration::from_secs(4), |cx| {
            let st = &cw.read(cx).state;
            !st.community_loading
                && st
                    .community_detail
                    .as_ref()
                    .is_some_and(|d| d.community.slug == srv.slug_python)
        })
        .await
    );
    assert!(
        pump_until(app, Duration::from_secs(5), |cx| {
            let st = &cw.read(cx).state;
            srv.wiki_put_completed.load(Ordering::SeqCst) > wk_done4a
                && srv.rules_patch_completed.load(Ordering::SeqCst) > rp_done4a
                && st
                    .communities
                    .iter()
                    .find(|c| c.slug == srv.slug_rust)
                    .is_some_and(|c| {
                        c.wiki_markdown == "# Wiki NavAway"
                            && c.rules.iter().any(|r| r.title == "Rule NavAway")
                    })
        })
        .await,
        "T11-04a (R10-ADV-NEW-4): staying on r11_python must STILL patch off-route r11_rust caches!"
    );
    let _ = cw.read_with(app, |w, _| {
        let mc = w.state.mod_community.as_ref().unwrap();
        assert_eq!(mc.wiki_markdown, "# Wiki NavAway");
        assert!(mc.rules.iter().any(|r| r.title == "Rule NavAway"));
        let sr_rust = w
            .state
            .search_result
            .as_ref()
            .unwrap()
            .communities
            .iter()
            .find(|c| c.slug == srv.slug_rust)
            .unwrap();
        assert_eq!(sr_rust.wiki_markdown, "# Wiki NavAway");
        assert!(sr_rust.rules.iter().any(|r| r.title == "Rule NavAway"));
    });

    // ------------------------------------------------------------------
    // T11-04b (R10-HUNT-3a/3b): Save #1 on r11_rust -> nav to r11_python -> nav BACK to r11_rust (NO Save #2)
    // ------------------------------------------------------------------
    let _ = cw.update(app, |w, cx| {
        w.navigate_to(CommunityRoute::CommunityPage(srv.slug_rust.clone()), cx);
    });
    assert!(
        pump_until(app, Duration::from_secs(4), |cx| {
            let st = &cw.read(cx).state;
            !st.community_loading
                && st
                    .community_detail
                    .as_ref()
                    .is_some_and(|d| d.community.slug == srv.slug_rust)
        })
        .await
    );
    let wk_rx4b = srv.wiki_put_received.load(Ordering::SeqCst);
    let wk_done4b = srv.wiki_put_completed.load(Ordering::SeqCst);
    let rp_rx4b = srv.rules_patch_received.load(Ordering::SeqCst);
    let rp_done4b = srv.rules_patch_completed.load(Ordering::SeqCst);
    srv.wiki_pre_delays_by_md
        .lock()
        .unwrap()
        .insert("# Wiki NavBack".to_string(), 320);
    srv.rules_pre_delays_by_last_title
        .lock()
        .unwrap()
        .insert("Rule NavBack".to_string(), 320);
    let _ = cw.update(app, |w, cx| {
        w.state.community_wiki_editing = true;
        w.state.community_wiki_draft = "# Wiki NavBack".to_string();
        w.save_community_wiki(srv.slug_rust.clone(), cx);
        w.state.community_new_rule_title = "Rule NavBack".to_string();
        w.add_community_rule(srv.slug_rust.clone(), cx);
    });
    assert!(
        pump_until(app, Duration::from_secs(4), |_| {
            srv.wiki_put_received.load(Ordering::SeqCst) > wk_rx4b
                && srv.rules_patch_received.load(Ordering::SeqCst) > rp_rx4b
        })
        .await
    );
    let _ = cw.update(app, |w, cx| {
        w.navigate_to(CommunityRoute::CommunityPage(srv.slug_python.clone()), cx);
    });
    assert!(
        pump_until(app, Duration::from_secs(4), |cx| {
            let st = &cw.read(cx).state;
            !st.community_loading
                && st
                    .community_detail
                    .as_ref()
                    .is_some_and(|d| d.community.slug == srv.slug_python)
        })
        .await
    );
    let _ = cw.update(app, |w, cx| {
        w.navigate_to(CommunityRoute::CommunityPage(srv.slug_rust.clone()), cx);
    });
    assert!(
        pump_until(app, Duration::from_secs(4), |cx| {
            let st = &cw.read(cx).state;
            !st.community_loading
                && st
                    .community_detail
                    .as_ref()
                    .is_some_and(|d| d.community.slug == srv.slug_rust)
        })
        .await
    );
    assert!(
        pump_until(app, Duration::from_secs(5), |cx| {
            let st = &cw.read(cx).state;
            !st.community_loading
                && srv.wiki_put_completed.load(Ordering::SeqCst) > wk_done4b
                && srv.rules_patch_completed.load(Ordering::SeqCst) > rp_done4b
                && st.community_detail.as_ref().is_some_and(|d| {
                    d.community.wiki_markdown == "# Wiki NavBack"
                        && d.community.rules.iter().any(|r| r.title == "Rule NavBack")
                })
        })
        .await,
        "T11-04b (R10-HUNT-3a/3b): nav-away-and-back without Save #2 MUST converge Wiki & Rules!"
    );

    // ------------------------------------------------------------------
    // T11-04c (R10-HUNT-3c): Comment Edit nav-away-and-back without Save #2
    // ------------------------------------------------------------------
    let _ = cw.update(app, |w, cx| {
        w.navigate_to(CommunityRoute::PostDetail(srv.p_part2.clone()), cx);
    });
    assert!(
        pump_until(app, Duration::from_secs(4), |cx| {
            let st = &cw.read(cx).state;
            !st.detail_loading
                && st
                    .detail_post
                    .as_ref()
                    .is_some_and(|d| d.post.post_id == srv.p_part2)
        })
        .await
    );
    let cm_rx4c = srv.comment_patch_received.load(Ordering::SeqCst);
    let cm_done4c = srv.comment_patch_completed.load(Ordering::SeqCst);
    srv.comment_patch_pre_delays_by_body
        .lock()
        .unwrap()
        .insert("Comment NavBack".to_string(), 320);
    let _ = cw.update(app, |w, cx| {
        w.start_edit_comment(srv.c_edit.clone(), "Edit Target Initial".to_string(), cx);
        w.state.detail_edit_comment_draft = "Comment NavBack".to_string();
        w.submit_edit_comment(cx);
    });
    assert!(
        pump_until(app, Duration::from_secs(4), |_| {
            srv.comment_patch_received.load(Ordering::SeqCst) > cm_rx4c
        })
        .await
    );
    let _ = cw.update(app, |w, cx| {
        w.navigate_to(CommunityRoute::PostDetail(srv.p_part1.clone()), cx);
    });
    assert!(
        pump_until(app, Duration::from_secs(4), |cx| {
            let st = &cw.read(cx).state;
            !st.detail_loading
                && st
                    .detail_post
                    .as_ref()
                    .is_some_and(|d| d.post.post_id == srv.p_part1)
        })
        .await
    );
    let _ = cw.update(app, |w, cx| {
        w.navigate_to(CommunityRoute::PostDetail(srv.p_part2.clone()), cx);
    });
    assert!(
        pump_until(app, Duration::from_secs(4), |cx| {
            let st = &cw.read(cx).state;
            !st.detail_loading
                && st
                    .detail_post
                    .as_ref()
                    .is_some_and(|d| d.post.post_id == srv.p_part2)
        })
        .await
    );
    assert!(
        pump_until(app, Duration::from_secs(5), |cx| {
            let st = &cw.read(cx).state;
            srv.comment_patch_completed.load(Ordering::SeqCst) > cm_done4c
                && st.detail_post.as_ref().is_some_and(|d| {
                    d.comments
                        .iter()
                        .any(|c| c.comment_id == srv.c_edit && c.body == "Comment NavBack")
                })
        })
        .await,
        "T11-04c (R10-HUNT-3c): comment edit nav-away-and-back without Save #2 MUST converge!"
    );

    // ------------------------------------------------------------------
    // T11-04d (Explorer 2 §4 #1): AutoMod Cross-Slug Nav Twin
    // ------------------------------------------------------------------
    let _ = cw.update(app, |w, cx| {
        w.navigate_to(CommunityRoute::ModQueue(Some(srv.slug_rust.clone())), cx);
    });
    assert!(
        pump_until(app, Duration::from_secs(4), |cx| {
            let st = &cw.read(cx).state;
            !st.mod_loading
                && st
                    .mod_community
                    .as_ref()
                    .is_some_and(|c| c.slug == srv.slug_rust)
        })
        .await
    );
    let am_rx = srv.automod_patch_received.load(Ordering::SeqCst);
    let am_done = srv.automod_patch_completed.load(Ordering::SeqCst);
    srv.automod_pre_delay_ms.store(260, Ordering::SeqCst);
    let _ = cw.update(app, |w, cx| {
        w.toggle_automod_rule_enabled("am_r11_1".to_string(), cx);
        assert!(w.state.automod_draft.saving);
    });
    assert!(
        pump_until(app, Duration::from_secs(4), |_| {
            srv.automod_patch_received.load(Ordering::SeqCst) > am_rx
        })
        .await
    );
    let _ = cw.update(app, |w, cx| {
        w.navigate_to(CommunityRoute::ModQueue(Some(srv.slug_python.clone())), cx);
        assert!(
            !w.state.automod_draft.saving,
            "T11-04d: automod_draft.saving must be scoped to active mod_slug (r11_python)"
        );
        assert!(
            w.state.automod_in_flight.contains_key(&srv.slug_rust),
            "T11-04d: load_mod_queue(r11_python) must NOT wipe r11_rust's automod_in_flight token"
        );
    });
    assert!(
        pump_until(app, Duration::from_secs(5), |cx| {
            let st = &cw.read(cx).state;
            !st.mod_loading
                && st.automod_in_flight.is_empty()
                && srv.automod_patch_completed.load(Ordering::SeqCst) > am_done
        })
        .await,
        "T11-04d: cross-slug AutoMod PATCH must complete cleanly without dropping token"
    );
    let _ = cw.read_with(app, |w, _| {
        let rust_comm = w
            .state
            .communities
            .iter()
            .find(|c| c.slug == srv.slug_rust)
            .unwrap();
        assert!(
            rust_comm.automod_rules[0].enabled,
            "T11-04d: communities[r11_rust] must reflect committed AutoMod toggle after slug switch"
        );
    });

    // ------------------------------------------------------------------
    // T11-04e (Regressions: BUG-HUNT-6, R9-HUNT-3a/3b, R10-COMMENT-ABA)
    // ------------------------------------------------------------------
    let _ = cw.update(app, |w, cx| {
        w.navigate_to(CommunityRoute::CommunityPage(srv.slug_rust.clone()), cx);
    });
    assert!(
        pump_until(app, Duration::from_secs(4), |cx| {
            let st = &cw.read(cx).state;
            !st.community_loading
                && st
                    .community_detail
                    .as_ref()
                    .is_some_and(|d| d.community.slug == srv.slug_rust)
        })
        .await
    );
    // BUG-HUNT-6: same-slug reload keeps community_wiki_saving == true
    let wk_done_bh6 = srv.wiki_put_completed.load(Ordering::SeqCst);
    srv.wiki_pre_delays_by_md
        .lock()
        .unwrap()
        .insert("# BugHunt6".to_string(), 180);
    let _ = cw.update(app, |w, cx| {
        w.state.community_wiki_editing = true;
        w.state.community_wiki_draft = "# BugHunt6".to_string();
        w.save_community_wiki(srv.slug_rust.clone(), cx);
        w.load_community_page(srv.slug_rust.clone(), cx);
        assert!(
            w.state.community_wiki_saving,
            "T11-04e (BUG-HUNT-6): same-slug reload must preserve community_wiki_saving"
        );
    });
    assert!(
        pump_until(app, Duration::from_secs(5), |cx| {
            let st = &cw.read(cx).state;
            !st.community_wiki_saving
                && !st.community_loading
                && srv.wiki_put_completed.load(Ordering::SeqCst) > wk_done_bh6
        })
        .await
    );

    // R9-HUNT-3a/3b: Wiki & Rules ABA with Save #2 (Save #1 post-delay 280ms vs Save #2 0ms)
    let wk_rx_aba = srv.wiki_put_received.load(Ordering::SeqCst);
    let wk_done_aba = srv.wiki_put_completed.load(Ordering::SeqCst);
    let rp_rx_aba = srv.rules_patch_received.load(Ordering::SeqCst);
    let rp_done_aba = srv.rules_patch_completed.load(Ordering::SeqCst);
    srv.wiki_post_delays_by_md
        .lock()
        .unwrap()
        .insert("# Wiki ABA V1".to_string(), 280);
    srv.rules_post_delays_by_last_title
        .lock()
        .unwrap()
        .insert("Rule ABA V1".to_string(), 280);
    let _ = cw.update(app, |w, cx| {
        w.state.community_wiki_editing = true;
        w.state.community_wiki_draft = "# Wiki ABA V1".to_string();
        w.save_community_wiki(srv.slug_rust.clone(), cx);
        w.state.community_new_rule_title = "Rule ABA V1".to_string();
        w.add_community_rule(srv.slug_rust.clone(), cx);
    });
    assert!(
        pump_until(app, Duration::from_secs(4), |_| {
            srv.wiki_put_received.load(Ordering::SeqCst) > wk_rx_aba
                && srv.rules_patch_received.load(Ordering::SeqCst) > rp_rx_aba
        })
        .await
    );
    let _ = cw.update(app, |w, cx| {
        w.navigate_to(CommunityRoute::CommunityPage(srv.slug_python.clone()), cx);
    });
    assert!(
        pump_until(app, Duration::from_secs(4), |cx| {
            !cw.read(cx).state.community_loading
                && cw
                    .read(cx)
                    .state
                    .community_detail
                    .as_ref()
                    .is_some_and(|d| d.community.slug == srv.slug_python)
        })
        .await
    );
    let _ = cw.update(app, |w, cx| {
        w.navigate_to(CommunityRoute::CommunityPage(srv.slug_rust.clone()), cx);
    });
    assert!(
        pump_until(app, Duration::from_secs(4), |cx| {
            !cw.read(cx).state.community_loading
                && cw
                    .read(cx)
                    .state
                    .community_detail
                    .as_ref()
                    .is_some_and(|d| d.community.slug == srv.slug_rust)
        })
        .await
    );
    let _ = cw.update(app, |w, cx| {
        w.state.community_wiki_editing = true;
        w.state.community_wiki_draft = "# Wiki ABA V2".to_string();
        w.save_community_wiki(srv.slug_rust.clone(), cx);
        w.state.community_new_rule_title = "Rule ABA V2".to_string();
        w.add_community_rule(srv.slug_rust.clone(), cx);
    });
    assert!(
        pump_until(app, Duration::from_secs(5), |cx| {
            let st = &cw.read(cx).state;
            !st.community_wiki_saving
                && !st.community_rules_saving
                && !st.community_loading
                && srv.wiki_put_completed.load(Ordering::SeqCst) >= wk_done_aba + 2
                && srv.rules_patch_completed.load(Ordering::SeqCst) >= rp_done_aba + 2
        })
        .await
    );
    // Deterministic loopback barrier to ensure Save #1's GPUI callback has drained
    let _ = cw.update(app, |w, cx| w.load_notifications(cx));
    assert!(
        pump_until(app, Duration::from_secs(4), |cx| {
            !cw.read(cx).state.notifications_loading
        })
        .await
    );
    let _ = cw.read_with(app, |w, _| {
        let det = w.state.community_detail.as_ref().unwrap();
        assert_eq!(
            det.community.wiki_markdown, "# Wiki ABA V2",
            "T11-04e (R9-HUNT-3a): stale Wiki Save #1 must NOT clobber Save #2!"
        );
        assert_eq!(
            det.community.rules.last().map(|r| r.title.as_str()),
            Some("Rule ABA V2"),
            "T11-04e (R9-HUNT-3b): stale Rules Save #1 must NOT clobber Save #2!"
        );
    });

    // R10-COMMENT-ABA: Comment Edit Save #1 (post-delay 280ms) vs Save #2 (0ms)
    let _ = cw.update(app, |w, cx| {
        w.navigate_to(CommunityRoute::PostDetail(srv.p_part2.clone()), cx);
    });
    assert!(
        pump_until(app, Duration::from_secs(4), |cx| {
            !cw.read(cx).state.detail_loading
                && cw
                    .read(cx)
                    .state
                    .detail_post
                    .as_ref()
                    .is_some_and(|d| d.post.post_id == srv.p_part2)
        })
        .await
    );
    let cm_rx_aba = srv.comment_patch_received.load(Ordering::SeqCst);
    let cm_done_aba = srv.comment_patch_completed.load(Ordering::SeqCst);
    srv.comment_patch_post_delays_by_body
        .lock()
        .unwrap()
        .insert("Comment ABA V1".to_string(), 280);
    let _ = cw.update(app, |w, cx| {
        w.start_edit_comment(srv.c_edit.clone(), "Comment NavBack".to_string(), cx);
        w.state.detail_edit_comment_draft = "Comment ABA V1".to_string();
        w.submit_edit_comment(cx);
    });
    assert!(
        pump_until(app, Duration::from_secs(4), |_| {
            srv.comment_patch_received.load(Ordering::SeqCst) > cm_rx_aba
        })
        .await
    );
    let _ = cw.update(app, |w, cx| {
        w.navigate_to(CommunityRoute::PostDetail(srv.p_part1.clone()), cx);
    });
    assert!(
        pump_until(app, Duration::from_secs(4), |cx| {
            !cw.read(cx).state.detail_loading
                && cw
                    .read(cx)
                    .state
                    .detail_post
                    .as_ref()
                    .is_some_and(|d| d.post.post_id == srv.p_part1)
        })
        .await
    );
    let _ = cw.update(app, |w, cx| {
        w.navigate_to(CommunityRoute::PostDetail(srv.p_part2.clone()), cx);
    });
    assert!(
        pump_until(app, Duration::from_secs(4), |cx| {
            !cw.read(cx).state.detail_loading
                && cw
                    .read(cx)
                    .state
                    .detail_post
                    .as_ref()
                    .is_some_and(|d| d.post.post_id == srv.p_part2)
        })
        .await
    );
    let _ = cw.update(app, |w, cx| {
        w.start_edit_comment(srv.c_edit.clone(), "Comment ABA V1".to_string(), cx);
        w.state.detail_edit_comment_draft = "Comment ABA V2".to_string();
        w.submit_edit_comment(cx);
    });
    assert!(
        pump_until(app, Duration::from_secs(5), |cx| {
            !cw.read(cx).state.detail_edit_comment_saving
                && srv.comment_patch_completed.load(Ordering::SeqCst) >= cm_done_aba + 2
        })
        .await
    );
    let _ = cw.update(app, |w, cx| w.load_notifications(cx));
    assert!(
        pump_until(app, Duration::from_secs(4), |cx| {
            !cw.read(cx).state.notifications_loading
        })
        .await
    );
    let _ = cw.read_with(app, |w, _| {
        let cm = w
            .state
            .detail_post
            .as_ref()
            .unwrap()
            .comments
            .iter()
            .find(|c| c.comment_id == srv.c_edit)
            .unwrap();
        assert_eq!(
            cm.body, "Comment ABA V2",
            "T11-04e (R10-COMMENT-ABA): stale Comment Save #1 must NOT clobber Save #2!"
        );
    });
}

async fn run_r11_t05_to_t06(
    app: &mut AsyncApp,
    cw: &Entity<CommunityWorkspace>,
    srv: &Arc<R11MockState>,
) {
    use std::sync::atomic::Ordering;

    // ------------------------------------------------------------------
    // T11-05 (R10-ADV-NEW-1 / 1B + ModQueue Sibling)
    // ------------------------------------------------------------------
    // (5a) Thread chain pruning on settled PostDetail(p_part2)
    let dp_rx5a = srv.delete_post_received.load(Ordering::SeqCst);
    let dp_done5a = srv.delete_post_completed.load(Ordering::SeqCst);
    srv.delete_post_pre_delay_ms.store(240, Ordering::SeqCst);
    let _ = cw.update(app, |w, cx| {
        w.navigate_to(CommunityRoute::Feed, cx);
        w.delete_post_action(srv.p_part1.clone(), false, cx);
    });
    assert!(
        pump_until(app, Duration::from_secs(4), |_| {
            srv.delete_post_received.load(Ordering::SeqCst) > dp_rx5a
        })
        .await
    );
    let _ = cw.update(app, |w, cx| {
        w.navigate_to(CommunityRoute::PostDetail(srv.p_part2.clone()), cx);
    });
    assert!(
        pump_until(app, Duration::from_secs(4), |cx| {
            let st = &cw.read(cx).state;
            !st.detail_loading
                && st.detail_post.as_ref().is_some_and(|d| {
                    d.post.post_id == srv.p_part2
                        && d.thread_chain.iter().any(|p| p.post_id == srv.p_part1)
                })
        })
        .await,
        "T11-05a setup: PostDetail(p_part2) must settle with p_part1 in thread_chain before DELETE commits"
    );
    assert!(
        pump_until(app, Duration::from_secs(5), |cx| {
            let st = &cw.read(cx).state;
            srv.delete_post_completed.load(Ordering::SeqCst) > dp_done5a
                && st
                    .detail_post
                    .as_ref()
                    .is_some_and(|d| !d.thread_chain.iter().any(|p| p.post_id == srv.p_part1))
        })
        .await,
        "T11-05a (R10-ADV-NEW-1): settled detail_post.thread_chain MUST prune deleted p_part1!"
    );

    // (5b) Quote chain pruning on settled PostDetail(p_part2)
    let dp_rx5b = srv.delete_post_received.load(Ordering::SeqCst);
    let dp_done5b = srv.delete_post_completed.load(Ordering::SeqCst);
    srv.delete_post_pre_delay_ms.store(240, Ordering::SeqCst);
    let _ = cw.update(app, |w, cx| {
        w.navigate_to(CommunityRoute::Feed, cx);
        w.delete_post_action(srv.p_quoted.clone(), false, cx);
    });
    assert!(
        pump_until(app, Duration::from_secs(4), |_| {
            srv.delete_post_received.load(Ordering::SeqCst) > dp_rx5b
        })
        .await
    );
    let _ = cw.update(app, |w, cx| {
        w.navigate_to(CommunityRoute::PostDetail(srv.p_part2.clone()), cx);
    });
    assert!(
        pump_until(app, Duration::from_secs(4), |cx| {
            let st = &cw.read(cx).state;
            !st.detail_loading
                && st.detail_post.as_ref().is_some_and(|d| {
                    d.post.post_id == srv.p_part2
                        && d.quote_chain.iter().any(|p| p.post_id == srv.p_quoted)
                })
        })
        .await,
        "T11-05b setup: PostDetail(p_part2) must settle with p_quoted in quote_chain"
    );
    assert!(
        pump_until(app, Duration::from_secs(5), |cx| {
            let st = &cw.read(cx).state;
            srv.delete_post_completed.load(Ordering::SeqCst) > dp_done5b
                && st
                    .detail_post
                    .as_ref()
                    .is_some_and(|d| !d.quote_chain.iter().any(|p| p.post_id == srv.p_quoted))
        })
        .await,
        "T11-05b (R10-ADV-NEW-1B): settled detail_post.quote_chain MUST prune deleted p_quoted!"
    );

    // (5c) ModQueue sibling: Remove & Lock from ModQueue immediately syncs off-route feed_items & community_posts
    let _ = cw.update(app, |w, cx| {
        w.load_feed(cx);
        w.navigate_to(CommunityRoute::CommunityPage(srv.slug_rust.clone()), cx);
    });
    assert!(
        pump_until(app, Duration::from_secs(4), |cx| {
            let st = &cw.read(cx).state;
            !st.feed_loading
                && !st.community_loading
                && st.feed_items.iter().any(|i| i.post.post_id == srv.p_mod_rem)
                && st
                    .community_posts
                    .iter()
                    .any(|i| i.post.post_id == srv.p_mod_lock)
        })
        .await
    );
    let _ = cw.update(app, |w, cx| {
        w.navigate_to(CommunityRoute::ModQueue(Some(srv.slug_rust.clone())), cx);
    });
    assert!(
        pump_until(app, Duration::from_secs(4), |cx| {
            !cw.read(cx).state.mod_loading
                && cw
                    .read(cx)
                    .state
                    .mod_community
                    .as_ref()
                    .is_some_and(|c| c.slug == srv.slug_rust)
        })
        .await
    );
    let ma_done = srv.mod_action_completed.load(Ordering::SeqCst);
    let _ = cw.update(app, |w, cx| {
        w.execute_mod_console_action(
            srv.slug_rust.clone(),
            ModActionKind::Remove,
            "post".to_string(),
            srv.p_mod_rem.clone(),
            None,
            cx,
        );
        w.execute_mod_console_action(
            srv.slug_rust.clone(),
            ModActionKind::Lock,
            "post".to_string(),
            srv.p_mod_lock.clone(),
            None,
            cx,
        );
    });
    assert!(
        pump_until(app, Duration::from_secs(5), |cx| {
            let st = &cw.read(cx).state;
            !st.mod_loading
                && srv.mod_action_completed.load(Ordering::SeqCst) >= ma_done + 2
                && !st.feed_items.iter().any(|i| i.post.post_id == srv.p_mod_rem)
                && st
                    .feed_items
                    .iter()
                    .find(|i| i.post.post_id == srv.p_mod_lock)
                    .is_some_and(|i| i.post.is_locked)
        })
        .await,
        "T11-05c: ModQueue Remove & Lock must sync off-route feed_items & community_posts"
    );
    let _ = cw.read_with(app, |w, _| {
        assert!(!w
            .state
            .community_posts
            .iter()
            .any(|i| i.post.post_id == srv.p_mod_rem));
        assert!(w
            .state
            .community_posts
            .iter()
            .find(|i| i.post.post_id == srv.p_mod_lock)
            .unwrap()
            .post
            .is_locked);
    });

    // ------------------------------------------------------------------
    // T11-06 (R10-ADV-NEW-2 / 2B + Parent Tombstone)
    // ------------------------------------------------------------------
    // (6a) Case B (R10-ADV-NEW-2): Delete c_leaf from cold ActorProfile -> open slow PostDetail(p_part2)
    let _ = cw.update(app, |w, cx| {
        w.navigate_to(CommunityRoute::ActorProfile("r11_alice".to_string()), cx);
    });
    assert!(
        pump_until(app, Duration::from_secs(4), |cx| {
            let st = &cw.read(cx).state;
            !st.profile_loading
                && st
                    .actor_profile
                    .as_ref()
                    .is_some_and(|p| p.recent_comments.iter().any(|c| c.comment_id == srv.c_leaf))
        })
        .await
    );
    let dc_rx6a = srv.delete_comment_received.load(Ordering::SeqCst);
    let dc_done6a = srv.delete_comment_completed.load(Ordering::SeqCst);
    let dg_rx6a = srv.detail_get_received.load(Ordering::SeqCst);
    let dg_done6a = srv.detail_get_completed.load(Ordering::SeqCst);
    srv.delete_comment_pre_delay_ms.store(140, Ordering::SeqCst);
    srv.detail_get_post_delay_ms.store(360, Ordering::SeqCst);
    let _ = cw.update(app, |w, cx| {
        // Cold ActorProfile: detail_post is None (so old pre-captured reload_pid was None!)
        w.state.detail_post = None;
        w.delete_comment_action(srv.c_leaf.clone(), cx);
    });
    assert!(
        pump_until(app, Duration::from_secs(4), |_| {
            srv.delete_comment_received.load(Ordering::SeqCst) > dc_rx6a
        })
        .await
    );
    let _ = cw.update(app, |w, cx| {
        w.navigate_to(CommunityRoute::PostDetail(srv.p_part2.clone()), cx);
    });
    assert!(
        pump_until(app, Duration::from_secs(4), |cx| {
            cw.read(cx).state.detail_loading
                && srv.detail_get_received.load(Ordering::SeqCst) > dg_rx6a
        })
        .await,
        "T11-06a: GET #1 must read pre-delete c_leaf while detail_loading"
    );
    assert!(
        pump_until(app, Duration::from_secs(5), |cx| {
            let st = &cw.read(cx).state;
            !st.detail_loading
                && srv.delete_comment_completed.load(Ordering::SeqCst) > dc_done6a
                && srv.detail_get_completed.load(Ordering::SeqCst) >= dg_done6a + 2
        })
        .await,
        "T11-06a (R10-ADV-NEW-2): delete_comment_action MUST fence stale GET #1 and reload PostDetail!"
    );
    let _ = cw.read_with(app, |w, _| {
        assert!(!w
            .state
            .detail_post
            .as_ref()
            .unwrap()
            .comments
            .iter()
            .any(|c| c.comment_id == srv.c_leaf));
        assert!(!w
            .state
            .actor_profile
            .as_ref()
            .unwrap()
            .recent_comments
            .iter()
            .any(|c| c.comment_id == srv.c_leaf));
    });

    // (6b) Settled (R10-ADV-NEW-2B): Delete c_leaf2 from cold ActorProfile -> fast PostDetail settles WITH c_leaf2 -> DELETE settles
    let _ = cw.update(app, |w, cx| {
        w.navigate_to(CommunityRoute::ActorProfile("r11_alice".to_string()), cx);
    });
    assert!(
        pump_until(app, Duration::from_secs(4), |cx| {
            let st = &cw.read(cx).state;
            !st.profile_loading
                && st
                    .actor_profile
                    .as_ref()
                    .is_some_and(|p| p.recent_comments.iter().any(|c| c.comment_id == srv.c_leaf2))
        })
        .await
    );
    let dc_rx6b = srv.delete_comment_received.load(Ordering::SeqCst);
    let dc_done6b = srv.delete_comment_completed.load(Ordering::SeqCst);
    srv.delete_comment_pre_delay_ms.store(240, Ordering::SeqCst);
    let _ = cw.update(app, |w, cx| {
        w.state.detail_post = None;
        w.delete_comment_action(srv.c_leaf2.clone(), cx);
    });
    assert!(
        pump_until(app, Duration::from_secs(4), |_| {
            srv.delete_comment_received.load(Ordering::SeqCst) > dc_rx6b
        })
        .await
    );
    let _ = cw.update(app, |w, cx| {
        w.navigate_to(CommunityRoute::PostDetail(srv.p_part2.clone()), cx);
    });
    assert!(
        pump_until(app, Duration::from_secs(4), |cx| {
            let st = &cw.read(cx).state;
            !st.detail_loading
                && st.detail_post.as_ref().is_some_and(|d| {
                    d.post.post_id == srv.p_part2
                        && d.comments.iter().any(|c| c.comment_id == srv.c_leaf2)
                })
        })
        .await,
        "T11-06b setup: fast PostDetail must settle with c_leaf2 still present before DELETE commits"
    );
    assert!(
        pump_until(app, Duration::from_secs(5), |cx| {
            let st = &cw.read(cx).state;
            !st.detail_loading
                && srv.delete_comment_completed.load(Ordering::SeqCst) > dc_done6b
                && st
                    .detail_post
                    .as_ref()
                    .is_some_and(|d| !d.comments.iter().any(|c| c.comment_id == srv.c_leaf2))
        })
        .await,
        "T11-06b (R10-ADV-NEW-2B): settled PostDetail MUST prune c_leaf2 once DELETE commits!"
    );
    let _ = cw.read_with(app, |w, _| {
        assert!(!w
            .state
            .actor_profile
            .as_ref()
            .unwrap()
            .recent_comments
            .iter()
            .any(|c| c.comment_id == srv.c_leaf2));
    });

    // (6c) Parent comment deletion tombstones c_parent while preserving child c_child
    let dc_done6c = srv.delete_comment_completed.load(Ordering::SeqCst);
    let _ = cw.update(app, |w, cx| {
        w.delete_comment_action(srv.c_parent.clone(), cx);
    });
    assert!(
        pump_until(app, Duration::from_secs(5), |cx| {
            let st = &cw.read(cx).state;
            !st.detail_loading
                && srv.delete_comment_completed.load(Ordering::SeqCst) > dc_done6c
                && st.detail_post.as_ref().is_some_and(|d| {
                    d.comments
                        .iter()
                        .find(|c| c.comment_id == srv.c_parent)
                        .is_some_and(|c| c.is_deleted && c.body == "[deleted]")
                })
        })
        .await,
        "T11-06c: parent delete must tombstone c_parent as [deleted]"
    );
    let _ = cw.read_with(app, |w, _| {
        let parent = w
            .state
            .detail_post
            .as_ref()
            .unwrap()
            .comments
            .iter()
            .find(|c| c.comment_id == srv.c_parent)
            .expect("tombstoned c_parent must remain in tree");
        assert!(parent.is_deleted && parent.body == "[deleted]");
        assert!(
            parent.children.iter().any(|ch| ch.comment_id == srv.c_child),
            "T11-06c: child c_child must be preserved under tombstoned c_parent"
        );
    });
}

// ============================================================================
// Wave r12 Regression Suite (T12-01..T12-06):
//   - T12-01: Cross-actor AutoMod ABA (settled + in-flight dirty server survival)
//             & per-slug automod_rules_gen_by_slug isolation (R11-R1-1 / R11-R2-1 / R11-HUNT-1a/1b / R11-R2-OBS-1)
//   - T12-02: Delayed submit_edit_comment vs delete_comment_action parent tombstone (R11-R1-2 / R11-HUNT-3 / R11-ADV-NEW-3)
//   - T12-03: Single-actor PostDetail -> settled ModQueue & cross-actor execute_mod_decision on settled ModQueue (R11-ADV-NEW-2A/2B / R11-HUNT-2c)
//   - T12-04: Cross-actor settled routes for submit_detail_reply, summon_bot_with_options, and submit_quick_post (R11-ADV-NEW-4 / R11-HUNT-2a/2b/2d)
//   - T12-05: delete_comment_action racing slow load_feed Case B post comment count reload (R11-R2-2 / R11-HUNT-5)
//   - T12-06: Same-tick vote_post + switch_active_actor("b:...") zero header theft (R11-HUNT-4b)
// ============================================================================

#[test]
fn test_m5_r12_cross_actor_automod_tombstone_and_settled_routes() {
    run_in_headless_subprocess("test_m5_r12_cross_actor_automod_tombstone_and_settled_routes");
}

struct R12MockState {
    alice_id: String,
    bot_id: String,
    slug_rust: String,
    slug_python: String,
    p_tree: String,
    p_mod1: String,
    p_mod2: String,
    p_empty: String,
    c_parent: String,
    c_child: String,
    comm_automod_rules: std::sync::Mutex<std::collections::HashMap<String, Vec<AutoModRuleDto>>>,
    automod_pre_delays_by_key: std::sync::Mutex<std::collections::HashMap<String, u64>>,
    automod_post_delays_by_key: std::sync::Mutex<std::collections::HashMap<String, u64>>,
    automod_patch_received: std::sync::atomic::AtomicUsize,
    automod_patch_committed: std::sync::atomic::AtomicUsize,
    automod_patch_completed: std::sync::atomic::AtomicUsize,
    comm_get_post_delays_by_slug: std::sync::Mutex<std::collections::HashMap<String, u64>>,
    mod_queue_by_slug: std::sync::Mutex<
        std::collections::HashMap<
            String,
            Vec<gpui_telegram_proto::community_contracts::ModQueueItemDto>,
        >,
    >,
    mod_logs_by_slug: std::sync::Mutex<
        std::collections::HashMap<
            String,
            Vec<gpui_telegram_proto::community_contracts::ModLogEntryDto>,
        >,
    >,
    mod_action_pre_delay_ms: std::sync::atomic::AtomicU64,
    mod_action_received: std::sync::atomic::AtomicUsize,
    mod_action_completed: std::sync::atomic::AtomicUsize,
    last_mod_action_actor: std::sync::Mutex<Option<String>>,
    posts: std::sync::Mutex<Vec<CommunityPostDto>>,
    comments_by_post: std::sync::Mutex<std::collections::HashMap<String, Vec<CommunityCommentDto>>>,
    votes_by_actor_post: std::sync::Mutex<std::collections::HashMap<(String, String), i8>>,
    comment_patch_post_delay_ms: std::sync::atomic::AtomicU64,
    comment_patch_received: std::sync::atomic::AtomicUsize,
    comment_patch_committed: std::sync::atomic::AtomicUsize,
    comment_patch_completed: std::sync::atomic::AtomicUsize,
    delete_comment_completed: std::sync::atomic::AtomicUsize,
    create_comment_pre_delay_ms: std::sync::atomic::AtomicU64,
    create_comment_received: std::sync::atomic::AtomicUsize,
    create_comment_completed: std::sync::atomic::AtomicUsize,
    last_create_comment_actor: std::sync::Mutex<Option<String>>,
    summon_pre_delay_ms: std::sync::atomic::AtomicU64,
    summon_received: std::sync::atomic::AtomicUsize,
    summon_completed: std::sync::atomic::AtomicUsize,
    last_summon_actor: std::sync::Mutex<Option<String>>,
    create_post_pre_delay_ms: std::sync::atomic::AtomicU64,
    create_post_received: std::sync::atomic::AtomicUsize,
    create_post_completed: std::sync::atomic::AtomicUsize,
    last_create_post_actor: std::sync::Mutex<Option<String>>,
    feed_get_post_delay_ms: std::sync::atomic::AtomicU64,
    feed_get_received: std::sync::atomic::AtomicUsize,
    feed_get_completed: std::sync::atomic::AtomicUsize,
    vote_calls_for_bot: std::sync::atomic::AtomicUsize,
}

impl R12MockState {
    fn build_actor(&self, actor_id: &str) -> CommunityActorDto {
        if actor_id == self.bot_id {
            let mut a = mock_actor_dto(&self.bot_id, ActorKind::Bot, "r12_mod_bot", "R12 Mod Bot");
            a.owner_user_id = Some(912001);
            a
        } else {
            let mut a = mock_actor_dto(&self.alice_id, ActorKind::Human, "r12_alice", "R12 Alice");
            a.linked_user_id = Some(912001);
            a
        }
    }

    fn build_community(&self, slug: &str, viewer: &str) -> SubCommunityDto {
        let role = if viewer == self.bot_id {
            Some(CommunityMemberRole::BotMod)
        } else {
            Some(CommunityMemberRole::Owner)
        };
        let mut c = mock_community_dto(slug, &format!("c/{slug}"), role);
        c.is_joined = true;
        c.automod_rules = self
            .comm_automod_rules
            .lock()
            .unwrap()
            .get(slug)
            .cloned()
            .unwrap_or_default();
        c
    }

    fn build_post_for_viewer(&self, mut post: CommunityPostDto, viewer: &str) -> CommunityPostDto {
        let dir = self
            .votes_by_actor_post
            .lock()
            .unwrap()
            .get(&(viewer.to_string(), post.post_id.clone()))
            .copied()
            .unwrap_or(0);
        post.viewer_vote = dir;
        post
    }

    fn build_detail(&self, pid: &str, viewer: &str) -> PostDetailResponse {
        let post = self
            .posts
            .lock()
            .unwrap()
            .iter()
            .find(|p| p.post_id == pid)
            .cloned()
            .unwrap_or_else(|| mock_post_dto(pid, &self.alice_id, false, Some(&self.slug_rust), 0));
        let post = self.build_post_for_viewer(post, viewer);
        let comments = self
            .comments_by_post
            .lock()
            .unwrap()
            .get(pid)
            .cloned()
            .unwrap_or_default();
        let total = post.comments_count;
        PostDetailResponse {
            post,
            comments,
            total_comments: total,
            thread_chain: Vec::new(),
            quote_chain: Vec::new(),
        }
    }
}

fn spawn_r12_server(
    rt: &tokio::runtime::Handle,
) -> (std::net::SocketAddr, Arc<R12MockState>, AbortOnDrop) {
    use gpui_telegram_proto::community_contracts::ModQueueItemDto;
    use std::collections::HashMap;
    use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    let alice_id = "u:912001".to_string();
    let bot_id = "b:912002".to_string();
    let slug_rust = "r12_rust".to_string();
    let slug_python = "r12_python".to_string();
    let p_tree = "p_r12_tree".to_string();
    let p_mod1 = "p_r12_mod1".to_string();
    let p_mod2 = "p_r12_mod2".to_string();
    let p_empty = "p_r12_empty".to_string();
    let c_parent = "c_r12_parent".to_string();
    let c_child = "c_r12_child".to_string();

    let mut automod_map = HashMap::new();
    automod_map.insert(slug_rust.clone(), Vec::new());
    automod_map.insert(
        slug_python.clone(),
        vec![AutoModRuleDto {
            rule_id: "am_py_init".to_string(),
            community_id: format!("comm_{slug_python}"),
            name: "Py Initial".to_string(),
            enabled: true,
            priority: 10,
            keywords: vec!["pyinit".to_string()],
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
            created_at_ms: 1_760_000_000_000,
        }],
    );

    let mut mq_map = HashMap::new();
    mq_map.insert(
        slug_rust.clone(),
        vec![ModQueueItemDto {
            item_id: "q_r12_1".to_string(),
            community_id: format!("comm_{slug_rust}"),
            target_kind: "post".to_string(),
            target_id: p_mod1.clone(),
            reporter_actor_id: Some(bot_id.clone()),
            source: "report".to_string(),
            reason: "Mod check 1".to_string(),
            rule_triggered: None,
            status: ModQueueStatus::Pending,
            resolved_by_actor_id: None,
            resolved_at_ms: None,
            created_at_ms: 1_760_000_000_100,
        }],
    );
    let mut logs_map = HashMap::new();
    logs_map.insert(slug_rust.clone(), Vec::new());

    let mut child = mock_comment_dto(&c_child, &p_tree, &bot_id);
    child.author_kind = ActorKind::Bot;
    child.body = "Child Reply".to_string();
    child.parent_comment_id = Some(c_parent.clone());
    child.depth = 1;
    let mut parent = mock_comment_dto(&c_parent, &p_tree, &alice_id);
    parent.body = "Original Parent Body".to_string();
    parent.children = vec![child];

    let mut comments_map = HashMap::new();
    comments_map.insert(p_tree.clone(), vec![parent]);
    comments_map.insert(p_empty.clone(), Vec::new());

    let mut post_tree = mock_post_dto(&p_tree, &alice_id, false, Some(&slug_rust), 10);
    post_tree.comments_count = 2;
    let post_mod1 = mock_post_dto(&p_mod1, &alice_id, false, Some(&slug_rust), 5);
    let post_mod2 = mock_post_dto(&p_mod2, &alice_id, false, Some(&slug_rust), 5);
    let post_empty = mock_post_dto(&p_empty, &alice_id, false, Some(&slug_rust), 10);

    let state = Arc::new(R12MockState {
        alice_id,
        bot_id,
        slug_rust,
        slug_python,
        p_tree,
        p_mod1,
        p_mod2,
        p_empty,
        c_parent,
        c_child,
        comm_automod_rules: std::sync::Mutex::new(automod_map),
        automod_pre_delays_by_key: std::sync::Mutex::new(HashMap::new()),
        automod_post_delays_by_key: std::sync::Mutex::new(HashMap::new()),
        automod_patch_received: AtomicUsize::new(0),
        automod_patch_committed: AtomicUsize::new(0),
        automod_patch_completed: AtomicUsize::new(0),
        comm_get_post_delays_by_slug: std::sync::Mutex::new(HashMap::new()),
        mod_queue_by_slug: std::sync::Mutex::new(mq_map),
        mod_logs_by_slug: std::sync::Mutex::new(logs_map),
        mod_action_pre_delay_ms: AtomicU64::new(0),
        mod_action_received: AtomicUsize::new(0),
        mod_action_completed: AtomicUsize::new(0),
        last_mod_action_actor: std::sync::Mutex::new(None),
        posts: std::sync::Mutex::new(vec![post_empty, post_tree, post_mod1, post_mod2]),
        comments_by_post: std::sync::Mutex::new(comments_map),
        votes_by_actor_post: std::sync::Mutex::new(HashMap::new()),
        comment_patch_post_delay_ms: AtomicU64::new(0),
        comment_patch_received: AtomicUsize::new(0),
        comment_patch_committed: AtomicUsize::new(0),
        comment_patch_completed: AtomicUsize::new(0),
        delete_comment_completed: AtomicUsize::new(0),
        create_comment_pre_delay_ms: AtomicU64::new(0),
        create_comment_received: AtomicUsize::new(0),
        create_comment_completed: AtomicUsize::new(0),
        last_create_comment_actor: std::sync::Mutex::new(None),
        summon_pre_delay_ms: AtomicU64::new(0),
        summon_received: AtomicUsize::new(0),
        summon_completed: AtomicUsize::new(0),
        last_summon_actor: std::sync::Mutex::new(None),
        create_post_pre_delay_ms: AtomicU64::new(0),
        create_post_received: AtomicUsize::new(0),
        create_post_completed: AtomicUsize::new(0),
        last_create_post_actor: std::sync::Mutex::new(None),
        feed_get_post_delay_ms: AtomicU64::new(0),
        feed_get_received: AtomicUsize::new(0),
        feed_get_completed: AtomicUsize::new(0),
        vote_calls_for_bot: AtomicUsize::new(0),
    });

    let std_listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind r12 server");
    std_listener.set_nonblocking(true).expect("nonblocking");
    let addr = std_listener.local_addr().expect("r12 local_addr");
    let st_accept = Arc::clone(&state);
    let join = rt.spawn(async move {
        let listener = tokio::net::TcpListener::from_std(std_listener).expect("tokio listener");
        loop {
            let Ok((mut sock, _)) = listener.accept().await else {
                break;
            };
            let st = Arc::clone(&st_accept);
            tokio::spawn(async move {
                let mut buf = Vec::with_capacity(8192);
                let mut tmp = [0u8; 4096];
                let mut header_end = None;
                let mut content_len = 0usize;
                loop {
                    let Ok(n) = sock.read(&mut tmp).await else { return; };
                    if n == 0 { return; }
                    buf.extend_from_slice(&tmp[..n]);
                    if header_end.is_none() {
                        if let Some(pos) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
                            header_end = Some(pos + 4);
                            let hdr = String::from_utf8_lossy(&buf[..pos]);
                            for line in hdr.lines() {
                                if let Some((k, v)) = line.split_once(':') {
                                    if k.eq_ignore_ascii_case("content-length") {
                                        content_len = v.trim().parse().unwrap_or(0);
                                    }
                                }
                            }
                        }
                    }
                    if let Some(he) = header_end {
                        if buf.len() >= he + content_len { break; }
                    }
                }
                let he = header_end.unwrap();
                let hdr_str = String::from_utf8_lossy(&buf[..he]).to_string();
                let mut lines = hdr_str.lines();
                let req_line = lines.next().unwrap_or_default();
                let mut parts = req_line.split_whitespace();
                let method = parts.next().unwrap_or("GET").to_string();
                let raw_target = parts.next().unwrap_or("/").to_string();
                let raw_path = raw_target.split_once('?').map(|(p, _)| p).unwrap_or(&raw_target);
                let path = raw_path.replace("%3A", ":").replace("%3a", ":");
                let mut acting_actor: Option<String> = None;
                for line in lines {
                    if let Some((k, v)) = line.split_once(':') {
                        if k.eq_ignore_ascii_case("x-community-actor-id") {
                            acting_actor = Some(v.trim().to_string());
                        }
                    }
                }
                let viewer = acting_actor.clone().unwrap_or_else(|| st.alice_id.clone());
                let body_bytes = &buf[he..he + content_len];
                let (status, resp_body) =
                    handle_r12_request(&st, &method, &path, &viewer, acting_actor, body_bytes).await;
                let http = format!(
                    "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                    resp_body.len(),
                    resp_body
                );
                let _ = sock.write_all(http.as_bytes()).await;
            });
        }
    });
    let _ = Ordering::SeqCst;
    (addr, state, AbortOnDrop(join))
}

async fn handle_r12_request(
    st: &R12MockState,
    method: &str,
    path: &str,
    viewer: &str,
    acting_actor: Option<String>,
    body_bytes: &[u8],
) -> (&'static str, String) {
    use gpui_telegram_proto::community_contracts::{ModActionRequest, ModLogEntryDto};
    use std::sync::atomic::Ordering;
    if method == "GET" && path == "/api/v1/community/identities" {
        let human = st.build_actor(&st.alice_id);
        let bot = st.build_actor(&st.bot_id);
        let resp = CommunityIdentitiesResponse {
            active_actor_id: viewer.to_string(),
            human_actor: Some(human.clone()),
            bot_actors: vec![bot.clone()],
            all_actors: vec![human, bot],
        };
        return ("200 OK", serde_json::to_string(&resp).unwrap());
    }
    if method == "GET" && path == "/api/v1/community/algo-prefs" {
        return ("200 OK", serde_json::to_string(&AlgoPreferencesDto::default()).unwrap());
    }
    if method == "GET" && path == "/api/v1/community/trending" {
        return ("200 OK", serde_json::to_string(&TrendingResponse::default()).unwrap());
    }
    if method == "GET" && path == "/api/v1/community/notifications" {
        return ("200 OK", serde_json::to_string(&NotificationsListResponse::default()).unwrap());
    }
    if method == "GET" && path == "/api/v1/community/lists" {
        return ("200 OK", serde_json::to_string(&CommunityListsResponse::default()).unwrap());
    }
    if method == "GET" && path == "/api/v1/community/search" {
        let snap = CommunitySearchResultDto {
            query: String::new(),
            tab: CommunitySearchTab::Top,
            posts: Vec::new(),
            actors: vec![st.build_actor(&st.bot_id), st.build_actor(&st.alice_id)],
            communities: vec![
                st.build_community(&st.slug_rust, viewer),
                st.build_community(&st.slug_python, viewer),
            ],
            total_hits: 2,
            next_offset: None,
        };
        return ("200 OK", serde_json::to_string(&snap).unwrap());
    }
    if method == "GET" && path == "/api/v1/community/communities" {
        let resp = ListCommunitiesResponse {
            communities: vec![
                st.build_community(&st.slug_rust, viewer),
                st.build_community(&st.slug_python, viewer),
            ],
        };
        return ("200 OK", serde_json::to_string(&resp).unwrap());
    }
    if method == "GET"
        && path.starts_with("/api/v1/community/communities/")
        && !path.ends_with("/mod-queue")
        && !path.ends_with("/mod-logs")
    {
        let slug = path.trim_start_matches("/api/v1/community/communities/").to_string();
        let resp = CommunityDetailResponse {
            community: st.build_community(&slug, viewer),
            pinned_posts: Vec::new(),
            moderators: vec![st.build_actor(&st.alice_id), st.build_actor(&st.bot_id)],
            wiki_revisions: Vec::new(),
        };
        let post_delay = st
            .comm_get_post_delays_by_slug
            .lock()
            .unwrap()
            .remove(&slug)
            .unwrap_or(0);
        if post_delay > 0 {
            tokio::time::sleep(Duration::from_millis(post_delay)).await;
        }
        return ("200 OK", serde_json::to_string(&resp).unwrap());
    }
    if method == "PATCH" && path.starts_with("/api/v1/community/communities/") {
        let slug = path.trim_start_matches("/api/v1/community/communities/").to_string();
        let req: UpdateCommunityRequest = serde_json::from_slice(body_bytes).expect("r12 patch comm");
        let rules = req.automod_rules.unwrap_or_default();
        let delay_key = format!("{slug}:{}", rules.len());
        st.automod_patch_received.fetch_add(1, Ordering::SeqCst);
        let pre = st
            .automod_pre_delays_by_key
            .lock()
            .unwrap()
            .remove(&delay_key)
            .unwrap_or(0);
        if pre > 0 {
            tokio::time::sleep(Duration::from_millis(pre)).await;
        }
        st.comm_automod_rules.lock().unwrap().insert(slug.clone(), rules);
        let comm = st.build_community(&slug, viewer);
        st.automod_patch_committed.fetch_add(1, Ordering::SeqCst);
        let post = st
            .automod_post_delays_by_key
            .lock()
            .unwrap()
            .remove(&delay_key)
            .unwrap_or(0);
        if post > 0 {
            tokio::time::sleep(Duration::from_millis(post)).await;
        }
        st.automod_patch_completed.fetch_add(1, Ordering::SeqCst);
        return ("200 OK", serde_json::to_string(&comm).unwrap());
    }
    if method == "GET" && path.ends_with("/mod-queue") {
        let slug = path
            .trim_start_matches("/api/v1/community/communities/")
            .trim_end_matches("/mod-queue");
        let items = st
            .mod_queue_by_slug
            .lock()
            .unwrap()
            .get(slug)
            .cloned()
            .unwrap_or_default();
        return ("200 OK", serde_json::to_string(&ModQueueResponse { items }).unwrap());
    }
    if method == "GET" && path.ends_with("/mod-logs") {
        let slug = path
            .trim_start_matches("/api/v1/community/communities/")
            .trim_end_matches("/mod-logs");
        let logs = st
            .mod_logs_by_slug
            .lock()
            .unwrap()
            .get(slug)
            .cloned()
            .unwrap_or_default();
        return ("200 OK", serde_json::to_string(&ModLogsResponse { logs }).unwrap());
    }
    if method == "POST" && path.ends_with("/mod-actions") {
        let slug = path
            .trim_start_matches("/api/v1/community/communities/")
            .trim_end_matches("/mod-actions")
            .to_string();
        let req: ModActionRequest = serde_json::from_slice(body_bytes).expect("r12 mod action");
        *st.last_mod_action_actor.lock().unwrap() = acting_actor.clone();
        st.mod_action_received.fetch_add(1, Ordering::SeqCst);
        let pre = st.mod_action_pre_delay_ms.swap(0, Ordering::SeqCst);
        if pre > 0 {
            tokio::time::sleep(Duration::from_millis(pre)).await;
        }
        {
            let mut mq = st.mod_queue_by_slug.lock().unwrap();
            if let Some(list) = mq.get_mut(&slug) {
                if let Some(qid) = req.queue_item_id.as_deref() {
                    list.retain(|q| q.item_id != qid);
                } else {
                    list.retain(|q| q.target_id != req.target_id);
                }
            }
        }
        let log_entry = {
            let mut logs = st.mod_logs_by_slug.lock().unwrap();
            let vec = logs.entry(slug.clone()).or_default();
            let entry = ModLogEntryDto {
                log_id: format!("ml_r12_{}", vec.len() + 1),
                community_id: format!("comm_{slug}"),
                moderator_actor_id: viewer.to_string(),
                moderator_handle: if viewer == st.bot_id { "r12_mod_bot" } else { "r12_alice" }.to_string(),
                moderator_kind: if viewer == st.bot_id { ActorKind::Bot } else { ActorKind::Human },
                action: req.action,
                target_kind: req.target_kind,
                target_id: req.target_id,
                reason: req.reason,
                rule_triggered: None,
                created_at_ms: 1_760_000_002_000,
            };
            vec.insert(0, entry.clone());
            entry
        };
        st.mod_action_completed.fetch_add(1, Ordering::SeqCst);
        return ("200 OK", serde_json::to_string(&log_entry).unwrap());
    }
    handle_r12_post_comment_routes(st, method, path, viewer, acting_actor, body_bytes).await
}

async fn handle_r12_post_comment_routes(
    st: &R12MockState,
    method: &str,
    path: &str,
    viewer: &str,
    acting_actor: Option<String>,
    body_bytes: &[u8],
) -> (&'static str, String) {
    use gpui_telegram_proto::community_contracts::{
        CreateCommentRequest, CreateCommentResponse, CreatePostResponse, FeedResponseDto,
        SummonBotRequest, SummonBotResponse, UpdateCommentRequest,
    };
    use std::sync::atomic::Ordering;
    if method == "GET" && path == "/api/v1/community/feed" {
        st.feed_get_received.fetch_add(1, Ordering::SeqCst);
        let items: Vec<FeedItemDto> = st
            .posts
            .lock()
            .unwrap()
            .iter()
            .filter(|p| !p.is_removed)
            .cloned()
            .map(|p| FeedItemDto {
                post: st.build_post_for_viewer(p, viewer),
                explanation: None,
                rank_score: 50.0,
                is_exploration_slot: false,
            })
            .collect();
        let resp = FeedResponseDto {
            items,
            next_cursor: None,
            ..Default::default()
        };
        let post = st.feed_get_post_delay_ms.swap(0, Ordering::SeqCst);
        if post > 0 {
            tokio::time::sleep(Duration::from_millis(post)).await;
        }
        st.feed_get_completed.fetch_add(1, Ordering::SeqCst);
        return ("200 OK", serde_json::to_string(&resp).unwrap());
    }
    if method == "POST" && path == "/api/v1/community/posts" {
        let req: CreatePostRequest = serde_json::from_slice(body_bytes).expect("r12 create post");
        *st.last_create_post_actor.lock().unwrap() = acting_actor;
        st.create_post_received.fetch_add(1, Ordering::SeqCst);
        let pre = st.create_post_pre_delay_ms.swap(0, Ordering::SeqCst);
        if pre > 0 {
            tokio::time::sleep(Duration::from_millis(pre)).await;
        }
        let mut created = {
            let mut posts = st.posts.lock().unwrap();
            let pid = format!("p_r12_new_{}", posts.len() + 1);
            let mut p = mock_post_dto(&pid, viewer, viewer == st.bot_id, req.community_slug.as_deref(), 1);
            p.title = req.title;
            p.body = req.body;
            p.status = req.status;
            posts.insert(0, p.clone());
            p
        };
        // Simulate submitter's auto-upvote in POST response (must NOT leak to Bot on cross-actor reload!)
        st.votes_by_actor_post
            .lock()
            .unwrap()
            .insert((viewer.to_string(), created.post_id.clone()), 1);
        created.viewer_vote = 1;
        st.create_post_completed.fetch_add(1, Ordering::SeqCst);
        let resp = CreatePostResponse {
            post: created,
            thread_posts: Vec::new(),
            automod_action: AutoModActionKind::Approve,
            automod_note: None,
        };
        return ("200 OK", serde_json::to_string(&resp).unwrap());
    }
    if method == "POST" && path.ends_with("/comments") && path.starts_with("/api/v1/community/posts/") {
        let pid = path
            .trim_start_matches("/api/v1/community/posts/")
            .trim_end_matches("/comments")
            .to_string();
        let req: CreateCommentRequest = serde_json::from_slice(body_bytes).expect("r12 create comment");
        *st.last_create_comment_actor.lock().unwrap() = acting_actor;
        st.create_comment_received.fetch_add(1, Ordering::SeqCst);
        let pre = st.create_comment_pre_delay_ms.swap(0, Ordering::SeqCst);
        if pre > 0 {
            tokio::time::sleep(Duration::from_millis(pre)).await;
        }
        let comment = {
            let mut cm = st.comments_by_post.lock().unwrap();
            let vec = cm.entry(pid.clone()).or_default();
            let cid = format!("c_r12_new_{}", vec.len() + 1);
            let mut c = mock_comment_dto(&cid, &pid, viewer);
            c.author_kind = if viewer == st.bot_id { ActorKind::Bot } else { ActorKind::Human };
            c.body = req.body;
            c.viewer_vote = 1;
            vec.push(c.clone());
            c
        };
        for p in st.posts.lock().unwrap().iter_mut() {
            if p.post_id == pid {
                p.comments_count = p.comments_count.saturating_add(1);
            }
        }
        st.create_comment_completed.fetch_add(1, Ordering::SeqCst);
        let resp = CreateCommentResponse {
            comment,
            automod_action: AutoModActionKind::Approve,
            automod_note: None,
        };
        return ("200 OK", serde_json::to_string(&resp).unwrap());
    }
    if method == "POST" && path.ends_with("/summon-bot") && path.starts_with("/api/v1/community/posts/") {
        let pid = path
            .trim_start_matches("/api/v1/community/posts/")
            .trim_end_matches("/summon-bot")
            .to_string();
        let _req: SummonBotRequest = serde_json::from_slice(body_bytes).expect("r12 summon");
        *st.last_summon_actor.lock().unwrap() = acting_actor;
        st.summon_received.fetch_add(1, Ordering::SeqCst);
        let pre = st.summon_pre_delay_ms.swap(0, Ordering::SeqCst);
        if pre > 0 {
            tokio::time::sleep(Duration::from_millis(pre)).await;
        }
        let created = {
            let mut cm = st.comments_by_post.lock().unwrap();
            let vec = cm.entry(pid.clone()).or_default();
            let cid = format!("c_r12_bot_{}", vec.len() + 1);
            let mut c = mock_comment_dto(&cid, &pid, &st.bot_id);
            c.author_kind = ActorKind::Bot;
            c.body = "Summoned Bot Reply R12".to_string();
            vec.push(c.clone());
            c
        };
        for p in st.posts.lock().unwrap().iter_mut() {
            if p.post_id == pid {
                p.comments_count = p.comments_count.saturating_add(1);
            }
        }
        st.summon_completed.fetch_add(1, Ordering::SeqCst);
        let resp = SummonBotResponse {
            summoned_bot_actor_id: st.bot_id.clone(),
            triggered: true,
            reason: "replied".to_string(),
            created_comment: Some(created),
        };
        return ("200 OK", serde_json::to_string(&resp).unwrap());
    }
    if method == "POST" && path.ends_with("/vote") && path.starts_with("/api/v1/community/posts/") {
        let pid = path
            .trim_start_matches("/api/v1/community/posts/")
            .trim_end_matches("/vote")
            .to_string();
        let req: VoteRequest = serde_json::from_slice(body_bytes).expect("r12 vote");
        if viewer == st.bot_id {
            st.vote_calls_for_bot.fetch_add(1, Ordering::SeqCst);
        }
        st.votes_by_actor_post
            .lock()
            .unwrap()
            .insert((viewer.to_string(), pid.clone()), req.direction);
        let resp = VoteResponse {
            target_id: pid,
            direction: req.direction,
            upvotes: i64::from(req.direction == 1),
            downvotes: i64::from(req.direction == -1),
            score: 10 + i64::from(req.direction),
        };
        return ("200 OK", serde_json::to_string(&resp).unwrap());
    }
    if method == "GET" && path.starts_with("/api/v1/community/posts/") {
        let pid = path.trim_start_matches("/api/v1/community/posts/");
        return ("200 OK", serde_json::to_string(&st.build_detail(pid, viewer)).unwrap());
    }
    if method == "PATCH" && path.starts_with("/api/v1/community/comments/") {
        let cid = path.trim_start_matches("/api/v1/community/comments/").to_string();
        let req: UpdateCommentRequest = serde_json::from_slice(body_bytes).expect("r12 patch comment");
        st.comment_patch_received.fetch_add(1, Ordering::SeqCst);
        let mut updated = mock_comment_dto(&cid, &st.p_tree, viewer);
        updated.body = req.body.clone();
        {
            let mut cm = st.comments_by_post.lock().unwrap();
            for list in cm.values_mut() {
                for c in list.iter_mut() {
                    if c.comment_id == cid && !c.is_deleted {
                        c.body = req.body.clone();
                        c.updated_at_ms += 100;
                        updated = c.clone();
                    }
                }
            }
        }
        st.comment_patch_committed.fetch_add(1, Ordering::SeqCst);
        let post = st.comment_patch_post_delay_ms.swap(0, Ordering::SeqCst);
        if post > 0 {
            tokio::time::sleep(Duration::from_millis(post)).await;
        }
        st.comment_patch_completed.fetch_add(1, Ordering::SeqCst);
        return ("200 OK", serde_json::to_string(&updated).unwrap());
    }
    if method == "DELETE" && path.starts_with("/api/v1/community/comments/") {
        let cid = path.trim_start_matches("/api/v1/community/comments/").to_string();
        let mut affected_pid = None;
        {
            let mut cm = st.comments_by_post.lock().unwrap();
            for (pid, list) in cm.iter_mut() {
                if let Some(idx) = list.iter().position(|c| c.comment_id == cid) {
                    affected_pid = Some(pid.clone());
                    if !list[idx].children.is_empty() {
                        list[idx].body = "[deleted]".to_string();
                        list[idx].is_deleted = true;
                        list[idx].is_removed = true;
                    } else {
                        list.remove(idx);
                    }
                    break;
                }
            }
        }
        if let Some(pid) = affected_pid {
            for p in st.posts.lock().unwrap().iter_mut() {
                if p.post_id == pid {
                    p.comments_count = p.comments_count.saturating_sub(1);
                }
            }
        }
        st.delete_comment_completed.fetch_add(1, Ordering::SeqCst);
        return (
            "200 OK",
            serde_json::to_string(&DeleteEntityResponse { id: cid, deleted: true }).unwrap(),
        );
    }
    ("404 Not Found", "{}".to_string())
}

async fn body_r12_cross_actor_automod_tombstone_and_settled_routes(mut app: AsyncApp) {
    let dir = tempfile::TempDir::new().expect("r12 tempdir");
    let rt_dir = tempfile::TempDir::new().expect("r12 rt tempdir");
    let rt_holder = TelegramClient::open_with_dir(
        ClientConfig {
            server_url: "http://127.0.0.1:9".to_string(),
            max_reconnect_attempts: 0,
            ..Default::default()
        },
        rt_dir.path(),
    )
    .expect("open r12 rt holder");
    let (addr, srv, _server_guard) = spawn_r12_server(rt_holder.runtime().handle());

    let client = Arc::new(
        TelegramClient::open_with_dir(
            ClientConfig {
                server_url: format!("http://{addr}"),
                max_reconnect_attempts: 0,
                ..Default::default()
            },
            dir.path(),
        )
        .expect("open r12 client"),
    );
    let auth = test_auth(912001, "r12_alice");
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
        .expect("mount r12 AppShell");
    let _ = shell.update(&mut app, |s, cx| s.open_community_workspace(cx));
    let cw = shell
        .read_with(&app, |s, _| s.community_workspace.clone().unwrap())
        .unwrap();

    assert!(
        pump_until(&mut app, Duration::from_secs(5), |cx| {
            let st = &cw.read(cx).state;
            !st.feed_loading
                && !st.identities_loading
                && !st.communities_loading
                && !st.recommended_actors_loading
                && st.identities.is_some()
                && st.communities.len() == 2
                && st.feed_items.iter().any(|it| it.post.post_id == srv.p_empty)
        })
        .await,
        "R12 workspace must bootstrap cleanly"
    );

    run_r12_t01_automod_and_slug_isolation(&mut app, &cw, &srv).await;
    run_r12_t02_t03_tombstone_and_mod_queue(&mut app, &cw, &srv).await;
    run_r12_t04_t05_t06_settled_routes_and_same_tick_guard(&mut app, &cw, &srv).await;

    drop(rt_holder);
}

async fn run_r12_t01_automod_and_slug_isolation(
    app: &mut AsyncApp,
    cw: &Entity<CommunityWorkspace>,
    srv: &Arc<R12MockState>,
) {
    use std::sync::atomic::Ordering;

    // T12-01a: Settled Cross-Actor AutoMod ABA (Alice [Rule A] delayed post-commit vs Bot [Rule A, Rule B])
    let _ = cw.update(app, |w, cx| {
        w.navigate_to(CommunityRoute::ModQueue(Some(srv.slug_rust.clone())), cx);
    });
    assert!(
        pump_until(app, Duration::from_secs(4), |cx| {
            let st = &cw.read(cx).state;
            !st.mod_loading
                && st.mod_community.as_ref().is_some_and(|c| c.slug == srv.slug_rust)
        })
        .await
    );
    let cm_before = srv.automod_patch_committed.load(Ordering::SeqCst);
    let done_before = srv.automod_patch_completed.load(Ordering::SeqCst);
    srv.automod_post_delays_by_key
        .lock()
        .unwrap()
        .insert(format!("{}:1", srv.slug_rust), 260);
    let _ = cw.update(app, |w, cx| {
        w.state.automod_draft.name = "Rule A".to_string();
        w.state.automod_draft.keywords = "alpha".to_string();
        w.submit_add_automod_rule(cx);
    });
    assert!(
        pump_until(app, Duration::from_secs(4), |_| {
            srv.automod_patch_committed.load(Ordering::SeqCst) > cm_before
        })
        .await,
        "T12-01a: Alice Rule A PATCH must commit before switching to Bot"
    );
    let _ = cw.update(app, |w, cx| {
        w.switch_active_actor(srv.bot_id.clone(), cx);
    });
    assert!(
        pump_until(app, Duration::from_secs(4), |cx| {
            let st = &cw.read(cx).state;
            !st.mod_loading
                && !st.communities_loading
                && st.mod_community.as_ref().is_some_and(|c| c.automod_rules.len() == 1)
        })
        .await,
        "T12-01a: Bot ModQueue must settle with [Rule A]"
    );
    let _ = cw.update(app, |w, cx| {
        w.state.automod_draft.name = "Rule B".to_string();
        w.state.automod_draft.keywords = "beta".to_string();
        w.submit_add_automod_rule(cx);
    });
    assert!(
        pump_until(app, Duration::from_secs(5), |cx| {
            let st = &cw.read(cx).state;
            st.automod_in_flight.is_empty()
                && !st.mod_loading
                && srv.automod_patch_completed.load(Ordering::SeqCst) >= done_before + 2
        })
        .await,
        "T12-01a: Both Bot Flight 2 and delayed Alice Flight 1 must finish"
    );
    // Drain one extra round-trip so Alice's Fence 1 callback is guaranteed processed
    let fg_drain_1a = srv.feed_get_completed.load(Ordering::SeqCst);
    let _ = cw.update(app, |w, cx| w.load_feed(cx));
    assert!(
        pump_until(app, Duration::from_secs(4), |cx| {
            !cw.read(cx).state.feed_loading
                && srv.feed_get_completed.load(Ordering::SeqCst) > fg_drain_1a
        })
        .await
    );
    let _ = cw.read_with(app, |w, _| {
        let mod_len = w.state.mod_community.as_ref().unwrap().automod_rules.len();
        let comm_len = w
            .state
            .find_community(&srv.slug_rust)
            .unwrap()
            .automod_rules
            .len();
        assert_eq!(mod_len, 2, "T12-01a: stale Alice Fence 1 must NOT clobber mod_community to 1 rule");
        assert_eq!(comm_len, 2, "T12-01a: stale Alice Fence 1 must NOT clobber communities cache to 1 rule");
    });

    // T12-01b: In-flight dirty follow-up survival (prevent server data loss when stale Alice Fence 1 lands mid-Flight B1)
    srv.comm_automod_rules
        .lock()
        .unwrap()
        .insert(srv.slug_rust.clone(), Vec::new());
    let _ = cw.update(app, |w, cx| {
        w.switch_active_actor(srv.alice_id.clone(), cx);
    });
    assert!(
        pump_until(app, Duration::from_secs(4), |cx| {
            let st = &cw.read(cx).state;
            !st.mod_loading
                && st.mod_community.as_ref().is_some_and(|c| c.automod_rules.is_empty())
        })
        .await
    );
    let cm_b = srv.automod_patch_committed.load(Ordering::SeqCst);
    let done_b = srv.automod_patch_completed.load(Ordering::SeqCst);
    {
        let mut post = srv.automod_post_delays_by_key.lock().unwrap();
        post.insert(format!("{}:1", srv.slug_rust), 220);
        let mut pre = srv.automod_pre_delays_by_key.lock().unwrap();
        pre.insert(format!("{}:2", srv.slug_rust), 340);
    }
    let _ = cw.update(app, |w, cx| {
        w.state.automod_draft.name = "Rule A".to_string();
        w.state.automod_draft.keywords = "alpha".to_string();
        w.submit_add_automod_rule(cx);
    });
    assert!(
        pump_until(app, Duration::from_secs(4), |_| {
            srv.automod_patch_committed.load(Ordering::SeqCst) > cm_b
        })
        .await
    );
    let _ = cw.update(app, |w, cx| {
        w.switch_active_actor(srv.bot_id.clone(), cx);
    });
    assert!(
        pump_until(app, Duration::from_secs(4), |cx| {
            let st = &cw.read(cx).state;
            !st.mod_loading
                && st.mod_community.as_ref().is_some_and(|c| c.automod_rules.len() == 1)
        })
        .await
    );
    let _ = cw.update(app, |w, cx| {
        w.state.automod_draft.name = "Rule B".to_string();
        w.state.automod_draft.keywords = "beta".to_string();
        w.submit_add_automod_rule(cx);
        let rule_b_id = w
            .state
            .mod_community
            .as_ref()
            .unwrap()
            .automod_rules
            .iter()
            .find(|r| r.name == "Rule B")
            .unwrap()
            .rule_id
            .clone();
        w.toggle_automod_rule_enabled(rule_b_id, cx);
        assert!(w.state.automod_dirty.contains(&srv.slug_rust));
    });
    // Wait until stale Alice Flight 1 has returned WHILE Bot Flight B1 is still sleeping pre-commit
    assert!(
        pump_until(app, Duration::from_secs(4), |_| {
            srv.automod_patch_completed.load(Ordering::SeqCst) >= done_b + 1
        })
        .await,
        "T12-01b: stale Alice Flight 1 must arrive while Bot Flight B1 is still in flight"
    );
    assert!(
        pump_until(app, Duration::from_secs(5), |cx| {
            let st = &cw.read(cx).state;
            st.automod_in_flight.is_empty()
                && st.automod_dirty.is_empty()
                && srv.automod_patch_completed.load(Ordering::SeqCst) >= done_b + 3
        })
        .await,
        "T12-01b: Alice F1 + Bot F1 + Bot dirty F2 must all finish"
    );
    let srv_rules = srv
        .comm_automod_rules
        .lock()
        .unwrap()
        .get(&srv.slug_rust)
        .cloned()
        .unwrap();
    let srv_names: Vec<&str> = srv_rules.iter().map(|r| r.name.as_str()).collect();
    assert_eq!(
        srv_names,
        vec!["Rule A", "Rule B"],
        "T12-01b: stale Alice Fence 1 must NOT corrupt Bot dirty Flight B2 into deleting Rule B on server!"
    );
    assert!(
        !srv_rules.iter().find(|r| r.name == "Rule B").unwrap().enabled,
        "T12-01b: Bot dirty toggle (Rule B enabled=false) must survive on server"
    );

    // T12-01c (R11-R2-OBS-1): Cross-slug automod_rules_gen_by_slug isolation
    {
        let mut map = srv.comm_automod_rules.lock().unwrap();
        let py = map.entry(srv.slug_python.clone()).or_default();
        py.push(AutoModRuleDto {
            rule_id: "am_py_fresh".to_string(),
            community_id: format!("comm_{}", srv.slug_python),
            name: "Py Server Fresh".to_string(),
            enabled: true,
            priority: 20,
            keywords: vec!["pyfresh".to_string()],
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
            created_at_ms: 1_760_000_001_000,
        });
    }
    let rx_c = srv.automod_patch_received.load(Ordering::SeqCst);
    let done_c = srv.automod_patch_completed.load(Ordering::SeqCst);
    srv.automod_pre_delays_by_key
        .lock()
        .unwrap()
        .insert(format!("{}:2", srv.slug_rust), 80);
    srv.comm_get_post_delays_by_slug
        .lock()
        .unwrap()
        .insert(srv.slug_python.clone(), 240);
    let gen_before_c = cw.read_with(app, |w, _| w.state.automod_rules_gen).unwrap();
    let _ = cw.update(app, |w, cx| {
        let rid = w.state.mod_community.as_ref().unwrap().automod_rules[0]
            .rule_id
            .clone();
        w.toggle_automod_rule_enabled(rid, cx);
    });
    assert!(
        pump_until(app, Duration::from_secs(4), |_| {
            srv.automod_patch_received.load(Ordering::SeqCst) > rx_c
        })
        .await
    );
    let _ = cw.update(app, |w, cx| {
        w.navigate_to(CommunityRoute::ModQueue(Some(srv.slug_python.clone())), cx);
    });
    assert!(
        pump_until(app, Duration::from_secs(5), |cx| {
            let st = &cw.read(cx).state;
            !st.mod_loading
                && st.automod_in_flight.is_empty()
                && srv.automod_patch_completed.load(Ordering::SeqCst) > done_c
        })
        .await
    );
    let _ = cw.read_with(app, |w, _| {
        assert_ne!(
            w.state.automod_rules_gen, gen_before_c,
            "T12-01c setup: rust save must have bumped global automod_rules_gen while python loaded"
        );
        let py_rules = &w.state.mod_community.as_ref().unwrap().automod_rules;
        assert_eq!(
            py_rules.len(),
            2,
            "T12-01c (R11-R2-OBS-1): rust AutoMod commit must NOT overwrite fresh python server rules with stale cache"
        );
        assert!(py_rules.iter().any(|r| r.name == "Py Server Fresh"));
    });
}

async fn run_r12_t02_t03_tombstone_and_mod_queue(
    app: &mut AsyncApp,
    cw: &Entity<CommunityWorkspace>,
    srv: &Arc<R12MockState>,
) {
    use gpui_telegram_proto::community_contracts::ModQueueItemDto;
    use std::sync::atomic::Ordering;

    // ------------------------------------------------------------------
    // T12-02 (R11-R1-2 / R11-HUNT-3 / R11-ADV-NEW-3):
    // Delayed submit_edit_comment must NEVER resurrect text onto deleted parent tombstone!
    // ------------------------------------------------------------------
    let _ = cw.update(app, |w, cx| {
        w.switch_active_actor(srv.alice_id.clone(), cx);
        w.navigate_to(CommunityRoute::PostDetail(srv.p_tree.clone()), cx);
    });
    assert!(
        pump_until(app, Duration::from_secs(4), |cx| {
            let st = &cw.read(cx).state;
            !st.detail_loading
                && st
                    .detail_post
                    .as_ref()
                    .is_some_and(|d| d.post.post_id == srv.p_tree && !d.comments.is_empty())
        })
        .await
    );
    let cp_cm = srv.comment_patch_committed.load(Ordering::SeqCst);
    let cp_done = srv.comment_patch_completed.load(Ordering::SeqCst);
    let dc_done = srv.delete_comment_completed.load(Ordering::SeqCst);
    srv.comment_patch_post_delay_ms.store(260, Ordering::SeqCst);
    let _ = cw.update(app, |w, cx| {
        w.start_edit_comment(srv.c_parent.clone(), "Original Parent Body".to_string(), cx);
        w.state.detail_edit_comment_draft = "SENSITIVE_EDITED_BEFORE_DELETE".to_string();
        w.submit_edit_comment(cx);
    });
    assert!(
        pump_until(app, Duration::from_secs(4), |_| {
            srv.comment_patch_committed.load(Ordering::SeqCst) > cp_cm
        })
        .await,
        "T12-02: PATCH /comments/c_parent must commit on server while sleeping post-commit"
    );
    let _ = cw.update(app, |w, cx| {
        w.delete_comment_action(srv.c_parent.clone(), cx);
    });
    assert!(
        pump_until(app, Duration::from_secs(4), |cx| {
            let st = &cw.read(cx).state;
            !st.detail_loading
                && srv.delete_comment_completed.load(Ordering::SeqCst) > dc_done
                && st
                    .find_comment(&srv.c_parent)
                    .is_some_and(|c| c.is_deleted && c.body == "[deleted]")
        })
        .await,
        "T12-02: delete_comment_action must settle tombstone before delayed edit returns"
    );
    assert!(
        pump_until(app, Duration::from_secs(5), |_| {
            srv.comment_patch_completed.load(Ordering::SeqCst) > cp_done
        })
        .await,
        "T12-02: delayed PATCH HTTP 200 must complete"
    );
    // Ensure GPUI foreground executor has drained the delayed PATCH callback via a fast round-trip
    let fg_before = srv.feed_get_completed.load(Ordering::SeqCst);
    let _ = cw.update(app, |w, cx| w.load_feed(cx));
    assert!(
        pump_until(app, Duration::from_secs(4), |cx| {
            !cw.read(cx).state.feed_loading
                && srv.feed_get_completed.load(Ordering::SeqCst) > fg_before
        })
        .await
    );
    let _ = cw.read_with(app, |w, _| {
        let parent = w
            .state
            .find_comment(&srv.c_parent)
            .expect("T12-02: tombstoned c_parent must stay in tree");
        assert!(parent.is_deleted, "T12-02: c_parent must remain is_deleted=true");
        assert_eq!(
            parent.body, "[deleted]",
            "T12-02: delayed edit MUST NOT overwrite [deleted] tombstone body!"
        );
        assert!(
            parent.children.iter().any(|ch| ch.comment_id == srv.c_child),
            "T12-02: child c_child must be preserved under tombstone"
        );
    });

    // ------------------------------------------------------------------
    // T12-03a (R11-ADV-NEW-2A): Single-actor PostDetail mod Remove -> settled ModQueue
    // ------------------------------------------------------------------
    let _ = cw.update(app, |w, cx| {
        w.navigate_to(CommunityRoute::PostDetail(srv.p_mod1.clone()), cx);
    });
    assert!(
        pump_until(app, Duration::from_secs(4), |cx| {
            let st = &cw.read(cx).state;
            !st.detail_loading
                && st
                    .detail_post
                    .as_ref()
                    .is_some_and(|d| d.post.post_id == srv.p_mod1)
        })
        .await
    );
    let ma_rx_3a = srv.mod_action_received.load(Ordering::SeqCst);
    let ma_done_3a = srv.mod_action_completed.load(Ordering::SeqCst);
    srv.mod_action_pre_delay_ms.store(220, Ordering::SeqCst);
    let _ = cw.update(app, |w, cx| {
        w.execute_post_mod_action(srv.slug_rust.clone(), srv.p_mod1.clone(), ModActionKind::Remove, cx);
    });
    assert!(
        pump_until(app, Duration::from_secs(4), |_| {
            srv.mod_action_received.load(Ordering::SeqCst) > ma_rx_3a
        })
        .await
    );
    let _ = cw.update(app, |w, cx| {
        w.navigate_to(CommunityRoute::ModQueue(Some(srv.slug_rust.clone())), cx);
    });
    assert!(
        pump_until(app, Duration::from_secs(4), |cx| {
            let st = &cw.read(cx).state;
            !st.mod_loading && st.mod_queue.len() == 1 && st.mod_logs.is_empty()
        })
        .await,
        "T12-03a setup: ModQueue must settle with pending q_r12_1 before Remove commits"
    );
    assert!(
        pump_until(app, Duration::from_secs(5), |cx| {
            let st = &cw.read(cx).state;
            !st.mod_loading
                && srv.mod_action_completed.load(Ordering::SeqCst) > ma_done_3a
                && st.mod_queue.is_empty()
                && st.mod_logs.len() == 1
        })
        .await,
        "T12-03a (R11-ADV-NEW-2A): settled ModQueue must clear resolved q_r12_1 and load mod_logs!"
    );

    // ------------------------------------------------------------------
    // T12-03b (R11-ADV-NEW-2B / R11-HUNT-2c): Cross-actor execute_mod_decision on settled ModQueue
    // ------------------------------------------------------------------
    srv.mod_queue_by_slug
        .lock()
        .unwrap()
        .entry(srv.slug_rust.clone())
        .or_default()
        .push(ModQueueItemDto {
            item_id: "q_r12_2".to_string(),
            community_id: format!("comm_{}", srv.slug_rust),
            target_kind: "post".to_string(),
            target_id: srv.p_mod2.clone(),
            reporter_actor_id: Some(srv.bot_id.clone()),
            source: "report".to_string(),
            reason: "Mod check 2".to_string(),
            rule_triggered: None,
            status: ModQueueStatus::Pending,
            resolved_by_actor_id: None,
            resolved_at_ms: None,
            created_at_ms: 1_760_000_000_200,
        });
    let _ = cw.update(app, |w, cx| {
        w.load_mod_queue(srv.slug_rust.clone(), cx);
    });
    assert!(
        pump_until(app, Duration::from_secs(4), |cx| {
            let st = &cw.read(cx).state;
            !st.mod_loading && st.mod_queue.iter().any(|q| q.item_id == "q_r12_2")
        })
        .await
    );
    let ma_rx_3b = srv.mod_action_received.load(Ordering::SeqCst);
    let ma_done_3b = srv.mod_action_completed.load(Ordering::SeqCst);
    srv.mod_action_pre_delay_ms.store(220, Ordering::SeqCst);
    let _ = cw.update(app, |w, cx| {
        w.execute_mod_decision(
            srv.slug_rust.clone(),
            "q_r12_2".to_string(),
            "post".to_string(),
            srv.p_mod2.clone(),
            ModActionKind::Remove,
            cx,
        );
    });
    assert!(
        pump_until(app, Duration::from_secs(4), |_| {
            srv.mod_action_received.load(Ordering::SeqCst) > ma_rx_3b
        })
        .await
    );
    assert_eq!(
        srv.last_mod_action_actor.lock().unwrap().as_deref(),
        Some(srv.alice_id.as_str()),
        "T12-03b: execute_mod_decision must arrive with Alice header before actor switch"
    );
    let _ = cw.update(app, |w, cx| {
        w.switch_active_actor(srv.bot_id.clone(), cx);
    });
    assert!(
        pump_until(app, Duration::from_secs(4), |cx| {
            let st = &cw.read(cx).state;
            !st.mod_loading && st.mod_queue.iter().any(|q| q.item_id == "q_r12_2")
        })
        .await,
        "T12-03b setup: co-mod Bot ModQueue must settle with q_r12_2 before Alice Remove commits"
    );
    assert!(
        pump_until(app, Duration::from_secs(5), |cx| {
            let st = &cw.read(cx).state;
            !st.mod_loading
                && srv.mod_action_completed.load(Ordering::SeqCst) > ma_done_3b
                && st.mod_queue.is_empty()
                && st.mod_logs.len() >= 2
        })
        .await,
        "T12-03b (R11-ADV-NEW-2B / R11-HUNT-2c): co-mod Bot settled ModQueue must clear q_r12_2 and reload mod_logs!"
    );
}

async fn run_r12_t04_t05_t06_settled_routes_and_same_tick_guard(
    app: &mut AsyncApp,
    cw: &Entity<CommunityWorkspace>,
    srv: &Arc<R12MockState>,
) {
    use std::sync::atomic::Ordering;

    // ------------------------------------------------------------------
    // T12-04a (R11-ADV-NEW-4 / R11-HUNT-2a): Cross-actor submit_detail_reply on settled PostDetail
    // ------------------------------------------------------------------
    let _ = cw.update(app, |w, cx| {
        w.switch_active_actor(srv.alice_id.clone(), cx);
        w.navigate_to(CommunityRoute::PostDetail(srv.p_empty.clone()), cx);
    });
    assert!(
        pump_until(app, Duration::from_secs(4), |cx| {
            let st = &cw.read(cx).state;
            !st.detail_loading
                && !st.feed_loading
                && st
                    .detail_post
                    .as_ref()
                    .is_some_and(|d| d.post.post_id == srv.p_empty && d.comments.is_empty())
        })
        .await
    );
    let cc_rx = srv.create_comment_received.load(Ordering::SeqCst);
    let cc_done = srv.create_comment_completed.load(Ordering::SeqCst);
    srv.create_comment_pre_delay_ms.store(220, Ordering::SeqCst);
    let _ = cw.update(app, |w, cx| {
        w.state.detail_reply_draft = "Cross-actor reply from Alice".to_string();
        w.submit_detail_reply(cx);
    });
    assert!(
        pump_until(app, Duration::from_secs(4), |_| {
            srv.create_comment_received.load(Ordering::SeqCst) > cc_rx
        })
        .await
    );
    assert_eq!(
        srv.last_create_comment_actor.lock().unwrap().as_deref(),
        Some(srv.alice_id.as_str()),
        "T12-04a: submit_detail_reply must arrive with Alice header before actor switch"
    );
    let _ = cw.update(app, |w, cx| {
        w.switch_active_actor(srv.bot_id.clone(), cx);
    });
    assert!(
        pump_until(app, Duration::from_secs(4), |cx| {
            let st = &cw.read(cx).state;
            !st.detail_loading
                && st
                    .detail_post
                    .as_ref()
                    .is_some_and(|d| d.post.post_id == srv.p_empty && d.comments.is_empty())
        })
        .await,
        "T12-04a setup: Bot PostDetail must settle with 0 comments before Alice reply commits"
    );
    assert!(
        pump_until(app, Duration::from_secs(5), |cx| {
            let st = &cw.read(cx).state;
            !st.detail_loading
                && srv.create_comment_completed.load(Ordering::SeqCst) > cc_done
                && st.detail_post.as_ref().is_some_and(|d| {
                    d.comments.len() == 1
                        && d.post.comments_count == 1
                        && d.comments[0].body == "Cross-actor reply from Alice"
                })
        })
        .await,
        "T12-04a (R11-ADV-NEW-4 / R11-HUNT-2a): Bot settled PostDetail must reload Alice's committed reply!"
    );

    // ------------------------------------------------------------------
    // T12-04b (R11-HUNT-2b): Cross-actor summon_bot_with_options on settled PostDetail
    // ------------------------------------------------------------------
    let _ = cw.update(app, |w, cx| {
        w.switch_active_actor(srv.alice_id.clone(), cx);
    });
    assert!(
        pump_until(app, Duration::from_secs(4), |cx| {
            let st = &cw.read(cx).state;
            !st.detail_loading && !st.feed_loading && st.active_actor_id == srv.alice_id
        })
        .await
    );
    let sum_rx = srv.summon_received.load(Ordering::SeqCst);
    let sum_done = srv.summon_completed.load(Ordering::SeqCst);
    srv.summon_pre_delay_ms.store(220, Ordering::SeqCst);
    let _ = cw.update(app, |w, cx| {
        w.summon_bot_with_options(
            srv.p_empty.clone(),
            Some(srv.bot_id.clone()),
            Some("Summarize thread".to_string()),
            None,
            cx,
        );
    });
    assert!(
        pump_until(app, Duration::from_secs(4), |_| {
            srv.summon_received.load(Ordering::SeqCst) > sum_rx
        })
        .await
    );
    assert_eq!(
        srv.last_summon_actor.lock().unwrap().as_deref(),
        Some(srv.alice_id.as_str()),
        "T12-04b: summon_bot_with_options must arrive with Alice header before actor switch"
    );
    let _ = cw.update(app, |w, cx| {
        w.switch_active_actor(srv.bot_id.clone(), cx);
    });
    assert!(
        pump_until(app, Duration::from_secs(4), |cx| {
            let st = &cw.read(cx).state;
            !st.detail_loading
                && st
                    .detail_post
                    .as_ref()
                    .is_some_and(|d| d.post.post_id == srv.p_empty && d.comments.len() == 1)
        })
        .await,
        "T12-04b setup: Bot PostDetail must settle with 1 comment before summon commits"
    );
    assert!(
        pump_until(app, Duration::from_secs(5), |cx| {
            let st = &cw.read(cx).state;
            !st.detail_loading
                && srv.summon_completed.load(Ordering::SeqCst) > sum_done
                && st.detail_post.as_ref().is_some_and(|d| {
                    d.comments.len() == 2
                        && d.post.comments_count == 2
                        && d.comments.iter().any(|c| c.body == "Summoned Bot Reply R12")
                })
        })
        .await,
        "T12-04b (R11-HUNT-2b): Bot settled PostDetail must reload summoned bot reply!"
    );

    // ------------------------------------------------------------------
    // T12-04c (R11-HUNT-2d): Cross-actor submit_quick_post on settled Feed
    // ------------------------------------------------------------------
    let _ = cw.update(app, |w, cx| {
        w.switch_active_actor(srv.alice_id.clone(), cx);
        w.navigate_to(CommunityRoute::Feed, cx);
    });
    assert!(
        pump_until(app, Duration::from_secs(4), |cx| {
            let st = &cw.read(cx).state;
            !st.feed_loading && st.active_actor_id == srv.alice_id
        })
        .await
    );
    let cp_rx = srv.create_post_received.load(Ordering::SeqCst);
    let cp_done = srv.create_post_completed.load(Ordering::SeqCst);
    srv.create_post_pre_delay_ms.store(220, Ordering::SeqCst);
    let _ = cw.update(app, |w, cx| {
        w.state.quick_post_body = "Alice Cross-Actor Public Post".to_string();
        w.submit_quick_post(cx);
    });
    assert!(
        pump_until(app, Duration::from_secs(4), |_| {
            srv.create_post_received.load(Ordering::SeqCst) > cp_rx
        })
        .await
    );
    assert_eq!(
        srv.last_create_post_actor.lock().unwrap().as_deref(),
        Some(srv.alice_id.as_str()),
        "T12-04c: submit_quick_post must arrive with Alice header before actor switch"
    );
    let _ = cw.update(app, |w, cx| {
        w.switch_active_actor(srv.bot_id.clone(), cx);
    });
    assert!(
        pump_until(app, Duration::from_secs(4), |cx| {
            let st = &cw.read(cx).state;
            !st.feed_loading
                && st.active_actor_id == srv.bot_id
                && !st
                    .feed_items
                    .iter()
                    .any(|it| it.post.body == "Alice Cross-Actor Public Post")
        })
        .await,
        "T12-04c setup: Bot Feed must settle without Alice's delayed post first"
    );
    assert!(
        pump_until(app, Duration::from_secs(5), |cx| {
            let st = &cw.read(cx).state;
            !st.feed_loading
                && srv.create_post_completed.load(Ordering::SeqCst) > cp_done
                && st
                    .feed_items
                    .iter()
                    .any(|it| it.post.body == "Alice Cross-Actor Public Post")
        })
        .await,
        "T12-04c (R11-HUNT-2d): Bot settled Feed must reload Alice's newly published public post!"
    );
    let _ = cw.read_with(app, |w, _| {
        let p = &w
            .state
            .feed_items
            .iter()
            .find(|it| it.post.body == "Alice Cross-Actor Public Post")
            .unwrap()
            .post;
        assert_eq!(
            p.viewer_vote, 0,
            "T12-04c: cross-actor reload must NOT leak Alice's viewer_vote=+1 into Bot's feed"
        );
    });

    // ------------------------------------------------------------------
    // T12-05 (R11-R2-2 / R11-HUNT-5): delete_comment_action racing slow load_feed Case B
    // ------------------------------------------------------------------
    let _ = cw.update(app, |w, cx| {
        w.navigate_to(CommunityRoute::PostDetail(srv.p_empty.clone()), cx);
    });
    assert!(
        pump_until(app, Duration::from_secs(4), |cx| {
            let st = &cw.read(cx).state;
            !st.detail_loading
                && st
                    .detail_post
                    .as_ref()
                    .is_some_and(|d| d.post.post_id == srv.p_empty && d.comments.len() == 2)
        })
        .await
    );
    let del_cid = cw
        .read_with(app, |w, _| {
            w.state.detail_post.as_ref().unwrap().comments[0]
                .comment_id
                .clone()
        })
        .unwrap();
    let fg_rx5 = srv.feed_get_received.load(Ordering::SeqCst);
    let fg_done5 = srv.feed_get_completed.load(Ordering::SeqCst);
    let dc_done5 = srv.delete_comment_completed.load(Ordering::SeqCst);
    srv.feed_get_post_delay_ms.store(260, Ordering::SeqCst);
    let _ = cw.update(app, |w, cx| {
        w.load_feed(cx);
    });
    assert!(
        pump_until(app, Duration::from_secs(4), |cx| {
            cw.read(cx).state.feed_loading && srv.feed_get_received.load(Ordering::SeqCst) > fg_rx5
        })
        .await,
        "T12-05 setup: slow GET /feed #1 must snapshot stale comments_count=2"
    );
    let _ = cw.update(app, |w, cx| {
        w.delete_comment_action(del_cid.clone(), cx);
    });
    assert!(
        pump_until(app, Duration::from_secs(5), |cx| {
            let st = &cw.read(cx).state;
            !st.feed_loading
                && !st.detail_loading
                && srv.delete_comment_completed.load(Ordering::SeqCst) > dc_done5
                && srv.feed_get_completed.load(Ordering::SeqCst) >= fg_done5 + 2
        })
        .await,
        "T12-05 (R11-R2-2 / R11-HUNT-5): delete_comment_action must trigger Case B load_feed #2!"
    );
    let _ = cw.read_with(app, |w, _| {
        let feed_cnt = w
            .state
            .feed_items
            .iter()
            .find(|it| it.post.post_id == srv.p_empty)
            .unwrap()
            .post
            .comments_count;
        let det_cnt = w.state.detail_post.as_ref().unwrap().post.comments_count;
        assert_eq!(feed_cnt, 1, "T12-05: feed_items comments_count must match 1 (not stale 2!)");
        assert_eq!(det_cnt, 1, "T12-05: detail_post comments_count must match 1");
    });

    // ------------------------------------------------------------------
    // T12-06 (R11-HUNT-4b): Same-tick vote_post + switch_active_actor("b:...") zero header theft
    // ------------------------------------------------------------------
    let _ = cw.update(app, |w, cx| {
        w.navigate_to(CommunityRoute::Feed, cx);
        w.switch_active_actor(srv.alice_id.clone(), cx);
    });
    assert!(
        pump_until(app, Duration::from_secs(4), |cx| {
            let st = &cw.read(cx).state;
            !st.feed_loading && !st.communities_loading && st.active_actor_id == srv.alice_id
        })
        .await
    );
    srv.vote_calls_for_bot.store(0, Ordering::SeqCst);
    let _ = cw.update(app, |w, cx| {
        w.vote_post(srv.p_empty.clone(), 1, cx);
        w.switch_active_actor(srv.bot_id.clone(), cx);
    });
    assert!(
        pump_until(app, Duration::from_secs(4), |cx| {
            let st = &cw.read(cx).state;
            !st.feed_loading
                && !st.communities_loading
                && !st.notifications_loading
                && !st.recommended_actors_loading
                && st.active_actor_id == srv.bot_id
        })
        .await
    );
    // One extra round-trip ensures all spawned GPUI/Tokio futures have run and aborted cleanly
    let fg_drain = srv.feed_get_completed.load(Ordering::SeqCst);
    let _ = cw.update(app, |w, cx| w.load_feed(cx));
    assert!(
        pump_until(app, Duration::from_secs(4), |cx| {
            !cw.read(cx).state.feed_loading
                && srv.feed_get_completed.load(Ordering::SeqCst) > fg_drain
        })
        .await
    );
    assert_eq!(
        srv.vote_calls_for_bot.load(Ordering::SeqCst),
        0,
        "T12-06 (R11-HUNT-4b): same-tick vote_post + switch_active_actor must send ZERO votes under Bot header!"
    );
    let srv_bot_vote = srv
        .votes_by_actor_post
        .lock()
        .unwrap()
        .get(&(srv.bot_id.clone(), srv.p_empty.clone()))
        .copied()
        .unwrap_or(0);
    assert_eq!(srv_bot_vote, 0, "T12-06: server Bot vote must remain 0");
    let _ = cw.read_with(app, |w, _| {
        let ui_bot_vote = w
            .state
            .feed_items
            .iter()
            .find(|it| it.post.post_id == srv.p_empty)
            .unwrap()
            .post
            .viewer_vote;
        assert_eq!(ui_bot_vote, 0, "T12-06: UI Bot viewer_vote must remain 0");
    });
}








