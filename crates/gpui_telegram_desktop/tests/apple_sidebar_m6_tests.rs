//! Milestone M6 (Features F14–F17) integration & headless GPUI tests:
//! - F14 Apple Books (`BooksHubView` & `BookReaderState`: 4 themes, 3 fonts, zoom, TOC, bookmarks, cloud state)
//! - F15 Apple Music (`MusicHubView`, `MusicPlayerBar`, pure-Rust WAV synth bridge, transport, lyrics, radio)
//! - F16 Apple Games / Arcade (`GamesHubView` & `ActiveGameSession` across all 7 `GameSpec` variants + leaderboard)
//! - F17 Apple App Store (`AppStoreHubView` unified storefront of Apps + Mini Apps + Cross-Verticals)
//! - Deep-link auto-open (`?action=read|play|install`), IME review inputs, Escape cascade, and eviction/logout cleanup.

use std::sync::Arc;

use gpui::{App, AppContext, Application, Entity};
use gpui_telegram_client::{AuthResponse, ClientConfig, TelegramClient};
use gpui_telegram_desktop::theme::{TdesktopThemeMode, TdesktopThemePalette, TelegramAssetSource};
use gpui_telegram_desktop::views::content_hub::app_store_view::{
    normalize_appstore_sub_tab, APPSTORE_SUB_TABS,
};
use gpui_telegram_desktop::views::content_hub::books_view::{
    normalize_books_sub_tab, BOOKS_SUB_TABS,
};
use gpui_telegram_desktop::views::content_hub::games_view::{
    normalize_games_sub_tab, GAMES_SUB_TABS,
};
use gpui_telegram_desktop::views::content_hub::music_view::{
    normalize_music_sub_tab, MUSIC_SUB_TABS,
};
use gpui_telegram_desktop::views::content_hub::{
    build_music_track_entries_from_creation, ActiveGameSession, BookReaderFontFamily,
    BookReaderState, BookReaderTheme, ContentHubEvent, ContentHubInputTarget, ContentHubSection,
    GameInput, GameStatus, DEFAULT_READER_FONT_SIZE, MAX_READER_FONT_SIZE, MIN_READER_FONT_SIZE,
};
use gpui_telegram_desktop::views::shell::{ActiveWorkspacePanel, AppShell};
use gpui_telegram_media::media::audio::{MusicPlaybackState, MusicRepeatMode};
use gpui_telegram_proto::creations::{
    AppStoreCatalogResponse, AppStoreListingPayload, AppStoreMiniAppEntry, BookChapter, BookPage,
    BookPayload, BookRichBlock, BookUserState, CardPair, CoverArtwork, CreationKind,
    CreationPayload, CreationReviewStatus, CreationVisibility, GameAchievementDef, GameCategory,
    GameEngineKind, GamePayload, GameSpec, GridPuzzleLevel, LinkedMiniAppRef, MusicPayload,
    MusicTrack, NoteEvent, PublicCatalogItemDto, ScreenshotAsset, StoryChoice, StoryNode,
    SyncedLyricLine, TriviaQuestion, UserCreationDto, UserCreationStateDto,
    CREATOR_STUDIO_BOT_USER_ID,
};
use tempfile::TempDir;

const CHILD_ENV: &str = "GPUI_M6_APPLE_VERTICALS_CHILD";
const CHILD_TARGET_ENV: &str = "GPUI_M6_APPLE_VERTICALS_TARGET";

fn dispatch_headless_target(target: &str) -> ! {
    let body: fn(&mut App) = match target {
        "test_m6_gpui_deep_link_auto_open_reader_music_game_appstore" => {
            body_deep_link_auto_open_reader_music_game_appstore
        }
        "test_m6_gpui_keyboard_escape_review_ime_and_eviction" => {
            body_keyboard_escape_review_ime_and_eviction
        }
        "test_m6_gpui_radio_and_all_four_verticals_state_flow" => {
            body_radio_and_all_four_verticals_state_flow
        }
        other => {
            eprintln!("unknown M6 headless target: {other}");
            std::process::exit(101);
        }
    };
    let target_owned = target.to_string();
    let outer = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        Application::headless()
            .with_assets(TelegramAssetSource::new())
            .run(move |cx: &mut App| {
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
                        eprintln!("[m6 headless panic] {target_owned}: {msg}");
                        std::process::exit(101);
                    }
                }
            });
    }));
    if let Err(payload) = outer {
        let msg = payload
            .downcast_ref::<&str>()
            .copied()
            .or_else(|| payload.downcast_ref::<String>().map(|s| s.as_str()))
            .unwrap_or("panic");
        eprintln!("[m6 outer panic] {target}: {msg}");
        std::process::exit(101);
    }
    std::process::exit(0);
}

#[cfg(target_os = "macos")]
#[used]
#[unsafe(link_section = "__DATA,__mod_init_func")]
static MACOS_M6_HEADLESS_ENTRY: extern "C" fn() = macos_mod_init_m6_entry;

#[cfg(target_os = "macos")]
extern "C" fn macos_mod_init_m6_entry() {
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
    let tmp_dir = TempDir::new().expect("create TempDir for M6 headless test");
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
        .expect("spawn M6 headless child");
    assert!(
        output.status.success(),
        "M6 headless test failed for {test_fn_name}:\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

fn make_sample_book(cid: &str, owner: i64) -> UserCreationDto {
    UserCreationDto {
        creation_id: cid.to_string(),
        owner_user_id: owner,
        creator_bot_user_id: Some(CREATOR_STUDIO_BOT_USER_ID),
        creator_chat_id: Some(101),
        kind: CreationKind::Book,
        title: "星际漫游手册".to_string(),
        subtitle: "Apple Books 精选".to_string(),
        summary: "一部跨越光年的原创科幻交互图书，包含完整章节与插画。".to_string(),
        author_display_name: "林星河".to_string(),
        language: "zh-CN".to_string(),
        genres: vec!["Sci-Fi".to_string(), "科幻".to_string()],
        tags: vec!["space".to_string()],
        cover: CoverArtwork {
            gradient_start_hex: "#FF9500".to_string(),
            gradient_end_hex: "#FF5E3A".to_string(),
            accent_hex: "#FFFFFF".to_string(),
            icon_emoji: "📖".to_string(),
            media_file_id: None,
            thumbnail_data_url: None,
        },
        age_rating: "4+".to_string(),
        payload: CreationPayload::Book(BookPayload {
            author: "林星河".to_string(),
            series_name: Some("星海三部曲".to_string()),
            series_index: Some(1),
            blurb: "星海启航的序章。".to_string(),
            estimated_read_mins: 18,
            word_count: 4200,
            chapters: vec![
                BookChapter {
                    chapter_id: "ch1".to_string(),
                    chapter_number: 1,
                    title: "启航".to_string(),
                    subtitle: Some("跃迁引擎点亮".to_string()),
                    pages: vec![
                        BookPage {
                            page_number: 1,
                            blocks: vec![
                                BookRichBlock::Heading {
                                    level: 1,
                                    text: "第一章 启航".to_string(),
                                },
                                BookRichBlock::Paragraph {
                                    text: "深空的星光洒在舷窗上，飞船缓缓驶离轨道船坞。".to_string(),
                                },
                                BookRichBlock::BlockQuote {
                                    text: "向着未知的星海进发。".to_string(),
                                    attribution: Some("舰长日志".to_string()),
                                },
                            ],
                        },
                        BookPage {
                            page_number: 2,
                            blocks: vec![
                                BookRichBlock::Callout {
                                    title: "导航提示".to_string(),
                                    text: "所有跃迁坐标已同步至云端。".to_string(),
                                },
                                BookRichBlock::Divider,
                                BookRichBlock::Illustration {
                                    caption: "曲率引擎蓝图".to_string(),
                                    media_file_id: None,
                                    palette_hex: Some("#007AFF".to_string()),
                                },
                            ],
                        },
                    ],
                },
                BookChapter {
                    chapter_id: "ch2".to_string(),
                    chapter_number: 2,
                    title: "星云彼岸".to_string(),
                    subtitle: None,
                    pages: vec![BookPage {
                        page_number: 1,
                        blocks: vec![BookRichBlock::Paragraph {
                            text: "穿过紫色星云之后，新的宜居行星浮现在眼前。".to_string(),
                        }],
                    }],
                },
            ],
        }),
        version: 1,
        cloud_revision: 3,
        review_status: CreationReviewStatus::Published,
        visibility: CreationVisibility::Public,
        has_live_published_version: true,
        published_version: Some(1),
        rejection_reason: None,
        review_notes: None,
        submitted_at_ms: Some(1_000),
        reviewed_at_ms: Some(2_000),
        published_at_ms: Some(2_000),
        created_at_ms: 500,
        updated_at_ms: 2_000,
        deleted_at_ms: None,
        rating_avg: 4.9,
        rating_count: 14,
        engagement_count: 120,
        save_or_install_count: 42,
    }
}

fn make_sample_music(cid: &str, owner: i64) -> UserCreationDto {
    UserCreationDto {
        creation_id: cid.to_string(),
        owner_user_id: owner,
        creator_bot_user_id: Some(CREATOR_STUDIO_BOT_USER_ID),
        creator_chat_id: Some(101),
        kind: CreationKind::Music,
        title: "午夜合成器".to_string(),
        subtitle: "Apple Music 空间合成".to_string(),
        summary: "使用纯 Rust 合成器创作的 Synthwave 原创双曲专辑。".to_string(),
        author_display_name: "Neon Synth".to_string(),
        language: "zh-CN".to_string(),
        genres: vec!["Electronic".to_string(), "Synthwave".to_string()],
        tags: vec!["chill".to_string(), "synth".to_string()],
        cover: CoverArtwork {
            gradient_start_hex: "#FA2D48".to_string(),
            gradient_end_hex: "#5E17EB".to_string(),
            accent_hex: "#FF6B81".to_string(),
            icon_emoji: "🎵".to_string(),
            media_file_id: None,
            thumbnail_data_url: None,
        },
        age_rating: "4+".to_string(),
        payload: CreationPayload::Music(MusicPayload {
            artist: "Neon Synth".to_string(),
            album_title: "Midnight Frequencies".to_string(),
            release_year: 2026,
            primary_genre: "Electronic".to_string(),
            mood_tags: vec!["Chill".to_string()],
            bpm: 120,
            musical_key: "A Minor".to_string(),
            time_signature: "4/4".to_string(),
            total_duration_secs: 8,
            tracks: vec![
                MusicTrack {
                    track_id: "tr_1".to_string(),
                    track_number: 1,
                    title: "霓虹初晓".to_string(),
                    duration_secs: 4,
                    bpm: 120,
                    musical_key: "A Minor".to_string(),
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
                            midi_note: 64,
                            start_beat: 1.0,
                            duration_beats: 1.0,
                            velocity: 96,
                            channel: 0,
                        },
                        NoteEvent {
                            midi_note: 67,
                            start_beat: 2.0,
                            duration_beats: 2.0,
                            velocity: 96,
                            channel: 0,
                        },
                    ],
                    chord_progression: Vec::new(),
                    lyrics: vec![
                        SyncedLyricLine {
                            timestamp_ms: 0,
                            text: "城市霓虹在午夜点亮".to_string(),
                        },
                        SyncedLyricLine {
                            timestamp_ms: 1000,
                            text: "合成器的旋律在云端回响".to_string(),
                        },
                    ],
                    audio_media_file_id: None,
                    waveform_5bit: Vec::new(),
                },
                MusicTrack {
                    track_id: "tr_2".to_string(),
                    track_number: 2,
                    title: "星轨漫步".to_string(),
                    duration_secs: 4,
                    bpm: 120,
                    musical_key: "C Major".to_string(),
                    instruments: Vec::new(),
                    melody_notes: vec![NoteEvent {
                        midi_note: 72,
                        start_beat: 0.0,
                        duration_beats: 2.0,
                        velocity: 90,
                        channel: 0,
                    }],
                    chord_progression: Vec::new(),
                    lyrics: Vec::new(),
                    audio_media_file_id: None,
                    waveform_5bit: Vec::new(),
                },
            ],
        }),
        version: 1,
        cloud_revision: 4,
        review_status: CreationReviewStatus::Published,
        visibility: CreationVisibility::Public,
        has_live_published_version: true,
        published_version: Some(1),
        rejection_reason: None,
        review_notes: None,
        submitted_at_ms: Some(1_000),
        reviewed_at_ms: Some(2_000),
        published_at_ms: Some(2_000),
        created_at_ms: 500,
        updated_at_ms: 2_000,
        deleted_at_ms: None,
        rating_avg: 4.8,
        rating_count: 9,
        engagement_count: 88,
        save_or_install_count: 31,
    }
}

fn make_sample_game(cid: &str, owner: i64, spec: GameSpec) -> UserCreationDto {
    let engine_kind = spec.engine_kind();
    UserCreationDto {
        creation_id: cid.to_string(),
        owner_user_id: owner,
        creator_bot_user_id: Some(CREATOR_STUDIO_BOT_USER_ID),
        creator_chat_id: Some(101),
        kind: CreationKind::Game,
        title: format!("Arcade {:?}", engine_kind),
        subtitle: "Apple Arcade 原生游戏".to_string(),
        summary: "即点即玩的纯 Rust 确定性小游戏，支持成就与排行榜。".to_string(),
        author_display_name: "Studio Dev".to_string(),
        language: "zh-CN".to_string(),
        genres: vec!["Arcade".to_string()],
        tags: vec!["playable".to_string()],
        cover: CoverArtwork {
            gradient_start_hex: "#5856D6".to_string(),
            gradient_end_hex: "#007AFF".to_string(),
            accent_hex: "#FFFFFF".to_string(),
            icon_emoji: "🎮".to_string(),
            media_file_id: None,
            thumbnail_data_url: None,
        },
        age_rating: "4+".to_string(),
        payload: CreationPayload::Game(GamePayload {
            developer: "Studio Dev".to_string(),
            category: GameCategory::Arcade,
            controls_hint: "WASD / 方向键操作".to_string(),
            rules_markdown: "躲避障碍并收集能量得分。".to_string(),
            engine_kind,
            supports_leaderboard: true,
            estimated_session_mins: 3,
            spec,
            achievements: vec![GameAchievementDef {
                achievement_id: "first_score".to_string(),
                title: "初露锋芒".to_string(),
                description: "获得首次得分".to_string(),
                icon_emoji: "🏆".to_string(),
                target_score: Some(10),
            }],
        }),
        version: 1,
        cloud_revision: 5,
        review_status: CreationReviewStatus::Published,
        visibility: CreationVisibility::Public,
        has_live_published_version: true,
        published_version: Some(1),
        rejection_reason: None,
        review_notes: None,
        submitted_at_ms: Some(1_000),
        reviewed_at_ms: Some(2_000),
        published_at_ms: Some(2_000),
        created_at_ms: 500,
        updated_at_ms: 2_000,
        deleted_at_ms: None,
        rating_avg: 4.7,
        rating_count: 11,
        engagement_count: 205,
        save_or_install_count: 55,
    }
}

fn make_sample_app(cid: &str, owner: i64) -> UserCreationDto {
    UserCreationDto {
        creation_id: cid.to_string(),
        owner_user_id: owner,
        creator_bot_user_id: Some(CREATOR_STUDIO_BOT_USER_ID),
        creator_chat_id: None,
        kind: CreationKind::App,
        title: "Focus Timer Pro".to_string(),
        subtitle: "专注番茄钟与任务清单".to_string(),
        summary: "面向创作者的极简效率工具，支持 Mini App 联动启动。".to_string(),
        author_display_name: "Apple Indie Lab".to_string(),
        language: "zh-CN".to_string(),
        genres: vec!["Productivity".to_string()],
        tags: vec!["work".to_string(), "studio".to_string()],
        cover: CoverArtwork {
            gradient_start_hex: "#007AFF".to_string(),
            gradient_end_hex: "#34C759".to_string(),
            accent_hex: "#FFFFFF".to_string(),
            icon_emoji: "⏱".to_string(),
            media_file_id: None,
            thumbnail_data_url: None,
        },
        age_rating: "4+".to_string(),
        payload: CreationPayload::App(AppStoreListingPayload {
            developer_name: "Apple Indie Lab".to_string(),
            version_string: "2.1.0".to_string(),
            release_notes: "新增深色模式与云端同步。".to_string(),
            category: "Productivity".to_string(),
            long_description: "帮助创作者高效规划写作、作曲和游戏开发时间。".to_string(),
            screenshots: vec![ScreenshotAsset {
                title: "专注计时器".to_string(),
                caption: Some("一键进入心流状态".to_string()),
                media_file_id: None,
                accent_hex: Some("#007AFF".to_string()),
            }],
            size_label: "4.2 MB".to_string(),
            price_label: "Free".to_string(),
            permissions: vec!["cloud_state".to_string()],
            linked_miniapp: Some(LinkedMiniAppRef {
                bot_username: "FocusBot".to_string(),
                short_name: "timer".to_string(),
                web_url: Some("https://example.invalid/focus".to_string()),
                launch_param: None,
            }),
            ui_schema_json: None,
        }),
        version: 2,
        cloud_revision: 6,
        review_status: CreationReviewStatus::Published,
        visibility: CreationVisibility::Public,
        has_live_published_version: true,
        published_version: Some(2),
        rejection_reason: None,
        review_notes: None,
        submitted_at_ms: Some(1_000),
        reviewed_at_ms: Some(2_000),
        published_at_ms: Some(2_000),
        created_at_ms: 500,
        updated_at_ms: 2_000,
        deleted_at_ms: None,
        rating_avg: 4.9,
        rating_count: 25,
        engagement_count: 340,
        save_or_install_count: 98,
    }
}

fn dto_to_catalog_item(dto: &UserCreationDto) -> PublicCatalogItemDto {
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
        published_version: dto.published_version.unwrap_or(1),
        published_at_ms: dto.published_at_ms.unwrap_or(2_000),
        updated_at_ms: dto.updated_at_ms,
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
// Pure Unit Tests (Fast, Deterministic, Zero GPUI App Required)
// ============================================================================

#[test]
fn test_m6_book_reader_themes_fonts_zoom_toc_bookmarks_and_cloud_progress() {
    let book = make_sample_book("cr_book_m6_1", 10);
    let CreationPayload::Book(bp) = book.payload.clone() else {
        panic!("expected Book payload");
    };
    let pal = TdesktopThemePalette::build(TdesktopThemeMode::Day, 0x007AFF, 14, 12);
    let mut reader = BookReaderState::open(&book, bp, None, pal);

    assert_eq!(reader.theme, BookReaderTheme::Light);
    assert_eq!(reader.font_family, BookReaderFontFamily::Serif);
    assert_eq!(reader.font_size_px, DEFAULT_READER_FONT_SIZE);
    assert_eq!(reader.total_page_slots(), 3); // 2 pages ch1 + 1 page ch2
    assert!((reader.progress_percent() - 33.3333).abs() < 1.0);

    // Cycle all 4 Apple Books reading themes
    assert_eq!(reader.theme.cycle(), BookReaderTheme::Sepia);
    reader.set_theme(BookReaderTheme::Sepia);
    assert_eq!(reader.theme.cycle(), BookReaderTheme::Gray);
    reader.set_theme(BookReaderTheme::Gray);
    assert_eq!(reader.theme.cycle(), BookReaderTheme::Night);
    reader.set_theme(BookReaderTheme::Night);
    assert_eq!(reader.theme.cycle(), BookReaderTheme::Light);

    // Font families & clamped zoom A-/A+
    reader.set_font_family(BookReaderFontFamily::Mono);
    assert_eq!(reader.font_family.font_name(), "Menlo");
    for _ in 0..20 {
        reader.increase_font_size();
    }
    assert_eq!(reader.font_size_px, MAX_READER_FONT_SIZE);
    for _ in 0..20 {
        reader.decrease_font_size();
    }
    assert_eq!(reader.font_size_px, MIN_READER_FONT_SIZE);

    // Multi-page & multi-chapter navigation + bookmarks
    assert!(reader.next_page()); // ch0 p1
    assert_eq!((reader.current_chapter_index, reader.current_page_index), (0, 1));
    assert!(reader.toggle_current_bookmark(Some("关键节点".to_string())));
    assert!(reader.is_current_page_bookmarked());
    assert_eq!(reader.bookmarks.len(), 1);

    assert!(reader.next_page()); // ch1 p0 (last slot => finished)
    assert_eq!((reader.current_chapter_index, reader.current_page_index), (1, 0));
    assert!(reader.finished);
    assert_eq!(reader.progress_percent(), 100.0);

    // Cloud state round-trip hydration
    let req = reader.to_upsert_state_request();
    assert_eq!(req.in_library, Some(true));
    assert_eq!(req.progress_percent, Some(100.0));
    let saved_dto = UserCreationStateDto {
        creation_id: book.creation_id.clone(),
        user_id: 10,
        kind: CreationKind::Book,
        in_library: true,
        favorite: true,
        progress_percent: 100.0,
        state_json: req.state_json.clone().unwrap(),
        updated_at_ms: 9_999,
    };
    let parsed: BookUserState = serde_json::from_value(saved_dto.state_json.clone()).unwrap();
    assert_eq!(parsed.current_chapter_index, 1);
    assert_eq!(parsed.bookmarks.len(), 1);

    // Reopen restores exact location & bookmarks
    let CreationPayload::Book(bp2) = book.payload.clone() else {
        unreachable!();
    };
    let reopened = BookReaderState::open(
        &book,
        bp2,
        Some(&saved_dto),
        TdesktopThemePalette::build(TdesktopThemeMode::Night, 0x007AFF, 14, 12),
    );
    assert_eq!(reopened.theme, BookReaderTheme::Night);
    assert_eq!(reopened.current_chapter_index, 1);
    assert_eq!(reopened.bookmarks.len(), 1);
    assert!(reopened.favorite);
}

#[test]
fn test_m6_music_synth_bridge_transport_seek_repeat_shuffle_and_lyrics() {
    let music = make_sample_music("cr_music_m6_1", 10);
    let entries = build_music_track_entries_from_creation(&music);
    assert_eq!(entries.len(), 2);
    assert_eq!(entries[0].creation_id.as_deref(), Some("cr_music_m6_1"));
    assert!(
        entries[0].wav_bytes.starts_with(b"RIFF"),
        "synthesized track must emit valid RIFF/WAVE bytes"
    );
    assert!(
        !entries[0].waveform_bars.is_empty(),
        "synthesized track must populate waveform bars"
    );
    assert_eq!(entries[0].lyrics.len(), 2);

    let mut engine = gpui_telegram_media::media::audio::MusicTransportEngine::new();
    engine.set_queue(entries);
    engine.play_index(0).expect("play track 0");
    assert_eq!(engine.state(), MusicPlaybackState::Playing);
    assert_eq!(engine.active_lyric_index(), Some(0));

    // Advance clock across synced lyric boundary
    engine.tick_ms(1_100);
    assert_eq!(engine.active_lyric_index(), Some(1));

    // Seek, pause, repeat, shuffle, next/prev
    engine.seek_ms(200).expect("seek to 200ms");
    assert_eq!(engine.active_lyric_index(), Some(0));
    engine.pause();
    assert_eq!(engine.state(), MusicPlaybackState::Paused);
    engine.set_repeat_mode(MusicRepeatMode::All);
    engine.set_shuffle(true);
    assert!(engine.next());
    assert!(engine.prev());
}

#[test]
fn test_m6_game_runtime_sheet_launches_all_7_specs_and_scores() {
    let specs = vec![
        GameSpec::GridPuzzle {
            rows: 2,
            cols: 2,
            levels: vec![GridPuzzleLevel {
                level_id: "lv1".to_string(),
                title: "2x2 Slide".to_string(),
                initial_grid: vec![vec![1, 2], vec![0, 3]],
                target_grid: Some(vec![vec![1, 2], vec![3, 0]]),
                max_moves: Some(5),
                hint: Some("Slide 3 left".to_string()),
            }],
        },
        GameSpec::TriviaQuiz {
            time_limit_secs: Some(30),
            questions: vec![TriviaQuestion {
                question_id: "q1".to_string(),
                prompt: "Rust memory safety?".to_string(),
                choices: vec!["Ownership".to_string(), "GC".to_string()],
                correct_index: 0,
                explanation: Some("Borrow checker".to_string()),
                points: 100,
            }],
        },
        GameSpec::InteractiveFiction {
            start_node_id: "start".to_string(),
            nodes: vec![
                StoryNode {
                    node_id: "start".to_string(),
                    title: "Orbit".to_string(),
                    body: "Choose path".to_string(),
                    choices: vec![StoryChoice {
                        label: "Land".to_string(),
                        target_node_id: "end".to_string(),
                        score_delta: 50,
                    }],
                    is_ending: false,
                    ending_badge: None,
                },
                StoryNode {
                    node_id: "end".to_string(),
                    title: "Safe".to_string(),
                    body: "Victory".to_string(),
                    choices: Vec::new(),
                    is_ending: true,
                    ending_badge: Some("Explorer".to_string()),
                },
            ],
        },
        GameSpec::ArcadeDodge {
            tick_ms: 50,
            arena_width: 400,
            arena_height: 300,
            player_speed: 200.0,
            obstacle_speed: 100.0,
            lives: 3,
            target_score: 200,
            theme_accent_hex: Some("#5856D6".to_string()),
        },
        GameSpec::SnakeGrid {
            grid_width: 12,
            grid_height: 12,
            tick_ms: 120,
            wrap_walls: true,
            target_score: 100,
        },
        GameSpec::CardMatch {
            pairs: vec![
                CardPair {
                    pair_id: "p1".to_string(),
                    label: "Star".to_string(),
                    symbol: "★".to_string(),
                    accent_hex: None,
                },
                CardPair {
                    pair_id: "p2".to_string(),
                    label: "Moon".to_string(),
                    symbol: "☾".to_string(),
                    accent_hex: None,
                },
            ],
            time_limit_secs: Some(60),
        },
        GameSpec::MiniAppHtml {
            bot_username: "GameBot".to_string(),
            short_name: "tetris".to_string(),
            html_bundle_media_id: None,
        },
    ];

    for (idx, spec) in specs.into_iter().enumerate() {
        let dto = make_sample_game(&format!("cr_game_spec_{idx}"), 10, spec);
        let CreationPayload::Game(gp) = &dto.payload else {
            unreachable!();
        };
        let mut session = ActiveGameSession::launch(&dto, gp, None, 42);
        let snap = session.snapshot();
        assert_eq!(snap.engine_kind, gp.engine_kind);

        match gp.engine_kind {
            GameEngineKind::TriviaQuiz => {
                session.apply_input(GameInput::SelectOption(0));
                session.apply_input(GameInput::Confirm);
                assert_eq!(session.snapshot().status, GameStatus::Won);
                assert!(session.to_submit_score_request().is_some());
            }
            GameEngineKind::InteractiveFiction => {
                session.apply_input(GameInput::SelectOption(0));
                assert_eq!(session.snapshot().status, GameStatus::Won);
                assert_eq!(session.snapshot().score, 50);
            }
            GameEngineKind::ArcadeDodge | GameEngineKind::SnakeGrid => {
                assert!(session.needs_realtime_ticks());
                session.apply_input(GameInput::MoveRight);
                let after = session.step(session.recommended_tick_ms() * 3);
                assert!(after.elapsed_ms > 0);
            }
            GameEngineKind::GridPuzzle => {
                session.apply_input(GameInput::ClickCell { row: 1, col: 1 });
                assert_eq!(session.snapshot().status, GameStatus::Won);
            }
            _ => {}
        }
        let state_req = session.to_upsert_state_request();
        assert_eq!(state_req.in_library, Some(true));
    }
}

#[test]
fn test_m6_sub_tab_normalization_and_canonical_counts() {
    assert_eq!(BOOKS_SUB_TABS.len(), 3);
    assert_eq!(MUSIC_SUB_TABS.len(), 4);
    assert_eq!(GAMES_SUB_TABS.len(), 4);
    assert_eq!(APPSTORE_SUB_TABS.len(), 7);
    assert_eq!(normalize_books_sub_tab("store"), "book_store");
    assert_eq!(normalize_books_sub_tab("my_books"), "reading_now");
    assert_eq!(normalize_music_sub_tab("listen_now"), "home");
    assert_eq!(normalize_music_sub_tab("my_music"), "home");
    assert_eq!(normalize_games_sub_tab("top"), "charts");
    assert_eq!(normalize_games_sub_tab("my_games"), "discover");
    assert_eq!(normalize_appstore_sub_tab("discover"), "today");
}

// ============================================================================
// Headless GPUI AppShell M6 Integration Tests (Subprocess Trampoline)
// ============================================================================

fn boot_authed_m6_shell(cx: &mut App) -> (TempDir, Entity<AppShell>) {
    let dir = TempDir::new().expect("temp dir");
    let cfg = ClientConfig {
        server_url: "http://127.0.0.1:9".to_string(),
        ..Default::default()
    };
    let client =
        Arc::new(TelegramClient::open_with_dir(cfg, dir.path().to_path_buf()).expect("client"));
    let shell = cx.new(|cx| AppShell::new_with_client(client, cx));
    shell.update(cx, |s, cx| {
        s.apply_authenticated_session(
            AuthResponse {
                token: "tok_m6_test".to_string(),
                session_id: "sess_m6_test".to_string(),
                user_id: 77,
                username: "m6_tester".to_string(),
                first_name: Some("M6".to_string()),
                created_at: None,
                expires_at: None,
            },
            cx,
        );
    });
    (dir, shell)
}

fn body_deep_link_auto_open_reader_music_game_appstore(cx: &mut App) {
    let (_dir, shell) = boot_authed_m6_shell(cx);
    let book = make_sample_book("cr_m6_book", 77);
    let music = make_sample_music("cr_m6_music", 77);
    let game = make_sample_game(
        "cr_m6_game",
        77,
        GameSpec::TriviaQuiz {
            time_limit_secs: None,
            questions: vec![TriviaQuestion {
                question_id: "q1".to_string(),
                prompt: "2+2?".to_string(),
                choices: vec!["4".to_string(), "5".to_string()],
                correct_index: 0,
                explanation: None,
                points: 100,
            }],
        },
    );
    let app = make_sample_app("cr_m6_app", 77);

    shell.update(cx, |s, cx| {
        s.content_hub.update(cx, |h, hcx| {
            h.set_my_creations(vec![book.clone(), music.clone(), game.clone(), app.clone()]);
            h.books_catalog = vec![dto_to_catalog_item(&book)];
            h.music_catalog = vec![dto_to_catalog_item(&music)];
            h.games_catalog = vec![dto_to_catalog_item(&game)];
            h.appstore_catalog = AppStoreCatalogResponse {
                featured: vec![dto_to_catalog_item(&app)],
                apps: vec![dto_to_catalog_item(&app)],
                mini_apps: vec![AppStoreMiniAppEntry {
                    bot_user_id: 777_020,
                    bot_username: "CreatorStudioBot".to_string(),
                    short_name: "studio".to_string(),
                    title: "Creator Mini App".to_string(),
                    description: "Create right in app".to_string(),
                    web_url: "https://example.invalid/studio".to_string(),
                    icon_url: None,
                    deep_link: "tg://miniapp/CreatorStudioBot/studio".to_string(),
                }],
                cross_vertical: vec![
                    dto_to_catalog_item(&book),
                    dto_to_catalog_item(&music),
                    dto_to_catalog_item(&game),
                ],
            };
            hcx.notify();
        });
    });

    // 1. tg://apple/books/cr_m6_book?action=read -> auto-opens BookReaderState!
    shell.update(cx, |s, cx| {
        assert!(s.handle_apple_deep_link("tg://apple/books/cr_m6_book?action=read", cx));
        assert_eq!(
            s.active_panel,
            ActiveWorkspacePanel::ContentHub(ContentHubSection::Books)
        );
        let h = s.content_hub.read(cx);
        assert!(
            h.book_reader.as_ref().is_some_and(|r| r.creation_id == "cr_m6_book"),
            "action=read must auto-open BookReaderState"
        );
    });

    // 2. tg://apple/music/cr_m6_music?action=play -> auto-synthesizes & starts MusicTransportEngine!
    shell.update(cx, |s, cx| {
        assert!(s.handle_apple_deep_link("tg://apple/music/cr_m6_music?action=play", cx));
        assert_eq!(
            s.active_panel,
            ActiveWorkspacePanel::ContentHub(ContentHubSection::Music)
        );
        let h = s.content_hub.read(cx);
        assert_eq!(h.music_engine.state(), MusicPlaybackState::Playing);
        assert_eq!(
            h.music_now_playing_creation_id.as_deref(),
            Some("cr_m6_music")
        );
    });

    // 3. tg://apple/games/cr_m6_game?action=play -> auto-launches ActiveGameSession!
    shell.update(cx, |s, cx| {
        assert!(s.handle_apple_deep_link("tg://apple/games/cr_m6_game?action=play", cx));
        assert_eq!(
            s.active_panel,
            ActiveWorkspacePanel::ContentHub(ContentHubSection::Games)
        );
        let h = s.content_hub.read(cx);
        assert!(
            h.active_game.as_ref().is_some_and(|g| g.creation_id == "cr_m6_game"),
            "action=play must auto-launch ActiveGameSession"
        );
    });

    // 4. tg://apple/appstore/cr_m6_app?action=install -> marks in_library!
    shell.update(cx, |s, cx| {
        assert!(s.handle_apple_deep_link("tg://apple/appstore/cr_m6_app?action=install", cx));
        assert_eq!(
            s.active_panel,
            ActiveWorkspacePanel::ContentHub(ContentHubSection::AppStore)
        );
        let h = s.content_hub.read(cx);
        assert!(
            h.my_creation_states
                .get("cr_m6_app")
                .is_some_and(|st| st.in_library),
            "action=install must mark app in_library"
        );
    });
}

fn body_keyboard_escape_review_ime_and_eviction(cx: &mut App) {
    let (_dir, shell) = boot_authed_m6_shell(cx);
    let book = make_sample_book("cr_m6_kb_book", 99); // owned by another user (non-owner)
    let game = make_sample_game(
        "cr_m6_kb_game",
        99,
        GameSpec::TriviaQuiz {
            time_limit_secs: None,
            questions: vec![TriviaQuestion {
                question_id: "q1".to_string(),
                prompt: "Select A".to_string(),
                choices: vec!["A".to_string(), "B".to_string()],
                correct_index: 0,
                explanation: None,
                points: 100,
            }],
        },
    );

    shell.update(cx, |s, cx| {
        s.content_hub.update(cx, |h, hcx| {
            h.books_catalog = vec![dto_to_catalog_item(&book)];
            h.games_catalog = vec![dto_to_catalog_item(&game)];
            hcx.notify();
        });
        s.switch_to_content_hub(ContentHubSection::Books, cx);
        // Focus ReviewBody and commit CJK + ASCII text; must NOT leak to CanonicalComposer
        s.content_hub.update(cx, |h, hcx| {
            h.focus_input(ContentHubInputTarget::ReviewBody, hcx);
        });
        s.insert_committed_text("太棒了 Excellent!", cx);
        assert_eq!(
            s.content_hub.read(cx).review_draft_body.text,
            "太棒了 Excellent!"
        );
        assert!(s.composer.read(cx).text().is_empty());

        // Open BookReader and turn page with right arrow, then close with Escape
        s.content_hub.update(cx, |h, hcx| {
            h.blur_input();
            h.selected_creation_id = Some(book.creation_id.clone());
            h.selected_detail_creation = Some(book.clone());
            assert!(h.open_book_reader_for_creation(&book));
            hcx.notify();
        });
        assert!(s.handle_non_char_key("right", false, cx));
        assert_eq!(
            s.content_hub
                .read(cx)
                .book_reader
                .as_ref()
                .unwrap()
                .current_page_index,
            1
        );
        // Escape #1 closes BookReader; Escape #2 clears detail selection
        assert!(s.handle_non_char_key("escape", false, cx));
        assert!(s.content_hub.read(cx).book_reader.is_none());
        assert!(s.content_hub.read(cx).selected_creation_id.is_some());
        assert!(s.handle_non_char_key("escape", false, cx));
        assert!(s.content_hub.read(cx).selected_creation_id.is_none());

        // Launch game, feed '1' + Enter via keyboard -> wins TriviaQuiz, then WS removed evicts it!
        s.switch_to_content_hub(ContentHubSection::Games, cx);
        s.launch_game_session_in_hub(game.clone(), cx);
        assert!(s.content_hub.read(cx).active_game.is_some());
        s.insert_committed_text("1", cx);
        assert!(s.handle_non_char_key("enter", false, cx));
        assert_eq!(
            s.content_hub
                .read(cx)
                .active_game
                .as_ref()
                .unwrap()
                .snapshot()
                .status,
            GameStatus::Won
        );
        // WS PublicCatalogChanged("removed") evicts non-owned game and closes active_game
        s.handle_ws_public_catalog_changed(
            game.creation_id.clone(),
            CreationKind::Game,
            "removed".to_string(),
            cx,
        );
        assert!(s.content_hub.read(cx).active_game.is_none());
        assert!(s.content_hub.read(cx).games_catalog.is_empty());
    });
}

fn body_radio_and_all_four_verticals_state_flow(cx: &mut App) {
    let (_dir, shell) = boot_authed_m6_shell(cx);
    let music = make_sample_music("cr_m6_radio_1", 12);
    shell.update(cx, |s, cx| {
        s.content_hub.update(cx, |h, hcx| {
            h.music_catalog = vec![dto_to_catalog_item(&music)];
            hcx.notify();
        });
        s.switch_to_content_hub(ContentHubSection::Music, cx);
        s.handle_content_hub_event(
            &ContentHubEvent::PlayMusicRadio {
                genre_filter: Some("electronic".to_string()),
            },
            cx,
        );
        assert_eq!(
            s.content_hub.read(cx).music_engine.state(),
            MusicPlaybackState::Playing
        );
        assert_eq!(s.content_hub.read(cx).music_engine.queue().len(), 2);

        // Switching to Books keeps persistent MusicTransportEngine playing!
        s.switch_to_content_hub(ContentHubSection::Books, cx);
        assert_eq!(
            s.content_hub.read(cx).music_engine.state(),
            MusicPlaybackState::Playing
        );

        // Logout stops music & clears all M6 state
        s.reset_content_hub_for_logout_or_account_switch(cx);
        let h = s.content_hub.read(cx);
        assert_eq!(h.music_engine.state(), MusicPlaybackState::Stopped);
        assert!(h.music_engine.queue().is_empty());
        assert!(h.book_reader.is_none());
        assert!(h.active_game.is_none());

        // Switching to Community syncs AppleAppSidebar effective_width (never stacking ChatSidebar width)
        s.open_community_workspace(cx);
        assert_eq!(s.active_panel, ActiveWorkspacePanel::Community);
        let expected_w = s.apple_sidebar.read(cx).effective_width();
        let cw = s.community_workspace().cloned().expect("mounted community workspace");
        assert!(
            (cw.read(cx).host_sidebar_width_px - expected_w).abs() < 0.5,
            "CommunityWorkspace host_sidebar_width_px must match AppleAppSidebar effective_width ({expected_w})"
        );
    });
}

#[test]
fn test_v1617_zero_fake_live_views_and_sidebar_layout_invariants() {
    use gpui_telegram_desktop::views::call_overlay::CallOverlay;
    use gpui_telegram_desktop::views::computer_workbench::ComputerWorkbench;
    use gpui_telegram_desktop::views::global_tasks_workspace::GlobalTasksWorkspace;
    use gpui_telegram_desktop::views::settings::sections::settings_calls::SettingsCallsSections;

    let palette = TdesktopThemePalette::build(TdesktopThemeMode::Night, 0x007AFF, 14, 16);

    // 1. CallOverlay::new_live has zero fake outputs or synthetic E2EE emojis
    let call = CallOverlay::new_live("Peer".into(), None, None, palette, "zh-CN");
    assert!(call.available_outputs.is_empty(), "CallOverlay::new_live must not seed fake audio outputs");
    assert!(call.e2ee_emojis.is_empty(), "CallOverlay::new_live must not seed fake E2EE emojis");

    // 2. SettingsCallsSections::new_live has zero hardcoded FaceTime/Built-in devices
    let calls_cfg = SettingsCallsSections::new_live(palette, "zh-CN");
    assert!(calls_cfg.available_inputs.is_empty());
    assert!(calls_cfg.available_outputs.is_empty());
    assert!(calls_cfg.available_cameras.is_empty());
    assert!(calls_cfg.input_device.is_empty());
    assert!(calls_cfg.output_device.is_empty());
    assert!(calls_cfg.camera_device.is_empty());

    // 3. ComputerWorkbench::new_live has zero fake displays or synthetic connection state
    let wb = ComputerWorkbench::new_live(String::new(), palette, "zh-CN");
    assert!(wb.displays.is_empty(), "ComputerWorkbench::new_live must not seed fake displays");
    assert!(!wb.is_connected);
    assert!(wb.device.ip_address.is_empty());

    // 4. GlobalTasksWorkspace::new_live has zero fake healthy/toolset counts
    let gt = GlobalTasksWorkspace::new_live(palette, "zh-CN");
    assert!(!gt.status_summary.healthy);
    assert_eq!(gt.status_summary.toolsets_count, 0);
    assert_eq!(gt.status_summary.tools_count, 0);
    assert!(gt.toolsets.is_empty());
}

#[test]
fn test_m6_gpui_deep_link_auto_open_reader_music_game_appstore() {
    run_in_headless_subprocess("test_m6_gpui_deep_link_auto_open_reader_music_game_appstore");
}

#[test]
fn test_m6_gpui_keyboard_escape_review_ime_and_eviction() {
    run_in_headless_subprocess("test_m6_gpui_keyboard_escape_review_ime_and_eviction");
}

#[test]
fn test_m6_gpui_radio_and_all_four_verticals_state_flow() {
    run_in_headless_subprocess("test_m6_gpui_radio_and_all_four_verticals_state_flow");
}

