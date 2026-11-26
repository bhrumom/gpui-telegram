use gpui_telegram_proto::*;

fn sample_book_creation() -> UserCreationDto {
    UserCreationDto {
        creation_id: "cr_book_001".to_string(),
        owner_user_id: 1001,
        creator_bot_user_id: Some(CREATOR_STUDIO_BOT_USER_ID),
        creator_chat_id: Some(5001),
        kind: CreationKind::Book,
        title: "Starlight Chronicles".to_string(),
        subtitle: "Book One of the Nebula Cycle".to_string(),
        summary: "An epic voyage across uncharted star lanes with richly illustrated chapters."
            .to_string(),
        author_display_name: "Elena Vance".to_string(),
        language: "en".to_string(),
        genres: vec!["Sci-Fi".to_string(), "Adventure".to_string()],
        tags: vec!["space".to_string(), "nebula".to_string()],
        cover: CoverArtwork {
            gradient_start_hex: "#1E3A8A".to_string(),
            gradient_end_hex: "#0F172A".to_string(),
            accent_hex: "#38BDF8".to_string(),
            icon_emoji: "📘".to_string(),
            media_file_id: Some("media_cover_1".to_string()),
            thumbnail_data_url: Some("data:image/png;base64,iVBORw0KGgo=".to_string()),
        },
        age_rating: "9+".to_string(),
        payload: CreationPayload::Book(BookPayload {
            author: "Elena Vance".to_string(),
            series_name: Some("Nebula Cycle".to_string()),
            series_index: Some(1),
            blurb: "Across the silent rim, a single beacon wakes.".to_string(),
            estimated_read_mins: 12,
            word_count: 140,
            chapters: vec![BookChapter {
                chapter_id: "ch_1".to_string(),
                chapter_number: 1,
                title: "The Beacon Awakes".to_string(),
                subtitle: Some("Orbit of Helios".to_string()),
                pages: vec![BookPage {
                    page_number: 1,
                    blocks: vec![
                        BookRichBlock::Heading {
                            level: 1,
                            text: "Chapter One".to_string(),
                        },
                        BookRichBlock::Paragraph {
                            text: "When the observatory sensors chimed at midnight, Captain Lin knew the ancient relay had finally answered across sixty light years of silence."
                                .to_string(),
                        },
                        BookRichBlock::BlockQuote {
                            text: "Listen closely to the quiet stars.".to_string(),
                            attribution: Some("Navigator's Codex".to_string()),
                        },
                        BookRichBlock::Callout {
                            title: "Field Note".to_string(),
                            text: "Signal pulse interval: 4.2 seconds.".to_string(),
                        },
                        BookRichBlock::Illustration {
                            caption: "Orbital relay silhouette".to_string(),
                            media_file_id: Some("media_ill_1".to_string()),
                            palette_hex: Some("#38BDF8".to_string()),
                        },
                        BookRichBlock::Divider,
                    ],
                }],
            }],
        }),
        version: 1,
        cloud_revision: 3,
        review_status: CreationReviewStatus::Draft,
        visibility: CreationVisibility::Public,
        has_live_published_version: false,
        published_version: None,
        rejection_reason: None,
        review_notes: None,
        submitted_at_ms: None,
        reviewed_at_ms: None,
        published_at_ms: None,
        created_at_ms: 1_700_000_000_000,
        updated_at_ms: 1_700_000_000_500,
        deleted_at_ms: None,
        rating_avg: 4.8,
        rating_count: 12,
        engagement_count: 42,
        save_or_install_count: 19,
    }
}

fn sample_music_creation() -> UserCreationDto {
    let mut dto = sample_book_creation();
    dto.creation_id = "cr_music_001".to_string();
    dto.kind = CreationKind::Music;
    dto.title = "Midnight Piano Etude".to_string();
    dto.payload = CreationPayload::Music(MusicPayload {
        artist: "Sora Takahashi".to_string(),
        album_title: "Nocturnes in Rust".to_string(),
        release_year: 2026,
        primary_genre: "Ambient".to_string(),
        mood_tags: vec!["calm".to_string(), "focus".to_string()],
        bpm: 112,
        musical_key: "C major".to_string(),
        time_signature: "4/4".to_string(),
        total_duration_secs: 24,
        tracks: vec![MusicTrack {
            track_id: "trk_1".to_string(),
            track_number: 1,
            title: "First Snow".to_string(),
            duration_secs: 24,
            bpm: 112,
            musical_key: "C".to_string(),
            instruments: vec![InstrumentSpec {
                channel: 0,
                name: "Felt Piano".to_string(),
                waveform: SynthWaveform::Piano,
                gain: 0.85,
                pan: -0.15,
            }],
            melody_notes: vec![
                NoteEvent { midi_note: 60, start_beat: 0.0, duration_beats: 1.0, velocity: 96, channel: 0 },
                NoteEvent { midi_note: 64, start_beat: 1.0, duration_beats: 1.0, velocity: 92, channel: 0 },
                NoteEvent { midi_note: 67, start_beat: 2.0, duration_beats: 1.0, velocity: 95, channel: 0 },
                NoteEvent { midi_note: 72, start_beat: 3.0, duration_beats: 1.5, velocity: 100, channel: 0 },
            ],
            chord_progression: vec![ChordEvent {
                symbol: "Cmaj7".to_string(),
                start_beat: 0.0,
                duration_beats: 4.0,
                velocity: 68,
                channel: None,
            }],
            lyrics: vec![SyncedLyricLine {
                timestamp_ms: 500,
                text: "Soft lanterns glow along the avenue".to_string(),
            }],
            audio_media_file_id: None,
            waveform_5bit: vec![1, 2, 3],
        }],
    });
    dto
}

fn sample_game_creation(engine_kind: GameEngineKind, spec: GameSpec) -> UserCreationDto {
    let mut dto = sample_book_creation();
    dto.creation_id = "cr_game_001".to_string();
    dto.kind = CreationKind::Game;
    dto.title = "Arcade Micro Challenge".to_string();
    dto.payload = CreationPayload::Game(GamePayload {
        developer: "Studio Pixel".to_string(),
        category: GameCategory::Arcade,
        controls_hint: "Arrow keys or WASD to move".to_string(),
        rules_markdown: "Complete the objective to record a new leaderboard score.".to_string(),
        engine_kind,
        supports_leaderboard: true,
        estimated_session_mins: 5,
        spec,
        achievements: vec![GameAchievementDef {
            achievement_id: "ach_1".to_string(),
            title: "First Victory".to_string(),
            description: "Clear your first round".to_string(),
            icon_emoji: "🏆".to_string(),
            target_score: Some(100),
        }],
    });
    dto
}

fn sample_app_creation() -> UserCreationDto {
    let mut dto = sample_book_creation();
    dto.creation_id = "cr_app_001".to_string();
    dto.kind = CreationKind::App;
    dto.title = "Focus Pomodoro".to_string();
    dto.payload = CreationPayload::App(AppStoreListingPayload {
        developer_name: "Flow Craft Labs".to_string(),
        version_string: "1.2.0".to_string(),
        release_notes: "Added deep focus stats".to_string(),
        category: "Productivity".to_string(),
        long_description: "A calm Apple-style timer and habit companion built as a Telegram Mini App."
            .to_string(),
        screenshots: vec![ScreenshotAsset {
            title: "Session View".to_string(),
            caption: Some("Minimalist countdown ring".to_string()),
            media_file_id: Some("media_shot_1".to_string()),
            accent_hex: Some("#2563EB".to_string()),
        }],
        size_label: "2.4 MB".to_string(),
        price_label: "Free".to_string(),
        permissions: vec!["cloud_storage".to_string()],
        linked_miniapp: Some(LinkedMiniAppRef {
            bot_username: "FocusBot".to_string(),
            short_name: "pomodoro".to_string(),
            web_url: Some("https://apps.example.com/pomodoro".to_string()),
            launch_param: Some("home".to_string()),
        }),
        ui_schema_json: Some(serde_json::json!({"layout": "timer", "theme": "macOS"})),
    });
    dto
}

#[test]
fn test_enum_snake_case_serde_and_helpers() {
    for kind in CreationKind::ALL {
        let json = serde_json::to_string(&kind).unwrap();
        assert_eq!(json, format!("\"{}\"", kind.as_str()));
        let back: CreationKind = serde_json::from_str(&json).unwrap();
        assert_eq!(back, kind);
        assert_eq!(CreationKind::parse(kind.as_str()), Some(kind));
    }
    assert_eq!(CreationKind::Book.apple_section_slug(), "books");
    assert_eq!(CreationKind::Music.apple_section_slug(), "music");
    assert_eq!(CreationKind::Game.apple_section_slug(), "games");
    assert_eq!(CreationKind::App.apple_section_slug(), "appstore");
    assert_eq!(
        CreationKind::App.deep_link_uri("cr_app_9"),
        "tg://apple/appstore/cr_app_9"
    );

    let statuses = [
        (CreationReviewStatus::Draft, "draft"),
        (CreationReviewStatus::PendingReview, "pending_review"),
        (CreationReviewStatus::Published, "published"),
        (CreationReviewStatus::Rejected, "rejected"),
        (CreationReviewStatus::Unpublished, "unpublished"),
    ];
    for (st, expected) in statuses {
        assert_eq!(serde_json::to_string(&st).unwrap(), format!("\"{expected}\""));
        assert_eq!(CreationReviewStatus::parse(expected), Some(st));
    }

    for (vis, expected) in [
        (CreationVisibility::Private, "private"),
        (CreationVisibility::Unlisted, "unlisted"),
        (CreationVisibility::Public, "public"),
    ] {
        assert_eq!(serde_json::to_string(&vis).unwrap(), format!("\"{expected}\""));
        assert_eq!(CreationVisibility::parse(expected), Some(vis));
    }

    for (sort, expected) in [
        (CatalogSortMode::Featured, "featured"),
        (CatalogSortMode::Popular, "popular"),
        (CatalogSortMode::TopRated, "top_rated"),
        (CatalogSortMode::Newest, "newest"),
    ] {
        assert_eq!(serde_json::to_string(&sort).unwrap(), format!("\"{expected}\""));
        assert_eq!(CatalogSortMode::parse(expected), Some(sort));
    }

    for (dec, expected) in [
        (ReviewDecision::Approve, "approve"),
        (ReviewDecision::Reject, "reject"),
    ] {
        assert_eq!(serde_json::to_string(&dec).unwrap(), format!("\"{expected}\""));
        assert_eq!(ReviewDecision::parse(expected), Some(dec));
    }
}

#[test]
fn test_all_payloads_and_seven_game_specs_roundtrip_and_validate() {
    let book = sample_book_creation();
    assert_eq!(validate_creation_for_review(&book), Ok(()));
    let book_json = serde_json::to_string(&book).unwrap();
    assert!(book_json.contains("\"kind\":\"book\""));
    let book_back: UserCreationDto = serde_json::from_str(&book_json).unwrap();
    assert_eq!(book_back, book);

    let music = sample_music_creation();
    assert_eq!(validate_creation_for_review(&music), Ok(()));
    let music_json = serde_json::to_string(&music).unwrap();
    assert!(music_json.contains("\"kind\":\"music\""));
    let music_back: UserCreationDto = serde_json::from_str(&music_json).unwrap();
    assert_eq!(music_back, music);

    let app = sample_app_creation();
    assert_eq!(validate_creation_for_review(&app), Ok(()));
    let app_json = serde_json::to_string(&app).unwrap();
    assert!(app_json.contains("\"kind\":\"app\""));
    let app_back: UserCreationDto = serde_json::from_str(&app_json).unwrap();
    assert_eq!(app_back, app);

    let seven_specs = vec![
        (
            GameEngineKind::GridPuzzle,
            GameSpec::GridPuzzle {
                rows: 3,
                cols: 3,
                levels: vec![GridPuzzleLevel {
                    level_id: "lvl_1".to_string(),
                    title: "Warmup".to_string(),
                    initial_grid: vec![vec![1, 2, 0], vec![4, 5, 3], vec![7, 8, 6]],
                    target_grid: Some(vec![vec![1, 2, 3], vec![4, 5, 6], vec![7, 8, 0]]),
                    max_moves: Some(12),
                    hint: Some("Slide 3 up".to_string()),
                }],
            },
        ),
        (
            GameEngineKind::TriviaQuiz,
            GameSpec::TriviaQuiz {
                time_limit_secs: Some(60),
                questions: vec![TriviaQuestion {
                    question_id: "q1".to_string(),
                    prompt: "Which planet is known as the Red Planet?".to_string(),
                    choices: vec!["Venus".to_string(), "Mars".to_string(), "Jupiter".to_string()],
                    correct_index: 1,
                    explanation: Some("Iron oxide dust gives Mars its color.".to_string()),
                    points: 100,
                }],
            },
        ),
        (
            GameEngineKind::InteractiveFiction,
            GameSpec::InteractiveFiction {
                start_node_id: "start".to_string(),
                nodes: vec![
                    StoryNode {
                        node_id: "start".to_string(),
                        title: "Docking Bay".to_string(),
                        body: "Two corridors stretch ahead.".to_string(),
                        choices: vec![StoryChoice {
                            label: "Enter bridge".to_string(),
                            target_node_id: "end_good".to_string(),
                            score_delta: 50,
                        }],
                        is_ending: false,
                        ending_badge: None,
                    },
                    StoryNode {
                        node_id: "end_good".to_string(),
                        title: "Safe Harbor".to_string(),
                        body: "You plot the course home.".to_string(),
                        choices: vec![],
                        is_ending: true,
                        ending_badge: Some("Captain".to_string()),
                    },
                ],
            },
        ),
        (
            GameEngineKind::ArcadeDodge,
            GameSpec::ArcadeDodge {
                tick_ms: 16,
                arena_width: 480,
                arena_height: 360,
                player_speed: 240.0,
                obstacle_speed: 180.0,
                lives: 3,
                target_score: 500,
                theme_accent_hex: Some("#F59E0B".to_string()),
            },
        ),
        (
            GameEngineKind::SnakeGrid,
            GameSpec::SnakeGrid {
                grid_width: 16,
                grid_height: 16,
                tick_ms: 90,
                wrap_walls: true,
                target_score: 20,
            },
        ),
        (
            GameEngineKind::CardMatch,
            GameSpec::CardMatch {
                pairs: vec![
                    CardPair {
                        pair_id: "p1".to_string(),
                        label: "Sun".to_string(),
                        symbol: "☀️".to_string(),
                        accent_hex: Some("#F59E0B".to_string()),
                    },
                    CardPair {
                        pair_id: "p2".to_string(),
                        label: "Moon".to_string(),
                        symbol: "🌙".to_string(),
                        accent_hex: Some("#6366F1".to_string()),
                    },
                ],
                time_limit_secs: Some(90),
            },
        ),
        (
            GameEngineKind::MiniAppHtml,
            GameSpec::MiniAppHtml {
                bot_username: "ArcadeBot".to_string(),
                short_name: "star_racer".to_string(),
                html_bundle_media_id: None,
            },
        ),
    ];

    for (engine, spec) in seven_specs {
        let game = sample_game_creation(engine, spec);
        assert_eq!(
            validate_creation_for_review(&game),
            Ok(()),
            "expected valid game for {engine:?}"
        );
        let json = serde_json::to_string(&game).unwrap();
        let back: UserCreationDto = serde_json::from_str(&json).unwrap();
        assert_eq!(back, game);
    }
}

#[test]
fn test_update_event_creation_and_catalog_wire_json() {
    let ev1 = UpdateEvent::UserCreationChanged {
        creation_id: "cr_book_77".to_string(),
        owner_user_id: 42,
        kind: CreationKind::Book,
        review_status: CreationReviewStatus::PendingReview,
        cloud_revision: 9,
    };
    let json1 = serde_json::to_string(&ev1).unwrap();
    let v1: serde_json::Value = serde_json::from_str(&json1).unwrap();
    assert_eq!(v1["type"], "UserCreationChanged");
    assert_eq!(v1["data"]["creation_id"], "cr_book_77");
    assert_eq!(v1["data"]["owner_user_id"], 42);
    assert_eq!(v1["data"]["kind"], "book");
    assert_eq!(v1["data"]["review_status"], "pending_review");
    assert_eq!(v1["data"]["cloud_revision"], 9);
    assert_eq!(serde_json::from_str::<UpdateEvent>(&json1).unwrap(), ev1);

    let ev2 = UpdateEvent::PublicCatalogChanged {
        creation_id: "cr_music_88".to_string(),
        kind: CreationKind::Music,
        action: "published".to_string(),
    };
    let json2 = serde_json::to_string(&ev2).unwrap();
    let v2: serde_json::Value = serde_json::from_str(&json2).unwrap();
    assert_eq!(v2["type"], "PublicCatalogChanged");
    assert_eq!(v2["data"]["creation_id"], "cr_music_88");
    assert_eq!(v2["data"]["kind"], "music");
    assert_eq!(v2["data"]["action"], "published");
    assert_eq!(serde_json::from_str::<UpdateEvent>(&json2).unwrap(), ev2);

    // WsServerFrame envelope round-trip
    let frame = WsServerFrame::Update(ev2);
    let frame_json = serde_json::to_string(&frame).unwrap();
    let frame_back: WsServerFrame = serde_json::from_str(&frame_json).unwrap();
    assert_eq!(frame_back, frame);
}

#[test]
fn test_validate_common_and_security_rejections() {
    // 1. Multiple common errors collected together
    let mut bad = sample_book_creation();
    bad.title = "   ".to_string();
    bad.summary = "short".to_string();
    bad.author_display_name = "".to_string();
    bad.genres = vec!["a".into(), "b".into(), "c".into(), "d".into(), "e".into(), "f".into()];
    bad.tags = vec!["".into(); 13];
    bad.cover.gradient_start_hex = "123456".to_string();
    bad.cover.gradient_end_hex = "#GGG123".to_string();
    bad.cover.accent_hex = "#123".to_string();
    bad.rating_avg = f32::NAN;
    let errs = validate_creation_for_review(&bad).unwrap_err();
    assert!(errs.len() >= 8, "expected all violations collected, got: {errs:?}");

    // 2. Kind vs payload mismatch
    let mut mismatch = sample_book_creation();
    mismatch.kind = CreationKind::Music;
    let errs = validate_creation_for_review(&mismatch).unwrap_err();
    assert!(errs.iter().any(|e| e.contains("does not match payload kind")));

    // 3. XSS across top-level and nested fields (case-insensitive)
    let mut xss_title = sample_book_creation();
    xss_title.title = "Hello <ScRiPt>alert(1)</script>".to_string();
    assert!(validate_creation_for_review(&xss_title).is_err());

    let mut xss_nested_book = sample_book_creation();
    if let CreationPayload::Book(ref mut b) = xss_nested_book.payload {
        b.chapters[0].pages[0].blocks.push(BookRichBlock::Paragraph {
            text: "Click JaVaScRiPt:alert(1) now".to_string(),
        });
    }
    assert!(validate_creation_for_review(&xss_nested_book).is_err());

    let mut xss_thumb = sample_book_creation();
    xss_thumb.cover.thumbnail_data_url =
        Some("data:image/svg+xml,<svg onload=alert(1)>".to_string());
    assert!(validate_creation_for_review(&xss_thumb).is_err());

    // 4. Payload exceeding 4 MiB
    let mut huge = sample_book_creation();
    if let CreationPayload::Book(ref mut b) = huge.payload {
        b.blurb = "x".repeat(MAX_CREATION_PAYLOAD_BYTES + 1024);
    }
    let errs = validate_creation_for_review(&huge).unwrap_err();
    assert!(errs.iter().any(|e| e.contains("exceeds maximum")));
}

#[test]
fn test_validate_book_music_game_app_boundaries() {
    // Book: < 50 non-whitespace readable chars rejected
    let mut short_book = sample_book_creation();
    if let CreationPayload::Book(ref mut b) = short_book.payload {
        b.chapters[0].pages[0].blocks = vec![BookRichBlock::Paragraph {
            text: "Too short text".to_string(),
        }];
    }
    let errs = validate_creation_for_review(&short_book).unwrap_err();
    assert!(errs.iter().any(|e| e.contains("readable non-whitespace")));

    // Music: bad BPM, < 4 notes without audio_media_file_id, MIDI > 127, NaN beat
    let mut bad_music = sample_music_creation();
    if let CreationPayload::Music(ref mut m) = bad_music.payload {
        m.bpm = 29;
        m.tracks[0].bpm = 301;
        m.tracks[0].duration_secs = 2;
        m.tracks[0].melody_notes = vec![NoteEvent {
            midi_note: 128,
            start_beat: -1.0,
            duration_beats: 0.0,
            velocity: 80,
            channel: 0,
        }];
    }
    let errs = validate_creation_for_review(&bad_music).unwrap_err();
    assert!(errs.len() >= 6, "got: {errs:?}");

    // Music: 0 melody_notes IS valid when audio_media_file_id is provided
    let mut audio_only_music = sample_music_creation();
    if let CreationPayload::Music(ref mut m) = audio_only_music.payload {
        m.tracks[0].melody_notes.clear();
        m.tracks[0].audio_media_file_id = Some("cas_wav_12345".to_string());
    }
    assert_eq!(validate_creation_for_review(&audio_only_music), Ok(()));

    // Game: engine_kind mismatch
    let mut mismatch_game = sample_game_creation(
        GameEngineKind::TriviaQuiz,
        GameSpec::SnakeGrid {
            grid_width: 10,
            grid_height: 10,
            tick_ms: 80,
            wrap_walls: false,
            target_score: 10,
        },
    );
    assert!(validate_creation_for_review(&mismatch_game).is_err());

    // Game: InteractiveFiction cycle with unreachable ending rejected
    let unreachable_if = sample_game_creation(
        GameEngineKind::InteractiveFiction,
        GameSpec::InteractiveFiction {
            start_node_id: "n1".to_string(),
            nodes: vec![
                StoryNode {
                    node_id: "n1".to_string(),
                    title: "Loop A".to_string(),
                    body: "Going in circles.".to_string(),
                    choices: vec![StoryChoice {
                        label: "Next".to_string(),
                        target_node_id: "n2".to_string(),
                        score_delta: 0,
                    }],
                    is_ending: false,
                    ending_badge: None,
                },
                StoryNode {
                    node_id: "n2".to_string(),
                    title: "Loop B".to_string(),
                    body: "Back to A.".to_string(),
                    choices: vec![StoryChoice {
                        label: "Back".to_string(),
                        target_node_id: "n1".to_string(),
                        score_delta: 0,
                    }],
                    is_ending: false,
                    ending_badge: None,
                },
                StoryNode {
                    node_id: "island_end".to_string(),
                    title: "Unreachable".to_string(),
                    body: "Nobody can reach this.".to_string(),
                    choices: vec![],
                    is_ending: true,
                    ending_badge: None,
                },
            ],
        },
    );
    let errs = validate_creation_for_review(&unreachable_if).unwrap_err();
    assert!(errs.iter().any(|e| e.contains("no ending reachable")));

    // Game: Trivia out-of-bounds correct_index
    let bad_trivia = sample_game_creation(
        GameEngineKind::TriviaQuiz,
        GameSpec::TriviaQuiz {
            time_limit_secs: None,
            questions: vec![TriviaQuestion {
                question_id: "q1".to_string(),
                prompt: "2 + 2?".to_string(),
                choices: vec!["3".to_string(), "4".to_string()],
                correct_index: 2,
                explanation: None,
                points: 10,
            }],
        },
    );
    assert!(validate_creation_for_review(&bad_trivia).is_err());

    // App: unsafe web_url & XSS inside ui_schema_json rejected
    let mut bad_app = sample_app_creation();
    if let CreationPayload::App(ref mut a) = bad_app.payload {
        a.linked_miniapp = Some(LinkedMiniAppRef {
            bot_username: "".to_string(),
            short_name: "".to_string(),
            web_url: Some("javascript:alert(1)".to_string()),
            launch_param: None,
        });
        a.ui_schema_json = Some(serde_json::json!({ "html": "<script>evil()</script>" }));
    }
    let errs = validate_creation_for_review(&bad_app).unwrap_err();
    assert!(errs.len() >= 2, "got: {errs:?}");

    // Ensure REST & catalog DTOs serialize cleanly
    mismatch_game.kind = CreationKind::Game;
    let rec = BotRecommendationItem {
        creation_id: "cr_book_001".to_string(),
        kind: CreationKind::Book,
        title: "Starlight Chronicles".to_string(),
        subtitle: "".to_string(),
        summary: "Epic sci-fi.".to_string(),
        author_display_name: "Elena Vance".to_string(),
        genres: vec!["Sci-Fi".to_string()],
        cover: CoverArtwork::default(),
        rating_avg: 4.9,
        rating_count: 25,
        score: 0.96,
        reason: "Top rated Sci-Fi book".to_string(),
        deep_link: CreationKind::Book.deep_link_uri("cr_book_001"),
    };
    let rec_resp = CatalogRecommendResponse { items: vec![rec] };
    let rec_json = serde_json::to_string(&rec_resp).unwrap();
    let rec_back: CatalogRecommendResponse = serde_json::from_str(&rec_json).unwrap();
    assert_eq!(rec_back, rec_resp);
}

#[test]
fn test_grid_puzzle_rectangular_dimension_validation() {
    // Valid rectangular 2x3 grid passes
    let valid_rect = sample_game_creation(
        GameEngineKind::GridPuzzle,
        GameSpec::GridPuzzle {
            rows: 2,
            cols: 3,
            levels: vec![GridPuzzleLevel {
                level_id: "lvl_rect".to_string(),
                title: "Rect".to_string(),
                initial_grid: vec![vec![1, 2, 3], vec![4, 5, 0]],
                target_grid: Some(vec![vec![1, 2, 3], vec![4, 0, 5]]),
                max_moves: Some(5),
                hint: None,
            }],
        },
    );
    assert_eq!(validate_creation_for_review(&valid_rect), Ok(()));

    // Ragged initial_grid (3x3 declared, row 1 has length 1) rejected
    let ragged_initial = sample_game_creation(
        GameEngineKind::GridPuzzle,
        GameSpec::GridPuzzle {
            rows: 3,
            cols: 3,
            levels: vec![GridPuzzleLevel {
                level_id: "lvl_ragged".to_string(),
                title: "Ragged".to_string(),
                initial_grid: vec![vec![1, 2, 3], vec![4], vec![7, 8, 9]],
                target_grid: None,
                max_moves: None,
                hint: None,
            }],
        },
    );
    let errs = validate_creation_for_review(&ragged_initial).unwrap_err();
    assert!(
        errs.iter().any(|e| e.contains("rectangular 3x3")),
        "expected rectangular 3x3 error, got: {errs:?}"
    );

    // Mismatched 2x2 initial_grid vs declared 4x4 + ragged target_grid rejected
    let mismatched_both = sample_game_creation(
        GameEngineKind::GridPuzzle,
        GameSpec::GridPuzzle {
            rows: 4,
            cols: 4,
            levels: vec![GridPuzzleLevel {
                level_id: "lvl_mis".to_string(),
                title: "Mismatch".to_string(),
                initial_grid: vec![vec![1, 2], vec![3, 4]],
                target_grid: Some(vec![vec![1], vec![2, 3, 4]]),
                max_moves: None,
                hint: None,
            }],
        },
    );
    let errs = validate_creation_for_review(&mismatched_both).unwrap_err();
    assert!(errs.iter().any(|e| e.contains("initial_grid") && e.contains("4x4")));
    assert!(errs.iter().any(|e| e.contains("target_grid") && e.contains("4x4")));

    // Valid 2x2 initial_grid + mismatched 1x3 target_grid rejected
    let mismatched_target = sample_game_creation(
        GameEngineKind::GridPuzzle,
        GameSpec::GridPuzzle {
            rows: 2,
            cols: 2,
            levels: vec![GridPuzzleLevel {
                level_id: "lvl_t".to_string(),
                title: "Bad Target".to_string(),
                initial_grid: vec![vec![1, 2], vec![3, 4]],
                target_grid: Some(vec![vec![1, 2, 3]]),
                max_moves: None,
                hint: None,
            }],
        },
    );
    let errs = validate_creation_for_review(&mismatched_target).unwrap_err();
    assert!(errs.iter().any(|e| e.contains("target_grid") && e.contains("2x2")));
}

#[test]
fn test_xss_rejected_in_all_identifier_and_note_fields() {
    let xss_payloads = [
        "<ScRiPt>alert(1)</script>",
        "JaVaScRiPt:alert(1)",
        "java\tscript:alert(1)",
        "<\0ScRiPt>alert(1)",
    ];

    for payload in xss_payloads {
        // creation_id, review_notes, rejection_reason
        let mut c1 = sample_book_creation();
        c1.creation_id = payload.to_string();
        assert!(validate_creation_for_review(&c1).is_err(), "creation_id accepted {payload:?}");

        let mut c2 = sample_book_creation();
        c2.review_notes = Some(payload.to_string());
        assert!(validate_creation_for_review(&c2).is_err(), "review_notes accepted {payload:?}");

        let mut c3 = sample_book_creation();
        c3.rejection_reason = Some(payload.to_string());
        assert!(validate_creation_for_review(&c3).is_err(), "rejection_reason accepted {payload:?}");

        // chapter_id
        let mut b = sample_book_creation();
        if let CreationPayload::Book(ref mut book) = b.payload {
            book.chapters[0].chapter_id = payload.to_string();
        }
        assert!(validate_creation_for_review(&b).is_err(), "chapter_id accepted {payload:?}");

        // track_id
        let mut m = sample_music_creation();
        if let CreationPayload::Music(ref mut music) = m.payload {
            music.tracks[0].track_id = payload.to_string();
        }
        assert!(validate_creation_for_review(&m).is_err(), "track_id accepted {payload:?}");

        // achievement_id & GridPuzzle level_id
        let mut g_ach = sample_game_creation(
            GameEngineKind::GridPuzzle,
            GameSpec::GridPuzzle {
                rows: 2,
                cols: 2,
                levels: vec![GridPuzzleLevel {
                    level_id: "lvl_ok".to_string(),
                    title: "OK".to_string(),
                    initial_grid: vec![vec![1, 2], vec![3, 0]],
                    target_grid: None,
                    max_moves: None,
                    hint: None,
                }],
            },
        );
        if let CreationPayload::Game(ref mut g) = g_ach.payload {
            g.achievements[0].achievement_id = payload.to_string();
        }
        assert!(validate_creation_for_review(&g_ach).is_err(), "achievement_id accepted {payload:?}");

        let g_lvl = sample_game_creation(
            GameEngineKind::GridPuzzle,
            GameSpec::GridPuzzle {
                rows: 2,
                cols: 2,
                levels: vec![GridPuzzleLevel {
                    level_id: payload.to_string(),
                    title: "OK".to_string(),
                    initial_grid: vec![vec![1, 2], vec![3, 0]],
                    target_grid: None,
                    max_moves: None,
                    hint: None,
                }],
            },
        );
        assert!(validate_creation_for_review(&g_lvl).is_err(), "level_id accepted {payload:?}");

        // Trivia question_id
        let g_q = sample_game_creation(
            GameEngineKind::TriviaQuiz,
            GameSpec::TriviaQuiz {
                time_limit_secs: None,
                questions: vec![TriviaQuestion {
                    question_id: payload.to_string(),
                    prompt: "2+2?".to_string(),
                    choices: vec!["3".to_string(), "4".to_string()],
                    correct_index: 1,
                    explanation: None,
                    points: 10,
                }],
            },
        );
        assert!(validate_creation_for_review(&g_q).is_err(), "question_id accepted {payload:?}");

        // InteractiveFiction start_node_id, node_id, target_node_id
        let g_if = sample_game_creation(
            GameEngineKind::InteractiveFiction,
            GameSpec::InteractiveFiction {
                start_node_id: payload.to_string(),
                nodes: vec![
                    StoryNode {
                        node_id: payload.to_string(),
                        title: "Start".to_string(),
                        body: "Begin.".to_string(),
                        choices: vec![StoryChoice {
                            label: "Finish".to_string(),
                            target_node_id: "end".to_string(),
                            score_delta: 1,
                        }],
                        is_ending: false,
                        ending_badge: None,
                    },
                    StoryNode {
                        node_id: "end".to_string(),
                        title: "End".to_string(),
                        body: "Done.".to_string(),
                        choices: vec![],
                        is_ending: true,
                        ending_badge: None,
                    },
                ],
            },
        );
        assert!(validate_creation_for_review(&g_if).is_err(), "start_node_id/node_id accepted {payload:?}");

        let g_if_target = sample_game_creation(
            GameEngineKind::InteractiveFiction,
            GameSpec::InteractiveFiction {
                start_node_id: "start".to_string(),
                nodes: vec![
                    StoryNode {
                        node_id: "start".to_string(),
                        title: "Start".to_string(),
                        body: "Begin.".to_string(),
                        choices: vec![StoryChoice {
                            label: "Finish".to_string(),
                            target_node_id: payload.to_string(),
                            score_delta: 1,
                        }],
                        is_ending: false,
                        ending_badge: None,
                    },
                    StoryNode {
                        node_id: payload.to_string(),
                        title: "End".to_string(),
                        body: "Done.".to_string(),
                        choices: vec![],
                        is_ending: true,
                        ending_badge: None,
                    },
                ],
            },
        );
        assert!(validate_creation_for_review(&g_if_target).is_err(), "target_node_id accepted {payload:?}");

        // CardMatch pair_id
        let g_pair = sample_game_creation(
            GameEngineKind::CardMatch,
            GameSpec::CardMatch {
                pairs: vec![
                    CardPair {
                        pair_id: payload.to_string(),
                        label: "Sun".to_string(),
                        symbol: "S".to_string(),
                        accent_hex: None,
                    },
                    CardPair {
                        pair_id: "p2".to_string(),
                        label: "Moon".to_string(),
                        symbol: "M".to_string(),
                        accent_hex: None,
                    },
                ],
                time_limit_secs: None,
            },
        );
        assert!(validate_creation_for_review(&g_pair).is_err(), "pair_id accepted {payload:?}");
    }
}

#[test]
fn test_url_pre_trim_crlf_null_ui_schema_alias_and_list_response_default() {
    // Leading/trailing CRLF on thumbnail_data_url rejected
    for raw_url in [
        "\r\nhttps://example.com/a.png",
        "https://example.com/a.png\r\n",
        "\thttps://example.com/a.png",
        "https://example.com/\u{202E}a.png",
    ] {
        let mut book = sample_book_creation();
        book.cover.thumbnail_data_url = Some(raw_url.to_string());
        assert!(
            validate_creation_for_review(&book).is_err(),
            "thumbnail_data_url accepted {raw_url:?}"
        );

        let mut app = sample_app_creation();
        if let CreationPayload::App(ref mut a) = app.payload {
            a.linked_miniapp = Some(LinkedMiniAppRef {
                bot_username: "Bot".to_string(),
                short_name: "app".to_string(),
                web_url: Some(raw_url.to_string()),
                launch_param: None,
            });
        }
        assert!(
            validate_creation_for_review(&app).is_err(),
            "linked_miniapp.web_url accepted {raw_url:?}"
        );
    }

    // AppStoreListingPayload with linked_miniapp: None & ui_schema_json: Some(Null) rejected
    let mut null_schema_app = sample_app_creation();
    if let CreationPayload::App(ref mut a) = null_schema_app.payload {
        a.linked_miniapp = None;
        a.ui_schema_json = Some(serde_json::Value::Null);
    }
    assert!(validate_creation_for_review(&null_schema_app).is_err());

    // Non-object ui_schema_json (e.g. number / array) without linked_miniapp also rejected
    let mut scalar_schema_app = sample_app_creation();
    if let CreationPayload::App(ref mut a) = scalar_schema_app.payload {
        a.linked_miniapp = None;
        a.ui_schema_json = Some(serde_json::json!(42));
    }
    assert!(validate_creation_for_review(&scalar_schema_app).is_err());

    // Valid object ui_schema_json without linked_miniapp accepted
    let mut object_schema_app = sample_app_creation();
    if let CreationPayload::App(ref mut a) = object_schema_app.payload {
        a.linked_miniapp = None;
        a.ui_schema_json = Some(serde_json::json!({"view": "dashboard"}));
    }
    assert_eq!(validate_creation_for_review(&object_schema_app), Ok(()));

    // arcade_runner serde alias for GameEngineKind and GameSpec
    let kind_from_alias: GameEngineKind = serde_json::from_str("\"arcade_runner\"").unwrap();
    assert_eq!(kind_from_alias, GameEngineKind::ArcadeDodge);
    assert_eq!(GameEngineKind::parse("arcade_runner"), Some(GameEngineKind::ArcadeDodge));
    let spec_from_alias: GameSpec = serde_json::from_str(
        r#"{"type":"arcade_runner","tick_ms":16,"arena_width":320,"arena_height":240,"player_speed":200.0,"obstacle_speed":150.0,"lives":3,"target_score":100}"#,
    )
    .unwrap();
    assert_eq!(spec_from_alias.engine_kind(), GameEngineKind::ArcadeDodge);

    // CreationListResponse deserializes cleanly when latest_revision is omitted (review queue)
    let list_resp: CreationListResponse = serde_json::from_str(r#"{"items":[]}"#).unwrap();
    assert!(list_resp.items.is_empty());
    assert_eq!(list_resp.latest_revision, 0);
}
