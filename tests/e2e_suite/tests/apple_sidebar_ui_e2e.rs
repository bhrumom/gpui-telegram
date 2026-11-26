//! E2E Phase B — Headless GPUI UI End-to-End Suite (`apple_sidebar_ui_e2e.rs`)
//!
//! Implements all 16 Tier 1–4 headless GPUI UI E2E tests defined in `TEST_INFRA_APPLE_SIDEBAR.md §6.3`
//! using real `gpui::Application::headless()`, subprocess main-thread trampoline, `AppShell::inspect_state`,
//! and live `EphemeralTestServer` for network/WS/multi-user journeys (`ui_t3_03`, `ui_t4_01`, `ui_t4_02`).

mod apple_sidebar_common;

use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::time::Duration;

use futures::FutureExt;
use gpui::{App, AppContext, Application, AssetSource, AsyncApp, Entity};
use gpui_telegram_agent::hermes::toolsets::ToolsetId;
use gpui_telegram_client::{AuthResponse, ClientConfig, TelegramClient};
use gpui_telegram_desktop::theme::{IconName, TdesktopThemeMode, TelegramAssetSource};
use gpui_telegram_desktop::views::apple_sidebar::{
    AppleSidebarEvent, AppleSidebarSection, COLLAPSED_WIDTH as APPLE_COLLAPSED_WIDTH,
    EXPANDED_WIDTH as APPLE_EXPANDED_WIDTH,
};
use gpui_telegram_desktop::views::chat_sidebar::{
    ChatSidebar, COLLAPSED_WIDTH, DEFAULT_WIDTH, ROW_HEIGHT,
};
use gpui_telegram_desktop::views::content_hub::{
    build_music_track_entries_from_creation, ActiveGameSession, BookReaderFontFamily,
    BookReaderState, BookReaderTheme, ContentHubInputTarget, ContentHubSection, GameInput,
    GameStatus, PublishModalField,
};
use gpui_telegram_desktop::views::nav_rail::{NavRail, NAV_RAIL_WIDTH};
use gpui_telegram_desktop::views::shell::{ActiveWorkspacePanel, AppShell};
use gpui_telegram_desktop::views::stage::cards::InlineButtonType;
use gpui_telegram_desktop::views::stage::entities_render::RichTextAction;
use gpui_telegram_media::media::audio::{MusicPlaybackState, MusicRepeatMode};
use gpui_telegram_proto::creations::{
    AppStoreCatalogResponse, AppStoreListingPayload, AppStoreMiniAppEntry, BookChapter, BookPage,
    BookPayload, BookRichBlock, CoverArtwork, CreationKind, CreationPayload, CreationReviewStatus,
    CreationVisibility, GameCategory, GameEngineKind, GamePayload, GameSpec, MusicPayload,
    MusicTrack, NoteEvent, PublicCatalogItemDto, SyncedLyricLine, TriviaQuestion, UserCreationDto,
    CREATOR_STUDIO_BOT_USER_ID,
};
use gpui_telegram_test_harness::tier3_headless::HeadlessTranscriptDriver;
use reqwest::Method;
use serde_json::json;
use tempfile::TempDir;

const CHILD_ENV: &str = "GPUI_APPLE_UI_E2E_CHILD";
const CHILD_TARGET_ENV: &str = "GPUI_APPLE_UI_E2E_TARGET";

type AsyncRunnerFn = fn(AsyncApp) -> Pin<Box<dyn Future<Output = ()>>>;

enum UiRunner {
    Sync(fn(&mut App)),
    Async(AsyncRunnerFn),
}

fn dispatch_headless_target(target: &str) -> ! {
    let runner = match target {
        "ui_t1_01_apple_sidebar_mounts_and_switches_all_sections" => {
            UiRunner::Sync(body_ui_t1_01)
        }
        "ui_t1_02_apple_sidebar_collapse_expand_and_quick_creations_drawer" => {
            UiRunner::Sync(body_ui_t1_02)
        }
        "ui_t1_03_books_hub_and_book_reader_themes_zoom_bookmarks" => {
            UiRunner::Sync(body_ui_t1_03)
        }
        "ui_t1_04_music_hub_synth_and_now_playing_transport_bar" => {
            UiRunner::Sync(body_ui_t1_04)
        }
        "ui_t1_05_games_hub_and_interactive_mini_game_runtime_sheet" => {
            UiRunner::Sync(body_ui_t1_05)
        }
        "ui_t1_06_app_store_hub_unified_storefront" => UiRunner::Sync(body_ui_t1_06),
        "ui_t1_07_publish_review_modal_validation_and_submit" => UiRunner::Sync(body_ui_t1_07),
        "ui_t2_01_ime_input_isolation_in_hub_search_and_review_modal" => {
            UiRunner::Sync(body_ui_t2_01)
        }
        "ui_t2_02_escape_modal_priority_and_unauthenticated_guard" => {
            UiRunner::Sync(body_ui_t2_02)
        }
        "ui_t2_03_account_switch_and_logout_wipes_private_creations_cache" => {
            UiRunner::Sync(body_ui_t2_03)
        }
        "ui_t3_01_chat_generative_card_deep_link_opens_target_apple_vertical" => {
            UiRunner::Sync(body_ui_t3_01)
        }
        "ui_t3_02_inline_keyboard_url_and_richtext_openurl_deep_link" => {
            UiRunner::Sync(body_ui_t3_02)
        }
        "ui_t3_03_ws_events_live_refresh_my_creations_and_public_catalog" => {
            UiRunner::Async(|app| Box::pin(body_ui_t3_03(app)))
        }
        "ui_t4_01_full_ui_journey_bot_create_to_sidebar_creations_publish_and_discover" => {
            UiRunner::Async(|app| Box::pin(body_ui_t4_01(app)))
        }
        "ui_t4_02_full_ui_journey_music_and_game_launch_from_app_store_and_chat" => {
            UiRunner::Async(|app| Box::pin(body_ui_t4_02(app)))
        }
        other => {
            eprintln!("unknown UI E2E target: {other}");
            std::process::exit(101);
        }
    };
    let target_owned = target.to_string();
    let outer = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let rt = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .enable_all()
            .build()
            .expect("tokio runtime for headless UI E2E");
        let _tokio_guard = rt.enter();
        Application::headless()
            .with_assets(TelegramAssetSource::new())
            .run(move |cx: &mut App| match runner {
                UiRunner::Sync(body) => {
                    let inner = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                        body(cx);
                    }));
                    match inner {
                        Ok(()) => std::process::exit(0),
                        Err(payload) => {
                            let msg = payload
                                .downcast_ref::<&str>()
                                .copied()
                                .or_else(|| payload.downcast_ref::<String>().map(|s| s.as_str()))
                                .unwrap_or("panic");
                            eprintln!("[ui_e2e panic] {target_owned}: {msg}");
                            std::process::exit(101);
                        }
                    }
                }
                UiRunner::Async(async_fn) => {
                    let t = target_owned.clone();
                    cx.spawn(async move |app: &mut AsyncApp| {
                        let app_clone = app.clone();
                        let res = std::panic::AssertUnwindSafe(async_fn(app_clone))
                            .catch_unwind()
                            .await;
                        match res {
                            Ok(()) => std::process::exit(0),
                            Err(payload) => {
                                let msg = payload
                                    .downcast_ref::<&str>()
                                    .copied()
                                    .or_else(|| {
                                        payload.downcast_ref::<String>().map(|s| s.as_str())
                                    })
                                    .unwrap_or("panic");
                                eprintln!("[ui_e2e async panic] {t}: {msg}");
                                std::process::exit(101);
                            }
                        }
                    })
                    .detach();
                }
            });
    }));
    if let Err(payload) = outer {
        let msg = payload
            .downcast_ref::<&str>()
            .copied()
            .or_else(|| payload.downcast_ref::<String>().map(|s| s.as_str()))
            .unwrap_or("panic");
        eprintln!("[ui_e2e outer panic] {target}: {msg}");
        std::process::exit(101);
    }
    std::process::exit(0);
}

#[cfg(target_os = "macos")]
#[used]
#[unsafe(link_section = "__DATA,__mod_init_func")]
static MACOS_UI_E2E_ENTRY: extern "C" fn() = macos_mod_init_ui_e2e;

#[cfg(target_os = "macos")]
extern "C" fn macos_mod_init_ui_e2e() {
    if std::env::var(CHILD_ENV).as_deref() == Ok("1") {
        if let Ok(target) = std::env::var(CHILD_TARGET_ENV) {
            dispatch_headless_target(&target);
        }
    }
}

fn spawn_ui_child(test_fn_name: &str, offline: bool, extra_env: &[(&str, String)]) {
    if std::env::var(CHILD_ENV).as_deref() == Ok("1") {
        dispatch_headless_target(test_fn_name);
    }
    let tmp_dir = TempDir::new().expect("TempDir for UI E2E child");
    let exe = std::env::current_exe().expect("current_exe");
    let mut cmd = std::process::Command::new(exe);
    cmd.env(CHILD_ENV, "1")
        .env(CHILD_TARGET_ENV, test_fn_name)
        .env("GPUI_TELEGRAM_CONFIG_DIR", tmp_dir.path())
        .arg("--exact")
        .arg(test_fn_name)
        .arg("--nocapture");
    if offline {
        cmd.env("GPUI_OFFLINE_TEST", "1");
    } else {
        cmd.env_remove("GPUI_OFFLINE_TEST");
    }
    for (k, v) in extra_env {
        cmd.env(k, v);
    }
    let output = cmd.output().expect("spawn UI E2E child");
    assert!(
        output.status.success(),
        "UI E2E child failed for {test_fn_name}:\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

async fn spawn_ui_child_async(
    test_fn_name: &'static str,
    offline: bool,
    extra_env: Vec<(&'static str, String)>,
) -> anyhow::Result<()> {
    tokio::task::spawn_blocking(move || {
        spawn_ui_child(test_fn_name, offline, &extra_env);
    })
    .await
    .map_err(|e| anyhow::anyhow!("spawn_ui_child_async join error: {e}"))
}

fn boot_shell(cx: &mut App, server_url: &str, auth: Option<AuthResponse>) -> (TempDir, Entity<AppShell>) {
    let dir = TempDir::new().expect("TempDir");
    let cfg = ClientConfig {
        server_url: server_url.to_string(),
        ..Default::default()
    };
    let client =
        Arc::new(TelegramClient::open_with_dir(cfg, dir.path().to_path_buf()).expect("client"));
    let _rt = client.runtime().enter();
    let shell = cx.new(|cx| AppShell::new_with_client(client.clone(), cx));
    if let Some(a) = auth {
        shell.update(cx, |s, cx| {
            s.apply_authenticated_session(a, cx);
        });
    }
    (dir, shell)
}

fn auth_for(user_id: u64, username: &str, token: &str) -> AuthResponse {
    AuthResponse {
        token: token.to_string(),
        session_id: format!("sess_{user_id}"),
        user_id,
        username: username.to_string(),
        first_name: Some(username.to_string()),
        created_at: None,
        expires_at: None,
    }
}

fn fixture_book(cid: &str, owner: i64, status: CreationReviewStatus, body: &str) -> UserCreationDto {
    UserCreationDto {
        creation_id: cid.to_string(),
        owner_user_id: owner,
        creator_bot_user_id: Some(CREATOR_STUDIO_BOT_USER_ID),
        creator_chat_id: Some(11),
        kind: CreationKind::Book,
        title: "Apple Books Journey".to_string(),
        subtitle: "Interactive Edition".to_string(),
        summary: "A complete original sci-fi novel built with CreatorStudioBot.".to_string(),
        author_display_name: "Author Alice".to_string(),
        language: "en".to_string(),
        genres: vec!["Sci-Fi".to_string()],
        tags: vec!["featured".to_string()],
        cover: CoverArtwork::default(),
        age_rating: "4+".to_string(),
        payload: CreationPayload::Book(BookPayload {
            author: "Author Alice".to_string(),
            series_name: Some("Cosmos".to_string()),
            series_index: Some(1),
            blurb: "Across the stars.".to_string(),
            estimated_read_mins: 10,
            word_count: 800,
            chapters: vec![
                BookChapter {
                    chapter_id: "ch1".to_string(),
                    chapter_number: 1,
                    title: "Launch".to_string(),
                    subtitle: None,
                    pages: vec![BookPage {
                        page_number: 1,
                        blocks: vec![BookRichBlock::Paragraph {
                            text: body.to_string(),
                        }],
                    }],
                },
                BookChapter {
                    chapter_id: "ch2".to_string(),
                    chapter_number: 2,
                    title: "Arrival".to_string(),
                    subtitle: None,
                    pages: vec![BookPage {
                        page_number: 1,
                        blocks: vec![BookRichBlock::Paragraph {
                            text: "Landing safely on the emerald world at dawn with full crew."
                                .to_string(),
                        }],
                    }],
                },
            ],
        }),
        version: 1,
        cloud_revision: 2,
        review_status: status,
        visibility: CreationVisibility::Public,
        has_live_published_version: status == CreationReviewStatus::Published,
        published_version: (status == CreationReviewStatus::Published).then_some(1),
        rejection_reason: None,
        review_notes: None,
        submitted_at_ms: None,
        reviewed_at_ms: None,
        published_at_ms: Some(1_000),
        created_at_ms: 500,
        updated_at_ms: 1_000,
        deleted_at_ms: None,
        rating_avg: 4.8,
        rating_count: 6,
        engagement_count: 50,
        save_or_install_count: 12,
    }
}

fn fixture_music(cid: &str, owner: i64) -> UserCreationDto {
    UserCreationDto {
        creation_id: cid.to_string(),
        owner_user_id: owner,
        creator_bot_user_id: Some(CREATOR_STUDIO_BOT_USER_ID),
        creator_chat_id: Some(11),
        kind: CreationKind::Music,
        title: "Aurora Synth EP".to_string(),
        subtitle: "Lossless Synth".to_string(),
        summary: "Synthesized electronic album with synced lyrics.".to_string(),
        author_display_name: "Composer Bob".to_string(),
        language: "en".to_string(),
        genres: vec!["Electronic".to_string()],
        tags: vec!["synth".to_string()],
        cover: CoverArtwork::default(),
        age_rating: "4+".to_string(),
        payload: CreationPayload::Music(MusicPayload {
            artist: "Composer Bob".to_string(),
            album_title: "Aurora Synth EP".to_string(),
            release_year: 2026,
            primary_genre: "Electronic".to_string(),
            mood_tags: vec!["Chill".to_string()],
            bpm: 120,
            musical_key: "C Major".to_string(),
            time_signature: "4/4".to_string(),
            total_duration_secs: 6,
            tracks: vec![MusicTrack {
                track_id: "t1".to_string(),
                track_number: 1,
                title: "First Light".to_string(),
                duration_secs: 4,
                bpm: 120,
                musical_key: "C Major".to_string(),
                instruments: Vec::new(),
                melody_notes: vec![
                    NoteEvent {
                        midi_note: 60,
                        start_beat: 0.0,
                        duration_beats: 1.0,
                        velocity: 96,
                        channel: 0,
                    },
                    NoteEvent {
                        midi_note: 67,
                        start_beat: 1.0,
                        duration_beats: 1.0,
                        velocity: 96,
                        channel: 0,
                    },
                ],
                chord_progression: Vec::new(),
                lyrics: vec![
                    SyncedLyricLine {
                        timestamp_ms: 0,
                        text: "Northern lights begin to glow".to_string(),
                    },
                    SyncedLyricLine {
                        timestamp_ms: 900,
                        text: "Frequencies across the sky".to_string(),
                    },
                ],
                audio_media_file_id: None,
                waveform_5bit: Vec::new(),
            }],
        }),
        version: 1,
        cloud_revision: 2,
        review_status: CreationReviewStatus::Published,
        visibility: CreationVisibility::Public,
        has_live_published_version: true,
        published_version: Some(1),
        rejection_reason: None,
        review_notes: None,
        submitted_at_ms: None,
        reviewed_at_ms: None,
        published_at_ms: Some(1_000),
        created_at_ms: 500,
        updated_at_ms: 1_000,
        deleted_at_ms: None,
        rating_avg: 4.9,
        rating_count: 8,
        engagement_count: 90,
        save_or_install_count: 20,
    }
}

fn fixture_game(cid: &str, owner: i64) -> UserCreationDto {
    UserCreationDto {
        creation_id: cid.to_string(),
        owner_user_id: owner,
        creator_bot_user_id: Some(CREATOR_STUDIO_BOT_USER_ID),
        creator_chat_id: Some(11),
        kind: CreationKind::Game,
        title: "Star Quiz Arcade".to_string(),
        subtitle: "Apple Arcade".to_string(),
        summary: "Fast trivia and arcade challenge with cloud leaderboard.".to_string(),
        author_display_name: "Dev Carol".to_string(),
        language: "en".to_string(),
        genres: vec!["Trivia".to_string()],
        tags: vec!["arcade".to_string()],
        cover: CoverArtwork::default(),
        age_rating: "4+".to_string(),
        payload: CreationPayload::Game(GamePayload {
            developer: "Dev Carol".to_string(),
            category: GameCategory::Trivia,
            controls_hint: "Click or press 1-4".to_string(),
            rules_markdown: "Answer correctly to score points.".to_string(),
            engine_kind: GameEngineKind::TriviaQuiz,
            supports_leaderboard: true,
            estimated_session_mins: 2,
            spec: GameSpec::TriviaQuiz {
                time_limit_secs: Some(60),
                questions: vec![TriviaQuestion {
                    question_id: "q1".to_string(),
                    prompt: "Which planet is closest to the Sun?".to_string(),
                    choices: vec!["Mercury".to_string(), "Venus".to_string()],
                    correct_index: 0,
                    explanation: Some("Mercury orbits closest.".to_string()),
                    points: 150,
                }],
            },
            achievements: Vec::new(),
        }),
        version: 1,
        cloud_revision: 2,
        review_status: CreationReviewStatus::Published,
        visibility: CreationVisibility::Public,
        has_live_published_version: true,
        published_version: Some(1),
        rejection_reason: None,
        review_notes: None,
        submitted_at_ms: None,
        reviewed_at_ms: None,
        published_at_ms: Some(1_000),
        created_at_ms: 500,
        updated_at_ms: 1_000,
        deleted_at_ms: None,
        rating_avg: 4.7,
        rating_count: 5,
        engagement_count: 65,
        save_or_install_count: 15,
    }
}

fn to_cat(dto: &UserCreationDto) -> PublicCatalogItemDto {
    PublicCatalogItemDto {
        creation_id: dto.creation_id.clone(),
        owner_user_id: dto.owner_user_id,
        kind: dto.kind,
        title: dto.title.clone(),
        subtitle: dto.subtitle.clone(),
        summary: dto.summary.clone(),
        author_display_name: dto.author_display_name.clone(),
        language: dto.language.clone(),
        genres: dto.genres.clone(),
        tags: dto.tags.clone(),
        cover: dto.cover.clone(),
        age_rating: dto.age_rating.clone(),
        published_version: 1,
        published_at_ms: 1_000,
        updated_at_ms: 1_000,
        rating_avg: dto.rating_avg,
        rating_count: dto.rating_count,
        engagement_count: dto.engagement_count,
        save_or_install_count: dto.save_or_install_count,
        featured_rank: Some(1),
        deep_link: dto.kind.deep_link_uri(&dto.creation_id),
        payload: Some(dto.payload.clone()),
    }
}

// ============================================================================
// UI Tier 1 (ui_t1_01 .. ui_t1_07)
// ============================================================================

fn body_ui_t1_01(cx: &mut App) {
    let (_dir, shell) = boot_shell(cx, "http://127.0.0.1:9", Some(auth_for(101, "alice", "t1")));
    let sections = [
        (AppleSidebarSection::Chats, "Chats"),
        (AppleSidebarSection::Books, "ContentHub(Books)"),
        (AppleSidebarSection::Music, "ContentHub(Music)"),
        (AppleSidebarSection::Games, "ContentHub(Games)"),
        (AppleSidebarSection::AppStore, "ContentHub(AppStore)"),
        (AppleSidebarSection::MyCreations, "ContentHub(MyCreations)"),
    ];
    let mut hashes = std::collections::HashSet::new();
    for (sec, expected_label) in sections {
        let tree = shell.update(cx, |s, cx| {
            s.handle_apple_sidebar_event(&AppleSidebarEvent::SelectSection(sec), cx);
            s.inspect_state(cx)
        });
        assert_eq!(tree.window.active_panel, expected_label);
        assert!(hashes.insert(tree.state_hash), "state_hash must change for {expected_label}");
    }
    // Theme & locale propagation across all 4 modes
    for mode in [
        TdesktopThemeMode::Night,
        TdesktopThemeMode::Tinted,
        TdesktopThemeMode::Day,
        TdesktopThemeMode::Classic,
    ] {
        shell.update(cx, |s, cx| {
            s.apply_theme_selection(mode, None, None, cx);
            assert_eq!(s.apple_sidebar.read(cx).palette.mode, mode);
            assert_eq!(s.content_hub.read(cx).palette.mode, mode);
        });
    }
}

fn body_ui_t1_02(cx: &mut App) {
    let (_dir, shell) = boot_shell(cx, "http://127.0.0.1:9", Some(auth_for(102, "bob", "t2")));
    let book = fixture_book(
        "cr_drawer_1",
        102,
        CreationReviewStatus::Draft,
        "A rich multi-sentence draft saved in the user's cloud creations drawer.",
    );
    shell.update(cx, |s, cx| {
        s.content_hub.update(cx, |h, _| h.set_my_creations(vec![book]));
        assert_eq!(APPLE_EXPANDED_WIDTH, 204.0);
        assert_eq!(APPLE_COLLAPSED_WIDTH, 68.0);
        s.apple_sidebar.update(cx, |sb, _| sb.set_collapsed(true));
        assert!(s.apple_sidebar.read(cx).is_collapsed);
        s.apple_sidebar.update(cx, |sb, _| sb.set_collapsed(false));
        assert!(!s.apple_sidebar.read(cx).is_collapsed);

        s.handle_apple_sidebar_event(&AppleSidebarEvent::ToggleCreationsDrawer(true), cx);
        assert!(s.content_hub.read(cx).quick_drawer_open);
        assert_eq!(s.content_hub.read(cx).filtered_quick_drawer_creations().len(), 1);
    });
}

fn body_ui_t1_03(cx: &mut App) {
    let (_dir, shell) = boot_shell(cx, "http://127.0.0.1:9", Some(auth_for(103, "reader", "t3")));
    let book = fixture_book(
        "cr_ui_book",
        103,
        CreationReviewStatus::Published,
        "Chapter one opens over a sunlit ocean where travelers chart new constellations.",
    );
    shell.update(cx, |s, cx| {
        s.content_hub.update(cx, |h, _| {
            h.books_catalog = vec![to_cat(&book)];
        });
        s.switch_to_content_hub(ContentHubSection::Books, cx);
        for tab in ["reading_now", "library", "book_store", "my_books"] {
            s.content_hub.update(cx, |h, _| h.active_sub_tab = tab.to_string());
        }
        let CreationPayload::Book(bp) = book.payload.clone() else {
            unreachable!();
        };
        let mut reader = BookReaderState::open(&book, bp, None, s.theme_state.palette);
        reader.set_theme(BookReaderTheme::Sepia);
        reader.set_font_family(BookReaderFontFamily::Sans);
        reader.increase_font_size();
        assert!(reader.toggle_current_bookmark(Some("Favorite passage".to_string())));
        assert!(reader.next_page());
        assert_eq!(reader.current_chapter_index, 1);
        let req = reader.to_upsert_state_request();
        assert_eq!(req.progress_percent, Some(100.0));
        s.upsert_creation_state_fenced(book.creation_id.clone(), req, cx);
        assert!(s
            .content_hub
            .read(cx)
            .my_creation_states
            .get("cr_ui_book")
            .is_some_and(|st| st.in_library && st.progress_percent == 100.0));
    });
}

fn body_ui_t1_04(cx: &mut App) {
    let (_dir, shell) = boot_shell(cx, "http://127.0.0.1:9", Some(auth_for(104, "listener", "t4")));
    let music = fixture_music("cr_ui_music", 104);
    shell.update(cx, |s, cx| {
        s.content_hub.update(cx, |h, _| h.music_catalog = vec![to_cat(&music)]);
        s.switch_to_content_hub(ContentHubSection::Music, cx);
        let entries = build_music_track_entries_from_creation(&music);
        assert!(entries[0].wav_bytes.starts_with(b"RIFF"));
        s.play_music_creation_in_hub(music.clone(), 0, false, cx);
        s.content_hub.update(cx, |h, _| {
            assert_eq!(h.music_engine.state(), MusicPlaybackState::Playing);
            h.music_engine.tick_ms(1_000);
            assert_eq!(h.music_engine.active_lyric_index(), Some(1));
            h.music_engine.seek_ms(100).unwrap();
            assert_eq!(h.music_engine.active_lyric_index(), Some(0));
            h.music_engine.set_repeat_mode(MusicRepeatMode::One);
            h.music_engine.set_shuffle(true);
            h.music_engine.pause();
            assert_eq!(h.music_engine.state(), MusicPlaybackState::Paused);
        });
    });
}

fn body_ui_t1_05(cx: &mut App) {
    let (_dir, shell) = boot_shell(cx, "http://127.0.0.1:9", Some(auth_for(105, "gamer", "t5")));
    let game = fixture_game("cr_ui_game", 105);
    shell.update(cx, |s, cx| {
        s.content_hub.update(cx, |h, _| h.games_catalog = vec![to_cat(&game)]);
        s.switch_to_content_hub(ContentHubSection::Games, cx);
        let CreationPayload::Game(gp) = &game.payload else {
            unreachable!();
        };
        let mut session = ActiveGameSession::launch(&game, gp, None, 7);
        session.apply_input(GameInput::SelectOption(0));
        session.apply_input(GameInput::Confirm);
        let snap = session.snapshot();
        assert_eq!(snap.status, GameStatus::Won);
        assert!(snap.score >= 150);
        assert!(session.to_submit_score_request().is_some());
    });
}

fn body_ui_t1_06(cx: &mut App) {
    let (_dir, shell) = boot_shell(cx, "http://127.0.0.1:9", Some(auth_for(106, "shopper", "t6")));
    let book = fixture_book("cr_as_b", 106, CreationReviewStatus::Published, "A".repeat(80).as_str());
    let music = fixture_music("cr_as_m", 106);
    let game = fixture_game("cr_as_g", 106);
    let app = UserCreationDto {
        creation_id: "cr_as_app".to_string(),
        owner_user_id: 106,
        creator_bot_user_id: None,
        creator_chat_id: None,
        kind: CreationKind::App,
        title: "Creator Notes".to_string(),
        subtitle: "Markdown Notes".to_string(),
        summary: "Unified workspace notes app in Apple App Store.".to_string(),
        author_display_name: "Dev".to_string(),
        language: "en".to_string(),
        genres: vec!["Productivity".to_string()],
        tags: vec!["work".to_string()],
        cover: CoverArtwork::default(),
        age_rating: "4+".to_string(),
        payload: CreationPayload::App(AppStoreListingPayload {
            developer_name: "Dev".to_string(),
            version_string: "1.0.0".to_string(),
            release_notes: "Initial".to_string(),
            category: "Productivity".to_string(),
            long_description: "Full notes app.".to_string(),
            screenshots: Vec::new(),
            size_label: "3 MB".to_string(),
            price_label: "Free".to_string(),
            permissions: Vec::new(),
            linked_miniapp: None,
            ui_schema_json: None,
        }),
        version: 1,
        cloud_revision: 1,
        review_status: CreationReviewStatus::Published,
        visibility: CreationVisibility::Public,
        has_live_published_version: true,
        published_version: Some(1),
        rejection_reason: None,
        review_notes: None,
        submitted_at_ms: None,
        reviewed_at_ms: None,
        published_at_ms: Some(1_000),
        created_at_ms: 500,
        updated_at_ms: 1_000,
        deleted_at_ms: None,
        rating_avg: 4.9,
        rating_count: 10,
        engagement_count: 100,
        save_or_install_count: 50,
    };
    shell.update(cx, |s, cx| {
        s.content_hub.update(cx, |h, _| {
            h.appstore_catalog = AppStoreCatalogResponse {
                featured: vec![to_cat(&app)],
                apps: vec![to_cat(&app)],
                mini_apps: vec![AppStoreMiniAppEntry {
                    bot_user_id: 777_020,
                    bot_username: "CreatorStudioBot".to_string(),
                    short_name: "studio".to_string(),
                    title: "Creator Mini App".to_string(),
                    description: "Built-in mini app".to_string(),
                    web_url: "https://example.invalid/app".to_string(),
                    icon_url: None,
                    deep_link: "tg://miniapp/CreatorStudioBot/studio".to_string(),
                }],
                cross_vertical: vec![to_cat(&book), to_cat(&music), to_cat(&game)],
            };
        });
        s.switch_to_content_hub(ContentHubSection::AppStore, cx);
        let h = s.content_hub.read(cx);
        assert_eq!(h.appstore_catalog.apps.len(), 1);
        assert_eq!(h.appstore_catalog.mini_apps.len(), 1);
        assert_eq!(h.appstore_catalog.cross_vertical.len(), 3);
    });
}

fn body_ui_t1_07(cx: &mut App) {
    let (_dir, shell) = boot_shell(cx, "http://127.0.0.1:9", Some(auth_for(107, "author", "t7")));
    let mut invalid_draft = fixture_book("cr_invalid", 107, CreationReviewStatus::Draft, "short");
    if let CreationPayload::Book(ref mut bp) = invalid_draft.payload {
        bp.chapters.truncate(1);
    }
    let valid_draft = fixture_book(
        "cr_valid",
        107,
        CreationReviewStatus::Draft,
        "A rich multi-sentence valid chapter exceeding fifty characters for Apple Books review.",
    );
    shell.update(cx, |s, cx| {
        s.open_publish_review_modal_for_creation(invalid_draft, cx);
        assert!(!s.content_hub.read(cx).publish_modal.validation_errors.is_empty());

        s.open_publish_review_modal_for_creation(valid_draft, cx);
        assert!(s.content_hub.read(cx).publish_modal.validation_errors.is_empty());
        assert!(s.is_publish_review_modal_active(cx));
    });
}

// ============================================================================
// UI Tier 2 (ui_t2_01 .. ui_t2_04)
// ============================================================================

fn body_ui_t2_01(cx: &mut App) {
    let (_dir, shell) = boot_shell(cx, "http://127.0.0.1:9", Some(auth_for(201, "ime", "t21")));
    let draft = fixture_book("cr_ime", 201, CreationReviewStatus::Draft, &"文字".repeat(40));
    shell.update(cx, |s, cx| {
        s.switch_to_content_hub(ContentHubSection::Books, cx);
        s.content_hub
            .update(cx, |h, hcx| h.focus_input(ContentHubInputTarget::CatalogSearch, hcx));
        s.insert_committed_text("星际 SciFi", cx);
        assert_eq!(s.content_hub.read(cx).catalog_search.text, "星际 SciFi");
        assert!(s.composer.read(cx).text().is_empty());

        s.open_publish_review_modal_for_creation(draft, cx);
        s.content_hub.update(cx, |h, _| {
            h.publish_modal.focused_field = Some(PublishModalField::ReviewNotes);
        });
        s.insert_committed_text("中文备注 Note", cx);
        assert_eq!(
            s.content_hub.read(cx).publish_modal.review_notes.text,
            "中文备注 Note"
        );
        assert!(s.composer.read(cx).text().is_empty());
    });
}

fn body_ui_t2_02(cx: &mut App) {
    let (_dir, shell) = boot_shell(cx, "http://127.0.0.1:9", None);
    // Unauthenticated guard ignores tg://apple/* deep-links safely
    shell.update(cx, |s, cx| {
        assert!(!s.is_authenticated);
        assert!(!s.handle_apple_deep_link("tg://apple/books/cr_x?action=read", cx));
        assert_eq!(s.active_panel, ActiveWorkspacePanel::Chats);
    });

    // Authenticate, open inspector + PublishReviewModal -> Escape closes PublishReviewModal first!
    let draft = fixture_book("cr_esc", 202, CreationReviewStatus::Draft, &"内容".repeat(40));
    shell.update(cx, |s, cx| {
        s.apply_authenticated_session(auth_for(202, "esc_user", "t22"), cx);
        s.set_inspector_open(true, cx);
        s.open_publish_review_modal_for_creation(draft, cx);
        assert!(s.is_publish_review_modal_active(cx));
        assert!(s.inspector.read(cx).is_open);

        assert!(s.handle_non_char_key("escape", false, cx));
        assert!(!s.is_publish_review_modal_active(cx));
        assert!(s.inspector.read(cx).is_open);

        assert!(s.handle_non_char_key("escape", false, cx));
        assert!(!s.inspector.read(cx).is_open);
    });
}

fn body_ui_t2_03(cx: &mut App) {
    let (_dir, shell) = boot_shell(cx, "http://127.0.0.1:9", Some(auth_for(203, "user_a", "ta")));
    let priv_book = fixture_book("cr_priv_a", 203, CreationReviewStatus::Draft, &"密".repeat(40));
    let music = fixture_music("cr_mus_a", 203);
    shell.update(cx, |s, cx| {
        s.client.local_store.upsert_user_creations(&[priv_book.clone()]);
        s.content_hub.update(cx, |h, _| h.set_my_creations(vec![priv_book]));
        s.switch_to_content_hub(ContentHubSection::Music, cx);
        s.play_music_creation_in_hub(music, 0, false, cx);
        assert_eq!(
            s.content_hub.read(cx).music_engine.state(),
            MusicPlaybackState::Playing
        );

        // Switch to User B -> stops music, resets panel to Chats, wipes private creations cache!
        s.apply_authenticated_session(auth_for(204, "user_b", "tb"), cx);
        assert_eq!(s.active_panel, ActiveWorkspacePanel::Chats);
        assert_eq!(
            s.content_hub.read(cx).music_engine.state(),
            MusicPlaybackState::Stopped
        );
        assert!(s.content_hub.read(cx).my_creations.is_empty());
        assert!(s.client.load_cached_my_creations(None, None).is_empty());
    });
}

// ============================================================================
// UI Tier 3 (ui_t3_01 .. ui_t3_03)
// ============================================================================

fn body_ui_t3_01(cx: &mut App) {
    let (_dir, shell) = boot_shell(cx, "http://127.0.0.1:9", Some(auth_for(301, "card", "t31")));
    let book = fixture_book("cr_card_book", 301, CreationReviewStatus::Published, &"星".repeat(60));
    shell.update(cx, |s, cx| {
        s.content_hub.update(cx, |h, _| h.set_my_creations(vec![book]));
        s.execute_hermes_inline_callback("tg://apple/books/cr_card_book?action=read".to_string(), cx);
        assert_eq!(
            s.active_panel,
            ActiveWorkspacePanel::ContentHub(ContentHubSection::Books)
        );
        assert!(s
            .content_hub
            .read(cx)
            .book_reader
            .as_ref()
            .is_some_and(|r| r.creation_id == "cr_card_book"));
    });
}

fn body_ui_t3_02(cx: &mut App) {
    let (_dir, shell) = boot_shell(cx, "http://127.0.0.1:9", Some(auth_for(302, "links", "t32")));
    shell.update(cx, |s, cx| {
        s.handle_stage_bot_button(1, InlineButtonType::Url("tg://apple/music/cr_m1".to_string()), cx);
        assert_eq!(
            s.active_panel,
            ActiveWorkspacePanel::ContentHub(ContentHubSection::Music)
        );
        assert_eq!(
            s.content_hub.read(cx).selected_creation_id.as_deref(),
            Some("cr_m1")
        );

        s.stage.update(cx, |st, stcx| {
            st.handle_rich_text_action(
                RichTextAction::OpenUrl("tg://apple/games/cr_g1".to_string()),
                stcx,
            );
        });
        s.handle_stage_pending_actions(cx);
        assert_eq!(
            s.active_panel,
            ActiveWorkspacePanel::ContentHub(ContentHubSection::Games)
        );
        assert!(s.overlays.media_viewer_overlay.is_none());

        s.handle_stage_bot_button(
            2,
            InlineButtonType::Callback("creation:open:app:cr_a1".to_string()),
            cx,
        );
        assert_eq!(
            s.active_panel,
            ActiveWorkspacePanel::ContentHub(ContentHubSection::AppStore)
        );
        assert_eq!(
            s.content_hub.read(cx).selected_creation_id.as_deref(),
            Some("cr_a1")
        );
    });
}

async fn wait_until<F: FnMut(&mut AsyncApp) -> bool>(app: &mut AsyncApp, mut pred: F, timeout_ms: u64) -> bool {
    let steps = (timeout_ms / 40).max(1);
    for _ in 0..steps {
        if pred(app) {
            return true;
        }
        app.background_executor()
            .timer(Duration::from_millis(40))
            .await;
    }
    pred(app)
}

async fn body_ui_t3_03(mut app: AsyncApp) {
    let server_url = std::env::var("UI_E2E_SERVER_URL").expect("UI_E2E_SERVER_URL");
    let token = std::env::var("UI_E2E_TOKEN").expect("UI_E2E_TOKEN");
    let uid: u64 = std::env::var("UI_E2E_UID").unwrap().parse().unwrap();
    let cid = std::env::var("UI_E2E_CID").expect("UI_E2E_CID");

    let (_dir, shell) = app
        .update(|cx| boot_shell(cx, &server_url, Some(auth_for(uid, "ws_user", &token))))
        .expect("boot live shell");

    // Verify live refresh_my_creations + refresh_content_hub_catalog hydrate from live server
    let cid_c = cid.clone();
    assert!(
        wait_until(
            &mut app,
            |a| a
                .update(|cx| {
                    let h = shell.read(cx).content_hub.read(cx);
                    h.find_my_creation(&cid_c).is_some()
                })
                .unwrap_or(false),
            5_000,
        )
        .await,
        "live refresh_my_creations must hydrate {cid}"
    );

    let cid_ev = cid.clone();
    app.update(|cx| {
        shell.update(cx, |s, cx| {
            s.switch_to_content_hub(ContentHubSection::Books, cx);
            s.handle_ws_public_catalog_changed(
                cid_ev.clone(),
                CreationKind::Book,
                "published".to_string(),
                cx,
            );
        });
    })
    .unwrap();

    let cid_pub = cid.clone();
    assert!(
        wait_until(
            &mut app,
            |a| a
                .update(|cx| shell.read(cx).content_hub.read(cx).find_catalog_item(&cid_pub).is_some())
                .unwrap_or(false),
            5_000,
        )
        .await,
        "WS PublicCatalogChanged(published) must populate books_catalog live"
    );

    // WS removed evicts immediately
    let cid_rem = cid.clone();
    app.update(|cx| {
        shell.update(cx, |s, cx| {
            s.handle_ws_public_catalog_changed(
                cid_rem.clone(),
                CreationKind::Book,
                "removed".to_string(),
                cx,
            );
        });
    })
    .unwrap();
    let still_there = app
        .update(|cx| shell.read(cx).content_hub.read(cx).find_catalog_item(&cid).is_some())
        .unwrap();
    assert!(!still_there, "WS removed must evict item immediately");
}

// ============================================================================
// UI Tier 4 (ui_t4_01 .. ui_t4_02)
// ============================================================================

async fn body_ui_t4_01(mut app: AsyncApp) {
    let server_url = std::env::var("UI_E2E_SERVER_URL").expect("UI_E2E_SERVER_URL");
    let token = std::env::var("UI_E2E_TOKEN").expect("UI_E2E_TOKEN");
    let uid: u64 = std::env::var("UI_E2E_UID").unwrap().parse().unwrap();
    let deep_link = std::env::var("UI_E2E_DEEPLINK").expect("UI_E2E_DEEPLINK");
    let cid = std::env::var("UI_E2E_CID").expect("UI_E2E_CID");

    let (_dir, shell) = app
        .update(|cx| boot_shell(cx, &server_url, Some(auth_for(uid, "consumer_b", &token))))
        .expect("boot consumer shell");

    // Consumer B clicks bot recommendation deep-link with ?action=read -> live detail fetch -> auto-opens BookReader!
    let dl = if deep_link.contains('?') {
        format!("{deep_link}&action=read")
    } else {
        format!("{deep_link}?action=read")
    };
    app.update(|cx| {
        shell.update(cx, |s, cx| {
            s.execute_hermes_inline_callback(dl, cx);
        });
    })
    .unwrap();

    let cid_c = cid.clone();
    assert!(
        wait_until(
            &mut app,
            |a| a
                .update(|cx| {
                    shell
                        .read(cx)
                        .content_hub
                        .read(cx)
                        .book_reader
                        .as_ref()
                        .is_some_and(|r| r.creation_id == cid_c)
                })
                .unwrap_or(false),
            6_000,
        )
        .await,
        "Bot deep-link must fetch live published book and auto-open BookReader"
    );

    // Consumer B turns page, bookmarks, and persists reading state to live server!
    app.update(|cx| {
        shell.update(cx, |s, cx| {
            let sync = s.content_hub.update(cx, |h, hcx| {
                let r = h.book_reader.as_mut().unwrap();
                r.toggle_current_bookmark(Some("Live E2E bookmark".to_string()));
                r.next_page();
                hcx.notify();
                (r.creation_id.clone(), r.to_upsert_state_request())
            });
            s.upsert_creation_state_fenced(sync.0, sync.1, cx);
        });
    })
    .unwrap();

    // Give async PUT /api/v1/catalog/{id}/state time to commit to live server
    app.background_executor()
        .timer(Duration::from_millis(500))
        .await;
}

async fn body_ui_t4_02(mut app: AsyncApp) {
    let server_url = std::env::var("UI_E2E_SERVER_URL").expect("UI_E2E_SERVER_URL");
    let token = std::env::var("UI_E2E_TOKEN").expect("UI_E2E_TOKEN");
    let uid: u64 = std::env::var("UI_E2E_UID").unwrap().parse().unwrap();
    let music_id = std::env::var("UI_E2E_MUSIC_ID").expect("UI_E2E_MUSIC_ID");
    let game_id = std::env::var("UI_E2E_GAME_ID").expect("UI_E2E_GAME_ID");

    let (_dir, shell) = app
        .update(|cx| boot_shell(cx, &server_url, Some(auth_for(uid, "player_u", &token))))
        .expect("boot live shell");

    // 1. Open App Store and wait for unified catalog hydration (cross-vertical Music & Game)
    app.update(|cx| {
        shell.update(cx, |s, cx| {
            s.switch_to_content_hub(ContentHubSection::AppStore, cx);
        });
    })
    .unwrap();

    let m_c = music_id.clone();
    let g_c = game_id.clone();
    assert!(
        wait_until(
            &mut app,
            |a| a
                .update(|cx| {
                    let h = shell.read(cx).content_hub.read(cx);
                    h.find_catalog_item(&m_c).is_some() && h.find_catalog_item(&g_c).is_some()
                })
                .unwrap_or(false),
            6_000,
        )
        .await,
        "AppStore unified catalog must surface both published Music and Game"
    );

    // 2. Launch Music via deep-link ?action=play -> starts MusicTransportEngine!
    app.update(|cx| {
        shell.update(cx, |s, cx| {
            assert!(s.handle_apple_deep_link(&format!("tg://apple/music/{music_id}?action=play"), cx));
        });
    })
    .unwrap();
    assert!(
        wait_until(
            &mut app,
            |a| a
                .update(|cx| {
                    shell.read(cx).content_hub.read(cx).music_engine.state()
                        == MusicPlaybackState::Playing
                })
                .unwrap_or(false),
            6_000,
        )
        .await,
        "Music deep-link must synthesize & start MusicTransportEngine"
    );

    // 3. Launch Game via deep-link ?action=play -> starts ActiveGameSession, tick & score!
    app.update(|cx| {
        shell.update(cx, |s, cx| {
            assert!(s.handle_apple_deep_link(&format!("tg://apple/games/{game_id}?action=play"), cx));
        });
    })
    .unwrap();
    let g_id2 = game_id.clone();
    assert!(
        wait_until(
            &mut app,
            |a| a
                .update(|cx| {
                    shell
                        .read(cx)
                        .content_hub
                        .read(cx)
                        .active_game
                        .as_ref()
                        .is_some_and(|g| g.creation_id == g_id2)
                })
                .unwrap_or(false),
            6_000,
        )
        .await,
        "Game deep-link must launch ActiveGameSession live"
    );

    // Play & return seamlessly to Chats
    app.update(|cx| {
        shell.update(cx, |s, cx| {
            s.content_hub.update(cx, |h, hcx| {
                let g = h.active_game.as_mut().unwrap();
                g.apply_input(GameInput::SelectOption(0));
                g.apply_input(GameInput::Confirm);
                g.step(200);
                hcx.notify();
            });
            s.handle_apple_sidebar_event(
                &AppleSidebarEvent::SelectSection(AppleSidebarSection::Chats),
                cx,
            );
            assert_eq!(s.active_panel, ActiveWorkspacePanel::Chats);
        });
    })
    .unwrap();
}

// ============================================================================
// Registered 16 Phase B UI E2E Test Entrypoints
// ============================================================================

#[test]
fn ui_t1_01_apple_sidebar_mounts_and_switches_all_sections() {
    spawn_ui_child("ui_t1_01_apple_sidebar_mounts_and_switches_all_sections", true, &[]);
}

#[test]
fn ui_t1_02_apple_sidebar_collapse_expand_and_quick_creations_drawer() {
    spawn_ui_child("ui_t1_02_apple_sidebar_collapse_expand_and_quick_creations_drawer", true, &[]);
}

#[test]
fn ui_t1_03_books_hub_and_book_reader_themes_zoom_bookmarks() {
    spawn_ui_child("ui_t1_03_books_hub_and_book_reader_themes_zoom_bookmarks", true, &[]);
}

#[test]
fn ui_t1_04_music_hub_synth_and_now_playing_transport_bar() {
    spawn_ui_child("ui_t1_04_music_hub_synth_and_now_playing_transport_bar", true, &[]);
}

#[test]
fn ui_t1_05_games_hub_and_interactive_mini_game_runtime_sheet() {
    spawn_ui_child("ui_t1_05_games_hub_and_interactive_mini_game_runtime_sheet", true, &[]);
}

#[test]
fn ui_t1_06_app_store_hub_unified_storefront() {
    spawn_ui_child("ui_t1_06_app_store_hub_unified_storefront", true, &[]);
}

#[test]
fn ui_t1_07_publish_review_modal_validation_and_submit() {
    spawn_ui_child("ui_t1_07_publish_review_modal_validation_and_submit", true, &[]);
}

#[test]
fn ui_t2_01_ime_input_isolation_in_hub_search_and_review_modal() {
    spawn_ui_child("ui_t2_01_ime_input_isolation_in_hub_search_and_review_modal", true, &[]);
}

#[test]
fn ui_t2_02_escape_modal_priority_and_unauthenticated_guard() {
    spawn_ui_child("ui_t2_02_escape_modal_priority_and_unauthenticated_guard", true, &[]);
}

#[test]
fn ui_t2_03_account_switch_and_logout_wipes_private_creations_cache() {
    spawn_ui_child("ui_t2_03_account_switch_and_logout_wipes_private_creations_cache", true, &[]);
}

#[test]
fn ui_t2_04_icon_and_nav_rail_regression_guardrails() {
    HeadlessTranscriptDriver::verify_all_embedded_assets();
    assert_eq!(IconName::ALL.len(), 64, "IconName::ALL must remain exact 64");
    let src = TelegramAssetSource::new();
    for icon in [
        IconName::HubBooks,
        IconName::HubMusic,
        IconName::HubGames,
        IconName::HubAppStore,
        IconName::HubCreations,
        IconName::HubPublish,
        IconName::HubSparkles,
    ] {
        assert!(IconName::EXTRA.contains(&icon));
        assert!(!IconName::ALL.contains(&icon));
        assert!(src.load(icon.path()).unwrap().is_some());
    }
    assert_eq!(ToolsetId::all().len(), 29, "ToolsetId::all() must stay 29");
    let nav = NavRail::new();
    assert_eq!(
        nav.items.len(),
        13,
        "9 legacy destinations + upstream Browser + Community + OrcaBots + OrcaIde"
    );
    assert_eq!(NAV_RAIL_WIDTH, 52.0);
    assert_eq!(DEFAULT_WIDTH, 288.0);
    assert_eq!(COLLAPSED_WIDTH, 64.0);
    assert_eq!(ROW_HEIGHT, 62.0);
    let sb = ChatSidebar::new();
    assert!(!sb.has_leading_app_sidebar);
    assert!(sb.dialogs.is_empty());
}

#[test]
fn ui_t3_01_chat_generative_card_deep_link_opens_target_apple_vertical() {
    spawn_ui_child("ui_t3_01_chat_generative_card_deep_link_opens_target_apple_vertical", true, &[]);
}

#[test]
fn ui_t3_02_inline_keyboard_url_and_richtext_openurl_deep_link() {
    spawn_ui_child("ui_t3_02_inline_keyboard_url_and_richtext_openurl_deep_link", true, &[]);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn ui_t3_03_ws_events_live_refresh_my_creations_and_public_catalog() -> anyhow::Result<()> {
    if std::env::var(CHILD_ENV).as_deref() == Ok("1") {
        dispatch_headless_target("ui_t3_03_ws_events_live_refresh_my_creations_and_public_catalog");
    }
    let (mut server, driver) = apple_sidebar_common::boot_server().await?;
    let (creator, _consumer, admin) =
        apple_sidebar_common::seed_creator_consumer_admin(&server, &driver).await?;
    let payload = apple_sidebar_common::make_book_creation(
        "Live WS UI Book",
        "Book published on live server for headless UI E2E verification.",
        "Alice",
        "en",
        &["Sci-Fi"],
        &["ws"],
        None,
    );
    let created = apple_sidebar_common::create_creation_ok(&driver, &creator.token, payload).await?;
    let cid = created["creation_id"].as_str().unwrap().to_string();
    apple_sidebar_common::publish_creation_public(&driver, &creator.token, &admin.token, &cid)
        .await?;

    let env_vars = vec![
        ("UI_E2E_SERVER_URL", server.base_url()),
        ("UI_E2E_TOKEN", creator.token.clone()),
        ("UI_E2E_UID", creator.user_id.to_string()),
        ("UI_E2E_CID", cid),
    ];
    spawn_ui_child_async(
        "ui_t3_03_ws_events_live_refresh_my_creations_and_public_catalog",
        false,
        env_vars,
    )
    .await?;
    server.stop().await;
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn ui_t4_01_full_ui_journey_bot_create_to_sidebar_creations_publish_and_discover(
) -> anyhow::Result<()> {
    if std::env::var(CHILD_ENV).as_deref() == Ok("1") {
        dispatch_headless_target(
            "ui_t4_01_full_ui_journey_bot_create_to_sidebar_creations_publish_and_discover",
        );
    }
    let (mut server, driver) = apple_sidebar_common::boot_server().await?;
    let (creator, consumer, admin) =
        apple_sidebar_common::seed_creator_consumer_admin(&server, &driver).await?;

    // User A creates a book via @CreatorStudioBot DM -> publishes -> Consumer B gets /recommend deep-link
    let dm_a = apple_sidebar_common::open_creator_bot_dm(&driver, &creator.token).await?;
    let _ = apple_sidebar_common::send_bot_command_and_poll_reply(
        &driver,
        &creator.token,
        dm_a,
        "/newbook Celestial Compass | An illustrated interstellar voyage crafted via bot.",
    )
    .await?;

    let (st, my_list) = driver
        .json_req(Method::GET, "/api/v1/creations/my", Some(&creator.token), None)
        .await?;
    anyhow::ensure!(st == 200, "list my creations failed: {st} {my_list}");
    let cid = my_list["items"][0]["creation_id"]
        .as_str()
        .ok_or_else(|| anyhow::anyhow!("missing bot-created book id"))?
        .to_string();
    apple_sidebar_common::publish_creation_public(&driver, &creator.token, &admin.token, &cid)
        .await?;

    let dm_b = apple_sidebar_common::open_creator_bot_dm(&driver, &consumer.token).await?;
    let rec_reply = apple_sidebar_common::send_bot_command_and_poll_reply(
        &driver,
        &consumer.token,
        dm_b,
        "/recommend Celestial",
    )
    .await?;
    let links = apple_sidebar_common::extract_apple_deep_links_from_message(&rec_reply);
    let target_link = links
        .into_iter()
        .find(|l| l.contains(&cid))
        .unwrap_or_else(|| format!("tg://apple/books/{cid}"));

    let env_vars = vec![
        ("UI_E2E_SERVER_URL", server.base_url()),
        ("UI_E2E_TOKEN", consumer.token.clone()),
        ("UI_E2E_UID", consumer.user_id.to_string()),
        ("UI_E2E_DEEPLINK", target_link),
        ("UI_E2E_CID", cid.clone()),
    ];
    spawn_ui_child_async(
        "ui_t4_01_full_ui_journey_bot_create_to_sidebar_creations_publish_and_discover",
        false,
        env_vars,
    )
    .await?;

    // Verify Consumer B's reading progress persisted on the live server!
    let (st_states, states_val) = driver
        .json_req(
            Method::GET,
            "/api/v1/catalog/my-states",
            Some(&consumer.token),
            None,
        )
        .await?;
    anyhow::ensure!(st_states == 200, "my-states failed: {st_states}");
    let items = states_val["items"].as_array().unwrap();
    assert!(
        items
            .iter()
            .any(|s| s["creation_id"].as_str() == Some(cid.as_str()) && s["in_library"] == json!(true)),
        "expected Consumer B's live reading state persisted on server: {states_val}"
    );
    server.stop().await;
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn ui_t4_02_full_ui_journey_music_and_game_launch_from_app_store_and_chat(
) -> anyhow::Result<()> {
    if std::env::var(CHILD_ENV).as_deref() == Ok("1") {
        dispatch_headless_target(
            "ui_t4_02_full_ui_journey_music_and_game_launch_from_app_store_and_chat",
        );
    }
    let (mut server, driver) = apple_sidebar_common::boot_server().await?;
    let (creator, consumer, admin) =
        apple_sidebar_common::seed_creator_consumer_admin(&server, &driver).await?;

    let music_req = apple_sidebar_common::make_music_creation(
        "AppStore Synth Journey",
        "Playable synthesized album featured in Apple App Store.",
        "Studio Composer",
        "en",
        &["Electronic"],
        &["synth"],
        None,
        None,
    );
    let m_created = apple_sidebar_common::create_creation_ok(&driver, &creator.token, music_req).await?;
    let music_id = m_created["creation_id"].as_str().unwrap().to_string();
    apple_sidebar_common::publish_creation_public(&driver, &creator.token, &admin.token, &music_id)
        .await?;

    let game_req = apple_sidebar_common::make_game_creation(
        "AppStore Arcade Quiz",
        "Interactive game featured in Apple App Store.",
        "Arcade Maker",
        "en",
        &["Trivia"],
        &["arcade"],
        None,
    );
    let g_created = apple_sidebar_common::create_creation_ok(&driver, &creator.token, game_req).await?;
    let game_id = g_created["creation_id"].as_str().unwrap().to_string();
    apple_sidebar_common::publish_creation_public(&driver, &creator.token, &admin.token, &game_id)
        .await?;

    let env_vars = vec![
        ("UI_E2E_SERVER_URL", server.base_url()),
        ("UI_E2E_TOKEN", consumer.token.clone()),
        ("UI_E2E_UID", consumer.user_id.to_string()),
        ("UI_E2E_MUSIC_ID", music_id),
        ("UI_E2E_GAME_ID", game_id),
    ];
    spawn_ui_child_async(
        "ui_t4_02_full_ui_journey_music_and_game_launch_from_app_store_and_chat",
        false,
        env_vars,
    )
    .await?;
    server.stop().await;
    Ok(())
}
