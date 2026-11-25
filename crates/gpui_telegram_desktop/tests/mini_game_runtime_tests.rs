use gpui_telegram_desktop::views::content_hub::{
    CardFaceState, GameInput, GameRuntimeState, GameStatus, GridCoord, GridPuzzleMode,
    MiniGameRuntime, SnakeDirection,
};
use gpui_telegram_proto::creations::{
    CardPair, GameAchievementDef, GameCategory, GameEngineKind, GamePayload, GameSpec,
    GameUserState, GridPuzzleLevel, StoryChoice, StoryNode, TriviaQuestion,
};

fn payload_with(spec: GameSpec, achievements: Vec<GameAchievementDef>) -> GamePayload {
    GamePayload {
        developer: "Test Studio".to_string(),
        category: GameCategory::Puzzle,
        controls_hint: "Arrow keys or click".to_string(),
        rules_markdown: "Complete the challenge.".to_string(),
        engine_kind: spec.engine_kind(),
        supports_leaderboard: true,
        estimated_session_mins: 3,
        spec,
        achievements,
    }
}

fn ach(id: &str, target_score: Option<i64>) -> GameAchievementDef {
    GameAchievementDef {
        achievement_id: id.to_string(),
        title: format!("Ach {id}"),
        description: "Test achievement".to_string(),
        icon_emoji: "🏆".to_string(),
        target_score,
    }
}

#[test]
fn grid_puzzle_sliding_tile_m1_fixture_click_cell_multi_level_and_max_moves() {
    let spec = GameSpec::GridPuzzle {
        rows: 3,
        cols: 3,
        levels: vec![
            GridPuzzleLevel {
                level_id: "lvl_m1".to_string(),
                title: "Classic 8-Puzzle".to_string(),
                initial_grid: vec![vec![1, 2, 0], vec![4, 5, 3], vec![7, 8, 6]],
                target_grid: Some(vec![vec![1, 2, 3], vec![4, 5, 6], vec![7, 8, 0]]),
                max_moves: Some(12),
                hint: Some("Slide 3 up".to_string()),
            },
            GridPuzzleLevel {
                level_id: "lvl_click".to_string(),
                title: "2x3 Click Level".to_string(),
                initial_grid: vec![vec![1, 2, 3], vec![4, 5, 0]],
                target_grid: Some(vec![vec![1, 2, 3], vec![4, 0, 5]]),
                max_moves: Some(2),
                hint: Some("Click 5".to_string()),
            },
        ],
    };
    let payload = payload_with(
        spec,
        vec![ach("score_150", Some(150)), ach("clear_all", None)],
    );
    let mut rt = MiniGameRuntime::from_payload(&payload, 42);
    assert_eq!(rt.snapshot().status, GameStatus::Ready);

    // Solve M1 3x3 fixture with two MoveUp moves ("Slide 3 up", then "Slide 6 up")
    rt.apply_input(GameInput::MoveUp);
    let mid = rt.snapshot();
    assert_eq!(mid.status, GameStatus::Playing);
    assert_eq!(mid.save_state_json["current_level_id"], "lvl_m1");
    match &mid.state {
        GameRuntimeState::GridPuzzle { mode, grid, moves_used, .. } => {
            assert_eq!(*mode, GridPuzzleMode::SlidingTile);
            assert_eq!(*moves_used, 1);
            assert_eq!(grid, &vec![vec![1, 2, 3], vec![4, 5, 0], vec![7, 8, 6]]);
        }
        other => panic!("expected GridPuzzle, got {other:?}"),
    }

    rt.apply_input(GameInput::MoveUp);
    let lvl2 = rt.snapshot();
    assert_eq!(lvl2.status, GameStatus::Playing);
    assert!(lvl2.score >= 200);
    assert!(lvl2.unlocked_achievements.contains(&"score_150".to_string()));
    assert_eq!(lvl2.save_state_json["current_level_id"], "lvl_click");

    // Solve level 2 via ClickCell on tile 5 at (1, 1) adjacent to blank (1, 2)
    rt.apply_input(GameInput::ClickCell { row: 1, col: 1 });
    let won = rt.snapshot();
    assert_eq!(won.status, GameStatus::Won);
    assert!(won.unlocked_achievements.contains(&"clear_all".to_string()));
    let peak_score = won.score;
    assert_eq!(won.high_score, peak_score);

    let user_state = rt.to_game_user_state(
        Some(&GameUserState {
            high_score: 50,
            play_count: 2,
            unlocked_achievements: vec!["legacy_ach".to_string()],
            save_state_json: None,
            in_library: true,
            last_played_at_ms: 100,
        }),
        9_999,
    );
    assert_eq!(user_state.high_score, peak_score);
    assert_eq!(user_state.play_count, 3);
    assert!(user_state.unlocked_achievements.contains(&"legacy_ach".to_string()));
    assert!(user_state.unlocked_achievements.contains(&"clear_all".to_string()));
    assert_eq!(user_state.last_played_at_ms, 9_999);

    // Restart preserves high_score and lets us test max_moves GameOver
    rt.restart();
    assert_eq!(rt.snapshot().score, 0);
    assert_eq!(rt.snapshot().high_score, peak_score);
    for _ in 0..6 {
        rt.apply_input(GameInput::MoveRight);
        rt.apply_input(GameInput::MoveLeft);
    }
    assert_eq!(rt.snapshot().status, GameStatus::GameOver);
}

#[test]
fn grid_puzzle_maze_and_sokoban_modes() {
    // Maze mode: -1 wall, 0 floor, 2 player, 3 goal
    let maze_payload = payload_with(
        GameSpec::GridPuzzle {
            rows: 3,
            cols: 4,
            levels: vec![GridPuzzleLevel {
                level_id: "maze_1".to_string(),
                title: "Corridor".to_string(),
                initial_grid: vec![
                    vec![-1, -1, -1, -1],
                    vec![-1, 2, 0, 3],
                    vec![-1, -1, -1, -1],
                ],
                target_grid: None,
                max_moves: Some(5),
                hint: None,
            }],
        },
        vec![],
    );
    let mut maze = MiniGameRuntime::from_payload(&maze_payload, 7);
    maze.apply_input(GameInput::MoveUp); // blocked by wall
    match maze.snapshot().state {
        GameRuntimeState::GridPuzzle {
            mode,
            moves_used,
            player_pos,
            ..
        } => {
            assert_eq!(mode, GridPuzzleMode::MazeSokoban);
            assert_eq!(moves_used, 0);
            assert_eq!(player_pos, Some((1, 1)));
        }
        other => panic!("unexpected {other:?}"),
    }
    maze.apply_input(GameInput::MoveRight);
    maze.apply_input(GameInput::MoveRight);
    assert_eq!(maze.snapshot().status, GameStatus::Won);

    // Sokoban mode: -1 wall, 2 player, 4 crate, 5 target -> 6 crate-on-target
    let soko_payload = payload_with(
        GameSpec::GridPuzzle {
            rows: 3,
            cols: 5,
            levels: vec![GridPuzzleLevel {
                level_id: "soko_1".to_string(),
                title: "Push Box".to_string(),
                initial_grid: vec![
                    vec![-1, -1, -1, -1, -1],
                    vec![-1, 2, 4, 5, -1],
                    vec![-1, -1, -1, -1, -1],
                ],
                target_grid: None,
                max_moves: Some(3),
                hint: None,
            }],
        },
        vec![],
    );
    let mut soko = MiniGameRuntime::from_payload(&soko_payload, 9);
    soko.apply_input(GameInput::MoveRight);
    let snap = soko.snapshot();
    assert_eq!(snap.status, GameStatus::Won);
    match snap.state {
        GameRuntimeState::GridPuzzle { grid, player_pos, .. } => {
            assert_eq!(player_pos, Some((1, 2)));
            assert_eq!(grid[1][3], 6);
        }
        other => panic!("unexpected {other:?}"),
    }
}

#[test]
fn trivia_quiz_combo_explanation_confirm_and_timeout() {
    let payload = payload_with(
        GameSpec::TriviaQuiz {
            time_limit_secs: Some(30),
            questions: vec![
                TriviaQuestion {
                    question_id: "q1".to_string(),
                    prompt: "What planet is red?".to_string(),
                    choices: vec!["Venus".into(), "Mars".into(), "Jupiter".into()],
                    correct_index: 1,
                    explanation: Some("Iron oxide gives Mars its red hue.".into()),
                    points: 100,
                },
                TriviaQuestion {
                    question_id: "q2".to_string(),
                    prompt: "2 + 2 = ?".to_string(),
                    choices: vec!["3".into(), "4".into()],
                    correct_index: 1,
                    explanation: Some("Basic arithmetic.".into()),
                    points: 100,
                },
            ],
        },
        vec![ach("quiz_ace", Some(200))],
    );
    let mut rt = MiniGameRuntime::from_payload(&payload, 1);
    rt.apply_input(GameInput::SelectOption(1));
    let s1 = rt.snapshot();
    assert_eq!(s1.score, 100);
    assert_eq!(s1.combo, 1);
    match &s1.state {
        GameRuntimeState::TriviaQuiz {
            awaiting_next,
            explanation,
            last_answer_correct,
            ..
        } => {
            assert!(*awaiting_next);
            assert_eq!(*last_answer_correct, Some(true));
            assert_eq!(explanation.as_deref(), Some("Iron oxide gives Mars its red hue."));
        }
        other => panic!("unexpected {other:?}"),
    }

    rt.apply_input(GameInput::Confirm);
    rt.apply_input(GameInput::SelectOption(1));
    let s2 = rt.snapshot();
    assert_eq!(s2.score, 225); // 100 + 100 + 25 streak bonus
    assert_eq!(s2.combo, 2);
    assert!(s2.unlocked_achievements.contains(&"quiz_ace".to_string()));
    rt.apply_input(GameInput::Confirm);
    assert_eq!(rt.snapshot().status, GameStatus::Won);

    // Timeout test
    rt.restart();
    rt.apply_input(GameInput::Start);
    rt.step(15_000);
    assert_eq!(rt.snapshot().status, GameStatus::Playing);
    rt.step(15_000);
    assert_eq!(rt.snapshot().status, GameStatus::GameOver);

    // Defensive time_limit_secs = Some(0) must NOT cause immediate GameOver
    let zero_timer = payload_with(
        GameSpec::TriviaQuiz {
            time_limit_secs: Some(0),
            questions: vec![TriviaQuestion {
                question_id: "q0".to_string(),
                prompt: "Alive?".to_string(),
                choices: vec!["Yes".into(), "No".into()],
                correct_index: 0,
                explanation: None,
                points: 50,
            }],
        },
        vec![],
    );
    let mut zrt = MiniGameRuntime::from_payload(&zero_timer, 1);
    zrt.apply_input(GameInput::Start);
    zrt.step(5_000);
    assert_eq!(zrt.snapshot().status, GameStatus::Playing);
}

#[test]
fn interactive_fiction_branching_trimmed_ids_dead_end_and_restart() {
    let payload = payload_with(
        GameSpec::InteractiveFiction {
            start_node_id: "  start  ".to_string(),
            nodes: vec![
                StoryNode {
                    node_id: "start".to_string(),
                    title: "Airlock".to_string(),
                    body: "Choose your path.".to_string(),
                    choices: vec![
                        StoryChoice {
                            label: "Bridge".to_string(),
                            target_node_id: " end_good ".to_string(),
                            score_delta: 50,
                        },
                        StoryChoice {
                            label: "Broken Vent".to_string(),
                            target_node_id: "missing_node".to_string(),
                            score_delta: -10,
                        },
                    ],
                    is_ending: false,
                    ending_badge: None,
                },
                StoryNode {
                    node_id: "end_good".to_string(),
                    title: "Command".to_string(),
                    body: "You saved the ship!".to_string(),
                    choices: vec![],
                    is_ending: true,
                    ending_badge: Some("Captain".to_string()),
                },
            ],
        },
        vec![ach("captain", None)],
    );
    let mut rt = MiniGameRuntime::from_payload(&payload, 10);
    rt.apply_input(GameInput::SelectOption(0));
    let won = rt.snapshot();
    assert_eq!(won.status, GameStatus::Won);
    assert_eq!(won.score, 50);
    assert_eq!(won.high_score, 50);
    assert!(won.unlocked_achievements.contains(&"captain".to_string()));
    match &won.state {
        GameRuntimeState::InteractiveFiction {
            current_node_id,
            visited_node_ids,
            is_ending,
            ending_badge,
            ..
        } => {
            assert_eq!(current_node_id, "end_good");
            assert_eq!(visited_node_ids, &vec!["start".to_string(), "end_good".to_string()]);
            assert!(*is_ending);
            assert_eq!(ending_badge.as_deref(), Some("Captain"));
        }
        other => panic!("unexpected {other:?}"),
    }

    // Restart preserves high_score, then follow dangling target -> GameOver
    rt.restart();
    assert_eq!(rt.snapshot().high_score, 50);
    rt.apply_input(GameInput::SelectOption(1));
    assert_eq!(rt.snapshot().status, GameStatus::GameOver);
}

#[test]
fn arcade_dodge_determinism_movement_pause_resume_and_progression() {
    let payload = payload_with(
        GameSpec::ArcadeDodge {
            tick_ms: 16,
            arena_width: 480,
            arena_height: 360,
            player_speed: 240.0,
            obstacle_speed: 180.0,
            lives: 3,
            target_score: 60,
            theme_accent_hex: Some("#F59E0B".to_string()),
        },
        vec![ach("survivor", Some(30))],
    );

    let mut a = MiniGameRuntime::from_payload(&payload, 12345);
    let mut b = MiniGameRuntime::from_payload(&payload, 12345);
    let mut c = MiniGameRuntime::from_payload(&payload, 99999);

    for rt in [&mut a, &mut b, &mut c] {
        rt.apply_input(GameInput::Start);
        rt.apply_input(GameInput::MoveLeft);
        rt.step(1600);
        rt.apply_input(GameInput::MoveRight);
        rt.step(1600);
    }
    assert_eq!(a.snapshot(), b.snapshot(), "same seed must produce identical state");
    assert_ne!(a.snapshot().state, c.snapshot().state, "different seeds must diverge");

    // Pause freezes simulation; resume continues
    a.pause();
    let paused_snap = a.snapshot();
    assert_eq!(paused_snap.status, GameStatus::Paused);
    a.step(2000);
    assert_eq!(a.snapshot().elapsed_ms, paused_snap.elapsed_ms);
    a.resume();
    assert_eq!(a.snapshot().status, GameStatus::Playing);

    // Step until game finishes (Won or GameOver) and verify restart preserves high_score
    let mut steps = 0;
    while a.snapshot().status == GameStatus::Playing && steps < 40 {
        a.apply_input(if steps % 2 == 0 {
            GameInput::MoveLeft
        } else {
            GameInput::MoveRight
        });
        a.step(1000);
        steps += 1;
    }
    let ended = a.snapshot();
    assert!(matches!(ended.status, GameStatus::Won | GameStatus::GameOver));
    let best = ended.high_score;
    assert!(best > 0);
    a.restart();
    assert_eq!(a.snapshot().high_score, best);
    assert_eq!(a.snapshot().lives, 3);
}

#[test]
fn snake_grid_wrap_vs_wall_hit_180_reversal_tail_vacate_and_win() {
    // 1. Non-wrapping wall hit + 180-degree reversal guard
    let wall_payload = payload_with(
        GameSpec::SnakeGrid {
            grid_width: 7,
            grid_height: 7,
            tick_ms: 100,
            wrap_walls: false,
            target_score: 20,
        },
        vec![],
    );
    let mut wall_rt = MiniGameRuntime::from_payload(&wall_payload, 77);
    wall_rt.apply_input(GameInput::Start);
    // Heading Right: MoveLeft must be ignored (no instant 180-degree self-collision)
    wall_rt.apply_input(GameInput::MoveLeft);
    wall_rt.step(100);
    assert_eq!(wall_rt.snapshot().status, GameStatus::Playing);
    match wall_rt.snapshot().state {
        GameRuntimeState::SnakeGrid { direction, .. } => {
            assert_eq!(direction, SnakeDirection::Right);
        }
        other => panic!("unexpected {other:?}"),
    }
    // Continue straight into right wall -> GameOver
    wall_rt.step(1000);
    assert_eq!(wall_rt.snapshot().status, GameStatus::GameOver);

    // 2. Wrapping walls + steering directly onto food -> Won (and tail-vacating square turn)
    let wrap_payload = payload_with(
        GameSpec::SnakeGrid {
            grid_width: 6,
            grid_height: 6,
            tick_ms: 50,
            wrap_walls: true,
            target_score: 20,
        },
        vec![ach("snake_win", None)],
    );
    let mut rt = MiniGameRuntime::from_payload(&wrap_payload, 42);
    rt.apply_input(GameInput::Start);
    // Verify wrap-around works across right border
    rt.step(50 * 8);
    assert_eq!(rt.snapshot().status, GameStatus::Playing);

    // Guide snake cleanly toward food step-by-step until Won
    let mut guard = 0;
    while rt.snapshot().status == GameStatus::Playing && guard < 120 {
        guard += 1;
        let (head, dir, food) = match rt.snapshot().state {
            GameRuntimeState::SnakeGrid {
                segments,
                direction,
                food,
                ..
            } => (segments[0], direction, food),
            other => panic!("unexpected {other:?}"),
        };
        if let Some(GridCoord { x: fx, y: fy }) = food {
            let desired = if head.x != fx {
                if head.x < fx {
                    SnakeDirection::Right
                } else {
                    SnakeDirection::Left
                }
            } else if head.y < fy {
                SnakeDirection::Down
            } else {
                SnakeDirection::Up
            };
            let input = match (dir, desired) {
                (SnakeDirection::Right, SnakeDirection::Left)
                | (SnakeDirection::Left, SnakeDirection::Right) => GameInput::MoveDown,
                (SnakeDirection::Up, SnakeDirection::Down)
                | (SnakeDirection::Down, SnakeDirection::Up) => GameInput::MoveRight,
                (_, SnakeDirection::Up) => GameInput::MoveUp,
                (_, SnakeDirection::Down) => GameInput::MoveDown,
                (_, SnakeDirection::Left) => GameInput::MoveLeft,
                (_, SnakeDirection::Right) => GameInput::MoveRight,
            };
            rt.apply_input(input);
        }
        rt.step(50);
    }
    let won = rt.snapshot();
    assert_eq!(won.status, GameStatus::Won);
    assert!(won.score >= 20);
    assert!(won.unlocked_achievements.contains(&"snake_win".to_string()));
}

#[test]
fn card_match_deterministic_shuffle_timed_mismatch_click_and_select_and_timeout() {
    let pairs = vec![
        CardPair {
            pair_id: "sun".into(),
            label: "Sun".into(),
            symbol: "☀️".into(),
            accent_hex: Some("#F59E0B".into()),
        },
        CardPair {
            pair_id: "moon".into(),
            label: "Moon".into(),
            symbol: "🌙".into(),
            accent_hex: Some("#6366F1".into()),
        },
        CardPair {
            pair_id: "star".into(),
            label: "Star".into(),
            symbol: "⭐️".into(),
            accent_hex: None,
        },
        CardPair {
            pair_id: "comet".into(),
            label: "Comet".into(),
            symbol: "☄️".into(),
            accent_hex: None,
        },
    ];
    let payload = payload_with(
        GameSpec::CardMatch {
            pairs,
            time_limit_secs: Some(60),
        },
        vec![ach("memory_master", None)],
    );

    let mut rt = MiniGameRuntime::from_payload(&payload, 2026);
    let rt_same = MiniGameRuntime::from_payload(&payload, 2026);
    let rt_diff = MiniGameRuntime::from_payload(&payload, 9001);
    assert_eq!(rt.snapshot(), rt_same.snapshot());
    assert_ne!(rt.snapshot().state, rt_diff.snapshot().state);

    let cards = match rt.snapshot().state {
        GameRuntimeState::CardMatch { cards, .. } => cards,
        other => panic!("unexpected {other:?}"),
    };
    assert_eq!(cards.len(), 8);

    // Find a mismatch pair and flip via ClickCell
    let c0 = &cards[0];
    let mismatch_card = cards.iter().find(|c| c.pair_id != c0.pair_id).unwrap();
    rt.apply_input(GameInput::ClickCell { row: c0.row, col: c0.col });
    rt.apply_input(GameInput::ClickCell {
        row: mismatch_card.row,
        col: mismatch_card.col,
    });
    rt.step(300);
    match rt.snapshot().state {
        GameRuntimeState::CardMatch {
            cards: cur,
            mismatch_timer_ms,
            ..
        } => {
            assert_eq!(mismatch_timer_ms, 400);
            assert_eq!(cur[c0.index].face, CardFaceState::FaceUp);
            assert_eq!(cur[mismatch_card.index].face, CardFaceState::FaceUp);
        }
        other => panic!("unexpected {other:?}"),
    }
    rt.step(400);
    match rt.snapshot().state {
        GameRuntimeState::CardMatch {
            cards: cur,
            mismatch_timer_ms,
            ..
        } => {
            assert_eq!(mismatch_timer_ms, 0);
            assert_eq!(cur[c0.index].face, CardFaceState::FaceDown);
            assert_eq!(cur[mismatch_card.index].face, CardFaceState::FaceDown);
        }
        other => panic!("unexpected {other:?}"),
    }

    // Match all 4 pairs using SelectOption
    for pid in ["sun", "moon", "star", "comet"] {
        let idxs: Vec<usize> = cards
            .iter()
            .filter(|c| c.pair_id == pid)
            .map(|c| c.index)
            .collect();
        rt.apply_input(GameInput::SelectOption(idxs[0]));
        rt.apply_input(GameInput::SelectOption(idxs[1]));
    }
    let won = rt.snapshot();
    assert_eq!(won.status, GameStatus::Won);
    assert!(won.score >= 400);
    assert!(won.unlocked_achievements.contains(&"memory_master".to_string()));

    // Timeout test after restart
    rt.restart();
    rt.apply_input(GameInput::Start);
    rt.step(60_000);
    assert_eq!(rt.snapshot().status, GameStatus::GameOver);
}

#[test]
fn mini_app_html_and_degenerate_drafts_and_u64_max_step_never_panic() {
    let mini_payload = payload_with(
        GameSpec::MiniAppHtml {
            bot_username: "ArcadeBot".to_string(),
            short_name: "star_racer".to_string(),
            html_bundle_media_id: Some("media_zip_1".to_string()),
        },
        vec![],
    );
    let mut mini = MiniGameRuntime::from_payload(&mini_payload, 1);
    mini.apply_input(GameInput::PrimaryAction);
    mini.step(u64::MAX);
    let mini_snap = mini.snapshot();
    assert_eq!(mini_snap.engine_kind, GameEngineKind::MiniAppHtml);
    assert_eq!(
        mini_snap.state,
        GameRuntimeState::MiniAppLaunch {
            bot_username: "ArcadeBot".to_string(),
            short_name: "star_racer".to_string(),
            html_bundle_media_id: Some("media_zip_1".to_string()),
        }
    );

    let degenerate_specs = vec![
        GameSpec::GridPuzzle {
            rows: 0,
            cols: 0,
            levels: vec![],
        },
        GameSpec::GridPuzzle {
            rows: 1,
            cols: 1,
            levels: vec![GridPuzzleLevel {
                level_id: "".into(),
                title: "".into(),
                initial_grid: vec![vec![1], vec![2, 3, 0]],
                target_grid: None,
                max_moves: Some(0),
                hint: None,
            }],
        },
        GameSpec::TriviaQuiz {
            time_limit_secs: None,
            questions: vec![],
        },
        GameSpec::InteractiveFiction {
            start_node_id: "nonexistent".into(),
            nodes: vec![],
        },
        GameSpec::ArcadeDodge {
            tick_ms: 0,
            arena_width: 0,
            arena_height: 0,
            player_speed: f32::NAN,
            obstacle_speed: -5.0,
            lives: 0,
            target_score: 0,
            theme_accent_hex: None,
        },
        GameSpec::SnakeGrid {
            grid_width: 0,
            grid_height: 0,
            tick_ms: 0,
            wrap_walls: false,
            target_score: 0,
        },
        GameSpec::CardMatch {
            pairs: vec![],
            time_limit_secs: Some(0),
        },
    ];

    for spec in degenerate_specs {
        let p = payload_with(spec, vec![]);
        let mut rt = MiniGameRuntime::from_payload(&p, 0);
        rt.apply_input(GameInput::Start);
        rt.apply_input(GameInput::MoveUp);
        rt.apply_input(GameInput::SelectOption(99));
        rt.apply_input(GameInput::ClickCell { row: 99, col: 99 });
        rt.step(u64::MAX);
        let _ = rt.snapshot();
        rt.restart();
    }
}

#[test]
fn snake_tail_vacate_and_arcade_hazard_invulnerability_and_lives_depletion() {
    // 1. Explicit SnakeGrid 4-segment tail-vacate loop test
    let snake_payload = payload_with(
        GameSpec::SnakeGrid {
            grid_width: 6,
            grid_height: 6,
            tick_ms: 50,
            wrap_walls: true,
            target_score: 100,
        },
        vec![],
    );
    let mut s = MiniGameRuntime::from_payload(&snake_payload, 42);
    s.apply_input(GameInput::Start);
    // Eat 1 food so snake length reaches 4
    let mut guard = 0;
    while guard < 80 {
        guard += 1;
        let (head, dir, food, len) = match s.snapshot().state {
            GameRuntimeState::SnakeGrid {
                segments,
                direction,
                food,
                ..
            } => (segments[0], direction, food, segments.len()),
            other => panic!("unexpected {other:?}"),
        };
        if len >= 4 {
            break;
        }
        if let Some(GridCoord { x: fx, y: fy }) = food {
            let desired = if head.x != fx {
                if head.x < fx { SnakeDirection::Right } else { SnakeDirection::Left }
            } else if head.y < fy { SnakeDirection::Down } else { SnakeDirection::Up };
            let input = match (dir, desired) {
                (SnakeDirection::Right, SnakeDirection::Left)
                | (SnakeDirection::Left, SnakeDirection::Right) => GameInput::MoveDown,
                (SnakeDirection::Up, SnakeDirection::Down)
                | (SnakeDirection::Down, SnakeDirection::Up) => GameInput::MoveRight,
                (_, SnakeDirection::Up) => GameInput::MoveUp,
                (_, SnakeDirection::Down) => GameInput::MoveDown,
                (_, SnakeDirection::Left) => GameInput::MoveLeft,
                (_, SnakeDirection::Right) => GameInput::MoveRight,
            };
            s.apply_input(input);
        }
        s.step(50);
    }
    let len_before = match s.snapshot().state {
        GameRuntimeState::SnakeGrid { segments, .. } => segments.len(),
        _ => 0,
    };
    assert!(len_before >= 4, "snake should have grown to >= 4 segments");
    // Execute a tight 2x2 square loop (turns directly into tail's vacated cell if len == 4)
    let turns = match s.snapshot().state {
        GameRuntimeState::SnakeGrid { direction: SnakeDirection::Right, .. } => [GameInput::MoveDown, GameInput::MoveLeft, GameInput::MoveUp, GameInput::MoveRight],
        GameRuntimeState::SnakeGrid { direction: SnakeDirection::Left, .. } => [GameInput::MoveUp, GameInput::MoveRight, GameInput::MoveDown, GameInput::MoveLeft],
        GameRuntimeState::SnakeGrid { direction: SnakeDirection::Up, .. } => [GameInput::MoveRight, GameInput::MoveDown, GameInput::MoveLeft, GameInput::MoveUp],
        GameRuntimeState::SnakeGrid { direction: SnakeDirection::Down, .. } => [GameInput::MoveLeft, GameInput::MoveUp, GameInput::MoveRight, GameInput::MoveDown],
        _ => unreachable!(),
    };
    for turn in turns {
        s.apply_input(turn);
        s.step(50);
    }
    assert_eq!(s.snapshot().status, GameStatus::Playing);

    // 2. Explicit ArcadeDodge narrow-arena hazard hit -> invulnerability -> GameOver
    let dodge_payload = payload_with(
        GameSpec::ArcadeDodge {
            tick_ms: 20,
            arena_width: 100,
            arena_height: 100,
            player_speed: 120.0,
            obstacle_speed: 200.0,
            lives: 2,
            target_score: 5_000,
            theme_accent_hex: None,
        },
        vec![],
    );
    let mut d = MiniGameRuntime::from_payload(&dodge_payload, 5);
    d.apply_input(GameInput::Start);
    let mut saw_invuln = false;
    for _ in 0..400 {
        d.step(100);
        let snap = d.snapshot();
        if let GameRuntimeState::ArcadeDodge { invulnerable_ms, .. } = snap.state {
            if invulnerable_ms > 0 && snap.lives == 1 {
                saw_invuln = true;
            }
        }
        if snap.status == GameStatus::GameOver {
            break;
        }
    }
    assert!(saw_invuln, "expected hazard collision to decrement lives and grant invulnerability");
    assert_eq!(d.snapshot().status, GameStatus::GameOver);
    assert_eq!(d.snapshot().lives, 0);
}

#[test]
fn adversarial_extreme_dimensions_speeds_unicode_cjk_and_exact_restart_parity() {
    // 1. Extreme SnakeGrid u16::MAX must clamp cleanly (no 65535*65535 OOM)
    let extreme_snake = payload_with(
        GameSpec::SnakeGrid {
            grid_width: u16::MAX,
            grid_height: u16::MAX,
            tick_ms: u32::MAX,
            wrap_walls: true,
            target_score: u32::MAX,
        },
        vec![],
    );
    let mut s_rt = MiniGameRuntime::from_payload(&extreme_snake, 777);
    s_rt.apply_input(GameInput::Start);
    s_rt.step(u64::MAX);
    match s_rt.snapshot().state {
        GameRuntimeState::SnakeGrid {
            grid_width,
            grid_height,
            tick_ms,
            food,
            ..
        } => {
            assert!(grid_width <= 256 && grid_height <= 256);
            assert!(tick_ms <= 5_000);
            assert!(food.is_some());
        }
        other => panic!("unexpected {other:?}"),
    }

    // 2. Extreme ArcadeDodge f32::MAX / f32::INFINITY / u32::MAX must yield strictly finite floats
    let extreme_arcade = payload_with(
        GameSpec::ArcadeDodge {
            tick_ms: u32::MAX,
            arena_width: u32::MAX,
            arena_height: u32::MAX,
            player_speed: f32::MAX,
            obstacle_speed: f32::INFINITY,
            lives: 3,
            target_score: 1000,
            theme_accent_hex: Some("#007AFF".to_string()),
        },
        vec![],
    );
    let mut a_rt = MiniGameRuntime::from_payload(&extreme_arcade, 888);
    a_rt.apply_input(GameInput::Start);
    a_rt.apply_input(GameInput::MoveLeft);
    a_rt.step(5_000);
    match a_rt.snapshot().state {
        GameRuntimeState::ArcadeDodge {
            player_x,
            player_y,
            entities,
            ..
        } => {
            assert!(player_x.is_finite() && player_y.is_finite());
            for e in entities {
                assert!(e.x.is_finite() && e.y.is_finite() && e.vx.is_finite() && e.vy.is_finite());
            }
        }
        other => panic!("unexpected {other:?}"),
    }

    // 3. Unicode / CJK round-trip + exact restart initial-state parity + deduplicated achievements
    let cjk_trivia = payload_with(
        GameSpec::TriviaQuiz {
            time_limit_secs: Some(45),
            questions: vec![
                TriviaQuestion {
                    question_id: "中文题_1".to_string(),
                    prompt: "《三体》的作者是谁？🚀".to_string(),
                    choices: vec!["刘慈欣".into(), "阿西莫夫".into(), "克拉克".into()],
                    correct_index: 0,
                    explanation: Some("刘慈欣创作了《三体》三部曲。".into()),
                    points: 100,
                },
                TriviaQuestion {
                    question_id: "empty_draft_q".to_string(),
                    prompt: "草稿占位题".to_string(),
                    choices: vec![],
                    correct_index: 0,
                    explanation: None,
                    points: 50,
                },
            ],
        },
        vec![ach("中文成就_100", Some(100)), ach("中文通关", None)],
    );
    let mut t_rt = MiniGameRuntime::from_payload(&cjk_trivia, 2026);
    let initial = t_rt.snapshot();
    t_rt.apply_input(GameInput::SelectOption(0));
    t_rt.apply_input(GameInput::Confirm);
    // Malformed empty-choices draft question must advance cleanly on Confirm without deadlock
    t_rt.apply_input(GameInput::Confirm);
    let finished = t_rt.snapshot();
    assert_eq!(finished.status, GameStatus::Won);
    assert_eq!(finished.score, 100);
    assert_eq!(finished.unlocked_achievements.len(), 2);

    // Restart resets gameplay state identically to initial while preserving high_score & deduplicating achievements
    t_rt.restart();
    let restarted = t_rt.snapshot();
    assert_eq!(restarted.state, initial.state);
    assert_eq!(restarted.save_state_json, initial.save_state_json);
    assert_eq!(restarted.score, 0);
    assert_eq!(restarted.combo, 0);
    assert_eq!(restarted.elapsed_ms, 0);
    assert_eq!(restarted.high_score, 100);
    assert!(restarted.newly_unlocked_achievements.is_empty());
    // Re-winning must NOT duplicate unlocked_achievements
    t_rt.apply_input(GameInput::SelectOption(0));
    t_rt.apply_input(GameInput::Confirm);
    t_rt.apply_input(GameInput::Confirm);
    assert_eq!(t_rt.snapshot().unlocked_achievements.len(), 2);

    // 4. Multi-level GridPuzzle with pre-solved level 0 advances straight to unsolved level 1
    let multi_presolved = payload_with(
        GameSpec::GridPuzzle {
            rows: 2,
            cols: 2,
            levels: vec![
                GridPuzzleLevel {
                    level_id: "already_done".into(),
                    title: "Done".into(),
                    initial_grid: vec![vec![1, 2], vec![3, 0]],
                    target_grid: Some(vec![vec![1, 2], vec![3, 0]]),
                    max_moves: Some(5),
                    hint: None,
                },
                GridPuzzleLevel {
                    level_id: "real_lvl".into(),
                    title: "Solve Me".into(),
                    initial_grid: vec![vec![1, 0], vec![3, 2]],
                    target_grid: Some(vec![vec![1, 2], vec![3, 0]]),
                    max_moves: Some(5),
                    hint: None,
                },
            ],
        },
        vec![],
    );
    let mut g_rt = MiniGameRuntime::from_payload(&multi_presolved, 1);
    let g_init = g_rt.snapshot();
    assert_eq!(g_init.status, GameStatus::Ready);
    assert_eq!(g_init.save_state_json["current_level_id"], "real_lvl");
    g_rt.apply_input(GameInput::MoveUp);
    assert_eq!(g_rt.snapshot().status, GameStatus::Won);
}

#[test]
fn f10_sliding_tile_boundary_underflow_and_click_cell_overflow_regressions() {
    // F10-1: Canonical M1 8-puzzle (blank at row 0, col 2) -> MoveDown must be a safe no-op
    let m1_spec = GameSpec::GridPuzzle {
        rows: 3,
        cols: 3,
        levels: vec![GridPuzzleLevel {
            level_id: "m1_8p".into(),
            title: "M1 8-Puzzle".into(),
            initial_grid: vec![vec![1, 2, 0], vec![4, 5, 3], vec![7, 8, 6]],
            target_grid: Some(vec![vec![1, 2, 3], vec![4, 5, 6], vec![7, 8, 0]]),
            max_moves: Some(12),
            hint: None,
        }],
    };
    let mut rt_m1 = MiniGameRuntime::from_payload(&payload_with(m1_spec, vec![]), 1);
    rt_m1.apply_input(GameInput::MoveDown);
    rt_m1.apply_input(GameInput::MoveLeft); // blank at rightmost col 2 -> safe no-op
    // F10-3: ClickCell with usize::MAX must never overflow
    rt_m1.apply_input(GameInput::ClickCell {
        row: usize::MAX,
        col: usize::MAX,
    });
    rt_m1.apply_input(GameInput::ClickCell {
        row: usize::MAX,
        col: 1,
    });
    match rt_m1.snapshot().state {
        GameRuntimeState::GridPuzzle {
            grid, moves_used, ..
        } => {
            assert_eq!(moves_used, 0);
            assert_eq!(grid, vec![vec![1, 2, 0], vec![4, 5, 3], vec![7, 8, 6]]);
        }
        other => panic!("unexpected {other:?}"),
    }

    // F10-2: Exact @CreatorStudioBot M3 Level 2 (blank at row 1, col 0) -> MoveRight must be safe no-op
    let bot_lvl2 = GameSpec::GridPuzzle {
        rows: 3,
        cols: 3,
        levels: vec![GridPuzzleLevel {
            level_id: "bot_lvl2".into(),
            title: "Bot Lvl 2".into(),
            initial_grid: vec![vec![1, 2, 3], vec![0, 4, 6], vec![7, 5, 8]],
            target_grid: Some(vec![vec![1, 2, 3], vec![4, 5, 6], vec![7, 8, 0]]),
            max_moves: Some(16),
            hint: None,
        }],
    };
    let mut rt_bot = MiniGameRuntime::from_payload(&payload_with(bot_lvl2, vec![]), 1);
    rt_bot.apply_input(GameInput::MoveRight);
    match rt_bot.snapshot().state {
        GameRuntimeState::GridPuzzle {
            grid, moves_used, ..
        } => {
            assert_eq!(moves_used, 0);
            assert_eq!(grid, vec![vec![1, 2, 3], vec![0, 4, 6], vec![7, 5, 8]]);
        }
        other => panic!("unexpected {other:?}"),
    }

    // Top-left blank (0, 0) resists both MoveDown and MoveRight; bottom-right (2, 2) resists MoveUp and MoveLeft
    let corner_spec = GameSpec::GridPuzzle {
        rows: 3,
        cols: 3,
        levels: vec![GridPuzzleLevel {
            level_id: "corner".into(),
            title: "Corner".into(),
            initial_grid: vec![vec![0, 1, 2], vec![3, 4, 5], vec![6, 7, 8]],
            target_grid: Some(vec![vec![1, 2, 3], vec![4, 5, 6], vec![7, 8, 0]]),
            max_moves: Some(10),
            hint: None,
        }],
    };
    let mut rt_corner = MiniGameRuntime::from_payload(&payload_with(corner_spec, vec![]), 1);
    rt_corner.apply_input(GameInput::MoveDown);
    rt_corner.apply_input(GameInput::MoveRight);
    match rt_corner.snapshot().state {
        GameRuntimeState::GridPuzzle { moves_used, .. } => assert_eq!(moves_used, 0),
        other => panic!("unexpected {other:?}"),
    }

    // F10-3: ClickCell { row: usize::MAX, col: usize::MAX } on MazeSokoban and CardMatch
    let maze_spec = GameSpec::GridPuzzle {
        rows: 3,
        cols: 4,
        levels: vec![GridPuzzleLevel {
            level_id: "maze_oob".into(),
            title: "Maze".into(),
            initial_grid: vec![
                vec![-1, -1, -1, -1],
                vec![-1, 2, 0, 3],
                vec![-1, -1, -1, -1],
            ],
            target_grid: None,
            max_moves: Some(5),
            hint: None,
        }],
    };
    let mut rt_maze = MiniGameRuntime::from_payload(&payload_with(maze_spec, vec![]), 2);
    rt_maze.apply_input(GameInput::ClickCell {
        row: usize::MAX,
        col: usize::MAX,
    });
    match rt_maze.snapshot().state {
        GameRuntimeState::GridPuzzle { moves_used, .. } => assert_eq!(moves_used, 0),
        other => panic!("unexpected {other:?}"),
    }

    let card_spec = GameSpec::CardMatch {
        pairs: vec![CardPair {
            pair_id: "p1".into(),
            label: "One".into(),
            symbol: "1".into(),
            accent_hex: None,
        }],
        time_limit_secs: None,
    };
    let mut rt_card = MiniGameRuntime::from_payload(&payload_with(card_spec, vec![]), 3);
    rt_card.apply_input(GameInput::ClickCell {
        row: usize::MAX,
        col: usize::MAX,
    });
    match rt_card.snapshot().state {
        GameRuntimeState::CardMatch { moves, .. } => assert_eq!(moves, 0),
        other => panic!("unexpected {other:?}"),
    }
}

#[test]
fn f10_grid_ragged_clamp_empty_row_no_autowin_max_moves_zero_and_card_match_cap() {
    // F10-4a & F10-4b: Ragged 2,000 empty rows + 1 row of 2,000 ints on BOTH initial_grid & target_grid
    let mut ragged_tail = vec![Vec::<i32>::new(); 2_000];
    ragged_tail.push((0..2_000).collect());
    let mut ragged_head = vec![(0..2_000).collect::<Vec<i32>>()];
    ragged_head.extend(vec![Vec::<i32>::new(); 2_000]);

    for (init, targ) in [
        (ragged_tail.clone(), Some(ragged_tail.clone())),
        (ragged_head.clone(), Some(ragged_head.clone())),
    ] {
        let spec = GameSpec::GridPuzzle {
            rows: 255,
            cols: 255,
            levels: vec![GridPuzzleLevel {
                level_id: "ragged".into(),
                title: "Ragged".into(),
                initial_grid: init,
                target_grid: targ,
                max_moves: Some(10),
                hint: None,
            }],
        };
        let rt = MiniGameRuntime::from_payload(&payload_with(spec, vec![]), 1);
        match rt.snapshot().state {
            GameRuntimeState::GridPuzzle {
                rows,
                cols,
                grid,
                target_grid,
                ..
            } => {
                assert!(rows <= 64 && cols <= 64, "got {rows}x{cols}");
                assert_eq!(grid.len(), rows);
                assert!(grid.iter().all(|r| r.len() == cols));
                if let Some(tg) = target_grid {
                    assert!(tg.len() <= 64, "target rows={}", tg.len());
                    assert!(tg.iter().all(|r| r.len() <= 64));
                }
            }
            other => panic!("unexpected {other:?}"),
        }
    }

    // F10-4c: Malformed initial_grid: vec![vec![]] (and [[0]]) MUST NOT resolve to Won or unlock achievements
    for (init_grid, target_grid) in [
        (vec![vec![]], None),
        (vec![vec![]], Some(vec![vec![]])),
        (vec![vec![0]], None),
    ] {
        let spec = GameSpec::GridPuzzle {
            rows: 3,
            cols: 3,
            levels: vec![GridPuzzleLevel {
                level_id: "malformed_empty".into(),
                title: "Empty".into(),
                initial_grid: init_grid,
                target_grid,
                max_moves: Some(5),
                hint: None,
            }],
        };
        let rt = MiniGameRuntime::from_payload(
            &payload_with(spec, vec![ach("undeserved_win", None)]),
            1,
        );
        let snap = rt.snapshot();
        assert_ne!(snap.status, GameStatus::Won);
        assert!(
            snap.unlocked_achievements.is_empty(),
            "empty draft must never unlock win achievements: {:?}",
            snap.unlocked_achievements
        );
    }

    // F10-5: Unsolved level with max_moves: Some(0) cannot win on Move 1 -> GameOver with score 0
    let zero_budget = GameSpec::GridPuzzle {
        rows: 2,
        cols: 2,
        levels: vec![GridPuzzleLevel {
            level_id: "zero_moves".into(),
            title: "Zero Moves".into(),
            initial_grid: vec![vec![1, 0], vec![3, 2]],
            target_grid: Some(vec![vec![1, 2], vec![3, 0]]),
            max_moves: Some(0),
            hint: None,
        }],
    };
    let mut rt_zero = MiniGameRuntime::from_payload(
        &payload_with(zero_budget, vec![ach("should_not_unlock", None)]),
        1,
    );
    assert_eq!(rt_zero.snapshot().status, GameStatus::GameOver);
    rt_zero.apply_input(GameInput::MoveUp); // would solve the board if allowed
    let snap_zero = rt_zero.snapshot();
    assert_eq!(snap_zero.status, GameStatus::GameOver);
    assert_eq!(snap_zero.score, 0);
    assert!(snap_zero.unlocked_achievements.is_empty());

    // Multi-level: Level 0 solved, Level 1 unsolved with max_moves: Some(0) -> GameOver on Level 1
    let multi_zero = GameSpec::GridPuzzle {
        rows: 2,
        cols: 2,
        levels: vec![
            GridPuzzleLevel {
                level_id: "l0".into(),
                title: "L0".into(),
                initial_grid: vec![vec![1, 0], vec![3, 2]],
                target_grid: Some(vec![vec![1, 2], vec![3, 0]]),
                max_moves: Some(2),
                hint: None,
            },
            GridPuzzleLevel {
                level_id: "l1_zero".into(),
                title: "L1 Zero".into(),
                initial_grid: vec![vec![1, 0], vec![3, 2]],
                target_grid: Some(vec![vec![1, 2], vec![3, 0]]),
                max_moves: Some(0),
                hint: None,
            },
        ],
    };
    let mut rt_mz = MiniGameRuntime::from_payload(&payload_with(multi_zero, vec![]), 1);
    rt_mz.apply_input(GameInput::MoveUp); // solves L0, enters L1 with max_moves=Some(0)
    assert_eq!(rt_mz.snapshot().status, GameStatus::GameOver);

    // Pre-solved level with max_moves: Some(0) uses 0 <= 0 moves and wins cleanly
    let presolved_zero = GameSpec::GridPuzzle {
        rows: 2,
        cols: 2,
        levels: vec![GridPuzzleLevel {
            level_id: "pre_zero".into(),
            title: "Pre-solved".into(),
            initial_grid: vec![vec![1, 2], vec![3, 0]],
            target_grid: Some(vec![vec![1, 2], vec![3, 0]]),
            max_moves: Some(0),
            hint: None,
        }],
    };
    let rt_pz = MiniGameRuntime::from_payload(&payload_with(presolved_zero, vec![]), 1);
    assert_eq!(rt_pz.snapshot().status, GameStatus::Won);

    // F10-6: CardMatch preserves 200 pairs (20x20) and caps oversized 600-pair drafts at 512 pairs
    let make_pairs = |n: usize| -> Vec<CardPair> {
        (0..n)
            .map(|i| CardPair {
                pair_id: format!("pair_{i}"),
                label: format!("Card {i}"),
                symbol: "★".into(),
                accent_hex: None,
            })
            .collect()
    };
    let rt_200 = MiniGameRuntime::from_payload(
        &payload_with(
            GameSpec::CardMatch {
                pairs: make_pairs(200),
                time_limit_secs: None,
            },
            vec![],
        ),
        7,
    );
    match rt_200.snapshot().state {
        GameRuntimeState::CardMatch {
            rows,
            cols,
            cards,
            total_pairs,
            ..
        } => {
            assert_eq!(total_pairs, 200);
            assert_eq!(cards.len(), 400);
            assert_eq!((rows, cols), (20, 20));
        }
        other => panic!("unexpected {other:?}"),
    }

    let rt_600 = MiniGameRuntime::from_payload(
        &payload_with(
            GameSpec::CardMatch {
                pairs: make_pairs(600),
                time_limit_secs: None,
            },
            vec![],
        ),
        7,
    );
    match rt_600.snapshot().state {
        GameRuntimeState::CardMatch {
            cards, total_pairs, ..
        } => {
            assert_eq!(total_pairs, 512);
            assert_eq!(cards.len(), 1_024);
        }
        other => panic!("unexpected {other:?}"),
    }
}


