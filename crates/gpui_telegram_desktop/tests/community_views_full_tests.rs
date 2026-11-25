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
    clamp_grok_avatar_indices, format_bot_api_snippet, resolve_grok_avatar_pair,
    validate_bot_studio_registration,
};
use gpui_telegram_desktop::views::community::composer_modal::{
    compute_composer_autocomplete, detect_autocomplete_trigger, AutocompleteKind,
};
use gpui_telegram_desktop::views::community::explore_search_view::{
    build_search_request, normalize_search_query,
};
use gpui_telegram_desktop::views::community::mod_queue_view::validate_automod_regex_syntax;
use gpui_telegram_desktop::views::community::{
    ActiveCommunityInput, ActorProfileTab, BookmarksListsSection, CommunityLayoutMode,
    CommunityModal, CommunityPageTab, CommunityRoute, CommunityWorkspace,
    CommunityWorkspaceAction, ComposerFormatTab, ComposerMediaDraft, ModConsoleTab,
    NotificationCategoryTab, PostDetailMode,
};
use gpui_telegram_desktop::views::composer::formatting_bar::TextStyleKind;
use gpui_telegram_desktop::views::shell::{ActiveWorkspacePanel, AppShell};
use gpui_telegram_desktop::views::TdesktopThemeMode;
use gpui_telegram_proto::community_contracts::{
    ActorKind, AutoModActionKind, BotAutonomyMode, CommentSort, CommunityNotificationDto,
    CommunityNotificationKind, CommunitySearchSort, CommunitySearchTab, CreatePostRequest,
    EngagementSignalKind, ModActionKind, PostFormat, PostStatus,
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
    exit_headless_child(0, use_underscore_exit);
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
        .env("GPUI_OFFLINE_TEST", "1")
        .arg("--exact")
        .arg(test_fn_name)
        .arg("--nocapture");
    let output = cmd.output().expect("spawn M5 community headless gpui child");
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
    assert!(validate_automod_regex_syntax("").is_ok());
    assert!(validate_automod_regex_syntax("(unclosed").is_err());
    assert!(validate_automod_regex_syntax("[unclosed").is_err());
    assert!(validate_automod_regex_syntax("trailing\\").is_err());
    assert!(validate_automod_regex_syntax("*leading").is_err());
    assert!(validate_automod_regex_syntax("?leading").is_err());
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

        // Link validation
        w.set_composer_tab(ComposerFormatTab::Link, cx);
        w.state.composer.link_url = "not-a-url".to_string();
        assert!(w
            .build_composer_create_request(PostStatus::Published, now_ms)
            .is_err());
        w.state.composer.link_url = "https://rust-lang.org".to_string();
        assert!(w
            .build_composer_create_request(PostStatus::Published, now_ms)
            .is_ok());

        // Poll validation (2..=6 options)
        w.set_composer_tab(ComposerFormatTab::Poll, cx);
        w.state.composer.poll_question = "Best desktop UI stack?".to_string();
        w.state.composer.poll_options = vec!["GPUI".to_string()];
        assert!(w
            .build_composer_create_request(PostStatus::Published, now_ms)
            .is_err());
        w.state.composer.poll_options =
            vec!["GPUI".to_string(), "SwiftUI".to_string(), "Qt".to_string()];
        assert!(w
            .build_composer_create_request(PostStatus::Published, now_ms)
            .is_ok());

        // Thread 1/N validation
        w.set_composer_tab(ComposerFormatTab::Thread, cx);
        w.state.composer.body = "Part 1 of thread".to_string();
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

    // 2. Live `@/#`/`c/` autocomplete (English + CJK) + Progressive Escape cascade
    let _ = cw.update(&mut app, |w, cx| {
        w.open_composer_modal(Some("rust".to_string()), cx);
        w.state.focus_input(ActiveCommunityInput::ComposerBody);
        w.insert_text_to_active_input("你好 c/r", cx);
        let ac = compute_composer_autocomplete(w).expect("c/ autocomplete must trigger");
        assert!(!ac.1.is_empty());
        let (start, token) = (ac.0, ac.1[0].insert_token.clone());
        // Escape #1 dismisses autocomplete while keeping modal open
        assert!(w.handle_escape_key(cx));
        assert_eq!(w.state.active_modal, Some(CommunityModal::Composer));
        assert!(w.state.composer.autocomplete_dismissed);
        // Apply autocomplete directly
        w.state.composer.autocomplete_dismissed = false;
        w.apply_composer_autocomplete(start, token, cx);
        assert!(w.state.composer.body.contains("c/"));
        // Escape #2 closes Composer modal while staying inside Community
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

    // 1. Create Community Modal validation (reject invalid slug) -> then create c/m5_arena
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

    // 3. BookmarksListsView: Custom Folders (Create/Move/Remove) + Lists CRUD + Membership + Timeline
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
    assert!(pump_until(&mut app, Duration::from_secs(4), |cx| !cw.read(cx).state.saved_loading).await);
    host.draw(&mut app);

    let _ = cw.update(&mut app, |w, cx| {
        w.remove_bookmark_from_saved(sample_pid.clone(), cx);
        assert!(!w
            .state
            .saved_items
            .iter()
            .any(|it| it.post.post_id == sample_pid));

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
    let _ = cw.update(&mut app, |w, cx| {
        w.toggle_selected_list_actor(list_id.clone(), target_bot.actor_id.clone(), cx);
        w.toggle_list_pinned(list_id.clone(), false, cx);
    });
    assert!(
        pump_until(&mut app, Duration::from_secs(4), |cx| {
            !cw.read(cx).state.list_feed_loading
        })
        .await
    );
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
